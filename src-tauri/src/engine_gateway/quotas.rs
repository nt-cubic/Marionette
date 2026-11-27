//! 订阅额度（合同 C3）——`GET /v1/magpie/quotas` 的真实拉取。
//!
//! Go 版（internal/provider 的 quotas.go + balance.go）把额度分三类卡：
//! subscription（订阅账号 discovery 出的窗口）/ plan（按 key 买的套餐）/
//! balance（key 余额）。前两类靠账号 discovery（Claude/Codex/Gemini 等订阅
//! 登录），是后续账本任务，**明确不做**；本文件落 balance 类里不依赖账号
//! discovery 的六家（Go `balanceSourceOf` 的 host 表）：DeepSeek、
//! Moonshot/Kimi、OpenRouter、SiliconFlow、StepFun、Zhipu/Z.ai 按量 key。
//! 其它来源一律不出行——Marionette 面板对 windows 空且无 balance 的行
//! 直接不显示（见 ModelLibraryDialog 的 `windows.length > 0 || balance`
//! 判定），空行是噪音。
//!
//! 端点 URL、响应字段、金额符号全部照 Go 版（internal/provider/balance.go）
//! 抄，别猜。Go 版还有 Command Code / ModelScope / Ollama Cloud / AiHubMix
//! 四家 key 余额来源（含自报 BalanceURL 的中继读法），本批未收——见模块
//! 尾部「仍未覆盖」清单的注释。
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

/// DeepSeek 余额端点（Go balance.go:66）。主机判定走 chat/responses/
/// anthropic 任一 host 是 api.deepseek.com（见 balance_source），端点地址
/// 固定、不跟 provider 自己配的 base 走。
const DEEPSEEK_BALANCE_URL: &str = "https://api.deepseek.com/user/balance";

/// Zhipu/Z.ai 的账号报告路径（Go zhipuAccountReport，balance.go:555-556）：
/// 控制台自己读的那个端点，两个 host 同路径，key 作 Bearer。
const ZHIPU_ACCOUNT_REPORT_PATH: &str = "/api/biz/account/query-customer-account-report";

/// Zhipu/Z.ai 按量（pay as you go）的 OpenAI 路径（Go zhipuPaygPath，
/// zhipu_coding.go:31）：base 落在这上面才算按量 key——决定「全零账号」
/// 出不出行（按量端点上 ¥0.00 是真实读数）。
const ZHIPU_PAYG_PATH: &str = "/api/paas/v4";

/// SiliconFlow 余额查询下线后的拒答 code（Go siliconFlowGone，
/// balance.go:499-510：HTTP 410 或信封 code=20092，#1094）。Go 的 Code
/// 字段是 JSON int——字符串 code 解析失败保持 0、不算下线拒答。
const SILICONFLOW_RETIRED_CODE: i64 = 20092;

/// 整表缓存（Go keyBalanceCache 同款包级单例形态；网关进程内只有一个
/// Gateway 实例）。锁 poisoned 当没有缓存用——展示面不值得 panic。
fn cache() -> &'static Mutex<Option<(Instant, Vec<Value>)>> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Vec<Value>)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// 测试缝：按 provider id 把余额端点指到本地假服务（生产路径恒固定地址
/// ——覆盖表只在 cfg(test) 存在）。每家来源的 providers.json 里写真实
/// host 的 base（测 host 判定），端点本身落到假服务（不打真网）。
#[cfg(test)]
static TEST_BALANCE_URLS: Mutex<Option<std::collections::HashMap<String, String>>> =
    Mutex::new(None);

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
    // 账号行与钥匙行并发读：壳侧读缓存那条路只等 4s（engine.rs LOOP_HTTP_TIMEOUT），
    // 串行相加会超时，超时丢的是整张面板而不只是慢的那一行。
    std::thread::scope(|scope| {
        // Codex 订阅（ChatGPT 登录）：账号 discovery 的只读窗口行；没账号/令牌
        // 过期/请求失败安静跳过，用户自建同名来源时让位。
        // 多账号：每个账号一行（user=邮箱）。
        let codex = scope.spawn(|| super::codex_account::quota_rows(&providers));
        // Claude / Grok（SuperGrok）/ Antigravity（Google 登录）：同款只读行，
        // 失败安静跳过。
        let claude = scope.spawn(|| super::claude_account::quota_row(&providers));
        let grok = scope.spawn(|| super::grok_account::quota_row(&providers));
        let antigravity = scope.spawn(|| super::antigravity_account::quota_row(&providers));

        let mut rows = merge_same_key_rows(key_rows(&providers), &providers);
        rows.extend(codex.join().unwrap_or_default());
        rows.extend(claude.join().ok().flatten());
        rows.extend(grok.join().ok().flatten());
        rows.extend(antigravity.join().ok().flatten());
        rows
    })
}

