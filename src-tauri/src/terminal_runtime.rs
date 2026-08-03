//! ACP client `terminal/*` methods — spawn + capture for agent tool use.
//!
//! Modeled on Codeg's `terminal_runtime.rs`, but sync (`std::process` + threads)
//! to match Marionette's non-async ACP host. Grok Build and other agents call
//! `terminal/create` whenever they need a shell; advertising `terminal: true`
//! without these handlers makes plan mode / tools fail with
//! `Method not implemented by Marionette: terminal/create`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

const DEFAULT_OUTPUT_BYTE_LIMIT: usize = 1_000_000;
const READER_DRAIN_GRACE: Duration = Duration::from_millis(200);

#[derive(Debug)]
pub enum TerminalError {
    InvalidParams(String),
    Internal(String),
}

impl TerminalError {
    pub fn message(&self) -> &str {
        match self {
            Self::InvalidParams(m) | Self::Internal(m) => m.as_str(),
        }
    }

    pub fn code(&self) -> i32 {
        match self {
            Self::InvalidParams(_) => -32602,
            Self::Internal(_) => -32000,
        }
    }
}

#[derive(Clone, Default)]
struct TerminalSnapshot {
    output: String,
    truncated: bool,
    exit_code: Option<i32>,
    signal: Option<String>,
}

struct TerminalInstance {
    /// ACP agent session id (from create request / process), not UI session id.
    agent_session_id: String,
    output_limit: usize,
    child: Mutex<Option<Child>>,
    snapshot: Mutex<TerminalSnapshot>,
    readers_done: Arc<Mutex<u8>>,
}

impl TerminalInstance {
    fn new(agent_session_id: String, output_limit: usize, child: Child) -> Self {
        Self {
            agent_session_id,
            output_limit,
            child: Mutex::new(Some(child)),
            snapshot: Mutex::new(TerminalSnapshot::default()),
            readers_done: Arc::new(Mutex::new(0)),
        }
    }

    fn append_output(&self, text: &str) {
        let Ok(mut snap) = self.snapshot.lock() else {
            return;
        };
        snap.output.push_str(text);
        if snap.output.len() > self.output_limit {
            let overflow = snap.output.len() - self.output_limit;
            snap.output.drain(..overflow);
            snap.truncated = true;
        }
    }

