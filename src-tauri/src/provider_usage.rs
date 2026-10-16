//! Probe provider billing/balance for the model currently selected in OpenCode (and similar).
//!
//! Provider credentials are read from OpenCode's local auth store
//! (`~/.local/share/opencode/auth.json`).
//! Secrets never leave this module in returned payloads.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageWindow {
    pub id: String,
    pub label: String,
    pub percentage: Option<f64>,
    pub detail: Option<String>,
    pub kind: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageSnapshot {
    pub provider: String,
    pub provider_label: String,
    pub model: Option<String>,
    pub model_label: Option<String>,
    pub windows: Vec<ProviderUsageWindow>,
    pub note: Option<String>,
    pub refreshed_at: String,
    pub source: String,
    pub ok: bool,
}

fn now_iso() -> String {
    // Keep dependency-free; UI formats for display.
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// OpenCode stores API keys here on all platforms we care about.
pub fn opencode_auth_path() -> Option<PathBuf> {
    let home = home_dir()?;
    let candidates = [
        home.join(".local").join("share").join("opencode").join("auth.json"),
        home.join(".config").join("opencode").join("auth.json"),
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|p| p.join("opencode").join("auth.json"))
            .unwrap_or_default(),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

fn load_auth_json() -> Result<Value, String> {
    let path = opencode_auth_path().ok_or_else(|| {
        "OpenCode auth.json not found (run `opencode providers login` first)".to_string()
    })?;
    let text = fs::read_to_string(&path)
        .map_err(|e| format!("Read auth.json failed: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("Parse auth.json failed: {e}"))
}

// ─── Provider catalog (builtin ∪ user providers.json ∪ auth.json) ───────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbeStrategy {
    Deepseek,
    Openrouter,
    Openai,
    OpencodeZen,
    Kimi,
    Glm,
    None,
}

impl ProbeStrategy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Deepseek => "deepseek",
            Self::Openrouter => "openrouter",
            Self::Openai => "openai",
            Self::OpencodeZen => "opencode-zen",
            Self::Kimi => "kimi",
            Self::Glm => "glm",
            Self::None => "none",
        }
    }

    fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "deepseek" => Self::Deepseek,
            "openrouter" => Self::Openrouter,
            "openai" | "chatgpt" | "codex" => Self::Openai,
            "opencode-zen" | "opencode_zen" | "zen" => Self::OpencodeZen,
            "kimi" | "moonshot" => Self::Kimi,
            "glm" | "zhipu" | "bigmodel" => Self::Glm,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Debug)]
struct ProviderMeta {
    id: String,
    label: String,
    key_aliases: Vec<String>,
    probe: ProbeStrategy,
    source: &'static str, // "builtin" | "user"
}

/// Built-in catalog — formerly hard-coded match arms in label / alias / display.
fn builtin_catalog() -> Vec<ProviderMeta> {
    fn b(id: &str, label: &str, aliases: &[&str], probe: ProbeStrategy) -> ProviderMeta {
        ProviderMeta {
            id: id.to_string(),
            label: label.to_string(),
            key_aliases: aliases.iter().map(|s| (*s).to_string()).collect(),
            probe,
            source: "builtin",
        }
    }
    vec![
        b("deepseek", "DeepSeek", &["deepseek"], ProbeStrategy::Deepseek),
        b("openrouter", "OpenRouter", &["openrouter"], ProbeStrategy::Openrouter),
        b(
            "openai",
            "OpenAI",
            &["openai", "chatgpt", "codex"],
            ProbeStrategy::Openai,
        ),
        b("anthropic", "Anthropic", &["anthropic"], ProbeStrategy::None),
        b("google", "Google", &["google"], ProbeStrategy::None),
        b("xai", "xAI (Grok)", &["xai", "grok"], ProbeStrategy::None),
        // Z.AI (z.ai) 与智谱 open.bigmodel.cn 是两家厂商（附录 B1「zai 不映射
        // glm」），glm/zhipu 的凭证归下面的 glm 条目，这里不再共用别名。
        b("zai", "Z.AI (GLM)", &["zai", "z-ai"], ProbeStrategy::None),
        b(
            "glm",
            "GLM Coding Plan (Zhipu)",
            &["glm", "zhipu", "bigmodel"],
            ProbeStrategy::Glm,
        ),
        b(
            "kimi",
            "Kimi (Moonshot)",
            &["kimi", "moonshot"],
            ProbeStrategy::Kimi,
        ),
        b(
            "siliconflow",
            "SiliconFlow",
            &["siliconflow", "siliconflow-cn"],
            ProbeStrategy::None,
        ),
        b(
            "opencode-go",
            "OpenCode Go",
            &["opencode-go", "opencode", "opencode-zen", "go"],
            ProbeStrategy::OpencodeZen,
        ),
        b(
            "opencode",
            "OpenCode Zen",
            &["opencode", "opencode-go", "opencode-zen", "zen"],
            ProbeStrategy::OpencodeZen,
        ),
        b("nvidia", "NVIDIA NIM", &["nvidia"], ProbeStrategy::None),
        b(
            "huggingface",
            "Hugging Face",
            &["huggingface", "hf"],
            ProbeStrategy::None,
        ),
    ]
}

