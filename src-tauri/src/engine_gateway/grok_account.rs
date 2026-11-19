//! Grok 订阅账号（SuperGrok / X Premium+，Grok Build CLI 的登录）的只读支持——
//! Go 版 `provider/grok*.go`（凭据发现 + 请求整形）加上 Grok 插件
//! （@magpie-community/opencode-grok-auth）负责的模型表 / 额度 / 出站签名的
//! 最小移植。
//!
//! 范围：
//! - 凭据发现：`$GROK_HOME|~/.grok/auth.json` 里（按键名排序）第一份带 `key`
//!   的登录（Go `readGrokCredential`）；
//! - 合成来源 `grok`（只在内存，**绝不写进 providers.json**；用户自己定义了
//!   同名来源就让位）；
//! - 上游是 `{GrokBase}/responses`（OpenAI Responses 协议）：Grok CLI 同款头；
//!   `/v1/responses` 入站整形后直通，`/v1/chat/completions` 与 `/v1/messages`
//!   入站经 translate.rs 转成 Responses 请求、应答再经
//!   `ResponsesToChat(Reader)` / `AnthropicSseTranslator` 翻回去；
//! - `/v1/models` 的 `grok/<id>` 行（`{GrokBase}/models`；30s 缓存、失败退避、
//!   回旧缓存）；上游给多个上下文窗口的模型按插件做法额外给 `grok-4.7@500k`
//!   这类 id，发出时带 `x-grok-context-window`；
//! - 额度行（`{GrokBase}/billing?format=credits`——Grok CLI `_x.ai/billing`
//!   扩展读的那个端点）。
//!
//! **安全红线（只读）**：绝不写/轮换/刷新任何凭据文件，绝不拉起 `grok` 二进制，
//! 绝不碰 auth.x.ai——token 过期就如实报「请先运行一次 Grok CLI 刷新登录」，
//! 刷新留给 Grok 自己。令牌值永不进日志/错误文本/测试输出。
//!
//! namespace 还原（Go grok_namespace.go）：出站把 Codex 的 namespace 工具压平
//! 成 `namespace__name` 发给 Grok；**应答侧把模型回的调用翻回原名 + namespace**
//! （nsTidy :85 / namespacedIn :31 / sealedArgs :232 的移植）——Codex 读不到
//! namespace 字段就把调用当普通函数，读不到 `encrypted_function_args` 清单就
//! 把参数当密文（#404 / #3183）。schema pattern 转写（Go grok_schema.go
//! grokToolPatterns :25 / ecmaClassForRust :41）：JSON Schema `pattern` 里的
//! ECMA 正则字符类转写成 Grok（Rust regex 语法）认的形式。
//!
//! 已知缺口（见交付报告）：不做设备码登录（`grok login` 是 CLI 的事）；没有
//! 「401 → 拉起 Grok Build 续期再重试」；没有 Responses 流首字节前被切断的
//! 自动重试；不做多账号（grok_accounts.go 的 logins.json 侧账号）；
//! 画图/视频/语音端点不走。
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use super::catalog::VendorRow;
use super::effort::Proto;
use super::httpd::{Reply, Request};
use super::store::Provider;