    fn refresh_exit_status(&self) -> Result<(), TerminalError> {
        {
            let snap = self
                .snapshot
                .lock()
                .map_err(|_| TerminalError::Internal("snapshot lock poisoned".into()))?;
            if snap.exit_code.is_some() || snap.signal.is_some() {
                return Ok(());
            }
        }

        let maybe_status = {
            let mut guard = self
                .child
                .lock()
                .map_err(|_| TerminalError::Internal("child lock poisoned".into()))?;
            if let Some(child) = guard.as_mut() {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        *guard = None;
                        Some(status)
                    }
                    Ok(None) => None,
                    Err(err) => {
                        return Err(TerminalError::Internal(format!(
                            "failed to query terminal exit status: {err}"
                        )));
                    }
                }
            } else {
                None
            }
        };

        if let Some(status) = maybe_status {
            self.drain_readers();
            let mut snap = self
                .snapshot
                .lock()
                .map_err(|_| TerminalError::Internal("snapshot lock poisoned".into()))?;
            snap.exit_code = status.code();
            #[cfg(unix)]
            {
                use std::os::unix::process::ExitStatusExt;
                if status.code().is_none() {
                    if let Some(sig) = status.signal() {
                        snap.signal = Some(sig.to_string());
                    }
                }
            }
        }
        Ok(())
    }

    fn drain_readers(&self) {
        let start = std::time::Instant::now();
        while start.elapsed() < READER_DRAIN_GRACE {
            let done = self.readers_done.lock().map(|n| *n).unwrap_or(2);
            if done >= 2 {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_exit(&self) -> Result<(Option<i32>, Option<String>), TerminalError> {
        self.refresh_exit_status()?;
        {
            let snap = self
                .snapshot
                .lock()
                .map_err(|_| TerminalError::Internal("snapshot lock poisoned".into()))?;
            if snap.exit_code.is_some() || snap.signal.is_some() {
                return Ok((snap.exit_code, snap.signal.clone()));
            }
        }

        let status = {
            let mut guard = self
                .child
                .lock()
                .map_err(|_| TerminalError::Internal("child lock poisoned".into()))?;
            if let Some(mut child) = guard.take() {
                child.wait().map_err(|err| {
                    TerminalError::Internal(format!("failed to wait for terminal: {err}"))
                })?
            } else {
                let snap = self
                    .snapshot
                    .lock()
                    .map_err(|_| TerminalError::Internal("snapshot lock poisoned".into()))?;
                return Ok((snap.exit_code, snap.signal.clone()));
            }
        };

        self.drain_readers();
        let code = status.code();
        #[cfg(unix)]
        let signal = {
            use std::os::unix::process::ExitStatusExt;
            if code.is_none() {
                status.signal().map(|sig| sig.to_string())
            } else {
                None
            }
        };
        #[cfg(not(unix))]
        let signal = None;
        let mut snap = self
            .snapshot
            .lock()
            .map_err(|_| TerminalError::Internal("snapshot lock poisoned".into()))?;
        snap.exit_code = code;
        snap.signal = signal.clone();
        Ok((code, signal))
    }

    fn kill_command(&self) -> Result<(), TerminalError> {
        let mut guard = self
            .child
            .lock()
            .map_err(|_| TerminalError::Internal("child lock poisoned".into()))?;
        if let Some(child) = guard.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
            *guard = None;
        }
        Ok(())
    }

    fn snapshot_json(&self) -> Result<Value, TerminalError> {
        self.refresh_exit_status()?;
        let snap = self
            .snapshot
            .lock()
            .map_err(|_| TerminalError::Internal("snapshot lock poisoned".into()))?;
        let mut out = json!({
            "output": snap.output,
            "truncated": snap.truncated,
        });
        if snap.exit_code.is_some() || snap.signal.is_some() {
            out["exitStatus"] = exit_status_json(snap.exit_code, snap.signal.as_deref());
        }
        Ok(out)
    }
}

fn exit_status_json(code: Option<i32>, signal: Option<&str>) -> Value {
    let mut m = serde_json::Map::new();
    if let Some(c) = code {
        m.insert("exitCode".into(), json!(c));
    }
    if let Some(s) = signal {
        m.insert("signal".into(), json!(s));
    }
    Value::Object(m)
}

fn spawn_reader<R: Read + Send + 'static>(
    mut reader: R,
    terminal: Arc<TerminalInstance>,
    readers_done: Arc<Mutex<u8>>,
) {
    thread::spawn(move || {
        let mut buf = [0_u8; 4096];
        let mut pending = Vec::new();
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    if !pending.is_empty() {
                        let text = String::from_utf8_lossy(&pending);
                        terminal.append_output(&text);
                    }
                    break;
                }
                Ok(n) => {
                    pending.extend_from_slice(&buf[..n]);
                    // Decode complete UTF-8; keep partial trailing bytes.
                    match String::from_utf8(pending.clone()) {
                        Ok(text) => {
                            if !text.is_empty() {
                                terminal.append_output(&text);
                            }
                            pending.clear();
                        }
                        Err(err) => {
                            let valid = err.utf8_error().valid_up_to();
                            if valid > 0 {
                                let text = String::from_utf8_lossy(&pending[..valid]);
                                terminal.append_output(&text);
                                pending.drain(..valid);
                            }
                            // Incomplete sequence at end — wait for more bytes.
                            if err.utf8_error().error_len().is_some() && pending.len() > 4 {
                                // Invalid bytes — lossy drain one byte.
                                let text = String::from_utf8_lossy(&pending[..1]);
                                terminal.append_output(&text);
                                pending.drain(..1);
                            }
                        }
                    }
                }
                Err(_) => break,
            }
        }
        if let Ok(mut n) = readers_done.lock() {
            *n = n.saturating_add(1);
        }
    });
}

