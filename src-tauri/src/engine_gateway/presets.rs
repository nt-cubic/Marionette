//! models.dev 目录兜底（账本 T8，合同 C2）——厂商 `/v1/models` 行漏掉
//! `context_length` 时，按模型 id 从 models.dev 目录补上下文窗口 / 输出
//! 上限 / 图片能力 / 显示名。Go 版对应 internal/catalog（catalog.go 的
//! ContextOf / SeesImages / OutputOf，随 catalog.KeepFresh 刷新）；这里
//! 简化为两件事：
//!
//! - [`lookup`]：纯内存 + 磁盘缓存读，**绝不联网、绝不落盘**——厂商行
//!   没漏字段时调用方（catalog.rs 接线，主线完成）根本不会问到它。
//! - [`refresh`]：唯一联网路径（显式调用，出站 agent 由调用方传
//!   `super::outbound_agent()`），抓 `https://models.dev/api.json` 原文
//!   原子落盘；**缓存永不过期**（拉取失败保持旧缓存继续服务）。
//!
//! 口径红线（C2）：窗口/上限 0 = 不知道——目录数据照实给 0，不编数；
//! 消费侧只在厂商行缺字段时用这里的兜底值。Go 的投票取多数 / 重命名
//! （`[1m]` 窗口标记）/ 拼写归一（点横线互认）都不做：providers 按字典序
//! 遍历、同一裸 id 先到先得（结果确定），匹配规则只有两条——裸 id、
//! 厂商前缀剥离后的 id（厂商 id 集合从目录数据派生）。
//!
//! api.json 的真实字段是 `limit.{context,input,output}` +
//! `modalities.input` + `name`（见 Go mdModel）；厂商 /v1/models 风格的
//! `context_length` / `max_output_tokens` 直写在模型对象上的松散形态也认
//! （仅作兜底，limit 优先）。
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;

/// models.dev 目录地址（生产固定；测试用 [`TEST_MODELS_DEV_URL`] 覆盖）。
const MODELS_DEV_URL: &str = "https://models.dev/api.json";
/// 单次抓取超时（catalog / quotas 同款 10s：一个慢请求最多拖自己）。
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// 目录缓存文件名（落在 `config_dir()/cache/` 下，见 [`cache_path`]）。
const CACHE_FILE: &str = "modelsdev.json";
/// 目录体量上限（Go io.LimitReader 64MB 同款，防异常应答撑爆内存）。
const MAX_BODY_BYTES: usize = 64 << 20;
/// 最小可信窗口（Go LeastWindow 同值）：低于它的窗口读作「不知道」（0）
/// ——过小的数字多半是目录噪音，按它办事会触发提前压缩。
const LEAST_WINDOW: u64 = 2048;

/// 一个模型的目录预设。窗口/上限 0 = 目录也不知道（C2：不编数）。
#[derive(Clone, Debug)]
pub struct Preset {
    pub context_window: u64,
    pub max_output: u64,
    pub images: bool,
    pub display_name: String,
}

/// 内存索引：裸 id（小写）→ 预设。从磁盘缓存懒加载一次，`refresh`
/// 成功后整体替换。
#[derive(Default)]
struct Index {
    /// 裸 id（小写）→ 预设。同一裸 id 多家 provider 给出时先到先得，
    /// providers 按字典序遍历，结果确定。
    entries: HashMap<String, Preset>,
    /// 目录顶层 provider id（小写），长度降序——厂商前缀剥离用
    /// （长 id 先试，`z-ai` 优先于假想的 `z`）。
    provider_ids: Vec<String>,
}

/// 进程级索引（网关进程内只有一份）。None = 还没从磁盘加载过。
fn index_cell() -> &'static Mutex<Option<Index>> {
    static INDEX: OnceLock<Mutex<Option<Index>>> = OnceLock::new();
    INDEX.get_or_init(|| Mutex::new(None))
}

/// 测试缝：把目录地址指到本地假服务（生产路径恒固定地址）。
#[cfg(test)]
static TEST_MODELS_DEV_URL: Mutex<Option<String>> = Mutex::new(None);

