//! 订阅额度（合同 C3）——`GET /v1/magpie/quotas` 的真实拉取。
//!
//! Go 版（internal/provider 的 quotas.go + balance.go）把额度分三类卡：
//! subscription（订阅账号 discovery 出的窗口）/ plan（按 key 买的套餐）/
//! balance（key 余额）。前两类靠账号 discovery（Claude/Codex 等订阅登录），
//! 是后续账本任务；本文件先落 balance 里唯一不依赖账号的一家：
//! DeepSeek 按 key 余额（`GET /user/balance`）。其它来源一律不出行——
//! Marionette 面板对 windows 空且无 balance 的行直接不显示（见
//! ModelLibraryDialog 的 `windows.length > 0 || balance` 判定），空行是噪音。
//!
//! 红线：额度是展示面——出站失败安静跳过该来源，不 panic、不阻塞别的
//! 路由（本路由在自己的连接线程上跑，单来源 10s 封顶）；store 锁只在
//! 快照 provider 清单时持有，HTTP 一律锁外（持锁等网络会饿死 /v1/models，
//! 2026-10-31 storage 锁事故同款教训）。stale 合并（两侧失败回旧快照并
//! 如实标注）是壳侧 engine.rs 的职责（C3 2026-11-13 批），网关只报当下真数。
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::store::Provider;

/// 行缓存 TTL（Go KeyBalances 同款 1 分钟）：额度面板/状态行反复问，
/// 不能每次都现场打厂商。force（面板点刷新 → 无参直连）跳过。
const CACHE_TTL: Duration = Duration::from_secs(60);

/// 单来源出站超时：一个慢供应商最多拖自己 10s（catalog 抓取同款值）。
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// DeepSeek 主机判定与余额端点（Go balanceSourceOf：chat/responses/
/// anthropic 任一 host 是 api.deepseek.com 即认，端点地址固定、不跟
/// provider 自己配的 base 走）。
const DEEPSEEK_HOST: &str = "api.deepseek.com";
const DEEPSEEK_BALANCE_URL: &str = "https://api.deepseek.com/user/balance";

/// 整表缓存（Go keyBalanceCache 同款包级单例形态；网关进程内只有一个
/// Gateway 实例）。锁 poisoned 当没有缓存用——展示面不值得 panic。
fn cache() -> &'static Mutex<Option<(Instant, Vec<Value>)>> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Vec<Value>)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// 测试缝：把 DeepSeek 端点指到本地假服务（生产路径恒固定地址）。
#[cfg(test)]
static TEST_BALANCE_URL: Mutex<Option<String>> = Mutex::new(None);

/// C3 的行数组（routes.rs 负责包 `{object:"list",data:…}`）。force=true
/// 现场问供应商；false 读 60s 缓存（`?kept=1`）。只有拿到真数据的来源
/// 才出行。
pub fn quota_entries(gateway: &super::Gateway, force: bool) -> Vec<Value> {
    if !force {
        if let Ok(guard) = cache().lock() {
            if let Some((at, rows)) = guard.as_ref() {
                if at.elapsed() < CACHE_TTL {
                    return rows.clone();
                }
            }
        }
    }
    let rows = collect(gateway);
    if let Ok(mut guard) = cache().lock() {
        *guard = Some((Instant::now(), rows.clone()));
    }
    rows
}

fn collect(gateway: &super::Gateway) -> Vec<Value> {
    // 先快照再放锁：绝不让 store 锁等 HTTP。
    let providers: Vec<Provider> = match gateway.store.lock() {
        Ok(store) => store.providers().to_vec(),
        Err(_) => return Vec::new(),
    };
    let mut rows = Vec::new();
    for provider in &providers {
        if provider.key.is_empty() || !is_deepseek(provider) {
            // 余额是 key 的钱：没 key 没得问；非 DeepSeek 的余额/订阅端点
            // 属后续任务，别猜（猜了就是编数）。
            continue;
        }
        if let Some(balance) = deepseek_balance(provider) {
            rows.push(serde_json::json!({
                "provider": provider.id,
                "name": provider.name,
                // key 余额没有账号身份（user 是订阅卡的账号名），不写；
                // windows 恒空——余额卡没有窗口。
                "windows": [],
                "balance": balance,
            }));
        }
    }
    rows
}

/// 三个协议端点任一指向 DeepSeek 即认（Go 按 chat/responses/anthropic
/// 的 host 逐一比对）。
fn is_deepseek(provider: &Provider) -> bool {
    [&provider.chat, &provider.responses, &provider.anthropic]
        .iter()
        .any(|url| url_host(url).eq_ignore_ascii_case(DEEPSEEK_HOST))
}

/// URL 主机名。不是通用解析器——只拿来和已知主机比对：剥 scheme /
/// userinfo / 路径，IPv6 字面量按 `]` 截（端口在外面）。
fn url_host(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if authority.starts_with('[') {
        return authority.split(']').next().unwrap_or(authority);
    }
    authority.split(':').next().unwrap_or(authority)
}