/// Shared terminal host for all ACP sessions.
pub struct TerminalRuntime {
    terminals: Mutex<HashMap<String, Arc<TerminalInstance>>>,
    /// Marionette UI session id → default cwd when agent omits cwd.
    session_cwd: Mutex<HashMap<String, PathBuf>>,
    next_id: AtomicU64,
}

impl Default for TerminalRuntime {
    fn default() -> Self {
        Self {
            terminals: Mutex::new(HashMap::new()),
            session_cwd: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }
}

impl TerminalRuntime {
    pub fn set_session_cwd(&self, marionette_session_id: &str, cwd: PathBuf) {
        if let Ok(mut map) = self.session_cwd.lock() {
            map.insert(marionette_session_id.to_string(), cwd);
        }
    }

    pub fn clear_session(&self, marionette_session_id: &str) {
        if let Ok(mut map) = self.session_cwd.lock() {
            map.remove(marionette_session_id);
        }
        // Kill terminals whose agent session id matches any stored under this
        // UI session is hard without a reverse map; release_all is called with
        // agent_session_id from process when available. Best-effort: kill all
        // if no agent id — caller uses release_all_for_agent.
    }

    pub fn release_all_for_agent(&self, agent_session_id: &str) {
        let removed: Vec<Arc<TerminalInstance>> = {
            let Ok(mut map) = self.terminals.lock() else {
                return;
            };
            let ids: Vec<String> = map
                .iter()
                .filter(|(_, t)| t.agent_session_id == agent_session_id)
                .map(|(id, _)| id.clone())
                .collect();
            ids.into_iter()
                .filter_map(|id| map.remove(&id))
                .collect()
        };
        for term in removed {
            let _ = term.kill_command();
        }
    }

    pub fn handle(
        &self,
        method: &str,
        params: &Value,
        marionette_session_id: &str,
        fallback_agent_session_id: Option<&str>,
    ) -> Result<Value, TerminalError> {
        match method {
            "terminal/create" => self.create(params, marionette_session_id, fallback_agent_session_id),
            "terminal/output" => self.output(params),
            "terminal/wait_for_exit" => self.wait_for_exit(params),
            "terminal/kill" => self.kill(params),
            "terminal/release" => self.release(params),
            other => Err(TerminalError::InvalidParams(format!(
                "unknown terminal method: {other}"
            ))),
        }
    }

    fn create(
        &self,
        params: &Value,
        marionette_session_id: &str,
        fallback_agent_session_id: Option<&str>,
    ) -> Result<Value, TerminalError> {
        let command = params
            .get("command")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                TerminalError::InvalidParams("terminal/create requires a non-empty command".into())
            })?;