fn providers_json_path() -> Option<PathBuf> {
    let home = home_dir()?;
    Some(home.join(".marionette").join("providers.json"))
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct UserProvidersFile {
    version: u32,
    #[serde(default)]
    providers: std::collections::BTreeMap<String, UserProviderEntry>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct UserProviderEntry {
    label: String,
    #[serde(default)]
    key_aliases: Vec<String>,
    #[serde(default = "default_probe_none")]
    probe_strategy: String,
}

fn default_probe_none() -> String {
    "none".into()
}

fn load_user_catalog() -> Vec<ProviderMeta> {
    let Some(path) = providers_json_path() else {
        return Vec::new();
    };
    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(file) = serde_json::from_str::<UserProvidersFile>(&raw) else {
        return Vec::new();
    };
    file.providers
        .into_iter()
        .map(|(id, e)| ProviderMeta {
            id,
            label: e.label,
            key_aliases: e.key_aliases,
            probe: ProbeStrategy::parse(&e.probe_strategy),
            source: "user",
        })
        .collect()
}

/// Builtin first, then user overrides / adds by id.
fn merged_catalog() -> Vec<ProviderMeta> {
    let mut by_id: std::collections::BTreeMap<String, ProviderMeta> = std::collections::BTreeMap::new();
    for p in builtin_catalog() {
        by_id.insert(p.id.clone(), p);
    }
    for p in load_user_catalog() {
        by_id.insert(p.id.clone(), p);
    }
    by_id.into_values().collect()
}

fn find_meta(provider: &str) -> Option<ProviderMeta> {
    let key = provider.trim().to_ascii_lowercase();
    for p in merged_catalog() {
        if p.id.eq_ignore_ascii_case(&key) {
            return Some(p);
        }
        if p.key_aliases.iter().any(|a| a.eq_ignore_ascii_case(&key)) {
            return Some(p);
        }
    }
    None
}

/// Extract API key for a provider id without ever returning the secret.
/// Best-effort API key from OpenCode `auth.json`. Never logs the secret.
pub fn peek_provider_key(provider: &str) -> Option<String> {
    let auth = load_auth_json().ok()?;
    auth_key_for(&auth, provider).ok().filter(|k| !k.trim().is_empty())
}

fn auth_key_for(auth: &Value, provider: &str) -> Result<String, String> {
    let mut keys: Vec<String> = Vec::new();
    if let Some(meta) = find_meta(provider) {
        keys.push(meta.id.clone());
        for a in meta.key_aliases {
            if !keys.iter().any(|k| k == &a) {
                keys.push(a);
            }
        }
    }
    if !keys.iter().any(|k| k.eq_ignore_ascii_case(provider)) {
        keys.push(provider.to_string());
    }

    let mut tried = Vec::new();
    for k in &keys {
        tried.push(k.clone());
        if let Some(entry) = auth.get(k) {
            if let Some(key) = entry.get("key").and_then(|v| v.as_str()) {
                if !key.is_empty() && !key.starts_with("http://") && !key.starts_with("https://") {
                    return Ok(key.to_string());
                }
            }
            if let Some(access) = entry.get("access").and_then(|v| v.as_str()) {
                if !access.is_empty() {
                    return Ok(access.to_string());
                }
            }
        }
    }
    Err(format!(
        "No usable API key for provider `{provider}` in OpenCode auth (tried: {})",
        tried.join(", ")
    ))
}

/// Extract an OAuth credential for ChatGPT usage probes.
///
/// Do not fall back to `key`: an OpenAI API key cannot authenticate the
/// ChatGPT subscription usage endpoint.
struct ChatgptOAuth {
    access_token: String,
    account_id: Option<String>,
}

fn chatgpt_oauth_for(auth: &Value, provider: &str) -> Result<ChatgptOAuth, String> {
    let mut keys: Vec<String> = Vec::new();
    if let Some(meta) = find_meta(provider) {
        keys.push(meta.id.clone());
        for alias in meta.key_aliases {
            if !keys.iter().any(|key| key == &alias) {
                keys.push(alias);
            }
        }
    }
    if !keys.iter().any(|key| key.eq_ignore_ascii_case(provider)) {
        keys.push(provider.to_string());
    }

    let mut tried = Vec::new();
    for key in &keys {
        tried.push(key.clone());
        let Some(entry) = auth.get(key) else {
            continue;
        };
        for field in ["access", "token"] {
            if let Some(access) = entry.get(field).and_then(|value| value.as_str()) {
                if !access.is_empty() {
                    let account_id = entry
                        .get("accountId")
                        .or_else(|| entry.get("account_id"))
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(str::to_string);
                    return Ok(ChatgptOAuth {
                        access_token: access.to_string(),
                        account_id,
                    });
                }
            }
        }
    }

    Err(format!(
        "No ChatGPT OAuth access token for provider `{provider}` in OpenCode auth (tried: {})",
        tried.join(", ")
    ))
}

/// Parse OpenCode-style `provider/model` ids (also bare model → unknown provider).
pub fn split_model_id(model_id: &str) -> (String, Option<String>) {
    let trimmed = model_id.trim();
    if trimmed.is_empty() {
        return ("unknown".into(), None);
    }
    if let Some((p, m)) = trimmed.split_once('/') {
        return (p.trim().to_string(), Some(m.trim().to_string()));
    }
    // Heuristics for bare ids from some agents
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("deepseek") {
        return ("deepseek".into(), Some(trimmed.to_string()));
    }
    if lower.contains("gpt") || lower.contains("claude") {
        return ("openrouter".into(), Some(trimmed.to_string()));
    }
    ("unknown".into(), Some(trimmed.to_string()))
}

fn provider_label(provider: &str) -> String {
    if let Some(meta) = find_meta(provider) {
        // Usage panel historically used "DeepSeek API" for deepseek.
        if meta.id == "deepseek" {
            return "DeepSeek API".into();
        }
        return meta.label;
    }
    provider.to_string()
}

fn http_get_json_with_account(
    url: &str,
    bearer: &str,
    account_id: Option<&str>,
) -> Result<Value, String> {
    let extra: Vec<(&str, &str)> = account_id
        .map(|id| vec![("ChatGPT-Account-Id", id)])
        .unwrap_or_default();
    http_get_json_headers(url, &format!("Bearer {bearer}"), &extra)
}

fn http_get_json(url: &str, bearer: &str) -> Result<Value, String> {
    http_get_json_with_account(url, bearer, None)
}

/// Core GET-JSON with full control over the `Authorization` header value and
/// extra headers. `authorization` is set verbatim — Bearer is the caller's
/// choice (GLM's monitor endpoint wants the bare key, see `probe_glm`).
fn http_get_json_headers(
    url: &str,
    authorization: &str,
    extra_headers: &[(&str, &str)],
) -> Result<Value, String> {
    let mut request = crate::http_client::agent()?
        .get(url)
        .set("Authorization", authorization)
        .set("Accept", "application/json")
        .set("User-Agent", "Marionette/0.1 (provider-usage)")
        .timeout(std::time::Duration::from_secs(15));
    for (name, value) in extra_headers {
        request = request.set(name, value);
    }
    let resp = match request.call() {
        Ok(response) => response,
        // ureq returns non-2xx responses here rather than from `Response::status`.
        // Keep only the status: provider bodies can include account diagnostics.
        Err(ureq::Error::Status(status, _)) => return Err(format!("HTTP {status} from {url}")),
        Err(error) => return Err(format!("HTTP request to {url}: {error}")),
    };
    let text = resp
        .into_string()
        .map_err(|e| format!("Read body {url}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("JSON from {url}: {e}"))
}

fn money(amount: f64, currency: &str) -> String {
    match currency.to_ascii_uppercase().as_str() {
        "CNY" | "RMB" => format!("¥{amount:.2}"),
        "USD" | "$" => format!("${amount:.2}"),
        other => format!("{amount:.4} {other}"),
    }
}

fn value_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<f64>().ok()))
}

/// Format ChatGPT's Unix reset timestamp as a short relative hint.
fn reset_hint(value: Option<&Value>) -> Option<String> {
    let raw = value.and_then(value_f64)?;
    reset_hint_secs(raw)
}

/// Same hint from raw Unix seconds (or milliseconds — auto-detected).
fn reset_hint_secs(raw: f64) -> Option<String> {
    let reset_seconds = if raw >= 1_000_000_000_000.0 {
        raw / 1000.0
    } else {
        raw
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as f64)?;
    let remaining = reset_seconds - now;
    if remaining <= 0.0 {
        return Some("resets soon".into());
    }
    if remaining < 3600.0 {
        return Some(format!(
            "resets in {}m",
            ((remaining / 60.0).round() as u64).max(1)
        ));
    }
    if remaining < 172_800.0 {
        return Some(format!("resets in {}h", (remaining / 3600.0).round() as u64));
    }
    Some(format!("resets in {}d", (remaining / 86_400.0).round() as u64))
}

/// Strict-UTC ISO-8601 (`YYYY-MM-DDTHH:MM:SS[.fff][Z]`) → Unix seconds.
///
/// Dependency-free (no chrono in this crate). Only the shape OpenCode's usage
/// endpoint returns for `resetsAt` is accepted; `+08:00`-style offsets are
/// rejected rather than guessed.
fn parse_iso8601_utc(text: &str) -> Option<f64> {
    let text = text.trim();
    let bytes = text.as_bytes();
    if bytes.len() < 19 {
        return None;
    }
    let num = |from: usize, len: usize| -> Option<i64> {
        let slice = text.get(from..from + len)?;
        if !slice.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        slice.parse::<i64>().ok()
    };
    let year = num(0, 4)?;
    let month = num(5, 2)?;
    let day = num(8, 2)?;
    // 'T' is the spec separator; tolerate a space for hand-written fixtures.
    if bytes[10] != b'T' && bytes[10] != b' ' {
        return None;
    }
    if bytes[4] != b'-' || bytes[7] != b'-' || bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let hour = num(11, 2)?;
    let minute = num(14, 2)?;
    let second = num(17, 2)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..=23).contains(&hour)
        || !(0..=59).contains(&minute)
        || !(0..=59).contains(&second)
    {
        return None;
    }
    // Days-from-civil (Howard Hinnant): proleptic Gregorian, valid for all
    // dates the endpoint can return.
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = (days * 86_400 + hour * 3_600 + minute * 60 + second) as f64;

    let mut idx = 19;
    let mut frac = 0.0f64;
    if bytes.get(idx) == Some(&b'.') {
        let start = idx + 1;
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if end == start {
            return None;
        }
        frac = format!("0.{}", &text[start..end]).parse::<f64>().ok()?;
        idx = end;
    }
    match bytes.get(idx) {
        // Bare local time is treated as UTC; anything with an offset is refused.
        None => Some(secs + frac),
        Some(&b'Z') | Some(&b'z') if idx + 1 == bytes.len() => Some(secs + frac),
        _ => None,
    }
}

fn openai_window_identity(window: &Value) -> (String, String) {
    let seconds = window
        .get("limit_window_seconds")
        .and_then(value_f64)
        .filter(|seconds| *seconds > 0.0)
        .map(|seconds| seconds.round() as u64);
    match seconds {
        Some(18_000) => ("five-hour".into(), "5-hour limit".into()),
        Some(604_800) => ("weekly".into(), "Weekly limit".into()),
        Some(seconds) if seconds % 86_400 == 0 => {
            let days = seconds / 86_400;
            (format!("window-{seconds}"), format!("{days}-day limit"))
        }
        Some(seconds) if seconds % 3_600 == 0 => {
            let hours = seconds / 3_600;
            (format!("window-{seconds}"), format!("{hours}-hour limit"))
        }
        Some(seconds) => (format!("window-{seconds}"), format!("{seconds}-second limit")),
        None => ("rate-limit".into(), "Rate limit".into()),
    }
}

fn parse_openai_window(window: &Value) -> Option<ProviderUsageWindow> {
    let percentage = window
        .get("used_percent")
        .and_then(value_f64)
        .map(|value| value.clamp(0.0, 100.0));
    let detail = reset_hint(window.get("reset_at"));
    if percentage.is_none() && detail.is_none() {
        return None;
    }
    let (id, label) = openai_window_identity(window);
    Some(ProviderUsageWindow {
        id,
        label,
        percentage,
        detail,
        kind: "rate_limit".into(),
    })
}

/// Parse the private ChatGPT subscription usage response used by OpenChamber.
///
/// Live shape:
/// `{ rate_limit: { primary_window: {...}, secondary_window: {...} } }`
fn parse_openai_usage(json: &Value) -> Vec<ProviderUsageWindow> {
    let rate_limit = json.get("rate_limit").unwrap_or(json);
    let mut windows = Vec::new();
    if let Some(primary) = rate_limit.get("primary_window") {
        if let Some(window) = parse_openai_window(primary) {
            windows.push(window);
        }
    }
    if let Some(secondary) = rate_limit.get("secondary_window") {
        if let Some(window) = parse_openai_window(secondary) {
            windows.push(window);
        }
    }
    windows
}

fn probe_openai_chatgpt(
    access_token: &str,
    account_id: Option<&str>,
    model: Option<&str>,
) -> Result<ProviderUsageSnapshot, String> {
    let json = match http_get_json_with_account(
        "https://chatgpt.com/backend-api/wham/usage",
        access_token,
        account_id,
    ) {
        Ok(json) => json,
        Err(error) if error.contains("HTTP 401") => {
            return Err("ChatGPT OAuth expired; run `opencode auth login` again".into());
        }
        Err(error) if error.contains("HTTP 429") => {
            return Err("ChatGPT usage is rate limited; try again later".into());
        }
        Err(error) => return Err(error),
    };
    let windows = parse_openai_usage(&json);
    if windows.is_empty() {
        return Err("ChatGPT usage response contained no rate-limit windows".into());
    }
    Ok(ProviderUsageSnapshot {
        provider: "openai".into(),
        provider_label: "ChatGPT".into(),
        model: model.map(str::to_string),
        model_label: model.map(str::to_string),
        windows,
        note: Some("Live from ChatGPT /backend-api/wham/usage".into()),
        refreshed_at: now_iso(),
        source: "chatgpt wham/usage".into(),
        ok: true,
    })
}