pub const GROK_ID: &str = "grok";
const DEFAULT_BASE: &str = "https://cli-chat-proxy.grok.com/v1";
/// Grok CLI 版本头的兜底（本机 `version.json` 更新就取新）。
const CLIENT_VERSION: &str = "1.0.77";
const CACHE_TTL: Duration = Duration::from_secs(30);
const FAIL_BACKOFF: Duration = Duration::from_secs(60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// token 离到期不足这个秒数就当过期（不刷新，只报错）。
const EXPIRY_MARGIN_SECS: i64 = 30;
const EXPIRED_MESSAGE: &str = "Grok sign-in expired — run Grok once to refresh";
const QUOTA_NAME: &str = "Grok (SuperGrok)";

// =====================================================================
// 测试缝（cfg(test) 才存在；生产路径恒走真实位置）
// =====================================================================

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct Seam {
    pub base: String,
    pub home: PathBuf,
}

/// 测试缝：未设置时 **没有任何凭据来源**（测试默认读不到真实账号）。
#[cfg(test)]
pub(crate) static TEST_SEAM: Mutex<Option<Seam>> = Mutex::new(None);

#[cfg(test)]
fn seam() -> Option<Seam> {
    TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

#[cfg(test)]
fn base_url() -> String {
    seam().map(|s| s.base).unwrap_or_else(|| "http://127.0.0.1:9/v1".into())
}
#[cfg(not(test))]
fn base_url() -> String {
    DEFAULT_BASE.to_string()
}

#[cfg(test)]
fn grok_home() -> Option<PathBuf> {
    seam().map(|s| s.home)
}
/// Go `GrokHome`：`GROK_HOME` 优先，否则 `~/.grok`。
#[cfg(not(test))]
fn grok_home() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("GROK_HOME").filter(|h| !h.is_empty()) {
        return Some(PathBuf::from(h));
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|h| PathBuf::from(h).join(".grok"))
}

// =====================================================================
// 凭据
// =====================================================================

/// 发现到的一份 Grok 登录（令牌值不 Debug、不进日志）。
#[derive(Clone)]
pub(crate) struct Account {
    token: String,
    email: String,
    user_id: String,
    /// auth.json 的 `expires_at`（unix 秒）；读不出 = None（按有效算）。
    exp: Option<i64>,
}

impl Account {
    fn expired(&self) -> bool {
        self.exp.is_some_and(|e| e <= now_secs() + EXPIRY_MARGIN_SECS)
    }
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn str_field(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").trim().to_string()
}

/// Go `readGrokCredential`：auth.json 是 `{<issuer::id>: {key, email, expires_at, …}}`，
/// 按键名排序取第一份带 `key` 的。
fn read_credential(home: &std::path::Path) -> Option<Account> {
    let all: Value = serde_json::from_slice(&std::fs::read(home.join("auth.json")).ok()?).ok()?;
    let map = all.as_object()?;
    let mut keys: Vec<&String> = map.keys().collect();
    keys.sort();
    for k in keys {
        let entry = &map[k];
        let token = str_field(entry, "key");
        if token.is_empty() {
            continue;
        }
        let exp = entry
            .get("expires_at")
            .and_then(|e| e.as_str())
            .and_then(|e| chrono::DateTime::parse_from_rfc3339(e.trim()).ok())
            .map(|t| t.timestamp());
        return Some(Account { token, email: str_field(entry, "email"), user_id: str_field(entry, "user_id"), exp });
    }
    None
}

/// 只读发现；没登录 / 文件坏 = None。
pub(crate) fn discover() -> Option<Account> {
    read_credential(&grok_home()?)
}

/// 合成来源：有账号才有。`responses` 指向 Grok 后端；不带 key/headers
/// （签名在出站时现做）。用户在 providers.json 里自己定义了 `grok` 时调用方
/// 负责让位。
pub fn synthetic_provider() -> Option<Provider> {
    discover()?;
    Some(Provider {
        id: GROK_ID.to_string(),
        name: "Grok".to_string(),
        responses: base_url(),
        ..Default::default()
    })
}

// =====================================================================
// 出站签名
// =====================================================================

fn version_parts(v: &str) -> Option<Vec<u64>> {
    let parts: Option<Vec<u64>> = v.trim().split('.').map(|p| p.parse().ok()).collect();
    parts.filter(|p| !p.is_empty())
}

fn newer_version(a: &str, b: &str) -> String {
    match (version_parts(a), version_parts(b)) {
        (Some(x), Some(y)) if y > x => b.trim().to_string(),
        _ => a.trim().to_string(),
    }
}

/// 本机 Grok CLI 自己记的版本（`version.json`）与内置版本取新——别报过时的
/// 版本号（后端会按客户端版本给功能）。
fn client_version() -> String {
    let mut v = CLIENT_VERSION.to_string();
    if let Some(home) = grok_home() {
        if let Some(local) = std::fs::read(home.join("version.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|j| j.get("version").and_then(|c| c.as_str()).map(String::from))
        {
            v = newer_version(&v, &local);
        }
    }
    v
}

/// 给一个 ureq 请求签上 Grok CLI 同款头。
fn sign(request: ureq::Request, account: &Account, ver: &str) -> ureq::Request {
    let mut request = request
        .set("Authorization", &format!("Bearer {}", account.token))
        .set("x-xai-token-auth", "xai-grok-cli")
        .set("x-grok-client-identifier", "grok-shell")
        .set("x-grok-client-version", ver)
        .set("User-Agent", &format!("xai-grok-shell/{ver}"));
    if !account.user_id.is_empty() {
        request = request.set("x-userid", &account.user_id);
    }
    request
}

// =====================================================================
// 模型目录
// =====================================================================

struct State {
    cache: Option<(Instant, Vec<VendorRow>)>,
    backoff_until: Option<Instant>,
}

static STATE: Mutex<State> = Mutex::new(State { cache: None, backoff_until: None });

fn lock_state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    let mut s = lock_state();
    s.cache = None;
    s.backoff_until = None;
}

/// 让缓存立刻过期但保留旧行（测 stale-on-error 用）。
#[cfg(test)]
pub(crate) fn expire_cache_for_tests() {
    let mut s = lock_state();
    if let Some(c) = s.cache.as_mut() {
        c.0 = Instant::now() - CACHE_TTL - Duration::from_secs(1);
    }
    s.backoff_until = None;
}

/// 窗口后缀：整千写 `500k`，否则写裸数字（`131072`）——插件 `windowsOf` 同款。
fn window_suffix(tokens: u64) -> String {
    if tokens > 0 && tokens % 1000 == 0 {
        format!("{}k", tokens / 1000)
    } else {
        tokens.to_string()
    }
}

/// `grok-4.7@500k` → (`grok-4.7`, Some(500000))；`@131072`（无 k）按 token 数；
/// 没有窗口后缀 → (原样, None)。
fn split_window(id: &str) -> (String, Option<u64>) {
    let Some((base, tail)) = id.rsplit_once('@') else { return (id.to_string(), None) };
    if base.is_empty() || tail.is_empty() {
        return (id.to_string(), None);
    }
    let (digits, mult) = match tail.strip_suffix('k') {
        Some(d) => (d, 1000),
        None => (tail, 1),
    };
    match digits.parse::<u64>() {
        Ok(n) if n > 0 && digits.bytes().all(|b| b.is_ascii_digit()) => (base.to_string(), Some(n * mult)),
        _ => (id.to_string(), None),
    }
}

/// `{GrokBase}/models` 的应答可能是 OpenAI 列表（`data[]`）、`{models:[…]}`、
/// `{models:{id:{…}}}`（CLI 缓存的形态）或裸数组；条目可能包一层 `info`。
fn model_entries(root: &Value) -> Vec<Value> {
    let from = |v: &Value| -> Option<Vec<Value>> {
        match v {
            Value::Array(a) => Some(a.clone()),
            Value::Object(o) => Some(
                o.iter()
                    .map(|(k, v)| {
                        let mut v = v.clone();
                        if let Some(obj) = v.as_object_mut() {
                            if !obj.contains_key("info") && !obj.contains_key("id") {
                                obj.insert("id".into(), Value::String(k.clone()));
                            }
                        }
                        v
                    })
                    .collect(),
            ),
            _ => None,
        }
    };
    if root.is_array() {
        return from(root).unwrap_or_default();
    }
    for key in ["data", "models"] {
        if let Some(list) = root.get(key).and_then(from) {
            return list;
        }
    }
    Vec::new()
}

fn parse_models(body: &[u8]) -> Vec<VendorRow> {
    let Ok(root) = serde_json::from_slice::<Value>(body) else { return Vec::new() };
    let mut rows = Vec::new();
    for entry in model_entries(&root) {
        let info = entry.get("info").filter(|i| i.is_object()).unwrap_or(&entry);
        let id = {
            let i = str_field(info, "id");
            if i.is_empty() {
                str_field(info, "model")
            } else {
                i
            }
        };
        if id.is_empty() || info.get("hidden").and_then(|h| h.as_bool()) == Some(true) {
            continue;
        }
        let windows: Vec<u64> = info
            .get("context_windows")
            .and_then(|w| w.as_array())
            .map(|l| l.iter().filter_map(|x| x.as_u64()).filter(|n| *n > 0).collect())
            .unwrap_or_default();
        let context = info
            .get("context_window")
            .and_then(|c| c.as_u64())
            .filter(|c| *c > 0)
            .or_else(|| windows.first().copied())
            .unwrap_or(0);
        let max_output = info.get("max_completion_tokens").and_then(|m| m.as_u64()).unwrap_or(0);
        let images = info
            .get("input_modalities")
            .and_then(|v| v.as_array())
            .is_some_and(|l| l.iter().any(|x| x.as_str() == Some("image")));
        let levels: Vec<String> = info
            .get("reasoning_efforts")
            .and_then(|v| v.as_array())
            .map(|l| {
                l.iter()
                    .filter_map(|x| {
                        x.get("value").or_else(|| x.get("id")).or(Some(x)).and_then(|e| e.as_str()).map(String::from)
                    })
                    .collect()
            })
            .unwrap_or_default();
        let name = {
            let n = str_field(info, "name");
            if n.is_empty() {
                id.clone()
            } else {
                n
            }
        };
        let mk = |row_id: String, display: String, window: u64| {
            let mut raw = json!({ "id": row_id, "display_name": display });
            if !levels.is_empty() {
                raw["reasoning_levels"] = json!(levels);
            }
            VendorRow { id: row_id, context_window: window, max_output, images, raw }
        };
        rows.push(mk(id.clone(), name.clone(), context));
        // 同一模型的其它窗口：`grok-4.7@500k`（插件 windowsOf）。
        for w in windows.iter().copied().filter(|w| *w != context) {
            let suffix = window_suffix(w);
            rows.push(mk(format!("{id}@{suffix}"), format!("{name} ({suffix})"), w));
        }
    }
    rows
}

fn fetch_models(account: &Account) -> Result<Vec<VendorRow>, String> {
    let url = format!("{}/models", base_url().trim_end_matches('/'));
    let response = sign(super::outbound_agent().get(&url), account, &client_version())
        .set("Accept", "application/json")
        .timeout(FETCH_TIMEOUT)
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("Grok models: HTTP {code}"),
            other => format!("Grok models: {other}"),
        })?;
    let mut buf = Vec::new();
    response.into_reader().take(16 << 20).read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let rows = parse_models(&buf);
    if rows.is_empty() {
        return Err("Grok listed no models".into());
    }
    Ok(rows)
}

/// `/v1/models` 的 grok 行（30s 缓存、失败 60s 退避、失败回旧缓存）。没有账号 /
/// 令牌过期 / 用户自有 `grok` 来源 → 空（过期时有旧缓存就继续给旧的）。
pub fn listed_rows(providers: &[Provider]) -> Vec<VendorRow> {
    if providers.iter().any(|p| p.id == GROK_ID) {
        return Vec::new();
    }
    let Some(account) = discover() else { return Vec::new() };
    {
        let state = lock_state();
        if let Some((at, rows)) = state.cache.as_ref() {
            if at.elapsed() < CACHE_TTL {
                return rows.clone();
            }
        }
        let stale = state.cache.as_ref().map(|c| c.1.clone()).unwrap_or_default();
        if account.expired() || state.backoff_until.is_some_and(|u| Instant::now() < u) {
            return stale;
        }
    }
    match fetch_models(&account) {
        Ok(rows) => {
            let mut state = lock_state();
            state.backoff_until = None;
            state.cache = Some((Instant::now(), rows.clone()));
            rows
        }
        Err(_) => {
            let mut state = lock_state();
            state.backoff_until = Some(Instant::now() + FAIL_BACKOFF);
            state.cache.as_ref().map(|c| c.1.clone()).unwrap_or_default()
        }
    }
}

/// 整串 `grok/<id>` 是不是已知模型（纯内存，组成员后缀拆分用）。
fn knows(whole: &str) -> bool {
    let Some(id) = whole.strip_prefix("grok/") else { return false };
    lock_state().cache.as_ref().is_some_and(|c| c.1.iter().any(|r| r.id == id))
}

fn levels_of(id: &str) -> Vec<String> {
    let state = lock_state();
    let Some(row) = state.cache.as_ref().and_then(|c| c.1.iter().find(|r| r.id == id)) else {
        return Vec::new();
    };
    row.raw
        .get("reasoning_levels")
        .and_then(|v| v.as_array())
        .map(|l| l.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default()
}

// =====================================================================
// 额度
// =====================================================================

/// `end`（RFC3339 串或 unix 秒/毫秒数）→ 秒级 RFC3339 UTC。
fn reset_time(end: &Value) -> Option<String> {
    let ts = match end {
        Value::String(s) => chrono::DateTime::parse_from_rfc3339(s.trim()).ok()?.timestamp(),
        Value::Number(n) => {
            let raw = n.as_i64()?;
            if raw > 1_000_000_000_000 {
                raw / 1000
            } else {
                raw
            }
        }
        _ => return None,
    };
    let t = chrono::DateTime::from_timestamp(ts, 0)?;
    Some(t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// 订阅额度行（C3）。没账号 / 过期 / 用户自有 `grok` 来源 / 请求失败 / 读不懂
/// → None（展示面，安静跳过）。`creditUsagePercent` 已是 0–100（实测 `1.0` = 1%）。
pub fn quota_row(providers: &[Provider]) -> Option<Value> {
    if providers.iter().any(|p| p.id == GROK_ID) {
        return None;
    }
    let account = discover()?;
    if account.expired() {
        return None;
    }
    let url = format!("{}/billing?format=credits", base_url().trim_end_matches('/'));
    let response = sign(super::outbound_agent().get(&url), &account, &client_version())
        .set("Accept", "application/json")
        .timeout(FETCH_TIMEOUT)
        .call()
        .ok()?;
    let mut buf = Vec::new();
    response.into_reader().take(1 << 20).read_to_end(&mut buf).ok()?;
    let data: Value = serde_json::from_slice(&buf).ok()?;
    let config = data.get("config").filter(|c| c.is_object()).unwrap_or(&data);
    let used = config.get("creditUsagePercent").and_then(|p| p.as_f64())?.clamp(0.0, 100.0);
    let period = config.get("currentPeriod").filter(|p| p.is_object());
    let kind = period
        .and_then(|p| p.get("type"))
        .and_then(|t| t.as_str())
        .or_else(|| config.get("periodType").and_then(|t| t.as_str()))
        .unwrap_or("")
        .to_ascii_lowercase();
    let name = if kind.contains("monthly") {
        "month"
    } else if kind.contains("weekly") || kind.is_empty() {
        "7 days"
    } else {
        "Allowance"
    };
    let mut window = json!({ "name": name, "used": used, "remaining": 100.0 - used });
    let end = period
        .and_then(|p| p.get("end"))
        .filter(|e| !e.is_null())
        .or_else(|| config.get("billingPeriodEnd"))
        .or_else(|| config.get("billing_period_end"));
    if let Some(at) = end.and_then(reset_time) {
        window["resetsAt"] = json!(at);
    }
    let mut row = json!({
        "provider": GROK_ID,
        "name": QUOTA_NAME,
        "kind": "subscription",
        "windows": [window],
    });
    if !account.email.is_empty() {
        row["user"] = json!(account.email);
    }
    let plan = str_field(&data, "subscription_tier");
    if !plan.is_empty() {
        row["plan"] = json!(plan);
    }
    Some(row)
}

// =====================================================================
// 请求整形（Go grokShaped / grokFlat / ObjectRoot / grokAgentMessage 的移植）
// =====================================================================

/// Grok 后端收的工具类型；其它（Codex 的 custom/apply_patch、namespace）整单拒。
const GROK_TOOLS: [&str; 11] = [
    "function",
    "web_search",
    "x_search",
    "image_generation",
    "collections_search",
    "file_search",
    "code_execution",
    "code_interpreter",
    "mcp",
    "shell",
    "tool_search",
];

/// Codex Responses Lite 自己的 namespace，Codex 当它不存在。
const LITE_NAMESPACE: &str = "functions";

/// Go `FlatName`：`namespace__name`，超 64 字符截断并加哈希。
fn flat_name(namespace: &str, name: &str) -> String {
    let flat = format!("{namespace}__{name}");
    if flat.len() <= 64 {
        return flat;
    }
    let mut h = Sha256::new();
    h.update(namespace.as_bytes());
    h.update([0u8]);
    h.update(name.as_bytes());
    let sum = h.finalize();
    let hex: String = sum[..4].iter().map(|b| format!("{b:02x}")).collect();
    // 在 char 边界上截（工具名一般是 ASCII，防御一下）
    let mut cut = 55;
    while !flat.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}_{hex}", &flat[..cut])
}

/// Go `flatCall`：带 `namespace` 的调用/选择项改叫扁平名；是不是这种项。
fn flat_call(it: &mut Map<String, Value>) -> bool {
    let Some(space) = it.get("namespace").and_then(|n| n.as_str()).map(String::from) else {
        return false;
    };
    it.remove("namespace");
    let name = it.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
    if !name.is_empty() && !space.is_empty() && space != LITE_NAMESPACE {
        it.insert("name".into(), Value::String(flat_name(&space, &name)));
    }
    true
}

/// Go `grokFlat`：namespace 里的 function 各自成一个扁平工具；其它的丢。
fn grok_flat(ns: &Map<String, Value>) -> Vec<Value> {
    let space = ns.get("name").and_then(|n| n.as_str()).unwrap_or("");
    let mut out = Vec::new();
    for n in ns.get("tools").and_then(|t| t.as_array()).into_iter().flatten() {
        let Some(nm) = n.as_object() else { continue };
        let name = nm.get("name").and_then(|x| x.as_str()).unwrap_or("");
        if nm.get("type").and_then(|t| t.as_str()) != Some("function") || name.is_empty() {
            continue;
        }
        let mut nm = nm.clone();
        if !space.is_empty() && space != LITE_NAMESPACE {
            nm.insert("name".into(), Value::String(flat_name(space, name)));
        }
        out.push(Value::Object(nm));
    }
    out
}

/// Go `grokRef`：分支本身，或它的本地 `$ref` 在根的 `$defs`/`definitions` 里
/// 指的那个。
fn grok_ref(root: &Value, b: &Value) -> Option<Map<String, Value>> {
    let bm = b.as_object()?;
    let Some(r) = bm.get("$ref").and_then(|r| r.as_str()) else { return Some(bm.clone()) };
    for k in ["$defs", "definitions"] {
        if let Some(name) = r.strip_prefix(&format!("#/{k}/")) {
            return root.get(k)?.get(name)?.as_object().cloned();
        }
    }
    None
}

fn nested_union(b: &Map<String, Value>) -> bool {
    ["anyOf", "oneOf", "allOf"].iter().any(|k| b.contains_key(*k))
}

fn append_distinct(vs: &mut Vec<Value>, v: &Value) {
    if !vs.iter().any(|w| w == v) {
        vs.push(v.clone());
    }
}

/// Go `ObjectRoot`：工具入参 schema 根上不许有 anyOf/oneOf/allOf——合成一个
/// 普通 object。返回是否改了东西。
fn object_root(ps: &mut Map<String, Value>) -> bool {
    let root = Value::Object(ps.clone());
    object_root_in(&root, ps, 0)
}

fn object_root_in(root: &Value, ps: &mut Map<String, Value>, depth: usize) -> bool {
    let unions = nested_union(ps);
    if ps.get("type").and_then(|t| t.as_str()) == Some("object") && !unions {
        return false;
    }
    let mut props: Map<String, Value> =
        ps.get("properties").and_then(|p| p.as_object()).cloned().unwrap_or_default();
    let mut required: Vec<Value> = ps.get("required").and_then(|r| r.as_array()).cloned().unwrap_or_default();
    let all_of: Vec<Value> = ps.get("allOf").and_then(|a| a.as_array()).cloned().unwrap_or_default();
    for b in &all_of {
        let Some(bm) = grok_ref(root, b) else { continue };
        if let Some(bp) = bm.get("properties").and_then(|p| p.as_object()) {
            for (k, v) in bp {
                props.entry(k.clone()).or_insert_with(|| v.clone());
            }
        }
        if let Some(br) = bm.get("required").and_then(|r| r.as_array()) {
            required.extend(br.iter().cloned());
        }
    }
    ps.remove("allOf");
    let mut branches: Vec<Map<String, Value>> = Vec::new();
    for k in ["anyOf", "oneOf"] {
        let list = match ps.remove(k) {
            Some(Value::Array(a)) => a,
            _ => Vec::new(),
        };
        for b in &list {
            let Some(mut bm) = grok_ref(root, b) else { continue };
            if nested_union(&bm) && depth < 8 {
                object_root_in(root, &mut bm, depth + 1);
            }
            if bm.get("type").and_then(|t| t.as_str()) != Some("object") && !bm.contains_key("properties") {
                continue;
            }
            branches.push(bm);
        }
    }
    let mut common: Vec<Value> = Vec::new();
    let mut from: HashMap<String, Vec<Value>> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for (i, b) in branches.iter().enumerate() {
        if let Some(bp) = b.get("properties").and_then(|p| p.as_object()) {
            let mut keys: Vec<&String> = bp.keys().collect();
            keys.sort();
            for k in keys {
                if props.contains_key(k) {
                    continue;
                }
                if !from.contains_key(k) {
                    order.push(k.clone());
                }
                append_distinct(from.entry(k.clone()).or_default(), &bp[k]);
            }
        }
        let br: Vec<Value> = b.get("required").and_then(|r| r.as_array()).cloned().unwrap_or_default();
        if i == 0 {
            common.extend(br);
            continue;
        }
        common.retain(|r| br.contains(r));
    }
    for k in order {
        let mut vs = from.remove(&k).unwrap_or_default();
        if vs.len() == 1 {
            props.insert(k, vs.remove(0));
        } else {
            props.insert(k, json!({ "anyOf": vs }));
        }
    }
    let mut seen: Vec<String> = Vec::new();
    let mut merged: Vec<Value> = Vec::new();
    for r in required.into_iter().chain(common) {
        if let Some(name) = r.as_str() {
            if seen.iter().any(|s| s == name) {
                continue;
            }
            seen.push(name.to_string());
        }
        merged.push(r);
    }
    ps.insert("type".into(), Value::String("object".into()));
    ps.insert("properties".into(), Value::Object(props));
    if merged.is_empty() {
        ps.remove("required");
    } else {
        ps.insert("required".into(), Value::Array(merged));
    }
    true
}

/// Go `grokAgentMessage`：Codex 的纯 agent_message 改成 user 消息（发件人/收件人
/// 写在前头）；加密的、空的、含别种分片的原样留着。
fn grok_agent_message(it: &Value) -> Option<Value> {
    let im = it.as_object()?;
    if im.get("type").and_then(|t| t.as_str()) != Some("agent_message") || im.contains_key("encrypted_content") {
        return None;
    }
    let parts = im.get("content")?.as_array()?;
    if parts.is_empty() {
        return None;
    }
    for p in parts {
        let pm = p.as_object()?;
        if pm.contains_key("encrypted_content") {
            return None;
        }
        let ty = pm.get("type").and_then(|t| t.as_str());
        let is_text = pm.get("text").is_some_and(|t| t.is_string());
        let is_url = pm.get("image_url").is_some_and(|t| t.is_string());
        if !(ty == Some("input_text") && is_text || ty == Some("input_image") && is_url) {
            return None;
        }
    }
    let from = im.get("author").and_then(|a| a.as_str()).unwrap_or("");
    let to = im.get("recipient").and_then(|a| a.as_str()).unwrap_or("");
    let header = match (from.is_empty(), to.is_empty()) {
        (false, false) => format!("From {from} to {to}"),
        (false, true) => format!("From {from}"),
        (true, false) => format!("To {to}"),
        (true, true) => String::new(),
    };
    let mut content: Vec<Value> = Vec::new();
    if !header.is_empty() {
        content.push(json!({ "type": "input_text", "text": format!("{header}\n\n") }));
    }
    content.extend(parts.iter().cloned());
    Some(json!({ "type": "message", "role": "user", "content": content }))
}

/// Go `grokBody`（非插件口径：不支持的 tool_choice 一并去掉）+ 插件做的两件
/// 小事（`include` 去掉 `output.encrypted_content`；Grok 拒收）。
fn shape_body(mut v: Value) -> Value {
    let Some(m) = v.as_object_mut() else { return v };
    // tools
    let mut kept: Vec<Value> = Vec::new();
    if let Some(Value::Array(tools)) = m.remove("tools") {
        for mut t in tools {
            if let Some(tm) = t.as_object_mut() {
                let ty = tm.get("type").and_then(|x| x.as_str()).unwrap_or("").to_string();
                if ty == "namespace" {
                    kept.extend(grok_flat(tm));
                    continue;
                }
                if !GROK_TOOLS.contains(&ty.as_str()) {
                    continue;
                }
                tm.remove("external_web_access");
            }
            kept.push(t);
        }
        for t in kept.iter_mut() {
            if let Some(tm) = t.as_object_mut() {
                if tm.get("type").and_then(|x| x.as_str()) == Some("function") {
                    if let Some(Value::Object(ps)) = tm.get_mut("parameters") {
                        object_root(ps);
                    }
                }
            }
            // Go grokToolPatterns（grok_schema.go :25，在整形后的最终体上跑）：
            // schema 里的 ECMA pattern 转写成 Rust regex 认的形式。这条路径
            // 全是 Grok 的模型（grokModel 恒真），不用再按模型判。
            fix_tool_patterns(t);
        }
        if let Some(Value::Object(tc)) = m.get_mut("tool_choice") {
            flat_call(tc);
            let ty = tc.get("type").and_then(|x| x.as_str()).unwrap_or("").to_string();
            if !GROK_TOOLS.contains(&ty.as_str()) {
                m.remove("tool_choice");
            }
        }
        m.insert("tools".into(), Value::Array(kept.clone()));
    }
    // 没有工具了：tool_choice 一并去（后端会因此拒整个请求）
    if kept.is_empty() {
        m.remove("tool_choice");
    }
    if let Some(Value::Array(input)) = m.get_mut("input") {
        for it in input.iter_mut() {
            if let Some(msg) = grok_agent_message(it) {
                *it = msg;
            }
            if let Some(im) = it.as_object_mut() {
                flat_call(im);
                if im.get("type").and_then(|t| t.as_str()) == Some("reasoning")
                    && im.get("content").is_some_and(|c| c.is_null())
                {
                    im.remove("content");
                }
            }
        }
    }
    if let Some(Value::Array(inc)) = m.get_mut("include") {
        inc.retain(|i| i.as_str() != Some("output.encrypted_content"));
        if inc.is_empty() {
            m.remove("include");
        }
    }
    v
}

// =====================================================================
// namespace 还原（Go grok_namespace.go 的移植）
// =====================================================================

/// Go ir.go nsTool 的最小面（Search/Custom 是 Codex 侧的，Grok 路径用不到）。
#[derive(Clone)]
struct NsTool {
    namespace: String,
    name: String,
}

/// Go `namespacedIn`（grok_namespace.go :31）：Responses 请求里声明/调用过的
/// namespace 工具，按 Grok 收到的扁平名索引。tools 的 namespace 条目（内层
/// function）+ input 里带 `namespace` 字段的调用项；空 namespace 与
/// `functions`（Codex Lite 自己的，Codex 当它不存在）不算。
fn namespaced_in(body: &Value) -> HashMap<String, NsTool> {
    let mut named: HashMap<String, NsTool> = HashMap::new();
    let mut add = |space: &str, name: &str| {
        if !space.is_empty() && space != LITE_NAMESPACE && !name.is_empty() {
            named.insert(flat_name(space, name), NsTool { namespace: space.to_string(), name: name.to_string() });
        }
    };
    for t in body.get("tools").and_then(|t| t.as_array()).into_iter().flatten() {
        if t.get("type").and_then(|x| x.as_str()) != Some("namespace") {
            continue;
        }
        let space = t.get("name").and_then(|n| n.as_str()).unwrap_or("");
        for nt in t.get("tools").and_then(|x| x.as_array()).into_iter().flatten() {
            if nt.get("type").and_then(|x| x.as_str()) == Some("function") {
                add(space, nt.get("name").and_then(|n| n.as_str()).unwrap_or(""));
            }
        }
    }
    // input 是字符串（正文单独来）时不解析——Go json.Unmarshal 到列表失败同款。
    for it in body.get("input").and_then(|i| i.as_array()).into_iter().flatten() {
        add(
            it.get("namespace").and_then(|n| n.as_str()).unwrap_or(""),
            it.get("name").and_then(|n| n.as_str()).unwrap_or(""),
        );
    }
    named
}

/// Go `nativeSealedAgentPayload`（codex_backend.go :53：
/// `^gAAAAA[A-Za-z0-9_-]+={0,2}$`）的纯字符串版。
fn native_sealed_payload(v: &str) -> bool {
    let Some(rest) = v.strip_prefix("gAAAAA") else { return false };
    let core = rest.trim_end_matches('=');
    if rest.len() - core.len() > 2 || core.is_empty() {
        return false;
    }
    core.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Go `sealedArgs`（grok_namespace.go :232）：参数对象里哪些键的值是 ChatGPT
/// 后端封印的 `gAAAAA…` 令牌（键名排序）；参数还不是 JSON 对象 / 还没来全
/// （流式 output_item.added）= None，调用原样走。
fn sealed_args(args: &str) -> Option<Vec<String>> {
    let parsed: Value = serde_json::from_str(args.trim()).ok()?;
    let obj = parsed.as_object()?;
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    let sealed: Vec<String> = keys
        .into_iter()
        .filter(|k| obj[*k].as_str().is_some_and(native_sealed_payload))
        .cloned()
        .collect();
    Some(sealed)
}

/// Go `nsTidy.call`（:187）+ `callTo` 的普通 function 分支（responses.go :2163）：
/// 把应答里的 function_call 翻回 Codex 认的形状。返回是否改过。
fn restore_call(it: &mut Value, named: &HashMap<String, NsTool>) -> bool {
    let Some(im) = it.as_object_mut() else { return false };
    let ty = im.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
    if ty != "function_call" && ty != "custom_tool_call" {
        return false;
    }
    let name = im.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
    let ns = im.get("namespace").and_then(|n| n.as_str()).unwrap_or("").to_string();
    if !ns.is_empty() {
        // 上游自己带了 namespace（被当 Codex 问的厂商，#3183）：名字已经是对的，
        // 只在缺 `encrypted_function_args` 清单时补——没有清单 Codex 会把参数
        // 整个当密文，明文任务就到不了子代理。
        if im.contains_key("encrypted_function_args") {
            return false;
        }
        let known = named.values().any(|q| q.namespace == ns && q.name == name);
        if !known {
            return false;
        }
        let args = im.get("arguments").and_then(|a| a.as_str()).unwrap_or("");
        if let Some(sealed) = sealed_args(args) {
            im.insert("encrypted_function_args".into(), json!(sealed));
            return true;
        }
        return false;
    }
    // custom_tool_call 只有改过名的才能翻（back 表）；Grok 路径没有改名。
    if ty != "function_call" {
        return false;
    }
    let Some(q) = named.get(&name) else { return false };
    let name = q.name.clone();
    let namespace = q.namespace.clone();
    let im = it.as_object_mut().unwrap();
    im.insert("name".into(), Value::String(name));
    im.insert("namespace".into(), Value::String(namespace));
    // Go callTo：空清单 = 「没有参数被封印」——magpie 侧不封参数，但 Codex 读
    // 不到这个字段就按封印解读（#3183）。
    im.insert("encrypted_function_args".into(), json!([]));
    true
}

/// Go `nsTidy.restore`（:141）：一个事件（或整个应答对象）里的调用翻回。
/// 返回改过后的新 JSON；没改过 = None（原字节原样走）。
fn restore_event(data: &[u8], named: &HashMap<String, NsTool>) -> Option<Vec<u8>> {
    let mut ev: Value = serde_json::from_slice(data).ok()?;
    let mut changed = false;
    if let Some(item) = ev.get_mut("item") {
        changed |= restore_call(item, named);
    }
    let whole = ev.get("object").and_then(|o| o.as_str()) == Some("response");
    if ev.get("response").is_some() || whole {
        let res = if whole { &mut ev } else { ev.get_mut("response").unwrap() };
        if let Some(out) = res.get_mut("output").and_then(|o| o.as_array_mut()) {
            for item in out.iter_mut() {
                changed |= restore_call(item, named);
            }
        }
    }
    if !changed {
        return None;
    }
    serde_json::to_vec(&ev).ok()
}

/// Go `nsTidy.carries`（:176）：字节里可能出现一个 function_call 项
/// （`"function_call"` 带引号的 JSON 字符串——`function_call_arguments` 这类
/// 不带尾引号的不算）。back 表为空（Grok 路径），custom_tool_call 不认。
fn carries(b: &[u8]) -> bool {
    b.windows(15).any(|w| w == b"\"function_call\"")
}

/// Go `nsTidy`（grok_namespace.go :76）的 Reader 形态：SSE 按行过，带
/// function_call 的 `data:` 行翻回命名空间形状，其余原样。非 SSE 的整体
/// 应答（聚合对象）也走同一条行逻辑——Grok 路径恒流式，这里只需要 SSE 面。
struct NsTidyReader<R: Read> {
    inner: R,
    named: HashMap<String, NsTool>,
    /// 还没到行尾的半行。
    pending: Vec<u8>,
    /// 已还原、待读出的字节。
    ready: Vec<u8>,
    ready_at: usize,
    eof: bool,
}

impl<R: Read> NsTidyReader<R> {
    fn new(inner: R, named: HashMap<String, NsTool>) -> NsTidyReader<R> {
        NsTidyReader { inner, named, pending: Vec::new(), ready: Vec::new(), ready_at: 0, eof: false }
    }

    /// Go `lines`（:111）：一段**以行为单位**的字节逐行翻回。
    fn tidy_lines(&self, b: &[u8]) -> Vec<u8> {
        if !carries(b) {
            return b.to_vec();
        }
        let mut out = Vec::with_capacity(b.len());
        let mut rest = b;
        while !rest.is_empty() {
            let line = match rest.iter().position(|&c| c == b'\n') {
                Some(i) => {
                    let l = &rest[..i + 1];
                    rest = &rest[i + 1..];
                    l
                }
                None => {
                    let l = rest;
                    rest = &[];
                    l
                }
            };
            // Go bytes.TrimRight(line, "\r\n")
            let body_end = line.len() - line.iter().rev().take_while(|&&c| c == b'\r' || c == b'\n').count();
            let body = &line[..body_end];
            let Some(data) = body.strip_prefix(b"data:") else {
                out.extend_from_slice(line);
                continue;
            };
            if !carries(data) {
                out.extend_from_slice(line);
                continue;
            }
            match restore_event(data, &self.named) {
                Some(nb) => {
                    out.extend_from_slice(b"data: ");
                    out.extend_from_slice(&nb);
                    out.extend_from_slice(&line[body_end..]);
                }
                None => out.extend_from_slice(line),
            }
        }
        out
    }
}

impl<R: Read> Read for NsTidyReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            if self.ready_at < self.ready.len() {
                let n = buf.len().min(self.ready.len() - self.ready_at);
                buf[..n].copy_from_slice(&self.ready[self.ready_at..self.ready_at + n]);
                self.ready_at += n;
                return Ok(n);
            }
            self.ready.clear();
            self.ready_at = 0;
            if self.eof {
                if self.pending.is_empty() {
                    return Ok(0);
                }
                // Go flush：尾部没换行的最后一段也过一遍。
                let rest = std::mem::take(&mut self.pending);
                self.ready = self.tidy_lines(&rest);
                continue;
            }
            let mut chunk = [0u8; 16384];
            let n = self.inner.read(&mut chunk)?;
            if n == 0 {
                self.eof = true;
                continue;
            }
            self.pending.extend_from_slice(&chunk[..n]);
            // 只处理到最后一行行尾（Go write 的 LastIndexByte('\n')），半行留着。
            let cut = self.pending.iter().rposition(|&b| b == b'\n').map(|i| i + 1).unwrap_or(0);
            if cut > 0 {
                let head: Vec<u8> = self.pending.drain(..cut).collect();
                self.ready = self.tidy_lines(&head);
            }
        }
    }
}

/// named 为空 = 没有可翻的调用，裸管直通（零开销）。
fn tidy(reader: impl Read + Send + 'static, named: &HashMap<String, NsTool>) -> Box<dyn Read + Send> {
    if named.is_empty() {
        Box::new(reader)
    } else {
        Box::new(NsTidyReader::new(reader, named.clone()))
    }
}

// =====================================================================
// schema pattern 转写（Go grok_schema.go + deepseek_schema.go toolPatterns）
// =====================================================================

/// Go `ecmaClassForRust`（grok_schema.go :41）——1:1 字符串改写：ECMA 正则
/// 字符类里的 `[`（Rust regex 当嵌套类）和 `&&`/`--`/`~~` 的第二个字符（Rust
/// regex 当集合运算）转义；转义对原样放行，类外的字符不动。xAI 的校验器用
/// Rust regex 语法读 pattern，ECMA 语义里这些就是普通字符，转义后对 ECMA
/// 也是同一个字符（#2317：Claude Code 的 `^[^\s[\]]+$` 让 Grok 整单被拒）。
fn ecma_class_for_rust(pat: &str) -> String {
    let src = pat.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(src.len() + 8);
    let mut class = false;
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        if c == b'\\' && i + 1 < src.len() {
            out.push(c);
            out.push(src[i + 1]);
            i += 2;
            continue;
        }
        if !class && c == b'[' {
            class = true;
        } else if class && c == b']' {
            class = false;
        } else if class && c == b'[' {
            out.extend_from_slice(b"\\[");
            i += 1;
            continue;
        } else if class && (c == b'&' || c == b'-' || c == b'~') && i + 1 < src.len() && src[i + 1] == c {
            out.push(c);
            out.push(b'\\');
            out.push(c);
            i += 2;
            continue;
        }
        out.push(c);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
}

/// Go `collectPatterns`（deepseek_schema.go :83）的遍历形状：只在 schema 语境
/// 里改 `"pattern"` 字符串；`properties`/`$defs`/… 的每个值、`items`/`anyOf`/…
/// 整值递归；非 schema 语境里 `parameters`/`input_schema` 进 schema、`function`
/// 保持非 schema（chat 形状的嵌套）。Go 为保字节做了原位拼接，这里本来就要
/// 重新序列化 Value，改值即可。
fn fix_tool_patterns(node: &mut Value) {
    fn walk(node: &mut Value, in_schema: bool) {
        match node {
            Value::Object(m) => {
                for (k, val) in m.iter_mut() {
                    if in_schema {
                        if k == "pattern" {
                            if let Value::String(s) = val {
                                let fixed = ecma_class_for_rust(s);
                                if fixed != *s {
                                    *val = Value::String(fixed);
                                }
                            }
                        }
                        match k.as_str() {
                            "properties" | "$defs" | "definitions" | "dependentSchemas" | "patternProperties" => {
                                if let Value::Object(pm) = val {
                                    for (_, sv) in pm.iter_mut() {
                                        walk(sv, true);
                                    }
                                }
                            }
                            "items" | "additionalProperties" | "contains" | "propertyNames" | "not"
                            | "if" | "then" | "else" | "allOf" | "anyOf" | "oneOf" | "prefixItems"
                            | "unevaluatedProperties" | "unevaluatedItems" | "contentSchema" => {
                                walk(val, true);
                            }
                            _ => {}
                        }
                    } else {
                        match k.as_str() {
                            "input_schema" | "parameters" => walk(val, true),
                            "function" => walk(val, false),
                            _ => {}
                        }
                    }
                }
            }
            Value::Array(a) => {
                for it in a.iter_mut() {
                    walk(it, in_schema);
                }
            }
            _ => {}
        }
    }
    walk(node, false);
}

/// 强度夹进模型自报的档位（Grok 的表外值整单 400）：`max`→`xhigh`，其余取最近
/// 的低档/高档。档位不详就原样。
fn clamp_effort(effort: &str, levels: &[String]) -> Option<String> {
    if levels.is_empty() || levels.iter().any(|l| l == effort) {
        return None;
    }
    let rank = |w: &str| super::effort::LEVELS.iter().position(|l| *l == w);
    let want = rank(effort)?;
    let mut ranked: Vec<(usize, &String)> = levels.iter().filter_map(|l| rank(l).map(|r| (r, l))).collect();
    ranked.sort();
    let pick = ranked
        .iter()
        .rev()
        .find(|(r, _)| *r <= want)
        .or_else(|| ranked.first())
        .map(|(_, l)| (*l).clone());
    pick
}

// =====================================================================
// 对话路径
// =====================================================================

fn error_reply(status: u16, message: &str) -> Reply {
    Reply::error(status, message)
}

/// anthropic 错误信封（`/v1/messages` 客户端读这个形状）。
fn anthropic_error(status: u16, message: &str) -> Reply {
    let ty = match status {
        400 | 422 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        429 => "rate_limit_error",
        s if s >= 500 => "api_error",
        _ => "invalid_request_error",
    };
    Reply::json(status, &json!({ "type": "error", "error": { "type": ty, "message": message } }))
}

fn vendor_message(status: u16, body: &[u8]) -> String {
    if let Ok(v) = serde_json::from_slice::<Value>(body) {
        for p in ["/error/message", "/message", "/detail", "/error"] {
            if let Some(s) = v.pointer(p).and_then(|m| m.as_str()).filter(|s| !s.is_empty()) {
                return s.to_string();
            }
        }
    }
    let text = String::from_utf8_lossy(body);
    let t = text.trim();
    if t.is_empty() {
        format!("Grok: HTTP {status}")
    } else {
        t.chars().take(500).collect()
    }
}

/// 把上游 SSE 读成一个最终 response 对象（客户端要非流式时）。
fn aggregate_sse(reader: impl Read) -> Result<Value, String> {
    let mut items: Vec<Value> = Vec::new();
    let mut final_response: Option<Value> = None;
    for line in BufReader::new(reader).lines() {
        let Ok(line) = line else { break };
        let Some(data) = line.strip_prefix("data:") else { continue };
        let Ok(event) = serde_json::from_str::<Value>(data.trim()) else { continue };
        match event.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "response.output_item.done" => {
                if let Some(item) = event.get("item") {
                    items.push(item.clone());
                }
            }
            "response.completed" | "response.incomplete" => {
                final_response = event.get("response").cloned();
                break;
            }
            "response.failed" => {
                let msg = event
                    .pointer("/response/error/message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("Grok response failed");
                return Err(msg.to_string());
            }
            "error" => {
                let msg = event
                    .get("message")
                    .or_else(|| event.pointer("/error/message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("Grok stream error");
                return Err(msg.to_string());
            }
            _ => {}
        }
    }
    let Some(mut response) = final_response else {
        return Err("Grok stream ended before response.completed".into());
    };
    let empty_output = response.get("output").and_then(|o| o.as_array()).map(|o| o.is_empty()).unwrap_or(true);
    if empty_output && !items.is_empty() {
        response["output"] = Value::Array(items);
    }
    Ok(response)
}

/// 发到 `{base}/responses`。成功 = 200 的响应（流）；上游 4xx/5xx 与传输失败
/// 变成 `(status, body)` 由调用方按各自协议说。
fn forward(account: &Account, body: &Value, window: Option<u64>) -> Result<ureq::Response, (u16, Vec<u8>)> {
    let url = format!("{}/responses", base_url().trim_end_matches('/'));
    let payload = serde_json::to_vec(body).unwrap_or_default();
    let mut request = sign(super::outbound_agent().post(&url), account, &client_version())
        .set("Content-Type", "application/json")
        .set("Accept", "text/event-stream")
        // Grok Build 的循环检查：复读的回复 ~1K token 后就停，不跑满输出上限。
        .set("x-grok-doom-loop-check", "1024")
        .set("x-grok-exact-repetition-check", "64");
    if let Some(w) = window {
        request = request.set("x-grok-context-window", &w.to_string());
    }
    let response = match request.send_bytes(&payload) {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            let mut buf = Vec::new();
            let _ = r.into_reader().take(1 << 20).read_to_end(&mut buf);
            return Err((code, buf));
        }
        Err(e) => return Err((502, format!("grok: {e}").into_bytes())),
    };
    if response.status() != 200 {
        let status = response.status();
        let mut buf = Vec::new();
        let _ = response.into_reader().take(1 << 20).read_to_end(&mut buf);
        return Err((status, buf));
    }
    Ok(response)
}

/// 整形好的 Responses 体：模型换裸名、恒流式（上游统一读 SSE，非流式客户端在
/// 本地聚合）、不存储。
fn finalize_body(mut v: Value, bare: &str) -> Value {
    if let Some(m) = v.as_object_mut() {
        m.insert("model".into(), Value::String(bare.to_string()));
        m.insert("stream".into(), Value::Bool(true));
        m.entry("store").or_insert(Value::Bool(false));
        // 档位夹进模型自报的菜单
        if let Some(effort) = m.get("reasoning").and_then(|r| r.get("effort")).and_then(|e| e.as_str()).map(String::from)
        {
            if let Some(fixed) = clamp_effort(&effort, &levels_of_bare(bare)) {
                m["reasoning"]["effort"] = Value::String(fixed);
            }
        }
    }
    shape_body(v)
}

/// 目录缓存里按（可能带 `@窗口` 的）id 找档位。
fn levels_of_bare(bare: &str) -> Vec<String> {
    let l = levels_of(bare);
    if !l.is_empty() {
        return l;
    }
    levels_of(&split_window(bare).0)
}

/// 路由层钩子：请求的模型若只能由合成 `grok` 来源接住，就在这里处理；否则
/// None（走原有路径）。
pub fn intercept(gateway: &super::Gateway, request: &Request, proto: Proto) -> Option<Reply> {
    let parsed: Value = serde_json::from_slice(&request.body).ok()?;
    let asked = parsed.get("model")?.as_str()?.to_string();
    let (groups, providers) = {
        let store = gateway.store.lock().ok()?;
        (store.groups().to_vec(), store.providers().to_vec())
    };
    let known = |id: &str| gateway.catalog.knows(&providers, id);
    // 真实来源（含用户自建的 grok）能接住的，一律走原路径。
    if super::groups::resolve_with(&groups, &providers, &asked, &known).is_some() {
        return None;
    }
    if providers.iter().any(|p| p.id == GROK_ID) {
        return None;
    }
    let synthetic = synthetic_provider()?;
    let mut with = providers.clone();
    with.push(synthetic);
    let known2 = |id: &str| knows(id) || gateway.catalog.knows(&with, id);
    let (provider, bare_asked, levels) = super::groups::resolve_with(&groups, &with, &asked, &known2)?;
    if provider != GROK_ID {
        return None;
    }
    let account = discover()?;
    let unauthorized = |proto: Proto| match proto {
        Proto::Anthropic => anthropic_error(401, EXPIRED_MESSAGE),
        _ => error_reply(401, EXPIRED_MESSAGE),
    };
    if account.expired() {
        return Some(unauthorized(proto));
    }
    // 成员强度后缀（`grok/grok-4.7:xhigh`）：与其它来源同一套语义。
    let body_bytes = if levels.is_empty() {
        request.body.clone()
    } else {
        super::effort::apply(proto, &request.body, &levels, &levels_of_bare(&bare_asked))
            .unwrap_or_else(|| request.body.clone())
    };
    let body: Value = serde_json::from_slice(&body_bytes).unwrap_or(parsed);
    let client_wants_stream = body.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    let include_usage = body.pointer("/stream_options/include_usage").and_then(|u| u.as_bool()).unwrap_or(false);
    let (bare, window) = split_window(&bare_asked);

    // 入站协议 → Responses 请求体
    let responses_body = match proto {
        Proto::Responses => body,
        Proto::Chat => match super::translate::chat_to_responses(&body) {
            Ok(v) => v,
            Err(e) => return Some(error_reply(400, &e)),
        },
        Proto::Anthropic => match super::translate::anthropic_to_responses(&body) {
            Ok(v) => v,
            Err(e) => return Some(anthropic_error(400, &e)),
        },
    };
    // namespace 索引要在压平（finalize_body→shape_body）之前建：出站压平、
    // 应答翻回（Go namespacedIn → nsTidy）。chat/anthropic 入站没有 namespace
    // 工具，named 为空 = 裸管直通。
    let named = namespaced_in(&responses_body);
    let shaped = finalize_body(responses_body, &bare);

    let response = match forward(&account, &shaped, window) {
        Ok(r) => r,
        // 厂商的 4xx/5xx 是合同面的一部分：Responses/Chat 原样取回；anthropic 换信封。
        Err((status, buf)) => {
            return Some(match proto {
                Proto::Anthropic => anthropic_error(status, &vendor_message(status, &buf)),
                _ => {
                    if serde_json::from_slice::<Value>(&buf).is_ok() {
                        Reply::Json { status, body: buf }
                    } else {
                        error_reply(status, &vendor_message(status, &buf))
                    }
                }
            })
        }
    };
    let back: super::translate::NamesBack = HashMap::new();
    Some(match proto {
        Proto::Responses => {
            if client_wants_stream {
                Reply::Stream {
                    status: 200,
                    content_type: "text/event-stream".into(),
                    reader: tidy(response.into_reader(), &named),
                }
            } else {
                match aggregate_sse(tidy(response.into_reader(), &named)) {
                    Ok(v) => Reply::json(200, &v),
                    Err(e) => error_reply(502, &e),
                }
            }
        }
        Proto::Chat => {
            if client_wants_stream {
                Reply::Stream {
                    status: 200,
                    content_type: "text/event-stream".into(),
                    reader: Box::new(super::translate::ResponsesToChatReader::new(
                        tidy(response.into_reader(), &named),
                        super::translate::ResponsesToChat::new(&asked, include_usage, back),
                    )),
                }
            } else {
                match aggregate_sse(tidy(response.into_reader(), &named)) {
                    Ok(v) => Reply::json(200, &super::translate::responses_object_to_chat(&v, &asked, &back)),
                    Err(e) => error_reply(502, &e),
                }
            }
        }
        Proto::Anthropic => {
            if client_wants_stream {
                // anthropic 的 message_delta 要 usage：中间的 chat 流恒带 usage 帧。
                let chat = super::translate::ResponsesToChatReader::new(
                    tidy(response.into_reader(), &named),
                    super::translate::ResponsesToChat::new(&asked, true, back),
                );
                Reply::Stream {
                    status: 200,
                    content_type: "text/event-stream".into(),
                    reader: Box::new(super::messages::AnthropicSseTranslator::new(Box::new(chat), &asked)),
                }
            } else {
                match aggregate_sse(tidy(response.into_reader(), &named)) {
                    Ok(v) => {
                        let chat = super::translate::responses_object_to_chat(&v, &asked, &back);
                        let bytes = serde_json::to_vec(&chat).unwrap_or_default();
                        match super::messages::chat_response_to_anthropic(&bytes, &asked) {
                            Ok(a) => Reply::json(200, &a),
                            Err(e) => anthropic_error(502, &e),
                        }
                    }
                    Err(e) => anthropic_error(502, &e),
                }
            }
        }
    })
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::super::Gateway;
    use super::*;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    static DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

    struct Recorded {
        path: String,
        headers: HashMap<String, String>,
        body: Vec<u8>,
    }

    /// 本地假后端：按路径前缀回固定应答；记录每个请求。
    struct Mock {
        base: String,
        hits: Arc<AtomicUsize>,
        seen: Arc<Mutex<Vec<Recorded>>>,
        routes: Arc<Mutex<Vec<(String, u16, String, String)>>>,
    }

    impl Mock {
        fn start() -> Mock {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let port = listener.local_addr().unwrap().port();
            let hits = Arc::new(AtomicUsize::new(0));
            let seen: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
            let routes: Arc<Mutex<Vec<(String, u16, String, String)>>> = Arc::new(Mutex::new(Vec::new()));
            let (h, s, r) = (hits.clone(), seen.clone(), routes.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    h.fetch_add(1, Ordering::SeqCst);
                    let (s, r) = (s.clone(), r.clone());
                    std::thread::spawn(move || {
                        let mut reader = BufReader::new(stream.try_clone().unwrap());
                        let mut request_line = String::new();
                        reader.read_line(&mut request_line).ok();
                        let path = request_line.split_whitespace().nth(1).unwrap_or("").to_string();
                        let mut headers = HashMap::new();
                        loop {
                            let mut line = String::new();
                            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                                break;
                            }
                            let t = line.trim_end();
                            if t.is_empty() {
                                break;
                            }
                            if let Some((k, v)) = t.split_once(':') {
                                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                            }
                        }
                        let len: usize = headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
                        let mut body = vec![0u8; len];
                        reader.read_exact(&mut body).ok();
                        let reply = r
                            .lock()
                            .unwrap()
                            .iter()
                            .find(|(prefix, ..)| path.starts_with(prefix.as_str()))
                            .map(|(_, status, ct, b)| (*status, ct.clone(), b.clone()))
                            .unwrap_or((404, "text/plain".into(), "no route".into()));
                        s.lock().unwrap().push(Recorded { path, headers, body });
                        let mut stream = stream;
                        let out = format!(
                            "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            reply.0,
                            reply.1,
                            reply.2.len(),
                            reply.2
                        );
                        let _ = stream.write_all(out.as_bytes());
                    });
                }
            });
            Mock { base: format!("http://127.0.0.1:{port}/v1"), hits, seen, routes }
        }

        fn route(&self, prefix: &str, status: u16, content_type: &str, body: &str) {
            let mut routes = self.routes.lock().unwrap();
            routes.retain(|(p, ..)| p != prefix);
            routes.push((prefix.to_string(), status, content_type.to_string(), body.to_string()));
        }

        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }

        fn last(&self, path_prefix: &str) -> (HashMap<String, String>, Vec<u8>) {
            let seen = self.seen.lock().unwrap();
            let r = seen.iter().rev().find(|r| r.path.starts_with(path_prefix)).expect("request seen");
            (r.headers.clone(), r.body.clone())
        }
    }

    struct World {
        dir: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for World {
        fn drop(&mut self) {
            *TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()) = None;
            reset_for_tests();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn world(providers_json: &str, base: &str) -> (Gateway, World) {
        let lock = crate::engine_gateway::TEST_SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_for_tests();
        let dir = std::env::temp_dir().join(format!(
            "marionette-engine-grok-{}-{}",
            std::process::id(),
            DIR_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("magpie")).unwrap();
        std::fs::create_dir_all(dir.join("home")).unwrap();
        std::fs::write(dir.join("magpie").join("providers.json"), providers_json).unwrap();
        *TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(Seam { base: base.to_string(), home: dir.join("home") });
        let prior = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let gateway = Gateway::open().expect("gateway");
        match &prior {
            Some(p) => std::env::set_var("XDG_CONFIG_HOME", p),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        (gateway, World { dir, _lock: lock })
    }

    fn rfc3339(offset_secs: i64) -> String {
        chrono::DateTime::from_timestamp(now_secs() + offset_secs, 0)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    }

    /// 假登录（全是测试假值）。键名模仿真实的 `<issuer>::<id>`。
    fn write_auth(world: &World, token: &str, email: &str, expires_in: i64) {
        let auth = json!({
            "https://auth.x.ai::test-id": {
                "key": token, "auth_mode": "oidc", "email": email, "user_id": "user-test",
                "refresh_token": "never-used-refresh", "expires_at": rfc3339(expires_in),
            }
        });
        std::fs::write(world.dir.join("home").join("auth.json"), serde_json::to_vec(&auth).unwrap()).unwrap();
    }

    const MODELS_BODY: &str = r#"{"models":{
        "grok-4.7":{"info":{"id":"grok-4.7","name":"Grok 4.7","context_window":256000,
            "context_windows":[256000,500000],"hidden":false,
            "reasoning_efforts":[{"id":"xhigh","value":"xhigh"},{"id":"high","value":"high"},
                                 {"id":"low","value":"low"}]}},
        "grok-secret":{"info":{"id":"grok-secret","name":"Hidden","context_window":1,"hidden":true}},
        "grok-4.6":{"info":{"id":"grok-4.6","name":"Grok 4.6","context_window":128000}}
    }}"#;

    fn post(path: &str, body: &Value) -> Request {
        Request { method: "POST".into(), path: path.into(), headers: Vec::new(), body: serde_json::to_vec(body).unwrap() }
    }

    fn get(path: &str) -> Request {
        Request { method: "GET".into(), path: path.into(), headers: Vec::new(), body: Vec::new() }
    }

    fn reply_json(reply: Reply) -> (u16, Value) {
        match reply {
            Reply::Json { status, body } => (status, serde_json::from_slice(&body).unwrap_or(Value::Null)),
            Reply::Stream { status, mut reader, .. } => {
                let mut buf = Vec::new();
                reader.read_to_end(&mut buf).unwrap();
                (status, Value::String(String::from_utf8_lossy(&buf).into_owned()))
            }
        }
    }

    const SSE_TEXT: &str = "event: response.created\n\
data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\"}}\n\n\
event: response.output_text.delta\n\
data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hi\"}\n\n\
event: response.output_item.done\n\
data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"hi\"}]}}\n\n\
event: response.completed\n\
data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":3,\"output_tokens\":1}}}\n\n";

    #[test]
    fn no_account_means_no_provider_rows_or_quota() {
        let mock = Mock::start();
        let (gateway, _w) = world(r#"{"providers":[]}"#, &mock.base);
        assert!(discover().is_none());
        assert!(synthetic_provider().is_none());
        assert!(listed_rows(&[]).is_empty());
        assert!(quota_row(&[]).is_none());
        let reply = super::super::routes::route(
            &post("/v1/responses", &json!({"model":"grok/grok-4.7","input":"hi"})),
            &gateway,
        );
        assert_eq!(reply_json(reply).0, 404);
        assert_eq!(mock.hits(), 0);
    }

    #[test]
    fn credential_discovery_is_sorted_first_with_key_and_never_written() {
        let mock = Mock::start();
        let (_gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        let auth = json!({
            "https://auth.x.ai::b": { "key": "tok-b", "email": "b@example.test", "expires_at": rfc3339(3600) },
            "https://auth.x.ai::a": { "key": "", "email": "a@example.test" },
            "https://auth.x.ai::c": { "key": "tok-c", "email": "c@example.test", "expires_at": rfc3339(3600) },
        });
        let path = w.dir.join("home").join("auth.json");
        std::fs::write(&path, serde_json::to_vec(&auth).unwrap()).unwrap();
        let before = std::fs::read(&path).unwrap();
        // 空 key 的 a 跳过，排序后第一份带 key 的是 b
        let acct = discover().unwrap();
        assert_eq!(acct.email, "b@example.test");
        assert!(!acct.expired());
        assert_eq!(before, std::fs::read(&path).unwrap());
        // 文件坏 = 没登录
        std::fs::write(&path, b"not json").unwrap();
        assert!(discover().is_none());
    }

    #[test]
    fn models_rows_windows_hidden_cache_and_stale_on_error() {
        let mock = Mock::start();
        mock.route("/v1/models", 200, "application/json", MODELS_BODY);
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_auth(&w, "tok-secret", "me@example.test", 3600);

        let body = reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1;
        let data = body["data"].as_array().unwrap();
        let ids: Vec<&str> = data.iter().map(|r| r["id"].as_str().unwrap()).collect();
        // hidden 不出；多窗口模型多一行 @500k
        assert!(ids.contains(&"grok/grok-4.7") && ids.contains(&"grok/grok-4.7@500k"));
        assert!(ids.contains(&"grok/grok-4.6") && !ids.contains(&"grok/grok-secret"));
        let wide = data.iter().find(|r| r["id"] == "grok/grok-4.7@500k").unwrap();
        assert_eq!(wide["context_window"], 500000);
        let base_row = data.iter().find(|r| r["id"] == "grok/grok-4.7").unwrap();
        assert_eq!(base_row["context_window"], 256000);
        assert_eq!(base_row["reasoning_levels"], json!(["xhigh", "high", "low"]));
        // 签名头
        let (headers, _) = mock.last("/v1/models");
        assert_eq!(headers["authorization"], "Bearer tok-secret");
        assert_eq!(headers["x-xai-token-auth"], "xai-grok-cli");
        assert_eq!(headers["x-grok-client-identifier"], "grok-shell");
        assert_eq!(headers["x-userid"], "user-test");

        // 30s 缓存
        let hits = mock.hits();
        let _ = super::super::routes::route(&get("/v1/models"), &gateway);
        assert_eq!(mock.hits(), hits);

        // 过期 + 500 → 回旧行；退避内不再打
        expire_cache_for_tests();
        mock.route("/v1/models", 500, "text/plain", "boom");
        let body = reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1;
        assert_eq!(body["data"].as_array().unwrap().len(), 3);
        let after_fail = mock.hits();
        let _ = super::super::routes::route(&get("/v1/models"), &gateway);
        assert_eq!(mock.hits(), after_fail);
    }

    #[test]
    fn failing_grok_does_not_hide_other_providers() {
        let mock = Mock::start(); // 没有 route → /models 404
        let vendor = Mock::start();
        vendor.route("/v1/models", 200, "application/json", r#"{"data":[{"id":"m1","context_window":1000}]}"#);
        let providers = format!(
            r#"{{"providers":[{{"id":"acme","name":"Acme","key":"k","chat":"{}"}}]}}"#,
            vendor.base
        );
        let (gateway, w) = world(&providers, &mock.base);
        write_auth(&w, "tok", "me@example.test", 3600);
        let body = reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1;
        let ids: Vec<&str> = body["data"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["acme/m1"]);
    }

    #[test]
    fn responses_stream_passthrough_and_shaping() {
        let mock = Mock::start();
        mock.route("/v1/responses", 200, "text/event-stream", SSE_TEXT);
        mock.route("/v1/models", 200, "application/json", MODELS_BODY);
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_auth(&w, "tok-secret", "me@example.test", 3600);
        let _ = super::super::routes::route(&get("/v1/models"), &gateway); // 填目录
        let reply = super::super::routes::route(
            &post(
                "/v1/responses",
                &json!({"model":"grok/grok-4.7@500k","stream":true,"input":"hello",
                        "tool_choice":"auto",
                        "reasoning":{"effort":"max"},
                        "include":["reasoning.encrypted_content","output.encrypted_content"],
                        "tools":[
                          {"type":"function","name":"f","parameters":{"type":"object"}},
                          {"type":"custom","name":"apply_patch"},
                          {"type":"web_search","external_web_access":true}]}),
            ),
            &gateway,
        );
        let (status, text) = reply_json(reply);
        assert_eq!(status, 200);
        assert!(text.as_str().unwrap().contains("response.completed"));
        let (headers, body) = mock.last("/v1/responses");
        assert_eq!(headers["authorization"], "Bearer tok-secret");
        assert_eq!(headers["x-grok-context-window"], "500000");
        assert_eq!(headers["x-grok-doom-loop-check"], "1024");
        assert_eq!(headers["accept"], "text/event-stream");
        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent["model"], "grok-4.7");
        assert_eq!(sent["stream"], true);
        // custom 工具被丢、external_web_access 被剥
        let tools = sent["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        assert!(tools[1].get("external_web_access").is_none());
        // max 不在模型菜单里 → 夹到 xhigh
        assert_eq!(sent["reasoning"]["effort"], "xhigh");
        assert_eq!(sent["include"], json!(["reasoning.encrypted_content"]));
        assert_eq!(sent["tool_choice"], "auto");
    }

    #[test]
    fn responses_nonstream_aggregates_sse() {
        let mock = Mock::start();
        mock.route("/v1/responses", 200, "text/event-stream", SSE_TEXT);
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_auth(&w, "tok", "me@example.test", 3600);
        let (status, resp) = reply_json(super::super::routes::route(
            &post("/v1/responses", &json!({"model":"grok/grok-4.7","stream":false,"input":"hello"})),
            &gateway,
        ));
        assert_eq!(status, 200);
        assert_eq!(resp["id"], "r1");
        assert_eq!(resp["output"][0]["content"][0]["text"], "hi");
        let (_, body) = mock.last("/v1/responses");
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap()["stream"], true);
    }

    #[test]
    fn chat_completions_stream_and_nonstream_via_responses() {
        let mock = Mock::start();
        mock.route("/v1/responses", 200, "text/event-stream", SSE_TEXT);
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_auth(&w, "tok", "me@example.test", 3600);
        // 非流式
        let (status, resp) = reply_json(super::super::routes::route(
            &post(
                "/v1/chat/completions",
                &json!({"model":"grok/grok-4.7","messages":[
                    {"role":"system","content":"be brief"},{"role":"user","content":"hello"}],
                    "reasoning_effort":"high"}),
            ),
            &gateway,
        ));
        assert_eq!(status, 200);
        assert_eq!(resp["object"], "chat.completion");
        assert_eq!(resp["model"], "grok/grok-4.7");
        assert_eq!(resp["choices"][0]["message"]["content"], "hi");
        assert_eq!(resp["usage"]["prompt_tokens"], 3);
        let (_, body) = mock.last("/v1/responses");
        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent["model"], "grok-4.7");
        assert_eq!(sent["instructions"], "be brief");
        assert_eq!(sent["reasoning"]["effort"], "high");
        assert_eq!(sent["input"][0]["role"], "user");
        // 流式
        let (status, text) = reply_json(super::super::routes::route(
            &post(
                "/v1/chat/completions",
                &json!({"model":"grok/grok-4.7","stream":true,"messages":[{"role":"user","content":"hello"}]}),
            ),
            &gateway,
        ));
        assert_eq!(status, 200);
        let text = text.as_str().unwrap().to_string();
        assert!(text.contains("chat.completion.chunk"));
        assert!(text.contains("\"content\":\"hi\""));
        assert!(text.contains("[DONE]"));
    }

    #[test]
    fn anthropic_messages_inbound_nonstream_and_stream() {
        let mock = Mock::start();
        mock.route("/v1/responses", 200, "text/event-stream", SSE_TEXT);
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_auth(&w, "tok", "me@example.test", 3600);
        let req = json!({"model":"grok/grok-4.7","max_tokens":64,"messages":[{"role":"user","content":"hello"}]});
        let (status, resp) = reply_json(super::super::routes::route(&post("/v1/messages", &req), &gateway));
        assert_eq!(status, 200);
        assert_eq!(resp["type"], "message");
        assert_eq!(resp["content"][0]["text"], "hi");
        let mut streamed = req.clone();
        streamed["stream"] = json!(true);
        let (status, text) = reply_json(super::super::routes::route(&post("/v1/messages", &streamed), &gateway));
        assert_eq!(status, 200);
        let text = text.as_str().unwrap().to_string();
        assert!(text.contains("message_start") && text.contains("text_delta") && text.contains("message_stop"));
    }

    #[test]
    fn upstream_error_status_is_passed_through_per_protocol() {
        let mock = Mock::start();
        mock.route("/v1/responses", 402, "application/json", r#"{"error":{"message":"Grok Build usage balance exhausted"}}"#);
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_auth(&w, "tok", "me@example.test", 3600);
        let (status, body) = reply_json(super::super::routes::route(
            &post("/v1/responses", &json!({"model":"grok/grok-4.7","input":"x"})),
            &gateway,
        ));
        assert_eq!(status, 402);
        assert_eq!(body["error"]["message"], "Grok Build usage balance exhausted");
        let (status, body) = reply_json(super::super::routes::route(
            &post("/v1/messages", &json!({"model":"grok/grok-4.7","max_tokens":8,"messages":[{"role":"user","content":"x"}]})),
            &gateway,
        ));
        assert_eq!(status, 402);
        assert_eq!(body["type"], "error");
        assert_eq!(body["error"]["message"], "Grok Build usage balance exhausted");
    }

    #[test]
    fn expired_token_is_a_clear_error_and_never_hits_network() {
        let mock = Mock::start();
        mock.route("/v1/models", 200, "application/json", MODELS_BODY);
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_auth(&w, "tok-secret", "me@example.test", -3600);
        assert!(synthetic_provider().is_some());
        for path in ["/v1/responses", "/v1/chat/completions"] {
            let (status, body) = reply_json(super::super::routes::route(
                &post(path, &json!({"model":"grok/grok-4.7","input":"x","messages":[{"role":"user","content":"x"}]})),
                &gateway,
            ));
            assert_eq!(status, 401, "{path}");
            assert_eq!(body["error"]["message"], EXPIRED_MESSAGE);
            assert!(!body.to_string().contains("tok-secret"));
        }
        let (status, body) = reply_json(super::super::routes::route(
            &post("/v1/messages", &json!({"model":"grok/grok-4.7","max_tokens":8,"messages":[{"role":"user","content":"x"}]})),
            &gateway,
        ));
        assert_eq!(status, 401);
        assert_eq!(body["error"]["message"], EXPIRED_MESSAGE);
        assert!(listed_rows(&[]).is_empty());
        assert!(quota_row(&[]).is_none());
        assert_eq!(mock.hits(), 0);
    }

    #[test]
    fn user_defined_grok_provider_takes_precedence() {
        let mock = Mock::start();
        mock.route("/v1/models", 200, "application/json", MODELS_BODY);
        let vendor = Mock::start();
        vendor.route("/v1/models", 200, "application/json", r#"{"data":[{"id":"real","context_window":5}]}"#);
        vendor.route("/v1/responses", 200, "application/json", r#"{"id":"from-real"}"#);
        let providers = format!(
            r#"{{"providers":[{{"id":"grok","name":"Mine","key":"k","chat":"{0}","responses":"{0}"}}]}}"#,
            vendor.base
        );
        let (gateway, w) = world(&providers, &mock.base);
        write_auth(&w, "tok", "me@example.test", 3600);
        let body = reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1;
        let ids: Vec<&str> = body["data"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["grok/real"]);
        let (status, resp) = reply_json(super::super::routes::route(
            &post("/v1/responses", &json!({"model":"grok/real","input":"x"})),
            &gateway,
        ));
        assert_eq!(status, 200);
        assert_eq!(resp["id"], "from-real");
        assert_eq!(mock.hits(), 0);
        assert!(quota_row(&[Provider { id: "grok".into(), ..Default::default() }]).is_none());
    }

    #[test]
    fn alias_group_member_effort_applies() {
        let mock = Mock::start();
        mock.route("/v1/responses", 200, "text/event-stream", SSE_TEXT);
        mock.route("/v1/models", 200, "application/json", MODELS_BODY);
        let providers = r#"{"providers":[],"groups":[
            {"id":"gpt-6.1-sol","name":"gpt-6.1-sol","members":["grok/grok-4.7:xhigh"],"pick":"grok/grok-4.7:xhigh"}]}"#;
        let (gateway, w) = world(providers, &mock.base);
        write_auth(&w, "tok", "me@example.test", 3600);
        let _ = super::super::routes::route(&get("/v1/models"), &gateway);
        let (status, _) = reply_json(super::super::routes::route(
            &post("/v1/responses", &json!({"model":"gpt-6.1-sol","stream":true,"input":"x","reasoning":{"effort":"low"}})),
            &gateway,
        ));
        assert_eq!(status, 200);
        let (_, body) = mock.last("/v1/responses");
        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent["model"], "grok-4.7");
        assert_eq!(sent["reasoning"]["effort"], "xhigh");
    }

    #[test]
    fn quota_row_comes_from_billing() {
        let mock = Mock::start();
        mock.route(
            "/v1/billing",
            200,
            "application/json",
            r#"{"config":{"creditUsagePercent":6,
                 "currentPeriod":{"type":"USAGE_PERIOD_TYPE_WEEKLY","start":"2030-01-01T00:00:00Z","end":"2030-01-08T00:00:00Z"},
                 "billingPeriodEnd":"2030-01-09T00:00:00Z"},
                "subscription_tier":"SuperGrok"}"#,
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_auth(&w, "tok-secret", "me@example.test", 3600);
        let rows = super::super::quotas::quota_entries(&gateway, true);
        let row = rows.iter().find(|r| r["provider"] == "grok").expect("grok row");
        assert_eq!(row["name"], "Grok (SuperGrok)");
        assert_eq!(row["kind"], "subscription");
        assert_eq!(row["user"], "me@example.test");
        assert_eq!(row["plan"], "SuperGrok");
        let windows = row["windows"].as_array().unwrap();
        assert_eq!(windows[0]["name"], "7 days");
        assert_eq!(windows[0]["used"], 6.0);
        assert_eq!(windows[0]["remaining"], 94.0);
        assert_eq!(windows[0]["resetsAt"], "2030-01-08T00:00:00Z");
        let (headers, _) = mock.last("/v1/billing");
        assert_eq!(headers["authorization"], "Bearer tok-secret");
        assert!(mock.seen.lock().unwrap().iter().any(|r| r.path.contains("format=credits")));
        // 失败安静跳过
        mock.route("/v1/billing", 500, "text/plain", "x");
        let rows = super::super::quotas::quota_entries(&gateway, true);
        assert!(rows.iter().all(|r| r["provider"] != "grok"));
    }

    #[test]
    fn window_suffix_roundtrip() {
        assert_eq!(window_suffix(500_000), "500k");
        assert_eq!(window_suffix(131_072), "131072");
        assert_eq!(split_window("grok-4.7@500k"), ("grok-4.7".to_string(), Some(500_000)));
        assert_eq!(split_window("grok-4.7@131072"), ("grok-4.7".to_string(), Some(131_072)));
        assert_eq!(split_window("grok-4.7"), ("grok-4.7".to_string(), None));
        assert_eq!(split_window("x@y"), ("x@y".to_string(), None));
        assert_eq!(split_window("@5k"), ("@5k".to_string(), None));
    }

    #[test]
    fn shape_flattens_namespace_and_drops_orphan_tool_choice() {
        let v = shape_body(json!({
            "tool_choice": {"type":"function","namespace":"collab","name":"spawn"},
            "tools": [
                {"type":"namespace","name":"collab","tools":[
                    {"type":"function","name":"spawn","parameters":{"type":"object"}},
                    {"type":"custom","name":"freeform"}]},
                {"type":"function","name":"g","parameters":{
                    "anyOf":[
                      {"type":"object","properties":{"a":{"type":"string"}},"required":["a","k"]},
                      {"type":"object","properties":{"b":{"type":"number"}},"required":["k"]}]}}],
            "input": [
                {"type":"function_call","namespace":"collab","name":"spawn","call_id":"c","arguments":"{}"},
                {"type":"reasoning","content":null,"encrypted_content":"x"},
                {"type":"agent_message","author":"lead","recipient":"w",
                 "content":[{"type":"input_text","text":"do it"}]},
                {"type":"agent_message","encrypted_content":"sealed","content":[{"type":"input_text","text":"s"}]}]
        }));
        let tools = v["tools"].as_array().unwrap();
        assert_eq!(tools[0]["name"], "collab__spawn");
        assert_eq!(tools.len(), 2); // freeform 被丢
        // 根 anyOf → 普通 object；共同必填保留
        assert_eq!(tools[1]["parameters"]["type"], "object");
        assert!(tools[1]["parameters"].get("anyOf").is_none());
        assert_eq!(tools[1]["parameters"]["required"], json!(["k"]));
        assert!(tools[1]["parameters"]["properties"].get("a").is_some());
        assert!(tools[1]["parameters"]["properties"].get("b").is_some());
        assert_eq!(v["tool_choice"]["name"], "collab__spawn");
        assert!(v["tool_choice"].get("namespace").is_none());
        let input = v["input"].as_array().unwrap();
        assert_eq!(input[0]["name"], "collab__spawn");
        assert!(input[1].get("content").is_none());
        assert_eq!(input[2]["type"], "message");
        assert_eq!(input[2]["role"], "user");
        assert_eq!(input[2]["content"][0]["text"], "From lead to w\n\n");
        assert_eq!(input[3]["type"], "agent_message"); // 加密的原样
        // 没有工具：tool_choice 去掉
        let v = shape_body(json!({"tool_choice":"auto","input":"x"}));
        assert!(v.get("tool_choice").is_none());
    }

    #[test]
    fn object_root_merges_allof_and_refs() {
        let mut ps = json!({
            "$defs": {"d": {"type":"object","properties":{"x":{"type":"integer"}},"required":["x"]}},
            "allOf": [{"$ref":"#/$defs/d"}, {"type":"object","properties":{"y":{"type":"string"}},"required":["y"]}]
        });
        let m = ps.as_object_mut().unwrap();
        assert!(object_root(m));
        assert_eq!(m["type"], "object");
        assert!(m["properties"].get("x").is_some() && m["properties"].get("y").is_some());
        assert_eq!(m["required"], json!(["x", "y"]));
        // 已是 object 的不动
        let mut plain = json!({"type":"object","properties":{}});
        assert!(!object_root(plain.as_object_mut().unwrap()));
    }

    #[test]
    fn flat_name_is_bounded() {
        assert_eq!(flat_name("a", "b"), "a__b");
        let long = flat_name(&"n".repeat(40), &"m".repeat(40));
        assert!(long.len() <= 64);
        assert_eq!(long, flat_name(&"n".repeat(40), &"m".repeat(40)));
    }

    #[test]
    fn clamp_effort_cases() {
        let menu: Vec<String> = ["xhigh", "high", "low"].iter().map(|s| s.to_string()).collect();
        assert_eq!(clamp_effort("max", &menu).as_deref(), Some("xhigh"));
        assert_eq!(clamp_effort("medium", &menu).as_deref(), Some("low"));
        assert_eq!(clamp_effort("high", &menu), None);
        assert_eq!(clamp_effort("max", &[]), None);
    }

    // ---- namespace 还原（Go grok_namespace.go nsTidy/namespacedIn/sealedArgs）----

    fn namespace_request(stream: bool) -> Value {
        json!({
            "model": "grok/grok-4.7", "stream": stream,
            "tools": [{"type":"namespace","name":"collab","tools":[
                {"type":"function","name":"spawn","parameters":{"type":"object"}}]}],
            "input": [
                {"type":"function_call","call_id":"c0","namespace":"collab","name":"spawn","arguments":"{}"},
                {"type":"function_call_output","call_id":"c0","output":"ok"},
                {"type":"message","role":"user","content":"go"}
            ]
        })
    }

    /// 上游应答：模型把调用写在**扁平名**下（Grok 只见过压平的工具）。
    fn ns_sse(item: Value) -> String {
        format!(
            "event: response.created\ndata: {{\"type\":\"response.created\",\"response\":{{\"id\":\"r9\"}}}}\n\n\
             event: response.output_item.done\n\
             data: {{\"type\":\"response.output_item.done\",\"item\":{item}}}\n\n\
             event: response.completed\n\
             data: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"r9\",\"status\":\"completed\",\
                    \"output\":[{item}],\"usage\":{{\"input_tokens\":3,\"output_tokens\":1}}}}}}\n\n"
        )
    }

    #[test]
    fn namespace_round_trip_nonstream() {
        let mock = Mock::start();
        mock.route(
            "/v1/responses",
            200,
            "text/event-stream",
            &ns_sse(json!({"type":"function_call","call_id":"c1","name":"collab__spawn","arguments":"{\"msg\":\"hi\"}"})),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_auth(&w, "tok", "me@example.test", 3600);
        let (status, resp) = reply_json(super::super::routes::route(
            &post("/v1/responses", &namespace_request(false)),
            &gateway,
        ));
        assert_eq!(status, 200);
        // 应答侧翻回：原名 + namespace + 空 sealed 清单（Go callTo）
        let call = &resp["output"][0];
        assert_eq!(call["type"], "function_call");
        assert_eq!(call["name"], "spawn");
        assert_eq!(call["namespace"], "collab");
        assert_eq!(call["encrypted_function_args"], json!([]));
        // 出站侧压平（既有行为不变）
        let (_, body) = mock.last("/v1/responses");
        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent["tools"][0]["name"], "collab__spawn");
        assert!(sent["tools"][0].get("type").is_none() || sent["tools"][0]["type"] != "namespace");
        let fc = sent["input"].as_array().unwrap().iter().find(|i| i["type"] == "function_call").unwrap();
        assert_eq!(fc["name"], "collab__spawn");
        assert!(fc.get("namespace").is_none());
    }

    #[test]
    fn namespace_round_trip_stream() {
        let mock = Mock::start();
        mock.route(
            "/v1/responses",
            200,
            "text/event-stream",
            &ns_sse(json!({"type":"function_call","call_id":"c1","name":"collab__spawn","arguments":"{\"msg\":\"hi\"}"})),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_auth(&w, "tok", "me@example.test", 3600);
        let (status, text) = reply_json(super::super::routes::route(
            &post("/v1/responses", &namespace_request(true)),
            &gateway,
        ));
        assert_eq!(status, 200);
        let text = text.as_str().unwrap();
        // 事件帧里的调用也翻回（output_item.done 与 response.completed 各一）
        let frames: Vec<Value> = text
            .split("\n\n")
            .filter_map(|b| b.lines().find_map(|l| l.strip_prefix("data: ")).and_then(|d| serde_json::from_str(d).ok()))
            .collect();
        let done = frames.iter().find(|f| f["type"] == "response.output_item.done").unwrap();
        assert_eq!(done["item"]["name"], "spawn");
        assert_eq!(done["item"]["namespace"], "collab");
        assert_eq!(done["item"]["encrypted_function_args"], json!([]));
        let completed = frames.iter().find(|f| f["type"] == "response.completed").unwrap();
        assert_eq!(completed["response"]["output"][0]["name"], "spawn");
        assert_eq!(completed["response"]["output"][0]["namespace"], "collab");
        // 别的行原样过
        assert!(text.contains("\"response.created\""));
    }

    #[test]
    fn sealed_args_names_the_tokens_when_namespace_came_back() {
        let mock = Mock::start();
        // 上游自己带了 namespace（被当 Codex 问的厂商，#3183）：名字不动，
        // 只补 encrypted_function_args 清单（键名排序）。
        mock.route(
            "/v1/responses",
            200,
            "text/event-stream",
            &ns_sse(json!({"type":"function_call","call_id":"c2","namespace":"collab","name":"spawn",
                           "arguments":"{\"ztask\":\"plain\",\"atask\":\"gAAAAABCdef123-_\"}"})),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_auth(&w, "tok", "me@example.test", 3600);
        let (status, resp) = reply_json(super::super::routes::route(
            &post("/v1/responses", &namespace_request(false)),
            &gateway,
        ));
        assert_eq!(status, 200);
        let call = &resp["output"][0];
        assert_eq!(call["name"], "spawn"); // 没翻名
        assert_eq!(call["namespace"], "collab");
        assert_eq!(call["encrypted_function_args"], json!(["atask"]));
    }

    #[test]
    fn namespace_restore_ignores_unknown_flat_names() {
        let mock = Mock::start();
        mock.route(
            "/v1/responses",
            200,
            "text/event-stream",
            &ns_sse(json!({"type":"function_call","call_id":"c9","name":"other__tool","arguments":"{}"})),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_auth(&w, "tok", "me@example.test", 3600);
        let (status, resp) = reply_json(super::super::routes::route(
            &post("/v1/responses", &namespace_request(false)),
            &gateway,
        ));
        assert_eq!(status, 200);
        assert_eq!(resp["output"][0]["name"], "other__tool"); // 不认识的扁平名原样
        assert!(resp["output"][0].get("namespace").is_none());
    }

    #[test]
    fn no_namespace_tools_means_plain_passthrough() {
        let req = json!({
            "model": "grok/grok-4.7",
            "tools": [{"type":"function","name":"plain","parameters":{"type":"object"}}],
            "input": [{"type":"message","role":"user","content":"go"}]
        });
        let named = namespaced_in(&req);
        assert!(named.is_empty());
        // Lite 自己的 namespace（functions）不算
        let req = json!({
            "tools": [{"type":"namespace","name":"functions","tools":[
                {"type":"function","name":"f"}]}],
            "input": [{"type":"function_call","namespace":"functions","name":"f","arguments":"{}"}]
        });
        assert!(namespaced_in(&req).is_empty());
        // input 是字符串（正文单独来）：不解析
        assert!(namespaced_in(&json!({"input":"just text"})).is_empty());
    }

    // ---- schema pattern 转写（Go grok_schema.go ecmaClassForRust）----

    #[test]
    fn ecma_class_for_rust_escapes_class_syntax() {
        // Go 注释里的实弹例子（#2317，Claude Code 的 breakpoint class_name）
        assert_eq!(ecma_class_for_rust("^[^\\s[\\]]+$"), "^[^\\s\\[\\]]+$");
        // 集合运算对：第二个字符转义
        assert_eq!(ecma_class_for_rust("[a&&b]"), "[a&\\&b]");
        assert_eq!(ecma_class_for_rust("[a--b]"), "[a-\\-b]");
        assert_eq!(ecma_class_for_rust("[a~~b]"), "[a~\\~b]");
        // 类外的 [ 不动；转义对原样放行（且不关类）
        assert_eq!(ecma_class_for_rust("ab[c]"), "ab[c]");
        assert_eq!(ecma_class_for_rust("[a\\[b]"), "[a\\[b]");
        assert_eq!(ecma_class_for_rust("[a\\]b]"), "[a\\]b]");
        // 已经转义过的不再动
        assert_eq!(ecma_class_for_rust("[\\&&]"), "[\\&&]");
        // 多字节字符按字节搬运（UTF-8 保真）
        assert_eq!(ecma_class_for_rust("[中文[x]"), "[中文\\[x]");
    }

    #[test]
    fn shape_body_transcribes_patterns_in_schema_contexts() {
        let v = shape_body(json!({
            "tools": [{
                "type": "function", "name": "brk",
                "parameters": {"type":"object","properties":{
                    "cls":   {"type":"string","pattern":"^[^\\s[\\]]+$"},
                    "amp":   {"type":"string","pattern":"[a&&b]"},
                    "dash":  {"type":"string","pattern":"[a--b]"},
                    "tilde": {"type":"string","pattern":"[a~~b]"},
                    "nested":{"type":"object","properties":{"deep":{"type":"string","pattern":"[[]"}}},
                    "anyof": {"anyOf":[{"type":"string","pattern":"[x[]"}]},
                    "items": {"type":"array","items":{"type":"string","pattern":"[y[]"}},
                    "defs":  {"$defs":{"d":{"type":"string","pattern":"[z[]"}},"$ref":"#/$defs/d"},
                    "plain": {"type":"string"}
                }}
            }]
        }));
        let p = &v["tools"][0]["parameters"]["properties"];
        assert_eq!(p["cls"]["pattern"], "^[^\\s\\[\\]]+$");
        assert_eq!(p["amp"]["pattern"], "[a&\\&b]");
        assert_eq!(p["dash"]["pattern"], "[a-\\-b]");
        assert_eq!(p["tilde"]["pattern"], "[a~\\~b]");
        assert_eq!(p["nested"]["properties"]["deep"]["pattern"], "[\\[]");
        assert_eq!(p["anyof"]["anyOf"][0]["pattern"], "[x\\[]");
        assert_eq!(p["items"]["items"]["pattern"], "[y\\[]");
        assert_eq!(p["defs"]["$defs"]["d"]["pattern"], "[z\\[]");
        // pattern 之外的字段、没有 pattern 的 schema 不动
        assert_eq!(p["cls"]["type"], "string");
        assert!(p["plain"].get("pattern").is_none());
    }

    #[test]
    fn pattern_transcription_survives_the_wire() {
        let mock = Mock::start();
        mock.route("/v1/responses", 200, "text/event-stream", SSE_TEXT);
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_auth(&w, "tok", "me@example.test", 3600);
        let chat = json!({
            "model": "grok/grok-4.7",
            "messages": [{"role":"user","content":"x"}],
            "tools": [{
                "type": "function",
                "function": {
                    "name": "brk",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "cls": {"type":"string","pattern":"^[^\\s[\\]]+$"}
                        }
                    }
                }
            }]
        });
        let _ = super::super::routes::route(&post("/v1/chat/completions", &chat), &gateway);
        let (_, body) = mock.last("/v1/responses");
        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent["tools"][0]["parameters"]["properties"]["cls"]["pattern"], "^[^\\s\\[\\]]+$");
    }
}
