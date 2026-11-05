//! Universal plugin host: relays MCP between ACP agents and installed plugins
//! (docs/plan-plugin-layer.md).
//!
//! Every enabled plugin is mounted on the tools-mcp localhost server under
//! `/mcp/p/<id>` (tools_mcp::handle_request hands the message over). stdio
//! plugins run as Marionette-owned child processes — one per ACP session,
//! spawned lazily on the agent's first request, killed when the session stops;
//! a kill-on-close Job Object carries them away if Marionette itself crashes.
//! Remote plugins are relayed over plain HTTP so their real credentials never
//! leave this process: the agent only ever holds the loopback URL and a
//! session token.
//!
//! Red lines:
//! - Never wait on a child while holding the CHILDREN lock (HelperIo lesson).
//! - A plugin's own JSON-RPC errors relay verbatim; only host failures
//!   (spawn/timeout/process exit) become -32000 transport errors.
//! - Credential values never enter debug logs.
//! - Server-initiated requests from a plugin (sampling, roots…) are declined
//!   with -32601 — the agent sits behind a request/response boundary and
//!   cannot answer them.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use crate::plugins::{PluginRecord, ProbeRecord};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
const CALL_TIMEOUT: Duration = Duration::from_secs(300);
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const NOTIFY_RELAY_TIMEOUT: Duration = Duration::from_secs(10);
/// Response body cap for the remote relay — a runaway response must not OOM.
const MAX_RELAY_BODY: u64 = 16 * 1024 * 1024;
const MAX_LOG_LINE: usize = 400;

type PendingMap = Arc<Mutex<HashMap<u64, Sender<Result<Value, Value>>>>>;

/// (session_id, plugin_id) → one hosted child. Sessions that never touch a
/// plugin never pay for it (lazy spawn on first request).
static CHILDREN: Mutex<Option<HashMap<(String, String), Arc<HostedChild>>>> = Mutex::new(None);
/// (session_id, plugin_id) → remote `Mcp-Session-Id` (streamable-HTTP session
/// pinning: the agent's initialize went to *us*, so we hold the remote half).
static REMOTE_SESSIONS: Mutex<Option<HashMap<(String, String), String>>> = Mutex::new(None);

struct HostedChild {
    child: Mutex<Option<Child>>,
    stdin_tx: Mutex<Sender<Value>>,
    pending: PendingMap,
    next_id: AtomicU64,
    initialized: AtomicBool,
    init_lock: Mutex<()>,
    dead: AtomicBool,
}

#[derive(Debug)]
enum RelayError {
    /// The plugin answered with a JSON-RPC error object — relay verbatim.
    Rpc(Value),
    /// Host-side failure (spawn/timeout/process gone) → -32000.
    Host(String),
}

impl RelayError {
    fn to_note(&self) -> String {
        match self {
            RelayError::Rpc(error) => format!("plugin error: {error}"),
            RelayError::Host(text) => text.clone(),
        }
    }
}

fn lock_poisoned() -> String {
    "plugin host lock poisoned".to_string()
}

// ─── Entry point (called from tools_mcp::handle_request) ────────────────────

pub(crate) fn proxy_handle(
    plugin_id: &str,
    session_id: String,
    message: Value,
    request: tiny_http::Request,
) {
    let is_notification = message.get("id").is_none();
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let method = message
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let Some(plugin) = crate::plugins::enabled_plugin(plugin_id) else {
        if is_notification {
            let _ = request.respond(tiny_http::Response::empty(202));
        } else {
            respond_json(
                request,
                jsonrpc_error_envelope(id, -32000, "plugin not installed or disabled"),
            );
        }
        return;
    };

    let outcome = match plugin.transport.as_str() {
        "stdio" => relay_stdio(&plugin, &session_id, &message, &method, is_notification),
        "http" => {
            if is_notification {
                // Fire-and-forget: answer the agent now, POST on a side thread.
                let plugin = plugin.clone();
                let session = session_id.clone();
                let message = message.clone();
                let method = method.clone();
                std::thread::spawn(move || {
                    let _ = relay_http(&plugin, &session, &message, &method, true);
                });
                Ok(Value::Null)
            } else {
                relay_http(&plugin, &session_id, &message, &method, false)
            }
        }
        other => Err(RelayError::Host(format!(
            "unsupported plugin transport: {other}"
        ))),
    };

    if is_notification {
        let _ = request.respond(tiny_http::Response::empty(202));
        return;
    }
    // stdio produced a bare result; http produced a full envelope. Normalize.
    let envelope = match outcome {
        Ok(Value::Object(mut fields)) if fields.contains_key("jsonrpc") => {
            // Already an envelope (remote relay) — keep the agent's id in sync.
            fields.insert("id".to_string(), id.clone());
            Value::Object(fields)
        }
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(RelayError::Rpc(error)) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
        Err(RelayError::Host(text)) => jsonrpc_error_envelope(
            id,
            -32000,
            &format!("plugin `{plugin_id}`: {text}"),
        ),
    };
    respond_json(request, envelope);
}