fn probe_deepseek(key: &str, model: Option<&str>) -> Result<ProviderUsageSnapshot, String> {
    let json = http_get_json("https://api.deepseek.com/user/balance", key)?;
    let available = json
        .get("is_available")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut windows = Vec::new();
    if let Some(arr) = json.get("balance_infos").and_then(|v| v.as_array()) {
        for info in arr {
            let currency = info
                .get("currency")
                .and_then(|v| v.as_str())
                .unwrap_or("CNY");
            let total = info
                .get("total_balance")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| info.get("total_balance").and_then(|v| v.as_f64()))
                .unwrap_or(0.0);
            let granted = info
                .get("granted_balance")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            let topped = info
                .get("topped_up_balance")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.0);
            windows.push(ProviderUsageWindow {
                id: format!("balance-{}", currency.to_ascii_lowercase()),
                label: format!("Balance ({currency})"),
                // No utilization % for prepaid balance — primary UI reads the
                // leading amount from `detail` (see formatPrimary).
                percentage: None,
                detail: Some(format!(
                    "{} · topped-up {} · grant {}",
                    money(total, currency),
                    money(topped, currency),
                    money(granted, currency)
                )),
                kind: "provider".into(),
            });
        }
    }
    windows.push(ProviderUsageWindow {
        id: "api-available".into(),
        label: "API status".into(),
        percentage: if available { Some(0.0) } else { Some(100.0) },
        detail: Some(if available {
            "Balance sufficient for calls".into()
        } else {
            "Insufficient balance".into()
        }),
        kind: "provider".into(),
    });
    Ok(ProviderUsageSnapshot {
        provider: "deepseek".into(),
        provider_label: provider_label("deepseek"),
        model: model.map(str::to_string),
        model_label: model.map(str::to_string),
        windows,
        note: Some("Live from DeepSeek GET /user/balance".into()),
        refreshed_at: now_iso(),
        source: "deepseek /user/balance".into(),
        ok: true,
    })
}

fn probe_openrouter(key: &str, model: Option<&str>) -> Result<ProviderUsageSnapshot, String> {
    let credits = http_get_json("https://openrouter.ai/api/v1/credits", key).ok();
    let key_info = http_get_json("https://openrouter.ai/api/v1/key", key).ok();

    let mut windows = Vec::new();
    let mut total_credits: Option<f64> = None;
    let mut total_usage: Option<f64> = None;

    if let Some(c) = &credits {
        let data = c.get("data").unwrap_or(c);
        total_credits = data.get("total_credits").and_then(|v| v.as_f64());
        total_usage = data.get("total_usage").and_then(|v| v.as_f64());
    }
    if let Some(k) = &key_info {
        let data = k.get("data").unwrap_or(k);
        if total_usage.is_none() {
            total_usage = data.get("usage").and_then(|v| v.as_f64());
        }
        if let Some(limit) = data.get("limit").and_then(|v| v.as_f64()) {
            let remaining = data.get("limit_remaining").and_then(|v| v.as_f64());
            windows.push(ProviderUsageWindow {
                id: "key-limit".into(),
                label: "Key limit".into(),
                percentage: remaining.map(|r| {
                    if limit > 0.0 {
                        ((limit - r) / limit * 100.0).clamp(0.0, 100.0)
                    } else {
                        0.0
                    }
                }),
                detail: Some(match remaining {
                    Some(r) => format!("${r:.2} left of ${limit:.2}"),
                    None => format!("limit ${limit:.2}"),
                }),
                kind: "provider".into(),
            });
        }
        if let Some(daily) = data.get("usage_daily").and_then(|v| v.as_f64()) {
            windows.push(ProviderUsageWindow {
                id: "usage-daily".into(),
                label: "Used today".into(),
                percentage: None,
                detail: Some(money(daily, "USD")),
                kind: "provider".into(),
            });
        }
    }

    if let (Some(c), Some(u)) = (total_credits, total_usage) {
        let remaining = (c - u).max(0.0);
        let pct = if c > 0.0 {
            Some((u / c * 100.0).clamp(0.0, 100.0))
        } else {
            None
        };
        windows.insert(
            0,
            ProviderUsageWindow {
                id: "remaining".into(),
                label: "Remaining credits".into(),
                percentage: pct,
                detail: Some(format!(
                    "{} left · {} used of {}",
                    money(remaining, "USD"),
                    money(u, "USD"),
                    money(c, "USD")
                )),
                kind: "provider".into(),
            },
        );
    } else if let Some(u) = total_usage {
        windows.push(ProviderUsageWindow {
            id: "usage".into(),
            label: "Lifetime (all-time)".into(),
            percentage: None,
            detail: Some(money(u, "USD")),
            kind: "provider".into(),
        });
    }

    if windows.is_empty() {
        return Err("OpenRouter returned no credit/usage fields".into());
    }

    Ok(ProviderUsageSnapshot {
        provider: "openrouter".into(),
        provider_label: provider_label("openrouter"),
        model: model.map(str::to_string),
        model_label: model.map(str::to_string),
        windows,
        note: Some("Live from OpenRouter /credits + /key".into()),
        refreshed_at: now_iso(),
        source: "openrouter api".into(),
        ok: true,
    })
}

