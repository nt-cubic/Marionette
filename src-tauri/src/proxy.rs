//! Agent proxy configuration.
//!
//! Marionette is an agent harness: it starts agent CLIs (codex, opencode, …)
//! as child processes. When the user lives behind a censored or metered
//! network, those agents need an HTTP(S) proxy exit. We store a *single*
//! proxy address and inject it into every spawned agent via the standard
//! `HTTPS_PROXY` / `HTTP_PROXY` / `NO_PROXY` environment variables.
//!
//! Deliberate scope:
//! - Only one address. Rule-vs-global routing belongs to the local proxy
//!   client (Clash, v2ray, …); we never try to control it.
//! - `NO_PROXY` is fixed to localhost/loopback so local MCP servers spawned
//!   by agents are never proxied. Not exposed in the UI.
//! - No per-agent overrides. One exit for everything.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Loopback is always kept direct — local MCP servers (Unity, drawio,
/// playwright, node_repl, …) must never round-trip through the proxy.
pub const NO_PROXY_DEFAULT: &str = "localhost,127.0.0.1,::1";

/// Endpoint used by `test_proxy`. Reaching it (even with HTTP 401) proves
/// the full path — DNS, CONNECT tunnel and TLS — works through the proxy.
const TEST_TARGET: &str = "https://api.openai.com/v1/models";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyConfig {
    /// Whether spawned agents should route through `url`.
    #[serde(default)]
    pub enabled: bool,
    /// Full proxy URL, e.g. `http://127.0.0.1:7890`. May carry `user:pass@`.
    #[serde(default)]
    pub url: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyTestResult {
    pub ok: bool,
    /// Human-readable outcome (latency or failure reason).
    pub message: String,
    /// Round-trip latency in ms when the proxy path responded.
    pub latency_ms: Option<u64>,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn proxy_config_path() -> Option<PathBuf> {
    Some(home_dir()?.join(".marionette").join("proxy.json"))
}

pub fn load_proxy_config() -> ProxyConfig {
    let Some(path) = proxy_config_path() else {
        return ProxyConfig::default();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return ProxyConfig::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Persist the config. An enabled config must be a usable proxy URL.
pub fn save_proxy_config(config: &ProxyConfig) -> Result<(), String> {
    if config.enabled {
        validate_proxy_url(&config.url)?;
    }
    let Some(path) = proxy_config_path() else {
        return Err("cannot locate home directory".to_string());
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("create ~/.marionette: {e}"))?;
    }
    let raw = serde_json::to_string_pretty(config)
        .map_err(|e| format!("serialize proxy config: {e}"))?;
    std::fs::write(&path, raw).map_err(|e| format!("write proxy config: {e}"))?;
    Ok(())
}

/// Accepts `http://host:port`, `https://host:port` or bare `host:port`.
pub fn validate_proxy_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("代理地址不能为空".to_string());
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        // Allow bare host:port (ureq treats it as http), but still require a port.
        if !url.contains(':') {
            return Err("代理地址缺少端口，例如 http://127.0.0.1:7890".to_string());
        }
    }
    ureq::Proxy::new(url)
        .map(|_| ())
        .map_err(|e| format!("代理地址无效: {e}"))
}

/// Environment to inject into agent processes when a proxy is enabled.
/// Returns `(HTTPS_PROXY/HTTP_PROXY value, NO_PROXY value)`.
pub fn proxy_env() -> Option<(String, String)> {
    let config = load_proxy_config();
    if !config.enabled {
        return None;
    }
    let url = config.url.trim();
    if url.is_empty() {
        return None;
    }
    Some((url.to_string(), NO_PROXY_DEFAULT.to_string()))
}

/// Round-trip through `url` to OpenAI's API endpoint. HTTP 401 still counts
/// as success: it means the proxy path itself is alive (auth is a later step).
pub fn test_proxy(url: &str) -> Result<ProxyTestResult, String> {
    let proxy = ureq::Proxy::new(url.trim())
        .map_err(|e| format!("代理地址无效: {e}"))?;
    let connector = ureq::native_tls::TlsConnector::new()
        .map_err(|e| format!("TLS init failed (schannel): {e}"))?;
    let agent = ureq::AgentBuilder::new()
        .proxy(proxy)
        .tls_connector(Arc::new(connector))
        .timeout(Duration::from_secs(12))
        .build();

    let started = Instant::now();
    match agent.get(TEST_TARGET).call() {
        Ok(_) => {
            let ms = started.elapsed().as_millis() as u64;
            Ok(ProxyTestResult {
                ok: true,
                message: format!("连接成功 · {ms}ms"),
                latency_ms: Some(ms),
            })
        }
        // Any HTTP status (notably 401 without a key) proves the tunnel works.
        Err(ureq::Error::Status(code, _)) => {
            let ms = started.elapsed().as_millis() as u64;
            Ok(ProxyTestResult {
                ok: true,
                message: format!("代理连通（HTTP {code}）· {ms}ms"),
                latency_ms: Some(ms),
            })
        }
        Err(error) => Ok(ProxyTestResult {
            ok: false,
            message: format!("连接失败: {error}"),
            latency_ms: None,
        }),
    }
}
