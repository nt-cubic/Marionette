//! Codex 订阅账号（ChatGPT 登录）的只读支持——Go 版 `codexAccount` 的最小移植。
//!
//! 范围（Go 参照 internal/provider：account.go codexAccount、codex_request.go
//! codexSign/codexBody、codex_models.go、subscription_usage.go codexWindows）：
//! - 凭据发现：`<config_dir>/logins.json` 里 `agent=="codex"` 的条目与 Codex CLI
//!   自己的 `$CODEX_HOME|~/.codex/auth.json`，取 access_token 到期更晚的那份；
//! - 合成来源 `codex`（只在内存，**绝不写进 providers.json**；用户自己定义了
//!   同名来源就让位）；
//! - `/v1/responses` → `{CodexBase}/responses`：Codex CLI 同款头、上游恒 stream、
//!   客户端要非流式就把 SSE 聚合回最终 response 对象；
//! - `/v1/models` 的 `codex/<slug>` 行（来自 `{base}/models?client_version=`）；
//! - 额度行（`/wham/usage` 的窗口）。
//!
//! **安全红线（只读）**：绝不写/轮换/刷新任何凭据文件，绝不碰 auth.openai.com——
//! access token 过期就如实报「请先运行一次 Codex 刷新登录」，刷新留给 Codex 自己。
//! 令牌值永不进日志/错误文本/测试输出。
//!
//! 入站协议：`/v1/responses` 直通；`/v1/chat/completions` 与 `/v1/messages`
//! 先把请求翻成 Responses、走同一条 Codex 出站，再把 responses SSE / 聚合对象
//! 翻回 chat / anthropic（translate.rs 的 ResponsesToChat；anthropic 出站复用
//! messages.rs 的 chat→anthropic 翻译器）。
//!
//! 多账号：`logins.json` 的 codex 条目 + CLI 自己的 auth.json 按
//! (chatgpt-account-id, email) 去重；默认取「未过期且到期最晚」的一份，
//! `codex/<model>#<账号>`（账号 = logins 条目 id / 邮箱 / account_id）钉死到
//! 指定账号（钉了却找不到 = 明确报错，**不**落到别的账号）；额度每账号一行。
//! 凭据每个请求现读（Codex CLI 在跑时自己保鲜 auth.json）。
//!
//! 出站压缩（Go codex_zstd.go CodexCompressed 的 Rust 侧）：>64KB 的请求体
//! gzip 后 `Content-Encoding: gzip` 发送。**与 Go 的有意偏差**：Go 用 zstd
//! （codex CLI 自己的形态，1.1MB 明文被 ChatGPT 回 503）；Rust 侧 ruzstd 只有
//! 解压器、加 C 绑定的压缩器是被禁的依赖，而 gzip 是 HTTP 通用编码，ChatGPT
//! 按 HTTP 语义应当解。若真机验证 ChatGPT 拒收 gzip，此路回退明文（压缩失败
//! 时已如此）并在错误信息里说明。
//!
//! 订阅耗尽回退（Go codex_backend.go codexFallBack :1818 / codexFallsBack
//! :1749 / codexAccountOut :1734 的最小版）：上游 402/403(usage_not_included)
//! /429/带额度词表的错误时按账号序换下一份重试；单账号/全耗尽 → 429 +
//! 「订阅耗尽」文案（`groups::retryable` 对 429 恒真，上层组路由的候选循环
//! 可以据此换成员）。
//!
//! 未移植（见交付报告）：Go 的组级 fallback（FallbackModels/GroupsAfter——
//! Rust 侧交上层候选循环）、resets/credits 明细、账号热备。
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::catalog::VendorRow;
use super::effort::Proto;
use super::httpd::{Reply, Request};
use super::store::Provider;

