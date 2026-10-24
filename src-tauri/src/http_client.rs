//! Shared HTTPS client using ureq + native-tls (Windows schannel).
//!
//! ureq 2.x does **not** auto-wire `native-tls` into `ureq::get` / default Agent.
//! With `default-features = false` (no rustls), convenience calls fail with:
//!   "cannot make HTTPS request because no TLS backend is configured"
//! Callers must build an Agent with `tls_connector` — this module does that once.

use std::sync::{Arc, OnceLock};
use ureq::Agent;

static AGENT: OnceLock<Agent> = OnceLock::new();

/// Cheap-to-clone Agent with native-tls enabled. Initialized once per process.
pub fn agent() -> Result<Agent, String> {
    if let Some(agent) = AGENT.get() {
        return Ok(agent.clone());
    }
    let connector = ureq::native_tls::TlsConnector::new()
        .map_err(|e| format!("TLS init failed (native-tls/schannel): {e}"))?;
    let built = ureq::AgentBuilder::new()
        .tls_connector(Arc::new(connector))
        .build();
    // OnceLock 只缓存成功结果：以前缓存 Result 让一次 TLS 初始化失败永久
    // 粘滞（整个进程再也不能出网）。并发首呼各自建一个等价 Agent，
    // get_or_init 只决定谁进缓存。
    Ok(AGENT.get_or_init(|| built).clone())
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
    let text = resp
        .into_string()
        .map_err(|e| format!("Read {url}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("JSON from {url}: {e}"))
}