/// 按模型 id 查目录预设。匹配规则（按序）：
/// 1. 裸 id：先剥 `marionette::` 一类 `::` 命名空间，再取 `/` 最后一段
///    （`deepseek/deepseek-chat`、`z-ai/glm-5.3`、openrouter 的
///    `openai/gpt-4o` 都在此列），小写比对；
/// 2. 厂商前缀剥离：裸 id 以「目录里出现过的 provider id + `-`/`:`」开头
///    时，用剥掉前缀的余段再查一次（`z-ai-glm-5.3` → `glm-5.3`）。
///
/// 没缓存 / 没见过 → None。**不联网、不落盘、不阻塞**——联网只发生在
/// [`refresh`]。
pub fn lookup(model_id: &str) -> Option<Preset> {
    with_index(|index| {
        let bare = bare_id(model_id).to_ascii_lowercase();
        if bare.is_empty() {
            return None;
        }
        if let Some(preset) = index.entries.get(&bare) {
            return Some(preset.clone());
        }
        for pid in &index.provider_ids {
            let Some(rest) = bare.strip_prefix(pid.as_str()) else { continue };
            // `/` 分隔在 bare_id（取最后一段）时已经剥过，这里只剩 `-`/`:`。
            let Some(rest) = rest.strip_prefix('-').or_else(|| rest.strip_prefix(':')) else {
                continue;
            };
            if !rest.is_empty() {
                if let Some(preset) = index.entries.get(rest) {
                    return Some(preset.clone());
                }
            }
        }
        None
    })
}

/// 抓取 models.dev 目录并落盘（唯一联网路径；出站 agent 由调用方传
/// `super::outbound_agent()`）。成功 = 解析通过 + 原子写缓存 + 替换内存
/// 索引；任何失败（网络 / 非 200 / 坏载荷）返回 Err，**旧缓存与内存索引
/// 原样保留**（拉取失败退缓存）。缓存永不过期，刷新只靠显式调用。
pub fn refresh(agent: &ureq::Agent) -> Result<(), String> {
    refresh_from(&source_url(), agent)
}

fn source_url() -> String {
    #[cfg(test)]
    if let Ok(guard) = TEST_MODELS_DEV_URL.lock() {
        if let Some(url) = guard.as_deref() {
            return url.to_string();
        }
    }
    MODELS_DEV_URL.to_string()
}

fn refresh_from(url: &str, agent: &ureq::Agent) -> Result<(), String> {
    let response = agent
        .get(url)
        .set("Accept", "application/json")
        .timeout(FETCH_TIMEOUT)
        .call()
        .map_err(|e| format!("models.dev: {e}"))?;
    if response.status() != 200 {
        return Err(format!("models.dev: HTTP {}", response.status()));
    }
    let body = response
        .into_string()
        .map_err(|e| format!("models.dev: {e}"))?;
    if body.len() > MAX_BODY_BYTES {
        return Err("models.dev: payload too large".to_string());
    }
    // 先解析、解析过了才落盘——失败绝不动磁盘上的旧缓存。
    let index = parse_catalog(&body).ok_or("models.dev: unexpected payload")?;
    write_cache(&body)?;
    let mut guard = index_cell().lock().unwrap_or_else(|p| p.into_inner());
    *guard = Some(index);
    Ok(())
}

/// 只读访问索引；第一次访问时从磁盘缓存懒加载（没有/读失败 → 空索引，
/// 不联网不重试——之后只有 refresh 能再填）。
fn with_index<T>(read: impl FnOnce(&Index) -> T) -> T {
    let cell = index_cell();
    let mut guard = cell.lock().unwrap_or_else(|p| p.into_inner());
    if guard.is_none() {
        *guard = Some(load_cached());
    }
    read(guard.as_ref().expect("上面刚填过"))
}

fn load_cached() -> Index {
    cache_path()
        .and_then(|path| std::fs::read_to_string(&path).ok())
        .and_then(|body| parse_catalog(&body))
        .unwrap_or_default()
}

/// 缓存路径：`config_dir()/cache/modelsdev.json`——安装形态
/// `~/.config/magpie/cache/modelsdev.json`（XDG_CONFIG_HOME 可重定向，
/// 测试沙箱依赖这一点），便携形态 exe 旁 `cache/modelsdev.json`
/// （mod.rs config_dir 同款判定）。
fn cache_path() -> Option<PathBuf> {
    super::config_dir().map(|dir| dir.join("cache").join(CACHE_FILE))
}

/// 原子写（tmp + rename；tmp 名带 pid 防两窗互吞，store.rs save 同款）；
/// rename 失败兜底直写（弃原子性保数据，storage.rs write_atomic 同思路）。
fn write_cache(body: &str) -> Result<(), String> {
    let path = cache_path().ok_or("no config dir (set USERPROFILE)")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("cache dir: {e}"))?;
    }
    let tmp = path.with_extension(format!("json.tmp{}", std::process::id()));
    std::fs::write(&tmp, body).map_err(|e| e.to_string())?;
    match std::fs::rename(&tmp, &path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            std::fs::write(&path, body).map_err(|e2| format!("rename {e}; write {e2}"))
        }
    }
}

