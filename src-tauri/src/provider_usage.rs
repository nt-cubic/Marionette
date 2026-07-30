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
    None,
}

impl ProbeStrategy {
    fn as_str(self) -> &'static str {
        match self {
            Self::Deepseek => "deepseek",
            Self::Openrouter => "openrouter",
            Self::Openai => "openai",
            Self::OpencodeZen => "opencode-zen",
            Self::None => "none",
        }
    }

    fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "deepseek" => Self::Deepseek,
            "openrouter" => Self::Openrouter,
            "openai" | "chatgpt" | "codex" => Self::Openai,
            "opencode-zen" | "opencode_zen" | "zen" => Self::OpencodeZen,
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
        b(
            "zai",
            "Z.AI (GLM)",
            &["zai", "zhipu", "glm", "z-ai"],
            ProbeStrategy::None,
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

/// Extract an OAuth access token for ChatGPT usage probes.
///
/// Do not fall back to `key`: an OpenAI API key cannot authenticate the
/// ChatGPT subscription usage endpoint.
fn oauth_access_for(auth: &Value, provider: &str) -> Result<String, String> {
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
                    return Ok(access.to_string());
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

fn http_get_json(url: &str, bearer: &str) -> Result<Value, String> {
    let resp = crate::http_client::agent()?
        .get(url)
        .set("Authorization", &format!("Bearer {bearer}"))
        .set("Accept", "application/json")
        .set("User-Agent", "Marionette/0.1 (provider-usage)")
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .map_err(|e| format!("HTTP {url}: {e}"))?;
    let status = resp.status();
    let text = resp
        .into_string()
        .map_err(|e| format!("Read body {url}: {e}"))?;
    if !(200..300).contains(&status) {
        return Err(format!("HTTP {status} from {url}"));
    }
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

fn parse_openai_window(
    window: &Value,
    id: &str,
    label: &str,
) -> Option<ProviderUsageWindow> {
    let percentage = window
        .get("used_percent")
        .and_then(value_f64)
        .map(|value| value.clamp(0.0, 100.0));
    let detail = reset_hint(window.get("reset_at"));
    if percentage.is_none() && detail.is_none() {
        return None;
    }
    Some(ProviderUsageWindow {
        id: id.into(),
        label: label.into(),
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
        if let Some(window) = parse_openai_window(primary, "five-hour", "5-hour limit") {
            windows.push(window);
        }
    }
    if let Some(secondary) = rate_limit.get("secondary_window") {
        if let Some(window) = parse_openai_window(secondary, "weekly", "Weekly limit") {
            windows.push(window);
        }
    }
    windows
}

fn probe_openai_chatgpt(
    access_token: &str,
    model: Option<&str>,
) -> Result<ProviderUsageSnapshot, String> {
    let json = match http_get_json("https://chatgpt.com/backend-api/wham/usage", access_token) {
        Ok(json) => json,
        Err(error) if error.contains("HTTP 401") => {
            return Err("ChatGPT OAuth expired; run `opencode auth login` again".into());
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

/// OpenCode Go/Zen: no public usage API yet (see upstream feature requests).
/// Surface plan ceilings + credential check so the panel is still useful.
fn probe_opencode_plan(provider: &str, key: &str, model: Option<&str>) -> ProviderUsageSnapshot {
    let is_go = matches!(provider, "opencode-go" | "opencode_go" | "go");
    // Auth works if models list succeeds.
    let models_url = if is_go {
        "https://opencode.ai/zen/go/v1/models"
    } else {
        "https://opencode.ai/zen/v1/models"
    };
    let auth_ok = http_get_json(models_url, key).is_ok();

    let mut windows = vec![
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
    ];

    if is_go {
        // Documented Go subscription ceilings (not remaining usage).
        windows.push(ProviderUsageWindow {
            id: "five-hour".into(),
            label: "5-hour limit".into(),
            percentage: None,
            detail: Some("$12 usage ceiling (plan)".into()),
            kind: "rate_limit".into(),
        });
        windows.push(ProviderUsageWindow {
            id: "weekly".into(),
            label: "Weekly limit".into(),
            percentage: None,
            detail: Some("$30 usage ceiling (plan)".into()),
            kind: "rate_limit".into(),
        });
        windows.push(ProviderUsageWindow {
            id: "monthly".into(),
            label: "Monthly limit".into(),
            percentage: None,
            detail: Some("$60 usage ceiling (plan)".into()),
            kind: "rate_limit".into(),
        });
    } else {
        windows.push(ProviderUsageWindow {
            id: "zen-balance".into(),
            label: "Zen balance".into(),
            percentage: None,
            detail: Some("No public balance API — check opencode.ai/auth".into()),
            kind: "provider".into(),
        });
    }

    ProviderUsageSnapshot {
        provider: if is_go {
            "opencode-go".into()
        } else {
            "opencode".into()
        },
        provider_label: provider_label(if is_go { "opencode-go" } else { "opencode" }),
        model: model.map(str::to_string),
        model_label: model.map(str::to_string),
        windows,
        note: Some(if is_go {
            "OpenCode Go has no public remaining-usage API yet; ceilings from docs. Track live usage at opencode.ai/auth.".into()
        } else {
            "OpenCode Zen balance API is not public yet; open the console for remaining credits.".into()
        }),
        refreshed_at: now_iso(),
        source: if is_go {
            "opencode-go models + plan docs".into()
        } else {
            "opencode zen models".into()
        },
        ok: auth_ok,
    }
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
        "openai" | "codex" | "chatgpt" => match oauth_access_for(&auth, &provider) {
            Ok(access) => probe_openai_chatgpt(&access, model_ref),
            Err(e) => Err(e),
        },
        "openrouter" => match auth_key_for(&auth, "openrouter") {
            Ok(key) => probe_openrouter(&key, model_ref),
            Err(e) => Err(e),
        },
        "opencode-go" | "opencode_go" | "go" => match auth_key_for(&auth, "opencode-go") {
            Ok(key) => Ok(probe_opencode_plan("opencode-go", &key, model_ref)),
            Err(e) => Err(e),
        },
        "opencode" | "opencode-zen" | "zen" => match auth_key_for(&auth, "opencode") {
            Ok(key) => Ok(probe_opencode_plan("opencode", &key, model_ref)),
            Err(e) => Err(e),
        },
        other => Err(format!(
            "No balance probe for `{other}` yet (supported: deepseek, openai, openrouter, opencode-go, opencode)"
        )),
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

    auth.insert(provider.to_string(), serde_json::json!({ "key": key }));
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
    use super::{oauth_access_for, parse_openai_usage, split_model_id};
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
    fn reads_chatgpt_oauth_access_without_using_api_key() {
        let auth = json!({
            "openai": {
                "type": "oauth",
                "access": "oauth-access",
                "refresh": "oauth-refresh",
                "key": "api-key-that-must-not-be-used"
            }
        });
        assert_eq!(
            oauth_access_for(&auth, "openai").unwrap(),
            "oauth-access"
        );

        let api_only = json!({ "openai": { "key": "api-key" } });
        assert!(oauth_access_for(&api_only, "openai").is_err());
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

        // Write another provider — original should survive.
        super::write_provider_key_at("openrouter", "or-test456", Some(&auth_path), false).unwrap();
        let content = std::fs::read_to_string(&auth_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["deepseek"]["key"].as_str(), Some("sk-test123"));
        assert_eq!(parsed["openrouter"]["key"].as_str(), Some("or-test456"));

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
        let parsed: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&auth_path).unwrap()).unwrap();
        assert_eq!(parsed["anthropic"]["key"].as_str(), Some("sk-new"));

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