fn jsonrpc_error_envelope(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn respond_json(request: tiny_http::Request, body: Value) {
    let response = tiny_http::Response::from_string(body.to_string())
        .with_header(
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..])
                .expect("static header"),
        );
    let _ = request.respond(response);
}

// ─── stdio relay ─────────────────────────────────────────────────────────────

fn relay_stdio(
    plugin: &PluginRecord,
    session_id: &str,
    message: &Value,
    method: &str,
    is_notification: bool,
) -> Result<Value, RelayError> {
    let child = ensure_child(session_id, plugin)?;
    if method == "initialize" && !is_notification {
        // The agent's initialize doubles as the child handshake.
        let params = message.get("params").cloned().unwrap_or(json!({}));
        return handshake(&child, Some(&params));
    }
    ensure_handshake(&child)?;
    if is_notification {
        // Our handshake already delivered notifications/initialized; relaying
        // the agent's copy would duplicate it. Everything else passes through.
        if method == "notifications/initialized" {
            return Ok(Value::Null);
        }
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        notify_child(&child, method, params)?;
        return Ok(Value::Null);
    }
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    request_child(&child, method, params, timeout_for(method))
}

fn timeout_for(method: &str) -> Duration {
    if method == "tools/call" {
        CALL_TIMEOUT
    } else {
        HANDSHAKE_TIMEOUT
    }
}

fn ensure_child(session_id: &str, plugin: &PluginRecord) -> Result<Arc<HostedChild>, RelayError> {
    {
        let guard = CHILDREN.lock().map_err(|_| RelayError::Host(lock_poisoned()))?;
        if let Some(child) = guard
            .as_ref()
            .and_then(|map| map.get(&(session_id.to_string(), plugin.id.clone())))
        {
            if child.dead.load(Ordering::Relaxed) {
                return Err(RelayError::Host(
                    "plugin process exited; reconnect the session to retry".to_string(),
                ));
            }
            return Ok(Arc::clone(child));
        }
    }
    // Spawn outside the lock: command resolution can hit where.exe (seconds).
    let spawned = spawn_plugin_child(session_id, plugin).map_err(RelayError::Host)?;
    let mut guard = CHILDREN.lock().map_err(|_| RelayError::Host(lock_poisoned()))?;
    let map = guard.get_or_insert_with(HashMap::new);
    match map.entry((session_id.to_string(), plugin.id.clone())) {
        std::collections::hash_map::Entry::Occupied(won) => {
            // Lost a spawn race — reap ours, the winner stays.
            spawned.kill_and_reap();
            Ok(Arc::clone(won.get()))
        }
        std::collections::hash_map::Entry::Vacant(slot) => {
            slot.insert(Arc::clone(&spawned));
            Ok(spawned)
        }
    }
}