/// Parse Moonshot's balance envelope `{"code":0,"data":{"available_balance":…}}`.
fn parse_kimi_balance(json: &Value) -> Result<f64, String> {
    if let Some(code) = json.get("code").and_then(value_f64) {
        if code != 0.0 {
            let msg = json
                .get("msg")
                .or_else(|| json.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("balance query rejected");
            return Err(format!(
                "Moonshot balance query failed (code {}): {msg}",
                code as i64
            ));
        }
    }
    let data = json
        .get("data")
        .ok_or_else(|| "Moonshot balance response missing `data`".to_string())?;
    data.get("available_balance")
        .and_then(value_f64)
        .ok_or_else(|| "Moonshot balance response missing `available_balance`".to_string())
}

/// Kimi (Moonshot): prepaid balance, presented like DeepSeek's balance window.
/// Endpoint: `GET https://api.moonshot.ai/v1/users/me/balance` (magpie-mined,
/// docs/cross-agent-bridge.md 附录 B2「配额端点补缺」).
fn probe_kimi(key: &str, model: Option<&str>) -> Result<ProviderUsageSnapshot, String> {
    let json = http_get_json("https://api.moonshot.ai/v1/users/me/balance", key)?;
    let available = parse_kimi_balance(&json)?;
    // Moonshot bills in CNY; mirror probe_deepseek's balance window shape.
    let windows = vec![ProviderUsageWindow {
        id: "balance-cny".into(),
        label: "Balance (CNY)".into(),
        // No utilization % for prepaid balance — the UI reads the leading
        // amount from `detail` (see formatPrimary).
        percentage: None,
        detail: Some(money(available, "CNY")),
        kind: "provider".into(),
    }];
    Ok(ProviderUsageSnapshot {
        provider: "kimi".into(),
        provider_label: provider_label("kimi"),
        model: model.map(str::to_string),
        model_label: model.map(str::to_string),
        windows,
        note: Some("Live from Moonshot GET /v1/users/me/balance".into()),
        refreshed_at: now_iso(),
        source: "moonshot /v1/users/me/balance".into(),
        ok: true,
    })
}

#[derive(Debug)]
struct GlmUsage {
    level: Option<String>,
    windows: Vec<ProviderUsageWindow>,
}

/// Map GLM's in-envelope error codes to user-facing semantics. All rejections
/// arrive as **HTTP 200** envelopes `{"success":false,"code":…,"msg":…}` —
/// the code carries the status (magpie-mined, 附录 B2):
/// 1000-1005 auth failure · 1113/1302-1399 quota (429-ish) · 1220 forbidden ·
/// 1210-1229/1261/1301 bad request.
fn glm_reject_message(code: i64, msg: &str) -> String {
    let msg = msg.trim();
    match code {
        1000..=1005 => format!("GLM key rejected (code {code}): {msg} — check the Zhipu API key"),
        1113 | 1302..=1399 => format!(
            "GLM quota hit (code {code}): {msg} — top up or upgrade the coding plan"
        ),
        1220 => format!("GLM refused this key/account (code {code}): {msg}"),
        1210..=1229 | 1261 | 1301 => {
            format!("GLM rejected the request (code {code}): {msg}")
        }
        _ => format!("GLM rejected the request (code {code}): {msg}"),
    }
}

/// Parse GLM's quota envelope:
/// `{"success":true,"data":{"level":"pro","limits":[{"type":"TOKENS_LIMIT",
///    "unit":3,"percentage":0.42,"nextResetTime":1798000000000},…]}}`
/// `unit` 3 = hourly window, 6 = weekly window; `percentage` is a 0-1 fraction;
/// `nextResetTime` is epoch milliseconds.
fn parse_glm_usage(json: &Value) -> Result<GlmUsage, String> {
    if json.get("success").and_then(Value::as_bool) == Some(false) {
        let code = json.get("code").and_then(value_f64).unwrap_or(-1.0) as i64;
        let msg = json
            .get("msg")
            .or_else(|| json.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("");
        return Err(glm_reject_message(code, msg));
    }
    let data = json
        .get("data")
        .ok_or_else(|| "GLM quota response missing `data`".to_string())?;
    let level = data
        .get("level")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let mut windows = Vec::new();
    if let Some(limits) = data.get("limits").and_then(Value::as_array) {
        for limit in limits {
            // Only the token-quota rows; unknown limit types carry no
            // window semantics we can honestly map.
            let limit_type = limit.get("type").and_then(Value::as_str);
            if limit_type.is_some_and(|t| t != "TOKENS_LIMIT") {
                continue;
            }
            let unit = limit.get("unit").and_then(value_f64).map(|u| u as i64);
            let (id, label) = match unit {
                Some(3) => ("hourly", "Hourly limit"),
                Some(6) => ("weekly", "Weekly limit"),
                _ => continue,
            };
            let percentage = limit
                .get("percentage")
                .and_then(value_f64)
                .map(|fraction| (fraction * 100.0).clamp(0.0, 100.0));
            let detail = reset_hint(limit.get("nextResetTime"));
            if percentage.is_none() && detail.is_none() {
                continue;
            }
            windows.push(ProviderUsageWindow {
                id: id.into(),
                label: label.into(),
                percentage,
                detail,
                kind: "rate_limit".into(),
            });
        }
    }
    if windows.is_empty() {
        return Err("GLM quota response contained no usable windows".to_string());
    }
    Ok(GlmUsage { level, windows })
}

/// GLM (Zhipu) coding-plan quota. Two endpoint quirks (magpie-mined, 附录 B2):
/// the monitor endpoint wants the **bare** key in `Authorization` — no Bearer
/// prefix — and rejections come back as HTTP 200 envelopes (see
/// `glm_reject_message`), so a 200 body must be inspected before trusting it.
fn probe_glm(key: &str, model: Option<&str>) -> Result<ProviderUsageSnapshot, String> {
    let json = http_get_json_headers(
        "https://open.bigmodel.cn/api/monitor/usage/quota/limit",
        key,
        &[],
    )?;
    let parsed = parse_glm_usage(&json)?;
    let note = match parsed.level.as_deref() {
        Some(level) => format!("Coding plan: {level} · live from bigmodel quota/limit"),
        None => "Live from bigmodel quota/limit".to_string(),
    };
    Ok(ProviderUsageSnapshot {
        provider: "glm".into(),
        provider_label: provider_label("glm"),
        model: model.map(str::to_string),
        model_label: model.map(str::to_string),
        windows: parsed.windows,
        note: Some(note),
        refreshed_at: now_iso(),
        source: "bigmodel /api/monitor/usage/quota/limit".into(),
        ok: true,
    })
}

/// opencode 网关 2026-09 起校验 OpenCode 客户端会话头：缺 `x-opencode-session`
/// 直接 400。会话 id 的形态是网关校验的 —— `ses_` + 12 位小写 hex + 14 位
/// base62（出处：magpie opencode_client.go 的实测形状，docs/cross-agent-bridge.md
/// 附录 B1「OpenCode 会话头」/ B2「配额端点补缺」；grok_dialect.rs 有同形态的
/// 生成器，但文件所有权不重叠，这里自带一份）。无 rand 依赖：时钟 + 计数器混合。
fn generate_opencode_session_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mixed = (nanos as u64) ^ count.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let hex: String = (0..6)
        .map(|i| format!("{:02x}", (mixed >> (i * 8)) as u8))
        .collect();
    const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let tail: String = (0..7)
        .map(|i| {
            let byte = (mixed >> (i * 4)) as u8 ^ (nanos >> (i * 11)) as u8;
            BASE62[(byte as usize) % BASE62.len()] as char
        })
        .collect();
    format!("ses_{hex}{tail}{tail}")
}

/// Process-fixed session id for usage probes — mirrors the opencode client,
/// which keeps one session id per process run.
fn opencode_usage_session_id() -> String {
    static SESSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SESSION
        .get_or_init(generate_opencode_session_id)
        .clone()
}

/// Turn a raw HTTP error from the Go usage endpoint into a friendly message.
/// 403 means the key is not the subscription owner ("Go belongs to the member
/// who subscribed" — magpie-mined, 附录 B2).
fn opencode_usage_error(error: &str) -> String {
    if error.contains("HTTP 403") {
        "OpenCode refused this key for Go usage: \"Go belongs to the member who \
         subscribed\" — use the API key of the subscribing account"
            .into()
    } else {
        error.to_string()
    }
}

/// Parse `{"usage":{"rolling":{"percent":42,"resetsAt":"…"},"weekly":{…},
/// "monthly":{…}}}` — `rolling` is the 5-hour window (hourly-window style),
/// `weekly`/`monthly` the longer ceilings. `percent` is 0-100, `resetsAt` an
/// ISO-8601 string.
fn parse_opencode_usage(usage: &Value) -> Vec<ProviderUsageWindow> {
    let mut windows = Vec::new();
    let specs: &[(&str, &str, &str)] = &[
        ("rolling", "five-hour", "5-hour limit"),
        ("weekly", "weekly", "Weekly limit"),
        ("monthly", "monthly", "Monthly limit"),
    ];
    for (field, id, label) in specs {
        let Some(entry) = usage.get(field) else {
            continue;
        };
        let percentage = entry
            .get("percent")
            .and_then(value_f64)
            .map(|value| value.clamp(0.0, 100.0));
        let detail = entry.get("resetsAt").and_then(Value::as_str).map(|raw| {
            match parse_iso8601_utc(raw).and_then(reset_hint_secs) {
                Some(hint) => hint,
                // Unparseable shape: still show something instead of nothing.
                None if raw.trim().is_empty() => "resets soon".into(),
                None => format!("resets at {}", raw.trim()),
            }
        });
        if percentage.is_none() && detail.is_none() {
            continue;
        }
        windows.push(ProviderUsageWindow {
            id: (*id).into(),
            label: (*label).into(),
            percentage,
            detail,
            kind: "rate_limit".into(),
        });
    }
    windows
}

/// OpenCode Go live usage: `GET https://opencode.ai/zen/go/v1/usage` with
/// Bearer auth **plus** the `x-opencode-session` header (see
/// `generate_opencode_session_id`). Replaces the old "no public usage API"
/// credential-check stub.
fn probe_opencode_go_usage(
    key: &str,
    model: Option<&str>,
) -> Result<ProviderUsageSnapshot, String> {
    let session = opencode_usage_session_id();
    let json = match http_get_json_headers(
        "https://opencode.ai/zen/go/v1/usage",
        &format!("Bearer {key}"),
        &[("x-opencode-session", session.as_str())],
    ) {
        Ok(json) => json,
        Err(error) => return Err(opencode_usage_error(&error)),
    };
    let usage = json.get("usage").unwrap_or(&json);
    let windows = parse_opencode_usage(usage);
    if windows.is_empty() {
        return Err("OpenCode Go usage response contained no windows".into());
    }
    Ok(ProviderUsageSnapshot {
        provider: "opencode-go".into(),
        provider_label: provider_label("opencode-go"),
        model: model.map(str::to_string),
        model_label: model.map(str::to_string),
        windows,
        note: Some("Live from OpenCode Go GET /zen/go/v1/usage".into()),
        refreshed_at: now_iso(),
        source: "opencode-go /zen/go/v1/usage".into(),
        ok: true,
    })
}

/// OpenCode Go: live usage via [`probe_opencode_go_usage`].
/// OpenCode Zen: no public usage API yet — surface a credential check so the
/// panel is still useful.
fn probe_opencode_plan(
    provider: &str,
    key: &str,
    model: Option<&str>,
) -> Result<ProviderUsageSnapshot, String> {
    let is_go = matches!(provider, "opencode-go" | "opencode_go" | "go");
    if is_go {
        return probe_opencode_go_usage(key, model);
    }

    // Auth works if models list succeeds.
    let auth_ok = http_get_json("https://opencode.ai/zen/v1/models", key).is_ok();

    let windows = vec![
        ProviderUsageWindow {
            id: "credential".into(),
            label: "API key".into(),
            percentage: if auth_ok { Some(0.0) } else { Some(100.0) },
            detail: Some(if auth_ok {
                "Accepted by models endpoint".into()
            } else {
                "Key rejected or network error".into()
            }),
            kind: "provider".into(),
        },
        ProviderUsageWindow {
            id: "zen-balance".into(),
            label: "Zen balance".into(),
            percentage: None,
            detail: Some("No public balance API — check opencode.ai/auth".into()),
            kind: "provider".into(),
        },
    ];

    Ok(ProviderUsageSnapshot {
        provider: "opencode".into(),
        provider_label: provider_label("opencode"),
        model: model.map(str::to_string),
        model_label: model.map(str::to_string),
        windows,
        note: Some(
            "OpenCode Zen balance API is not public yet; open the console for remaining credits."
                .into(),
        ),
        refreshed_at: now_iso(),
        source: "opencode zen models".into(),
        ok: auth_ok,
    })
}

fn unsupported(provider: &str, model: Option<&str>, reason: &str) -> ProviderUsageSnapshot {
    ProviderUsageSnapshot {
        provider: provider.to_string(),
        provider_label: provider_label(provider),
        model: model.map(str::to_string),
        model_label: model.map(str::to_string),
        windows: vec![ProviderUsageWindow {
            id: "unsupported".into(),
            label: "Provider balance".into(),
            percentage: None,
            detail: Some(reason.to_string()),
            kind: "provider".into(),
        }],
        note: Some(reason.to_string()),
        refreshed_at: now_iso(),
        source: "none".into(),
        ok: false,
    }
}

/// Probe balance/usage for the active model id (`provider/model`).
pub fn probe_provider_usage(model_id: Option<String>) -> ProviderUsageSnapshot {
    let model_id = model_id.unwrap_or_default();
    let (provider, model) = if model_id.trim().is_empty() {
        ("unknown".to_string(), None)
    } else {
        split_model_id(&model_id)
    };
    let model_ref = model.as_deref();

    let auth = match load_auth_json() {
        Ok(v) => v,
        Err(e) => {
            return unsupported(&provider, model_ref, &e);
        }
    };

    let result = match provider.as_str() {
        "deepseek" => match auth_key_for(&auth, "deepseek") {
            Ok(key) => probe_deepseek(&key, model_ref),
            Err(e) => Err(e),
        },
        "openai" | "codex" | "chatgpt" => match chatgpt_oauth_for(&auth, &provider) {
            Ok(oauth) => probe_openai_chatgpt(
                &oauth.access_token,
                oauth.account_id.as_deref(),
                model_ref,
            ),
            Err(e) => Err(e),
        },
        "openrouter" => match auth_key_for(&auth, "openrouter") {
            Ok(key) => probe_openrouter(&key, model_ref),
            Err(e) => Err(e),
        },
        "kimi" | "moonshot" => match auth_key_for(&auth, "kimi") {
            Ok(key) => probe_kimi(&key, model_ref),
            Err(e) => Err(e),
        },
        "glm" | "zhipu" | "bigmodel" => match auth_key_for(&auth, "glm") {
            Ok(key) => probe_glm(&key, model_ref),
            Err(e) => Err(e),
        },
        "opencode-go" | "opencode_go" | "go" => match auth_key_for(&auth, "opencode-go") {
            Ok(key) => probe_opencode_plan("opencode-go", &key, model_ref),
            Err(e) => Err(e),
        },
        "opencode" | "opencode-zen" | "zen" => match auth_key_for(&auth, "opencode") {
            Ok(key) => probe_opencode_plan("opencode", &key, model_ref),
            Err(e) => Err(e),
        },
        other => {
            // Catalog-driven fallback: a user providers.json entry can map an
            // arbitrary id/alias onto a probe strategy.
            match find_meta(other).map(|meta| meta.probe) {
                Some(ProbeStrategy::Kimi) => match auth_key_for(&auth, other) {
                    Ok(key) => probe_kimi(&key, model_ref),
                    Err(e) => Err(e),
                },
                Some(ProbeStrategy::Glm) => match auth_key_for(&auth, other) {
                    Ok(key) => probe_glm(&key, model_ref),
                    Err(e) => Err(e),
                },
                _ => Err(format!(
                    "No balance probe for `{other}` yet (supported: deepseek, openai, openrouter, opencode-go, opencode, kimi, glm)"
                )),
            }
        }
    };

    match result {
        Ok(mut snap) => {
            if snap.model.is_none() {
                snap.model = model;
            }
            snap
        }
        Err(e) => unsupported(&provider, model_ref, &e),
    }
}

/// Like `opencode_auth_path()` but returns the path to *write* to:
/// uses the first existing location, or falls back to the default.
pub fn opencode_auth_path_write() -> PathBuf {
    if let Some(existing) = opencode_auth_path() {
        return existing;
    }
    let home = home_dir().unwrap_or_else(|| PathBuf::from("."));
    let default = home
        .join(".local")
        .join("share")
        .join("opencode")
        .join("auth.json");
    if let Some(parent) = default.parent() {
        let _ = fs::create_dir_all(parent);
    }
    default
}

/// How a provider's `auth.json` entry stores its credential.
///
/// An OAuth entry holds a refresh token that this app cannot re-create — once
/// it is replaced by `{ "key": … }` or removed, the only way back is
/// `opencode auth login`. Both the write and the delete path therefore refuse
/// to touch an OAuth entry unless the caller explicitly confirms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthKind {
    Api,
    #[serde(rename = "oauth")]
    OAuth,
    Unknown,
}