fn balance_url() -> String {
    #[cfg(test)]
    if let Ok(guard) = TEST_BALANCE_URL.lock() {
        if let Some(url) = guard.as_deref() {
            return url.to_string();
        }
    }
    DEEPSEEK_BALANCE_URL.to_string()
}

/// 问 DeepSeek 按 key 余额；任何失败（网络 / 非 200 / 坏形状 / 一个能读
/// 的金额都没有）返回 None——安静跳过该来源，绝不 panic、不记 key
/// （密钥不进任何字符串）。
fn deepseek_balance(provider: &Provider) -> Option<String> {
    let agent = super::outbound_agent();
    let response = agent
        .get(&balance_url())
        .set("Accept", "application/json")
        .set("Authorization", &format!("Bearer {}", provider.key))
        .timeout(FETCH_TIMEOUT)
        .call()
        .ok()?;
    if response.status() != 200 {
        return None;
    }
    let body = response.into_string().ok()?;
    format_deepseek_balance(&body)
}

/// DeepSeek 应答 → `"USD 1.23"` 形态（多币种 `" · "` 连接，Go 同款分隔）。
/// `total_balance` 字符串/数字两种形态都认（Go number() 同款）；
/// `is_available` 不读——Go 版也不读它（余额 0 也是有效读数，别编语义）。
fn format_deepseek_balance(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let infos = value.get("balance_infos")?.as_array()?;
    let mut parts: Vec<(String, f64)> = Vec::new();
    for info in infos {
        let currency = info
            .get("currency")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_ascii_uppercase();
        // 读不出的币种丢掉，不拖垮整行（Go 对非数字金额 continue）。
        let Some(total) = json_amount(info.get("total_balance")) else {
            continue;
        };
        parts.push((currency, total));
    }
    if parts.is_empty() {
        return None; // Go："no balance in the reply"
    }
    // DeepSeek 一个账号多币种的次序两次读会换（Go 注释实锤）：还有钱的
    // 币种在前、再按币种码排序——相邻两次读数不互换位，面板趋势才稳。
    parts.sort_by(|a, b| match (a.1 > 0.0, b.1 > 0.0) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.0.cmp(&b.0),
    });
    Some(
        parts
            .iter()
            .map(|(code, total)| {
                if code.is_empty() {
                    format!("{:.2}", total)
                } else {
                    format!("{} {:.2}", code, total)
                }
            })
            .collect::<Vec<_>>()
            .join(" · "),
    )
}