fn spawn_plugin_child(session_id: &str, plugin: &PluginRecord) -> Result<Arc<HostedChild>, String> {
    let resolved = crate::process_util::resolve_spawn_command(plugin.command.trim())
        .map_err(|error| format!("resolve `{}`: {error}", plugin.command))?;
    let mut command = Command::new(&resolved.program);
    resolved.apply_to(&mut command);
    command.args(&plugin.args);
    for (key, value) in &plugin.env {
        command.env(key, value);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("spawn `{}` failed: {error}", plugin.command))?;
    // Crash safety: the kill-on-close job takes this child down with us.
    #[cfg(target_os = "windows")]
    crate::engine::assign_child_to_kill_job(&child);
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| "plugin stdin unavailable".to_string())?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "plugin stdout unavailable".to_string())?;
    let stderr = child.stderr.take();
    let hosted = attach(session_id.to_string(), plugin.id.clone(), Some(child), stdin, stdout);
    if let Some(stderr) = stderr {
        spawn_stderr_log_thread(session_id.to_string(), plugin.id.clone(), stderr);
    }
    crate::debug_log::append(
        "plugins",
        "info",
        session_id,
        "plugin child spawned",
        Some(&plugin.id),
    );
    Ok(hosted)
}

/// Wire pipes into a `HostedChild`. Split from `spawn_plugin_child` so tests
/// can attach in-memory pipes instead of a real process.
fn attach(
    session_id: String,
    plugin_id: String,
    child: Option<Child>,
    stdin: impl Write + Send + 'static,
    stdout: impl Read + Send + 'static,
) -> Arc<HostedChild> {
    let writer_label = format!("{session_id}/plugin/{plugin_id}");
    let log_plugin_id = plugin_id.clone();
    let stdin_tx = crate::acp::acp_transport::spawn_stdin_writer(stdin, writer_label, move |error| {
        // The stdout reader observing EOF is the authoritative death signal;
        // this only explains why queued writes stopped landing.
        crate::debug_log::append(
            "plugins",
            "warn",
            &log_plugin_id,
            "plugin stdin writer stopped",
            Some(&error),
        );
    });
    let hosted = Arc::new(HostedChild {
        child: Mutex::new(child),
        stdin_tx: Mutex::new(stdin_tx),
        pending: PendingMap::default(),
        next_id: AtomicU64::new(1),
        initialized: AtomicBool::new(false),
        init_lock: Mutex::new(()),
        dead: AtomicBool::new(false),
    });
    spawn_stdout_reader(session_id, plugin_id, stdout, Arc::clone(&hosted));
    hosted
}

fn spawn_stdout_reader(
    session_id: String,
    plugin_id: String,
    stdout: impl Read + Send + 'static,
    child: Arc<HostedChild>,
) {
    let _ = std::thread::Builder::new()
        .name("plugin-mcp-read".to_string())
        .spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                match serde_json::from_str::<Value>(trimmed) {
                    Ok(message) => {
                        handle_child_message(&child, &session_id, &plugin_id, message)
                    }
                    Err(_) => crate::debug_log::append(
                        "plugins",
                        "warn",
                        &session_id,
                        "plugin child sent non-JSON line",
                        Some(&truncate_for_log(trimmed)),
                    ),
                }
            }
            // EOF: the child is gone. Fail every waiter so nothing hangs.
            child.dead.store(true, Ordering::SeqCst);
            if let Ok(mut pending) = child.pending.lock() {
                for (_, sender) in pending.drain() {
                    let _ = sender.send(Err(json!({
                        "code": -32000, "message": "plugin process exited"
                    })));
                }
            }
            crate::debug_log::append(
                "plugins",
                "info",
                &session_id,
                "plugin child stream ended",
                Some(&plugin_id),
            );
        });
}

fn handle_child_message(
    child: &Arc<HostedChild>,
    session_id: &str,
    plugin_id: &str,
    message: Value,
) {
    if message.get("method").is_none() {
        // Response to one of our requests.
        if let Some(id) = message
            .get("id")
            .and_then(crate::acp::acp_transport::json_rpc_id_as_u64)
        {
            let sender = child.pending.lock().ok().and_then(|mut map| map.remove(&id));
            if let Some(sender) = sender {
                if let Some(error) = message.get("error") {
                    let _ = sender.send(Err(error.clone()));
                } else {
                    let _ = sender.send(Ok(
                        message.get("result").cloned().unwrap_or(Value::Null)
                    ));
                }
            }
        }
    } else if message.get("id").is_some() {
        // Server-initiated request from the plugin (sampling, roots…): the
        // agent is behind a request/response boundary and cannot answer.
        // Decline instead of leaving the child hanging on a response.
        let _ = send_to_child(
            child,
            json!({
                "jsonrpc": "2.0",
                "id": message.get("id").cloned().unwrap_or(Value::Null),
                "error": {
                    "code": -32601,
                    "message": "marionette plugin host does not support server-initiated requests"
                }
            }),
        );
    } else {
        // Notification from the plugin — method name only, never the payload
        // (it may carry credentials or bulk content).
        let method = message.get("method").and_then(Value::as_str).unwrap_or("");
        crate::debug_log::append(
            "plugins",
            "info",
            session_id,
            "plugin notification",
            Some(&format!("{plugin_id} {method}")),
        );
    }
}

