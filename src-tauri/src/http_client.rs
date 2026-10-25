//! Shared HTTPS client using ureq + native-tls (Windows schannel).
//!
//! ureq 2.x does **not** auto-wire `native-tls` into `ureq::get` / default Agent.
//! With `default-features = false` (no rustls), convenience calls fail with:
//!   "cannot make HTTPS request because no TLS backend is configured"
//! Callers must build an Agent with `tls_connector` — this module does that once.

use std::sync::{Arc, Mutex, OnceLock};
use ureq::Agent;

static AGENT: OnceLock<Agent> = OnceLock::new();
static PROXY_AGENT: Mutex<Option<(String, Agent)>> = Mutex::new(None);

/// Cheap-to-clone Agent with native-tls enabled. Initialized once per process.
pub fn agent() -> Result<Agent, String> {
    if let Some(agent) = AGENT.get() {
        return Ok(agent.clone());
    }
    let connector = ureq::native_tls::TlsConnector::new()
        .map_err(|e| format!("TLS init failed (native-tls/schannel): {e}"))?;
    let built = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .tls_connector(Arc::new(connector))
        .build();
    // OnceLock 只缓存成功结果：以前缓存 Result 让一次 TLS 初始化失败永久
    // 粘滞（整个进程再也不能出网）。并发首呼各自建一个等价 Agent，
    // get_or_init 只决定谁进缓存。
    Ok(AGENT.get_or_init(|| built).clone())
}

/// External requests that follow the app proxy setting (e.g. Telegram).
/// Read settings on every call; reuse connections only for the same proxy URL.
/// Keep `agent()` direct so loopback callers never enter the proxy.
pub fn agent_with_proxy() -> Result<Agent, String> {
    let config = crate::proxy::load_proxy_config();
    if !config.enabled {
        return agent();
    }
    let url = config.url.trim();
    let mut cached = PROXY_AGENT
        .lock()
        .map_err(|_| "proxy HTTP client lock poisoned".to_string())?;
    if let Some((cached_url, agent)) = cached.as_ref() {
        if cached_url == url {
            return Ok(agent.clone());
        }
    }
    // Do not fall back to direct traffic or include proxy credentials in errors.
    let proxy = ureq::Proxy::new(url).map_err(|_| "应用代理地址无效，请检查代理设置".to_string())?;
    let connector = ureq::native_tls::TlsConnector::new()
        .map_err(|e| format!("TLS init failed (native-tls/schannel): {e}"))?;
    let agent = ureq::AgentBuilder::new()
        .try_proxy_from_env(false)
        .tls_connector(Arc::new(connector))
        .proxy(proxy)
        .build();
    *cached = Some((url.to_string(), agent.clone()));
    Ok(agent)
}

/// GET JSON with a short timeout. Used for the public ACP agent registry.
pub fn get_json(url: &str) -> Result<serde_json::Value, String> {
    let resp = agent()?
        .get(url)
        .set("Accept", "application/json")
        .set("User-Agent", "Marionette/0.1")
        .timeout(std::time::Duration::from_secs(12))
        .call()
        .map_err(|e| format!("GET {url}: {e}"))?;
    let text = {
        // 读取加硬上限：注册表 JSON 正常远小于 16MiB，别让远端塞爆内存。
        const MAX_REGISTRY_BYTES: u64 = 16 * 1024 * 1024;
        use std::io::Read;
        let reader = resp.into_reader();
        let mut text = String::new();
        let mut capped = reader.take(MAX_REGISTRY_BYTES + 1);
        capped
            .read_to_string(&mut text)
            .map_err(|e| format!("Read {url}: {e}"))?;
        if text.len() as u64 > MAX_REGISTRY_BYTES {
            return Err(format!(
                "Response from {url} exceeds {} MiB limit",
                MAX_REGISTRY_BYTES / (1024 * 1024)
            ));
        }
        text
    };
    serde_json::from_str(&text).map_err(|e| format!("JSON from {url}: {e}"))
}