/// 金额字段：JSON 数字或数字字符串（Go number() 同款），空白不认。
fn json_amount(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::Gateway;
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// 测试串行锁：配置目录靠 XDG_CONFIG_HOME（进程级 env），并行测试会
    /// 互踩——全局锁串行（council store 沙箱同款做法）。
    // 统一走 mod.rs 的跨模块共享锁（XDG_CONFIG_HOME 是进程级 env）。
    static DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

    /// 一次测试的世界：%TEMP% 下唯一配置目录 + 串行锁，Drop 清目录。
    /// 绝不碰真实 `~/.config/magpie`（config_dir 只认绝对路径的
    /// XDG_CONFIG_HOME）。env 只在 Store::load 的一瞬持有——见 sandbox()。
    struct Sandbox {
        dir: std::path::PathBuf,
        lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn sandbox(providers_json: &str) -> (Gateway, Sandbox) {
        // 先拿串行锁再动共享状态（缓存/端点覆盖/env），测试间零耦合。
        let lock = crate::engine_gateway::TEST_SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        if let Ok(mut guard) = cache().lock() {
            *guard = None;
        }
        if let Ok(mut guard) = TEST_BALANCE_URL.lock() {
            *guard = None;
        }
        let dir = std::env::temp_dir().join(format!(
            "marionette-engine-quotas-{}-{}",
            std::process::id(),
            DIR_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        // config_dir() = XDG_CONFIG_HOME/magpie（不是 XDG 根本身）。
        std::fs::create_dir_all(dir.join("magpie")).expect("temp config dir");
        std::fs::write(dir.join("magpie").join("providers.json"), providers_json)
            .expect("providers.json");
        // XDG_CONFIG_HOME 是进程级 env，本测试箱里还有别的沙箱模块
        // （presets）在抢：quotas 打开 Gateway 后全走内存 store，不再读
        // config_dir——所以设完立刻恢复，把互相踩的窗口压到一次文件读。
        let prior_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let gateway = Gateway::open().expect("gateway");
        match &prior_xdg {
            Some(prior) => std::env::set_var("XDG_CONFIG_HOME", prior),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        (gateway, Sandbox { dir, lock })
    }

    /// 本地假厂商服务（std TcpListener 手写 HTTP，零新依赖）：每个连接
    /// 计一击并抓 Authorization，回固定 JSON。hits 就是「真打了网络」的
    /// 计数器——缓存命中不该产生新连接。
    struct FakeVendor {
        url: String,
        hits: Arc<AtomicUsize>,
        authorizations: Arc<Mutex<Vec<String>>>,
    }

    fn fake_vendor(body: &'static str) -> FakeVendor {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake vendor");
        let port = listener.local_addr().expect("port").port();
        let hits = Arc::new(AtomicUsize::new(0));
        let authorizations = Arc::new(Mutex::new(Vec::new()));
        let thread_hits = hits.clone();
        let thread_auth = authorizations.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                thread_hits.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(match stream.try_clone() {
                    Ok(s) => s,
                    Err(_) => continue,
                });
                // GET 无请求体：读到空行 = 头结束。顺手记下 Authorization。
                let mut authorization = String::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() {
                        break;
                    }
                    if let Some(value) = trimmed.strip_prefix("Authorization:") {
                        authorization = value.trim().to_string();
                    }
                }
                thread_auth
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(authorization);
                let mut stream = stream;
                // Connection: close + 精确 Content-Length：ureq 读完即弃，
                // 不会把连接丢回池里复用——一请求一连接，计数器才准。
                let reply = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(reply.as_bytes());
                let _ = stream.flush();
            }
        });
        FakeVendor {
            url: format!("http://127.0.0.1:{port}/user/balance"),
            hits,
            authorizations,
        }
    }

    const PROVIDERS: &str = r#"{
        "providers": [
            { "id": "deepseek", "name": "DeepSeek", "key": "sk-test", "chat": "https://api.deepseek.com/v1" },
            { "id": "openai", "name": "OpenAI", "key": "sk-other", "chat": "https://api.openai.com/v1" },
            { "id": "nokey", "name": "DeepSeek NoKey", "chat": "https://api.deepseek.com/v1" }
        ]
    }"#;

    #[test]
    fn deepseek_balance_row_shape_and_cache() {
        let (gateway, _sandbox) = sandbox(PROVIDERS);
        // CNY 字符串金额 + USD 数字金额（两种形态都认）+ 一个没钱币种
        // （排最后：Go 定序「有钱在前、再按币种码」，两次读不互换位）。
        let body = r#"{"is_available":true,"balance_infos":[
            {"currency":"cny","total_balance":"110.00","granted_balance":"10.00","topped_up_balance":"100.00"},
            {"currency":"USD","total_balance":1.234},
            {"currency":"JPY","total_balance":"0.00"}]}"#;
        let vendor = fake_vendor(body);
        *TEST_BALANCE_URL.lock().unwrap_or_else(|p| p.into_inner()) = Some(vendor.url.clone());

        let rows = quota_entries(&gateway, true);
        assert_eq!(
            rows.len(),
            1,
            "只有 DeepSeek（有 key）出行；openai 无实现、nokey 无 key"
        );
        let row = &rows[0];
        assert_eq!(row["provider"], "deepseek");
        assert_eq!(row["name"], "DeepSeek");
        assert_eq!(row["windows"], serde_json::json!([]));
        assert_eq!(row["balance"], "CNY 110.00 · USD 1.23 · JPY 0.00");
        assert!(row.get("user").is_none(), "key 余额没有账号身份");
        assert_eq!(vendor.hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            vendor.authorizations.lock().unwrap_or_else(|p| p.into_inner())[0],
            "Bearer sk-test",
            "余额是 key 的钱：必须带 Bearer"
        );

        // 60s 缓存（?kept=1 语义）：不打网络——计数器不涨。
        let cached = quota_entries(&gateway, false);
        assert_eq!(cached, rows);
        assert_eq!(vendor.hits.load(Ordering::SeqCst), 1, "缓存命中不该再打网络");

        // force（面板点刷新 / 无参直连）跳过缓存。
        let again = quota_entries(&gateway, true);
        assert_eq!(again, rows);
        assert_eq!(vendor.hits.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn failing_source_is_skipped_quietly() {
        let (gateway, _sandbox) = sandbox(PROVIDERS);
        // 指向必然没人听的端口（先占后放拿一个空闲口）：连接被拒 →
        // 空表，不 panic、不编行。
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("port").port();
        drop(listener);
        *TEST_BALANCE_URL.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(format!("http://127.0.0.1:{port}/user/balance"));
        let rows = quota_entries(&gateway, true);
        assert!(rows.is_empty(), "出站失败安静跳过该来源");
    }

    #[test]
    fn bad_reply_shape_yields_no_row() {
        let (gateway, _sandbox) = sandbox(PROVIDERS);
        let vendor = fake_vendor("not json at all");
        *TEST_BALANCE_URL.lock().unwrap_or_else(|p| p.into_inner()) = Some(vendor.url.clone());
        let rows = quota_entries(&gateway, true);
        assert!(rows.is_empty(), "坏形状 → 空表不 panic（C3 红线）");
        assert_eq!(vendor.hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn amounts_that_do_not_parse_are_dropped() {
        // 非数字金额的币种丢掉（Go number() 不认就 continue）。
        assert_eq!(
            format_deepseek_balance(
                r#"{"balance_infos":[{"currency":"USD","total_balance":"n/a"},{"currency":"CNY","total_balance":7.5}]}"#
            ),
            Some("CNY 7.50".to_string())
        );
        // 一个能读的都没有 → 没行（Go："no balance in the reply"）。
        assert_eq!(
            format_deepseek_balance(
                r#"{"balance_infos":[{"currency":"USD","total_balance":"n/a"}]}"#
            ),
            None
        );
    }
}