/// Classify one `auth.json` entry. OAuth wins over `key` when both are present,
/// because losing the refresh token is the irreversible half.
fn classify_entry(entry: &Value) -> AuthKind {
    let non_empty = |field: &str| {
        entry
            .get(field)
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty())
    };
    if entry.get("type").and_then(|v| v.as_str()) == Some("oauth")
        || non_empty("refresh")
        || non_empty("access")
    {
        return AuthKind::OAuth;
    }
    if non_empty("key") {
        return AuthKind::Api;
    }
    AuthKind::Unknown
}

/// Extract the API key of a single `auth.json` entry — strict variant for the
/// model bridge's reference credentials (`model_bridge::peek_reference_secret`).
///
/// OAuth / hybrid entries are rejected outright: an L1 direct connection needs a
/// real API key, and an OAuth access token is not one. The lenient `access`
/// fallback inside [`auth_key_for`] is a usage-probe privilege — never reuse it
/// for injection. Key validity rules match `auth_key_for`: non-empty, and not
/// an http(s) placeholder.
pub fn strict_api_key_of(entry: &Value) -> Result<String, String> {
    if classify_entry(entry) != AuthKind::Api {
        return Err("not an api-type entry (OAuth login state cannot direct-connect)".to_string());
    }
    let key = entry.get("key").and_then(Value::as_str).unwrap_or("");
    if key.is_empty() || key.starts_with("http://") || key.starts_with("https://") {
        return Err("api entry has no usable key".to_string());
    }
    Ok(key.to_string())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub provider: String,
    pub label: String,
    pub has_key: bool,
    /// Lets the UI warn before overwriting or deleting an OAuth login.
    pub auth_kind: AuthKind,
    /// Whether auth.json has an entry for this provider (or an alias).
    pub configured: bool,
    /// Where this row came from.
    pub source: String,
    /// Balance probe strategy owned by Rust (`none` = honest "unsupported").
    pub probe_strategy: String,
}

/// Provider id → human label (catalog first).
fn provider_display_name(id: &str) -> String {
    find_meta(id).map(|m| m.label).unwrap_or_else(|| id.to_string())
}

/// Read `auth.json` for a mutation.
///
/// Deliberately stricter than [`load_auth_json`]: unparseable content is an
/// error instead of an empty object. Treating a half-written or hand-edited file
/// as `{}` means the very next write persists only the key being added and drops
/// every other provider's credential — a silent wipe of the whole auth store.
fn read_auth_for_write(path: &std::path::Path) -> Result<serde_json::Map<String, Value>, String> {
    if !path.is_file() {
        return Ok(serde_json::Map::new());
    }
    let text = fs::read_to_string(path).map_err(|e| format!("读取 auth.json 失败: {e}"))?;
    if text.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }
    match serde_json::from_str::<Value>(&text) {
        Ok(Value::Object(obj)) => Ok(obj),
        Ok(_) => Err("auth.json 顶层不是 JSON 对象 — 已中止，以免覆盖其中的凭证".to_string()),
        Err(error) => Err(format!(
            "auth.json 解析失败（{error}）— 已中止，以免覆盖其他服务商的凭证"
        )),
    }
}

/// Temp sibling for the atomic rename, unique per process *and* per call.
///
/// A single fixed `auth.json.tmp` lets two concurrent writes (say a save racing
/// a delete) clobber each other's staging file and rename a half-merged result
/// into place.
fn temp_sibling(path: &std::path::Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("auth.json");
    path.with_file_name(format!("{name}.{}.{seq}.tmp", std::process::id()))
}

fn write_auth_atomic(
    path: &std::path::Path,
    auth: &serde_json::Map<String, Value>,
) -> Result<(), String> {
    let json_str = serde_json::to_string_pretty(&Value::Object(auth.clone()))
        .map_err(|e| format!("序列化 auth.json 失败: {e}"))?;
    let temp_path = temp_sibling(path);
    fs::write(&temp_path, &json_str).map_err(|e| format!("写入临时文件失败: {e}"))?;
    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!("重命名临时文件失败: {error}"));
    }
    Ok(())
}

/// OpenCode's Auth schema (ApiAuth) requires `type: "api"` + `key`.
/// Entries with only `{ "key": "…" }` are **silently dropped** on load
/// (`Schema.decodeUnknownOption` filters them out) — so DeepSeek and friends
/// never appear in the TUI or ACP model list even though the file has a key.
/// See anomalyco/opencode `packages/opencode/src/auth/index.ts`.
fn api_auth_entry(key: &str) -> Value {
    serde_json::json!({ "type": "api", "key": key })
}

/// Upgrade legacy key-only entries so OpenCode accepts them.
/// Returns true when the map was mutated (caller should rewrite the file).
fn repair_auth_api_types(auth: &mut serde_json::Map<String, Value>) -> bool {
    let mut changed = false;
    for (_id, entry) in auth.iter_mut() {
        let Some(obj) = entry.as_object_mut() else {
            continue;
        };
        let has_key = obj
            .get("key")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty());
        if !has_key {
            continue;
        }
        // Never rewrite oauth (refresh/access) or already-typed api/wellknown.
        let type_str = obj.get("type").and_then(|v| v.as_str());
        let is_oauth = type_str == Some("oauth")
            || obj
                .get("refresh")
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty());
        if is_oauth {
            continue;
        }
        match type_str {
            Some("api") | Some("wellknown") => {}
            _ => {
                obj.insert("type".into(), Value::String("api".into()));
                changed = true;
            }
        }
    }
    changed
}