fn spawn_stderr_log_thread(session_id: String, plugin_id: String, stderr: impl Read + Send + 'static) {
    let _ = std::thread::Builder::new()
        .name("plugin-mcp-stderr".to_string())
        .spawn(move || {
            for line in BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                crate::debug_log::append(
                    "plugins",
                    "warn",
                    &session_id,
                    &format!("{plugin_id} stderr"),
                    Some(&truncate_for_log(trimmed)),
                );
            }
        });
}

fn send_to_child(child: &HostedChild, message: Value) -> Result<(), String> {
    child
        .stdin_tx
        .lock()
        .map_err(|_| "plugin stdin lock poisoned".to_string())?
        .send(message)
        .map_err(|_| "plugin stdin is closed (process gone?)".to_string())
}

fn notify_child(child: &Arc<HostedChild>, method: &str, params: Value) -> Result<(), RelayError> {
    send_to_child(
        child,
        json!({"jsonrpc": "2.0", "method": method, "params": params}),
    )
    .map_err(RelayError::Host)
}

fn request_child(
    child: &Arc<HostedChild>,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value, RelayError> {
    if child.dead.load(Ordering::Relaxed) {
        return Err(RelayError::Host(
            "plugin process exited; reconnect the session to retry".to_string(),
        ));
    }
    let id = child.next_id.fetch_add(1, Ordering::Relaxed);
    let (sender, receiver) = channel();
    child
        .pending
        .lock()
        .map_err(|_| RelayError::Host(lock_poisoned()))?
        .insert(id, sender);
    if let Err(error) = send_to_child(
        child,
        json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
    ) {
        let _ = child.pending.lock().map(|mut map| map.remove(&id));
        return Err(RelayError::Host(error));
    }
    match receiver.recv_timeout(timeout) {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(error)) => Err(RelayError::Rpc(error)),
        Err(RecvTimeoutError::Timeout) => {
            let _ = child.pending.lock().map(|mut map| map.remove(&id));
            Err(RelayError::Host(format!(
                "timed out after {}s",
                timeout.as_secs()
            )))
        }
        Err(RecvTimeoutError::Disconnected) => Err(RelayError::Host(
            "plugin process exited before answering".to_string(),
        )),
    }
}

/// One MCP handshake per child, exactly once, whoever triggers it. When the
/// agent's own `initialize` arrives it is forwarded verbatim (its
/// protocolVersion/clientInfo are what the child should see); a first request
/// that is not `initialize` (stateless-style clients) synthesizes one.
fn handshake(child: &Arc<HostedChild>, agent_params: Option<&Value>) -> Result<Value, RelayError> {
    let _guard = child.init_lock.lock().map_err(|_| RelayError::Host(lock_poisoned()))?;
    if child.initialized.load(Ordering::Relaxed) {
        return Err(RelayError::Host(
            "plugin session already initialized".to_string(),
        ));
    }
    let params = match agent_params {
        Some(params) => params.clone(),
        None => json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "marionette-plugin-host", "version": "1.0.0" },
        }),
    };
    let result = request_child(child, "initialize", params, HANDSHAKE_TIMEOUT)?;
    // Complete the lifecycle the child expects.
    let _ = send_to_child(
        child,
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    );
    child.initialized.store(true, Ordering::Relaxed);
    Ok(result)
}

fn ensure_handshake(child: &Arc<HostedChild>) -> Result<(), RelayError> {
    if child.initialized.load(Ordering::Relaxed) {
        return Ok(());
    }
    handshake(child, None).map(|_| ())
}