        let args: Vec<String> = params
            .get("args")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        let agent_session_id = params
            .get("sessionId")
            .or_else(|| params.get("session_id"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| fallback_agent_session_id.map(str::to_string))
            .unwrap_or_else(|| marionette_session_id.to_string());

        let output_limit = params
            .get("outputByteLimit")
            .or_else(|| params.get("output_byte_limit"))
            .and_then(Value::as_u64)
            .map(|n| n as usize)
            .unwrap_or(DEFAULT_OUTPUT_BYTE_LIMIT);
        if output_limit == 0 {
            return Err(TerminalError::InvalidParams(
                "terminal/create outputByteLimit must be greater than 0".into(),
            ));
        }

        let cwd = resolve_cwd(params, &self.session_cwd, marionette_session_id)?;

        let mut child = match spawn_command(command, &args, cwd.as_deref(), params) {
            Ok(c) => c,
            Err(err) if args.is_empty() && command.contains(char::is_whitespace) => {
                // Codeg: agents sometimes pass a full shell line in `command`.
                spawn_shell_line(command, cwd.as_deref(), params).map_err(|e| {
                    TerminalError::Internal(format!(
                        "failed to spawn terminal command {command}: {err}; shell fallback: {e}"
                    ))
                })?
            }
            Err(err) => {
                return Err(TerminalError::Internal(format!(
                    "failed to spawn terminal command {command}: {err}"
                )));
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let terminal = Arc::new(TerminalInstance::new(
            agent_session_id,
            output_limit,
            child,
        ));
        let readers_done = Arc::clone(&terminal.readers_done);
        let mut reader_count = 0_u8;
        if let Some(out) = stdout {
            reader_count += 1;
            spawn_reader(out, Arc::clone(&terminal), Arc::clone(&readers_done));
        }
        if let Some(err) = stderr {
            reader_count += 1;
            spawn_reader(err, Arc::clone(&terminal), Arc::clone(&readers_done));
        }
        // If fewer than 2 streams, mark missing readers done so wait doesn't hang.
        if let Ok(mut n) = readers_done.lock() {
            *n = 2u8.saturating_sub(reader_count);
        }

        let id_num = self.next_id.fetch_add(1, Ordering::Relaxed);
        let terminal_id = format!("term_{id_num}");
        if let Ok(mut map) = self.terminals.lock() {
            map.insert(terminal_id.clone(), terminal);
        }

        Ok(json!({ "terminalId": terminal_id }))
    }

    fn find(&self, params: &Value) -> Result<Arc<TerminalInstance>, TerminalError> {
        let terminal_id = params
            .get("terminalId")
            .or_else(|| params.get("terminal_id"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                TerminalError::InvalidParams("terminal request requires terminalId".into())
            })?;
        let map = self
            .terminals
            .lock()
            .map_err(|_| TerminalError::Internal("terminals lock poisoned".into()))?;
        let term = map.get(terminal_id).cloned().ok_or_else(|| {
            TerminalError::InvalidParams(format!("terminal {terminal_id} not found"))
        })?;
        if let Some(sid) = params
            .get("sessionId")
            .or_else(|| params.get("session_id"))
            .and_then(Value::as_str)
        {
            if term.agent_session_id != sid {
                return Err(TerminalError::InvalidParams(format!(
                    "terminal {terminal_id} does not belong to session {sid}"
                )));
            }
        }
        Ok(term)
    }

    fn output(&self, params: &Value) -> Result<Value, TerminalError> {
        self.find(params)?.snapshot_json()
    }

    fn wait_for_exit(&self, params: &Value) -> Result<Value, TerminalError> {
        let term = self.find(params)?;
        let (code, signal) = term.wait_for_exit()?;
        Ok(json!({
            "exitStatus": exit_status_json(code, signal.as_deref())
        }))
    }

    fn kill(&self, params: &Value) -> Result<Value, TerminalError> {
        self.find(params)?.kill_command()?;
        Ok(json!({}))
    }

    fn release(&self, params: &Value) -> Result<Value, TerminalError> {
        let terminal_id = params
            .get("terminalId")
            .or_else(|| params.get("terminal_id"))
            .and_then(Value::as_str)
            .ok_or_else(|| {
                TerminalError::InvalidParams("terminal/release requires terminalId".into())
            })?;
        let term = {
            let mut map = self
                .terminals
                .lock()
                .map_err(|_| TerminalError::Internal("terminals lock poisoned".into()))?;
            map.remove(terminal_id).ok_or_else(|| {
                TerminalError::InvalidParams(format!("terminal {terminal_id} not found"))
            })?
        };
        if let Some(sid) = params
            .get("sessionId")
            .or_else(|| params.get("session_id"))
            .and_then(Value::as_str)
        {
            if term.agent_session_id != sid {
                // Put it back if session mismatch.
                if let Ok(mut map) = self.terminals.lock() {
                    map.insert(terminal_id.to_string(), Arc::clone(&term));
                }
                return Err(TerminalError::InvalidParams(format!(
                    "terminal {terminal_id} does not belong to session {sid}"
                )));
            }
        }
        term.kill_command()?;
        Ok(json!({}))
    }
}

fn resolve_cwd(
    params: &Value,
    session_cwd: &Mutex<HashMap<String, PathBuf>>,
    marionette_session_id: &str,
) -> Result<Option<PathBuf>, TerminalError> {
    if let Some(cwd) = params
        .get("cwd")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let path = PathBuf::from(cwd);
        if !path.is_absolute() {
            return Err(TerminalError::InvalidParams(
                "terminal/create requires an absolute cwd when provided".into(),
            ));
        }
        return Ok(Some(path));
    }
    let fallback = session_cwd
        .lock()
        .ok()
        .and_then(|m| m.get(marionette_session_id).cloned())
        .filter(|p| p.is_dir());
    Ok(fallback)
}

fn apply_env(cmd: &mut Command, params: &Value) {
    if let Some(env) = params.get("env").and_then(Value::as_array) {
        for item in env {
            let name = item
                .get("name")
                .or_else(|| item.get("key"))
                .and_then(Value::as_str);
            let value = item.get("value").and_then(Value::as_str);
            if let (Some(n), Some(v)) = (name, value) {
                cmd.env(n, v);
            }
        }
    }
    // Also accept object form { "KEY": "val" }
    if let Some(obj) = params.get("env").and_then(Value::as_object) {
        for (k, v) in obj {
            if let Some(s) = v.as_str() {
                cmd.env(k, s);
            }
        }
    }
}

fn configure_stdio(cmd: &mut Command, cwd: Option<&Path>) {
    cmd.stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    // Agent-requested terminals need the same widened PATH as the agents
    // themselves; an explicit PATH in params.env (applied after) still wins.
    #[cfg(not(target_os = "windows"))]
    {
        cmd.env("PATH", crate::process_util::env_path_for_children(""));
    }
}

fn spawn_command(
    command: &str,
    args: &[String],
    cwd: Option<&Path>,
    params: &Value,
) -> Result<Child, std::io::Error> {
    // Prefer PATH resolution helpers for Windows npm shims when no args look shell-like.
    let mut cmd = if args.is_empty() && !command.contains(char::is_whitespace) {
        match crate::process_util::resolve_spawn_command(command) {
            Ok(resolved) => {
                let mut c = Command::new(&resolved.program);
                resolved.apply_to(&mut c);
                c
            }
            Err(_) => Command::new(command),
        }
    } else {
        let mut c = Command::new(command);
        c.args(args);
        c
    };
    configure_stdio(&mut cmd, cwd);
    apply_env(&mut cmd, params);
    cmd.spawn()
}

fn spawn_shell_line(
    line: &str,
    cwd: Option<&Path>,
    params: &Value,
) -> Result<Child, std::io::Error> {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("cmd.exe");
        cmd.arg("/C").arg(line);
        configure_stdio(&mut cmd, cwd);
        apply_env(&mut cmd, params);
        cmd.spawn()
    }
    #[cfg(not(windows))]
    {
        let mut cmd = Command::new("/bin/sh");
        cmd.arg("-c").arg(line);
        configure_stdio(&mut cmd, cwd);
        apply_env(&mut cmd, params);
        cmd.spawn()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn create_echo_and_wait() {
        let rt = TerminalRuntime::default();
        let sid = "ui-sess";
        rt.set_session_cwd(sid, std::env::temp_dir());

        #[cfg(windows)]
        let command = "cmd.exe";
        #[cfg(windows)]
        let args = json!(["/C", "echo hello-marionette"]);
        #[cfg(not(windows))]
        let command = "echo";
        #[cfg(not(windows))]
        let args = json!(["hello-marionette"]);

        let created = rt
            .handle(
                "terminal/create",
                &json!({
                    "sessionId": "agent-1",
                    "command": command,
                    "args": args,
                }),
                sid,
                Some("agent-1"),
            )
            .expect("create");
        let term_id = created["terminalId"].as_str().expect("id");

        let waited = rt
            .handle(
                "terminal/wait_for_exit",
                &json!({ "sessionId": "agent-1", "terminalId": term_id }),
                sid,
                None,
            )
            .expect("wait");
        assert!(waited["exitStatus"]["exitCode"].as_i64().is_some());

        let out = rt
            .handle(
                "terminal/output",
                &json!({ "sessionId": "agent-1", "terminalId": term_id }),
                sid,
                None,
            )
            .expect("output");
        let text = out["output"].as_str().unwrap_or("");
        assert!(
            text.contains("hello-marionette"),
            "output was: {text:?}"
        );

        let _ = rt.handle(
            "terminal/release",
            &json!({ "sessionId": "agent-1", "terminalId": term_id }),
            sid,
            None,
        );
    }
}