/// Load auth.json, inject missing `type: "api"`, and persist if anything changed.
/// Safe no-op when the file is missing or already valid.
pub fn repair_opencode_auth_file() -> Result<bool, String> {
    let Some(path) = opencode_auth_path() else {
        return Ok(false);
    };
    let mut auth = read_auth_for_write(&path)?;
    if !repair_auth_api_types(&mut auth) {
        return Ok(false);
    }
    write_auth_atomic(&path, &auth)?;
    Ok(true)
}

/// Write a provider API key to the given auth file path.
/// If `path` is `None`, uses the default OpenCode auth.json location.
///
/// `force` is required to overwrite an OAuth login — see [`AuthKind`].
pub fn write_provider_key_at(
    provider: &str,
    key: &str,
    path: Option<&std::path::Path>,
    force: bool,
) -> Result<(), String> {
    let path: PathBuf = match path {
        Some(p) => p.to_path_buf(),
        None => opencode_auth_path_write(),
    };

    let mut auth = read_auth_for_write(&path)?;

    if let Some(existing) = auth.get(provider) {
        if classify_entry(existing) == AuthKind::OAuth && !force {
            return Err(format!(
                "`{provider}` 目前是 OAuth 登录。写入 API Key 会覆盖登录凭证，refresh token 无法从本应用恢复。"
            ));
        }
    }

    // OpenCode requires the `type` discriminator — bare `{ key }` is ignored.
    auth.insert(provider.to_string(), api_auth_entry(key));
    // Heal any other legacy key-only providers while we have the file open.
    let _ = repair_auth_api_types(&mut auth);
    write_auth_atomic(&path, &auth)
}

/// Write a provider API key to the default OpenCode auth.json path.
pub fn write_provider_key(provider: &str, key: &str, force: bool) -> Result<(), String> {
    write_provider_key_at(provider, key, None, force)
}

/// Catalog ∪ auth.json keys — for the OpenCode provider Key dialog.
///
/// Does not expose secrets. New installs get the builtin catalog even when
/// `auth.json` / `providers.json` are empty.
pub fn list_providers() -> Result<Vec<ProviderInfo>, String> {
    // Opportunistically heal legacy key-only entries so OpenCode and this
    // dialog agree on what's configured (type: "api" required by OpenCode).
    let _ = repair_opencode_auth_file();
    let auth = load_auth_json().unwrap_or(Value::Object(Default::default()));
    let auth_obj = auth.as_object().cloned().unwrap_or_default();

    let mut out: Vec<ProviderInfo> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for meta in merged_catalog() {
        // Prefer matching an auth entry by canonical id or any alias.
        let mut auth_key: Option<String> = None;
        let mut auth_kind = AuthKind::Unknown;
        let candidates: Vec<String> = std::iter::once(meta.id.clone())
            .chain(meta.key_aliases.iter().cloned())
            .collect();
        for c in &candidates {
            if let Some(value) = auth_obj.get(c) {
                auth_key = Some(c.clone());
                auth_kind = classify_entry(value);
                break;
            }
        }
        let configured = auth_key.is_some();
        let has_key = configured && auth_kind != AuthKind::Unknown;
        seen.insert(meta.id.clone());
        out.push(ProviderInfo {
            provider: meta.id,
            label: meta.label,
            has_key,
            auth_kind,
            configured,
            source: meta.source.to_string(),
            probe_strategy: meta.probe.as_str().to_string(),
        });
    }

    // Auth-only keys (hand-edited foobar) still show up.
    for (key, value) in &auth_obj {
        if seen.contains(key) {
            continue;
        }
        // Skip if this key is only an alias of something we already listed.
        if find_meta(key).is_some_and(|m| seen.contains(&m.id)) {
            continue;
        }
        let auth_kind = classify_entry(value);
        out.push(ProviderInfo {
            provider: key.clone(),
            label: provider_display_name(key),
            has_key: auth_kind != AuthKind::Unknown,
            auth_kind,
            configured: true,
            source: "auth".into(),
            probe_strategy: ProbeStrategy::None.as_str().to_string(),
        });
    }

    out.sort_by(|a, b| a.label.to_ascii_lowercase().cmp(&b.label.to_ascii_lowercase()));
    Ok(out)
}

/// Upsert user catalog entry in `~/.marionette/providers.json` (does not touch auth.json).
pub fn upsert_provider_meta(
    id: String,
    label: String,
    key_aliases: Vec<String>,
    probe_strategy: String,
) -> Result<(), String> {
    let id = id.trim().to_ascii_lowercase();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Err("Provider id 只能用字母、数字、连字符".into());
    }
    let path = providers_json_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Create .marionette failed: {e}"))?;
    }
    let mut file = if path.is_file() {
        let raw = fs::read_to_string(&path).map_err(|e| format!("Read providers.json: {e}"))?;
        serde_json::from_str::<UserProvidersFile>(&raw).unwrap_or(UserProvidersFile {
            version: 1,
            providers: Default::default(),
        })
    } else {
        UserProvidersFile {
            version: 1,
            providers: Default::default(),
        }
    };
    let probe = ProbeStrategy::parse(&probe_strategy);
    file.providers.insert(
        id,
        UserProviderEntry {
            label: if label.trim().is_empty() {
                "Custom".into()
            } else {
                label.trim().to_string()
            },
            key_aliases,
            probe_strategy: probe.as_str().to_string(),
        },
    );
    let body = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, format!("{body}\n")).map_err(|e| format!("Write providers.json: {e}"))?;
    fs::rename(&temp, &path).map_err(|e| format!("Rename providers.json: {e}"))?;
    Ok(())
}

/// Remove user catalog meta only (auth.json key is left alone).
pub fn delete_provider_meta(id: String) -> Result<(), String> {
    let id = id.trim().to_ascii_lowercase();
    let path = providers_json_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    if !path.is_file() {
        return Ok(());
    }
    let raw = fs::read_to_string(&path).map_err(|e| format!("Read providers.json: {e}"))?;
    let mut file: UserProvidersFile = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    file.providers.remove(&id);
    let body = serde_json::to_string_pretty(&file).map_err(|e| e.to_string())?;
    fs::write(&path, format!("{body}\n")).map_err(|e| format!("Write providers.json: {e}"))?;
    Ok(())
}

/// Delete a provider API key from auth.json.
/// If `path` is `None`, uses the default OpenCode auth.json location.
///
/// `force` is required to remove an OAuth login — see [`AuthKind`].
pub fn delete_provider_key_at(
    provider: &str,
    path: Option<&std::path::Path>,
    force: bool,
) -> Result<(), String> {
    let path: PathBuf = match path {
        Some(p) => p.to_path_buf(),
        None => opencode_auth_path_write(),
    };

    // A non-object or unparseable file used to fall through the `if let` below
    // and still get rewritten — reporting success while deleting nothing.
    let mut auth = read_auth_for_write(&path)?;

    let Some(existing) = auth.get(provider) else {
        return Err(format!("Provider `{provider}` not found in auth.json"));
    };
    if classify_entry(existing) == AuthKind::OAuth && !force {
        return Err(format!(
            "`{provider}` 是 OAuth 登录，删除后只能用 `opencode auth login` 重新登录。"
        ));
    }

    auth.remove(provider);
    write_auth_atomic(&path, &auth)
}

/// Delete a provider API key from the default OpenCode auth.json path.
pub fn delete_provider_key(provider: &str, force: bool) -> Result<(), String> {
    delete_provider_key_at(provider, None, force)
}

#[cfg(test)]
mod tests {
    use super::{
        builtin_catalog, chatgpt_oauth_for, generate_opencode_session_id, glm_reject_message,
        money, opencode_usage_error, parse_glm_usage, parse_iso8601_utc, parse_kimi_balance,
        parse_opencode_usage, parse_openai_usage, reset_hint_secs, split_model_id, ProbeStrategy,
    };
    use serde_json::json;

    #[test]
    fn splits_provider_model() {
        let (p, m) = split_model_id("deepseek/deepseek-v4-pro");
        assert_eq!(p, "deepseek");
        assert_eq!(m.as_deref(), Some("deepseek-v4-pro"));
    }

    #[test]
    fn splits_opencode_go() {
        let (p, m) = split_model_id("opencode-go/deepseek-v4-flash");
        assert_eq!(p, "opencode-go");
        assert_eq!(m.as_deref(), Some("deepseek-v4-flash"));
    }

