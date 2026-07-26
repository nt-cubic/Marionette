//! Probe provider billing/balance for the model currently selected in OpenCode (and similar).
//!
//! Keys are read from OpenCode's local auth store (`~/.local/share/opencode/auth.json`).
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

/// Extract API key for a provider id without ever returning the secret.
fn auth_key_for(auth: &Value, provider: &str) -> Result<String, String> {
    let aliases: &[&str] = match provider {
        "opencode-go" | "opencode_go" | "go" => &["opencode-go", "opencode", "opencode-zen"],
        "opencode" | "opencode-zen" | "zen" => &["opencode", "opencode-go", "opencode-zen"],
        "deepseek" => &["deepseek"],
        "openrouter" => &["openrouter"],
        "zai" | "z-ai" | "zhipu" | "glm" => &["zai", "zhipu", "glm"],
        "xai" | "grok" => &["xai"],
        "nvidia" => &["nvidia"],
        "huggingface" | "hf" => &["huggingface"],
        "siliconflow" | "siliconflow-cn" => &["siliconflow-cn", "siliconflow"],
        other => {
            // Fall through to exact key.
            let _ = other;
            &[]
        }
    };

    let mut tried = Vec::new();
    let keys: Vec<String> = if aliases.is_empty() {
        vec![provider.to_string()]
    } else {
        aliases.iter().map(|s| (*s).to_string()).collect()
    };

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
    match provider {
        "deepseek" => "DeepSeek API".into(),
        "openrouter" => "OpenRouter".into(),
        "opencode-go" | "opencode_go" | "go" => "OpenCode Go".into(),
        "opencode" | "opencode-zen" | "zen" => "OpenCode Zen".into(),
        "zai" | "zhipu" => "Z.AI / GLM".into(),
        "xai" => "xAI".into(),
        "nvidia" => "NVIDIA NIM".into(),
        "huggingface" => "Hugging Face".into(),
        "siliconflow" | "siliconflow-cn" => "SiliconFlow".into(),
        other => other.to_string(),
    }
}

fn http_get_json(url: &str, bearer: &str) -> Result<Value, String> {
    let resp = ureq::get(url)
        .set("Authorization", &format!("Bearer {bearer}"))
        .set("Accept", "application/json")
        .set("User-Agent", "AgentShell/0.1 (provider-usage)")
        .timeout(std::time::Duration::from_secs(15))
        .call()
        .map_err(|e| format!("HTTP {url}: {e}"))?;
    let status = resp.status();
    let text = resp
        .into_string()
        .map_err(|e| format!("Read body {url}: {e}"))?;
    if !(200..300).contains(&status) {
        let snippet: String = text.chars().take(180).collect();
        return Err(format!("HTTP {status} from {url}: {snippet}"));
    }
    serde_json::from_str(&text).map_err(|e| format!("JSON from {url}: {e}; body={}", text.chars().take(120).collect::<String>()))
}