pub const CODEX_ID: &str = "codex";
const DEFAULT_BASE: &str = "https://chatgpt.com/backend-api/codex";
/// Go codexClientVersion：没人告诉我们更新的版本时，模型表按这个版本问
/// （后端只把新模型列给够新的客户端）。下限 0.169.1 = 上游 2026-11-18 的
/// npm latest；此前停在 0.159.0——GPT-6.2 Sol / Astra 要 0.166.0 才被列出来，
/// 本机没有更新的 Codex CLI（或 models_cache.json）的用户永远看不到它们
/// （上游 zzstar101 / m0eaki 实锤，Rust 侧 2026-11-26 跟上）。上游另外还会在
/// 后台跟 npm latest；这里只取「内置下限 vs 本机缓存」的新者，如 Go codexVersion。
const CLIENT_VERSION: &str = "0.169.1";
const CACHE_TTL: Duration = Duration::from_secs(30);
const FAIL_BACKOFF: Duration = Duration::from_secs(60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// access token 离到期不足这个秒数就当过期（不刷新，只报错）。
const EXPIRY_MARGIN_SECS: i64 = 30;
const EXPIRED_MESSAGE: &str = "Codex sign-in expired — run Codex once to refresh";
/// 出站压缩阈值：超过这个字节数的请求体 gzip 发送（Go codex_zstd.go 的注释：
/// 1.1MB 明文 503、压缩后 200——我们没等真机撞墙，直接按同一动机做）。
const COMPRESS_MIN: usize = 64 * 1024;
/// 单账号/全部账号都耗尽时的兜底文案。状态恒 429：`groups::retryable` 对 429
/// 恒真（chat.rs 的候选循环按它换成员），词表判定见 [`codex_account_out`]。
const EXHAUSTED_MESSAGE: &str =
    "Codex subscription usage limit exhausted (no signed-in account can take this turn)";
/// Go codexInstructions 的最后兜底之前没有任何来源时用的极简指令
/// （后端要求 instructions 非空）。
const FALLBACK_INSTRUCTIONS: &str = "You are Codex, a coding agent. Be concise and accurate.";

// =====================================================================
// 测试缝（cfg(test) 才存在；生产路径恒走真实位置）
// =====================================================================

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct Seam {
    pub base: String,
    pub logins_dir: PathBuf,
    pub codex_home: PathBuf,
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
    seam()
        .map(|s| s.base)
        .unwrap_or_else(|| "http://127.0.0.1:9/codex".into())
}
#[cfg(not(test))]
fn base_url() -> String {
    DEFAULT_BASE.to_string()
}

#[cfg(test)]
fn logins_dir() -> Option<PathBuf> {
    seam().map(|s| s.logins_dir)
}
#[cfg(not(test))]
fn logins_dir() -> Option<PathBuf> {
    super::config_dir()
}

#[cfg(test)]
fn codex_home() -> Option<PathBuf> {
    seam().map(|s| s.codex_home)
}
#[cfg(not(test))]
fn codex_home() -> Option<PathBuf> {
    if let Some(h) = std::env::var_os("CODEX_HOME").filter(|h| !h.is_empty()) {
        return Some(PathBuf::from(h));
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|h| PathBuf::from(h).join(".codex"))
}

// =====================================================================
// 凭据
// =====================================================================

/// 发现到的一份 ChatGPT 登录（令牌值不 Debug、不进日志）。
#[derive(Clone)]
pub(crate) struct Account {
    /// logins.json 条目的稳定 id（auth.json 来源没有 = 空）。
    id: String,
    token: String,
    account_id: String,
    email: String,
    plan: String,
    /// access token 的 JWT `exp`（unix 秒）；读不出 = None。
    exp: Option<i64>,
}

impl Account {
    fn expired(&self) -> bool {
        self.exp
            .is_some_and(|e| e <= now_secs() + EXPIRY_MARGIN_SECS)
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// JWT 载荷（不验签——只读展示/到期判断用）。
fn jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn claim_str(claims: &Value, path: &[&str]) -> String {
    let mut v = claims;
    for k in path {
        match v.get(*k) {
            Some(next) => v = next,
            None => return String::new(),
        }
    }
    v.as_str().unwrap_or("").to_string()
}

/// `{tokens:{access_token,id_token,account_id}, auth_mode?}` → Account。
fn account_from_auth(
    auth: &Value,
    id: &str,
    fallback_user: &str,
    fallback_plan: &str,
) -> Option<Account> {
    if auth.get("auth_mode").and_then(|m| m.as_str()) == Some("apikey") {
        return None;
    }
    let tokens = auth.get("tokens")?;
    let token = tokens.get("access_token")?.as_str()?.trim().to_string();
    if token.is_empty() {
        return None;
    }
    let id_claims = tokens
        .get("id_token")
        .and_then(|t| t.as_str())
        .and_then(jwt_claims)
        .unwrap_or(Value::Null);
    let access_claims = jwt_claims(&token).unwrap_or(Value::Null);
    let mut email = claim_str(&id_claims, &["email"]);
    if email.is_empty() {
        email = claim_str(&access_claims, &["https://api.openai.com/profile", "email"]);
    }
    if email.is_empty() {
        email = fallback_user.to_string();
    }
    let mut plan = claim_str(
        &id_claims,
        &["https://api.openai.com/auth", "chatgpt_plan_type"],
    );
    if plan.is_empty() {
        plan = fallback_plan.to_string();
    }
    let mut account_id = tokens
        .get("account_id")
        .and_then(|a| a.as_str())
        .unwrap_or("")
        .to_string();
    if account_id.is_empty() {
        account_id = claim_str(
            &id_claims,
            &["https://api.openai.com/auth", "chatgpt_account_id"],
        );
    }
    Some(Account {
        id: id.to_string(),
        exp: access_claims.get("exp").and_then(|e| e.as_i64()),
        token,
        account_id,
        email,
        plan,
    })
}

/// 收集全部候选账号（logins.json 的 codex 条目 + CLI 自己的 auth.json），
/// 同一账号（chatgpt-account-id + 邮箱相同）只留到期更晚的那份。顺序：先出现
/// 的在前（logins.json 在前）。每次调用现读文件——不缓存（Codex CLI 会自己
/// 刷新 auth.json）。
pub(crate) fn discover_all() -> Vec<Account> {
    let mut found: Vec<Account> = Vec::new();
    if let Some(dir) = logins_dir() {
        if let Some(list) = std::fs::read(dir.join("logins.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        {
            for entry in list.as_array().into_iter().flatten() {
                if entry.get("agent").and_then(|a| a.as_str()) != Some("codex") {
                    continue;
                }
                let Some(auth) = entry.get("auth").filter(|a| a.is_object()) else {
                    continue;
                };
                let id = entry.get("id").and_then(|u| u.as_str()).unwrap_or("");
                let user = entry.get("user").and_then(|u| u.as_str()).unwrap_or("");
                let plan = entry.get("plan").and_then(|u| u.as_str()).unwrap_or("");
                if let Some(a) = account_from_auth(auth, id, user, plan) {
                    found.push(a);
                }
            }
        }
    }
    if let Some(home) = codex_home() {
        if let Some(auth) = std::fs::read(home.join("auth.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        {
            if let Some(a) = account_from_auth(&auth, "", "", "") {
                found.push(a);
            }
        }
    }
    let mut out: Vec<Account> = Vec::new();
    for a in found {
        let key = |x: &Account| (x.account_id.clone(), x.email.to_ascii_lowercase());
        match out.iter_mut().find(|o| key(o) == key(&a)) {
            Some(o) => {
                let keep_id = if o.id.is_empty() {
                    a.id.clone()
                } else {
                    o.id.clone()
                };
                if a.exp.unwrap_or(0) > o.exp.unwrap_or(0) {
                    *o = a;
                }
                o.id = keep_id;
            }
            None => out.push(a),
        }
    }
    out
}

/// 未过期的优先，再取到期最晚的；全过期仍返回一份（让调用方报「登录过期」
/// 而不是「没登录」）。平局先出现的赢。
fn pick_best(list: Vec<Account>) -> Option<Account> {
    let mut best: Option<Account> = None;
    for a in list {
        let better = match &best {
            None => true,
            Some(b) => match (a.expired(), b.expired()) {
                (false, true) => true,
                (true, false) => false,
                _ => a.exp.unwrap_or(0) > b.exp.unwrap_or(0),
            },
        };
        if better {
            best = Some(a);
        }
    }
    best
}

pub(crate) fn discover() -> Option<Account> {
    pick_best(discover_all())
}

/// 轮换账号序（Go codexFallBack 的账号轮换半边）：未过期的账号、最优的
/// 提到队首（默认请求与旧行为一致），其余按发现序跟在后面——上一个
/// 账号被额度类错误拒了就换下一个。
fn ordered_live(all: Vec<Account>) -> Vec<Account> {
    let mut all: Vec<Account> = all.into_iter().filter(|a| !a.expired()).collect();
    if all.len() > 1 {
        // pick_best 在「全未过期」集合上的选择 = 到期最晚、平局先出现的赢。
        let mut best = 0;
        for i in 1..all.len() {
            if all[i].exp.unwrap_or(0) > all[best].exp.unwrap_or(0) {
                best = i;
            }
        }
        all.rotate_left(best);
    }
    all
}

/// 账号引用（`#` 后的串）命中哪些账号：logins 条目 id（精确）/ 邮箱（不分
/// 大小写）/ chatgpt-account-id（精确）。
fn matches_ref(a: &Account, want: &str) -> bool {
    let want = want.trim();
    !want.is_empty()
        && ((!a.id.is_empty() && a.id == want)
            || a.email.eq_ignore_ascii_case(want)
            || (!a.account_id.is_empty() && a.account_id == want))
}

enum Selected {
    Ok(Account),
    /// 一个 codex 账号都没有。
    NoAccount,
    /// 钉了账号但没有这个账号（不落到别的账号上——Go memberAccountKept 同款）。
    UnknownPin(String),
}

fn select_account(pin: Option<&str>) -> Selected {
    let all = discover_all();
    match pin.map(str::trim).filter(|p| !p.is_empty()) {
        Some(want) => {
            let hits: Vec<Account> = all.into_iter().filter(|a| matches_ref(a, want)).collect();
            match pick_best(hits) {
                Some(a) => Selected::Ok(a),
                None => Selected::UnknownPin(want.to_string()),
            }
        }
        None => match pick_best(all) {
            Some(a) => Selected::Ok(a),
            None => Selected::NoAccount,
        },
    }
}

/// 合成来源：有账号才有。`responses` 指向 Codex 后端；不带 key/headers
/// （签名在出站时现做）。用户在 providers.json 里自己定义了 `codex` 时调用方
/// 负责让位。
pub fn synthetic_provider() -> Option<Provider> {
    discover()?;
    Some(Provider {
        id: CODEX_ID.to_string(),
        name: "Codex".to_string(),
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

/// 两个版本里新的那个；b 不是版本就是 a。
fn newer_version(a: &str, b: &str) -> String {
    match (version_parts(a), version_parts(b)) {
        (Some(x), Some(y)) if y > x => b.trim().to_string(),
        _ => a.trim().to_string(),
    }
}

/// Codex CLI 自己问过的版本（models_cache.json 的 client_version）与内置版本
/// 取新（Go codexVersion）。
fn client_version() -> String {
    let mut v = CLIENT_VERSION.to_string();
    if let Some(home) = codex_home() {
        if let Some(c) = std::fs::read(home.join("models_cache.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|j| {
                j.get("client_version")
                    .and_then(|c| c.as_str())
                    .map(String::from)
            })
        {
            v = newer_version(&v, &c);
        }
    }
    v
}

fn user_agent(ver: &str) -> String {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    let (os, term) = if cfg!(windows) {
        ("Windows 10.0.26100", "WindowsTerminal")
    } else {
        ("Linux 6.8.0", "xterm-256color")
    };
    format!("codex_cli_rs/{ver} ({os}; {arch}) {term}")
}

/// 给一个 ureq 请求签上 Codex CLI 同款头。`body` 为 Some = 模型请求（要 SSE、
/// 带会话头）；None = /models、/wham 一类账号读取。
fn sign(
    mut request: ureq::Request,
    account: &Account,
    ver: &str,
    body: Option<&Value>,
) -> ureq::Request {
    request = request
        .set("Authorization", &format!("Bearer {}", account.token))
        .set("originator", "codex_cli_rs")
        .set("User-Agent", &user_agent(ver));
    if !account.account_id.is_empty() {
        request = request.set("chatgpt-account-id", &account.account_id);
    }
    match body {
        Some(body) => {
            request = request
                .set("OpenAI-Beta", "responses=experimental")
                .set("version", ver)
                .set("Accept", "text/event-stream");
            if let Some(key) = body
                .get("prompt_cache_key")
                .and_then(|k| k.as_str())
                .filter(|k| !k.is_empty())
            {
                request = request.set("session-id", key).set("thread-id", key);
            }
            request
        }
        None => request.set("Accept", "application/json"),
    }
}

// =====================================================================
// 模型目录
// =====================================================================

struct ModelCache {
    at: Instant,
    rows: Vec<VendorRow>,
    prompts: HashMap<String, String>,
}

struct State {
    cache: Option<ModelCache>,
    backoff_until: Option<Instant>,
}

static STATE: Mutex<State> = Mutex::new(State {
    cache: None,
    backoff_until: None,
});

fn lock_state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    let mut s = lock_state();
    s.cache = None;
    s.backoff_until = None;
    drop(s);
    // WS 侧的进程级状态（keep 池 / shun 名单 / 1009 尺寸记忆）一并清零——
    // world() 每个测试开头都调这里，等价 Go socketOn 的收尾
    // （codex_socket_test.go :142-146：codexKeepDrain + clear(shun/tooBig)）。
    lock_keep_pool().clear();
    let mut g = lock_ws_guard();
    g.shun.clear();
    g.too_big.clear();
}

/// 让缓存立刻过期但保留旧行（测 stale-on-error 用）。
#[cfg(test)]
pub(crate) fn expire_cache_for_tests() {
    let mut s = lock_state();
    if let Some(c) = s.cache.as_mut() {
        c.at = Instant::now() - CACHE_TTL - Duration::from_secs(1);
    }
    s.backoff_until = None;
}

/// Go parseCodexModels：按 priority 排序，visibility=="hide" 跳过。
fn parse_models(body: &[u8]) -> (Vec<VendorRow>, HashMap<String, String>) {
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return (Vec::new(), HashMap::new());
    };
    let mut items: Vec<&Value> = value
        .get("models")
        .and_then(|m| m.as_array())
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    items.sort_by_key(|m| m.get("priority").and_then(|p| p.as_i64()).unwrap_or(0));
    let mut rows = Vec::new();
    let mut prompts = HashMap::new();
    for m in items {
        let slug = m.get("slug").and_then(|s| s.as_str()).unwrap_or("").trim();
        if slug.is_empty() {
            continue;
        }
        // 指令表覆盖全部条目（含 hide 的——Go saveCodexPrompts 同款）。
        let prompt = m
            .pointer("/model_messages/instructions_template")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .or_else(|| {
                m.get("base_instructions")
                    .and_then(|s| s.as_str())
                    .filter(|s| !s.is_empty())
            });
        if let Some(p) = prompt {
            prompts.insert(slug.to_string(), p.to_string());
        }
        if m.get("visibility").and_then(|v| v.as_str()) == Some("hide") {
            continue;
        }
        let context = m
            .get("context_window")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let images = m
            .get("input_modalities")
            .and_then(|v| v.as_array())
            .is_some_and(|l| l.iter().any(|x| x.as_str() == Some("image")));
        let levels: Vec<String> = m
            .get("supported_reasoning_levels")
            .and_then(|v| v.as_array())
            .map(|l| {
                l.iter()
                    .filter_map(|x| x.get("effort").and_then(|e| e.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let display = m
            .get("display_name")
            .and_then(|d| d.as_str())
            .filter(|d| !d.is_empty())
            .unwrap_or(slug);
        let mut raw = json!({ "id": slug, "display_name": display });
        if !levels.is_empty() {
            raw["reasoning_levels"] = json!(levels);
        }
        rows.push(VendorRow {
            id: slug.to_string(),
            context_window: context,
            max_output: 0,
            images,
            raw,
        });
    }
    (rows, prompts)
}

fn fetch_models(account: &Account) -> Result<(Vec<VendorRow>, HashMap<String, String>), String> {
    let ver = client_version();
    let url = format!("{}/models?client_version={}", base_url(), ver);
    let response = sign(super::outbound_agent().get(&url), account, &ver, None)
        .timeout(FETCH_TIMEOUT)
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(code, _) => format!("ChatGPT models: HTTP {code}"),
            other => format!("ChatGPT models: {other}"),
        })?;
    let mut buf = Vec::new();
    response
        .into_reader()
        .take(16 << 20)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    let (rows, prompts) = parse_models(&buf);
    if rows.is_empty() {
        return Err("ChatGPT listed no Codex models".into());
    }
    Ok((rows, prompts))
}

/// `/v1/models` 的 codex 行（30s 缓存、失败 60s 退避、失败回旧缓存）。没有
/// 账号 / 令牌过期 / 用户自有 `codex` 来源 / 订阅共享未开启（2026-11-22 四族
/// 默认不共享）→ 空（过期时有旧缓存就继续给旧的）。
pub fn listed_rows(providers: &[Provider]) -> Vec<VendorRow> {
    rows_impl(providers, true)
}

/// 管理面（模型设置里点开订阅行）：无视共享开关的只读清单，不进菜单/注入。
pub fn listed_rows_for_inspection(providers: &[Provider]) -> Vec<VendorRow> {
    rows_impl(providers, false)
}

fn rows_impl(providers: &[Provider], gated: bool) -> Vec<VendorRow> {
    if providers.iter().any(|p| p.id == CODEX_ID) || (gated && !super::family_enabled(CODEX_ID)) {
        return Vec::new();
    }
    let Some(account) = discover() else {
        return Vec::new();
    };
    {
        let state = lock_state();
        if let Some(c) = state.cache.as_ref() {
            if c.at.elapsed() < CACHE_TTL {
                return c.rows.clone();
            }
        }
        let stale = state
            .cache
            .as_ref()
            .map(|c| c.rows.clone())
            .unwrap_or_default();
        if account.expired() || state.backoff_until.is_some_and(|u| Instant::now() < u) {
            return stale;
        }
    }
    match fetch_models(&account) {
        Ok((rows, prompts)) => {
            let mut state = lock_state();
            state.backoff_until = None;
            state.cache = Some(ModelCache {
                at: Instant::now(),
                rows: rows.clone(),
                prompts,
            });
            rows
        }
        Err(_) => {
            let mut state = lock_state();
            state.backoff_until = Some(Instant::now() + FAIL_BACKOFF);
            state
                .cache
                .as_ref()
                .map(|c| c.rows.clone())
                .unwrap_or_default()
        }
    }
}

/// 整串 `codex/<slug>` 是不是已知模型（纯内存，组成员后缀拆分用）。
fn knows(whole: &str) -> bool {
    let Some(slug) = whole.strip_prefix("codex/") else {
        return false;
    };
    lock_state()
        .cache
        .as_ref()
        .is_some_and(|c| c.rows.iter().any(|r| r.id == slug))
}

fn levels_of(slug: &str) -> Vec<String> {
    let state = lock_state();
    let Some(row) = state
        .cache
        .as_ref()
        .and_then(|c| c.rows.iter().find(|r| r.id == slug))
    else {
        return Vec::new();
    };
    row.raw
        .get("reasoning_levels")
        .and_then(|v| v.as_array())
        .map(|l| {
            l.iter()
                .filter_map(|x| x.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

fn prompt_of(slug: &str) -> Option<String> {
    lock_state()
        .cache
        .as_ref()
        .and_then(|c| c.prompts.get(slug).cloned())
}

// =====================================================================
// 额度
// =====================================================================

fn duration_name(secs: i64) -> String {
    if secs > 0 && secs % 86_400 == 0 {
        return format!("{} days", secs / 86_400);
    }
    if secs > 0 && secs % 3_600 == 0 {
        return format!("{} hours", secs / 3_600);
    }
    "Allowance".to_string()
}

fn window_row(w: &Value) -> Value {
    let used = w
        .get("used_percent")
        .and_then(|u| u.as_f64())
        .unwrap_or(0.0)
        .clamp(0.0, 100.0);
    let span = w
        .get("limit_window_seconds")
        .and_then(|s| s.as_i64())
        .unwrap_or(0);
    let reset_after = w
        .get("reset_after_seconds")
        .and_then(|s| s.as_i64())
        .unwrap_or(0);
    let reset_at = w
        .get("reset_at")
        .and_then(|s| s.as_i64())
        .filter(|t| *t > 0);
    let mut row = json!({
        "name": duration_name(span),
        "used": used,
        "remaining": 100.0 - used,
    });
    if reset_after > 0 {
        row["resetSecs"] = json!(reset_after);
    }
    if let Some(at) = reset_at {
        if let Some(t) = chrono::DateTime::from_timestamp(at, 0) {
            row["resetsAt"] = json!(t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        }
    }
    row
}

/// 订阅额度行（C3），**每个账号一行**（user=邮箱）。没账号 / 过期 / 用户自有
/// `codex` 来源 / 请求失败 → 该账号不出行（展示面，安静跳过）。多账号并发问。
pub fn quota_rows(providers: &[Provider]) -> Vec<Value> {
    if providers.iter().any(|p| p.id == CODEX_ID) {
        return Vec::new();
    }
    let accounts: Vec<Account> = discover_all()
        .into_iter()
        .filter(|a| !a.expired())
        .collect();
    std::thread::scope(|scope| {
        let handles: Vec<_> = accounts
            .iter()
            .map(|a| scope.spawn(move || quota_row_of(a)))
            .collect();
        handles
            .into_iter()
            .filter_map(|h| h.join().ok().flatten())
            .collect()
    })
}

/// 单行便捷入口（只取最优账号那一行；多账号用 [`quota_rows`]）。
pub fn quota_row(providers: &[Provider]) -> Option<Value> {
    if providers.iter().any(|p| p.id == CODEX_ID) {
        return None;
    }
    let account = discover()?;
    if account.expired() {
        return None;
    }
    quota_row_of(&account)
}

fn quota_row_of(account: &Account) -> Option<Value> {
    let ver = client_version();
    let url = format!(
        "{}/wham/usage",
        base_url().trim_end_matches('/').trim_end_matches("/codex")
    );
    // /wham 读取：Codex 不发 version/OpenAI-Beta，只有账号头 + 客户端头。
    let response = sign(super::outbound_agent().get(&url), account, &ver, None)
        .timeout(FETCH_TIMEOUT)
        .call()
        .ok()?;
    let mut buf = Vec::new();
    response
        .into_reader()
        .take(1 << 20)
        .read_to_end(&mut buf)
        .ok()?;
    let data: Value = serde_json::from_slice(&buf).ok()?;
    let mut windows = Vec::new();
    for key in ["primary_window", "secondary_window"] {
        if let Some(w) = data
            .pointer(&format!("/rate_limit/{key}"))
            .filter(|w| w.is_object())
        {
            windows.push(window_row(w));
        }
    }
    if windows.is_empty() {
        return None;
    }
    let plan = data
        .get("plan_type")
        .and_then(|p| p.as_str())
        .filter(|p| !p.is_empty())
        .map(String::from)
        .unwrap_or_else(|| account.plan.clone());
    let mut row = json!({
        "provider": CODEX_ID,
        "name": "Codex",
        "icon": "codex-color",
        "windows": windows,
    });
    if !account.email.is_empty() {
        row["user"] = json!(account.email);
    }
    if !plan.is_empty() {
        row["plan"] = json!(plan);
    }
    // 买的/送的 credits（Go：has && !unlimited && balance>0 → "N credits"）。
    if let Some(c) = data.get("credits") {
        let has = c
            .get("has_credits")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let unlimited = c
            .get("unlimited")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let balance = c.get("balance").and_then(|b| {
            b.as_str()
                .map(|s| s.trim().parse::<f64>().ok())
                .unwrap_or_else(|| b.as_f64())
        });
        if let (true, false, Some(n)) = (has, unlimited, balance) {
            if n > 0.0 {
                row["balance"] = json!(format!("{} credits", (n * 100.0).round() / 100.0));
            }
        }
    }
    Some(row)
}

// =====================================================================
// 订阅耗尽判定（Go codex_backend.go codexAccountOut :1734 + routing.go
// failure :570 的词表，纯字符串移植——Rust 侧无 regex crate）
// =====================================================================

/// 词表匹配面：Go `wordsOf`（routing.go :542）+ 小写化 + 去 URL（urlWords）。
struct Words {
    /// 原文（JSON 字符串解出后拼接）小写化——CJK 与大小写不敏感词共用。
    plain: Vec<u8>,
    /// 去 URL 后的小写面（Go urlWords：链接里的词不算厂商说的）。
    stripped: Vec<u8>,
    /// stripped 再去掉非 [a-z0-9]——`usage.?limit` 这类可分离缀的紧凑面。
    squeezed: Vec<u8>,
}

impl Words {
    fn of(body: &[u8]) -> Words {
        let mut plain = body.to_vec();
        // Go wordsOf：带 \u 转义时把 JSON 里的字符串解出来拼在后面再匹配。
        if plain.windows(2).any(|w| w == b"\\u") {
            if let Ok(v) = serde_json::from_slice::<Value>(body) {
                fn walk(v: &Value, out: &mut Vec<u8>) {
                    match v {
                        Value::String(s) => {
                            out.push(b'\n');
                            out.extend_from_slice(s.as_bytes());
                        }
                        Value::Object(m) => {
                            for x in m.values() {
                                walk(x, out);
                            }
                        }
                        Value::Array(a) => {
                            for x in a {
                                walk(x, out);
                            }
                        }
                        _ => {}
                    }
                }
                walk(&v, &mut plain);
            }
        }
        plain.make_ascii_lowercase();
        // Go urlWords = `https?://\S+`：抹掉链接（到下一个空白）。
        let mut stripped = Vec::with_capacity(plain.len());
        let mut i = 0;
        while i < plain.len() {
            let scheme = if plain[i..].starts_with(b"https://") {
                Some(8)
            } else if plain[i..].starts_with(b"http://") {
                Some(7)
            } else {
                None
            };
            match scheme {
                Some(skip) => {
                    i += skip;
                    while i < plain.len() && !(plain[i] as char).is_ascii_whitespace() {
                        i += 1;
                    }
                }
                None => {
                    stripped.push(plain[i]);
                    i += 1;
                }
            }
        }
        let squeezed: Vec<u8> = stripped
            .iter()
            .copied()
            .filter(|b| b.is_ascii_alphanumeric())
            .collect();
        Words {
            plain,
            stripped,
            squeezed,
        }
    }
}

fn has(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len().max(1)).any(|w| w == needle)
}

/// `\b词\b` 的近似：出现处两侧都不是字母数字（Go 词表里的 \b 包围短词）。
fn has_word(hay: &[u8], word: &[u8]) -> bool {
    let mut from = 0;
    while let Some(rel) = hay[from..].windows(word.len()).position(|w| w == word) {
        let at = from + rel;
        let before_ok = at == 0 || !hay[at - 1].is_ascii_alphanumeric();
        let after = at + word.len();
        let after_ok = after >= hay.len() || !hay[after].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        from = at + 1;
    }
    false
}

/// `a` 之后 `span` 字节内出现 `b`（Go `a.{0,N}b` / `a.*b`——后者 span 给足）。
fn has_after(hay: &[u8], a: &[u8], b: &[u8], span: usize) -> bool {
    let mut from = 0;
    while let Some(rel) = hay[from..].windows(a.len().max(1)).position(|w| w == a) {
        let at = from + rel + a.len();
        if has(&hay[at.min(hay.len())..(at + span).min(hay.len())], b) {
            return true;
        }
        from = from + rel + 1;
    }
    false
}

const MAXSPAN: usize = usize::MAX / 4;

// ---- Go routing.go :339-439 的词表（词面 1:1；`.?` 可分离缀用 squeezed 面）----

/// creditWords：账户/钥匙没钱了。
fn credit_words(w: &Words) -> bool {
    let sq = &w.squeezed;
    has(sq, b"insufficientbalance")
        || has(sq, b"insufficientcredit")
        || has(sq, b"insufficientfund")
        || [
            "balance",
            "credit",
            "billing",
            "payment",
            "arrear",
            "overdue",
            "suspended",
        ]
        .iter()
        .any(|x| has(&w.stripped, x.as_bytes()))
        || has(&w.stripped, "余额".as_bytes())
        || has(&w.stripped, "欠费".as_bytes())
        || has(&w.stripped, "充值".as_bytes())
        || (has(&w.stripped, "账户".as_bytes())
            && (has_after(&w.stripped, "账户".as_bytes(), "不足".as_bytes(), MAXSPAN)
                || has_after(&w.stripped, "账户".as_bytes(), "停".as_bytes(), MAXSPAN)))
}

/// brokeWords：429 里说余额本身花光的（`credits? (are )?exhausted` 等）。
fn broke_words(w: &Words) -> bool {
    let sq = &w.squeezed;
    has(sq, b"insufficientbalance")
        || has(sq, b"insufficientcredit")
        || has(sq, b"insufficientfund")
        || has(sq, b"creditsexhausted")
        || has(sq, b"creditexhausted")
        || has(sq, b"creditsareexhausted")
        || has(sq, b"creditareexhausted")
        || has(sq, b"creditbalanceexhausted")
        || has(&w.stripped, "余额不足".as_bytes())
        || has(&w.stripped, "欠费".as_bytes())
        || has(&w.stripped, "请充值".as_bytes())
}

/// usedUpWords：套餐额度用尽（quota / usage limit / budget…）。
fn used_up_words(w: &Words) -> bool {
    let st = &w.stripped;
    let sq = &w.squeezed;
    has(st, b"quota")
        || has(sq, b"usagelimit")
        || has(st, b"out of budget")
        || has_after(st, b"budget", b"exceeded", 32 + 9)
        || has_after(st, b"budget", b"exhausted", 32 + 9)
        || has(sq, b"limitreached")
        || has_after(st, b"hit your", b"limit", MAXSPAN)
        || has_after(st, b"reached your", b"limit", MAXSPAN)
        || has_after(st, b"limit", b"resets", 24 + 6)
        || has_after(st, b"exceeded", b"plan", MAXSPAN)
        || has_after(st, b"exceeded", b"limit", MAXSPAN)
        || ["额度", "用量", "套餐", "上限"]
            .iter()
            .any(|x| has(st, x.as_bytes()))
}

/// rateWords：按分钟/秒的短限速。
fn rate_words(w: &Words) -> bool {
    let sq = &w.squeezed;
    has(sq, b"ratelimit")
        || has(sq, b"frequencylimit")
        || ["persecond", "persec", "perminute", "permin"]
            .iter()
            .any(|x| has(sq, x.as_bytes()))
        || has(&w.stripped, b"too many requests")
        || has_word(&w.stripped, b"rpm")
        || has_word(&w.stripped, b"tpm")
        || ["频率", "太频繁", "过于频繁", "速率"]
            .iter()
            .any(|x| has(&w.stripped, x.as_bytes()))
}

/// plannedWords：429 里说套餐自身额度用尽的（与 rateWords 互斥判）。
fn planned_words(w: &Words) -> bool {
    let st = &w.stripped;
    let sq = &w.squeezed;
    has(st, b"quota")
        || has(sq, b"usagelimit")
        || has_after(st, b"hit your", b"limit", MAXSPAN)
        || has_after(st, b"reached your", b"limit", MAXSPAN)
        || has_after(st, b"limit", b"resets", 24 + 6)
        || ["perday", "perweek", "permonth"]
            .iter()
            .any(|x| has(sq, x.as_bytes()))
        || [
            "daily",
            "weekly",
            "monthly",
            "额度",
            "用量",
            "套餐",
            "限额已用完",
        ]
        .iter()
        .any(|x| has(st, x.as_bytes()))
}

/// minuteWords：窗口是一分钟或更短（TPM/RPM）。
fn minute_words(w: &Words) -> bool {
    let sq = &w.squeezed;
    ["persecond", "perminute", "persec", "permin"]
        .iter()
        .any(|x| has(sq, x.as_bytes()))
        || ["rpm", "tpm", "rps", "tps"]
            .iter()
            .any(|x| has_word(&w.stripped, x.as_bytes()))
        || ["tokenratelimit", "callratelimit", "requestratelimit"]
            .iter()
            .any(|x| has(sq, x.as_bytes()))
        || ["每分钟", "每秒"]
            .iter()
            .any(|x| has(&w.stripped, x.as_bytes()))
}

/// longWords：比一分钟长的窗口或套餐限额（minuteWords 的反面）。
fn long_words(w: &Words) -> bool {
    let st = &w.stripped;
    let sq = &w.squeezed;
    has(sq, b"usagelimit")
        || has_after(st, b"hit your", b"limit", MAXSPAN)
        || has_after(st, b"reached your", b"limit", MAXSPAN)
        || has_after(st, b"limit", b"resets", 24 + 6)
        || ["perhour", "perday", "perweek", "permonth"]
            .iter()
            .any(|x| has(sq, x.as_bytes()))
        || has_word(st, b"hour")
        || has_word(st, b"hours")
        || ["rpd", "rph", "tpd", "tph"]
            .iter()
            .any(|x| has_word(st, x.as_bytes()))
        || [
            "hourly",
            "daily",
            "weekly",
            "monthly",
            "额度已用",
            "用量",
            "套餐",
            "限额已用完",
            "小时",
            "每天",
            "每日",
            "每周",
            "每月",
        ]
        .iter()
        .any(|x| has(st, x.as_bytes()))
}

/// quotaCodes：厂商自己的额度用尽代码（5xx 里光词不算，代码才算）。
fn quota_codes(w: &Words) -> bool {
    if has(&w.stripped, b"usage_limit_reached") || has(&w.stripped, b"resource_exhausted") {
        return true;
    }
    // Go `limit reached\|\d{10}\b`（Claude Code 的 resets 行）。
    let hay = &w.stripped;
    let needle = b"limit reached|";
    let mut from = 0;
    while let Some(rel) = hay[from.min(hay.len())..]
        .windows(needle.len())
        .position(|x| x == needle)
    {
        let at = from + rel + needle.len();
        if at + 10 <= hay.len()
            && hay[at..at + 10].iter().all(|b| b.is_ascii_digit())
            && (at + 10 >= hay.len() || !hay[at + 10].is_ascii_alphanumeric())
        {
            return true;
        }
        from += rel + 1;
    }
    false
}

/// Go minuteRate（routing.go :645）：分钟级限速且没提更长窗口/余额花光。
fn minute_rate(w: &Words) -> bool {
    minute_words(w) && !long_words(w) && !broke_words(w)
}

/// Go failure（routing.go :570）的 Codex 相关子集。跳过的两支：resetsWhen
/// （带时区偏移的整日期正则——WorkBuddy/Ark 的措辞，ChatGPT 的额度拒答走
/// plannedWords/usedUpWords 已覆盖）与 backSoon（要解析等待时长）。
fn failure_class(status: u16, body: &[u8]) -> &'static str {
    let w = Words::of(body);
    let insufficient_quota = has(body, b"insufficient_quota"); // Go bodyHas：大小写敏感原文
    if status == 402 && used_up_words(&w) && !credit_words(&w) && !insufficient_quota {
        return "quota";
    }
    if status == 402
        || (credit_words(&w) && (status != 429 || broke_words(&w)))
        || (insufficient_quota && !minute_rate(&w))
    {
        return "credit";
    }
    if status == 429 {
        if minute_rate(&w) {
            return "rate";
        }
        if rate_words(&w) && !planned_words(&w) {
            return "rate";
        }
        if planned_words(&w) {
            return "quota";
        }
        return "rate";
    }
    if status >= 500 && quota_codes(&w) {
        return "quota";
    }
    if status < 500 && used_up_words(&w) {
        return "quota";
    }
    "other"
}

/// Go codexAccountOut（codex_backend.go :1734）：ChatGPT 后端以「账户不行」
/// （不是请求不行）拒了这一轮——额度用尽 / 没credits / 限速。**401 永远不算**
/// （Go：Codex 在 401 上自己续登录）。命中 = 该换下一账号。
fn codex_account_out(status: u16, body: &[u8]) -> bool {
    if status == 401 {
        return false;
    }
    if status == 403 && has(body, b"usage_not_included") {
        return true;
    }
    failure_class(status, body) != "other"
}

// =====================================================================
// 对话路径
// =====================================================================

fn first_line(s: &str) -> &str {
    s.trim().lines().next().unwrap_or("")
}

/// Go BoundCallID：超过 64 字符的 call id 换成 sha256 hex（同 id 同哈希，
/// 调用与输出仍配对）。
fn bound_call_id(id: &str) -> String {
    if id.len() <= 64 {
        return id.to_string();
    }
    hex(&Sha256::digest(id.trim().as_bytes()))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Go conversationKey：从会话开头（instructions + 到第一条 user 为止的 input）
/// 派生稳定的 prompt_cache_key；没有 user 消息就不造。
fn conversation_key(instructions: &str, input: &[Value]) -> Option<String> {
    let mut hasher = Sha256::new();
    hasher.update(instructions.as_bytes());
    let mut user = false;
    for item in input {
        hasher.update(serde_json::to_vec(item).unwrap_or_default());
        if item.get("role").and_then(|r| r.as_str()) == Some("user") {
            user = true;
            break;
        }
    }
    if !user {
        return None;
    }
    let s = hex(&hasher.finalize());
    Some(format!(
        "{}-{}-{}-{}-{}",
        &s[0..8],
        &s[8..12],
        &s[12..16],
        &s[16..20],
        &s[20..32]
    ))
}

const CALL_TYPES: [&str; 8] = [
    "function_call",
    "function_call_output",
    "local_shell_call",
    "local_shell_call_output",
    "custom_tool_call",
    "custom_tool_call_output",
    "tool_search_call",
    "tool_search_output",
];

/// Go orphanOutput：调用不在 input 里的工具输出，改说成 assistant 的一句备注
/// （后端拒收孤儿 output 项）。
fn orphan_output(it: &Value, id: &str) -> Value {
    let name = it
        .get("name")
        .and_then(|n| n.as_str())
        .filter(|n| !n.is_empty())
        .unwrap_or("tool");
    let id = if id.is_empty() { "unknown" } else { id };
    let mut text = match it.get("output") {
        Some(Value::String(s)) => s.clone(),
        Some(other) => serde_json::to_string(other).unwrap_or_default(),
        None => "null".to_string(),
    };
    if text.len() > 16000 {
        let mut cut = 16000;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push_str("\n...[truncated]");
    }
    json!({ "type": "message", "role": "assistant",
            "content": format!("[Previous {name} result; call_id={id}]: {text}") })
}

/// Go codexInput：丢 item_reference、去 id、message/reasoning 去 status、
/// system→developer、超长 call_id 收成哈希、孤儿工具输出改成备注消息。
fn shape_input(mut input: Vec<Value>) -> Vec<Value> {
    // 第一遍：收口 call_id、记「哪个 call 期望哪种 output」。
    let mut calls: HashMap<String, &'static str> = HashMap::new();
    for item in input.iter_mut() {
        let Some(obj) = item.as_object_mut() else {
            continue;
        };
        let ty = obj.get("type").and_then(|t| t.as_str()).map(String::from);
        let Some(ty) = ty else { continue };
        if !CALL_TYPES.contains(&ty.as_str()) {
            continue;
        }
        let mut id = obj
            .get("call_id")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        let bound = bound_call_id(&id);
        if bound != id {
            id = bound.clone();
            obj.insert("call_id".into(), Value::String(bound));
        }
        let want = match ty.as_str() {
            "function_call" => "function_call_output",
            "local_shell_call" => "local_shell_call_output",
            "custom_tool_call" => "custom_tool_call_output",
            _ => continue,
        };
        calls.insert(id.trim().to_string(), want);
    }
    let mut out = Vec::new();
    for mut item in input {
        let Some(obj) = item.as_object_mut() else {
            out.push(item);
            continue;
        };
        let ty = obj.get("type").and_then(|t| t.as_str()).map(String::from);
        if ty.as_deref() == Some("item_reference") {
            continue;
        }
        // Codex 自己发的无 call id 的独立通知：原样放过。
        let standalone_output = matches!(
            ty.as_deref(),
            Some("function_call_output" | "custom_tool_call_output")
        ) && obj
            .get("call_id")
            .map(|c| c.is_null() || c.as_str() == Some(""))
            .unwrap_or(true);
        if standalone_output {
            out.push(item);
            continue;
        }
        obj.remove("id");
        if matches!(ty.as_deref(), None | Some("message") | Some("reasoning")) {
            obj.remove("status");
        }
        if let Some(
            t @ ("function_call_output" | "custom_tool_call_output" | "local_shell_call_output"),
        ) = ty.as_deref()
        {
            let id = obj
                .get("call_id")
                .and_then(|c| c.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            let want = calls.get(&id).copied().unwrap_or("");
            let paired =
                want == t || (t == "function_call_output" && want == "local_shell_call_output");
            if !paired {
                out.push(orphan_output(&item, &id));
                continue;
            }
        }
        if let Some(obj) = item.as_object_mut() {
            if obj.get("role").and_then(|r| r.as_str()) == Some("system")
                && matches!(ty.as_deref(), None | Some("message"))
            {
                obj.insert("role".into(), Value::String("developer".into()));
            }
        }
        out.push(item);
    }
    out
}

// ---- Go codexToolNames：后端拒收 ^[a-zA-Z0-9_.-]+$ 之外的工具名 ----

fn tool_name_ok(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|c| c == b'_' || c == b'.' || c == b'-' || c.is_ascii_alphanumeric())
}

/// Go CodexToolName：合法原样；否则每个非法**字节**换成 `_`、截 55、
/// 接 `_` + sha256(原名) 前 4 字节 hex（同名恒同结果、不同名不撞）。
pub(crate) fn codex_tool_name(name: &str) -> String {
    if tool_name_ok(name) {
        return name.to_string();
    }
    let mut out: Vec<u8> = name
        .bytes()
        .map(|c| {
            if c == b'_' || c == b'.' || c == b'-' || c.is_ascii_alphanumeric() {
                c
            } else {
                b'_'
            }
        })
        .collect();
    out.truncate(55);
    let sum = Sha256::digest(name.as_bytes());
    format!("{}_{}", String::from_utf8_lossy(&out), hex(&sum[..4]))
}

fn fix_tool_name(o: &mut serde_json::Map<String, Value>, changed: &mut bool) {
    if let Some(n) = o.get("name").and_then(|n| n.as_str()).map(String::from) {
        let c = codex_tool_name(&n);
        if c != n {
            o.insert("name".into(), Value::String(c));
            *changed = true;
        }
    }
}

fn fix_tools(tools: &mut [Value], changed: &mut bool) {
    for t in tools.iter_mut() {
        let Some(tm) = t.as_object_mut() else {
            continue;
        };
        match tm.get("type").and_then(|x| x.as_str()) {
            Some("function" | "custom") => fix_tool_name(tm, changed),
            Some("namespace") => {
                if let Some(nested) = tm.get_mut("tools").and_then(|n| n.as_array_mut()) {
                    fix_tools(nested, changed);
                }
            }
            _ => {}
        }
    }
}

/// 请求里每个工具名（tools / input 里的调用与 additional_tools /
/// tool_choice 点名的那个）都改成后端收的形态；返回是否改过。
fn codex_tool_names(m: &mut serde_json::Map<String, Value>) -> bool {
    let mut changed = false;
    if let Some(ts) = m.get_mut("tools").and_then(|t| t.as_array_mut()) {
        fix_tools(ts, &mut changed);
    }
    if let Some(input) = m.get_mut("input").and_then(|t| t.as_array_mut()) {
        for it in input.iter_mut() {
            let Some(o) = it.as_object_mut() else {
                continue;
            };
            match o.get("type").and_then(|x| x.as_str()) {
                Some("function_call" | "custom_tool_call") => fix_tool_name(o, &mut changed),
                Some("additional_tools" | "tool_search_output") => {
                    if let Some(n) = o.get_mut("tools").and_then(|n| n.as_array_mut()) {
                        fix_tools(n, &mut changed);
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(tc) = m.get_mut("tool_choice").and_then(|t| t.as_object_mut()) {
        match tc.get("type").and_then(|x| x.as_str()) {
            Some("function" | "custom") => fix_tool_name(tc, &mut changed),
            Some("allowed_tools") => {
                if let Some(n) = tc.get_mut("tools").and_then(|n| n.as_array_mut()) {
                    fix_tools(n, &mut changed);
                }
            }
            _ => {}
        }
    }
    changed
}

/// 改名回程表（改过名的声明工具：后端名 → agent 声明名，Go CodexToolNamesBack）。
pub(crate) fn names_back_of(request: &Value) -> super::translate::NamesBack {
    let mut back = super::translate::NamesBack::new();
    fn walk(ts: &[Value], back: &mut super::translate::NamesBack) {
        for t in ts {
            match t.get("type").and_then(|x| x.as_str()) {
                Some("function" | "custom") => {
                    if let Some(n) = t.get("name").and_then(|n| n.as_str()) {
                        let c = codex_tool_name(n);
                        if c != n {
                            back.insert(c, n.to_string());
                        }
                    }
                }
                Some("namespace") => {
                    if let Some(nested) = t.get("tools").and_then(|n| n.as_array()) {
                        walk(nested, back);
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(ts) = request.get("tools").and_then(|t| t.as_array()) {
        walk(ts, &mut back);
    }
    for it in request
        .get("input")
        .and_then(|i| i.as_array())
        .into_iter()
        .flatten()
    {
        if it.get("type").and_then(|t| t.as_str()) == Some("additional_tools") {
            if let Some(ts) = it.get("tools").and_then(|t| t.as_array()) {
                walk(ts, &mut back);
            }
        }
    }
    back
}

// ---- Go declareSearch：回放了 web_search_call 却没声明 web_search 工具会被拒 ----

fn declares_search(tools: &[Value]) -> bool {
    tools.iter().any(|t| {
        let typ = t.get("type").and_then(|x| x.as_str()).unwrap_or("");
        let name = t.get("name").and_then(|x| x.as_str()).unwrap_or("");
        if typ.starts_with("web_search") {
            return true;
        }
        if typ == "function" && name == "web.run" {
            return true;
        }
        typ == "namespace"
            && name == "web"
            && t.get("tools")
                .and_then(|x| x.as_array())
                .is_some_and(|inner| {
                    inner.iter().any(|f| {
                        f.get("type").and_then(|x| x.as_str()) == Some("function")
                            && f.get("name").and_then(|x| x.as_str()) == Some("run")
                    })
                })
    })
}

fn cached_search() -> Value {
    json!({ "type": "web_search", "external_web_access": false })
}

fn declare_search(m: &mut serde_json::Map<String, Value>) -> bool {
    let Some(input) = m.get("input").and_then(|i| i.as_array()).cloned() else {
        return false;
    };
    let (mut searched, mut offered) = (false, false);
    let mut additional: Option<usize> = None;
    for (i, it) in input.iter().enumerate() {
        match it.get("type").and_then(|t| t.as_str()) {
            Some("web_search_call") => searched = true,
            Some("additional_tools") => {
                let tools = it
                    .get("tools")
                    .and_then(|t| t.as_array())
                    .cloned()
                    .unwrap_or_default();
                if declares_search(&tools) {
                    return false;
                }
                offered = offered || !tools.is_empty();
                if additional.is_none() {
                    additional = Some(i);
                }
            }
            _ => {}
        }
    }
    if !searched {
        return false;
    }
    let tools: Vec<Value> = match m.get("tools") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(a)) => a.clone(),
        Some(_) => return false, // 不是列表：留给后端去说
    };
    if declares_search(&tools) {
        return false;
    }
    offered = offered || !tools.is_empty();
    let all_turns = m
        .get("reasoning")
        .and_then(|r| r.get("context"))
        .and_then(|c| c.as_str())
        == Some("all_turns");
    let lite = additional.is_some() || all_turns;
    let mut input = input;
    match additional {
        Some(i) => {
            if let Some(o) = input[i].as_object_mut() {
                let mut have = o
                    .get("tools")
                    .and_then(|t| t.as_array())
                    .cloned()
                    .unwrap_or_default();
                have.push(cached_search());
                o.insert("tools".into(), Value::Array(have));
            }
            m.insert("input".into(), Value::Array(input));
        }
        None if lite => {
            input.insert(0, json!({ "type": "additional_tools", "role": "developer", "tools": [cached_search()] }));
            m.insert("input".into(), Value::Array(input));
        }
        None => {
            let mut tools = tools;
            tools.push(cached_search());
            m.insert("tools".into(), Value::Array(tools));
        }
    }
    if !offered {
        let none = match m.get("tool_choice") {
            None | Some(Value::Null) => true,
            Some(Value::String(c)) => c.is_empty() || c == "auto" || c == "none",
            _ => false,
        };
        if none {
            m.insert("tool_choice".into(), Value::String("none".into()));
        }
    }
    true
}

/// Go codexBody 的移植：把任意 Responses 请求整形成 ChatGPT 后端收的那种
/// （恒流式、不存储、带 Codex 指令、去掉它拒收的采样/缓存参数）。
fn shape_body(mut v: Value, bare: &str, prompt: Option<String>) -> Value {
    let Some(map) = v.as_object_mut() else {
        return v;
    };
    for k in [
        "max_output_tokens",
        "max_completion_tokens",
        "temperature",
        "top_p",
        "previous_response_id",
        "user",
        "safety_identifier",
        "prompt_cache_options",
    ] {
        map.remove(k);
    }
    let tier_ok = matches!(
        map.get("service_tier").and_then(|t| t.as_str()),
        Some("priority" | "ultrafast")
    );
    if !tier_ok {
        map.remove("service_tier");
    }
    map.insert("model".into(), Value::String(bare.to_string()));
    if let Some(s) = map.get("input").and_then(|i| i.as_str()).map(String::from) {
        map.insert(
            "input".into(),
            json!([{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": s }] }]),
        );
    }
    let mut input = match map.remove("input") {
        Some(Value::Array(a)) => shape_input(a),
        _ => Vec::new(),
    };
    let own = map
        .get("instructions")
        .and_then(|i| i.as_str())
        .unwrap_or("")
        .to_string();
    if map
        .get("prompt_cache_key")
        .and_then(|k| k.as_str())
        .unwrap_or("")
        .is_empty()
    {
        if let Some(k) = conversation_key(&own, &input) {
            map.insert("prompt_cache_key".into(), Value::String(k));
        }
    }
    // 客户端自己的指令当 developer 消息打头，Codex 的指令占位——除非它们
    // 本来就是 Codex 的（Codex 自己发来的）。没拿到 Codex 指令就用客户端的。
    let mut instructions = prompt.unwrap_or_default();
    if !own.trim().is_empty() {
        if instructions.is_empty() || first_line(&own) == first_line(&instructions) {
            instructions = own;
        } else {
            input.insert(
                0,
                json!({ "type": "message", "role": "developer",
                        "content": [{ "type": "input_text", "text": own }] }),
            );
        }
    }
    if instructions.is_empty() {
        instructions = FALLBACK_INSTRUCTIONS.to_string();
    }
    map.insert("input".into(), Value::Array(input));
    map.insert("instructions".into(), Value::String(instructions));
    map.insert("store".into(), Value::Bool(false));
    map.insert("stream".into(), Value::Bool(true));
    map.insert("tool_choice".into(), Value::String("auto".into()));
    declare_search(map);
    // 历史里别家模型写的非法工具名会让整个请求被拒（回程由 names_back_of 还原）
    codex_tool_names(map);
    map.entry("parallel_tool_calls")
        .or_insert(Value::Bool(true));
    map.entry("text")
        .or_insert(json!({ "verbosity": "medium" }));

    // 推理：除非关了，否则要摘要并带回加密推理内容（跨轮）；"ultra" 是 Codex
    // CLI 对 "max" 的叫法。
    let effort = map
        .get("reasoning")
        .and_then(|r| r.get("effort"))
        .and_then(|e| e.as_str())
        .unwrap_or("")
        .to_string();
    if effort == "ultra" {
        if let Some(r) = map.get_mut("reasoning").and_then(|r| r.as_object_mut()) {
            r.insert("effort".into(), Value::String("max".into()));
        }
    }
    if effort == "none" {
        map.remove("include");
    } else {
        if let Some(r) = map.get_mut("reasoning").and_then(|r| r.as_object_mut()) {
            r.entry("summary").or_insert(Value::String("auto".into()));
        }
        let mut include = map
            .get("include")
            .and_then(|i| i.as_array())
            .cloned()
            .unwrap_or_default();
        if !include
            .iter()
            .any(|i| i.as_str() == Some("reasoning.encrypted_content"))
        {
            include.push(Value::String("reasoning.encrypted_content".into()));
        }
        map.insert("include".into(), Value::Array(include));
    }
    v
}

/// 各入站协议自己的错误形状（anthropic 客户端读不懂 openai 形状的错误）。
fn proto_error(proto: Proto, status: u16, message: &str) -> Reply {
    match proto {
        Proto::Anthropic => {
            let ty = match status {
                400 => "invalid_request_error",
                401 => "authentication_error",
                403 => "permission_error",
                404 => "not_found_error",
                413 => "request_too_large",
                429 => "rate_limit_error",
                529 => "overloaded_error",
                _ => "api_error",
            };
            Reply::json(
                status,
                &json!({ "type": "error", "error": { "type": ty, "message": message } }),
            )
        }
        _ => Reply::error(status, message),
    }
}

/// 上游错误体里的人话（upstream_error_reply 与耗尽兜底共用）。
fn vendor_message(status: u16, body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| {
            ["/error/message", "/error", "/message", "/detail"]
                .iter()
                .find_map(|p| {
                    v.pointer(p)
                        .and_then(|x| x.as_str())
                        .filter(|s| !s.trim().is_empty())
                        .map(String::from)
                })
        })
        .unwrap_or_else(|| {
            let t = String::from_utf8_lossy(body);
            let t = t.trim();
            if t.is_empty() {
                format!("Codex answered {status} with an empty body")
            } else {
                t.chars().take(400).collect()
            }
        })
}

/// 上游非 200 的应答体 → 入站协议的错误：Responses 入站原样（合同面）；
/// chat / anthropic 入站抠出人话重包成各自的错误形状，状态码透传。
fn upstream_error_reply(proto: Proto, status: u16, body: Vec<u8>) -> Reply {
    if proto == Proto::Responses {
        return Reply::Json { status, body };
    }
    proto_error(proto, status, &vendor_message(status, &body))
}

/// 订阅耗尽兜底应答（Go codexFallBack 的「无下一跳」半边）：429 + 明确文案。
/// `groups::retryable` 对 429 恒真——上层组路由的候选循环据此换成员。
fn exhausted_reply(proto: Proto, upstream_status: u16, body: &[u8]) -> Reply {
    let msg = format!(
        "{}: {}",
        EXHAUSTED_MESSAGE,
        vendor_message(upstream_status, body)
    );
    match proto {
        Proto::Anthropic => proto_error(proto, 429, &msg),
        _ => Reply::json(
            429,
            &json!({ "error": { "message": msg, "type": "rate_limit_error", "code": "usage_limit_reached" } }),
        ),
    }
}

/// SSE 聚合成最终 response 对象（客户端要非流式时）。失败 → (状态, 消息)。
fn aggregate_sse_value(reader: impl Read) -> Result<Value, (u16, String)> {
    let mut items: Vec<Value> = Vec::new();
    let mut final_response: Option<Value> = None;
    for line in BufReader::new(reader).lines() {
        let Ok(line) = line else { break };
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let Ok(event) = serde_json::from_str::<Value>(data.trim()) else {
            continue;
        };
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
                    .unwrap_or("Codex response failed");
                return Err((502, msg.to_string()));
            }
            "error" => {
                let msg = event
                    .get("message")
                    .or_else(|| event.pointer("/error/message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("Codex stream error");
                return Err((502, msg.to_string()));
            }
            _ => {}
        }
    }
    let Some(mut response) = final_response else {
        return Err((502, "Codex stream ended before response.completed".into()));
    };
    let empty_output = response
        .get("output")
        .and_then(|o| o.as_array())
        .map(|o| o.is_empty())
        .unwrap_or(true);
    if empty_output && !items.is_empty() {
        response["output"] = Value::Array(items);
    }
    Ok(response)
}

/// 把 SSE 读成一个最终 response 对象（Responses 入站、客户端要非流式时）。
fn aggregate_sse(reader: impl Read) -> Reply {
    match aggregate_sse_value(reader) {
        Ok(v) => Reply::json(200, &v),
        Err((status, msg)) => Reply::error(status, &msg),
    }
}

/// 出站体 gzip（Go codex_zstd.go CodexCompressed :70 的 gzip 偏差版）：超过
/// [`COMPRESS_MIN`] 的明文 gzip 后 `Content-Encoding: gzip` 发送（flate2 已在
/// 依赖，chat 端 bodyprep 同源）。压缩失败 → 回明文（宁可 503 也不能不发）。
/// Go 用 zstd（codex CLI 同款）；ruzstd 只有解压器，加 C 绑定被红线禁止。
fn gzip_payload(payload: &[u8]) -> Option<Vec<u8>> {
    use flate2::write::GzEncoder;
    if payload.len() <= COMPRESS_MIN {
        return None;
    }
    let mut enc = GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(payload).ok()?;
    enc.finish().ok().filter(|z| !z.is_empty())
}

// =====================================================================
// 出站 WebSocket（Go codex_socket.go + codex_socket_keep.go 的移植；默认
// 关，失败当轮回退 HTTP）
// =====================================================================

/// WS 出站开关：`MARIONETTE_CODEX_WS=1` 时对话出站改走 ChatGPT 后端的
/// WebSocket（HTTP 流在 ~120s 被中间层掐断、WS 能跑完整个回合——Go
/// codex_socket.go 头注释的实测）。**默认关**：稳态发布（HTTP 路径）不受
/// 任何影响；真机验证 WS 路径后可翻默认（把这里的判空改掉即可）。WS 的
/// 任何「应答未交付」失败（握手被拒/连接失败/写失败/应答前断线）当轮回退
/// HTTPS POST（Go errCodexSocketFallBack :213 同款语义），所以开关开着的最
/// 坏情况也只是多一次白连。开着时另有三件 Go 同款记忆：**keep 连接复用**
/// （会话的 socket 留在池里，后续回合以 previous_response_id 只发增量输入，
/// codex_socket_keep.go）、**shun 名单**（握手被拒的主机 10 分钟内直接 HTTP，
/// codex_socket.go :63）、**1009 尺寸记忆**（message too big 后按主机记最小
/// 拒收尺寸，期内同等大小直接 HTTP，:83 refusedTooBig）。
fn ws_enabled() -> bool {
    std::env::var("MARIONETTE_CODEX_WS").is_ok_and(|v| v.trim() == "1")
}

/// Go codexSocketBeta（codex_socket.go :61）：WS 握手的 OpenAI-Beta 值
/// （HTTP 版是 `responses=experimental`，WS 版是另一个常量）。
const WS_BETA: &str = "responses_websockets=2026-02-06";
/// Go codexSocketPrelude（:73）：应答正文交给调用方之前最多读几帧（吸收
/// codex.rate_limits / codex.response.metadata 这类「头帧」+ 首条正文事件）。
const WS_PRELUDE: usize = 8;
/// Go codexSocketDial（:69）：握手 30s 超时。
const WS_DIAL: Duration = Duration::from_secs(30);
/// TCP 连接超时（outbound_agent 的 https 连接是 5s，这里略放宽——WS 会在
/// 回退 HTTP 前多花这一段）。
const WS_CONNECT: Duration = Duration::from_secs(10);
/// 帧流的终态事件（Go codexSocketBody.frame :698/:709：这些类型后 socket
/// 对本回合而言结束，读到它们流收口到 EOF）。
const WS_TERMINAL: [&str; 5] = [
    "response.completed",
    "response.failed",
    "response.incomplete",
    "response.done",
    "error",
];
/// RFC 6455 §7.4.1 关闭码 1009 = message too big（Go 用 coder/websocket 的
/// StatusMessageTooBig 判同一件事，codex_socket.go :530）。
const WS_CLOSE_TOO_BIG: u16 = 1009;

// ---------------------------------------------------------------------
// shun 名单 + 1009 尺寸记忆（Go codex_socket.go :63-134）
// ---------------------------------------------------------------------

/// Go codexSocketShun（:65）：握手被拒的主机多久内不再试 WS；也是 1009
/// 尺寸记忆的窗口（refusedTooBig :107 用同一常量）。
const WS_SHUN: Duration = Duration::from_secs(10 * 60);

/// 主机（host:port）维度的「这阵子别再试 WS」记忆。Go 把 codexShun 与
/// codexTooBig 挂在同一把 codexShunMu 下（:76-81）；锁内只有 map 读写，
/// **绝无网络 IO**（关连接是结构 drop，不占锁）。
struct WsGuard {
    shun: HashMap<String, Instant>,
    too_big: HashMap<String, TooBig>,
}

/// Go tooBig（:83）：该主机 1009 拒收过的**最小**消息尺寸 + 有效期。
struct TooBig {
    size: usize,
    until: Instant,
}

/// 进程级静态（chat.rs `rests()` 的 OnceLock<Mutex> 同款——HashMap::new 非
/// const，进不了 static 初始化）。
fn lock_ws_guard() -> std::sync::MutexGuard<'static, WsGuard> {
    static G: std::sync::OnceLock<Mutex<WsGuard>> = std::sync::OnceLock::new();
    G.get_or_init(|| {
        Mutex::new(WsGuard {
            shun: HashMap::new(),
            too_big: HashMap::new(),
        })
    })
    .lock()
    .unwrap_or_else(|p| p.into_inner())
}

/// Go shunCodexSocket（:126）：主机拒了握手，10 分钟内整轮直接 HTTP。Go
/// 首次记录时打一条日志；engine_gateway 没有日志面，静默（差异记在模块头）。
fn shun_host(host: &str, _why: &str) {
    let mut g = lock_ws_guard();
    g.shun.insert(host.to_string(), Instant::now() + WS_SHUN);
}

/// Go codexShunned（:115）：命中且未过期才拦；过期条目顺手清掉。
fn ws_shunned(host: &str) -> bool {
    let mut g = lock_ws_guard();
    match g.shun.get(host) {
        Some(&until) if Instant::now() >= until => {
            g.shun.remove(host);
            false
        }
        Some(_) => true,
        None => false,
    }
}

/// Go refusedTooBig（:101）：1009 拒收后记下尺寸——比已记的更小或首次才
/// 覆盖 size（保持「最小拒收尺寸」语义），有效期一律续 10 分钟。
fn refused_too_big(host: &str, size: usize) {
    let now = Instant::now();
    let mut g = lock_ws_guard();
    let tb = g
        .too_big
        .entry(host.to_string())
        .or_insert(TooBig { size, until: now });
    if size < tb.size || now >= tb.until {
        tb.size = size;
    }
    tb.until = now + WS_SHUN;
}

/// Go codexMessageTooBig（:90）：这条消息是否已到该主机 1009 拒收过的尺寸。
fn message_too_big(host: &str, size: usize) -> bool {
    let mut g = lock_ws_guard();
    match g.too_big.get(host) {
        Some(tb) if Instant::now() >= tb.until => {
            g.too_big.remove(host);
            false
        }
        Some(tb) => size >= tb.size,
        None => false,
    }
}

/// WS URL → host:port（Go 用 `req.URL.Host` 做 shun/too-big 的键，:379/:531）。
fn ws_host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    match rest.find('/') {
        Some(i) => rest[..i].to_string(),
        None => rest.to_string(),
    }
}

/// dial 失败要不要记 shun（Go socket :375-386 的分类）：
/// - 非 101 的 HTTP 应答（`ws handshake refused: HTTP <code>`）：401/429/5xx
///   是账号自身或瞬时故障，HTTP 会如实说 → **不** shun；其它（403 等）→ shun；
/// - 连接层失败（连不上/TLS/代理/对端早断/Accept 不对——Go 的 hs==nil）→ shun。
fn shun_worthy(err: &str) -> bool {
    match err.strip_prefix("ws handshake refused: HTTP ") {
        Some(rest) => {
            let code: u16 = rest
                .split_whitespace()
                .next()
                .and_then(|c| c.parse().ok())
                .unwrap_or(0);
            !(code == 401 || code == 429 || (500..=599).contains(&code))
        }
        None => true,
    }
}

// ---------------------------------------------------------------------
// keep 连接池（Go codex_socket_keep.go）
// ---------------------------------------------------------------------

/// Go codexKeepIdle（:51）：keep 连接等下一回合的上限。
const KEEP_IDLE: Duration = Duration::from_secs(10 * 60);
/// Go codexKeepAge（:53）：一条连接自握手起的最长使用期（后端 60 分钟断
/// 连，提前 5 分钟弃用）。
const KEEP_AGE: Duration = Duration::from_secs(55 * 60);
/// Go codexKeepMax（:57）：池内最多保持多少条连接。
const KEEP_MAX: usize = 32;

/// 池里的一条 keep 连接 + 上一回合发过/收回的形状（Go codexKept :66）。
/// `conn` drop = CloseNow（TcpStream 落盘即断），对应 Go 的 `close()`。
struct KeptConn {
    conn: super::ws::WsConn,
    /// 会话键（Go codexKeepKey :102 的 sha256 摘要）；空 = 不 keep。
    key: String,
    /// 本连接的握手时刻（Go `opened`，跨回合保留——AGE 从这里算）。
    opened: Instant,
    /// 上次使用时刻（Go `used`，LRU 淘汰用）。
    used: Instant,
    /// 上一回合除 input/client_metadata/stream_options 外的顶层字段
    /// （紧凑 JSON 串；Go `fields`）。
    fields: BTreeMap<String, String>,
    /// 上一回合发送的 input 摘要 + 应答的 output 摘要（Go `input`/`output`）。
    input: Vec<[u8; 32]>,
    output: Vec<[u8; 32]>,
    /// 上一回合 response.completed 的 response.id（Go `replyID`）。
    reply_id: String,
}

/// 进程级连接池（Go codexKeepPool :61 + codexKeepMu；OnceLock<Mutex> 同款
/// 惰性初始化；锁内只有 map 读写，关连接（drop）一律在锁释放后由结构析构
/// 完成，绝不在持锁时做网络 IO）。
fn lock_keep_pool() -> std::sync::MutexGuard<'static, HashMap<String, KeptConn>> {
    static P: std::sync::OnceLock<Mutex<HashMap<String, KeptConn>>> = std::sync::OnceLock::new();
    P.get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

/// Go codexKeepTake（:124）：把会话的连接**取走**（并发回合各开新连接，最后
/// 结束的那条才是留在池里的——「取走」本身就是并发语义）。超龄（55 分钟）弃
/// 用；**与 Go 的偏差**：Go 用 time.AfterFunc 在 idle 10 分钟时主动关，这里
/// 没有定时器线程，idle 也惰性判定（取用时发现超时就弃用）——语义等价
/// （过期连接绝不会被复用），只是空连接在池里多躺到下一次取用/put 清扫。
fn keep_take(key: &str) -> Option<KeptConn> {
    if key.is_empty() {
        return None;
    }
    let k = { lock_keep_pool().remove(key)? };
    if k.opened.elapsed() >= KEEP_AGE || k.used.elapsed() >= KEEP_IDLE {
        return None; // drop = CloseNow（Go :132-135 超龄 close 后返回 nil）
    }
    Some(k)
}

/// Go codexKeepPut（:139）：回合收在 response.completed 时把连接放回池。
/// 空键/超龄不收；同键旧连接被顶替关闭；池超上限按最久未用淘汰；顺手清扫
/// 已过 idle/age 的条目（Go 靠 AfterFunc，这里靠 put/take 时的惰性清扫）。
fn keep_put(mut k: KeptConn) {
    if k.key.is_empty() || k.opened.elapsed() >= KEEP_AGE {
        return; // drop = close（Go :142-145）
    }
    k.used = Instant::now();
    let mut pool = lock_keep_pool();
    let now = Instant::now();
    pool.retain(|_, e| now < e.used + KEEP_IDLE && e.opened.elapsed() < KEEP_AGE);
    let old = pool.insert(k.key.clone(), k); // 顶替的旧连接在此 drop（close）
    drop(old);
    if pool.len() > KEEP_MAX {
        // Go :155-162：淘汰最久未用的那一条。
        let oldest = pool
            .iter()
            .min_by_key(|(_, e)| e.used)
            .map(|(key, _)| key.clone());
        if let Some(key) = oldest {
            let evicted = pool.remove(&key);
            drop(evicted);
        }
    }
}

// ---------------------------------------------------------------------
// 回合形状与增量（Go codexShape / codexSocketItem / codexKept.delta）
// ---------------------------------------------------------------------

/// 一个回合可被下一回合增量续跑的形状（Go codexTurnShape :209）。
#[derive(Clone)]
struct TurnShape {
    /// 除 input/client_metadata/stream_options 外的顶层字段 → 紧凑 JSON 串
    /// （Go :231-241 的 json.Compact；serde_json 序列化本就是紧凑且键序
    /// 确定——BTreeMap——两回合间可稳定比对）。
    fields: BTreeMap<String, String>,
    /// input 的原始条目（拼增量时取尾部；Go `input []json.RawMessage`）。
    items: Vec<Value>,
    /// input 条目的摘要（Go `sums []itemSum` = sha256）。
    sums: Vec<[u8; 32]>,
}

/// Go codexShape（:218）：body 能否参与 keep 续跑——input 必须是条目数组、
/// body 自身不点名 previous_response_id（点名的回合已是客户端自拼的增量，
/// 不再续）；每条 input 还要能算出摘要。
fn turn_shape(body: &Value) -> Option<TurnShape> {
    let top = body.as_object()?;
    if top.contains_key("previous_response_id") {
        return None;
    }
    let input = top.get("input")?.as_array()?;
    let mut sums = Vec::with_capacity(input.len());
    for item in input {
        sums.push(item_digest(item)?);
    }
    let fields: BTreeMap<String, String> = top
        .iter()
        .filter(|(k, _)| {
            k.as_str() != "input"
                && k.as_str() != "client_metadata"
                && k.as_str() != "stream_options"
        })
        .map(|(k, v)| {
            (
                k.as_str().to_string(),
                serde_json::to_string(v).unwrap_or_default(),
            )
        })
        .collect();
    Some(TurnShape {
        fields,
        items: input.clone(),
        sums,
    })
}

/// Go codexSocketItem（:256）：条目的「后端口径」摘要——顶层去 id/status；
/// 任意深度去 annotations、logprobs、null 值与空列表（Codex 与后端都不保
/// 留这些），序列化后取 sha256。非对象条目不可摘要（None）。
fn item_digest(item: &Value) -> Option<[u8; 32]> {
    if !item.is_object() {
        return None;
    }
    let mut v = item.clone();
    {
        let obj = v.as_object_mut()?;
        obj.remove("id");
        obj.remove("status");
    }
    let bare = item_bare(v);
    let bytes = serde_json::to_vec(&sorted_value(&bare)).ok()?;
    Some(Sha256::digest(&bytes).into())
}

/// 序列化前把对象键递归排序（Go `json.Marshal` 对 map 按键排序的对应物）。
/// 必要性实锤：serde_json 的 preserve_order 被依赖图传递点亮，Map 走
/// IndexMap 且 `remove` 是 swap_remove——剥除 id/status 后键序随原位置漂移，
/// 同一逻辑条目会得出不同摘要（2026-11-18 keep 复用集成测试实测定位）。
/// Go 的摘要经 map 往返天然有序；这里显式排序对齐，键序从此与摘要无关。
fn sorted_value(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let mut out = serde_json::Map::new();
            for k in keys {
                out.insert(k.clone(), sorted_value(&m[k]));
            }
            Value::Object(out)
        }
        Value::Array(a) => Value::Array(a.iter().map(sorted_value).collect()),
        other => other.clone(),
    }
}

/// Go codexSocketItemBare（:276）：递归剥「两边都不保留」的字段。
fn item_bare(v: Value) -> Value {
    match v {
        Value::Object(mut m) => {
            m.retain(|k, e| {
                if k == "annotations" || k == "logprobs" || e.is_null() {
                    return false;
                }
                if let Value::Array(a) = e {
                    if a.is_empty() {
                        return false;
                    }
                }
                true
            });
            for (_, e) in m.iter_mut() {
                *e = item_bare(e.take());
            }
            Value::Object(m)
        }
        Value::Array(a) => Value::Array(a.into_iter().map(item_bare).collect()),
        other => other,
    }
}

/// Go codexReplyItem（:359）：response.output_item.done 帧的 item 摘要
/// （item 不是对象 = None → 形状作废，Go :692-696 把 shape 置 nil）。
fn reply_item_digest(frame: &[u8]) -> Option<[u8; 32]> {
    let v: Value = serde_json::from_slice(frame).ok()?;
    let item = v.get("item").filter(|i| i.is_object())?;
    item_digest(item)
}

/// response.completed 帧的 response.id（Go :700 gjson `response.id`；空 = 没有）。
fn completed_response_id(frame: &[u8]) -> String {
    serde_json::from_slice::<Value>(frame)
        .ok()
        .and_then(|v| {
            v.pointer("/response/id")
                .and_then(|i| i.as_str())
                .map(String::from)
        })
        .unwrap_or_default()
}

/// Go codexKept.delta（:300）：本回合能否作为池内连接的增量续跑——除
/// input/client_metadata/stream_options 外字段全同；input 的前缀 = 上一回合
/// 发过的 input + 上一回合应答的 output（逐条摘要相等）。可行则产出
/// response.create 消息：input 只留新条目 + previous_response_id 指向上一
/// 回合。Go 用字节拼贴保原序，这里改经 Value 重组（serde 键序确定，后端
/// 只看 JSON 语义）。
fn delta_message(kept: &KeptConn, body: &Value, shape: &TurnShape) -> Option<Vec<u8>> {
    if kept.reply_id.is_empty() || shape.fields != kept.fields {
        return None;
    }
    let n = kept.input.len() + kept.output.len();
    if shape.sums.len() < n
        || shape.sums[..kept.input.len()] != kept.input[..]
        || shape.sums[kept.input.len()..n] != kept.output[..]
    {
        return None;
    }
    let mut out = body.clone();
    let rest: Vec<Value> = shape.items.get(n..)?.to_vec();
    if let Some(map) = out.as_object_mut() {
        map.insert("input".into(), Value::Array(rest));
        map.insert(
            "previous_response_id".into(),
            Value::String(kept.reply_id.clone()),
        );
    }
    let payload = serde_json::to_vec(&out).ok()?;
    Some(response_create_message(&payload))
}

/// Go codexKeepKey（:102）：会话键 = 出站 URL + 鉴权（Authorization +
/// chatgpt-account-id）+ 会话标识（session-id/thread-id 头；我们的出站头
/// 由 body.prompt_cache_key 派生，见 [`ws_plan`]）串接后 sha256——别的
/// 账号/别的会话绝不上同一条连接。Go 还计入 X-Api-Key/Api-Key/
/// X-Openai-Subagent/X-Codex-Parent-Thread-Id 头，我们的出站从不设它们，
/// 无从分叉，不搬。None = 请求不归属任何会话（不 keep）。
fn keep_key(plan: &WsPlan) -> Option<String> {
    let header = |name: &str| {
        plan.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    };
    let (sid, tid) = (header("session-id"), header("thread-id"));
    if sid.is_empty() && tid.is_empty() {
        return None;
    }
    let mut h = Sha256::new();
    for part in [
        plan.url.as_str(),
        header("Authorization"),
        header("chatgpt-account-id"),
        sid,
        tid,
    ] {
        h.update(part.as_bytes());
        h.update(&[0]);
    }
    Some(hex(&h.finalize()))
}

/// 出站字节流（HTTP 应答体或 WS 帧还原的 SSE 行流；`Reply::Stream` 与翻译
/// 层的 reader 同型）。
type ByteStream = Box<dyn Read + Send>;

/// base URL（https://…/codex）→ WS URL（wss://…/codex/responses）。
fn ws_url(base: &str) -> String {
    let b = base.trim_end_matches('/');
    let ws = if let Some(rest) = b.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = b.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        b.to_string()
    };
    format!("{ws}/responses")
}

/// WS 出站计划（Go codexSocketMessage :222 + codexCreate :265 +
/// codexSocketHeaders :334）：握手 URL / 握手头 / 单帧消息。None = 请求不合
/// WS 形状（非流式或带 type——shape_body 恒满足，防御再判一次）→ HTTP。
struct WsPlan {
    url: String,
    headers: Vec<(String, String)>,
    message: Vec<u8>,
}

fn ws_plan(account: &Account, ver: &str, body: &Value) -> Option<WsPlan> {
    // Go codexSocketMessage :247：只接 stream:true 且不带 type 的整段回合。
    if body.get("type").is_some() || body.get("stream").and_then(|s| s.as_bool()) != Some(true) {
        return None;
    }
    let payload = serde_json::to_vec(body).ok()?;
    // Go :252-259 把 X-Codex-Turn-State / responses-lite 头挪进 client_metadata
    // ——我们的出站请求从不带这两个头（sign 不设、Codex CLI 也不经过我们），
    // 无可迁移，跳过。
    let message = response_create_message(&payload);
    // 头 = sign() 会设的那些，去掉 Go codexSocketHeaders :338 的剥除名单
    // （Content-Type/Content-Length/Content-Encoding/Accept/Accept-Encoding/
    // 幂等键/X-Codex-Turn-State/responses-lite/Host/OpenAI-Beta），再覆写
    // WS 的 OpenAI-Beta。鉴权头放 Upgrade 请求头里（任务要求：照 HTTP 版
    // 同款）。
    let mut headers: Vec<(String, String)> = vec![
        ("Authorization".into(), format!("Bearer {}", account.token)),
        ("originator".into(), "codex_cli_rs".into()),
        ("User-Agent".into(), user_agent(ver)),
        ("version".into(), ver.to_string()),
        ("OpenAI-Beta".into(), WS_BETA.into()),
    ];
    if !account.account_id.is_empty() {
        headers.push(("chatgpt-account-id".into(), account.account_id.clone()));
    }
    if let Some(key) = body
        .get("prompt_cache_key")
        .and_then(|k| k.as_str())
        .filter(|k| !k.is_empty())
    {
        headers.push(("session-id".into(), key.to_string()));
        headers.push(("thread-id".into(), key.to_string()));
    }
    Some(WsPlan {
        url: ws_url(&base_url()),
        headers,
        message,
    })
}

/// Go codexCreate（:265）：HTTP 体前面接 `{"type":"response.create",`——原
/// JSON 对象 `{` 之后的字节原样保留（键序不变），非空对象补逗号，`{}` 则
/// 不补。消息 = 一帧 text 发出（Go :427）。
fn response_create_message(body: &[u8]) -> Vec<u8> {
    let body = body.trim_ascii();
    let rest = if body.first() == Some(&b'{') {
        &body[1..]
    } else {
        body
    };
    let mut out = Vec::with_capacity(body.len() + 32);
    out.extend_from_slice(br#"{"type":"response.create""#);
    if !rest.is_empty() && rest.first() != Some(&b'}') {
        out.push(b',');
    }
    out.extend_from_slice(rest);
    out
}

/// error 帧先于正文时合成的 HTTP 形状（Go codexSocketError :539）：
/// - `error.code == "websocket_connection_limit_reached"` → `Err(())`：回退
///   HTTP（再开新 socket 也会被限）；
/// - `status` / `status_code` 在 400..=599 → `Ok((status, body))`，body =
///   `{"error":…}`（error 是对象时）或原帧——喂现有的错误路径（账号轮换
///   的 codex_account_out 照常判定）；
/// - 其它（无可用 status）→ None：当普通事件传下去（Go :469 两者皆空落空）。
/// Go 还从帧的 headers/error.headers 拼应答头——我们的管线不透传上游头，
/// 跳过。
fn ws_error_reply(frame: &[u8]) -> Option<Result<(u16, Vec<u8>), ()>> {
    let v: Value = serde_json::from_slice(frame).ok()?;
    if v.pointer("/error/code").and_then(|c| c.as_str())
        == Some("websocket_connection_limit_reached")
    {
        return Some(Err(()));
    }
    let status = v
        .get("status")
        .and_then(|s| s.as_u64())
        .or_else(|| v.get("status_code").and_then(|s| s.as_u64()))
        .unwrap_or(0);
    if !(400..=599).contains(&status) {
        return None;
    }
    let status = status as u16;
    let body = match v.get("error") {
        Some(e) if e.is_object() => {
            serde_json::to_vec(&json!({ "error": e })).unwrap_or_else(|_| frame.to_vec())
        }
        _ => frame.to_vec(),
    };
    Some(Ok((status, body)))
}

/// 一帧 → SSE 事件行（Go sseEvent :634）：`event: <type>\n\ndata: <帧>\n\n`；
/// 帧里带 \r/\n 时先压成紧凑 JSON（data 行里不能有裸换行；压不动就原样）。
/// 每条 WS 消息就是一条 SSE 事件的搬运（Go 同款直拼）。
fn ws_sse_event(kind: &str, frame: &[u8]) -> Vec<u8> {
    let frame = if frame.contains(&b'\r') || frame.contains(&b'\n') {
        match serde_json::from_slice::<Value>(frame) {
            Ok(v) => serde_json::to_vec(&v).unwrap_or_else(|_| frame.to_vec()),
            Err(_) => frame.to_vec(),
        }
    } else {
        frame.to_vec()
    };
    let mut out = Vec::with_capacity(kind.len() + frame.len() + 18);
    if !kind.is_empty() {
        out.extend_from_slice(format!("event: {kind}\n").as_bytes());
    }
    out.extend_from_slice(b"data: ");
    out.extend_from_slice(&frame);
    out.extend_from_slice(b"\n\n");
    out
}

/// 帧载荷的 JSON `type` 字段（gjson 同语义：非对象/没有 = 空串）。
fn json_type_of(data: &[u8]) -> String {
    serde_json::from_slice::<Value>(data)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(String::from))
        .unwrap_or_default()
}

/// 本回合结束后要不要把连接放回池（Go codexSocketBody 的 shape + codexKept
/// 接续半边）：completed 且有 id 才 keep，output 摘要随帧累计。
struct KeepTurn {
    key: String,
    /// 本连接的握手时刻（新连接 = 本回合开始；复用连接 = 池里带出来的原值，
    /// Go codexKept.opened 跨回合不重置）。
    opened: Instant,
    fields: BTreeMap<String, String>,
    input: Vec<[u8; 32]>,
    output: Vec<[u8; 32]>,
}

/// WS 帧流 → SSE 行流（Go codexSocketBody :653）：每条 text 帧一条 SSE 事件
/// 直拼；`responsesapi.websocket_timing` 丢弃（:688）；终态事件后收口——
/// keep 场景下**不关连接**：response.completed 且有 id 且形状还在时把连接
/// 交回池（conn.take()，Go :700-705 codexKeepPut），否则正常关闭
/// （closeNormally :707）。正文中途断线/被关 = 流被掐（HTTP 半截同款，
/// io::Error 交给下游按「流提前结束」处理——聚合层据此 502；中途的 1009
/// **不**记尺寸——应答已部分交付，Go TestCodexSocketTooBigMidReplyIsCut）。
/// 调用方半途放弃（reader 被 drop）= 连接直接断（CloseNow，Go Close() :745）。
struct WsSseReader {
    conn: Option<super::ws::WsConn>,
    pending: Vec<u8>,
    done: bool,
    err: Option<std::io::Error>,
    keep: Option<KeepTurn>,
}

impl Read for WsSseReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.pending.is_empty() && !self.done && self.err.is_none() {
            let incoming = match self.conn.as_mut() {
                Some(c) => c.read_message(),
                // conn 只有在置 done 的同一刻才会被 take——循环条件保证进不来。
                None => unreachable!("ws conn kept while stream not done"),
            };
            match incoming {
                Ok(super::ws::Incoming::Text(data)) => {
                    let data = data.trim_ascii().to_vec();
                    let kind = json_type_of(&data);
                    if kind == "responsesapi.websocket_timing" {
                        continue;
                    }
                    self.pending = ws_sse_event(&kind, &data);
                    frame_keep_step(&kind, &data, &mut self.keep, &mut self.conn, &mut self.done);
                }
                Ok(super::ws::Incoming::Binary(_)) => continue, // Go :683 非 text 帧丢弃
                Ok(super::ws::Incoming::Close(code)) => {
                    let why = match code {
                        Some(c) => format!("closed by peer ({c})"),
                        None => "closed by peer".to_string(),
                    };
                    self.err = Some(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        format!("codex websocket: {why} before the turn's end"),
                    ));
                }
                Err(e) => {
                    self.err = Some(std::io::Error::new(
                        std::io::ErrorKind::ConnectionAborted,
                        format!("codex websocket: {e}"),
                    ));
                }
            }
        }
        if !self.pending.is_empty() {
            let n = self.pending.len().min(buf.len());
            buf[..n].copy_from_slice(&self.pending[..n]);
            self.pending.drain(..n);
            return Ok(n);
        }
        if let Some(e) = self.err.take() {
            return Err(e);
        }
        Ok(0)
    }
}

/// 一条正文帧对 keep 侧的处置（Go codexSocketBody.frame :687-712，prelude
/// 与 reader 共用——prelude 里读到 completed 同样当场放回池）：
/// - `response.output_item.done`：item 摘要累计进本回合 output（item 不可
///   摘要 = 形状作废，本回合不再 keep，Go :692-696）；
/// - `response.completed`：有 id 且形状还在 → 连接放回池（不关）；否则正常
///   关闭；
/// - 终态事件：done = true（流收口到 EOF）。
fn frame_keep_step(
    kind: &str,
    data: &[u8],
    keep: &mut Option<KeepTurn>,
    conn: &mut Option<super::ws::WsConn>,
    done: &mut bool,
) {
    if kind == "response.output_item.done" {
        match (keep.as_mut(), reply_item_digest(data)) {
            (Some(k), Some(d)) => k.output.push(d),
            (Some(_), None) => *keep = None, // 形状作废：不 keep（Go shape=nil）
            _ => {}
        }
    }
    if kind == "response.completed" {
        let id = completed_response_id(data);
        if let Some(k) = keep.take().filter(|_| !id.is_empty()) {
            if let Some(conn) = conn.take() {
                keep_put(KeptConn {
                    conn,
                    key: k.key,
                    opened: k.opened,
                    used: Instant::now(),
                    fields: k.fields,
                    input: k.input,
                    output: k.output,
                    reply_id: id,
                });
                // 连接已回池：终态置 done 后 reader 只吐 pending，不再碰连接。
                *done = true;
                return;
            }
        }
    }
    if WS_TERMINAL.contains(&kind) {
        *done = true;
        if let Some(c) = conn.as_mut() {
            c.close_normally();
        }
    }
}

/// WS 回合的四种收尾（对应 Go turn :415 的路径）。
enum WsOutcome {
    /// 200 应答已在手（SSE 行流）。
    Stream(ByteStream),
    /// error 帧先来：合成 (status, body) 走现有错误路径（账号轮换照常）。
    Error(u16, Vec<u8>),
    /// 应答未交付（握手被拒/连接失败/写失败/应答前断线）→ 当轮回退 HTTP
    /// （Go errCodexSocketFallBack :213；连接上限错误也在此列，Go :541）。
    FallBack,
    /// 复用的 keep 连接在应答交付前失败 → 关连接、换新连接整段重发
    /// （Go errCodexSocketAgain :216；由 [`open_upstream_ws`] 消化，不外漏）。
    Again,
}

/// 已连接的 socket 上的回合：发消息（一帧 text）→ prelude 读帧（头帧吸收
/// + 首条正文事件缓冲 + error 帧前置判定）→ 交出 SSE 流。dial 与本函数的
/// 拆分让 prelude/还原逻辑可以用内存双工测试（真 WS 端到端待真机）。
/// `reused`：跑在池里取出的 keep 连接上（Go turn 的 reused 参数——应答交付
/// 前的任何失败，含 error 帧，都是 Again 而非回退 HTTP；error 帧在复用连接
/// 上不分类，新连接会替它自己定性，Go :462-466）。`keep`：回合收在
/// completed 时连接的去处（None = 不 keep）。
fn ws_turn_over(
    mut conn: super::ws::WsConn,
    message: &[u8],
    host: &str,
    reused: bool,
    keep: Option<KeepTurn>,
) -> WsOutcome {
    // 应答交付前失败：1009 先记尺寸（对 delta/整段一样按实发字节数记，Go
    // codexSocketEnded :529-533），再按是否复用连接决定 Again / 回退。
    let again_or_fallback = |code: Option<u16>| {
        if code == Some(WS_CLOSE_TOO_BIG) {
            refused_too_big(host, message.len());
        }
        if reused {
            WsOutcome::Again
        } else {
            WsOutcome::FallBack
        }
    };
    if conn.send_text(message).is_err() {
        // Go turn :427-429：写失败同样按 reused 分流（写错误没有关闭码，不记）。
        return if reused {
            WsOutcome::Again
        } else {
            WsOutcome::FallBack
        };
    }
    let mut conn = Some(conn);
    let mut keep = keep;
    let mut pending: Vec<u8> = Vec::new();
    for _ in 0..WS_PRELUDE {
        let data = match conn.as_mut().unwrap().read_message() {
            Ok(super::ws::Incoming::Text(d)) => d,
            // 非 text 帧照 Go（:683 frame 返回 nil,nil）丢弃；但 error 帧判定
            // 只对 text 帧——binary 帧不是回合应答的一部分。
            Ok(super::ws::Incoming::Binary(_)) => continue,
            // 应答前断线：什么都没交付（Go codexSocketEnded :529）。
            Ok(super::ws::Incoming::Close(code)) => return again_or_fallback(code),
            Err(_) => return again_or_fallback(None),
        };
        let data = data.trim_ascii().to_vec();
        let kind = json_type_of(&data);
        if kind == "responsesapi.websocket_timing" {
            continue; // 与时序帧同款丢弃（Go :688；prelude 段同样不转发）
        }
        if kind == "error" {
            if reused {
                // Go :462-465：复用连接上的任何 error 帧（previous_response_
                // not_found、连接上限……）→ Again，整段重发由新回合自己定性。
                return WsOutcome::Again;
            }
            match ws_error_reply(&data) {
                Some(Err(())) => return WsOutcome::FallBack,
                Some(Ok((status, body))) => return WsOutcome::Error(status, body),
                None => {}
            }
        }
        pending.extend_from_slice(&ws_sse_event(&kind, &data));
        if kind != "codex.rate_limits" && kind != "codex.response.metadata" {
            // 首条正文就是终态（如无 status 的 error 帧）：流到此收口，
            // reader 不再回头读连接（否则空连接上会报读错而非干净 EOF）。
            // completed 在 prelude 里出现时连接当场回池（Go frame 同款）。
            let mut done = false;
            frame_keep_step(&kind, &data, &mut keep, &mut conn, &mut done);
            return WsOutcome::Stream(Box::new(WsSseReader {
                conn,
                pending,
                done,
                err: None,
                keep,
            }));
        }
        // 头帧（rate_limits / metadata）不参与 keep 处置（Go frame 的 switch
        // 对它们无分支），只进 pending。
    }
    WsOutcome::Stream(Box::new(WsSseReader {
        conn,
        pending,
        done: false,
        err: None,
        keep,
    }))
}

/// WS 出站入口（Go overSocket :162 的形状）：
/// 1. shun 名单命中 → 整轮直接 HTTP（不动 keep 池，Go RoundTrip :147）；
/// 2. 有会话键且形状可解析 → 从池里取连接，能续跑就发增量
///    （previous_response_id + 只新输入）；增量尺寸已到 1009 记忆线 → 弃用
///    池内连接走新连接；复用连接应答交付前失败（Again）→ 也走新连接；
/// 3. 整段消息已到 1009 记忆线 → 直接 HTTP；
/// 4. dial 新连接 → 回合；握手被拒按 Go :375-386 的分类决定要不要 shun。
fn open_upstream_ws(account: &Account, ver: &str, body: &Value) -> WsOutcome {
    let Some(plan) = ws_plan(account, ver, body) else {
        return WsOutcome::FallBack;
    };
    let host = ws_host_of(&plan.url);
    if ws_shunned(&host) {
        return WsOutcome::FallBack;
    }
    // Go overSocket :178-186：键与形状成对——形状解析失败键作废（不取池）。
    let key = match (keep_key(&plan), turn_shape(body)) {
        (Some(k), Some(s)) => Some((k, s)),
        _ => None,
    };
    if let Some((key, shape)) = key.clone() {
        if let Some(kept) = keep_take(&key) {
            // 能续跑就只发增量；不能（字段变了/前缀对不上/上轮无 id）就整段
            // 发在同一条连接上（Go :188-190）。
            let send = delta_message(&kept, body, &shape).unwrap_or_else(|| plan.message.clone());
            if !message_too_big(&host, send.len()) {
                let keep = KeepTurn {
                    key,
                    opened: kept.opened,
                    fields: shape.fields.clone(),
                    input: shape.sums.clone(),
                    output: Vec::new(),
                };
                // kept 的其余字段（fields/input/output/reply_id）已被增量判定
                // 消费；conn 交给回合，失败即弃（Again/交付前断连都关连接）。
                match ws_turn_over(kept.conn, &send, &host, true, Some(keep)) {
                    WsOutcome::Again => {} // 换新连接整段重发（Go :196-198）
                    other => return other,
                }
            }
            // 增量太大：池内连接弃用（drop = CloseNow，Go :192-194 k.close()），
            // 落到下面新连接路径。
        }
    }
    if message_too_big(&host, plan.message.len()) {
        return WsOutcome::FallBack; // 整段也已到记忆线 → 直接 HTTP（Go :201-203）
    }
    match super::ws::dial(&plan.url, &plan.headers, WS_CONNECT, WS_DIAL) {
        Ok(conn) => {
            let keep = key.map(|(key, shape)| KeepTurn {
                key,
                opened: Instant::now(),
                fields: shape.fields,
                input: shape.sums,
                output: Vec::new(),
            });
            ws_turn_over(conn, &plan.message, &host, false, keep)
        }
        Err(e) => {
            // 握手被拒/连不上：当轮回退 HTTP；要不要记 shun按 Go :375-386 分类。
            if shun_worthy(&e) {
                shun_host(&host, &e);
            }
            WsOutcome::FallBack
        }
    }
}

/// 打开上游流（HTTP 或 WS）。WS 开（`MARIONETTE_CODEX_WS=1`）时先试 WS，
/// 未交付即回退 HTTP——HTTP 半边与原路径一字不差。
fn open_stream(account: &Account, ver: &str, body: &Value) -> Result<ByteStream, (u16, Vec<u8>)> {
    if ws_enabled() {
        match open_upstream_ws(account, ver, body) {
            WsOutcome::Stream(reader) => return Ok(reader),
            WsOutcome::Error(status, body) => return Err((status, body)),
            // Again 由 open_upstream_ws 内部消化（换新连接重发）；漏到这里
            // 属缺陷，按回退 HTTP 兜底（不该让应答丢失）。
            WsOutcome::FallBack | WsOutcome::Again => {}
        }
    }
    open_upstream(account, ver, body).map(|r| Box::new(r.into_reader()) as ByteStream)
}

/// 打开上游流：200 → Response（SSE 待读）；其它 → Err(Reply 由调用方按入站
/// 协议整形)。大请求体 gzip 出站。
fn open_upstream(
    account: &Account,
    ver: &str,
    body: &Value,
) -> Result<ureq::Response, (u16, Vec<u8>)> {
    let url = format!("{}/responses", base_url().trim_end_matches('/'));
    let payload = serde_json::to_vec(body).unwrap_or_default();
    let mut request = sign(super::outbound_agent().post(&url), account, ver, Some(body))
        .set("Content-Type", "application/json");
    // Go CodexCompressed：已带非 identity 编码的绝不二压（sign 从不设它，
    // 这里照抄守卫语义）。
    let already = request
        .header("content-encoding")
        .is_some_and(|v| !v.eq_ignore_ascii_case("identity"));
    let wire = if !already {
        gzip_payload(&payload)
    } else {
        None
    };
    if wire.is_some() {
        request = request.set("Content-Encoding", "gzip");
    }
    let bytes = wire.unwrap_or_else(|| payload.clone());
    let response = match request.send_bytes(&bytes) {
        Ok(r) => r,
        // 厂商的 4xx/5xx 是合同面的一部分，原样取回（chat.rs 同款）。
        Err(ureq::Error::Status(_, r)) => r,
        Err(e) => {
            let msg =
                json!({ "error": { "message": format!("codex: {e}"), "type": "engine_error" } });
            return Err((502, serde_json::to_vec(&msg).unwrap_or_default()));
        }
    };
    let status = response.status();
    if status != 200 {
        let mut buf = Vec::new();
        let _ = response.into_reader().take(1 << 20).read_to_end(&mut buf);
        return Err((status, buf));
    }
    Ok(response)
}

/// 一次轮换里的失败：原样错误（现有路径）或「全部账号都耗尽」。
enum UpstreamFailure {
    Raw(u16, Vec<u8>),
    Exhausted { upstream: u16, body: Vec<u8> },
}

/// 按账号序试到成功（Go codexFallBack 的账号轮换半边）：额度类错误
/// （[`codex_account_out`]）换下一账号重发**同一个**整形请求；其它错误
/// （401/400/5xx 过载……）原样返回——那些不是「这个账号不行」。全部账号
/// 都耗尽 → [`UpstreamFailure::Exhausted`]。WS 开关开时每个账号先试 WS、
/// 未交付回退该账号的 HTTP（[`open_stream`]）。
fn open_stream_rotating(
    accounts: &[Account],
    ver: &str,
    body: &Value,
) -> Result<ByteStream, UpstreamFailure> {
    let mut last: Option<(u16, Vec<u8>)> = None;
    for (i, account) in accounts.iter().enumerate() {
        match open_stream(account, ver, body) {
            Ok(r) => return Ok(r),
            Err((status, err_body)) => {
                let out = codex_account_out(status, &err_body);
                if out && i + 1 < accounts.len() {
                    last = Some((status, err_body));
                    continue;
                }
                if out {
                    return Err(UpstreamFailure::Exhausted {
                        upstream: status,
                        body: err_body,
                    });
                }
                return Err(UpstreamFailure::Raw(status, err_body));
            }
        }
    }
    // 到这里只有一个可能：accounts 为空（调用方保证非空，防御一下）。
    match last {
        Some((status, body)) => Err(UpstreamFailure::Raw(status, body)),
        None => Err(UpstreamFailure::Raw(
            502,
            br#"{"error":{"message":"codex: no account to try","type":"engine_error"}}"#.to_vec(),
        )),
    }
}

fn forward_responses(
    accounts: &[Account],
    ver: &str,
    body: &Value,
    client_wants_stream: bool,
) -> Reply {
    let response = match open_stream_rotating(accounts, ver, body) {
        Ok(r) => r,
        Err(UpstreamFailure::Raw(status, body)) => return Reply::Json { status, body },
        Err(UpstreamFailure::Exhausted { upstream, body }) => {
            return exhausted_reply(Proto::Responses, upstream, &body)
        }
    };
    if client_wants_stream {
        return Reply::Stream {
            status: 200,
            content_type: "text/event-stream".into(),
            reader: response,
        };
    }
    aggregate_sse(response)
}

/// chat / anthropic 入站：已翻成 Responses 并整形的请求 → 上游 → 翻回去。
fn forward_translated(
    proto: Proto,
    accounts: &[Account],
    ver: &str,
    shaped: &Value,
    names_back: super::translate::NamesBack,
    asked_model: &str,
    client_wants_stream: bool,
    include_usage: bool,
) -> Reply {
    let response = match open_stream_rotating(accounts, ver, shaped) {
        Ok(r) => r,
        Err(UpstreamFailure::Raw(status, body)) => {
            return upstream_error_reply(proto, status, body)
        }
        Err(UpstreamFailure::Exhausted { upstream, body }) => {
            return exhausted_reply(proto, upstream, &body)
        }
    };
    if client_wants_stream {
        // anthropic 出站复用 messages.rs 的 chat→anthropic 翻译器（它要
        // usage 才能给 message_delta 带真数，所以内层恒带 usage）。
        let conv = super::translate::ResponsesToChat::new(
            asked_model,
            include_usage || proto == Proto::Anthropic,
            names_back,
        );
        let chat_reader: Box<dyn Read + Send> =
            Box::new(super::translate::ResponsesToChatReader::new(response, conv));
        let reader: Box<dyn Read + Send> = match proto {
            Proto::Anthropic => Box::new(super::messages::AnthropicSseTranslator::new(
                chat_reader,
                asked_model,
            )),
            _ => chat_reader,
        };
        return Reply::Stream {
            status: 200,
            content_type: "text/event-stream".into(),
            reader,
        };
    }
    let aggregated = match aggregate_sse_value(response) {
        Ok(v) => v,
        Err((status, msg)) => return proto_error(proto, status, &msg),
    };
    let chat = super::translate::responses_object_to_chat(&aggregated, asked_model, &names_back);
    match proto {
        Proto::Anthropic => {
            let bytes = serde_json::to_vec(&chat).unwrap_or_default();
            match super::messages::chat_response_to_anthropic(&bytes, asked_model) {
                Ok(v) => Reply::json(200, &v),
                Err(e) => proto_error(proto, 502, &e),
            }
        }
        _ => Reply::json(200, &chat),
    }
}

/// `codex/<slug>#<账号>` 直写形态：从裸模型名里剥出账号引用。
fn split_direct_pin(bare: &str) -> (String, Option<String>) {
    if let Some(i) = bare.rfind('#') {
        let acct = bare[i + 1..].trim();
        if i > 0 && !acct.is_empty() && !acct.contains('/') {
            return (bare[..i].to_string(), Some(acct.to_string()));
        }
    }
    (bare.to_string(), None)
}

/// 路由层钩子：请求的模型若只能由合成 `codex` 来源接住，就在这里处理；
/// 否则 None（走原有路径）。三种入站协议都真转发：Responses 直通，chat /
/// anthropic 翻成 Responses 出站、应答翻回去。
pub fn intercept(gateway: &super::Gateway, request: &Request, proto: Proto) -> Option<Reply> {
    // 订阅共享执行闸（2026-11-22）：codex/gpt 族没开 → 让位（listed_rows 也
    // 不列，直连请求与未安装无异）。真实 providers 能接住的本来就走原路径。
    if !super::family_enabled(CODEX_ID) {
        return None;
    }
    let parsed: Value = serde_json::from_slice(&request.body).ok()?;
    let asked = parsed.get("model")?.as_str()?.to_string();
    let (groups, providers) = {
        let store = gateway.store.lock().ok()?;
        (store.groups().to_vec(), store.providers().to_vec())
    };
    let known = |id: &str| gateway.catalog.knows(&providers, id);
    // 真实来源（含用户自建的 codex）能接住的，一律走原路径。
    if super::groups::resolve_with(&groups, &providers, &asked, &known).is_some() {
        return None;
    }
    if providers.iter().any(|p| p.id == CODEX_ID) {
        return None;
    }
    let synthetic = synthetic_provider()?;
    let mut with = providers.clone();
    with.push(synthetic);
    let known2 = |id: &str| knows(id) || gateway.catalog.knows(&with, id);
    // 候选表（groups.rs 的成员语法：强度/:fast/#账号都已拆好）；首发不是
    // codex 的（组里排在前面的是别的来源）交还原路径。
    let no_ids = || Vec::new();
    let ctx = super::groups::PlanCtx {
        has_model: &known2,
        catalog_ids: &no_ids,
    };
    let cand = super::groups::plan_with(&groups, &with, &asked, &ctx)
        .into_iter()
        .next()?;
    if cand.provider != CODEX_ID {
        return None;
    }
    let (bare, levels, member_pin, fast) = (cand.model, cand.levels, cand.account, cand.fast);
    let (bare, direct_pin) = split_direct_pin(&bare);
    let pin = member_pin.or(direct_pin);
    // 账号序（Go codexFallBack 的账号轮换半边）：钉了就只有它（找不到 =
    // 明确报错，不落到别的账号）；没钉取「最优打头 + 发现序」的未过期
    // 队列——上一个被额度类错误拒了就换下一个。凭据每个请求现读，
    // 从不写、从不刷新。
    let accounts: Vec<Account> = match pin.as_deref() {
        Some(_) => match select_account(pin.as_deref()) {
            Selected::Ok(a) => {
                if a.expired() {
                    return Some(proto_error(proto, 401, EXPIRED_MESSAGE));
                }
                vec![a]
            }
            Selected::NoAccount => return None,
            Selected::UnknownPin(p) => {
                return Some(proto_error(
                    proto,
                    404,
                    &format!(
                    "no Codex account matches \"{p}\" (use the account's email, id or account id)"
                ),
                ))
            }
        },
        None => {
            let live = ordered_live(discover_all());
            if live.is_empty() {
                // 没有账号（原路径 404）或全过期（最优那份报「登录过期」）。
                return match discover() {
                    None => None,
                    Some(_) => Some(proto_error(proto, 401, EXPIRED_MESSAGE)),
                };
            }
            live
        }
    };
    // 成员强度后缀（`codex/gpt-6-luna:xhigh`）：与其它来源同一套语义，按入站协议的字段改。
    let body = if levels.is_empty() {
        request.body.clone()
    } else {
        super::effort::apply(proto, &request.body, &levels, &levels_of(&bare))
            .unwrap_or_else(|| request.body.clone())
    };
    let inbound: Value = serde_json::from_slice(&body).unwrap_or(parsed);
    let client_wants_stream = inbound
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    let ver = newer_version(&client_version(), request.header("version").unwrap_or(""));
    // 入站 → Responses 请求
    let responses_request = match proto {
        Proto::Responses => inbound.clone(),
        Proto::Chat => match super::translate::chat_to_responses(&inbound) {
            Ok(v) => v,
            Err(e) => return Some(proto_error(proto, 400, &e)),
        },
        Proto::Anthropic => match super::translate::anthropic_to_responses(&inbound) {
            Ok(v) => v,
            Err(e) => return Some(proto_error(proto, 400, &e)),
        },
    };
    let names_back = names_back_of(&responses_request);
    let mut shaped = shape_body(responses_request, &bare, prompt_of(&bare));
    if fast {
        // `:fast` 成员 = Codex 的 Fast 模式（service_tier priority，shape_body 放行）。
        shaped["service_tier"] = Value::String("priority".into());
    }
    if proto == Proto::Responses {
        return Some(forward_responses(
            &accounts,
            &ver,
            &shaped,
            client_wants_stream,
        ));
    }
    let include_usage = inbound
        .pointer("/stream_options/include_usage")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    Some(forward_translated(
        proto,
        &accounts,
        &ver,
        &shaped,
        names_back,
        &asked,
        client_wants_stream,
        include_usage,
    ))
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

    fn b64(v: &Value) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).unwrap())
    }

    /// 假 JWT（只有载荷有意义；全是测试假值）。
    fn fake_jwt(claims: Value) -> String {
        format!("{}.{}.sig", b64(&json!({"alg":"none"})), b64(&claims))
    }

    fn auth_json(access: &str, email: &str) -> Value {
        json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": access,
                "id_token": fake_jwt(json!({
                    "email": email,
                    "https://api.openai.com/auth": { "chatgpt_plan_type": "pro" }
                })),
                "refresh_token": "never-used-refresh",
                "account_id": "acct-test",
            }
        })
    }

    struct Recorded {
        method: String,
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
        /// 序列路由：同前缀第 N 次请求回第 N 条（用尽重复最后一条）——
        /// 账号轮换测试用（第一次 429、第二次 200）。
        seqs: Arc<Mutex<Vec<(String, Vec<(u16, String, String)>)>>>,
    }

    impl Mock {
        fn start() -> Mock {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let port = listener.local_addr().unwrap().port();
            let hits = Arc::new(AtomicUsize::new(0));
            let seen: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
            let routes: Arc<Mutex<Vec<(String, u16, String, String)>>> =
                Arc::new(Mutex::new(Vec::new()));
            let seqs: Arc<Mutex<Vec<(String, Vec<(u16, String, String)>)>>> =
                Arc::new(Mutex::new(Vec::new()));
            let (h, s, r, q) = (hits.clone(), seen.clone(), routes.clone(), seqs.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    h.fetch_add(1, Ordering::SeqCst);
                    let (s, r, q) = (s.clone(), r.clone(), q.clone());
                    std::thread::spawn(move || {
                        let mut reader = BufReader::new(stream.try_clone().unwrap());
                        let mut request_line = String::new();
                        reader.read_line(&mut request_line).ok();
                        let mut parts = request_line.split_whitespace();
                        let method = parts.next().unwrap_or("").to_string();
                        let path = parts.next().unwrap_or("").to_string();
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
                        let len: usize = headers
                            .get("content-length")
                            .and_then(|l| l.parse().ok())
                            .unwrap_or(0);
                        let mut body = vec![0u8; len];
                        reader.read_exact(&mut body).ok();
                        let seq_reply = {
                            let hit = q
                                .lock()
                                .unwrap()
                                .iter()
                                .find(|(prefix, _)| path.starts_with(prefix.as_str()))
                                .map(|(prefix, reps)| {
                                    let nth = s
                                        .lock()
                                        .unwrap()
                                        .iter()
                                        .filter(|r| r.path.starts_with(prefix.as_str()))
                                        .count();
                                    reps[nth.min(reps.len() - 1)].clone()
                                });
                            hit
                        };
                        let reply = match seq_reply {
                            Some(r) => r,
                            None => r
                                .lock()
                                .unwrap()
                                .iter()
                                .find(|(prefix, ..)| path.starts_with(prefix.as_str()))
                                .map(|(_, status, ct, b)| (*status, ct.clone(), b.clone()))
                                .unwrap_or((404, "text/plain".into(), "no route".into())),
                        };
                        s.lock().unwrap().push(Recorded {
                            method,
                            path,
                            headers,
                            body,
                        });
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
            Mock {
                base: format!("http://127.0.0.1:{port}/backend-api/codex"),
                hits,
                seen,
                routes,
                seqs,
            }
        }

        fn route(&self, prefix: &str, status: u16, content_type: &str, body: &str) {
            let mut routes = self.routes.lock().unwrap();
            routes.retain(|(p, ..)| p != prefix);
            routes.push((
                prefix.to_string(),
                status,
                content_type.to_string(),
                body.to_string(),
            ));
        }

        /// 同前缀按次序回（用尽重复最后一条）。
        fn route_seq(&self, prefix: &str, reps: &[(u16, &str, &str)]) {
            let mut seqs = self.seqs.lock().unwrap();
            seqs.retain(|(p, _)| p != prefix);
            let reps: Vec<(u16, String, String)> = reps
                .iter()
                .map(|(s, c, b)| (*s, c.to_string(), b.to_string()))
                .collect();
            seqs.push((prefix.to_string(), reps));
        }

        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }

        fn last(&self, path_prefix: &str) -> (HashMap<String, String>, Vec<u8>) {
            let seen = self.seen.lock().unwrap();
            let r = seen
                .iter()
                .rev()
                .find(|r| r.path.starts_with(path_prefix))
                .expect("request seen");
            let _ = &r.method;
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

    /// 沙箱：串行锁 + 临时配置目录（providers.json）+ 种子缝。logins/auth 由
    /// 各测试自己写。
    fn world(providers_json: &str, base: &str) -> (Gateway, World) {
        let lock = crate::engine_gateway::TEST_SANDBOX_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        reset_for_tests();
        let dir = std::env::temp_dir().join(format!(
            "marionette-engine-codex-{}-{}",
            std::process::id(),
            DIR_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("magpie")).unwrap();
        std::fs::create_dir_all(dir.join("home")).unwrap();
        std::fs::write(dir.join("magpie").join("providers.json"), providers_json).unwrap();
        *TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()) = Some(Seam {
            base: base.to_string(),
            logins_dir: dir.join("magpie"),
            codex_home: dir.join("home"),
        });
        let prior = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let gateway = Gateway::open().expect("gateway");
        match &prior {
            Some(p) => std::env::set_var("XDG_CONFIG_HOME", p),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        (gateway, World { dir, _lock: lock })
    }

    fn write_logins(world: &World, access: &str, email: &str) {
        let logins = json!([{ "id": "x", "agent": "codex", "user": email, "plan": "pro",
                              "auth": auth_json(access, email) }]);
        std::fs::write(
            world.dir.join("magpie").join("logins.json"),
            serde_json::to_vec(&logins).unwrap(),
        )
        .unwrap();
    }

    fn write_codex_auth(world: &World, access: &str, email: &str) {
        std::fs::write(
            world.dir.join("home").join("auth.json"),
            serde_json::to_vec(&auth_json(access, email)).unwrap(),
        )
        .unwrap();
    }

    fn valid_token() -> String {
        fake_jwt(json!({ "exp": now_secs() + 3600 }))
    }

    fn expired_token() -> String {
        fake_jwt(json!({ "exp": now_secs() - 3600 }))
    }

    const MODELS_BODY: &str = r#"{"models":[
        {"slug":"gpt-6-sol","display_name":"GPT-6 Sol","priority":2,"visibility":"list",
         "input_modalities":["text"],"context_window":272000,
         "supported_reasoning_levels":[{"effort":"low"},{"effort":"high"},{"effort":"xhigh"}],
         "base_instructions":"You are Codex SOL. Rules follow."},
        {"slug":"gpt-6-luna","display_name":"GPT-6 Luna","priority":1,"visibility":"list",
         "input_modalities":["text","image"],"context_window":400000,
         "supported_reasoning_levels":[{"effort":"low"},{"effort":"medium"},{"effort":"xhigh"}]},
        {"slug":"secret-hidden","priority":0,"visibility":"hide","context_window":1}
    ]}"#;

    fn post(path: &str, body: &Value) -> Request {
        Request {
            method: "POST".into(),
            path: path.into(),
            headers: Vec::new(),
            body: serde_json::to_vec(body).unwrap(),
        }
    }

    fn get(path: &str) -> Request {
        Request {
            method: "GET".into(),
            path: path.into(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    fn reply_json(reply: Reply) -> (u16, Value) {
        match reply {
            Reply::Json { status, body } => {
                (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
            }
            Reply::Stream {
                status, mut reader, ..
            } => {
                let mut buf = Vec::new();
                reader.read_to_end(&mut buf).unwrap();
                (
                    status,
                    Value::String(String::from_utf8_lossy(&buf).into_owned()),
                )
            }
        }
    }

    const SSE_OK: &str = "event: response.created\n\
data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\"}}\n\n\
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
        // 路由：codex 模型 404（没有来源），且没打任何网络
        let reply = super::super::routes::route(
            &post(
                "/v1/responses",
                &json!({"model":"codex/gpt-6-luna","input":"hi"}),
            ),
            &gateway,
        );
        assert_eq!(reply_json(reply).0, 404);
        assert_eq!(mock.hits(), 0);
        let rows = reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1;
        assert!(rows["data"].as_array().unwrap().is_empty());
        assert_eq!(mock.hits(), 0);
    }

    #[test]
    fn models_rows_cache_and_stale_on_error() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/models",
            200,
            "application/json",
            MODELS_BODY,
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        let token = valid_token();
        write_logins(&w, &token, "me@example.test");

        let body = reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1;
        let data = body["data"].as_array().unwrap();
        let ids: Vec<&str> = data.iter().map(|r| r["id"].as_str().unwrap()).collect();
        // priority 排序、hide 不出
        assert_eq!(ids, vec!["codex/gpt-6-luna", "codex/gpt-6-sol"]);
        assert_eq!(data[0]["context_window"], 400000);
        assert_eq!(data[0]["modalities"]["input"], json!(["text", "image"]));
        assert_eq!(data[1]["modalities"]["input"], json!(["text"]));
        assert_eq!(
            data[0]["reasoning_levels"],
            json!(["low", "medium", "xhigh"])
        );
        // 请求带了 Codex 同款头 + client_version
        let (headers, _) = mock.last("/backend-api/codex/models");
        assert_eq!(headers["authorization"], format!("Bearer {token}"));
        assert_eq!(headers["chatgpt-account-id"], "acct-test");
        assert_eq!(headers["originator"], "codex_cli_rs");
        assert!(headers["user-agent"].starts_with("codex_cli_rs/"));
        assert!(mock.seen.lock().unwrap()[0]
            .path
            .contains("client_version=0.169.1"));

        // 30s 缓存：第二次不再打后端
        let hits = mock.hits();
        let _ = super::super::routes::route(&get("/v1/models"), &gateway);
        assert_eq!(mock.hits(), hits);

        // 缓存过期 + 后端 500 → 回旧行；随后退避内不再打
        expire_cache_for_tests();
        mock.route("/backend-api/codex/models", 500, "text/plain", "boom");
        let body = reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1;
        assert_eq!(body["data"].as_array().unwrap().len(), 2);
        let after_fail = mock.hits();
        let _ = super::super::routes::route(&get("/v1/models"), &gateway);
        assert_eq!(mock.hits(), after_fail);
    }

    #[test]
    fn failing_codex_does_not_hide_other_providers() {
        let mock = Mock::start(); // 没有任何 route → /models 404
        let vendor = Mock::start();
        vendor.route(
            "/v1/models",
            200,
            "application/json",
            r#"{"data":[{"id":"m1","context_window":1000}]}"#,
        );
        let providers = format!(
            r#"{{"providers":[{{"id":"acme","name":"Acme","key":"k","chat":"{}/v1"}}]}}"#,
            vendor.base.trim_end_matches("/backend-api/codex")
        );
        let (gateway, w) = world(&providers, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let body = reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1;
        let ids: Vec<&str> = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["acme/m1"]);
    }

    #[test]
    fn responses_stream_passthrough_headers_and_body() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            SSE_OK,
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        let token = valid_token();
        write_logins(&w, &token, "me@example.test");
        let reply = super::super::routes::route(
            &post(
                "/v1/responses",
                &json!({"model":"codex/gpt-6-luna","stream":true,"input":"hello",
                        "temperature":0.2,"max_output_tokens":99,"prompt_cache_key":"pck-1",
                        "instructions":"client rules"}),
            ),
            &gateway,
        );
        let (status, text) = reply_json(reply);
        assert_eq!(status, 200);
        assert!(text.as_str().unwrap().contains("response.completed"));
        let (headers, body) = mock.last("/backend-api/codex/responses");
        assert_eq!(headers["authorization"], format!("Bearer {token}"));
        assert_eq!(headers["chatgpt-account-id"], "acct-test");
        assert_eq!(headers["openai-beta"], "responses=experimental");
        assert_eq!(headers["session-id"], "pck-1");
        assert_eq!(headers["accept"], "text/event-stream");
        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent["model"], "gpt-6-luna");
        assert_eq!(sent["stream"], true);
        assert_eq!(sent["store"], false);
        assert!(sent.get("temperature").is_none());
        assert!(sent.get("max_output_tokens").is_none());
        assert_eq!(sent["instructions"], "client rules");
        // 字符串 input 被折成 message 项
        assert_eq!(sent["input"][0]["role"], "user");
        assert!(sent["include"]
            .as_array()
            .unwrap()
            .iter()
            .any(|i| i == "reasoning.encrypted_content"));
    }

    #[test]
    fn responses_nonstream_aggregates_sse() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            SSE_OK,
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let reply = super::super::routes::route(
            &post(
                "/v1/responses",
                &json!({"model":"codex/gpt-6-luna","stream":false,"input":"hello"}),
            ),
            &gateway,
        );
        let (status, resp) = reply_json(reply);
        assert_eq!(status, 200);
        assert_eq!(resp["id"], "r1");
        assert_eq!(resp["output"][0]["content"][0]["text"], "hi");
        assert_eq!(resp["usage"]["input_tokens"], 3);
        // 上游被强制成流
        let (_, body) = mock.last("/backend-api/codex/responses");
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["stream"],
            true
        );
    }

    #[test]
    fn nonstream_failed_event_is_an_error() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"nope\"}}}\n\n",
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let (status, body) = reply_json(super::super::routes::route(
            &post(
                "/v1/responses",
                &json!({"model":"codex/gpt-6-luna","input":"x"}),
            ),
            &gateway,
        ));
        assert_eq!(status, 502);
        assert_eq!(body["error"]["message"], "nope");
    }

    #[test]
    fn upstream_error_status_is_passed_through() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            404,
            "application/json",
            r#"{"detail":"Model not found"}"#,
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let (status, body) = reply_json(super::super::routes::route(
            &post(
                "/v1/responses",
                &json!({"model":"codex/nope","stream":true,"input":"x"}),
            ),
            &gateway,
        ));
        assert_eq!(status, 404);
        assert_eq!(body["detail"], "Model not found");
    }

    #[test]
    fn alias_group_member_effort_applies_to_reasoning() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            SSE_OK,
        );
        mock.route(
            "/backend-api/codex/models",
            200,
            "application/json",
            MODELS_BODY,
        );
        let providers = r#"{"providers":[],"groups":[
            {"id":"gpt-6.1-sol","name":"gpt-6.1-sol","members":["codex/gpt-6-luna:xhigh"],"pick":"codex/gpt-6-luna:xhigh"}]}"#;
        let (gateway, w) = world(providers, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let (status, _) = reply_json(super::super::routes::route(
            &post(
                "/v1/responses",
                &json!({"model":"gpt-6.1-sol[low]","stream":true,"input":"x","reasoning":{"effort":"low"}}),
            ),
            &gateway,
        ));
        assert_eq!(status, 200);
        let (_, body) = mock.last("/backend-api/codex/responses");
        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent["model"], "gpt-6-luna");
        assert_eq!(sent["reasoning"]["effort"], "xhigh");
        assert_eq!(sent["reasoning"]["summary"], "auto");
    }

    #[test]
    fn catalog_prompt_becomes_instructions_and_client_goes_developer() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            SSE_OK,
        );
        mock.route(
            "/backend-api/codex/models",
            200,
            "application/json",
            MODELS_BODY,
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        // 先拉模型表（填指令缓存）
        let _ = super::super::routes::route(&get("/v1/models"), &gateway);
        let _ = super::super::routes::route(
            &post(
                "/v1/responses",
                &json!({"model":"codex/gpt-6-sol","stream":true,"input":"x","instructions":"Be a pirate."}),
            ),
            &gateway,
        );
        let (_, body) = mock.last("/backend-api/codex/responses");
        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent["instructions"], "You are Codex SOL. Rules follow.");
        assert_eq!(sent["input"][0]["role"], "developer");
        assert_eq!(sent["input"][0]["content"][0]["text"], "Be a pirate.");
    }

    #[test]
    fn expired_token_is_a_clear_error_and_never_hits_network() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/models",
            200,
            "application/json",
            MODELS_BODY,
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &expired_token(), "me@example.test");
        // 账号存在（合成来源在）→ 路由给出明确原因，不是 404
        assert!(synthetic_provider().is_some());
        let (status, body) = reply_json(super::super::routes::route(
            &post(
                "/v1/responses",
                &json!({"model":"codex/gpt-6-luna","input":"x"}),
            ),
            &gateway,
        ));
        assert_eq!(status, 401);
        assert_eq!(body["error"]["message"], EXPIRED_MESSAGE);
        assert!(listed_rows(&[]).is_empty());
        assert!(quota_row(&[]).is_none());
        assert_eq!(mock.hits(), 0);
    }

    #[test]
    fn later_expiry_source_wins_and_files_are_never_written() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            SSE_OK,
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &expired_token(), "old@example.test");
        let good = valid_token();
        write_codex_auth(&w, &good, "new@example.test");
        let before = std::fs::read(w.dir.join("magpie").join("logins.json")).unwrap();
        let before_auth = std::fs::read(w.dir.join("home").join("auth.json")).unwrap();
        let (status, _) = reply_json(super::super::routes::route(
            &post(
                "/v1/responses",
                &json!({"model":"codex/gpt-6-luna","stream":true,"input":"x"}),
            ),
            &gateway,
        ));
        assert_eq!(status, 200);
        let (headers, _) = mock.last("/backend-api/codex/responses");
        assert_eq!(headers["authorization"], format!("Bearer {good}"));
        assert_eq!(discover().unwrap().email, "new@example.test");
        // 只读：两份文件逐字节未动
        assert_eq!(
            before,
            std::fs::read(w.dir.join("magpie").join("logins.json")).unwrap()
        );
        assert_eq!(
            before_auth,
            std::fs::read(w.dir.join("home").join("auth.json")).unwrap()
        );
    }

    #[test]
    fn apikey_mode_and_other_agents_are_ignored() {
        let mock = Mock::start();
        let (_gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        let mut auth = auth_json(&valid_token(), "k@example.test");
        auth["auth_mode"] = json!("apikey");
        let logins = json!([
            { "agent": "codex", "user": "k@example.test", "auth": auth },
            { "agent": "claude", "user": "c@example.test", "auth": auth_json(&valid_token(), "c@example.test") },
            { "agent": "codex", "user": "n", "auth": null }
        ]);
        std::fs::write(
            w.dir.join("magpie").join("logins.json"),
            serde_json::to_vec(&logins).unwrap(),
        )
        .unwrap();
        assert!(discover().is_none());
    }

    #[test]
    fn user_defined_codex_provider_takes_precedence() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/models",
            200,
            "application/json",
            MODELS_BODY,
        );
        let vendor = Mock::start();
        vendor.route(
            "/v1/models",
            200,
            "application/json",
            r#"{"data":[{"id":"real","context_window":5}]}"#,
        );
        vendor.route(
            "/v1/responses",
            200,
            "application/json",
            r#"{"id":"from-real"}"#,
        );
        let root = vendor.base.trim_end_matches("/backend-api/codex");
        let providers = format!(
            r#"{{"providers":[{{"id":"codex","name":"Mine","key":"k","chat":"{root}/v1","responses":"{root}/v1"}}]}}"#
        );
        let (gateway, w) = world(&providers, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let body = reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1;
        let ids: Vec<&str> = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["codex/real"]);
        let (status, resp) = reply_json(super::super::routes::route(
            &post("/v1/responses", &json!({"model":"codex/real","input":"x"})),
            &gateway,
        ));
        assert_eq!(status, 200);
        assert_eq!(resp["id"], "from-real");
        assert_eq!(mock.hits(), 0);
        assert!(quota_row(&[Provider {
            id: "codex".into(),
            ..Default::default()
        }])
        .is_none());
    }

    #[test]
    fn quota_row_comes_from_wham_usage() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/wham/usage",
            200,
            "application/json",
            r#"{"plan_type":"pro",
                "credits":{"has_credits":true,"unlimited":false,"balance":"12.345"},
                "rate_limit":{"allowed":true,
                  "primary_window":{"used_percent":37.5,"limit_window_seconds":18000,"reset_after_seconds":600,"reset_at":1893456000},
                  "secondary_window":{"used_percent":10,"limit_window_seconds":604800,"reset_after_seconds":86400,"reset_at":1893542400}}}"#,
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        let token = valid_token();
        write_logins(&w, &token, "me@example.test");
        let rows = super::super::quotas::quota_entries(&gateway, true);
        let row = rows
            .iter()
            .find(|r| r["provider"] == "codex")
            .expect("codex row");
        assert_eq!(row["name"], "Codex");
        assert_eq!(row["user"], "me@example.test");
        assert_eq!(row["plan"], "pro");
        assert_eq!(row["balance"], "12.35 credits");
        let windows = row["windows"].as_array().unwrap();
        assert_eq!(windows[0]["name"], "5 hours");
        assert_eq!(windows[0]["used"], 37.5);
        assert_eq!(windows[0]["remaining"], 62.5);
        assert_eq!(windows[0]["resetsAt"], "2030-01-01T00:00:00Z");
        assert_eq!(windows[1]["name"], "7 days");
        let (headers, _) = mock.last("/backend-api/wham/usage");
        assert_eq!(headers["authorization"], format!("Bearer {token}"));
        assert_eq!(headers["chatgpt-account-id"], "acct-test");
        // 失败安静跳过
        mock.route("/backend-api/wham/usage", 500, "text/plain", "x");
        let rows = super::super::quotas::quota_entries(&gateway, true);
        assert!(rows.iter().all(|r| r["provider"] != "codex"));
    }

    #[test]
    fn jwt_and_version_helpers() {
        let t = fake_jwt(json!({"exp": 123, "email": "a@b"}));
        assert_eq!(jwt_claims(&t).unwrap()["exp"], 123);
        assert!(jwt_claims("not-a-jwt").is_none());
        assert_eq!(newer_version("0.159.0", "0.160.1"), "0.160.1");
        assert_eq!(newer_version("0.159.0", "0.158.9"), "0.159.0");
        assert_eq!(newer_version("0.159.0", "garbage"), "0.159.0");
        assert_eq!(newer_version("0.159.0", ""), "0.159.0");
        // 内置下限必须够新：后端只把 GPT-6.2 Sol / Astra 列给 0.166.0 起的客户端，
        // 本机没有任何 Codex 缓存时也得看得到（Go codexClientVersion 2026-11-18）。
        assert_eq!(newer_version(CLIENT_VERSION, "0.166.0"), CLIENT_VERSION);
        assert_eq!(
            client_version(),
            CLIENT_VERSION,
            "no models_cache.json in the test home → the built-in floor"
        );
    }

    #[test]
    fn shape_body_keeps_codex_contract() {
        let long_id = "x".repeat(80);
        let v = shape_body(
            json!({
                "model": "codex/m", "service_tier": "flex", "tool_choice": "required",
                "input": [
                    {"type":"item_reference","id":"r"},
                    {"type":"message","id":"m1","role":"system","status":"completed","content":"s"},
                    {"type":"function_call","id":"fc","call_id": long_id, "name":"f","arguments":"{}"},
                    {"type":"message","role":"user","content":"hi"}
                ],
                "reasoning": {"effort":"none"}, "include": ["x"]
            }),
            "m",
            None,
        );
        assert!(v.get("service_tier").is_none());
        assert_eq!(v["tool_choice"], "auto");
        assert_eq!(v["store"], false);
        assert_eq!(v["instructions"], FALLBACK_INSTRUCTIONS);
        let input = v["input"].as_array().unwrap();
        assert_eq!(input.len(), 3); // item_reference 被丢
        assert_eq!(input[0]["role"], "developer");
        assert!(input[0].get("id").is_none() && input[0].get("status").is_none());
        assert_eq!(input[1]["call_id"].as_str().unwrap().len(), 64);
        // effort none：不要 include
        assert!(v.get("include").is_none());
        assert!(v["prompt_cache_key"].as_str().unwrap().len() == 36);
    }

    // ---------------------------------------------------------------
    // chat / anthropic 入站（responses 请求翻译 + 应答翻回）
    // ---------------------------------------------------------------

    fn sse(events: &[Value]) -> String {
        events
            .iter()
            .map(|e| format!("event: {}\ndata: {}\n\n", e["type"].as_str().unwrap(), e))
            .collect()
    }

    fn data_frames(text: &str) -> Vec<String> {
        text.split("\n\n")
            .filter_map(|b| {
                b.lines()
                    .find_map(|l| l.strip_prefix("data: "))
                    .map(String::from)
            })
            .collect()
    }

    fn chat_chunks(text: &str) -> Vec<Value> {
        data_frames(text)
            .iter()
            .filter_map(|d| serde_json::from_str(d).ok())
            .collect()
    }

    fn anthropic_frames(text: &str) -> Vec<(String, Value)> {
        text.split("\n\n")
            .filter_map(|b| {
                let ev = b
                    .lines()
                    .find_map(|l| l.strip_prefix("event: "))?
                    .to_string();
                let data = b.lines().find_map(|l| l.strip_prefix("data: "))?;
                Some((ev, serde_json::from_str(data).ok()?))
            })
            .collect()
    }

    fn text_events() -> Vec<Value> {
        vec![
            json!({"type":"response.created","response":{"id":"r1"}}),
            json!({"type":"response.output_text.delta","output_index":0,"delta":"Hel"}),
            json!({"type":"response.output_text.delta","output_index":0,"delta":"lo"}),
            json!({"type":"response.output_item.done","output_index":0,
                   "item":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Hello"}]}}),
            json!({"type":"response.completed","response":{"id":"r1","status":"completed","output":[],
                   "usage":{"input_tokens":10,"output_tokens":5,"total_tokens":15,
                            "input_tokens_details":{"cached_tokens":4},
                            "output_tokens_details":{"reasoning_tokens":2}}}}),
        ]
    }

    fn tool_events(name: &str) -> Vec<Value> {
        vec![
            json!({"type":"response.created","response":{"id":"r2"}}),
            json!({"type":"response.output_item.added","output_index":0,
                   "item":{"type":"function_call","id":"fc_1","call_id":"call_9","name":name,"arguments":""}}),
            json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":"fc_1","delta":"{\"a\":"}),
            json!({"type":"response.function_call_arguments.delta","output_index":0,"item_id":"fc_1","delta":"1}"}),
            json!({"type":"response.output_item.done","output_index":0,
                   "item":{"type":"function_call","id":"fc_1","call_id":"call_9","name":name,"arguments":"{\"a\":1}"}}),
            json!({"type":"response.completed","response":{"id":"r2","status":"completed","output":[],
                   "usage":{"input_tokens":7,"output_tokens":3}}}),
        ]
    }

    fn sent_body(mock: &Mock) -> Value {
        serde_json::from_slice(&mock.last("/backend-api/codex/responses").1).unwrap()
    }

    #[test]
    fn chat_stream_translates_responses_sse() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            &sse(&text_events()),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let (status, text) = reply_json(super::super::routes::route(
            &post(
                "/v1/chat/completions",
                &json!({"model":"codex/gpt-6-luna","stream":true,"stream_options":{"include_usage":true},
                        "reasoning_effort":"high",
                        "messages":[{"role":"system","content":"sys rules"},{"role":"user","content":"hi"}]}),
            ),
            &gateway,
        ));
        assert_eq!(status, 200);
        let text = text.as_str().unwrap().to_string();
        let chunks = chat_chunks(&text);
        assert_eq!(chunks[0]["choices"][0]["delta"]["role"], "assistant");
        let content: String = chunks
            .iter()
            .filter_map(|c| {
                c.pointer("/choices/0/delta/content")
                    .and_then(|v| v.as_str())
            })
            .collect();
        assert_eq!(content, "Hello");
        let finish = chunks
            .iter()
            .find(|c| c.pointer("/choices/0/finish_reason") == Some(&json!("stop")));
        assert!(finish.is_some(), "{text}");
        let usage = chunks
            .iter()
            .find_map(|c| c.get("usage").filter(|u| u.is_object()))
            .expect("usage chunk");
        assert_eq!(usage["prompt_tokens"], 10);
        assert_eq!(usage["completion_tokens"], 5);
        assert_eq!(usage["prompt_tokens_details"]["cached_tokens"], 4);
        assert_eq!(usage["completion_tokens_details"]["reasoning_tokens"], 2);
        assert!(text.trim_end().ends_with("data: [DONE]"));
        // 上游请求：恒流、effort 带过去、system 进 instructions、user 进 input
        let sent = sent_body(&mock);
        assert_eq!(sent["stream"], true);
        assert_eq!(sent["model"], "gpt-6-luna");
        assert_eq!(sent["reasoning"]["effort"], "high");
        assert_eq!(sent["instructions"], "sys rules");
        assert_eq!(sent["input"][0]["role"], "user");
        assert_eq!(sent["input"][0]["content"][0]["text"], "hi");
        let (headers, _) = mock.last("/backend-api/codex/responses");
        assert_eq!(headers["accept"], "text/event-stream");
    }

    #[test]
    fn chat_stream_without_include_usage_has_no_usage_chunk() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            &sse(&text_events()),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let (_, text) = reply_json(super::super::routes::route(
            &post(
                "/v1/chat/completions",
                &json!({"model":"codex/gpt-6-luna","stream":true,"messages":[{"role":"user","content":"hi"}]}),
            ),
            &gateway,
        ));
        let chunks = chat_chunks(text.as_str().unwrap());
        assert!(chunks.iter().all(|c| c.get("usage").is_none()));
    }

    #[test]
    fn chat_nonstream_aggregates_to_completion_json() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            &sse(&text_events()),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let (status, v) = reply_json(super::super::routes::route(
            &post(
                "/v1/chat/completions",
                &json!({"model":"codex/gpt-6-luna","messages":[{"role":"user","content":"hi"}]}),
            ),
            &gateway,
        ));
        assert_eq!(status, 200);
        assert_eq!(v["object"], "chat.completion");
        assert_eq!(v["model"], "codex/gpt-6-luna");
        assert_eq!(v["choices"][0]["message"]["content"], "Hello");
        assert_eq!(v["choices"][0]["finish_reason"], "stop");
        assert_eq!(v["usage"]["prompt_tokens"], 10);
        assert_eq!(v["usage"]["total_tokens"], 15);
        assert_eq!(v["usage"]["prompt_tokens_details"]["cached_tokens"], 4);
        // 上游恒流
        assert_eq!(sent_body(&mock)["stream"], true);
    }

    fn tool_chat_request(stream: bool) -> Value {
        json!({
            "model": "codex/gpt-6-luna", "stream": stream,
            "tools": [{"type":"function","function":{"name":"my tool","description":"d",
                       "parameters":{"type":"object","properties":{"a":{"type":"integer"}}}}}],
            "messages": [
                {"role":"user","content":"go"},
                {"role":"assistant","content":null,
                 "tool_calls":[{"id":"call_1","type":"function","function":{"name":"my tool","arguments":"{}"}}]},
                {"role":"tool","tool_call_id":"call_1","content":"ok"},
                {"role":"tool","tool_call_id":"zzz","content":"stray"}
            ]
        })
    }

    #[test]
    fn chat_tool_turn_stream_renames_and_restores_tool_names() {
        let mock = Mock::start();
        let wire = codex_tool_name("my tool");
        assert_ne!(wire, "my tool");
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            &sse(&tool_events(&wire)),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let (status, text) = reply_json(super::super::routes::route(
            &post("/v1/chat/completions", &tool_chat_request(true)),
            &gateway,
        ));
        assert_eq!(status, 200);
        let text = text.as_str().unwrap().to_string();
        let chunks = chat_chunks(&text);
        let calls: Vec<&Value> = chunks
            .iter()
            .filter_map(|c| c.pointer("/choices/0/delta/tool_calls/0"))
            .collect();
        assert_eq!(calls[0]["id"], "call_9");
        assert_eq!(calls[0]["function"]["name"], "my tool"); // 回程还原
        let args: String = calls
            .iter()
            .filter_map(|c| c.pointer("/function/arguments").and_then(|a| a.as_str()))
            .collect();
        assert_eq!(args, "{\"a\":1}");
        assert!(chunks
            .iter()
            .any(|c| c.pointer("/choices/0/finish_reason") == Some(&json!("tool_calls"))));
        // 上游：工具名与历史调用名都改成后端收的；孤儿输出改成 assistant 备注
        let sent = sent_body(&mock);
        assert_eq!(sent["tools"][0]["name"], wire.as_str());
        let input = sent["input"].as_array().unwrap();
        let fc = input.iter().find(|i| i["type"] == "function_call").unwrap();
        assert_eq!(fc["name"], wire.as_str());
        assert_eq!(fc["call_id"], "call_1");
        assert!(input
            .iter()
            .any(|i| i["type"] == "function_call_output" && i["call_id"] == "call_1"));
        let orphan = input
            .iter()
            .find(|i| {
                i["role"] == "assistant"
                    && i["content"]
                        .as_str()
                        .is_some_and(|c| c.contains("call_id=zzz"))
            })
            .expect("orphan became a note");
        assert!(orphan["content"].as_str().unwrap().ends_with("stray"));
        assert!(!input
            .iter()
            .any(|i| i["type"] == "function_call_output" && i["call_id"] == "zzz"));
    }

    #[test]
    fn chat_tool_turn_nonstream_returns_tool_calls_json() {
        let mock = Mock::start();
        let wire = codex_tool_name("my tool");
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            &sse(&tool_events(&wire)),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let (status, v) = reply_json(super::super::routes::route(
            &post("/v1/chat/completions", &tool_chat_request(false)),
            &gateway,
        ));
        assert_eq!(status, 200);
        let msg = &v["choices"][0]["message"];
        assert!(msg["content"].is_null());
        assert_eq!(msg["tool_calls"][0]["id"], "call_9");
        assert_eq!(msg["tool_calls"][0]["function"]["name"], "my tool");
        assert_eq!(msg["tool_calls"][0]["function"]["arguments"], "{\"a\":1}");
        assert_eq!(v["choices"][0]["finish_reason"], "tool_calls");
    }

    #[test]
    fn anthropic_stream_text_and_usage() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            &sse(&text_events()),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let (status, text) = reply_json(super::super::routes::route(
            &post(
                "/v1/messages",
                &json!({"model":"codex/gpt-6-luna","max_tokens":100,"stream":true,"system":"sys rules",
                        "thinking":{"type":"enabled","budget_tokens":24000},
                        "messages":[{"role":"user","content":"hi"}]}),
            ),
            &gateway,
        ));
        assert_eq!(status, 200);
        let frames = anthropic_frames(text.as_str().unwrap());
        assert_eq!(frames[0].0, "message_start");
        let delta_text: String = frames
            .iter()
            .filter(|(e, _)| e == "content_block_delta")
            .filter_map(|(_, v)| v.pointer("/delta/text").and_then(|t| t.as_str()))
            .collect();
        assert_eq!(delta_text, "Hello");
        let md = frames.iter().find(|(e, _)| e == "message_delta").unwrap();
        assert_eq!(md.1["delta"]["stop_reason"], "end_turn");
        // 净额口径（input 10 − cached 4；Go chat.go Usage 同款）
        assert_eq!(md.1["usage"]["input_tokens"], 6);
        assert_eq!(md.1["usage"]["cache_read_input_tokens"], 4);
        assert_eq!(md.1["usage"]["output_tokens"], 5);
        assert_eq!(frames.last().unwrap().0, "message_stop");
        let sent = sent_body(&mock);
        assert_eq!(sent["instructions"], "sys rules");
        assert_eq!(sent["reasoning"]["effort"], "high"); // 24000 预算 → high
    }

    #[test]
    fn anthropic_tool_use_stream_and_nonstream() {
        let mock = Mock::start();
        let wire = codex_tool_name("my tool");
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            &sse(&tool_events(&wire)),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let req = |stream: bool| {
            json!({"model":"codex/gpt-6-luna","max_tokens":100,"stream":stream,
                   "tools":[{"name":"my tool","description":"d","input_schema":{"type":"object"}}],
                   "messages":[{"role":"user","content":"go"}]})
        };
        let (_, text) = reply_json(super::super::routes::route(
            &post("/v1/messages", &req(true)),
            &gateway,
        ));
        let frames = anthropic_frames(text.as_str().unwrap());
        let start = frames
            .iter()
            .find(|(e, v)| {
                e == "content_block_start"
                    && v.pointer("/content_block/type") == Some(&json!("tool_use"))
            })
            .expect("tool_use block");
        assert_eq!(start.1["content_block"]["name"], "my tool");
        assert_eq!(start.1["content_block"]["id"], "call_9");
        let partial: String = frames
            .iter()
            .filter_map(|(_, v)| v.pointer("/delta/partial_json").and_then(|p| p.as_str()))
            .collect();
        assert_eq!(partial, "{\"a\":1}");
        let md = frames.iter().find(|(e, _)| e == "message_delta").unwrap();
        assert_eq!(md.1["delta"]["stop_reason"], "tool_use");

        let (status, v) = reply_json(super::super::routes::route(
            &post("/v1/messages", &req(false)),
            &gateway,
        ));
        assert_eq!(status, 200);
        assert_eq!(v["type"], "message");
        assert_eq!(v["stop_reason"], "tool_use");
        assert_eq!(v["content"][0]["type"], "tool_use");
        assert_eq!(v["content"][0]["name"], "my tool");
        assert_eq!(v["content"][0]["input"]["a"], 1);
        assert_eq!(v["usage"]["input_tokens"], 7);
    }

    #[test]
    fn anthropic_nonstream_text() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            &sse(&text_events()),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let (status, v) = reply_json(super::super::routes::route(
            &post(
                "/v1/messages",
                &json!({"model":"codex/gpt-6-luna","max_tokens":50,"messages":[{"role":"user","content":"hi"}]}),
            ),
            &gateway,
        ));
        assert_eq!(status, 200);
        assert_eq!(v["content"][0]["text"], "Hello");
        assert_eq!(v["stop_reason"], "end_turn");
        assert_eq!(v["model"], "codex/gpt-6-luna");
        assert_eq!(v["usage"]["cache_read_input_tokens"], 4);
    }

    #[test]
    fn length_finish_reason_maps_from_incomplete() {
        let mock = Mock::start();
        let events = vec![
            json!({"type":"response.output_text.delta","output_index":0,"delta":"cut"}),
            json!({"type":"response.incomplete","response":{"status":"incomplete",
                   "incomplete_details":{"reason":"max_output_tokens"},"output":[]}}),
        ];
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            &sse(&events),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let (_, v) = reply_json(super::super::routes::route(
            &post(
                "/v1/chat/completions",
                &json!({"model":"codex/gpt-6-luna","messages":[{"role":"user","content":"hi"}]}),
            ),
            &gateway,
        ));
        assert_eq!(v["choices"][0]["finish_reason"], "length");
    }

    #[test]
    fn stream_failure_midway_surfaces_error_on_each_protocol() {
        let mock = Mock::start();
        let events = vec![
            json!({"type":"response.output_text.delta","output_index":0,"delta":"par"}),
            json!({"type":"response.failed","response":{"error":{"message":"boom","code":"server_error"}}}),
        ];
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            &sse(&events),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let (_, text) = reply_json(super::super::routes::route(
            &post(
                "/v1/chat/completions",
                &json!({"model":"codex/gpt-6-luna","stream":true,"messages":[{"role":"user","content":"hi"}]}),
            ),
            &gateway,
        ));
        let chunks = chat_chunks(text.as_str().unwrap());
        assert!(chunks.iter().any(|c| c["error"]["message"] == "boom"));
        assert!(text.as_str().unwrap().trim_end().ends_with("[DONE]"));
        let (_, text) = reply_json(super::super::routes::route(
            &post(
                "/v1/messages",
                &json!({"model":"codex/gpt-6-luna","max_tokens":9,"stream":true,
                        "messages":[{"role":"user","content":"hi"}]}),
            ),
            &gateway,
        ));
        let frames = anthropic_frames(text.as_str().unwrap());
        let err = frames
            .iter()
            .find(|(e, _)| e == "error")
            .expect("anthropic error event");
        assert_eq!(err.1["error"]["message"], "boom");
        // 非流式：502 + 各自错误形状
        let (status, body) = reply_json(super::super::routes::route(
            &post(
                "/v1/chat/completions",
                &json!({"model":"codex/gpt-6-luna","messages":[{"role":"user","content":"x"}]}),
            ),
            &gateway,
        ));
        assert_eq!(
            (status, body["error"]["message"].as_str()),
            (502, Some("boom"))
        );
        let (status, body) = reply_json(super::super::routes::route(
            &post(
                "/v1/messages",
                &json!({"model":"codex/gpt-6-luna","max_tokens":9,"messages":[{"role":"user","content":"x"}]}),
            ),
            &gateway,
        ));
        assert_eq!((status, body["type"].as_str()), (502, Some("error")));
    }

    #[test]
    fn truncated_stream_is_an_error_not_a_clean_finish() {
        let mock = Mock::start();
        let events =
            vec![json!({"type":"response.output_text.delta","output_index":0,"delta":"half"})];
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            &sse(&events),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let (_, text) = reply_json(super::super::routes::route(
            &post(
                "/v1/chat/completions",
                &json!({"model":"codex/gpt-6-luna","stream":true,"messages":[{"role":"user","content":"hi"}]}),
            ),
            &gateway,
        ));
        let chunks = chat_chunks(text.as_str().unwrap());
        assert!(chunks
            .iter()
            .all(|c| c.pointer("/choices/0/finish_reason") != Some(&json!("stop"))));
        assert!(chunks.iter().any(|c| c.get("error").is_some()));
    }

    #[test]
    fn upstream_errors_take_each_protocols_shape() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            401,
            "application/json",
            r#"{"detail":"Token expired"}"#,
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let (status, body) = reply_json(super::super::routes::route(
            &post(
                "/v1/chat/completions",
                &json!({"model":"codex/gpt-6-luna","messages":[{"role":"user","content":"x"}]}),
            ),
            &gateway,
        ));
        assert_eq!(status, 401);
        assert_eq!(body["error"]["message"], "Token expired");
        let (status, body) = reply_json(super::super::routes::route(
            &post(
                "/v1/messages",
                &json!({"model":"codex/gpt-6-luna","max_tokens":9,"messages":[{"role":"user","content":"x"}]}),
            ),
            &gateway,
        ));
        assert_eq!(status, 401);
        assert_eq!(body["type"], "error");
        assert_eq!(body["error"]["type"], "authentication_error");
        assert_eq!(body["error"]["message"], "Token expired");
    }

    #[test]
    fn expired_token_is_a_clear_401_on_every_inbound_path() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            &sse(&text_events()),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &expired_token(), "me@example.test");
        for (path, body) in [
            (
                "/v1/responses",
                json!({"model":"codex/gpt-6-luna","input":"x"}),
            ),
            (
                "/v1/chat/completions",
                json!({"model":"codex/gpt-6-luna","messages":[{"role":"user","content":"x"}]}),
            ),
            (
                "/v1/messages",
                json!({"model":"codex/gpt-6-luna","max_tokens":9,"messages":[{"role":"user","content":"x"}]}),
            ),
        ] {
            let (status, v) = reply_json(super::super::routes::route(&post(path, &body), &gateway));
            assert_eq!(status, 401, "{path}");
            let msg = v
                .pointer("/error/message")
                .and_then(|m| m.as_str())
                .unwrap_or("");
            assert_eq!(msg, EXPIRED_MESSAGE, "{path}");
        }
        assert_eq!(mock.hits(), 0);
        // Codex CLI 刷新了 auth.json（我们没动任何东西）：同一个网关下一个请求就好了
        let fresh = valid_token();
        write_codex_auth(&w, &fresh, "me@example.test");
        let (status, _) = reply_json(super::super::routes::route(
            &post(
                "/v1/chat/completions",
                &json!({"model":"codex/gpt-6-luna","messages":[{"role":"user","content":"x"}]}),
            ),
            &gateway,
        ));
        assert_eq!(status, 200);
        let (headers, _) = mock.last("/backend-api/codex/responses");
        assert_eq!(headers["authorization"], format!("Bearer {fresh}"));
    }

    #[test]
    fn credentials_are_reread_on_every_request() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            &sse(&text_events()),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        let one = fake_jwt(json!({ "exp": now_secs() + 3600, "n": 1 }));
        let two = fake_jwt(json!({ "exp": now_secs() + 7200, "n": 2 }));
        write_codex_auth(&w, &one, "me@example.test");
        let chat = json!({"model":"codex/gpt-6-luna","messages":[{"role":"user","content":"x"}]});
        let _ = super::super::routes::route(&post("/v1/chat/completions", &chat), &gateway);
        assert_eq!(
            mock.last("/backend-api/codex/responses").0["authorization"],
            format!("Bearer {one}")
        );
        write_codex_auth(&w, &two, "me@example.test");
        let _ = super::super::routes::route(&post("/v1/chat/completions", &chat), &gateway);
        assert_eq!(
            mock.last("/backend-api/codex/responses").0["authorization"],
            format!("Bearer {two}")
        );
        // 从不写凭据文件
        assert!(!w.dir.join("magpie").join("logins.json").exists());
    }

    fn write_logins_multi(world: &World, rows: &[(&str, &str, &str, &str)]) {
        let list: Vec<Value> = rows
            .iter()
            .map(|(id, email, token, account_id)| {
                let mut auth = auth_json(token, email);
                auth["tokens"]["account_id"] = json!(account_id);
                json!({ "id": id, "agent": "codex", "user": email, "plan": "pro", "auth": auth })
            })
            .collect();
        std::fs::write(
            world.dir.join("magpie").join("logins.json"),
            serde_json::to_vec(&list).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn multi_account_pin_and_default_pick() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            &sse(&text_events()),
        );
        let providers = r#"{"providers":[],"groups":[
            {"id":"pinned","name":"pinned","members":["codex/gpt-6-luna:xhigh#acc-a"],"pick":"codex/gpt-6-luna:xhigh#acc-a"}]}"#;
        let (gateway, w) = world(providers, &mock.base);
        let a = fake_jwt(json!({ "exp": now_secs() + 1000, "who": "a" }));
        let b = fake_jwt(json!({ "exp": now_secs() + 9000, "who": "b" }));
        write_logins_multi(
            &w,
            &[
                ("acc-a", "a@example.test", &a, "acct-a"),
                ("acc-b", "b@example.test", &b, "acct-b"),
            ],
        );
        let chat =
            |model: &str| json!({"model": model, "messages":[{"role":"user","content":"x"}]});
        let seen = |mock: &Mock| mock.last("/backend-api/codex/responses").0;
        // 默认：到期最晚的 b
        let _ = super::super::routes::route(
            &post("/v1/chat/completions", &chat("codex/gpt-6-luna")),
            &gateway,
        );
        let h = seen(&mock);
        assert_eq!(h["authorization"], format!("Bearer {b}"));
        assert_eq!(h["chatgpt-account-id"], "acct-b");
        // 直写 #邮箱 钉 a
        let _ = super::super::routes::route(
            &post(
                "/v1/chat/completions",
                &chat("codex/gpt-6-luna#a@example.test"),
            ),
            &gateway,
        );
        let h = seen(&mock);
        assert_eq!(h["authorization"], format!("Bearer {a}"));
        assert_eq!(h["chatgpt-account-id"], "acct-a");
        assert_eq!(sent_body(&mock)["model"], "gpt-6-luna");
        // 组成员 #条目id 钉 a，且强度后缀仍生效
        let _ = super::super::routes::route(
            &post(
                "/v1/responses",
                &json!({"model":"pinned","input":"x","stream":true,"reasoning":{"effort":"low"}}),
            ),
            &gateway,
        );
        assert_eq!(seen(&mock)["chatgpt-account-id"], "acct-a");
        assert_eq!(sent_body(&mock)["reasoning"]["effort"], "xhigh");
        // 钉了不存在的账号：明确报错，不落到别的账号
        let hits = mock.hits();
        let (status, body) = reply_json(super::super::routes::route(
            &post("/v1/chat/completions", &chat("codex/gpt-6-luna#nobody")),
            &gateway,
        ));
        assert_eq!(status, 404);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("nobody"));
        assert_eq!(mock.hits(), hits);
        let (status, body) = reply_json(super::super::routes::route(
            &post(
                "/v1/messages",
                &json!({"model":"codex/gpt-6-luna#nobody","max_tokens":9,
                                          "messages":[{"role":"user","content":"x"}]}),
            ),
            &gateway,
        ));
        assert_eq!(
            (status, body["error"]["type"].as_str()),
            (404, Some("not_found_error"))
        );
    }

    #[test]
    fn multi_account_expired_pin_is_401_and_default_skips_expired() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            &sse(&text_events()),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        let old = expired_token();
        let live = fake_jwt(json!({ "exp": now_secs() + 100, "who": "live" }));
        write_logins_multi(
            &w,
            &[
                ("acc-old", "old@example.test", &old, "acct-old"),
                ("acc-live", "live@example.test", &live, "acct-live"),
            ],
        );
        let chat =
            |model: &str| json!({"model": model, "messages":[{"role":"user","content":"x"}]});
        let (status, _) = reply_json(super::super::routes::route(
            &post("/v1/chat/completions", &chat("codex/gpt-6-luna#acc-old")),
            &gateway,
        ));
        assert_eq!(status, 401);
        assert_eq!(mock.hits(), 0);
        let (status, _) = reply_json(super::super::routes::route(
            &post("/v1/chat/completions", &chat("codex/gpt-6-luna")),
            &gateway,
        ));
        assert_eq!(status, 200);
        assert_eq!(
            mock.last("/backend-api/codex/responses").0["chatgpt-account-id"],
            "acct-live"
        );
    }

    #[test]
    fn same_account_in_logins_and_auth_json_is_one_account() {
        let mock = Mock::start();
        let (_gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        let older = fake_jwt(json!({ "exp": now_secs() + 100, "v": "old" }));
        let newer = fake_jwt(json!({ "exp": now_secs() + 5000, "v": "new" }));
        write_logins_multi(&w, &[("acc-x", "x@example.test", &older, "acct-x")]);
        let mut auth = auth_json(&newer, "x@example.test");
        auth["tokens"]["account_id"] = json!("acct-x");
        std::fs::write(
            w.dir.join("home").join("auth.json"),
            serde_json::to_vec(&auth).unwrap(),
        )
        .unwrap();
        let all = discover_all();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].token, newer); // 取更新的令牌
        assert_eq!(all[0].id, "acc-x"); // 但保留 logins 条目的 id（钉死用）
    }

    #[test]
    fn quota_rows_one_per_account() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/wham/usage",
            200,
            "application/json",
            r#"{"plan_type":"pro","rate_limit":{"primary_window":{"used_percent":10,"limit_window_seconds":18000}}}"#,
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        let a = valid_token();
        let b = fake_jwt(json!({ "exp": now_secs() + 4000 }));
        write_logins_multi(
            &w,
            &[
                ("acc-a", "a@example.test", &a, "acct-a"),
                ("acc-b", "b@example.test", &b, "acct-b"),
            ],
        );
        let rows = super::super::quotas::quota_entries(&gateway, true);
        let mut users: Vec<&str> = rows
            .iter()
            .filter(|r| r["provider"] == "codex")
            .map(|r| r["user"].as_str().unwrap())
            .collect();
        users.sort();
        assert_eq!(users, vec!["a@example.test", "b@example.test"]);
        // 每个账号用自己的令牌问
        let seen = mock.seen.lock().unwrap();
        let tokens: Vec<String> = seen
            .iter()
            .filter(|r| r.path.starts_with("/backend-api/wham/usage"))
            .map(|r| r.headers["authorization"].clone())
            .collect();
        assert!(tokens.contains(&format!("Bearer {a}")) && tokens.contains(&format!("Bearer {b}")));
    }

    #[test]
    fn fast_member_sets_priority_tier() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            &sse(&text_events()),
        );
        let providers = r#"{"providers":[],"groups":[
            {"id":"quick","name":"quick","members":["codex/gpt-6-luna:fast"],"pick":"codex/gpt-6-luna:fast"}]}"#;
        let (gateway, w) = world(providers, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let _ = super::super::routes::route(
            &post(
                "/v1/responses",
                &json!({"model":"quick","input":"x","stream":true}),
            ),
            &gateway,
        );
        assert_eq!(sent_body(&mock)["service_tier"], "priority");
        assert_eq!(sent_body(&mock)["model"], "gpt-6-luna");
    }

    // ---- 请求整形（Go codexInput / codexToolNames / declareSearch 的移植） ----

    #[test]
    fn tool_name_sanitising_matches_go() {
        assert_eq!(codex_tool_name("fine_name-1.x"), "fine_name-1.x");
        let n = codex_tool_name("get weather: 天气");
        assert!(n.starts_with("get_weather"));
        assert!(n
            .bytes()
            .all(|c| c == b'_' || c == b'.' || c == b'-' || c.is_ascii_alphanumeric()));
        assert_eq!(n, codex_tool_name("get weather: 天气")); // 恒定
        assert_ne!(codex_tool_name("a b"), codex_tool_name("a:b")); // 不同名不撞
                                                                    // 超长：55 + _ + 8 hex
        let long = format!("{} x", "n".repeat(100));
        assert_eq!(codex_tool_name(&long).len(), 55 + 1 + 8);
        // 空名也要合法
        assert!(tool_name_ok(&codex_tool_name("")));
        // 嵌套 namespace / additional_tools / tool_choice 都改
        let mut v = json!({
            "tools": [{"type":"namespace","name":"ns","tools":[{"type":"function","name":"a b"}]}],
            "input": [{"type":"function_call","call_id":"c","name":"x y","arguments":"{}"},
                      {"type":"additional_tools","tools":[{"type":"custom","name":"p q"}]}],
            "tool_choice": {"type":"function","name":"a b"}
        });
        assert!(codex_tool_names(v.as_object_mut().unwrap()));
        assert_eq!(
            v["tools"][0]["tools"][0]["name"],
            codex_tool_name("a b").as_str()
        );
        assert_eq!(v["input"][0]["name"], codex_tool_name("x y").as_str());
        assert_eq!(
            v["input"][1]["tools"][0]["name"],
            codex_tool_name("p q").as_str()
        );
        assert_eq!(v["tool_choice"]["name"], codex_tool_name("a b").as_str());
        // 合法名：原样、不报改动
        let mut ok = json!({"tools":[{"type":"function","name":"fine"}]});
        assert!(!codex_tool_names(ok.as_object_mut().unwrap()));
        // 回程表只含改过名的
        let back = names_back_of(
            &json!({"tools":[{"type":"function","name":"a b"},{"type":"function","name":"fine"}]}),
        );
        assert_eq!(
            back.get(&codex_tool_name("a b")).map(String::as_str),
            Some("a b")
        );
        assert_eq!(back.len(), 1);
    }

    #[test]
    fn orphan_outputs_and_pairing() {
        let out = shape_input(vec![
            json!({"type":"function_call","call_id":"c1","name":"f","arguments":"{}"}),
            json!({"type":"function_call_output","call_id":"c1","output":"fine"}),
            json!({"type":"function_call_output","call_id":"gone","name":"grep","output":{"k":1}}),
            json!({"type":"custom_tool_call_output","call_id":"c1","output":"wrong kind for c1"}),
            json!({"type":"function_call_output","output":"standalone notification"}),
            json!({"type":"local_shell_call","call_id":"sh","action":{}}),
            json!({"type":"function_call_output","call_id":"sh","output":"shell ok"}),
        ]);
        assert_eq!(out[1]["type"], "function_call_output");
        assert_eq!(out[2]["role"], "assistant");
        assert_eq!(
            out[2]["content"],
            "[Previous grep result; call_id=gone]: {\"k\":1}"
        );
        assert_eq!(out[3]["role"], "assistant"); // call 种类不配对
        assert_eq!(out[4]["output"], "standalone notification"); // 无 call_id：原样
        assert_eq!(out[6]["type"], "function_call_output"); // function_call_output 可配 local_shell_call
                                                            // 超长输出截断并保持 UTF-8 合法
        let big = "界".repeat(9000);
        let o = orphan_output(&json!({"output": big}), "");
        let c = o["content"].as_str().unwrap();
        assert!(c.contains("call_id=unknown") && c.ends_with("...[truncated]"));
    }

    #[test]
    fn declare_search_follows_go_rules() {
        // 回放了搜索、没声明工具 → 补 cached web_search；没给任何工具 → tool_choice none
        let v = shape_body(
            json!({"model":"m","input":[{"type":"web_search_call","id":"ws","status":"completed"},
                                        {"type":"message","role":"user","content":"x"}]}),
            "m",
            None,
        );
        assert_eq!(v["tools"][0]["type"], "web_search");
        assert_eq!(v["tools"][0]["external_web_access"], false);
        assert_eq!(v["tool_choice"], "none");
        // 已有函数工具：补搜索，tool_choice 保持 auto
        let v = shape_body(
            json!({"input":[{"type":"web_search_call"}],"tools":[{"type":"function","name":"f"}]}),
            "m",
            None,
        );
        assert_eq!(v["tools"].as_array().unwrap().len(), 2);
        assert_eq!(v["tool_choice"], "auto");
        // 已声明（含 web.run）：不重复
        let v = shape_body(
            json!({"input":[{"type":"web_search_call"}],"tools":[{"type":"function","name":"web.run"}]}),
            "m",
            None,
        );
        assert_eq!(v["tools"].as_array().unwrap().len(), 1);
        // 没回放搜索：不动
        let v = shape_body(
            json!({"input":[{"type":"message","role":"user","content":"x"}]}),
            "m",
            None,
        );
        assert!(v.get("tools").is_none());
        // additional_tools（Lite）：补进该项而不是 tools
        let v = shape_body(
            json!({"input":[{"type":"additional_tools","tools":[{"type":"function","name":"f"}]},
                            {"type":"web_search_call"}]}),
            "m",
            None,
        );
        assert!(v.get("tools").is_none());
        assert_eq!(v["input"][0]["tools"][1]["type"], "web_search");
    }

    // ---- 出站压缩（gzip 偏差版）+ 订阅耗尽回退（codexFallBack 最小版） ----

    #[test]
    fn oversized_body_is_gzipped_and_equivalent() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            SSE_OK,
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let big = "x".repeat(COMPRESS_MIN + 10);
        let (status, _) = reply_json(super::super::routes::route(
            &post(
                "/v1/responses",
                &json!({"model":"codex/gpt-6-luna","stream":true,"input":big}),
            ),
            &gateway,
        ));
        assert_eq!(status, 200);
        let (headers, body) = mock.last("/backend-api/codex/responses");
        assert_eq!(headers["content-encoding"], "gzip");
        use flate2::read::GzDecoder;
        let mut plain = Vec::new();
        GzDecoder::new(&body[..])
            .read_to_end(&mut plain)
            .expect("gunzip");
        let sent: Value = serde_json::from_slice(&plain).expect("json");
        assert_eq!(sent["model"], "gpt-6-luna");
        // 解压后等价：整段正文原样在（shape 过的形态）
        let text = sent["input"][0]["content"][0]["text"].as_str().unwrap();
        assert_eq!(text.len(), COMPRESS_MIN + 10);
    }

    #[test]
    fn small_body_stays_plain() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            SSE_OK,
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let _ = super::super::routes::route(
            &post(
                "/v1/responses",
                &json!({"model":"codex/gpt-6-luna","stream":true,"input":"small"}),
            ),
            &gateway,
        );
        let (headers, _) = mock.last("/backend-api/codex/responses");
        assert!(!headers.contains_key("content-encoding"));
    }

    // ---- 出站 WebSocket（开关默认关；失败当轮回退 HTTP）----

    /// MARIONETTE_CODEX_WS 的测试态：进程 env 全局——设置期间必须已持有
    /// 沙箱锁（world() 自带；不再另拿防自锁），所有出站测试都串行在同一把
    /// 锁上，别的 codex 测试不会读到半截开关态。
    struct WsEnv;
    impl WsEnv {
        fn on() -> WsEnv {
            std::env::set_var("MARIONETTE_CODEX_WS", "1");
            WsEnv
        }
        fn off() -> WsEnv {
            std::env::remove_var("MARIONETTE_CODEX_WS");
            WsEnv
        }
    }
    impl Drop for WsEnv {
        fn drop(&mut self) {
            std::env::remove_var("MARIONETTE_CODEX_WS");
        }
    }

    /// 内存双工（读侧按序供给预置帧、写侧吞掉）——ws_turn_over / WsSseReader
    /// 的假对端（真 WS 端到端待真机验证）。
    struct FrameFeed {
        data: Vec<u8>,
        at: usize,
    }
    impl FrameFeed {
        fn frame(&mut self, opcode: u8, payload: &[u8]) {
            self.data
                .extend(super::super::ws::encode_server_frame(opcode, payload));
        }
    }
    impl Default for FrameFeed {
        fn default() -> Self {
            FrameFeed {
                data: Vec::new(),
                at: 0,
            }
        }
    }
    impl std::io::Read for FrameFeed {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.at >= self.data.len() {
                return Ok(0);
            }
            let n = (self.data.len() - self.at).min(buf.len());
            buf[..n].copy_from_slice(&self.data[self.at..self.at + n]);
            self.at += n;
            Ok(n)
        }
    }
    impl std::io::Write for FrameFeed {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn text_frame(payload: &[u8]) -> Vec<u8> {
        super::super::ws::encode_server_frame(super::super::ws::OP_TEXT, payload)
    }

    #[test]
    fn ws_url_maps_schemes() {
        assert_eq!(
            super::ws_url("https://chatgpt.com/backend-api/codex"),
            "wss://chatgpt.com/backend-api/codex/responses"
        );
        assert_eq!(
            super::ws_url("http://127.0.0.1:9/codex/"),
            "ws://127.0.0.1:9/codex/responses"
        );
    }

    #[test]
    fn response_create_message_prefixes_body_bytes() {
        // Go codexCreate：`{"type":"response.create",` 接在 `{` 后，原字节不动。
        let msg = super::response_create_message(br#"{ "model": "x", "stream": true }"#);
        assert_eq!(
            msg,
            br#"{"type":"response.create", "model": "x", "stream": true }"#.to_vec()
        );
        // 空对象不补逗号。
        assert_eq!(
            super::response_create_message(b"{}"),
            br#"{"type":"response.create"}"#.to_vec()
        );
        // 前后空白剥掉（Go TrimSpace 同款）。
        let msg = super::response_create_message(b"\r\n{\"stream\":true}\n");
        assert_eq!(msg, br#"{"type":"response.create","stream":true}"#.to_vec());
    }

    #[test]
    fn ws_plan_builds_go_shaped_message_headers_and_url() {
        // base_url() 读测试缝——world() 串行持有沙箱锁。
        let (_gateway, _w) = world(r#"{"providers":[]}"#, "http://127.0.0.1:9/codex");
        let account = Account {
            id: String::new(),
            token: "tok".into(),
            account_id: "acct-1".into(),
            email: "e@t.test".into(),
            plan: "pro".into(),
            exp: Some(now_secs() + 3600),
        };
        let body =
            json!({"model":"gpt-6-luna","stream":true,"input":"hi","prompt_cache_key":"thread-1"});
        let plan = super::ws_plan(&account, "0.200.0", &body).expect("plan");
        assert_eq!(plan.url, "ws://127.0.0.1:9/codex/responses");
        // 消息 = 原体前接 response.create；round-trip 回来字段原样。
        assert!(plan.message.starts_with(br#"{"type":"response.create","#));
        let msg: Value = serde_json::from_slice(&plan.message).unwrap();
        assert_eq!(msg["type"], "response.create");
        assert_eq!(msg["model"], "gpt-6-luna");
        assert_eq!(msg["prompt_cache_key"], "thread-1");
        assert_eq!(msg["stream"], true);
        // 头集合：鉴权/originator/UA/version/会话对 + WS 的 OpenAI-Beta；
        // Accept / Content-Type / Content-Encoding 一律不带（Go 剥除名单）。
        let h = |k: &str| {
            plan.headers
                .iter()
                .find(|(a, _)| a.eq_ignore_ascii_case(k))
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        assert_eq!(h("Authorization"), "Bearer tok");
        assert_eq!(h("originator"), "codex_cli_rs");
        assert!(h("User-Agent").starts_with("codex_cli_rs/0.200.0"));
        assert_eq!(h("version"), "0.200.0");
        assert_eq!(h("chatgpt-account-id"), "acct-1");
        assert_eq!(h("session-id"), "thread-1");
        assert_eq!(h("thread-id"), "thread-1");
        assert_eq!(h("OpenAI-Beta"), super::WS_BETA);
        assert!(plan
            .headers
            .iter()
            .all(|(k, _)| !k.eq_ignore_ascii_case("Accept")
                && !k.eq_ignore_ascii_case("Content-Type")
                && !k.eq_ignore_ascii_case("Content-Encoding")));
    }

    #[test]
    fn ws_plan_refuses_unshaped_bodies() {
        let account = Account {
            id: String::new(),
            token: "t".into(),
            account_id: String::new(),
            email: String::new(),
            plan: String::new(),
            exp: None,
        };
        // 非流式 / 顶层带 type：不合 WS 形状（Go codexSocketMessage :247）。
        assert!(super::ws_plan(&account, "v", &json!({"model":"m","stream":false})).is_none());
        assert!(super::ws_plan(
            &account,
            "v",
            &json!({"model":"m","stream":true,"type":"response.create"})
        )
        .is_none());
    }

    #[test]
    fn ws_sse_event_lines_and_compaction() {
        let ev = super::ws_sse_event("response.created", br#"{"type":"response.created"}"#);
        assert_eq!(
            ev,
            b"event: response.created\ndata: {\"type\":\"response.created\"}\n\n".to_vec()
        );
        // 空类型：只有 data 行（Go kind=="" 分支）。
        let ev = super::ws_sse_event("", br#"{"a":1}"#);
        assert_eq!(ev, b"data: {\"a\":1}\n\n".to_vec());
        // 帧里带换行：压成紧凑 JSON 再进 data 行。
        let ev = super::ws_sse_event("x", b"{\n  \"a\": 1\n}");
        assert_eq!(ev, b"event: x\ndata: {\"a\":1}\n\n".to_vec());
    }

    #[test]
    fn ws_error_reply_classification() {
        // 连接上限 → 回退 HTTP。
        let f = br#"{"type":"error","error":{"code":"websocket_connection_limit_reached"}}"#;
        assert!(matches!(super::ws_error_reply(f), Some(Err(()))));
        // status 429 + error 对象 → 合成 (429, {"error":…})。
        let f = br#"{"type":"error","status":429,"error":{"message":"slow down","code":"rate_limited"}}"#;
        let Some(Ok((status, body))) = super::ws_error_reply(f) else {
            panic!("429")
        };
        assert_eq!(status, 429);
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["message"], "slow down");
        // status_code 字段同样认。
        let f = br#"{"type":"error","status_code":500,"error":{"message":"boom"}}"#;
        assert!(matches!(super::ws_error_reply(f), Some(Ok((500, _)))));
        // 无可用 status：None（当普通事件传下去）。
        assert!(super::ws_error_reply(br#"{"type":"error","message":"odd"}"#).is_none());
        // status 出 400..=599：None。
        assert!(super::ws_error_reply(br#"{"type":"error","status":302}"#).is_none());
    }

    #[test]
    fn ws_turn_over_prelude_and_sse_restore() {
        let mut feed = FrameFeed::default();
        // timing 帧（丢弃）→ 头帧 rate_limits → 首条正文 → 终态。
        feed.data.extend(text_frame(
            br#"{"type":"responsesapi.websocket_timing","ms":1}"#,
        ));
        feed.data.extend(text_frame(
            br#"{"type":"codex.rate_limits","rate_limits":{"primary":{"used_percent":1}}}"#,
        ));
        feed.data.extend(text_frame(
            br#"{"type":"response.created","response":{"id":"r1"}}"#,
        ));
        feed.data.extend(text_frame(
            br#"{"type":"response.completed","response":{"id":"r1","usage":{"input_tokens":1}}}"#,
        ));
        let conn = super::super::ws::WsConn::over(Box::new(feed));
        let outcome = super::ws_turn_over(conn, br#"{"type":"response.create"}"#, "h", false, None);
        let super::WsOutcome::Stream(mut reader) = outcome else {
            panic!("expected Stream")
        };
        let mut sse = String::new();
        reader.read_to_string(&mut sse).expect("read to end");
        assert_eq!(
            sse,
            "event: codex.rate_limits\n\
             data: {\"type\":\"codex.rate_limits\",\"rate_limits\":{\"primary\":{\"used_percent\":1}}}\n\n\
             event: response.created\n\
             data: {\"type\":\"response.created\",\"response\":{\"id\":\"r1\"}}\n\n\
             event: response.completed\n\
             data: {\"type\":\"response.completed\",\"response\":{\"id\":\"r1\",\"usage\":{\"input_tokens\":1}}}\n\n"
        );
        // timing 不出现在流里；终态后干净 EOF。
        assert!(!sse.contains("websocket_timing"));
    }

    #[test]
    fn ws_turn_over_error_frame_paths() {
        // 连接上限 error 帧 → 回退 HTTP。
        let mut feed = FrameFeed::default();
        feed.data.extend(text_frame(
            br#"{"type":"error","error":{"code":"websocket_connection_limit_reached"}}"#,
        ));
        let conn = super::super::ws::WsConn::over(Box::new(feed));
        assert!(matches!(
            super::ws_turn_over(conn, b"m", "h", false, None),
            super::WsOutcome::FallBack
        ));

        // 带 status 的 error 帧 → 合成错误走现有路径（账号轮换可判）。
        let mut feed = FrameFeed::default();
        feed.data.extend(text_frame(
            br#"{"type":"error","status":429,"error":{"message":"usage limit"}}"#,
        ));
        let conn = super::super::ws::WsConn::over(Box::new(feed));
        let super::WsOutcome::Error(429, body) = super::ws_turn_over(conn, b"m", "h", false, None)
        else {
            panic!("429")
        };
        assert!(String::from_utf8_lossy(&body).contains("usage limit"));

        // 无 status 的 error 帧 → 当普通事件传下去（且是终态）。
        let mut feed = FrameFeed::default();
        feed.data
            .extend(text_frame(br#"{"type":"error","message":"odd"}"#));
        let conn = super::super::ws::WsConn::over(Box::new(feed));
        let super::WsOutcome::Stream(mut reader) =
            super::ws_turn_over(conn, b"m", "h", false, None)
        else {
            panic!("Stream")
        };
        let mut sse = String::new();
        reader.read_to_string(&mut sse).expect("eof after terminal");
        assert_eq!(
            sse,
            "event: error\ndata: {\"type\":\"error\",\"message\":\"odd\"}\n\n"
        );

        // 应答前 close / 连不上（EOF）→ 回退。
        let mut feed = FrameFeed::default();
        feed.data.extend(super::super::ws::encode_server_frame(
            super::super::ws::OP_CLOSE,
            &1000u16.to_be_bytes(),
        ));
        let conn = super::super::ws::WsConn::over(Box::new(feed));
        assert!(matches!(
            super::ws_turn_over(conn, b"m", "h", false, None),
            super::WsOutcome::FallBack
        ));
        let conn = super::super::ws::WsConn::over(Box::new(FrameFeed::default()));
        assert!(matches!(
            super::ws_turn_over(conn, b"m", "h", false, None),
            super::WsOutcome::FallBack
        ));
    }

    #[test]
    fn ws_cut_midstream_is_an_error_not_clean_eof() {
        // 正文开始后、终态前断线：事件已交付的部分保留 + io::Error（HTTP 半截
        // 同款——聚合层据此 502「stream ended before response.completed」）。
        let mut feed = FrameFeed::default();
        feed.data.extend(text_frame(
            br#"{"type":"response.created","response":{"id":"r1"}}"#,
        ));
        let conn = super::super::ws::WsConn::over(Box::new(feed));
        let super::WsOutcome::Stream(mut reader) =
            super::ws_turn_over(conn, b"m", "h", false, None)
        else {
            panic!("Stream")
        };
        let mut buf = Vec::new();
        let res = reader.read_to_end(&mut buf);
        assert!(res.is_err(), "cut stream must surface an error");
        assert!(String::from_utf8_lossy(&buf).contains("response.created"));
    }

    #[test]
    fn ws_switch_default_off_and_failure_falls_back_to_http() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            200,
            "text/event-stream",
            SSE_OK,
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        let token = valid_token();
        write_logins(&w, &token, "me@example.test");

        // 默认（无 MARIONETTE_CODEX_WS）：纯 HTTP，一次 POST。
        {
            let _off = WsEnv::off();
            let reply = super::super::routes::route(
                &post(
                    "/v1/responses",
                    &json!({"model":"codex/gpt-6-luna","stream":true,"input":"hi"}),
                ),
                &gateway,
            );
            assert_eq!(reply_json(reply).0, 200);
        }
        {
            let seen = mock.seen.lock().unwrap();
            assert_eq!(seen.len(), 1, "default must be plain HTTP");
            assert_eq!(seen[0].method, "POST");
        }

        // 开关开：先 WS 升级（Mock 回的是 200 不是 101 → 握手失败）→ 当轮
        // 回退 HTTP POST。同一回合两次请求，应答不丢。
        {
            let _on = WsEnv::on();
            let reply = super::super::routes::route(
                &post(
                    "/v1/responses",
                    &json!({"model":"codex/gpt-6-luna","stream":true,"input":"hi"}),
                ),
                &gateway,
            );
            assert_eq!(reply_json(reply).0, 200);
        }
        {
            let seen = mock.seen.lock().unwrap();
            assert_eq!(seen.len(), 3, "GET upgrade + fallback POST");
            let upgrade = &seen[1];
            assert_eq!(upgrade.method, "GET");
            assert_eq!(upgrade.path, "/backend-api/codex/responses");
            assert_eq!(upgrade.headers["upgrade"], "websocket");
            assert_eq!(upgrade.headers["connection"], "Upgrade");
            assert_eq!(upgrade.headers["sec-websocket-version"], "13");
            assert!(upgrade.headers.contains_key("sec-websocket-key"));
            assert_eq!(upgrade.headers["authorization"], format!("Bearer {token}"));
            assert_eq!(upgrade.headers["openai-beta"], super::WS_BETA);
            assert_eq!(upgrade.headers["originator"], "codex_cli_rs");
            // 回退的 POST 走原路径（明文体、SSE 应答）。
            let post = &seen[2];
            assert_eq!(post.method, "POST");
            let sent: Value = serde_json::from_slice(&post.body).unwrap();
            assert_eq!(sent["model"], "gpt-6-luna");
            assert!(
                sent.get("type").is_none(),
                "HTTP body must not carry the ws response.create prefix"
            );
        }
    }

    // ---- keep 复用 / shun 名单 / 1009 尺寸记忆（对照 codex_socket_keep_test.go
    // ---- 与 codex_socket_test.go 的同名场景）----

    /// 纯逻辑：turn_shape 的字段口径 + delta 的前缀比对（Go codexShape :218 /
    /// codexKept.delta :300 / codexSocketItem :256 的剥除口径）。
    #[test]
    fn turn_shape_and_delta_go_semantics() {
        let user = |t: &str| json!({ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": t }] });
        let asst = |t: &str| json!({ "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": t }] });
        let body1 = json!({ "model": "m", "stream": true, "store": false, "prompt_cache_key": "c1", "input": [user("ping")] });
        let shape1 = super::turn_shape(&body1).expect("shape");
        // 字段排除 input/client_metadata/stream_options，其余全收。
        assert!(shape1.fields.contains_key("model") && shape1.fields.contains_key("store"));
        assert!(
            !shape1.fields.contains_key("input") && !shape1.fields.contains_key("client_metadata")
        );

        // 池内条目：上轮 input [user ping] + output [asst pong]、reply resp-1。
        let kept = super::KeptConn {
            conn: super::super::ws::WsConn::over(Box::new(FrameFeed::default())),
            key: String::new(),
            opened: Instant::now(),
            used: Instant::now(),
            fields: shape1.fields.clone(),
            input: shape1.sums.clone(),
            output: vec![super::item_digest(&json!({ "type": "message", "role": "assistant", "id": "msg_a",
                "status": "completed", "content": [{ "type": "output_text", "annotations": [], "logprobs": null, "text": "pong" }] }))
                .unwrap()],
            reply_id: "resp-1".into(),
        };
        // 续跑回合：前缀 = 上轮 input + 上轮 output（output 的 id/status/
        // annotations/logprobs 都被剥掉后应与请求里的干净条目同摘要）。
        let body2 = json!({ "model": "m", "stream": true, "store": false, "prompt_cache_key": "c1",
            "input": [user("ping"), asst("pong"), user("again")] });
        let shape2 = super::turn_shape(&body2).expect("shape 2");
        let delta = super::delta_message(&kept, &body2, &shape2).expect("delta");
        let msg: Value = serde_json::from_slice(&delta).unwrap();
        assert_eq!(msg["previous_response_id"], "resp-1");
        assert_eq!(msg["input"].as_array().map(|a| a.len()), Some(1));
        assert_eq!(msg["input"][0]["content"][0]["text"], "again");
        assert_eq!(msg["model"], "m", "delta keeps the turn's fields");
        assert!(delta.starts_with(br#"{"type":"response.create","#));

        // 编辑过的历史：前缀对不上 → 整段。
        let edited = json!({ "model": "m", "stream": true, "store": false, "prompt_cache_key": "c1",
            "input": [user("PING"), asst("pong"), user("again")] });
        let s = super::turn_shape(&edited).unwrap();
        assert!(
            super::delta_message(&kept, &edited, &s).is_none(),
            "edited history goes whole"
        );

        // 换模型（字段变了）→ 整段；少一条前缀 → 整段。
        let other_model = json!({ "model": "m2", "stream": true, "store": false, "prompt_cache_key": "c1",
            "input": [user("ping"), asst("pong"), user("again")] });
        assert!(super::delta_message(
            &kept,
            &other_model,
            &super::turn_shape(&other_model).unwrap()
        )
        .is_none());
        let short = json!({ "model": "m", "stream": true, "store": false, "prompt_cache_key": "c1", "input": [user("ping")] });
        assert!(super::delta_message(&kept, &short, &super::turn_shape(&short).unwrap()).is_none());

        // 上轮无 reply id → 整段；body 自带 previous_response_id → 不参与形状。
        let no_id = super::KeptConn {
            conn: super::super::ws::WsConn::over(Box::new(FrameFeed::default())),
            key: String::new(),
            opened: Instant::now(),
            used: Instant::now(),
            fields: shape1.fields.clone(),
            input: shape1.sums.clone(),
            output: kept.output.clone(),
            reply_id: String::new(),
        };
        assert!(super::delta_message(&no_id, &body2, &shape2).is_none());
        let named = json!({ "model": "m", "stream": true, "previous_response_id": "x", "input": [user("ping")] });
        assert!(
            super::turn_shape(&named).is_none(),
            "a body naming previous_response_id is not continuable"
        );
        // input 非数组 → 不参与形状（Go Unmarshal 到数组失败同款）。
        assert!(
            super::turn_shape(&json!({ "model": "m", "stream": true, "input": "hi" })).is_none()
        );
    }

    /// 纯逻辑：dial 错误 → 要不要记 shun 的分类（Go socket :375-386）。
    #[test]
    fn shun_worthy_classification() {
        // 非 101 的 HTTP 应答：401/429/5xx 豁免，403/404 记 shun。
        assert!(!super::shun_worthy("ws handshake refused: HTTP 401 X"));
        assert!(!super::shun_worthy(
            "ws handshake refused: HTTP 429 Too Many"
        ));
        assert!(!super::shun_worthy("ws handshake refused: HTTP 503 busy"));
        assert!(super::shun_worthy("ws handshake refused: HTTP 403 nope"));
        assert!(super::shun_worthy("ws handshake refused: HTTP 404 X"));
        // 连接层失败（Go hs == nil）：一律 shun。
        assert!(super::shun_worthy("ws connect 1.2.3.4:443: timed out"));
        assert!(super::shun_worthy(
            "ws handshake: connection closed before 101"
        ));
        assert!(super::shun_worthy(
            "ws handshake: Sec-WebSocket-Accept mismatch"
        ));
    }

    /// 纯逻辑：会话键把账号/会话分开（Go codexKeepKey :102）。
    #[test]
    fn keep_key_separates_accounts_and_conversations() {
        let (_gateway, _w) = world(r#"{"providers":[]}"#, "http://127.0.0.1:9/codex");
        let mk = |token: &str, acct: &str, conv: &str| {
            let account = Account {
                id: String::new(),
                token: token.into(),
                account_id: acct.into(),
                email: String::new(),
                plan: String::new(),
                exp: Some(now_secs() + 3600),
            };
            let body = json!({"model":"m","stream":true,"input":"hi","prompt_cache_key":conv});
            super::ws_plan(&account, "v", &body).unwrap()
        };
        let a1 = super::keep_key(&mk("t1", "acct-1", "conv-1")).unwrap();
        let a1b = super::keep_key(&mk("t1", "acct-1", "conv-1")).unwrap();
        let a2 = super::keep_key(&mk("t2", "acct-2", "conv-1")).unwrap();
        let c2 = super::keep_key(&mk("t1", "acct-1", "conv-2")).unwrap();
        assert_eq!(a1, a1b, "same account+conversation → same key");
        assert_ne!(a1, a2, "another account never finds it");
        assert_ne!(a1, c2, "another conversation never finds it");
        // 没有会话标识（prompt_cache_key 缺席 → 头里没有 session-id）→ None。
        let account = Account {
            id: String::new(),
            token: "t".into(),
            account_id: String::new(),
            email: String::new(),
            plan: String::new(),
            exp: None,
        };
        let plan = super::ws_plan(
            &account,
            "v",
            &json!({"model":"m","stream":true,"input":"hi"}),
        )
        .unwrap();
        assert!(super::keep_key(&plan).is_none());
    }

    /// 一条 WS 消息的记录：载荷、第几条握手、鉴权头。
    struct FakeTurn {
        msg: Vec<u8>,
        socket: usize,
        auth: String,
    }

    /// ChatGPT 后端假身（Go fakeSocketBackend codex_socket_test.go :33 的形
    /// 态）：Upgrade 可拒（refuse）；每条 WS 消息按脚本应答——frames(握手
    /// 序号, 消息) → (帧列表, 关闭码)：帧逐条 text 发出，关闭码 Some 则补
    /// close 帧后断开（1009 = message too big、1000 = 正常收尾），None 则
    /// 连接留着等下一回合（keep 复用的形状）；HTTP POST 回整段 SSE_OK。
    struct WsFake {
        base: String,
        turns: Arc<Mutex<Vec<FakeTurn>>>,
        handshakes: Arc<AtomicUsize>,
        posts: Arc<AtomicUsize>,
    }

    impl WsFake {
        fn start(
            refuse: Option<u16>,
            frames: impl Fn(usize, &[u8]) -> (Vec<String>, Option<u16>) + Send + Sync + 'static,
        ) -> WsFake {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let port = listener.local_addr().unwrap().port();
            let turns: Arc<Mutex<Vec<FakeTurn>>> = Arc::new(Mutex::new(Vec::new()));
            let handshakes = Arc::new(AtomicUsize::new(0));
            let posts = Arc::new(AtomicUsize::new(0));
            let (t, h, p) = (turns.clone(), handshakes.clone(), posts.clone());
            let frames = Arc::new(frames);
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let (t, h, p) = (t.clone(), h.clone(), p.clone());
                    let frames = Arc::clone(&frames);
                    std::thread::spawn(move || {
                        let mut reader = BufReader::new(stream.try_clone().unwrap());
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            return;
                        }
                        let mut headers = HashMap::new();
                        loop {
                            let mut hl = String::new();
                            if reader.read_line(&mut hl).unwrap_or(0) == 0 {
                                break;
                            }
                            let trimmed = hl.trim_end();
                            if trimmed.is_empty() {
                                break;
                            }
                            if let Some((k, v)) = trimmed.split_once(':') {
                                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                            }
                        }
                        if line.starts_with("POST") {
                            let len: usize = headers
                                .get("content-length")
                                .and_then(|l| l.parse().ok())
                                .unwrap_or(0);
                            let mut body = vec![0u8; len];
                            let _ = reader.read_exact(&mut body);
                            p.fetch_add(1, Ordering::SeqCst);
                            let out = format!(
                                "HTTP/1.1 200 X\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                SSE_OK.len(),
                                SSE_OK
                            );
                            let _ = stream.write_all(out.as_bytes());
                            return;
                        }
                        // GET Upgrade：握手尝试数在这里计（被拒的也算——Go
                        // fakeSocketBackend 的 handshakes「a refused message's
                        // too」同款口径）。
                        let n = h.fetch_add(1, Ordering::SeqCst) + 1;
                        let auth = headers.get("authorization").cloned().unwrap_or_default();
                        if let Some(code) = refuse {
                            let out = format!(
                                "HTTP/1.1 {code} refused\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            );
                            let _ = stream.write_all(out.as_bytes());
                            return;
                        }
                        let key = headers
                            .get("sec-websocket-key")
                            .cloned()
                            .unwrap_or_default();
                        let accept = super::super::ws::accept_for(&key);
                        let res = format!(
                            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
                        );
                        if stream.write_all(res.as_bytes()).is_err() {
                            return;
                        }
                        loop {
                            let Some((op, payload)) = read_client_frame(&mut reader) else {
                                break;
                            };
                            if op == super::super::ws::OP_CLOSE {
                                break;
                            }
                            if op != super::super::ws::OP_TEXT {
                                continue;
                            }
                            t.lock().unwrap().push(FakeTurn {
                                msg: payload.clone(),
                                socket: n,
                                auth: auth.clone(),
                            });
                            let (reply, close) = frames(n, &payload);
                            for fr in reply {
                                if stream
                                    .write_all(&super::super::ws::encode_server_frame(
                                        super::super::ws::OP_TEXT,
                                        fr.as_bytes(),
                                    ))
                                    .is_err()
                                {
                                    return;
                                }
                            }
                            if let Some(code) = close {
                                let _ = stream.write_all(&super::super::ws::encode_server_frame(
                                    super::super::ws::OP_CLOSE,
                                    &code.to_be_bytes(),
                                ));
                                return; // drop 连接
                            }
                        }
                    });
                }
            });
            WsFake {
                base: format!("http://127.0.0.1:{port}/backend-api/codex"),
                turns,
                handshakes,
                posts,
            }
        }

        fn opened(&self) -> usize {
            self.handshakes.load(Ordering::SeqCst)
        }
        fn posts(&self) -> usize {
            self.posts.load(Ordering::SeqCst)
        }
        fn turn(&self, i: usize) -> (usize, String, Vec<u8>) {
            let turns = self.turns.lock().unwrap();
            let t = &turns[i];
            (t.socket, t.auth.clone(), t.msg.clone())
        }
    }

    /// 读一条客户端帧（恒掩码）并解掩码；EOF/断线 → None。
    fn read_client_frame<S: Read>(stream: &mut S) -> Option<(u8, Vec<u8>)> {
        let mut h = [0u8; 2];
        stream.read_exact(&mut h).ok()?;
        let opcode = h[0] & 0x0F;
        let len = match h[1] & 0x7F {
            126 => {
                let mut b = [0u8; 2];
                stream.read_exact(&mut b).ok()?;
                u16::from_be_bytes(b) as usize
            }
            127 => {
                let mut b = [0u8; 8];
                stream.read_exact(&mut b).ok()?;
                u64::from_be_bytes(b) as usize
            }
            n => n as usize,
        };
        let mut mask = [0u8; 4];
        stream.read_exact(&mut mask).ok()?;
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).ok()?;
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= mask[i & 3];
        }
        Some((opcode, payload))
    }

    /// 假后端的一条完整回合应答（created → output_item.done(item pong) →
    /// completed(id)，item 带 id/status/annotations/logprobs——剥除口径的
    /// 「后端原话」形态）。
    fn kept_reply(id: &str) -> Vec<String> {
        vec![
            format!(r#"{{"type":"response.created","response":{{"id":"{id}"}}}}"#),
            r#"{"type":"response.output_item.done","output_index":0,"item":{"id":"msg_a","type":"message","role":"assistant","status":"completed","content":[{"type":"output_text","annotations":[],"logprobs":null,"text":"pong"}]}}"#.into(),
            format!(r#"{{"type":"response.completed","response":{{"id":"{id}","usage":{{"input_tokens":7,"output_tokens":1}}}}}}"#),
        ]
    }

    const USER_PING: &str =
        r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}"#;
    const ASST_PONG: &str =
        r#"{"type":"message","role":"assistant","content":[{"type":"output_text","text":"pong"}]}"#;
    const USER_AGAIN: &str =
        r#"{"type":"message","role":"user","content":[{"type":"input_text","text":"again"}]}"#;

    /// 带 prompt_cache_key 的整段回合体（input = 逗号拼接的条目串）。
    fn kept_turn_body(conv: &str, items: &str) -> Value {
        serde_json::from_str(&format!(
            r#"{{"model":"codex/gpt-6-luna","stream":true,"prompt_cache_key":"{conv}","input":[{items}]}}"#
        ))
        .unwrap()
    }

    fn run_turn(gateway: &Gateway, body: &Value) -> (u16, String) {
        let reply = super::super::routes::route(&post("/v1/responses", body), gateway);
        let (status, v) = reply_json(reply);
        (status, v.as_str().unwrap_or_default().to_string())
    }

    /// rem_thank 场景（Go TestCodexKeptSocketSendsTheDelta）：会话的第二回合
    /// 在 keep 连接上只发增量（previous_response_id + 新输入）；编辑过的历史
    /// 和别的模型在前缀/字段对不上时整段发，但仍在这条连接上。
    #[test]
    fn ws_keep_sends_the_delta_and_edited_history_goes_whole() {
        let fake = WsFake::start(None, |socket, _| {
            (kept_reply(&format!("resp-{socket}")), None)
        });
        let (gateway, w) = world(r#"{"providers":[]}"#, &fake.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let _on = WsEnv::on();

        // 第一回合：整段（input 只有 user ping）。
        let (status, body) = run_turn(&gateway, &kept_turn_body("conv-1", USER_PING));
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("response.completed"), "{body}");

        // 第二回合：增量（input = ping + assistant pong + user again）。
        let (status, body) = run_turn(
            &gateway,
            &kept_turn_body("conv-1", &format!("{USER_PING}, {ASST_PONG}, {USER_AGAIN}")),
        );
        assert_eq!(status, 200, "{body}");

        // 两回合一条连接、零 HTTP POST；第二回合带 previous_response_id 且
        // input 只剩新条目。
        assert_eq!((fake.posts(), fake.opened()), (0, 1));
        let (sock1, _, msg1) = fake.turn(0);
        let m1: Value = serde_json::from_slice(&msg1).unwrap();
        assert_eq!(sock1, 1);
        assert!(m1.get("previous_response_id").is_none());
        let (sock2, _, msg2) = fake.turn(1);
        assert_eq!(sock2, 1, "second turn on the kept socket");
        let m2: Value = serde_json::from_slice(&msg2).unwrap();
        assert_eq!(m2["previous_response_id"], "resp-1");
        let input = m2["input"].as_array().unwrap();
        assert_eq!(input.len(), 1, "delta input is only the new item");
        assert_eq!(input[0]["content"][0]["text"], "again");
        assert_eq!(m2["model"], "gpt-6-luna", "delta keeps the turn's fields");
        assert!(msg2.starts_with(br#"{"type":"response.create","#));

        // 编辑过的历史：整段，但还在 keep 连接上（不重握手）。
        let edited = USER_PING.replace("ping", "PING");
        let (status, body) = run_turn(
            &gateway,
            &kept_turn_body("conv-1", &format!("{edited}, {ASST_PONG}, {USER_AGAIN}")),
        );
        assert_eq!(status, 200, "{body}");
        let (sock3, _, msg3) = fake.turn(2);
        let m3: Value = serde_json::from_slice(&msg3).unwrap();
        assert_eq!(sock3, 1, "edited history still on the kept socket");
        assert!(
            m3.get("previous_response_id").is_none(),
            "edited history goes whole"
        );
        assert_eq!(m3["input"].as_array().unwrap().len(), 3);
        assert_eq!((fake.posts(), fake.opened()), (0, 1));
    }

    /// 后端丢了 response（previous_response_not_found，Go
    /// TestCodexKeptSocketLostResponseGoesWhole）：复用连接上的 error 帧 →
    /// Again → 新连接整段重发，回合照常完成。
    #[test]
    fn ws_keep_lost_response_goes_whole_on_new_socket() {
        let fake = WsFake::start(None, |socket, msg| {
            if msg
                .windows(21)
                .any(|w| w.starts_with(br#""previous_response_id"#))
            {
                (
                    vec![r#"{"type":"error","status":400,"error":{"type":"invalid_request_error","code":"previous_response_not_found","message":"Previous response with id 'resp-1' not found."}}"#.to_string()],
                    None,
                )
            } else {
                (kept_reply(&format!("resp-{socket}")), None)
            }
        });
        let (gateway, w) = world(r#"{"providers":[]}"#, &fake.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let _on = WsEnv::on();

        let (status, body) = run_turn(&gateway, &kept_turn_body("conv-1", USER_PING));
        assert_eq!(status, 200, "{body}");
        let (status, body) = run_turn(
            &gateway,
            &kept_turn_body("conv-1", &format!("{USER_PING}, {ASST_PONG}, {USER_AGAIN}")),
        );
        assert_eq!(status, 200, "{body}");

        // 三条消息、两条握手、零 POST：delta 被拒后整段重发在新连接上。
        assert_eq!((fake.posts(), fake.opened()), (0, 2));
        let turns = fake.turns.lock().unwrap();
        assert_eq!(turns.len(), 3);
        let last = &turns[2];
        assert_eq!(last.socket, 2, "retry on the second socket");
        let m: Value = serde_json::from_slice(&last.msg).unwrap();
        assert!(m.get("previous_response_id").is_none(), "retry goes whole");
        assert_eq!(m["input"].as_array().unwrap().len(), 3);
    }

    /// 后端在回合后关了连接（Go TestCodexKeptSocketClosedMeanwhileGoesWhole）：
    /// 下一回合取出的 keep 连接已死 → Again → 新连接整段。
    #[test]
    fn ws_keep_closed_meanwhile_goes_whole_on_new_socket() {
        let fake = WsFake::start(None, |socket, _| {
            (kept_reply(&format!("resp-{socket}")), Some(1000))
        });
        let (gateway, w) = world(r#"{"providers":[]}"#, &fake.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let _on = WsEnv::on();

        let (status, body) = run_turn(&gateway, &kept_turn_body("conv-1", USER_PING));
        assert_eq!(status, 200, "{body}");
        let (status, body) = run_turn(
            &gateway,
            &kept_turn_body("conv-1", &format!("{USER_PING}, {ASST_PONG}, {USER_AGAIN}")),
        );
        assert_eq!(status, 200, "{body}");
        assert_eq!((fake.posts(), fake.opened()), (0, 2));
        let (sock, _, msg) = fake.turn(1);
        assert_eq!(sock, 2, "the dead kept socket is replaced");
        let m: Value = serde_json::from_slice(&msg).unwrap();
        assert!(
            m.get("previous_response_id").is_none(),
            "a new socket is sent the turn whole"
        );
    }

    /// 别的账号绝不复用第一家的连接，也不点名它家的 response（Go
    /// TestCodexKeptSocketIsTheAccounts）。
    #[test]
    fn ws_keep_is_the_accounts() {
        let fake = WsFake::start(None, |socket, _| {
            (kept_reply(&format!("resp-{socket}")), None)
        });
        let (gateway, w) = world(r#"{"providers":[]}"#, &fake.base);
        let a = fake_jwt(json!({ "exp": now_secs() + 9000 }));
        let b = fake_jwt(json!({ "exp": now_secs() + 1000 }));
        write_logins_multi(
            &w,
            &[
                ("acc-a", "a@example.test", &a, "acct-a"),
                ("acc-b", "b@example.test", &b, "acct-b"),
            ],
        );
        let _on = WsEnv::on();

        let first = kept_turn_body("conv-1", USER_PING);
        let mut pinned = first.clone();
        pinned["model"] = Value::String("codex/gpt-6-luna#acc-a".into());
        assert_eq!(run_turn(&gateway, &pinned).0, 200);
        let mut second =
            kept_turn_body("conv-1", &format!("{USER_PING}, {ASST_PONG}, {USER_AGAIN}"));
        second["model"] = Value::String("codex/gpt-6-luna#acc-b".into());
        assert_eq!(run_turn(&gateway, &second).0, 200);

        assert_eq!(
            (fake.posts(), fake.opened()),
            (0, 2),
            "each account its own socket"
        );
        let (sock, auth, msg) = fake.turn(1);
        assert_eq!(sock, 2);
        assert_eq!(auth, format!("Bearer {b}"));
        let m: Value = serde_json::from_slice(&msg).unwrap();
        assert!(
            m.get("previous_response_id").is_none(),
            "acct-2 never names acct-1's response"
        );
    }

    /// 握手被拒（403）：当轮回退 HTTP，10 分钟内不再试 WS（Go
    /// TestCodexSocketRefusedGoesOverHTTP）。
    #[test]
    fn ws_shun_refused_handshake_goes_http_and_stays_http() {
        let fake = WsFake::start(Some(403), |_, _| (Vec::new(), None));
        let (gateway, w) = world(r#"{"providers":[]}"#, &fake.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let _on = WsEnv::on();

        for _ in 0..2 {
            let (status, body) = run_turn(&gateway, &kept_turn_body("conv-1", USER_PING));
            assert_eq!(status, 200, "{body}");
            assert!(body.contains("response.completed"), "{body}");
        }
        // 两次 HTTP POST、只有一次升级尝试（第二次被 shun 名单直接拦下）。
        assert_eq!((fake.posts(), fake.opened()), (2, 1));
    }

    /// Lamrfu 场景（Go TestCodexSocketTooBigGoesOverHTTP）：大回合被 1009
    /// 关闭 → 当轮回退 HTTP 并记下尺寸；期内同等大小直接 HTTP 不再连；小
    /// 回合不受影响照走 WS。
    #[test]
    fn ws_too_big_1009_goes_http_and_small_turns_return() {
        let fake = WsFake::start(None, |socket, msg| {
            // 大回合：读完后 1009 关闭（后端 message too big 的形状，Go 用
            // SetReadLimit 让 Read 内部关——等价于读完即 close）；小回合正常应答。
            if msg.len() > 2000 {
                (Vec::new(), Some(1009))
            } else {
                (kept_reply(&format!("resp-{socket}")), None)
            }
        });
        let (gateway, w) = world(r#"{"providers":[]}"#, &fake.base);
        write_logins(&w, &valid_token(), "me@example.test");
        let _on = WsEnv::on();

        let big_text = format!("long conversation {}", "x".repeat(4000));
        let big = json!({ "model": "codex/gpt-6-luna", "stream": true, "prompt_cache_key": "conv-1",
            "input": [{ "type": "message", "role": "user", "content": [{ "type": "input_text", "text": big_text }] }] });
        // 第一回合：WS 连上、消息发出、应答前被 1009 关 → 回退 HTTP。
        let (status, body) = run_turn(&gateway, &big);
        assert_eq!(status, 200, "{body}");
        // 第二回合（同样大）：期内不再连 WS，直接 HTTP。
        let (status, body) = run_turn(&gateway, &big);
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            (fake.posts(), fake.opened()),
            (2, 1),
            "one socket tried, both turns over HTTP"
        );
        // 小回合回到 WS。
        let (status, body) = run_turn(&gateway, &kept_turn_body("conv-2", USER_PING));
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("response.completed"), "{body}");
        assert_eq!(
            (fake.posts(), fake.opened()),
            (2, 2),
            "the small turn goes over the socket"
        );
        assert_eq!(
            fake.turns.lock().unwrap().len(),
            2,
            "both WS messages were seen"
        );
    }

    #[test]
    fn account_out_word_table_matches_go() {
        // 429：ChatGPT 的标准额度拒绝（代码 / 措辞）
        assert!(codex_account_out(429, br#"{"error":{"code":"usage_limit_reached","message":"You've reached your usage limit"}}"#));
        assert!(codex_account_out(
            429,
            br#"{"error":{"message":"Too many requests"}}"#
        ));
        assert!(codex_account_out(429, b"Rate limit exceeded"));
        assert!(codex_account_out(429, br#"rate limit per minute exceeded"#)); // 分钟级 → rate
                                                                               // 403 + usage_not_included（没有 Codex 的套餐）
        assert!(codex_account_out(
            403,
            br#"{"detail":"usage_not_included"}"#
        ));
        // 402：credit
        assert!(codex_account_out(
            402,
            br#"{"error":{"message":"insufficient credits"}}"#
        ));
        // 401 永远不算（Go codexAccountOut：Codex 在 401 上自己续登录）
        assert!(!codex_account_out(401, br#"{"detail":"Token expired"}"#));
        // 400/404 普通措辞不算
        assert!(!codex_account_out(400, br#"{"detail":"invalid request"}"#));
        assert!(!codex_account_out(404, br#"{"detail":"Model not found"}"#));
        assert!(!codex_account_out(
            400,
            b"This model's maximum context length is 8192 tokens"
        ));
        // 5xx 过载不算；5xx + 额度代码算（quotaCodes）
        assert!(!codex_account_out(
            500,
            br#"{"error":{"message":"internal error"}}"#
        ));
        assert!(codex_account_out(
            500,
            br#"{"error":{"code":"usage_limit_reached"}}"#
        ));
        // <500 + 额度词算（usedUpWords）
        assert!(codex_account_out(
            400,
            br#"{"error":{"message":"You exceeded your current quota"}}"#
        ));
        // 中文词表（额度/余额）
        assert!(codex_account_out(429, "额度已用完，请明天再来".as_bytes()));
        assert!(codex_account_out(402, "余额不足，请充值".as_bytes()));
    }

    #[test]
    fn account_out_rotates_to_the_next_account() {
        let mock = Mock::start();
        mock.route_seq(
            "/backend-api/codex/responses",
            &[
                (
                    429,
                    "application/json",
                    r#"{"error":{"code":"usage_limit_reached","message":"usage limit reached"}}"#,
                ),
                (200, "text/event-stream", SSE_OK),
            ],
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        // a 到期更晚 → 默认先试 a；a 被额度拒 → 换 b
        let a = fake_jwt(json!({ "exp": now_secs() + 9000 }));
        let b = fake_jwt(json!({ "exp": now_secs() + 1000 }));
        write_logins_multi(
            &w,
            &[
                ("acc-a", "a@example.test", &a, "acct-a"),
                ("acc-b", "b@example.test", &b, "acct-b"),
            ],
        );
        let (status, resp) = reply_json(super::super::routes::route(
            &post(
                "/v1/chat/completions",
                &json!({"model":"codex/gpt-6-luna","messages":[{"role":"user","content":"x"}]}),
            ),
            &gateway,
        ));
        assert_eq!(status, 200);
        assert_eq!(resp["choices"][0]["message"]["content"], "hi");
        assert_eq!(mock.hits(), 2);
        let seen = mock.seen.lock().unwrap();
        assert_eq!(seen[0].headers["authorization"], format!("Bearer {a}"));
        assert_eq!(seen[1].headers["authorization"], format!("Bearer {b}"));
        assert_eq!(seen[1].headers["chatgpt-account-id"], "acct-b");
    }

    #[test]
    fn all_accounts_exhausted_is_a_determinable_429() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            429,
            "application/json",
            r#"{"error":{"code":"usage_limit_reached","message":"You've reached your usage limit"}}"#,
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        let a = fake_jwt(json!({ "exp": now_secs() + 9000 }));
        let b = fake_jwt(json!({ "exp": now_secs() + 1000 }));
        write_logins_multi(
            &w,
            &[
                ("acc-a", "a@example.test", &a, "acct-a"),
                ("acc-b", "b@example.test", &b, "acct-b"),
            ],
        );
        // chat 入站：429 + 「订阅耗尽」文案 + 代码（groups::retryable 对 429 恒真）
        let (status, body) = reply_json(super::super::routes::route(
            &post(
                "/v1/chat/completions",
                &json!({"model":"codex/gpt-6-luna","messages":[{"role":"user","content":"x"}]}),
            ),
            &gateway,
        ));
        assert_eq!(status, 429);
        let msg = body["error"]["message"].as_str().unwrap();
        assert!(msg.contains("exhausted"), "{msg}");
        assert!(msg.contains("usage limit"), "{msg}");
        assert_eq!(body["error"]["code"], "usage_limit_reached");
        assert_eq!(mock.hits(), 2); // 两个账号都试过了
                                    // anthropic 入站：rate_limit_error 信封
        let (status, body) = reply_json(super::super::routes::route(
            &post(
                "/v1/messages",
                &json!({"model":"codex/gpt-6-luna","max_tokens":9,"messages":[{"role":"user","content":"x"}]}),
            ),
            &gateway,
        ));
        assert_eq!(status, 429);
        assert_eq!(body["error"]["type"], "rate_limit_error");
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("exhausted"));
        // Responses 入站（流式/非流式同错）
        let (status, body) = reply_json(super::super::routes::route(
            &post(
                "/v1/responses",
                &json!({"model":"codex/gpt-6-luna","stream":true,"input":"x"}),
            ),
            &gateway,
        ));
        assert_eq!(status, 429);
        assert_eq!(body["error"]["code"], "usage_limit_reached");
    }

    #[test]
    fn pinned_account_exhausted_does_not_leak_to_other_accounts() {
        let mock = Mock::start();
        mock.route(
            "/backend-api/codex/responses",
            429,
            "application/json",
            r#"{"error":{"code":"usage_limit_reached","message":"usage limit reached"}}"#,
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        let a = fake_jwt(json!({ "exp": now_secs() + 9000 }));
        let b = fake_jwt(json!({ "exp": now_secs() + 1000 }));
        write_logins_multi(
            &w,
            &[
                ("acc-a", "a@example.test", &a, "acct-a"),
                ("acc-b", "b@example.test", &b, "acct-b"),
            ],
        );
        // 钉死 = 只有它：耗尽就是耗尽，不落到别的账号（Go memberAccountKept 同款）
        let (status, body) = reply_json(super::super::routes::route(
            &post(
                "/v1/chat/completions",
                &json!({"model":"codex/gpt-6-luna#acc-a","messages":[{"role":"user","content":"x"}]}),
            ),
            &gateway,
        ));
        assert_eq!(status, 429);
        assert!(body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("exhausted"));
        assert_eq!(mock.hits(), 1);
        assert_eq!(
            mock.last("/backend-api/codex/responses").0["authorization"],
            format!("Bearer {a}")
        );
    }

    #[test]
    fn non_account_errors_do_not_rotate() {
        let mock = Mock::start();
        mock.route("/backend-api/codex/responses", 500, "text/plain", "busy");
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        let a = fake_jwt(json!({ "exp": now_secs() + 9000 }));
        let b = fake_jwt(json!({ "exp": now_secs() + 1000 }));
        write_logins_multi(
            &w,
            &[
                ("acc-a", "a@example.test", &a, "acct-a"),
                ("acc-b", "b@example.test", &b, "acct-b"),
            ],
        );
        let (status, body) = reply_json(super::super::routes::route(
            &post(
                "/v1/chat/completions",
                &json!({"model":"codex/gpt-6-luna","messages":[{"role":"user","content":"x"}]}),
            ),
            &gateway,
        ));
        // 5xx 过载不是「这个账号不行」：原样取回（Go codexBusy 另有重试，这里
        // 交给上层），不换账号
        assert_eq!(status, 500);
        assert_eq!(mock.hits(), 1);
        assert_eq!(body["error"]["message"], "busy");
    }
}