impl HostedChild {
    fn kill_and_reap(&self) {
        self.dead.store(true, Ordering::SeqCst);
        // Fail waiters first so a hung tools/call does not wait out its 300s.
        if let Ok(mut pending) = self.pending.lock() {
            for (_, sender) in pending.drain() {
                let _ = sender.send(Err(json!({
                    "code": -32000, "message": "plugin process exited"
                })));
            }
        }
        if let Ok(mut guard) = self.child.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

// ─── Remote (http) relay ─────────────────────────────────────────────────────

fn relay_http(
    plugin: &PluginRecord,
    session_id: &str,
    message: &Value,
    method: &str,
    is_notification: bool,
) -> Result<Value, RelayError> {
    let timeout = if is_notification {
        NOTIFY_RELAY_TIMEOUT
    } else {
        timeout_for(method)
    };
    let agent = crate::http_client::agent_with_timeout(timeout, !is_loopback_url(&plugin.url))
        .map_err(RelayError::Host)?;
    let mut request = agent
        .post(&plugin.url)
        .set("Content-Type", "application/json")
        // JSON only: an SSE stream cannot be surfaced back through the ACP
        // entry, so a streaming-only server fails loudly instead of hanging.
        .set("Accept", "application/json");
    for (key, value) in &plugin.headers {
        request = request.set(key, value);
    }
    if let Some(remote_session) = remote_session_of(session_id, &plugin.id) {
        request = request.set("Mcp-Session-Id", &remote_session);
    }
    let body =
        serde_json::to_string(message).map_err(|error| RelayError::Host(error.to_string()))?;
    let response = match request.send_string(&body) {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let text = read_capped(response.into_reader());
            return Err(RelayError::Host(format!(
                "HTTP {code}: {}",
                truncate_for_log(&text)
            )));
        }
        Err(other) => return Err(RelayError::Host(format!("request failed: {other}"))),
    };
    if method == "initialize" {
        if let Some(remote_session) = response.header("Mcp-Session-Id") {
            remember_remote_session(session_id, &plugin.id, remote_session);
        }
    }
    let status = response.status();
    if status == 202 || status == 204 {
        return Ok(json!({
            "jsonrpc": "2.0",
            "id": message.get("id").cloned().unwrap_or(Value::Null),
            "result": Value::Null
        }));
    }
    let text = read_capped(response.into_reader());
    let envelope: Value = serde_json::from_str(&text).map_err(|_| {
        RelayError::Host(format!("non-JSON response: {}", truncate_for_log(&text)))
    })?;
    Ok(envelope)
}

fn read_capped(reader: impl Read) -> String {
    let mut text = String::new();
    let _ = reader
        .take(MAX_RELAY_BODY)
        .read_to_string(&mut text);
    text
}

fn is_loopback_url(url: &str) -> bool {
    let rest = url
        .strip_prefix("http://")
        .unwrap_or_else(|| url.strip_prefix("https://").unwrap_or(url));
    // IPv6 bracket form first — a plain split would stop at the first ':'.
    let host = if let Some(bracketed) = rest.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or("")
    } else {
        rest.split(['/', ':']).next().unwrap_or("")
    };
    host == "127.0.0.1"
        || host.eq_ignore_ascii_case("localhost")
        || host.eq_ignore_ascii_case("::1")
}

fn remote_session_of(session_id: &str, plugin_id: &str) -> Option<String> {
    let guard = REMOTE_SESSIONS.lock().ok()?;
    guard
        .as_ref()?
        .get(&(session_id.to_string(), plugin_id.to_string()))
        .cloned()
}

fn remember_remote_session(session_id: &str, plugin_id: &str, value: &str) {
    if let Ok(mut guard) = REMOTE_SESSIONS.lock() {
        guard
            .get_or_insert_with(HashMap::new)
            .insert((session_id.to_string(), plugin_id.to_string()), value.to_string());
    }
}

fn forget_remote_sessions_where(matcher: impl Fn(&(String, String)) -> bool) {
    if let Ok(mut guard) = REMOTE_SESSIONS.lock() {
        if let Some(map) = guard.as_mut() {
            map.retain(|key, _| !matcher(key));
        }
    }
}

// ─── Lifecycle ───────────────────────────────────────────────────────────────

