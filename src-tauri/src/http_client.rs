//! Shared HTTPS client using ureq + native-tls (Windows schannel).
//!
//! ureq 2.x does **not** auto-wire `native-tls` into `ureq::get` / default Agent.
//! With `default-features = false` (no rustls), convenience calls fail with:
//!   "cannot make HTTPS request because no TLS backend is configured"
//! Callers must build an Agent with `tls_connector` — this module does that once.

use std::sync::{Arc, OnceLock};
use ureq::Agent;

static AGENT: OnceLock<Result<Agent, String>> = OnceLock::new();

/// Cheap-to-clone Agent with native-tls enabled. Initialized once per process.
pub fn agent() -> Result<Agent, String> {
    AGENT
        .get_or_init(|| {
            let connector = ureq::native_tls::TlsConnector::new().map_err(|e| {
                format!("TLS init failed (native-tls/schannel): {e}")
            })?;
            Ok(ureq::AgentBuilder::new()
                .tls_connector(Arc::new(connector))
                .build())
        })
        .clone()
}