    #[test]
    fn parses_chatgpt_usage_windows() {
        let json = json!({
            "rate_limit": {
                "primary_window": {
                    "used_percent": 23.5,
                    "limit_window_seconds": 18000,
                    "reset_at": 4102444800_i64
                },
                "secondary_window": {
                    "used_percent": 41,
                    "limit_window_seconds": 604800,
                    "reset_at": 4102444800_i64
                }
            }
        });
        let windows = parse_openai_usage(&json);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].id, "five-hour");
        assert_eq!(windows[0].percentage, Some(23.5));
        assert_eq!(windows[0].kind, "rate_limit");
        assert_eq!(windows[1].id, "weekly");
        assert_eq!(windows[1].percentage, Some(41.0));
    }

    #[test]
    fn labels_a_weekly_primary_window_by_its_duration() {
        let json = json!({
            "rate_limit": {
                "primary_window": {
                    "used_percent": 3,
                    "limit_window_seconds": 604800
                }
            }
        });
        let windows = parse_openai_usage(&json);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].id, "weekly");
        assert_eq!(windows[0].label, "Weekly limit");
    }

    // ─── kimi (moonshot) ────────────────────────────────────────────────────

    #[test]
    fn parses_kimi_balance_envelope() {
        // Shape from magpie source mining (docs/cross-agent-bridge.md 附录 B2).
        let json = json!({
            "code": 0,
            "data": { "available_balance": 49.58 }
        });
        assert_eq!(parse_kimi_balance(&json).unwrap(), 49.58);
        // Balance also arrives as a string on some accounts.
        let as_string = json!({
            "code": 0,
            "data": { "available_balance": "12.5" }
        });
        assert_eq!(parse_kimi_balance(&as_string).unwrap(), 12.5);
        // Presentation follows DeepSeek's balance window style.
        assert_eq!(money(parse_kimi_balance(&json).unwrap(), "CNY"), "¥49.58");

        let rejected = json!({ "code": 1003, "msg": "invalid key" });
        let err = parse_kimi_balance(&rejected).unwrap_err();
        assert!(err.contains("1003"), "unexpected error: {err}");
        assert!(err.contains("invalid key"));

        let no_data = json!({ "code": 0 });
        assert!(parse_kimi_balance(&no_data).is_err());
    }

    #[test]
    fn catalog_maps_kimi_and_glm_aliases() {
        let catalog = builtin_catalog();
        let kimi = catalog.iter().find(|p| p.id == "kimi").unwrap();
        assert_eq!(kimi.probe, ProbeStrategy::Kimi);
        assert!(kimi.key_aliases.iter().any(|a| a == "moonshot"));

        let glm = catalog.iter().find(|p| p.id == "glm").unwrap();
        assert_eq!(glm.probe, ProbeStrategy::Glm);
        assert!(glm.key_aliases.iter().any(|a| a == "zhipu"));
        assert!(glm.key_aliases.iter().any(|a| a == "bigmodel"));

        // zai is a different vendor (Z.AI ≠ open.bigmodel.cn) — its aliases
        // must not claim the GLM coding-plan keys anymore.
        let zai = catalog.iter().find(|p| p.id == "zai").unwrap();
        assert!(!zai.key_aliases.iter().any(|a| a == "glm" || a == "zhipu"));

        assert_eq!(ProbeStrategy::parse("moonshot"), ProbeStrategy::Kimi);
        assert_eq!(ProbeStrategy::parse("Kimi"), ProbeStrategy::Kimi);
        assert_eq!(ProbeStrategy::parse("zhipu"), ProbeStrategy::Glm);
        assert_eq!(ProbeStrategy::parse("bigmodel"), ProbeStrategy::Glm);
        assert_eq!(ProbeStrategy::parse("glm"), ProbeStrategy::Glm);
        assert_eq!(ProbeStrategy::Glm.as_str(), "glm");
        assert_eq!(ProbeStrategy::Kimi.as_str(), "kimi");
    }

    // ─── glm (zhipu coding plan) ────────────────────────────────────────────

    #[test]
    fn parses_glm_quota_windows() {
        let json = json!({
            "success": true,
            "data": {
                "level": "pro",
                "limits": [
                    { "type": "TOKENS_LIMIT", "unit": 3, "percentage": 0.42, "nextResetTime": 1798000000000_i64 },
                    { "type": "TOKENS_LIMIT", "unit": 6, "percentage": 0.71, "nextResetTime": 1798000000000_i64 },
                    { "type": "TOKENS_LIMIT", "unit": 9, "percentage": 0.5 },
                    { "type": "REQUESTS_LIMIT", "unit": 3, "percentage": 0.1 }
                ]
            }
        });
        let parsed = parse_glm_usage(&json).unwrap();
        assert_eq!(parsed.level.as_deref(), Some("pro"));
        assert_eq!(parsed.windows.len(), 2, "unknown units/types must be skipped");
        assert_eq!(parsed.windows[0].id, "hourly");
        assert_eq!(parsed.windows[0].label, "Hourly limit");
        assert_eq!(parsed.windows[0].percentage, Some(42.0));
        assert_eq!(parsed.windows[0].kind, "rate_limit");
        assert!(
            parsed.windows[0].detail.as_deref().unwrap().starts_with("resets"),
            "detail: {:?}",
            parsed.windows[0].detail
        );
        assert_eq!(parsed.windows[1].id, "weekly");
        assert_eq!(parsed.windows[1].percentage, Some(71.0));

        // Millisecond epochs must not be read as seconds ("resets soon").
        // ~68 days out — the days-scale branch regardless of run time.
        let hint = reset_hint_secs(1_798_000_000_000.0).unwrap();
        assert!(hint.starts_with("resets in "), "hint: {hint}");
        assert!(hint.ends_with('d'), "hint: {hint}");
    }

    #[test]
    fn glm_rejection_envelope_is_an_error_not_windows() {
        // GLM rejects inside a HTTP 200 envelope — success:false wins over data.
        let json = json!({
            "success": false,
            "code": 1001,
            "msg": "身份验证失败。token 校验失败",
            "data": { "level": "pro", "limits": [] }
        });
        let err = parse_glm_usage(&json).unwrap_err();
        assert!(err.contains("check the Zhipu API key"), "error: {err}");
        assert!(err.contains("身份验证失败"), "error: {err}");

        let empty = json!({ "success": true, "data": { "limits": [] } });
        assert!(parse_glm_usage(&empty).is_err());
        let no_data = json!({ "success": true });
        assert!(parse_glm_usage(&no_data).is_err());
    }

    #[test]
    fn glm_reject_codes_map_to_semantics() {
        // 1113 / 1302-1399 → quota (suggest recharge/plan).
        assert!(glm_reject_message(1113, "x").contains("top up"));
        assert!(glm_reject_message(1302, "x").contains("top up"));
        assert!(glm_reject_message(1399, "x").contains("top up"));
        // 1220 → forbidden.
        assert!(glm_reject_message(1220, "x").contains("refused"));
        // 1210-1229 (minus 1220) / 1261 / 1301 → bad request.
        assert!(glm_reject_message(1210, "x").contains("rejected the request"));
        assert!(glm_reject_message(1229, "x").contains("rejected the request"));
        assert!(glm_reject_message(1261, "x").contains("rejected the request"));
        assert!(glm_reject_message(1301, "x").contains("rejected the request"));
        // 1000-1005 → auth.
        assert!(glm_reject_message(1005, "身份验证失败。").contains("key rejected"));
        // Unknown codes still surface the code and message.
        assert!(glm_reject_message(4242, "boom").contains("4242"));
    }

    // ─── opencode-go live usage ─────────────────────────────────────────────

    #[test]
    fn parses_opencode_go_usage_windows() {
        let json = json!({
            "usage": {
                "rolling": { "percent": 42, "resetsAt": "2999-01-01T00:00:00Z" },
                "weekly": { "percent": 71, "resetsAt": "2999-01-08T00:00:00Z" },
                "monthly": { "percent": 8, "resetsAt": "2999-02-01T00:00:00Z" }
            }
        });
        let usage = json.get("usage").unwrap();
        let windows = parse_opencode_usage(usage);
        assert_eq!(windows.len(), 3);
        // rolling maps to the 5-hour window id, like the old plan ceilings.
        assert_eq!(windows[0].id, "five-hour");
        assert_eq!(windows[0].label, "5-hour limit");
        assert_eq!(windows[0].percentage, Some(42.0));
        assert_eq!(windows[0].kind, "rate_limit");
        assert!(
            windows[0].detail.as_deref().unwrap().starts_with("resets in"),
            "detail: {:?}",
            windows[0].detail
        );
        assert_eq!(windows[1].id, "weekly");
        assert_eq!(windows[1].percentage, Some(71.0));
        assert_eq!(windows[2].id, "monthly");
        assert_eq!(windows[2].percentage, Some(8.0));

        // Unparseable reset strings degrade to the raw value, not nothing.
        let odd = json!({
            "usage": { "rolling": { "percent": 5, "resetsAt": "sometime" } }
        });
        let windows = parse_opencode_usage(odd.get("usage").unwrap());
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].detail.as_deref(), Some("resets at sometime"));

        // Empty usage → no windows (probe turns that into an error).
        assert!(parse_opencode_usage(&json!({})).is_empty());
    }

    #[test]
    fn opencode_usage_403_gets_a_friendly_message() {
        let friendly = opencode_usage_error("HTTP 403 from https://opencode.ai/zen/go/v1/usage");
        assert!(
            friendly.contains("Go belongs to the member who subscribed"),
            "message: {friendly}"
        );
        // Everything else passes through untouched.
        assert_eq!(
            opencode_usage_error("HTTP 500 from https://opencode.ai/zen/go/v1/usage"),
            "HTTP 500 from https://opencode.ai/zen/go/v1/usage"
        );
    }

    #[test]
    fn session_id_matches_opencode_client_shape() {
        // `ses_` + 12 lowercase hex + 14 base62 (gateway validates the shape —
        // magpie opencode_client.go, docs appendix B1).
        let id = generate_opencode_session_id();
        assert!(id.starts_with("ses_"), "id = {id}");
        assert_eq!(id.len(), 4 + 12 + 14, "id = {id}");
        let hex = &id[4..16];
        assert!(
            hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "hex part = {hex}"
        );
        const BASE62: &str =
            "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        let tail = &id[16..];
        assert!(
            tail.chars().all(|c| BASE62.contains(c)),
            "tail part = {tail}"
        );
        // Successive ids differ (counter mix), still well-formed.
        let another = generate_opencode_session_id();
        assert_ne!(id, another);
        assert!(another.starts_with("ses_"));
    }

    #[test]
    fn iso8601_utc_parsing() {
        assert_eq!(
            parse_iso8601_utc("1970-01-01T00:00:00Z"),
            Some(0.0)
        );
        assert_eq!(
            parse_iso8601_utc("1970-01-02T00:00:00Z"),
            Some(86_400.0)
        );
        // 2026-10-16T00:00:00Z = day 20742 since epoch.
        assert_eq!(
            parse_iso8601_utc("2026-10-16T00:00:00Z"),
            Some(20_742.0 * 86_400.0)
        );
        assert_eq!(
            parse_iso8601_utc("2026-10-16T00:00:30.5Z"),
            Some(20_742.0 * 86_400.0 + 30.5)
        );
        // Offsets other than Z are refused rather than guessed.
        assert_eq!(parse_iso8601_utc("2026-10-16T00:00:00+08:00"), None);
        // Malformed input.
        assert_eq!(parse_iso8601_utc("not a date"), None);
        assert_eq!(parse_iso8601_utc("2026-13-40T99:99:99Z"), None);
        assert_eq!(parse_iso8601_utc(""), None);
    }

    #[test]
    fn reads_chatgpt_oauth_access_without_using_api_key() {
        let auth = json!({
            "openai": {
                "type": "oauth",
                "access": "oauth-access",
                "refresh": "oauth-refresh",
                "key": "api-key-that-must-not-be-used"
            }
        });
        let oauth = chatgpt_oauth_for(&auth, "openai").unwrap();
        assert_eq!(oauth.access_token, "oauth-access");
        assert_eq!(oauth.account_id, None);

        let with_account = json!({
            "openai": { "access": "oauth-access", "accountId": "acct-123" }
        });
        assert_eq!(
            chatgpt_oauth_for(&with_account, "openai")
                .unwrap()
                .account_id
                .as_deref(),
            Some("acct-123")
        );

        let api_only = json!({ "openai": { "key": "api-key" } });
        assert!(chatgpt_oauth_for(&api_only, "openai").is_err());
    }

    #[test]
    fn write_and_read_provider_key() {
        let dir = std::env::temp_dir().join(format!(
            "marionette_test_auth_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let auth_path = dir.join("auth.json");

        super::write_provider_key_at("deepseek", "sk-test123", Some(&auth_path), false).unwrap();
        assert!(auth_path.is_file());
        let content = std::fs::read_to_string(&auth_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["deepseek"]["key"].as_str(), Some("sk-test123"));
        // OpenCode Auth schema requires the type discriminator.
        assert_eq!(parsed["deepseek"]["type"].as_str(), Some("api"));

        // Write another provider — original should survive.
        super::write_provider_key_at("openrouter", "or-test456", Some(&auth_path), false).unwrap();
        let content = std::fs::read_to_string(&auth_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["deepseek"]["key"].as_str(), Some("sk-test123"));
        assert_eq!(parsed["deepseek"]["type"].as_str(), Some("api"));
        assert_eq!(parsed["openrouter"]["key"].as_str(), Some("or-test456"));
        assert_eq!(parsed["openrouter"]["type"].as_str(), Some("api"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_overwrites_existing_key() {
        let dir = std::env::temp_dir().join(format!(
            "marionette_test_auth_overwrite_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let auth_path = dir.join("auth.json");

        super::write_provider_key_at("deepseek", "sk-old", Some(&auth_path), false).unwrap();
        super::write_provider_key_at("deepseek", "sk-new", Some(&auth_path), false).unwrap();

        let content = std::fs::read_to_string(&auth_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["deepseek"]["key"].as_str(), Some("sk-new"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_removes_provider_key() {
        let dir = std::env::temp_dir().join(format!(
            "marionette_test_delete_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let auth_path = dir.join("auth.json");

        super::write_provider_key_at("deepseek", "sk-111", Some(&auth_path), false).unwrap();
        super::write_provider_key_at("openrouter", "sk-222", Some(&auth_path), false).unwrap();

        // Delete one provider
        super::delete_provider_key_at("deepseek", Some(&auth_path), false).unwrap();

        let content = std::fs::read_to_string(&auth_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(parsed.get("deepseek").is_none(), "deepseek should be removed");
        assert_eq!(
            parsed["openrouter"]["key"].as_str(),
            Some("sk-222"),
            "openrouter should survive"
        );

        // Delete non-existent returns error
        let err = super::delete_provider_key_at("nonexistent", Some(&auth_path), false).unwrap_err();
        assert!(err.contains("not found"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An `opencode auth login` entry must survive both a save and a delete
    /// unless the caller passes `force` — the refresh token is unrecoverable.
    #[test]
    fn oauth_entries_are_protected_unless_forced() {
        let dir = std::env::temp_dir().join(format!(
            "marionette_test_oauth_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let auth_path = dir.join("auth.json");
        std::fs::write(
            &auth_path,
            r#"{"anthropic":{"type":"oauth","refresh":"rt-secret","access":"at-secret"}}"#,
        )
        .unwrap();

        let err = super::write_provider_key_at("anthropic", "sk-new", Some(&auth_path), false)
            .unwrap_err();
        assert!(err.contains("OAuth"), "write should refuse: {err}");
        let err =
            super::delete_provider_key_at("anthropic", Some(&auth_path), false).unwrap_err();
        assert!(err.contains("OAuth"), "delete should refuse: {err}");

        // Refusing must not have touched the file.
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&auth_path).unwrap()).unwrap();
        assert_eq!(parsed["anthropic"]["refresh"].as_str(), Some("rt-secret"));

        // `force` goes through.
        super::write_provider_key_at("anthropic", "sk-new", Some(&auth_path), true).unwrap();
        let forced: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&auth_path).unwrap()).unwrap();
        assert_eq!(forced["anthropic"]["type"].as_str(), Some("api"));
        assert_eq!(forced["anthropic"]["key"].as_str(), Some("sk-new"));
    }

    /// Legacy Marionette writes used `{ "key": "…" }` without `type`. OpenCode
    /// silently drops those on load — repair must inject `type: "api"`.
    #[test]
    fn repairs_legacy_key_only_auth_entries() {
        let dir = std::env::temp_dir().join(format!(
            "marionette_test_auth_repair_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let auth_path = dir.join("auth.json");
        std::fs::write(
            &auth_path,
            r#"{"deepseek":{"key":"sk-legacy"},"anthropic":{"type":"oauth","refresh":"rt","access":"at"}}"#,
        )
        .unwrap();

        let mut auth = super::read_auth_for_write(&auth_path).unwrap();
        assert!(super::repair_auth_api_types(&mut auth));
        super::write_auth_atomic(&auth_path, &auth).unwrap();

        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&auth_path).unwrap()).unwrap();
        assert_eq!(parsed["deepseek"]["type"].as_str(), Some("api"));
        assert_eq!(parsed["deepseek"]["key"].as_str(), Some("sk-legacy"));
        // OAuth left untouched.
        assert_eq!(parsed["anthropic"]["type"].as_str(), Some("oauth"));
        assert_eq!(parsed["anthropic"]["refresh"].as_str(), Some("rt"));

        // Second pass is a no-op.
        let mut again = super::read_auth_for_write(&auth_path).unwrap();
        assert!(!super::repair_auth_api_types(&mut again));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A corrupt auth.json used to parse as `{}`, so the next write persisted
    /// only the new key and silently dropped every other provider.
    #[test]
    fn corrupt_auth_json_aborts_instead_of_wiping() {
        let dir = std::env::temp_dir().join(format!(
            "marionette_test_corrupt_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let auth_path = dir.join("auth.json");
        let corrupt = r#"{"deepseek":{"key":"sk-keep"},"openrouter":{"key":"#;
        std::fs::write(&auth_path, corrupt).unwrap();

        let err =
            super::write_provider_key_at("zai", "sk-new", Some(&auth_path), false).unwrap_err();
        assert!(err.contains("解析失败"), "unexpected error: {err}");
        assert_eq!(
            std::fs::read_to_string(&auth_path).unwrap(),
            corrupt,
            "file must be left exactly as it was"
        );

        // A top-level array is refused too, rather than rewritten as an object.
        std::fs::write(&auth_path, "[]").unwrap();
        let err =
            super::delete_provider_key_at("deepseek", Some(&auth_path), false).unwrap_err();
        assert!(err.contains("JSON 对象"), "unexpected error: {err}");
        assert_eq!(std::fs::read_to_string(&auth_path).unwrap(), "[]");

        // An empty file is a legitimate fresh start, not corruption.
        std::fs::write(&auth_path, "").unwrap();
        super::write_provider_key_at("zai", "sk-new", Some(&auth_path), false).unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&auth_path).unwrap()).unwrap();
        assert_eq!(parsed["zai"]["key"].as_str(), Some("sk-new"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_providers_from_written_keys() {
        let dir = std::env::temp_dir().join(format!(
            "marionette_test_list_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let auth_path = dir.join("auth.json");

        super::write_provider_key_at("deepseek", "sk-111", Some(&auth_path), false).unwrap();
        super::write_provider_key_at("openrouter", "sk-222", Some(&auth_path), false).unwrap();

        // Point load_auth_json at our temp file by overriding the search path.
        // We call write_provider_key_at directly; list_providers uses load_auth_json
        // which searches standard paths. Instead, test the read-back through the file.
        let content = std::fs::read_to_string(&auth_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        let obj = parsed.as_object().unwrap();
        assert!(obj.contains_key("deepseek"));
        assert!(obj.contains_key("openrouter"));
        assert_eq!(obj["deepseek"]["key"].as_str(), Some("sk-111"));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