/// Kill every plugin child of one ACP session. Called from BOTH teardown
/// paths in acp.rs (explicit stop and agent stdout EOF) — missing one leaks
/// children until process exit.
pub fn release_session(session_id: &str) {
    let victims: Vec<Arc<HostedChild>> = {
        let Ok(mut guard) = CHILDREN.lock() else { return };
        let Some(map) = guard.as_mut() else { return };
        let keys: Vec<(String, String)> = map
            .keys()
            .filter(|(s, _)| s == session_id)
            .cloned()
            .collect();
        keys.iter().filter_map(|key| map.remove(key)).collect()
    };
    for child in &victims {
        child.kill_and_reap();
    }
    forget_remote_sessions_where(|(s, _)| s == session_id);
    if !victims.is_empty() {
        crate::debug_log::append(
            "plugins",
            "info",
            session_id,
            "plugin children released",
            Some(&victims.len().to_string()),
        );
    }
}

/// Kill every child of one plugin across all sessions (disable/uninstall).
pub fn kill_plugin(plugin_id: &str) {
    let victims: Vec<Arc<HostedChild>> = {
        let Ok(mut guard) = CHILDREN.lock() else { return };
        let Some(map) = guard.as_mut() else { return };
        let keys: Vec<(String, String)> = map
            .keys()
            .filter(|(_, p)| p == plugin_id)
            .cloned()
            .collect();
        keys.iter().filter_map(|key| map.remove(key)).collect()
    };
    for child in &victims {
        child.kill_and_reap();
    }
    forget_remote_sessions_where(|(_, p)| p == plugin_id);
    if !victims.is_empty() {
        crate::debug_log::append(
            "plugins",
            "info",
            "",
            "plugin children killed",
            Some(plugin_id),
        );
    }
}

// ─── Probe (install-time / on-demand health check) ──────────────────────────

pub fn probe(plugin: &PluginRecord) -> ProbeRecord {
    let at = now_ms();
    match plugin.transport.as_str() {
        "stdio" => {
            let pseudo_session = format!("probe:{}", plugin.id);
            match spawn_plugin_child(&pseudo_session, plugin) {
                Ok(child) => {
                    let outcome = (|| -> Result<usize, RelayError> {
                        handshake(&child, None)?;
                        let result =
                            request_child(&child, "tools/list", json!({}), PROBE_TIMEOUT)?;
                        Ok(result
                            .get("tools")
                            .and_then(Value::as_array)
                            .map(|tools| tools.len())
                            .unwrap_or(0))
                    })();
                    let record = match outcome {
                        Ok(count) => ProbeRecord {
                            ok: true,
                            tool_count: Some(count),
                            note: format!("{count} tools"),
                            at: at.clone(),
                        },
                        Err(error) => ProbeRecord {
                            ok: false,
                            tool_count: None,
                            note: error.to_note(),
                            at,
                        },
                    };
                    child.kill_and_reap();
                    record
                }
                Err(error) => ProbeRecord {
                    ok: false,
                    tool_count: None,
                    note: error,
                    at,
                },
            }
        }
        "http" => {
            let pseudo_session = format!("probe:{}", plugin.id);
            let outcome = (|| -> Result<usize, RelayError> {
                let envelope = relay_http(
                    plugin,
                    &pseudo_session,
                    &json!({
                        "jsonrpc": "2.0", "id": 1, "method": "initialize",
                        "params": {
                            "protocolVersion": "2024-11-05",
                            "capabilities": {},
                            "clientInfo": { "name": "marionette-plugin-host", "version": "1.0.0" }
                        }
                    }),
                    "initialize",
                    false,
                )?;
                if let Some(error) = envelope.get("error") {
                    return Err(RelayError::Rpc(error.clone()));
                }
                let envelope = relay_http(
                    plugin,
                    &pseudo_session,
                    &json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}),
                    "tools/list",
                    false,
                )?;
                if let Some(error) = envelope.get("error") {
                    return Err(RelayError::Rpc(error.clone()));
                }
                Ok(envelope
                    .get("result")
                    .and_then(|result| result.get("tools"))
                    .and_then(Value::as_array)
                    .map(|tools| tools.len())
                    .unwrap_or(0))
            })();
            // The probe's remote-session pin must not linger in the map.
            forget_remote_sessions_where(|(s, p)| s == &pseudo_session && p == &plugin.id);
            match outcome {
                Ok(count) => ProbeRecord {
                    ok: true,
                    tool_count: Some(count),
                    note: format!("{count} tools"),
                    at,
                },
                Err(error) => ProbeRecord {
                    ok: false,
                    tool_count: None,
                    note: error.to_note(),
                    at,
                },
            }
        }
        other => ProbeRecord {
            ok: false,
            tool_count: None,
            note: format!("unsupported transport: {other}"),
            at,
        },
    }
}