/// 按钥匙读的行：六家余额 + quota_extra 的套餐与余额补全。
fn key_rows(providers: &[Provider]) -> Vec<Value> {
    let mut rows = Vec::new();
    for provider in providers {
        // 余额是 key 的钱：没 key 没得问。
        if provider.key.is_empty() {
            continue;
        }
        // 用户自报了 balanceURL 的来源由 key_balances 接管（Go：BalanceURL 优先于 host 表）。
        if super::key_balances::takes_over(provider) {
            continue;
        }
        let Some((url, kind)) = balance_source(provider) else {
            // 订阅/plan 类（Claude/Codex/Gemini 账号 discovery）明确不做；
            // 没实现的来源别猜（猜了就是编数）。
            continue;
        };
        let url = balance_url(&provider.id, &url);
        let Some(body) = fetch_balance_body(&url, &provider.key) else {
            // 出站失败安静跳过该来源（红线），绝不 panic、不记 key
            // （密钥不进任何字符串）。
            continue;
        };
        if let Some(balance) = format_balance(kind, &body) {
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
    // 补全来源（quota_extra）：ZCode 订阅 / 钥匙套餐（智谱 coding、OpenCode Go、
    // Kimi Code、Ollama…）/ 余额补全（Command Code、ModelScope、Ollama credits、
    // AiHubMix、自报 balanceURL、中继探测）。并发读，失败安静跳过。
    rows.extend(super::quota_extra::collect(providers));
    rows
}

/// 同一把钥匙被登记成多个来源（预设 + 从各 CLI 配置发现的 `source-*`）时只出
/// 一行，其余来源 id 记进 `aliases`，按来源找额度的界面仍然对得上。
///
/// 只并确定是同一份的：钥匙相同 + 厂商主机相同 + 行形状相同（窗口名、余额）。
/// 不比 used / resetsAt：并发读会差一点。带 user 的行（多钥匙来源）不并。
fn merge_same_key_rows(rows: Vec<Value>, providers: &[Provider]) -> Vec<Value> {
    type Identity = (String, String, Vec<String>, String);
    let field = |row: &Value, name: &str| {
        row.get(name)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    let identity = |row: &Value| -> Option<Identity> {
        if !field(row, "user").is_empty() {
            return None;
        }
        let id = field(row, "provider");
        let provider = providers.iter().find(|p| p.id == id && !p.key.is_empty())?;
        let host = [&provider.chat, &provider.responses, &provider.anthropic]
            .into_iter()
            .map(|base| url_host(base).to_ascii_lowercase())
            .find(|host| !host.is_empty())?;
        let windows = row
            .get("windows")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|window| field(window, "name"))
            .collect();
        Some((provider.key.clone(), host, windows, field(row, "balance")))
    };

    let mut groups: Vec<(Option<Identity>, Vec<Value>)> = Vec::new();
    for row in rows {
        let id = identity(&row);
        match groups
            .iter_mut()
            .find(|(seen, _)| id.is_some() && *seen == id)
        {
            Some((_, members)) => members.push(row),
            None => groups.push((id, vec![row])),
        }
    }
    groups
        .into_iter()
        .map(|(_, mut members)| {
            // 预设来源有正经名字（"OpenCode Go"），发现来的只有主机名。
            let keep = members
                .iter()
                .position(|row| !field(row, "provider").starts_with("source-"))
                .unwrap_or(0);
            let mut kept = members.remove(keep);
            let aliases: Vec<String> = members.iter().map(|row| field(row, "provider")).collect();
            if !aliases.is_empty() {
                kept["aliases"] = serde_json::json!(aliases);
            }
            kept
        })
        .collect()
}

/// 这个 provider 是不是 quotas.rs 自己那六家 host 来源（key_balances 据此让位）。
pub(super) fn has_balance_source(provider: &Provider) -> bool {
    balance_source(provider).is_some()
}

/// 一个 key 余额来源的读法。端点固定、不跟 provider 自己配的 base 走
/// （Go balanceSource 的 url 全是常量）。
#[derive(Clone, Copy)]
enum BalanceKind {
    /// DeepSeek：balance_infos 多币种（Go readDeepSeek）。
    DeepSeek,
    /// Moonshot/Kimi：data.available_balance（Go readMoonshot）；
    /// cn 记 ¥、ai 记 $。
    Moonshot { sign: &'static str },
    /// OpenRouter：data.total_credits − data.total_usage，美元
    /// （Go readOpenRouter）。
    OpenRouter,
    /// SiliconFlow：data.totalBalance（查询已下线 → 无行）；
    /// cn 记 ¥、com 记 $。
    SiliconFlow { sign: &'static str },
    /// StepFun：顶层 balance（Go readStepFun）；com 记 ¥、ai 记 $。
    StepFun { sign: &'static str },
    /// Zhipu/Z.ai 按量：data.availableBalance（拒答信封 → 无行；非按量
    /// key 的全零账号不出行，Go readZhipuBalance）。
    Zhipu { sign: &'static str, payg: bool },
}

/// 来源判定（Go balanceSourceOf，balance.go:50-96）：先认 Zhipu 的两个
/// host（端点落在 /api/coding/ 上 = GLM Coding Plan，窗口归那张 plan 卡，
/// 不在这里问余额——Go 只退出 zhipu 判定，不拦后面的 host 表）；再按
/// chat/responses/anthropic 的 host 逐家匹配，先到先得。
fn balance_source(provider: &Provider) -> Option<(String, BalanceKind)> {
    if let Some(zhipu) = zhipu_source(provider) {
        return Some(zhipu);
    }
    for url in [&provider.chat, &provider.responses, &provider.anthropic] {
        // Go hostOf 是小写比对；这里同样把 host 归一成小写再进表。
        match url_host(url).to_ascii_lowercase().as_str() {
            "api.deepseek.com" => {
                return Some((DEEPSEEK_BALANCE_URL.to_string(), BalanceKind::DeepSeek))
            }
            // Go balance.go:67-70：两个 host 各自的端点与货币符号。
            "api.moonshot.cn" => {
                return Some((
                    "https://api.moonshot.cn/v1/users/me/balance".to_string(),
                    BalanceKind::Moonshot { sign: "¥" },
                ))
            }
            "api.moonshot.ai" => {
                return Some((
                    "https://api.moonshot.ai/v1/users/me/balance".to_string(),
                    BalanceKind::Moonshot { sign: "$" },
                ))
            }
            "openrouter.ai" => {
                return Some((
                    "https://openrouter.ai/api/v1/credits".to_string(),
                    BalanceKind::OpenRouter,
                ))
            }
            "api.siliconflow.cn" => {
                return Some((
                    "https://api.siliconflow.cn/v1/user/info".to_string(),
                    BalanceKind::SiliconFlow { sign: "¥" },
                ))
            }
            "api.siliconflow.com" => {
                return Some((
                    "https://api.siliconflow.com/v1/user/info".to_string(),
                    BalanceKind::SiliconFlow { sign: "$" },
                ))
            }
            "api.stepfun.com" => {
                return Some((
                    "https://api.stepfun.com/v1/accounts".to_string(),
                    BalanceKind::StepFun { sign: "¥" },
                ))
            }
            "api.stepfun.ai" => {
                return Some((
                    "https://api.stepfun.ai/v1/accounts".to_string(),
                    BalanceKind::StepFun { sign: "$" },
                ))
            }
            _ => {}
        }
    }
    None
}

/// Zhipu（open.bigmodel.cn）/ Z.ai（api.z.ai）的按量 key 余额来源
/// （Go zhipuBalanceSource，balance.go:531-553）：遍历三个协议端点，
/// host 命中且 base 含 `/api/coding/` → 整个 zhipu 判定作废（Coding
/// Plan 的窗口是那张 plan 卡的事）；否则记 host（后一个覆盖前一个，
/// Go 同款）并累积 payg（base 路径 == /api/paas/v4）。url 固定拼在
/// API 自己的 host 上；api.z.ai 记 $、open.bigmodel.cn 记 ¥。
fn zhipu_source(provider: &Provider) -> Option<(String, BalanceKind)> {
    let mut host = String::new();
    let mut payg = false;
    for base in [&provider.chat, &provider.responses, &provider.anthropic] {
        let h = url_host(base);
        if !matches!(
            h.to_ascii_lowercase().as_str(),
            "open.bigmodel.cn" | "api.z.ai"
        ) {
            continue;
        }
        if base.contains("/api/coding/") {
            // GLM Coding Plan 端点：余额查询与之无关（Go return false，
            // 让位后面的 host 表）。
            return None;
        }
        host = h.to_ascii_lowercase();
        payg = payg || base_path(base.trim().trim_end_matches('/')) == ZHIPU_PAYG_PATH;
    }
    if host.is_empty() {
        return None;
    }
    let sign = if host == "api.z.ai" { "$" } else { "¥" };
    Some((
        format!("https://{}{}", host, ZHIPU_ACCOUNT_REPORT_PATH),
        BalanceKind::Zhipu { sign, payg },
    ))
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

/// base URL 的路径部分（Go basePath，models.go:1239-1247）：剥 scheme 后
/// 从第一个 `/#?` 截到结尾（含 query）。只拿来比对 Zhipu 的按量路径。
fn base_path(raw: &str) -> &str {
    let rest = raw.split_once("://").map(|(_, r)| r).unwrap_or(raw);
    match rest.find(['/', '#', '?']) {
        Some(i) => &rest[i..],
        None => "",
    }
}

/// 余额端点地址：测试覆盖表按 provider id 查（生产恒用固定地址）。
fn balance_url(provider_id: &str, default: &str) -> String {
    if let Some(url) = test_balance_url_override(provider_id) {
        return url;
    }
    default.to_string()
}

#[cfg(test)]
fn test_balance_url_override(provider_id: &str) -> Option<String> {
    TEST_BALANCE_URLS
        .lock()
        .ok()?
        .as_ref()?
        .get(provider_id)
        .cloned()
}

#[cfg(not(test))]
fn test_balance_url_override(_provider_id: &str) -> Option<String> {
    None
}

/// 固定端点 Bearer GET，回 200 的 JSON 体。任何失败（网络 / 非 200 /
/// 读体）返回 None——出站层只认 200，Go 里状态码携带语义的只有
/// SiliconFlow 的 410 下线拒答（安静跳过已覆盖它）。五家的 key 都作
/// `Authorization: Bearer`（Go AuthHeaders(p, Chat) 对 OpenAI 兼容端点
/// 的形态；zhipu 的账号报告明确吃 Bearer）。
fn fetch_balance_body(url: &str, key: &str) -> Option<String> {
    let agent = super::outbound_agent();
    let response = agent
        .get(url)
        .set("Accept", "application/json")
        .set("Authorization", &format!("Bearer {}", key))
        .timeout(FETCH_TIMEOUT)
        .call()
        .ok()?;
    if response.status() != 200 {
        return None;
    }
    response.into_string().ok()
}

fn format_balance(kind: BalanceKind, body: &str) -> Option<String> {
    match kind {
        BalanceKind::DeepSeek => format_deepseek_balance(body),
        BalanceKind::Moonshot { sign } => format_moonshot_balance(body, sign),
        BalanceKind::OpenRouter => format_openrouter_balance(body),
        BalanceKind::SiliconFlow { sign } => format_siliconflow_balance(body, sign),
        BalanceKind::StepFun { sign } => format_stepfun_balance(body, sign),
        BalanceKind::Zhipu { sign, payg } => format_zhipu_balance(body, sign, payg),
    }
}

/// DeepSeek 应答 → `"¥110.00 · $1.23"` 形态（多币种 `" · "` 连接，Go 同款
/// 分隔）。`total_balance` 字符串/数字两种形态都认（Go number() 同款）；
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
            .map(|(code, total)| money(&currency_sign(code), *total))
            .collect::<Vec<_>>()
            .join(" · "),
    )
}

/// Moonshot/Kimi（Go readMoonshot，balance.go:433-450）：
/// `{"code":0,"data":{"available_balance":49.58,…},"status":true}`——只读
/// data.available_balance（Go 版就这一个字段，别的 balance 字段别猜）。
fn format_moonshot_balance(body: &str, sign: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let available = value.get("data")?.get("available_balance");
    Some(money(sign, json_amount(available)?))
}

/// OpenRouter（Go readOpenRouter，balance.go:452-469）：
/// `{"data":{"total_credits":20,"total_usage":3.5}}`，美元——credits 必须
/// 读到，usage 读不出当 0（Go `used, _ := number(...)`），余额 = 差值。
fn format_openrouter_balance(body: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    let data = value.get("data")?;
    let credits = json_amount(data.get("total_credits"))?;
    let used = json_amount(data.get("total_usage")).unwrap_or(0.0);
    Some(money("$", credits - used))
}

/// SiliconFlow（Go readSiliconFlow + siliconFlowGone，balance.go:470-521）：
/// 正常应答 `{"code":20000,"data":{"totalBalance":"88.88",…}}` 读
/// data.totalBalance。查询 2026-08-14 起已下线：HTTP 410（fetch 层已安静
/// 掉）或 200 应答里信封 code=20092 都当「没有余额可读」，不出行——Go
/// 把它单独措辞成 retired 错误，网关口径 = 无行。code 只认 JSON 数字
/// （Go 的 Code int：字符串 "20092" 解析失败保持 0，不算下线）。
fn format_siliconflow_balance(body: &str, sign: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    if value.get("code").and_then(Value::as_i64) == Some(SILICONFLOW_RETIRED_CODE) {
        return None;
    }
    let total = value.get("data")?.get("totalBalance");
    Some(money(sign, json_amount(total)?))
}

/// StepFun（Go readStepFun，balance.go:624-643）：
/// `{"object":"account","type":"prepaid","balance":26.00,…}`——顶层
/// balance 就是可花余额（代金券在内；total_cash/voucher 是历史累计，
/// 不读）。Step Plan 的 key 也读同一端点：账号余额就是 plan 外的花销。
fn format_stepfun_balance(body: &str, sign: &str) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    Some(money(sign, json_amount(value.get("balance"))?))
}

/// Zhipu/Z.ai 按量 key（Go readZhipuBalance，balance.go:558-614）：
/// 成功应答 `{"code":200,"msg":"操作成功","data":{"balance":0E-9,
/// "rechargeAmount":0.000000,"giveAmount":0.000000,"availableBalance":0E-9,
/// "frozenBalance":0E-9,…},"success":true}`——读 data.availableBalance
/// （可花余额，冻结已扣）。两类不出行：①拒答信封（Go zhipuEnvelope，
/// zhipu_envelope.go:43-60——200 也拒答是这家的常态，key 被拒的
/// 1000–1005 同路，网关口径统一安静跳过）；②非按量端点上的全零账号
/// （availableBalance/balance/rechargeAmount/giveAmount 全 0，Go
/// errNoBalance——按量端点上 ¥0.00 是真实读数，照出）。
fn format_zhipu_balance(body: &str, sign: &str, payg: bool) -> Option<String> {
    let value: Value = serde_json::from_str(body).ok()?;
    if zhipu_refusal(&value) {
        return None;
    }
    // data 缺失或 null → 没有可读的（Go "no data in it"）。
    let data = value.get("data").filter(|d| !d.is_null())?;
    let available = json_amount(data.get("availableBalance"))?;
    if !payg {
        let balance = json_amount(data.get("balance")).unwrap_or(0.0);
        let recharged = json_amount(data.get("rechargeAmount")).unwrap_or(0.0);
        let given = json_amount(data.get("giveAmount")).unwrap_or(0.0);
        if available == 0.0 && balance == 0.0 && recharged == 0.0 && given == 0.0 {
            return None;
        }
    }
    Some(money(sign, available))
}

/// Zhipu 平台的拒答信封（Go zhipuEnvelope）：`"success":false` 且
/// code/msg 任一非空。code 认 JSON 数字或数字串，非整数/读不出都当 0
/// （Go Atoi 失败保持 0）。
fn zhipu_refusal(value: &Value) -> bool {
    if value.get("success").and_then(Value::as_bool) != Some(false) {
        return false;
    }
    let code: i64 = match value.get("code") {
        Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
        Some(Value::String(s)) => s.parse::<i64>().unwrap_or(0),
        _ => 0,
    };
    let msg = value
        .get("msg")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    code != 0 || !msg.is_empty()
}

/// 金额 → `"¥12.34"`（Go money，balance.go:291-293）：符号贴前、两位小数。
fn money(sign: &str, v: f64) -> String {
    format!("{}{:.2}", sign, v)
}

/// 币种码 → 符号（Go currencySign，balance.go:295-305）：CNY/RMB → ¥、
/// USD → $、空 → 空串、其它大写原样 + 空格（`"JPY 0.00"`）。
fn currency_sign(code: &str) -> String {
    match code.to_ascii_uppercase().as_str() {
        "CNY" | "RMB" => "¥".to_string(),
        "USD" => "$".to_string(),
        "" => String::new(),
        other => format!("{} ", other),
    }
}

/// 金额字段：JSON 数字或数字字符串（Go number() 同款），空白不认。
fn json_amount(value: Option<&Value>) -> Option<f64> {
    match value {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 仍未覆盖（Go balance.go 有、本模块没做的来源，后续任务按此清单收）：
// - Command Code（api.commandcode.ai → /alpha/billing/credits，Go:81-82）
// - ModelScope（api-inference.modelscope.cn，Go:83-84，魔粒单位）
// - Ollama Cloud 余额（ollama.com，Go:85-87）
// - AiHubMix（aihubmix.com，key 限额 / 账号 token 两种读法，Go:88-92）
// - 用户自报 BalanceURL/BalancePath 的中继读法（readBalancePath 表达式）
// - 订阅类（Claude/Codex/Gemini 账号 discovery 的窗口卡）——依赖账号
//   体系，T2 已定调不在网关侧做。
// ---------------------------------------------------------------------------

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
        let lock = crate::engine_gateway::TEST_SANDBOX_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Ok(mut guard) = cache().lock() {
            *guard = None;
        }
        if let Ok(mut guard) = TEST_BALANCE_URLS.lock() {
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

    /// 把某 provider id 的余额端点指到假服务（T2 单端点缝的 per-provider
    /// 扩展）。
    fn override_url(id: &str, url: String) {
        TEST_BALANCE_URLS
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get_or_insert_with(std::collections::HashMap::new)
            .insert(id.to_string(), url);
    }

    /// 本地假厂商服务（std TcpListener 手写 HTTP，零新依赖）：每个连接
    /// 计一击并抓请求行（断言端点路径）与 Authorization，回固定 JSON。
    /// hits 就是「真打了网络」的计数器——缓存命中不该产生新连接。
    struct FakeVendor {
        url: String,
        hits: Arc<AtomicUsize>,
        /// 每个连接的（请求行, Authorization）。
        requests: Arc<Mutex<Vec<(String, String)>>>,
    }

    fn fake_vendor(path: &str, body: &'static str) -> FakeVendor {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake vendor");
        let port = listener.local_addr().expect("port").port();
        let hits = Arc::new(AtomicUsize::new(0));
        let requests: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
        let thread_hits = hits.clone();
        let thread_requests = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                thread_hits.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(match stream.try_clone() {
                    Ok(s) => s,
                    Err(_) => continue,
                });
                // GET 无请求体：读到空行 = 头结束。记请求行 + Authorization。
                let mut request_line = String::new();
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
                    if request_line.is_empty() {
                        request_line = trimmed.to_string();
                    }
                    if let Some(value) = trimmed.strip_prefix("Authorization:") {
                        authorization = value.trim().to_string();
                    }
                }
                thread_requests
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push((request_line, authorization));
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
            url: format!("http://127.0.0.1:{port}{path}"),
            hits,
            requests,
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
        // 币种符号走 Go currencySign：CNY→¥、USD→$、表外币种原样带空格。
        let body = r#"{"is_available":true,"balance_infos":[
            {"currency":"cny","total_balance":"110.00","granted_balance":"10.00","topped_up_balance":"100.00"},
            {"currency":"USD","total_balance":1.234},
            {"currency":"JPY","total_balance":"0.00"}]}"#;
        let vendor = fake_vendor("/user/balance", body);
        override_url("deepseek", vendor.url.clone());

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
        assert_eq!(row["balance"], "¥110.00 · $1.23 · JPY 0.00");
        assert!(row.get("user").is_none(), "key 余额没有账号身份");
        assert_eq!(vendor.hits.load(Ordering::SeqCst), 1);
        // 块作用域拿锁：守卫只活在断言里——绑到外层变量会活到测试结束，
        // vendor 线程在下一个连接上等这把锁写记录，ureq 等应答超时
        // （实测 10s 双向死锁）。
        {
            let requests = vendor.requests.lock().unwrap_or_else(|p| p.into_inner());
            assert_eq!(
                requests[0].0, "GET /user/balance HTTP/1.1",
                "端点路径固定（Go balance.go:66）"
            );
            assert_eq!(
                requests[0].1, "Bearer sk-test",
                "余额是 key 的钱：必须带 Bearer"
            );
        }

        // 60s 缓存（?kept=1 语义）：不打网络——计数器不涨。
        let cached = quota_entries(&gateway, false);
        assert_eq!(cached, rows);
        assert_eq!(
            vendor.hits.load(Ordering::SeqCst),
            1,
            "缓存命中不该再打网络"
        );

        // force（面板点刷新 / 无参直连）跳过缓存。
        let again = quota_entries(&gateway, true);
        assert_eq!(again, rows);
        assert_eq!(vendor.hits.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn moonshot_balance_row_shape() {
        // 两个 host 各自的端点与符号（Go balance.go:67-70）：cn ¥ / ai $，
        // 都读 data.available_balance（Go readMoonshot）。
        let providers = r#"{
            "providers": [
                { "id": "moonshot", "name": "Kimi", "key": "sk-ms", "chat": "https://api.moonshot.cn/v1" },
                { "id": "moonshot-intl", "name": "Kimi Intl", "key": "sk-ms2", "chat": "https://api.moonshot.ai/v1" }
            ]
        }"#;
        let (gateway, _sandbox) = sandbox(providers);
        let cn = fake_vendor(
            "/v1/users/me/balance",
            r#"{"code":0,"data":{"available_balance":49.58},"status":true}"#,
        );
        let intl = fake_vendor(
            "/v1/users/me/balance",
            r#"{"code":0,"data":{"available_balance":12},"status":true}"#,
        );
        override_url("moonshot", cn.url.clone());
        override_url("moonshot-intl", intl.url.clone());

        let rows = quota_entries(&gateway, true);
        assert_eq!(rows.len(), 2, "cn / ai 两个 host 各出行");
        assert_eq!(rows[0]["provider"], "moonshot");
        assert_eq!(rows[0]["balance"], "¥49.58");
        assert_eq!(rows[1]["provider"], "moonshot-intl");
        assert_eq!(rows[1]["balance"], "$12.00");
        {
            let requests = cn.requests.lock().unwrap_or_else(|p| p.into_inner());
            assert_eq!(
                requests[0].0, "GET /v1/users/me/balance HTTP/1.1",
                "Moonshot 端点路径（Go balance.go:68）"
            );
            assert_eq!(requests[0].1, "Bearer sk-ms");
        }
    }

    #[test]
    fn openrouter_balance_row_shape() {
        let providers = r#"{
            "providers": [
                { "id": "openrouter", "name": "OpenRouter", "key": "sk-or", "chat": "https://openrouter.ai/api/v1" }
            ]
        }"#;
        let (gateway, _sandbox) = sandbox(providers);
        let vendor = fake_vendor(
            "/api/v1/credits",
            r#"{"data":{"total_credits":20,"total_usage":3.5}}"#,
        );
        override_url("openrouter", vendor.url.clone());

        let rows = quota_entries(&gateway, true);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["provider"], "openrouter");
        assert_eq!(
            rows[0]["balance"], "$16.50",
            "credits − usage（Go readOpenRouter）"
        );
        {
            let requests = vendor.requests.lock().unwrap_or_else(|p| p.into_inner());
            assert_eq!(
                requests[0].0, "GET /api/v1/credits HTTP/1.1",
                "OpenRouter 端点路径（Go balance.go:72）"
            );
            assert_eq!(requests[0].1, "Bearer sk-or");
        }
    }

    #[test]
    fn siliconflow_balance_row_shape() {
        // cn / com 两 host（Go balance.go:73-76）：¥ / $，读 data.totalBalance。
        let providers = r#"{
            "providers": [
                { "id": "siliconflow", "name": "SiliconFlow", "key": "sk-sf", "chat": "https://api.siliconflow.cn/v1" },
                { "id": "siliconflow-intl", "name": "SiliconFlow Intl", "key": "sk-sf2", "chat": "https://api.siliconflow.com/v1" }
            ]
        }"#;
        let (gateway, _sandbox) = sandbox(providers);
        let cn = fake_vendor(
            "/v1/user/info",
            r#"{"code":20000,"data":{"totalBalance":"88.88"}}"#,
        );
        let intl = fake_vendor(
            "/v1/user/info",
            r#"{"code":20000,"data":{"totalBalance":3.5}}"#,
        );
        override_url("siliconflow", cn.url.clone());
        override_url("siliconflow-intl", intl.url.clone());

        let rows = quota_entries(&gateway, true);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["balance"], "¥88.88");
        assert_eq!(rows[1]["balance"], "$3.50");
        {
            let requests = cn.requests.lock().unwrap_or_else(|p| p.into_inner());
            assert_eq!(
                requests[0].0, "GET /v1/user/info HTTP/1.1",
                "SiliconFlow 端点路径（Go balance.go:74）"
            );
            assert_eq!(requests[0].1, "Bearer sk-sf");
        }
    }

    #[test]
    fn stepfun_balance_row_shape() {
        // com / ai 两 host（Go balance.go:77-80）：¥ / $，读顶层 balance。
        let providers = r#"{
            "providers": [
                { "id": "stepfun", "name": "StepFun", "key": "sk-step", "chat": "https://api.stepfun.com/v1" },
                { "id": "stepfun-intl", "name": "StepFun Intl", "key": "sk-step2", "chat": "https://api.stepfun.ai/v1" }
            ]
        }"#;
        let (gateway, _sandbox) = sandbox(providers);
        let cn = fake_vendor(
            "/v1/accounts",
            r#"{"object":"account","type":"prepaid","balance":26.00,"total_cash_balance":0.00,"total_voucher_balance":26.00}"#,
        );
        let intl = fake_vendor(
            "/v1/accounts",
            r#"{"object":"account","type":"prepaid","balance":1.5}"#,
        );
        override_url("stepfun", cn.url.clone());
        override_url("stepfun-intl", intl.url.clone());

        let rows = quota_entries(&gateway, true);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["balance"], "¥26.00");
        assert_eq!(rows[1]["balance"], "$1.50");
        {
            let requests = cn.requests.lock().unwrap_or_else(|p| p.into_inner());
            assert_eq!(
                requests[0].0, "GET /v1/accounts HTTP/1.1",
                "StepFun 端点路径（Go balance.go:78）"
            );
            assert_eq!(requests[0].1, "Bearer sk-step");
        }
    }

    #[test]
    fn zhipu_balance_row_shape() {
        // bigmodel ¥ / z.ai $（Go zhipuBalanceSource）；base 在按量路径
        // /api/paas/v4 上 → payg（全零也出行）；端点固定在 API 自己的 host。
        let providers = r#"{
            "providers": [
                { "id": "zhipu", "name": "Zhipu", "key": "sk-zp", "chat": "https://open.bigmodel.cn/api/paas/v4" },
                { "id": "zai", "name": "Z.ai", "key": "sk-zai", "chat": "https://api.z.ai/api/paas/v4" }
            ]
        }"#;
        let (gateway, _sandbox) = sandbox(providers);
        let bigmodel = fake_vendor(
            "/api/biz/account/query-customer-account-report",
            r#"{"code":200,"msg":"操作成功","data":{"balance":20.000000,"rechargeAmount":20.000000,"giveAmount":0.500000,"totalSpendAmount":0E-9,"availableBalance":12.340000,"frozenBalance":0E-9},"success":true}"#,
        );
        let zai = fake_vendor(
            "/api/biz/account/query-customer-account-report",
            r#"{"code":200,"msg":"操作成功","data":{"balance":8,"rechargeAmount":8,"giveAmount":0,"availableBalance":8},"success":true}"#,
        );
        override_url("zhipu", bigmodel.url.clone());
        override_url("zai", zai.url.clone());

        let rows = quota_entries(&gateway, true);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["provider"], "zhipu");
        assert_eq!(rows[0]["balance"], "¥12.34", "读 data.availableBalance");
        assert_eq!(rows[1]["balance"], "$8.00");
        {
            let requests = bigmodel.requests.lock().unwrap_or_else(|p| p.into_inner());
            assert_eq!(
                requests[0].0, "GET /api/biz/account/query-customer-account-report HTTP/1.1",
                "Zhipu 账号报告路径（Go zhipuAccountReport）"
            );
            assert_eq!(
                requests[0].1, "Bearer sk-zp",
                "key 作 Bearer（Go 注释实锤）"
            );
        }
    }

    #[test]
    fn failing_source_is_skipped_quietly() {
        // 六家全部指到必然没人听的端口（先占后放拿一个空闲口）：连接被拒
        // → 空表，不 panic、不编行。
        let providers = r#"{
            "providers": [
                { "id": "deepseek", "key": "k", "chat": "https://api.deepseek.com/v1" },
                { "id": "moonshot", "key": "k", "chat": "https://api.moonshot.cn/v1" },
                { "id": "openrouter", "key": "k", "chat": "https://openrouter.ai/api/v1" },
                { "id": "siliconflow", "key": "k", "chat": "https://api.siliconflow.cn/v1" },
                { "id": "stepfun", "key": "k", "chat": "https://api.stepfun.com/v1" },
                { "id": "zhipu", "key": "k", "chat": "https://open.bigmodel.cn/api/paas/v4" }
            ]
        }"#;
        let (gateway, _sandbox) = sandbox(providers);
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("port").port();
        drop(listener);
        for id in [
            "deepseek",
            "moonshot",
            "openrouter",
            "siliconflow",
            "stepfun",
            "zhipu",
        ] {
            override_url(id, format!("http://127.0.0.1:{port}/x"));
        }
        let rows = quota_entries(&gateway, true);
        assert!(rows.is_empty(), "出站失败安静跳过每个来源（红线）");
    }

    #[test]
    fn bad_reply_shape_yields_no_row() {
        let (gateway, _sandbox) = sandbox(PROVIDERS);
        let vendor = fake_vendor("/user/balance", "not json at all");
        override_url("deepseek", vendor.url.clone());
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
            Some("¥7.50".to_string())
        );
        // 一个能读的都没有 → 没行（Go："no balance in the reply"）。
        assert_eq!(
            format_deepseek_balance(
                r#"{"balance_infos":[{"currency":"USD","total_balance":"n/a"}]}"#
            ),
            None
        );
    }

    #[test]
    fn openrouter_usage_missing_counts_as_zero() {
        // usage 读不出当 0（Go `used, _ := number(...)`）。
        assert_eq!(
            format_openrouter_balance(r#"{"data":{"total_credits":20}}"#),
            Some("$20.00".to_string())
        );
        // credits 读不出 → 无行。
        assert_eq!(
            format_openrouter_balance(r#"{"data":{"total_usage":1}}"#),
            None
        );
    }

    #[test]
    fn siliconflow_retired_reply_yields_no_row() {
        // 查询下线的拒答（410 在 fetch 层安静掉；200 应答里 code=20092
        // 在解析层识别，Go siliconFlowGone）→ 无行。
        assert_eq!(
            format_siliconflow_balance(
                r#"{"code":20092,"message":"This endpoint is deprecated and is no longer available.","data":null}"#,
                "¥"
            ),
            None
        );
        // 字符串 code 不算下线（Go Code int 解析失败保持 0）→ 照常读。
        assert_eq!(
            format_siliconflow_balance(r#"{"code":"20092","data":{"totalBalance":"5.00"}}"#, "¥"),
            Some("¥5.00".to_string())
        );
    }

    #[test]
    fn zhipu_refusals_and_zero_account_have_no_row() {
        // 拒答信封（200 也拒答是常态）：key 被拒 1000 与别的 code 同路。
        assert_eq!(
            format_zhipu_balance(
                r#"{"code":1000,"msg":"身份验证失败。","success":false}"#,
                "¥",
                true
            ),
            None
        );
        assert_eq!(
            format_zhipu_balance(
                r#"{"code":1113,"msg":"余额不足或无可用资源包","success":false}"#,
                "$",
                false
            ),
            None
        );
        // 非按量端点上的全零账号 → 无卡（Go errNoBalance）。
        assert_eq!(
            format_zhipu_balance(
                r#"{"code":200,"msg":"操作成功","data":{"balance":0E-9,"rechargeAmount":0.000000,"giveAmount":0.000000,"availableBalance":0E-9},"success":true}"#,
                "¥",
                false
            ),
            None
        );
        // 按量端点上 ¥0.00 是真实读数，照出；非按量有充值也照出。
        assert_eq!(
            format_zhipu_balance(
                r#"{"code":200,"msg":"操作成功","data":{"balance":0E-9,"rechargeAmount":0.000000,"giveAmount":0.000000,"availableBalance":0E-9},"success":true}"#,
                "¥",
                true
            ),
            Some("¥0.00".to_string())
        );
        assert_eq!(
            format_zhipu_balance(
                r#"{"code":200,"msg":"操作成功","data":{"balance":5,"rechargeAmount":5,"giveAmount":0,"availableBalance":5},"success":true}"#,
                "$",
                false
            ),
            Some("$5.00".to_string())
        );
    }

    #[test]
    fn one_key_registered_three_times_is_one_quota_row() {
        use super::super::quota_kit::testkit::{redirect, Mock, Redirects};
        // 2026-11-22 实机形状：预设 + 两个 CLI 里发现的来源，钥匙与地址都相同。
        let provider = |id: &str, name: &str, key: &str| {
            serde_json::json!({
                "id": id, "name": name, "key": key,
                "chat": "https://opencode.ai/inference/go/openai/v1",
                "anthropic": "https://opencode.ai/inference/go/anthropic",
            })
        };
        let providers = serde_json::json!({ "providers": [
            provider("source-a", "opencode.ai", "same"),
            provider("opencode-go", "OpenCode Go", "same"),
            provider("source-b", "opencode.ai", "same"),
            provider("source-other", "opencode.ai", "another-account"),
        ]});
        let (gateway, _sandbox) = sandbox(&providers.to_string());
        let mock = Mock::start();
        mock.route(
            "/zen/go/v1/usage",
            200,
            r#"{"usage":{"rolling":{"percent":10,"resetsAt":"2030-01-01T05:00:00Z"},"weekly":{"percent":4,"resetsAt":"2030-01-07T00:00:00Z"},"monthly":{"percent":2,"resetsAt":"2030-01-31T00:00:00Z"}}}"#,
        );
        let _redirects = Redirects::new(&[redirect("https://opencode.ai", &mock.base)]);

        let rows: Vec<Value> = quota_entries(&gateway, true)
            .into_iter()
            .filter(|row| {
                row["provider"]
                    .as_str()
                    .is_some_and(|id| id.starts_with("source-") || id == "opencode-go")
            })
            .collect();
        assert_eq!(rows.len(), 2, "同钥匙一行、另一把钥匙一行: {rows:?}");
        assert_eq!(rows[0]["provider"], "opencode-go", "留预设的名字");
        assert_eq!(
            rows[0]["aliases"],
            serde_json::json!(["source-a", "source-b"])
        );
        assert_eq!(rows[1]["provider"], "source-other");
        assert!(rows[1].get("aliases").is_none());
    }

    #[test]
    fn merge_only_joins_rows_that_are_the_same_quota() {
        let provider = |id: &str, chat: &str| {
            let mut p = Provider::default();
            p.id = id.to_string();
            p.key = "same".to_string();
            p.chat = chat.to_string();
            p
        };
        let providers = vec![
            provider("source-d", "https://api.deepseek.com/v1"),
            provider("deepseek", "https://api.deepseek.com/v1"),
            provider("ollama", "https://ollama.com/v1"),
            provider("elsewhere", "https://relay.example/v1"),
        ];
        let balance = |id: &str, money: &str| serde_json::json!({"provider": id, "windows": [], "balance": money});
        let rows = vec![
            balance("source-d", "¥18.21"),
            balance("deepseek", "¥18.21"),
            // 同一来源的套餐行和余额行是两回事
            serde_json::json!({"provider": "ollama", "windows": [{"name": "Month", "used": 100.0}]}),
            balance("ollama", "$0.00"),
            // 同钥匙但不是同一家
            balance("elsewhere", "¥18.21"),
            // 多钥匙来源（带 user）和没有钥匙的账号行都不并
            serde_json::json!({"provider": "deepseek", "user": "key 2", "windows": [], "balance": "¥18.21"}),
            balance("codex", "1"),
            balance("codex", "1"),
        ];
        let merged = merge_same_key_rows(rows, &providers);
        assert_eq!(merged.len(), 7, "{merged:?}");
        assert_eq!(merged[0]["provider"], "deepseek");
        assert_eq!(merged[0]["aliases"], serde_json::json!(["source-d"]));
    }

    #[test]
    fn zhipu_coding_endpoint_is_not_a_balance_source() {
        // GLM Coding Plan 端点不问余额（Go zhipuBalanceSource return false）；
        // 但它不拦 host 表——同 provider 的 deepseek 端点照常命中。
        let mut coding = Provider::default();
        coding.chat = "https://open.bigmodel.cn/api/coding/paas/v4".to_string();
        assert!(balance_source(&coding).is_none(), "Coding Plan 不出余额行");

        let mut mixed = Provider::default();
        mixed.chat = "https://open.bigmodel.cn/api/coding/paas/v4".to_string();
        mixed.responses = "https://api.deepseek.com/v1".to_string();
        assert!(
            matches!(balance_source(&mixed), Some((_, BalanceKind::DeepSeek))),
            "coding 端点只退出 zhipu 判定，不拦后面的 host 表（Go 同款）"
        );

        // anthropic 端点（/api/anthropic）也是 zhipu host：非按量路径，
        // 非全零才出行。
        let mut via_anthropic = Provider::default();
        via_anthropic.anthropic = "https://open.bigmodel.cn/api/anthropic".to_string();
        assert!(matches!(
            balance_source(&via_anthropic),
            Some((url, BalanceKind::Zhipu { payg: false, .. })) if url == "https://open.bigmodel.cn/api/biz/account/query-customer-account-report"
        ));
    }
}