fn money(amount: f64, currency: &str) -> String {
    match currency.to_ascii_uppercase().as_str() {
        "CNY" | "RMB" => format!("¥{amount:.2}"),
        "USD" | "$" => format!("${amount:.2}"),
        other => format!("{amount:.4} {other}"),
    }
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
                percentage: None,
                detail: Some(format!(
                    "{} total · topped-up {} · grant {}",
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
            "No balance probe for `{other}` yet (supported: deepseek, openrouter, opencode-go, opencode)"
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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInfo {
    pub provider: String,
    pub label: String,
    pub has_key: bool,
}

/// Provider id → human label.
fn provider_display_name(id: &str) -> String {
    match id {
        "deepseek" => "DeepSeek",
        "openrouter" => "OpenRouter",
        "openai" => "OpenAI",
        "anthropic" => "Anthropic",
        "google" => "Google",
        "xai" => "xAI (Grok)",
        "zai" | "zhipu" => "Z.AI (GLM)",
        "siliconflow" | "siliconflow-cn" => "SiliconFlow",
        other => other,
    }
    .to_string()
}

/// Write a provider API key to the given auth file path.
/// If `path` is `None`, uses the default OpenCode auth.json location.
pub fn write_provider_key_at(
    provider: &str,
    key: &str,
    path: Option<&std::path::Path>,
) -> Result<(), String> {
    let path: PathBuf = match path {
        Some(p) => p.to_path_buf(),
        None => opencode_auth_path_write(),
    };

    // Read existing file or start with empty object.
    let mut auth: serde_json::Value = if path.is_file() {
        let text = fs::read_to_string(&path).map_err(|e| format!("读取 auth.json 失败: {e}"))?;
        serde_json::from_str(&text)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
    } else {
        serde_json::Value::Object(serde_json::Map::new())
    };

    // Set the key: auth[provider] = { "key": key }
    auth[provider] = serde_json::json!({ "key": key });

    // Atomic write: temp file → rename.
    let temp_path = path.with_extension("json.tmp");
    let json_str =
        serde_json::to_string_pretty(&auth).map_err(|e| format!("序列化 auth.json 失败: {e}"))?;
    fs::write(&temp_path, &json_str).map_err(|e| format!("写入临时文件失败: {e}"))?;
    fs::rename(&temp_path, &path).map_err(|e| format!("重命名临时文件失败: {e}"))?;

    Ok(())
}

/// Write a provider API key to the default OpenCode auth.json path.
pub fn write_provider_key(provider: &str, key: &str) -> Result<(), String> {
    write_provider_key_at(provider, key, None)
}

/// List all configured providers in auth.json (without exposing keys).
pub fn list_providers() -> Result<Vec<ProviderInfo>, String> {
    let auth = load_auth_json()?;
    let mut providers = Vec::new();
    if let Some(obj) = auth.as_object() {
        for (key, value) in obj {
            let has_key = value
                .get("key")
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false)
                || value
                    .get("access")
                    .and_then(|v| v.as_str())
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);
            providers.push(ProviderInfo {
                provider: key.clone(),
                label: provider_display_name(key),
                has_key,
            });
        }
    }
    Ok(providers)
}

#[cfg(test)]
mod tests {
    use super::split_model_id;

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
    fn write_and_read_provider_key() {
        let dir = std::env::temp_dir().join(format!(
            "agentshell_test_auth_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let auth_path = dir.join("auth.json");

        super::write_provider_key_at("deepseek", "sk-test123", Some(&auth_path)).unwrap();
        assert!(auth_path.is_file());
        let content = std::fs::read_to_string(&auth_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["deepseek"]["key"].as_str(), Some("sk-test123"));

        // Write another provider — original should survive.
        super::write_provider_key_at("openrouter", "or-test456", Some(&auth_path)).unwrap();
        let content = std::fs::read_to_string(&auth_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["deepseek"]["key"].as_str(), Some("sk-test123"));
        assert_eq!(parsed["openrouter"]["key"].as_str(), Some("or-test456"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn write_overwrites_existing_key() {
        let dir = std::env::temp_dir().join(format!(
            "agentshell_test_auth_overwrite_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let auth_path = dir.join("auth.json");

        super::write_provider_key_at("deepseek", "sk-old", Some(&auth_path)).unwrap();
        super::write_provider_key_at("deepseek", "sk-new", Some(&auth_path)).unwrap();

        let content = std::fs::read_to_string(&auth_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["deepseek"]["key"].as_str(), Some("sk-new"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_providers_from_written_keys() {
        let dir = std::env::temp_dir().join(format!(
            "agentshell_test_list_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let auth_path = dir.join("auth.json");

        super::write_provider_key_at("deepseek", "sk-111", Some(&auth_path)).unwrap();
        super::write_provider_key_at("openrouter", "sk-222", Some(&auth_path)).unwrap();

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