fn now_ms() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn truncate_for_log(text: &str) -> String {
    if text.len() <= MAX_LOG_LINE {
        return text.to_string();
    }
    // Char-boundary safe (PTY mid-codepoint lesson).
    let mut cut = MAX_LOG_LINE;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &text[..cut])
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::mpsc::{Receiver, TryRecvError};

    /// Minimal in-memory duplex pipe (std::io::duplex is nightly-only).
    /// Byte-per-send is plenty for scripted tests; EOF surfaces when every
    /// writer end drops, exactly like a real pipe.
    fn channel_pipe() -> (ChannelWriter, ChannelReader) {
        let (tx, rx) = channel::<u8>();
        (ChannelWriter { tx }, ChannelReader { rx })
    }

    struct ChannelWriter {
        tx: Sender<u8>,
    }

    impl Write for ChannelWriter {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            for byte in buf {
                self.tx.send(*byte).map_err(|_| {
                    io::Error::new(io::ErrorKind::BrokenPipe, "channel closed")
                })?;
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    struct ChannelReader {
        rx: Receiver<u8>,
    }

    impl Read for ChannelReader {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if buf.is_empty() {
                return Ok(0);
            }
            // Block for the first byte (pipe semantics), then drain the rest.
            let first = self.rx.recv().map_err(|_| {
                io::Error::new(io::ErrorKind::UnexpectedEof, "channel closed")
            })?;
            buf[0] = first;
            let mut n = 1;
            while n < buf.len() {
                match self.rx.try_recv() {
                    Ok(byte) => {
                        buf[n] = byte;
                        n += 1;
                    }
                    Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
                }
            }
            Ok(n)
        }
    }

    fn fake_plugin() -> PluginRecord {
        PluginRecord {
            id: "fake".into(),
            transport: "stdio".into(),
            command: "unused-in-tests".into(),
            args: vec![],
            env: HashMap::new(),
            url: String::new(),
            headers: HashMap::new(),
            enabled: true,
            added_at: String::new(),
            last_probe: None,
        }
    }

    /// Wire attach() to a scripted fake child; returns the hosted handle.
    /// The fake answers initialize → tools/list → tools/call, optionally
    /// failing one method with a plugin-side JSON-RPC error, and can inject a
    /// server-initiated request to verify the host declines it with -32601.
    fn spawn_with_pipes(
        fail_method: Option<&'static str>,
        decline_check: bool,
    ) -> Arc<HostedChild> {
        let (agent_to_child, child_reads) = channel_pipe();
        let (child_writes, agent_from_child) = channel_pipe();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(child_reads);
            let mut writer = child_writes;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let Ok(message) = serde_json::from_str::<Value>(line.trim()) else {
                    continue;
                };
                let method = message.get("method").and_then(Value::as_str).unwrap_or("");
                let id = message.get("id").cloned().unwrap_or(Value::Null);
                if method.is_empty() {
                    continue; // our own -32601 replies to server requests
                }
                if decline_check && method == "tools/list" {
                    // Server-initiated request: the host must answer -32601.
                    let _ = writeln!(
                        writer,
                        "{}",
                        json!({"jsonrpc": "2.0", "id": 9001, "method": "sampling/createMessage", "params": {}})
                    );
                    let _ = writer.flush();
                    let mut reply = String::new();
                    if reader.read_line(&mut reply).unwrap_or(0) > 0 {
                        let reply: Value = serde_json::from_str(reply.trim()).unwrap();
                        assert!(reply.get("error").is_some(), "server request must be declined");
                        assert_eq!(
                            reply["error"]["code"].as_i64(),
                            Some(-32601),
                            "decline must be method-not-found"
                        );
                    }
                }
                let response = if Some(method) == fail_method {
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32001, "message": "plugin said no"}})
                } else {
                    match method {
                        "initialize" => json!({"jsonrpc": "2.0", "id": id, "result": {
                            "protocolVersion": "2024-11-05",
                            "capabilities": {"tools": {}},
                            "serverInfo": {"name": "fake", "version": "0"}
                        }}),
                        "tools/list" => json!({"jsonrpc": "2.0", "id": id, "result": {
                            "tools": [{"name": "echo"}, {"name": "ping"}]
                        }}),
                        "tools/call" => json!({"jsonrpc": "2.0", "id": id, "result": {
                            "content": [{"type": "text", "text": "pong"}], "isError": false
                        }}),
                        _ => json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "unknown"}}),
                    }
                };
                let _ = writeln!(writer, "{response}");
                let _ = writer.flush();
            }
        });
        attach(
            "test-session".into(),
            "fake".into(),
            None,
            agent_to_child,
            agent_from_child,
        )
    }

    #[test]
    fn relay_roundtrip_with_memory_child() {
        let child = spawn_with_pipes(None, false);
        let init = handshake(&child, None).expect("handshake");
        assert_eq!(init["serverInfo"]["name"], "fake");
        assert!(child.initialized.load(Ordering::Relaxed));
        let list = request_child(&child, "tools/list", json!({}), HANDSHAKE_TIMEOUT)
            .expect("tools/list");
        assert_eq!(list["tools"].as_array().map(Vec::len), Some(2));
        let call = request_child(
            &child,
            "tools/call",
            json!({"name": "echo", "arguments": {}}),
            HANDSHAKE_TIMEOUT,
        )
        .expect("tools/call");
        assert_eq!(call["content"][0]["text"], "pong");
        child.kill_and_reap();
    }

    #[test]
    fn plugin_rpc_errors_relay_verbatim() {
        let child = spawn_with_pipes(Some("tools/call"), false);
        handshake(&child, None).expect("handshake");
        let error = request_child(
            &child,
            "tools/call",
            json!({"name": "echo"}),
            HANDSHAKE_TIMEOUT,
        )
        .expect_err("must fail");
        match error {
            RelayError::Rpc(value) => {
                assert_eq!(value["code"], -32001);
                assert_eq!(value["message"], "plugin said no");
            }
            other => panic!("expected Rpc, got Host: {:?}", other.to_note()),
        }
        child.kill_and_reap();
    }

    #[test]
    fn server_initiated_requests_are_declined() {
        let child = spawn_with_pipes(None, true);
        handshake(&child, None).expect("handshake");
        let list = request_child(&child, "tools/list", json!({}), HANDSHAKE_TIMEOUT)
            .expect("tools/list");
        assert_eq!(list["tools"].as_array().map(Vec::len), Some(2));
        child.kill_and_reap();
    }

    #[test]
    fn kill_fails_pending_waiters_immediately() {
        let child = spawn_with_pipes(None, false);
        handshake(&child, None).expect("handshake");
        // Queue a call against a fake that never answers tools/call… the
        // scripted child answers everything, so simulate a hung waiter by
        // registering one directly.
        let (sender, receiver) = channel();
        child
            .pending
            .lock()
            .unwrap()
            .insert(999, sender);
        child.kill_and_reap();
        let outcome = receiver.recv_timeout(Duration::from_secs(1)).expect("waiter failed");
        assert!(outcome.is_err());
    }

    #[test]
    fn loopback_urls_are_detected() {
        assert!(is_loopback_url("http://127.0.0.1:123/mcp"));
        assert!(is_loopback_url("http://localhost:9/mcp"));
        assert!(is_loopback_url("http://[::1]:9/mcp"));
        assert!(!is_loopback_url("https://example.com/mcp"));
        assert!(!is_loopback_url("http://example.com/mcp"));
    }

    #[test]
    fn handshake_is_once_per_child() {
        let child = spawn_with_pipes(None, false);
        handshake(&child, None).expect("first handshake");
        let second = handshake(&child, None);
        assert!(second.is_err(), "second initialize must be rejected");
        child.kill_and_reap();
    }
}