/// 裸模型 id：剥 `marionette::` 一类 `::` 命名空间（取后段），再取 `/`
/// 最后一段。只切不改——小写化在查表处做，显示名另有来处。
fn bare_id(id: &str) -> &str {
    let id = id.trim();
    let id = id.rsplit("::").next().unwrap_or(id);
    id.rsplit('/').next().unwrap_or(id)
}

/// 解析 api.json → 索引。None = 坏载荷（空目录 / 无模型一律拒收，
/// Go `len(probe) > 0` 同款——宁可不兜底也不喂半截目录）。
fn parse_catalog(body: &str) -> Option<Index> {
    let value: Value = serde_json::from_str(body).ok()?;
    let providers = value.as_object()?;
    if providers.is_empty() {
        return None;
    }
    // provider 按字典序遍历（HashMap 迭代序随机，先到先得必须定序）。
    let mut sorted: Vec<(&String, &Value)> = providers.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let mut index = Index::default();
    for (_, provider) in &sorted {
        let Some(models) = provider.get("models").and_then(Value::as_object) else {
            continue;
        };
        for (model_id, model) in models {
            let key = bare_id(model_id).to_ascii_lowercase();
            if key.is_empty() {
                continue;
            }
            let preset = Preset {
                context_window: window_of(model),
                max_output: output_of(model),
                images: takes_images(model),
                display_name: model
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|n| !n.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| bare_id(model_id).to_string()),
            };
            index.entries.entry(key).or_insert(preset);
        }
    }
    if index.entries.is_empty() {
        return None;
    }
    // 厂商 id 集合从目录数据派生：顶层键，小写、去重（BTreeSet）、
    // 长度降序（长前缀先试）。
    let mut pids: Vec<String> = providers
        .keys()
        .map(|pid| pid.to_ascii_lowercase())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    pids.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    index.provider_ids = pids;
    Some(index)
}

/// 模型对象上的正整数字段（0/非数 = 0）——松散形态兜底用。
fn model_u64(model: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|k| model.get(*k).and_then(Value::as_u64).filter(|v| *v > 0))
        .unwrap_or(0)
}

/// 窗口（Go mdModel.window() 同语义）：`limit.input` 优先、被更小的
/// `limit.context` 封顶（Cloudflare 把 gpt-5 标 272K input / 128K 窗口
/// 那类行按窗口算），两者各自先过 [`LEAST_WINDOW`] 地板（低于它 =
/// 不知道）；都缺时认直写的 `context_length`/`context_window`。
fn window_of(model: &Value) -> u64 {
    let limit = |key: &str| {
        model
            .get("limit")
            .and_then(|l| l.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let floor = |n: u64| if n < LEAST_WINDOW { 0 } else { n };
    let input = floor(limit("input"));
    let context = floor(limit("context"));
    if input > 0 {
        return if context > 0 && context < input { context } else { input };
    }
    if context > 0 {
        return context;
    }
    floor(model_u64(model, &["context_length", "context_window"]))
}

/// 输出上限：`limit.output`，缺时认直写的 `max_output_tokens`/`max_tokens`。
fn output_of(model: &Value) -> u64 {
    let output = model
        .get("limit")
        .and_then(|l| l.get("output"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if output > 0 {
        return output;
    }
    model_u64(model, &["max_output_tokens", "max_tokens"])
}

/// 图片输入：`modalities.input` 数组含 `image` 或 `image/*` 媒体类型
/// （Go Kinds() 把 `image/jpeg` 这类也当 image——按字面读会错杀一整批
/// 能看图的模型）。
fn takes_images(model: &Value) -> bool {
    model
        .get("modalities")
        .and_then(|m| m.get("input"))
        .and_then(Value::as_array)
        .map(|items| {
            items.iter().any(|v| {
                v.as_str()
                    .map(|s| {
                        let s = s.trim().to_ascii_lowercase();
                        s == "image" || s.starts_with("image/")
                    })
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// 测试串行锁：配置目录靠 XDG_CONFIG_HOME（进程级 env），并行测试会
    /// 互踩——全局锁串行（quotas.rs 沙箱同款做法）。
    static TEST_LOCK: Mutex<()> = Mutex::new(());
    static DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

    /// 一次测试的世界：%TEMP% 下唯一配置目录 + 模块状态复位 + env 恢复 +
    /// 串行锁，Drop 收尾（含 panic 展开的路径）。绝不碰真实
    /// `~/.config/magpie`。
    struct Sandbox {
        dir: PathBuf,
        prior_xdg: Option<String>,
        lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            if let Ok(mut guard) = index_cell().lock() {
                *guard = None;
            }
            if let Ok(mut guard) = TEST_MODELS_DEV_URL.lock() {
                *guard = None;
            }
            if let Some(prior) = &self.prior_xdg {
                std::env::set_var("XDG_CONFIG_HOME", prior);
            } else {
                std::env::remove_var("XDG_CONFIG_HOME");
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn sandbox() -> Sandbox {
        let lock = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        if let Ok(mut guard) = index_cell().lock() {
            *guard = None;
        }
        if let Ok(mut guard) = TEST_MODELS_DEV_URL.lock() {
            *guard = None;
        }
        let dir = std::env::temp_dir().join(format!(
            "marionette-engine-presets-{}-{}",
            std::process::id(),
            DIR_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp config dir");
        let prior_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        Sandbox { dir, prior_xdg, lock }
    }

    /// 本地假目录服务（std TcpListener 手写 HTTP，零新依赖）：`/ok` 路径
    /// 回固定 api.json，`/fail` 路径回 500。返回两个地址。
    fn fake_catalog(body: &'static str) -> (String, String) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake models.dev");
        let port = listener.local_addr().expect("port").port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                // 读掉请求头（GET 无请求体；不读就写，Windows 上可能 RST）。
                let mut line = String::new();
                let mut path = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        break;
                    }
                    if path.is_empty() && trimmed.starts_with("GET ") {
                        path = trimmed.split_whitespace().nth(1).unwrap_or("").to_string();
                    }
                }
                let (code, reason, payload): (u16, &str, &[u8]) = if path.contains("fail") {
                    (500, "Internal Server Error", b"")
                } else {
                    (200, "OK", body.as_bytes())
                };
                let head = format!(
                    "HTTP/1.1 {code} {reason}\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                    payload.len()
                );
                let mut stream = stream;
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(payload);
                let _ = stream.flush();
            }
        });
        (
            format!("http://127.0.0.1:{port}/ok.json"),
            format!("http://127.0.0.1:{port}/fail.json"),
        )
    }

    /// 小目录：z-ai（带连字符的 provider id，喂前缀剥离）与 deepseek；
    /// 覆盖 input 被窗口封顶（capped-lm）、低于地板线（tiny-lm）、
    /// 缺 name（anon-lm）、纯文本（glm-5.3-air）。
    const API: &str = r#"{
        "z-ai": {
            "name": "Z.AI",
            "models": {
                "glm-5.3": {
                    "id": "glm-5.3",
                    "name": "GLM-5.3",
                    "limit": { "context": 204800, "input": 0, "output": 32768 },
                    "modalities": { "input": ["text", "image"], "output": ["text"] }
                },
                "glm-5.3-air": {
                    "name": "GLM-5.3 Air",
                    "limit": { "context": 204800, "output": 32768 },
                    "modalities": { "input": ["text"] }
                }
            }
        },
        "deepseek": {
            "name": "DeepSeek",
            "models": {
                "deepseek-chat": {
                    "name": "DeepSeek Chat",
                    "limit": { "context": 131072, "input": 131072, "output": 8192 },
                    "modalities": { "input": ["text", "image"] }
                },
                "capped-lm": {
                    "name": "Capped",
                    "limit": { "context": 128000, "input": 300000, "output": 4096 }
                },
                "tiny-lm": {
                    "name": "Tiny",
                    "limit": { "context": 512, "output": 512 }
                },
                "anon-lm": {
                    "limit": { "context": 8192 }
                }
            }
        }
    }"#;

    #[test]
    fn lookup_without_cache_is_none() {
        let _sandbox = sandbox();
        // 没跑过 refresh、没有缓存文件：查什么都不命中，也不产生缓存。
        assert!(lookup("glm-5.3").is_none());
        assert!(lookup("deepseek/deepseek-chat").is_none());
        assert!(lookup("z-ai-glm-5.3").is_none());
        let cache = cache_path().expect("sandbox cache path");
        assert!(!cache.exists(), "lookup 不得落盘");
    }

    #[test]
    fn refresh_then_lookup_hits() {
        let _sandbox = sandbox();
        let (ok_url, _fail_url) = fake_catalog(API);
        if let Ok(mut guard) = TEST_MODELS_DEV_URL.lock() {
            *guard = Some(ok_url);
        }
        refresh(&super::super::outbound_agent()).expect("refresh ok");
        // 缓存已落盘（config_dir()/cache/modelsdev.json）。
        let cache = cache_path().expect("sandbox cache path");
        assert!(cache.exists(), "refresh 后缓存文件应存在");
        // 裸 id 直接命中：窗口/输出/图片/显示名全对。
        let glm = lookup("glm-5.3").expect("bare hit");
        assert_eq!(glm.context_window, 204800);
        assert_eq!(glm.max_output, 32768);
        assert!(glm.images);
        assert_eq!(glm.display_name, "GLM-5.3");
        // provider/model 形态（取 / 最后一段）。
        let chat = lookup("deepseek/deepseek-chat").expect("provider/model hit");
        assert_eq!(chat.context_window, 131072);
        assert_eq!(chat.max_output, 8192);
        assert!(chat.images);
        // marionette:: 命名空间 + 大小写不敏感。
        let via_ns = lookup("MARIONETTE::z-ai/GLM-5.3").expect("namespace hit");
        assert_eq!(via_ns.context_window, 204800);
        // 厂商前缀剥离：z-ai-glm-5.3 → glm-5.3（provider id 集合来自目录）。
        let stripped = lookup("z-ai-glm-5.3").expect("prefix-stripped hit");
        assert_eq!(stripped.context_window, 204800);
        assert_eq!(stripped.display_name, "GLM-5.3");
        // 纯文本模型 images=false；没见过的不命中。
        assert!(!lookup("glm-5.3-air").expect("air hit").images);
        assert!(lookup("gpt-not-a-model").is_none());
    }

    #[test]
    fn window_semantics_match_go() {
        let _sandbox = sandbox();
        let (ok_url, _) = fake_catalog(API);
        if let Ok(mut guard) = TEST_MODELS_DEV_URL.lock() {
            *guard = Some(ok_url);
        }
        refresh(&super::super::outbound_agent()).expect("refresh ok");
        // input 超过 context 时被窗口封顶（Cloudflare gpt-5 那类行）。
        assert_eq!(lookup("capped-lm").expect("capped").context_window, 128000);
        // 低于地板线的「窗口」读作不知道（0 = 不编数）；输出上限不吃
        // 地板线（Go Output = limit.output 原值，Window()/LeastWindow 只
        // 罩窗口）。
        let tiny = lookup("tiny-lm").expect("tiny hit");
        assert_eq!(tiny.context_window, 0);
        assert_eq!(tiny.max_output, 512);
        // 缺 name 的条目回落到模型 id 本身。
        assert_eq!(lookup("anon-lm").expect("anon").display_name, "anon-lm");
    }

    #[test]
    fn refresh_failure_keeps_cache_serving() {
        let _sandbox = sandbox();
        let (ok_url, fail_url) = fake_catalog(API);
        if let Ok(mut guard) = TEST_MODELS_DEV_URL.lock() {
            *guard = Some(ok_url);
        }
        refresh(&super::super::outbound_agent()).expect("refresh ok");
        assert!(lookup("glm-5.3").is_some());
        // 抓取失败（HTTP 500）：refresh 报错，旧缓存原样继续服务。
        if let Ok(mut guard) = TEST_MODELS_DEV_URL.lock() {
            *guard = Some(fail_url);
        }
        assert!(refresh(&super::super::outbound_agent()).is_err());
        let glm = lookup("glm-5.3").expect("旧缓存仍命中");
        assert_eq!(glm.context_window, 204800);
        assert_eq!(glm.display_name, "GLM-5.3");
    }

    #[test]
    fn cache_reload_serves_after_index_reset() {
        let _sandbox = sandbox();
        let (ok_url, _) = fake_catalog(API);
        if let Ok(mut guard) = TEST_MODELS_DEV_URL.lock() {
            *guard = Some(ok_url);
        }
        refresh(&super::super::outbound_agent()).expect("refresh ok");
        // 模拟新进程：内存索引清空后，lookup 从磁盘缓存重新加载。
        if let Ok(mut guard) = index_cell().lock() {
            *guard = None;
        }
        let chat = lookup("deepseek/deepseek-chat").expect("disk reload hit");
        assert_eq!(chat.context_window, 131072);
        assert_eq!(chat.max_output, 8192);
        assert!(chat.images);
    }

    /// 坏载荷拒收：不落盘、不替换索引（喂一段合法 JSON 但不是目录）。
    #[test]
    fn bad_payload_is_rejected() {
        let _sandbox = sandbox();
        let (ok_url, _) = fake_catalog(r#"{"not":"a catalog"}"#);
        if let Ok(mut guard) = TEST_MODELS_DEV_URL.lock() {
            *guard = Some(ok_url);
        }
        assert!(refresh(&super::super::outbound_agent()).is_err());
        assert!(lookup("anything").is_none());
        let cache = cache_path().expect("sandbox cache path");
        assert!(!cache.exists(), "坏载荷不得落盘");
    }
}
