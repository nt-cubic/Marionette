use agent_client_protocol_schema::v1 as acp_schema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

const ACP_EVENT: &str = "acp-event";

// ─── Capability Types ──────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeDef {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDef {
    pub id: String,
    pub label: String,
    /// From ACP select option description (e.g. "Opus 4.8 with 1M context · …").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Build a human label from ACP model option name + description.
/// Claude surfaces family aliases (`opus`, `sonnet`) with the concrete generation
/// in the description — prefer that so the UI is not just bare "Opus".
fn model_display_label(name: &str, description: Option<&str>) -> String {
    let Some(desc) = description.map(str::trim).filter(|s| !s.is_empty()) else {
        return name.to_string();
    };
    // First clause before bullet / middot / price fragment
    let head = desc
        .split(['·', '•', '●', '|'])
        .next()
        .unwrap_or(desc)
        .split(" $")
        .next()
        .unwrap_or(desc)
        .trim();
    if head.is_empty() {
        return name.to_string();
    }
    let name_l = name.to_ascii_lowercase();
    let head_l = head.to_ascii_lowercase();
    if head_l == name_l || head_l.starts_with(&format!("{name_l} ")) {
        // "Opus 4.8 with 1M context" or exact match
        return head.to_string();
    }
    if name_l.contains("default") || name_l.contains("recommend") {
        // "Use the default model (currently Opus 4.8 (1M context))"
        if let Some(inner) = head
            .find("currently ")
            .map(|i| &head[i + "currently ".len()..])
        {
            let ver = inner.trim_end_matches(')').trim();
            if !ver.is_empty() {
                return format!("Default · {ver}");
            }
        }
        return format!("{name} · {head}");
    }
    // Short family name + richer head
    if name.len() <= 16 && head.len() > name.len() {
        return format!("{name} · {head}");
    }
    name.to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ThinkingEffort {
    pub min: f64,
    pub max: f64,
    pub default: f64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilitySnapshot {
    pub modes: Vec<ModeDef>,
    pub models: Vec<ModelDef>,
    pub thinking_effort: Option<ThinkingEffort>,
    /// Discrete effort levels (Claude: default/low/high/max). Prefer over numeric slider.
    pub effort_options: Vec<ModeDef>,
    pub supports_cancel: bool,
    pub current_mode: Option<String>,
    pub current_model: Option<String>,
    pub current_effort: Option<f64>,
    /// String effort id when the agent uses select options.
    pub current_effort_id: Option<String>,
    /// ACP config option ids used by session/set_config_option
    pub model_config_id: Option<String>,
    pub mode_config_id: Option<String>,
    pub effort_config_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AcpEvent {
    pub session_id: String,
    pub kind: String,
    pub method: Option<String>,
    pub data: Value,
}

struct AcpProcess {
    /// Outbound JSON-RPC queue. A dedicated writer thread owns the pipe, so a
    /// wedged agent that stops draining its stdin can never block a caller —
    /// `session/cancel` in particular has to get through while a turn is stuck.
    stdin_tx: Mutex<Sender<Value>>,
    child: Mutex<Child>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, Sender<Result<Value, String>>>>>,
    agent_session_id: Mutex<Option<String>>,
    /// Set when AgentShell intentionally stops the process (agent switch / delete).
    /// Suppresses false "process/ended" crash UX on the UI.
    intentional_stop: AtomicBool,
}

struct PendingPermission {
    process: Arc<AcpProcess>,
    rpc_id: Value,
    options: Value,
}

#[derive(Clone, Default)]
pub struct AcpService {
    sessions: Arc<Mutex<HashMap<String, Arc<AcpProcess>>>>,
    capabilities: Arc<Mutex<HashMap<String, CapabilitySnapshot>>>,
    /// UI-gated `session/request_permission` waiters, keyed by request_id.
    permissions: Arc<Mutex<HashMap<String, PendingPermission>>>,
    permission_seq: Arc<AtomicU64>,
}

impl AcpService {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start(
        &self,
        app: AppHandle,
        session_id: String,
        command: String,
        args: Vec<String>,
        cwd: String,
        // Which agent this dialog is bound to — decides what gets lent to it.
        agent_id: Option<String>,
    ) -> Result<CapabilitySnapshot, String> {
        if !std::path::Path::new(&cwd).is_dir() {
            return Err(format!("ACP cwd is not a directory: {cwd}"));
        }

        {
            let sessions = self
                .sessions
                .lock()
                .map_err(|_| "ACP session lock poisoned".to_string())?;
            if let Some(process) = sessions.get(&session_id) {
                let alive = process
                    .child
                    .lock()
                    .ok()
                    .and_then(|mut child| child.try_wait().ok())
                    .map(|status| status.is_none())
                    .unwrap_or(false);
                if alive {
                    if let Some(caps) = self.get_capabilities(&session_id) {
                        return Ok(caps);
                    }
                }
            }
        }
        // Dead process, missing caps, or partial warm — kill and restart
        let _ = self.stop(&session_id);

        // Prefer global ACP bins over `npx -y …` (npx cold start can hang UI for minutes).
        let (command, args) =
            crate::process_util::prefer_fast_acp_launch(&command, &args);

        let _ = app.emit(
            ACP_EVENT,
            AcpEvent {
                session_id: session_id.clone(),
                kind: "system".to_string(),
                method: Some("session/starting".to_string()),
                data: json!({
                    "phase": "spawn",
                    "command": command,
                    "args": args,
                    "hint": if command.eq_ignore_ascii_case("npx") {
                        "First launch via npx can take 1–3 minutes while packages download."
                    } else {
                        "Starting agent process…"
                    }
                }),
            },
        );

        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "ACP session lock poisoned".to_string())?;

        // Windows npm shims (`opencode` without .exe) fail with "program not found"
        // under CreateProcess. Resolve to a real executable (or cmd.exe /C .cmd).
        let resolved = crate::process_util::resolve_spawn_command(&command).map_err(|error| {
            format!(
                "Start ACP agent failed: {error}. \
                 Tip: install the adapter globally once: npm i -g @agentclientprotocol/codex-acp \
                 (or @agentclientprotocol/claude-agent-acp) so AgentShell can skip slow npx."
            )
        })?;
        let mut child_command = Command::new(&resolved.program);
        resolved.apply_to(&mut child_command);
        child_command
            .args(&args)
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Ensure npm global bin stays on PATH for nested tools
        #[cfg(target_os = "windows")]
        {
            if let Some(appdata) = std::env::var_os("APPDATA") {
                let npm_bin = std::path::PathBuf::from(appdata).join("npm");
                if npm_bin.is_dir() {
                    let mut path = std::env::var_os("PATH").unwrap_or_default();
                    let npm_str = npm_bin.to_string_lossy();
                    if !path.to_string_lossy().to_ascii_lowercase().contains(&npm_str.to_ascii_lowercase()) {
                        let mut new_path = npm_bin.into_os_string();
                        new_path.push(";");
                        new_path.push(&path);
                        path = new_path;
                    }
                    child_command.env("PATH", path);
                }
            }
        }
        // Folders the user granted outside the project. opencode gates these
        // behind its `external_directory` permission and reads the grant from
        // this env var — the only lever that works for a *subagent*, whose
        // permission prompt never reaches us (ACP `additionalDirectories` is
        // accepted but does not widen the scope; measured both ways).
        if agent_id.as_deref() == Some("opencode") {
            let existing = std::env::var("OPENCODE_PERMISSION").ok();
            if let Some(value) = crate::context_inventory::opencode_permission_env(
                std::path::Path::new(&cwd),
                existing.as_deref(),
            ) {
                crate::debug_log::append(
                    "context",
                    "info",
                    &session_id,
                    "OPENCODE_PERMISSION set for granted folders",
                    Some(&value),
                );
                child_command.env("OPENCODE_PERMISSION", value);
            }
        }

        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            child_command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = child_command.spawn().map_err(|error| {
            format!(
                "Start ACP agent failed: {error} (resolved `{}` → `{}`)",
                command, resolved.resolved_path
            )
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "ACP agent stdin is unavailable".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "ACP agent stdout is unavailable".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "ACP agent stderr is unavailable".to_string())?;
        let pending = Arc::new(Mutex::new(HashMap::new()));
        let stdin_tx = {
            // A failed write happens after its caller already got `Ok`, so the
            // only way it reaches anyone is from in here.
            let fail_app = app.clone();
            let fail_session = session_id.clone();
            let fail_pending: Arc<Mutex<HashMap<u64, Sender<Result<Value, String>>>>> =
                Arc::clone(&pending);
            spawn_stdin_writer(stdin, session_id.clone(), move |error| {
                if let Ok(mut waiters) = fail_pending.lock() {
                    for (_, sender) in waiters.drain() {
                        let _ = sender.send(Err(format!("ACP agent stdin closed: {error}")));
                    }
                }
                // Same event the reader emits on stdout EOF — the UI already
                // knows how to unstick a session from it. Needed as its own
                // signal because stdin can break while stdout stays open (a
                // grandchild still holds the handle), and then no EOF arrives.
                emit_event(
                    &fail_app,
                    &fail_session,
                    "error",
                    Some("process/ended"),
                    json!({
                        "message": format!("Agent stopped reading input ({error}). The turn was not delivered."),
                        "reason": "stdin-closed"
                    }),
                );
            })
        };
        let process = Arc::new(AcpProcess {
            stdin_tx: Mutex::new(stdin_tx),
            child: Mutex::new(child),
            next_id: AtomicU64::new(1),
            pending: Arc::clone(&pending),
            agent_session_id: Mutex::new(None),
            intentional_stop: AtomicBool::new(false),
        });
        sessions.insert(session_id.clone(), Arc::clone(&process));
        drop(sessions);

        // Must exist before the readers start, or their first lines would be
        // dispatched inline on the reader thread — the exact thing to avoid.
        register_emitter(&session_id, &app);

        let reader_app = app.clone();
        let reader_session = session_id.clone();
        let reader_process = Arc::clone(&process);
        let permissions = Arc::clone(&self.permissions);
        let permission_seq = Arc::clone(&self.permission_seq);
        thread::spawn(move || {
            read_stdout(
                reader_app,
                reader_session,
                stdout,
                reader_process,
                pending,
                permissions,
                permission_seq,
            )
        });
        let stderr_app = app.clone();
        let stderr_session = session_id.clone();
        thread::spawn(move || read_stderr(stderr_app, stderr_session, stderr));

        let _ = app.emit(
            ACP_EVENT,
            AcpEvent {
                session_id: session_id.clone(),
                kind: "system".to_string(),
                method: Some("session/starting".to_string()),
                data: json!({
                    "phase": "initialize",
                    "hint": "Handshake with agent (initialize)…"
                }),
            },
        );

        // ── Phase 1: initialize (advertise client capabilities like Zed) ──
        let initialized = request(
            &process,
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {
                    "fs": {
                        "readTextFile": true,
                        "writeTextFile": true
                    },
                    "terminal": true,
                    "session": {
                        "configOptions": {
                            "boolean": {}
                        }
                    }
                },
                "clientInfo": { "name": "AgentShell", "version": "0.1.0" }
            }),
        );
        let initialized = match initialized {
            Ok(value) => value,
            Err(error) => {
                let _ = self.stop(&session_id);
                return Err(format!("ACP initialize failed: {error}"));
            }
        };

        // Project context (needs 1: what this agent is missing). Remote transports
        // are only offered when the agent said it can take them.
        let mcp_caps = initialized
            .get("agentCapabilities")
            .and_then(|caps| caps.get("mcpCapabilities"));
        let supports_http = mcp_caps
            .and_then(|caps| caps.get("http"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let supports_sse = mcp_caps
            .and_then(|caps| caps.get("sse"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let (mcp_servers, mcp_skipped) = match agent_id.as_deref() {
            Some(agent) => crate::context_inventory::mcp_payload_for_agent(
                std::path::Path::new(&cwd),
                agent,
                supports_http,
                supports_sse,
            ),
            None => (Vec::new(), Vec::new()),
        };
        if !mcp_servers.is_empty() || !mcp_skipped.is_empty() {
            // Names only — an MCP env can hold API tokens.
            let injected: Vec<String> = mcp_servers
                .iter()
                .filter_map(|server| server.get("name").and_then(Value::as_str))
                .map(str::to_string)
                .collect();
            let skipped_note = if mcp_skipped.is_empty() {
                String::new()
            } else {
                format!("skipped: {}", mcp_skipped.join(", "))
            };
            crate::debug_log::append(
                "context",
                "info",
                &session_id,
                &format!("mcp inject: {}", injected.join(", ")),
                if skipped_note.is_empty() {
                    None
                } else {
                    Some(&skipped_note)
                },
            );
        }

        let _ = app.emit(
            ACP_EVENT,
            AcpEvent {
                session_id: session_id.clone(),
                kind: "system".to_string(),
                method: Some("session/starting".to_string()),
                data: json!({
                    "phase": "session_new",
                    "hint": "Creating agent session…"
                }),
            },
        );

        // ── Phase 2: session/new ────────────────────────────────────────
        // Folders the user granted outside the project. Must be set here: a
        // subagent's permission prompt never reaches us, so scope has to be
        // agreed before the turn starts.
        let extra_roots = crate::context_inventory::workspace_roots(std::path::Path::new(&cwd));
        if !extra_roots.is_empty() {
            crate::debug_log::append(
                "context",
                "info",
                &session_id,
                &format!("additionalDirectories: {}", extra_roots.join(" · ")),
                None,
            );
        }

        let new_session = match request(
            &process,
            "session/new",
            json!({
                "cwd": cwd,
                "mcpServers": mcp_servers,
                "additionalDirectories": extra_roots,
            }),
        ) {
            Ok(value) => value,
            Err(error) => {
                let _ = self.stop(&session_id);
                return Err(format!("ACP session/new failed: {error}"));
            }
        };
        let agent_session_id = new_session
            .get("sessionId")
            .and_then(Value::as_str)
            .ok_or_else(|| "ACP session/new response did not contain sessionId".to_string())?
            .to_string();
        *process
            .agent_session_id
            .lock()
            .map_err(|_| "ACP session id lock poisoned".to_string())? =
            Some(agent_session_id.clone());

        let caps = parse_session_capabilities(&new_session);
        if let Ok(mut caps_map) = self.capabilities.lock() {
            caps_map.insert(session_id.clone(), caps.clone());
        }

        let _ = app.emit(
            ACP_EVENT,
            AcpEvent {
                session_id: session_id.clone(),
                kind: "system".to_string(),
                method: Some("session/ready".to_string()),
                data: json!({
                    "sessionId": agent_session_id,
                    "configOptions": new_session.get("configOptions").cloned().unwrap_or(Value::Null),
                    "modes": new_session.get("modes").cloned().unwrap_or(Value::Null),
                    "capabilities": caps,
                }),
            },
        );
        Ok(caps)
    }

    /// Kill every agent process we own — nothing should outlive the window.
    pub fn stop_all(&self) -> usize {
        let ids: Vec<String> = self
            .sessions
            .lock()
            .map(|sessions| sessions.keys().cloned().collect())
            .unwrap_or_default();
        let count = ids.len();
        for id in ids {
            let _ = self.stop(&id);
        }
        count
    }

    /// Is an agent process for this dialog alive in *this* app run?
    pub fn is_live(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .map(|map| map.contains_key(session_id))
            .unwrap_or(false)
    }

    pub fn get_capabilities(&self, session_id: &str) -> Option<CapabilitySnapshot> {
        self.capabilities
            .lock()
            .ok()
            .and_then(|map| map.get(session_id).cloned())
    }

    /// Apply a session config change via ACP `session/set_config_option`.
    ///
    /// Accepted shapes:
    /// - `{ "configId": "model", "value": "provider/model" }`
    /// - `{ "model": "provider/model" }`
    /// - `{ "mode": "build" }`
    /// - `{ "thinkingEffort": 0.5 }` (maps to effort config id if present)
    pub fn update_session(&self, session_id: &str, config: Value) -> Result<Value, String> {
        let process = self.process(session_id)?;
        let agent_session_id = process
            .agent_session_id
            .lock()
            .map_err(|_| "ACP session id lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| "ACP session is not initialized".to_string())?;

        let caps = self.get_capabilities(session_id);
        let updates = expand_config_updates(&config, caps.as_ref())?;
        if updates.is_empty() {
            return Err("No config options to update".to_string());
        }

        let mut last_result = Value::Null;
        for (config_id, value) in updates {
            let params = json!({
                "sessionId": agent_session_id,
                "configId": config_id,
                "value": value,
            });
            last_result = request(&process, "session/set_config_option", params)?;

            // Prefer refreshed options from the response when present
            if let Some(refreshed) = last_result.get("configOptions") {
                let mut refreshed_session = json!({
                    "sessionId": agent_session_id,
                    "configOptions": refreshed,
                });
                if let Some(modes) = last_result.get("modes") {
                    refreshed_session["modes"] = modes.clone();
                }
                let next_caps = parse_session_capabilities(&refreshed_session);
                if let Ok(mut caps_map) = self.capabilities.lock() {
                    caps_map.insert(session_id.to_string(), next_caps);
                }
            } else if let Ok(mut caps_map) = self.capabilities.lock() {
                if let Some(caps) = caps_map.get_mut(session_id) {
                    apply_local_config_change(caps, &config_id, &value);
                }
            }
        }
        Ok(last_result)
    }

    /// Resolve a UI-gated `session/request_permission` with the chosen optionId.
    pub fn respond_permission(
        &self,
        request_id: &str,
        option_id: &str,
    ) -> Result<(), String> {
        let pending = {
            let mut map = self
                .permissions
                .lock()
                .map_err(|_| "Permission lock poisoned".to_string())?;
            map.remove(request_id)
                .ok_or_else(|| format!("Unknown or expired permission request: {request_id}"))?
        };
        write_response(
            &pending.process,
            pending.rpc_id,
            json!({
                "outcome": {
                    "outcome": "selected",
                    "optionId": option_id
                }
            }),
        );
        Ok(())
    }

    /// Fire `session/prompt` and return immediately so the UI can stream
    /// `session/update` events (thinking / tool_call / message chunks) live.
    /// Turn completion arrives later as an `rpc/response` acp-event.
    pub fn send_prompt(&self, session_id: &str, text: String) -> Result<Value, String> {
        let process = self.process(session_id)?;
        let agent_session_id = process
            .agent_session_id
            .lock()
            .map_err(|_| "ACP session id lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| "ACP session is not initialized".to_string())?;

        let id = process.next_id.fetch_add(1, Ordering::Relaxed);
        // No pending waiter: response is still emitted on the event bus in read_stdout.
        write_message(
            &process,
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "session/prompt",
                "params": {
                    "sessionId": agent_session_id,
                    "prompt": [{ "type": "text", "text": text }]
                }
            }),
        )?;
        crate::debug_log::append(
            "acp",
            "info",
            session_id,
            "session/prompt sent (async stream)",
            Some(&format!("rpcId={id} chars={}", text.len())),
        );
        Ok(json!({ "accepted": true, "id": id }))
    }

    pub fn cancel(&self, session_id: &str) -> Result<(), String> {
        let process = self.process(session_id)?;
        let agent_session_id = process
            .agent_session_id
            .lock()
            .map_err(|_| "ACP session id lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| "ACP session is not initialized".to_string())?;
        notify(
            &process,
            "session/cancel",
            json!({ "sessionId": agent_session_id }),
        )
    }

    pub fn stop(&self, session_id: &str) -> Result<(), String> {
        let process = self
            .sessions
            .lock()
            .map_err(|_| "ACP session lock poisoned".to_string())?
            .remove(session_id);
        let _ = self
            .capabilities
            .lock()
            .map(|mut map| map.remove(session_id));
        if let Some(process) = process {
            process.intentional_stop.store(true, Ordering::SeqCst);
            let mut child = process
                .child
                .lock()
                .map_err(|_| "ACP child lock poisoned".to_string())?;
            let _ = child.kill();
            let _ = child.wait();
        }
        // Dropping the sender lets the dispatch thread flush its backlog and
        // exit. Done here rather than in the reader's teardown so a restart
        // cannot race and unregister the *new* session's queue.
        unregister_emitter(session_id);
        Ok(())
    }

    fn process(&self, session_id: &str) -> Result<Arc<AcpProcess>, String> {
        self.sessions
            .lock()
            .map_err(|_| "ACP session lock poisoned".to_string())?
            .get(session_id)
            .cloned()
            .ok_or_else(|| "ACP session is not running".to_string())
    }
}

fn expand_config_updates(
    config: &Value,
    caps: Option<&CapabilitySnapshot>,
) -> Result<Vec<(String, Value)>, String> {
    let obj = config
        .as_object()
        .ok_or_else(|| "Config update must be a JSON object".to_string())?;

    if let (Some(config_id), Some(value)) = (obj.get("configId"), obj.get("value")) {
        let id = config_id
            .as_str()
            .ok_or_else(|| "configId must be a string".to_string())?
            .to_string();
        return Ok(vec![(id, value.clone())]);
    }

    let mut updates = Vec::new();
    if let Some(model) = obj.get("model") {
        let id = caps
            .and_then(|c| c.model_config_id.clone())
            .unwrap_or_else(|| "model".to_string());
        updates.push((id, model.clone()));
    }
    if let Some(mode) = obj.get("mode") {
        let id = caps
            .and_then(|c| c.mode_config_id.clone())
            .unwrap_or_else(|| "mode".to_string());
        updates.push((id, mode.clone()));
    }
    if let Some(effort) = obj.get("thinkingEffort") {
        let id = caps
            .and_then(|c| c.effort_config_id.clone())
            .ok_or_else(|| "Agent does not expose a thinking/effort config option".to_string())?;
        // Prefer string form when agent uses select options
        let value = if let Some(n) = effort.as_f64() {
            // Keep numeric if that's what we got; many agents use string ids
            json!(n)
        } else {
            effort.clone()
        };
        updates.push((id, value));
    }
    Ok(updates)
}

fn apply_local_config_change(caps: &mut CapabilitySnapshot, config_id: &str, value: &Value) {
    let text = value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_f64().map(|n| n.to_string()));
    if Some(config_id) == caps.model_config_id.as_deref() || config_id == "model" {
        if let Some(text) = text.clone() {
            caps.current_model = Some(text);
        }
    }
    if Some(config_id) == caps.mode_config_id.as_deref() || config_id == "mode" {
        if let Some(text) = text.clone() {
            caps.current_mode = Some(text);
        }
    }
    if Some(config_id) == caps.effort_config_id.as_deref()
        || config_id == "effort"
        || config_id == "thought_level"
    {
        if let Some(n) = value.as_f64() {
            caps.current_effort = Some(n);
            caps.current_effort_id = Some(n.to_string());
        } else if let Some(text) = text {
            caps.current_effort_id = Some(text.clone());
            caps.current_effort = text.parse().ok();
        }
    }
}

// ─── Capability parsing from session/new response ──────────────────────────

fn parse_session_capabilities(session_response: &Value) -> CapabilitySnapshot {
    if let Ok(response) =
        serde_json::from_value::<acp_schema::NewSessionResponse>(session_response.clone())
    {
        return parse_capabilities_from_typed(response);
    }
    parse_capabilities_fallback(session_response)
}

fn parse_capabilities_from_typed(response: acp_schema::NewSessionResponse) -> CapabilitySnapshot {
    use acp_schema::SessionConfigKind as Kind;
    use acp_schema::SessionConfigOptionCategory as Cat;

    let mut modes: Vec<ModeDef> = Vec::new();
    let mut models: Vec<ModelDef> = Vec::new();
    let mut effort_options: Vec<ModeDef> = Vec::new();
    let mut current_mode: Option<String> = None;
    let mut current_model: Option<String> = None;
    let mut current_effort: Option<f64> = None;
    let mut current_effort_id: Option<String> = None;
    let mut thinking_effort: Option<ThinkingEffort> = None;
    let mut model_config_id: Option<String> = None;
    let mut mode_config_id: Option<String> = None;
    let mut effort_config_id: Option<String> = None;

    /// (value, name, optional description)
    fn extract_options(
        options: &acp_schema::SessionConfigSelectOptions,
    ) -> Vec<(String, String, Option<String>)> {
        match options {
            acp_schema::SessionConfigSelectOptions::Ungrouped(list) => list
                .iter()
                .map(|o| {
                    (
                        o.value.0.to_string(),
                        o.name.clone(),
                        o.description.clone(),
                    )
                })
                .collect(),
            acp_schema::SessionConfigSelectOptions::Grouped(groups) => groups
                .iter()
                .flat_map(|g| &g.options)
                .map(|o| {
                    (
                        o.value.0.to_string(),
                        o.name.clone(),
                        o.description.clone(),
                    )
                })
                .collect(),
            _ => vec![],
        }
    }

    // Legacy modes field on NewSessionResponse
    if let Some(mode_state) = &response.modes {
        current_mode = Some(mode_state.current_mode_id.0.to_string());
        for mode in &mode_state.available_modes {
            modes.push(ModeDef {
                id: mode.id.0.to_string(),
                label: mode.name.clone(),
            });
        }
        if mode_config_id.is_none() {
            mode_config_id = Some("mode".to_string());
        }
    }

    if let Some(config_options) = &response.config_options {
        for opt in config_options {
            let id_str = opt.id.0.to_string();
            let category = opt.category.clone();
            match category {
                Some(Cat::Mode) => {
                    mode_config_id = Some(id_str.clone());
                    if let Kind::Select(select) = &opt.kind {
                        let extracted = extract_options(&select.options);
                        if !extracted.is_empty() {
                            modes = extracted
                                .into_iter()
                                .map(|(id, name, _)| ModeDef { id, label: name })
                                .collect();
                        }
                        current_mode = Some(select.current_value.0.to_string());
                    }
                }
                Some(Cat::Model) => {
                    model_config_id = Some(id_str.clone());
                    if let Kind::Select(select) = &opt.kind {
                        let raw = extract_options(&select.options);
                        // DEBUG: log raw model options from ACP typed path
                        for (mid, mname, mdesc) in &raw {
                            crate::debug_log::append(
                                "acp",
                                "debug",
                                "",
                                "raw_model_option",
                                Some(&format!("id={mid} name={mname} desc={:?}", mdesc.as_deref().unwrap_or("(none)"))),
                            );
                        }
                        models = raw
                            .into_iter()
                            .map(|(id, name, desc)| ModelDef {
                                id,
                                label: model_display_label(&name, desc.as_deref()),
                                description: desc,
                            })
                            .collect();
                        current_model = Some(select.current_value.0.to_string());
                    }
                }
                Some(Cat::ThoughtLevel) => {
                    effort_config_id = Some(id_str.clone());
                    if let Kind::Select(select) = &opt.kind {
                        let extracted = extract_options(&select.options);
                        effort_options = extracted
                            .iter()
                            .map(|(id, name, _)| ModeDef {
                                id: id.clone(),
                                label: name.clone(),
                            })
                            .collect();
                        let vals: Vec<f64> = extracted
                            .iter()
                            .filter_map(|(v, _, _)| v.parse::<f64>().ok())
                            .collect();
                        if !vals.is_empty() {
                            let min_val = vals.iter().cloned().fold(f64::INFINITY, f64::min);
                            let max_val = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                            thinking_effort = Some(ThinkingEffort {
                                min: min_val,
                                max: max_val,
                                default: min_val + (max_val - min_val) * 0.5,
                            });
                        }
                        let cur = select.current_value.0.to_string();
                        current_effort_id = Some(cur.clone());
                        current_effort = cur.parse::<f64>().ok();
                    }
                }
                _ => {
                    let lower = id_str.to_lowercase();
                    if lower.contains("effort")
                        || lower.contains("thinking")
                        || lower.contains("thought")
                    {
                        effort_config_id = Some(id_str);
                        if let Kind::Select(select) = &opt.kind {
                            let extracted = extract_options(&select.options);
                            if effort_options.is_empty() {
                                effort_options = extracted
                                    .iter()
                                    .map(|(id, name, _)| ModeDef {
                                        id: id.clone(),
                                        label: name.clone(),
                                    })
                                    .collect();
                            }
                            let vals: Vec<f64> = extracted
                                .iter()
                                .filter_map(|(v, _, _)| v.parse::<f64>().ok())
                                .collect();
                            if !vals.is_empty() {
                                let min_val = vals.iter().cloned().fold(f64::INFINITY, f64::min);
                                let max_val =
                                    vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                                thinking_effort = Some(ThinkingEffort {
                                    min: min_val,
                                    max: max_val,
                                    default: min_val + (max_val - min_val) * 0.5,
                                });
                            }
                            let cur = select.current_value.0.to_string();
                            current_effort_id = Some(cur.clone());
                            current_effort = cur.parse::<f64>().ok();
                        }
                    }
                }
            }
        }
    }

    // Ensure current model appears even if not in options list
    if let Some(current) = &current_model {
        if !models.iter().any(|m| &m.id == current) {
            models.insert(
                0,
                ModelDef {
                    id: current.clone(),
                    label: current.clone(),
                    description: None,
                },
            );
        }
    }

    CapabilitySnapshot {
        modes,
        models,
        thinking_effort,
        effort_options,
        supports_cancel: true,
        current_mode,
        current_model,
        current_effort,
        current_effort_id,
        model_config_id,
        mode_config_id,
        effort_config_id,
    }
}

fn parse_capabilities_fallback(session_response: &Value) -> CapabilitySnapshot {
    let config_options = session_response
        .get("configOptions")
        .and_then(|c| c.as_array());

    let mut modes: Vec<ModeDef> = Vec::new();
    let mut models: Vec<ModelDef> = Vec::new();
    let mut effort_options: Vec<ModeDef> = Vec::new();
    let mut thinking_effort: Option<ThinkingEffort> = None;
    let mut current_mode: Option<String> = None;
    let mut current_model: Option<String> = None;
    let mut current_effort: Option<f64> = None;
    let mut current_effort_id: Option<String> = None;
    let mut model_config_id: Option<String> = None;
    let mut mode_config_id: Option<String> = None;
    let mut effort_config_id: Option<String> = None;

    if let Some(options) = config_options {
        for opt in options {
            let id = opt.get("id").and_then(Value::as_str).unwrap_or("").to_string();
            let category = opt
                .get("category")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let current_value = opt
                .get("currentValue")
                .and_then(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .or_else(|| v.as_f64().map(|n| n.to_string()))
                        .or_else(|| v.as_bool().map(|b| b.to_string()))
                });

            // (value, name, description)
            let select_options = opt
                .get("options")
                .and_then(|o| o.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|item| {
                            // Flat options or one level of groups
                            if item.get("options").and_then(|o| o.as_array()).is_some() {
                                return None; // handled below via flatten - skip group headers
                            }
                            let value = item.get("value")?.as_str()?.to_string();
                            let name = item
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or(value.as_str())
                                .to_string();
                            let description = item
                                .get("description")
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            Some((value, name, description))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| {
                    // Grouped: options[].options[]
                    opt.get("options")
                        .and_then(|o| o.as_array())
                        .map(|arr| {
                            arr.iter()
                                .flat_map(|item| {
                                    item.get("options")
                                        .and_then(|o| o.as_array())
                                        .into_iter()
                                        .flatten()
                                        .filter_map(|inner| {
                                            let value = inner.get("value")?.as_str()?.to_string();
                                            let name = inner
                                                .get("name")
                                                .and_then(Value::as_str)
                                                .unwrap_or(value.as_str())
                                                .to_string();
                                            let description = inner
                                                .get("description")
                                                .and_then(Value::as_str)
                                                .map(str::to_string);
                                            Some((value, name, description))
                                        })
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default()
                });

            match category.as_str() {
                "mode" => {
                    mode_config_id = Some(id);
                    if !select_options.is_empty() {
                        modes = select_options
                            .into_iter()
                            .map(|(id, name, _)| ModeDef { id, label: name })
                            .collect();
                    }
                    current_mode = current_value;
                }
                "model" => {
                    model_config_id = Some(id);
                    if !select_options.is_empty() {
                        // DEBUG: log raw model options from ACP fallback path (category="model")
                        for (mid, mname, mdesc) in &select_options {
                            crate::debug_log::append(
                                "acp",
                                "debug",
                                "",
                                "raw_model_option",
                                Some(&format!("id={mid} name={mname} desc={:?}", mdesc.as_deref().unwrap_or("(none)"))),
                            );
                        }
                        models = select_options
                            .into_iter()
                            .map(|(id, name, desc)| ModelDef {
                                id,
                                label: model_display_label(&name, desc.as_deref()),
                                description: desc,
                            })
                            .collect();
                    }
                    current_model = current_value;
                }
                "thought_level" | "effort" | "thinking" => {
                    effort_config_id = Some(id);
                    if !select_options.is_empty() {
                        effort_options = select_options
                            .iter()
                            .map(|(id, name, _)| ModeDef {
                                id: id.clone(),
                                label: name.clone(),
                            })
                            .collect();
                        let vals: Vec<f64> = select_options
                            .iter()
                            .filter_map(|(v, _, _)| v.parse::<f64>().ok())
                            .collect();
                        if !vals.is_empty() {
                            let min_val = vals.iter().cloned().fold(f64::INFINITY, f64::min);
                            let max_val = vals.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
                            thinking_effort = Some(ThinkingEffort {
                                min: min_val,
                                max: max_val,
                                default: min_val + (max_val - min_val) * 0.5,
                            });
                        }
                    }
                    current_effort_id = current_value.clone();
                    current_effort = current_value.and_then(|v| v.parse().ok());
                }
                _ => {
                    let lower = id.to_lowercase();
                    if lower.contains("model") {
                        model_config_id = Some(id);
                        if !select_options.is_empty() {
                            // DEBUG: log raw model options from ACP fallback path (generic "model")
                            for (mid, mname, mdesc) in &select_options {
                                crate::debug_log::append(
                                    "acp",
                                    "debug",
                                    "",
                                    "raw_model_option",
                                    Some(&format!("id={mid} name={mname} desc={:?}", mdesc.as_deref().unwrap_or("(none)"))),
                                );
                            }
                            models = select_options
                                .into_iter()
                                .map(|(id, name, desc)| ModelDef {
                                    id,
                                    label: model_display_label(&name, desc.as_deref()),
                                    description: desc,
                                })
                                .collect();
                        }
                        current_model = current_value;
                    } else if lower.contains("mode") {
                        mode_config_id = Some(id);
                        if !select_options.is_empty() {
                            modes = select_options
                                .into_iter()
                                .map(|(id, name, _)| ModeDef { id, label: name })
                                .collect();
                        }
                        current_mode = current_value;
                    } else if lower.contains("effort")
                        || lower.contains("thinking")
                        || lower.contains("thought")
                    {
                        effort_config_id = Some(id);
                        if !select_options.is_empty() && effort_options.is_empty() {
                            effort_options = select_options
                                .iter()
                                .map(|(id, name, _)| ModeDef {
                                    id: id.clone(),
                                    label: name.clone(),
                                })
                                .collect();
                        }
                        current_effort_id = current_value.clone();
                        current_effort = current_value.and_then(|v| v.parse().ok());
                    }
                }
            }
        }
    }

    if let Some(current) = &current_model {
        if !models.iter().any(|m| &m.id == current) {
            models.insert(
                0,
                ModelDef {
                    id: current.clone(),
                    label: current.clone(),
                    description: None,
                },
            );
        }
    }

    CapabilitySnapshot {
        modes,
        models,
        thinking_effort,
        effort_options,
        supports_cancel: true,
        current_mode,
        current_model,
        current_effort,
        current_effort_id,
        model_config_id,
        mode_config_id,
        effort_config_id,
    }
}

fn request(process: &Arc<AcpProcess>, method: &str, params: Value) -> Result<Value, String> {
    request_with_timeout(process, method, params, Duration::from_secs(60))
}

fn request_with_timeout(
    process: &Arc<AcpProcess>,
    method: &str,
    params: Value,
    timeout: Duration,
) -> Result<Value, String> {
    let id = process.next_id.fetch_add(1, Ordering::Relaxed);
    let (sender, receiver) = channel();
    process
        .pending
        .lock()
        .map_err(|_| "ACP pending lock poisoned".to_string())?
        .insert(id, sender);
    if let Err(error) = write_message(
        process,
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
    ) {
        let _ = process
            .pending
            .lock()
            .map(|mut pending| pending.remove(&id));
        return Err(error);
    }
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(error) => {
            let _ = process
                .pending
                .lock()
                .map(|mut pending| pending.remove(&id));
            Err(format!(
                "ACP request `{method}` timed out after {}s: {error}",
                timeout.as_secs()
            ))
        }
    }
}

fn json_rpc_id_as_u64(id: &Value) -> Option<u64> {
    id.as_u64()
        .or_else(|| id.as_i64().and_then(|n| u64::try_from(n).ok()))
        .or_else(|| id.as_str()?.parse().ok())
}

fn notify(process: &Arc<AcpProcess>, method: &str, params: Value) -> Result<(), String> {
    write_message(
        process,
        json!({ "jsonrpc": "2.0", "method": method, "params": params }),
    )
}

/// Own the agent's stdin on a dedicated thread and feed it from a queue.
///
/// The pipe write is the one operation here that can block for an unbounded
/// time: if the agent stops draining stdin (stuck in a slow model call, hung
/// tool, deadlocked runtime) the OS pipe buffer fills and `write` parks. When
/// that happened on a caller's thread it took the stdin mutex down with it and
/// wedged every other writer. Isolating it here means callers only ever enqueue.
///
/// The trade-off is that enqueueing succeeds before the write is attempted, so
/// a write failure has no caller left to return an error to — `on_fail` is how
/// that surfaces instead. Without it a dropped `session/prompt` would leave the
/// UI streaming forever against a pipe nobody is reading.
fn spawn_stdin_writer(
    mut stdin: impl Write + Send + 'static,
    session_id: String,
    on_fail: impl FnOnce(String) + Send + 'static,
) -> Sender<Value> {
    let (tx, rx) = channel::<Value>();
    thread::spawn(move || {
        // Ends when every Sender drops (process torn down) or the pipe breaks
        // (child killed) — no other shutdown signal is needed.
        for message in rx {
            let line = match serde_json::to_string(&message) {
                Ok(line) => line,
                Err(error) => {
                    crate::debug_log::append(
                        "acp",
                        "warn",
                        &session_id,
                        "drop unserializable outbound message",
                        Some(&error.to_string()),
                    );
                    continue;
                }
            };
            if let Err(error) = writeln!(stdin, "{line}").and_then(|()| stdin.flush()) {
                crate::debug_log::append(
                    "acp",
                    "warn",
                    &session_id,
                    "stdin writer stopped",
                    Some(&error.to_string()),
                );
                on_fail(error.to_string());
                return;
            }
        }
    });
    tx
}

/// Enqueue an outbound message. Never blocks: the queue is unbounded and the
/// lock is only held for the hand-off, never across the pipe write.
fn write_message(process: &Arc<AcpProcess>, message: Value) -> Result<(), String> {
    process
        .stdin_tx
        .lock()
        .map_err(|_| "ACP stdin lock poisoned".to_string())?
        .send(message)
        .map_err(|_| "ACP agent stdin is closed (process gone?)".to_string())
}

fn write_response(process: &Arc<AcpProcess>, id: Value, result: Value) {
    let _ = write_message(
        process,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        }),
    );
}

fn write_error_response(process: &Arc<AcpProcess>, id: Value, code: i64, message: &str) {
    let _ = write_message(
        process,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        }),
    );
}

fn read_stdout(
    app: AppHandle,
    session_id: String,
    stdout: impl std::io::Read,
    process: Arc<AcpProcess>,
    pending: Arc<Mutex<HashMap<u64, Sender<Result<Value, String>>>>>,
    permissions: Arc<Mutex<HashMap<String, PendingPermission>>>,
    permission_seq: Arc<AtomicU64>,
) {
    // Ground truth for "did the bytes arrive at all". Everything downstream of
    // here is our own processing, so a line present in this file but missing
    // from dev.log localises the bug precisely. Off by default; the write goes
    // through a channel so it can never stall the read loop.
    let raw_tap = raw_stdout_tap(&session_id);

    for line in BufReader::new(stdout).lines() {
        let line = match line {
            Ok(line) => line,
            Err(error) => {
                // Was a silent `break`: a decode/IO error looked exactly like a
                // clean EOF, so a reader that died mid-stream was untraceable.
                crate::debug_log::append(
                    "acp",
                    "error",
                    &session_id,
                    "stdout reader aborted — stream will go silent",
                    Some(&error.to_string()),
                );
                break;
            }
        };
        READER_LINES.fetch_add(1, Ordering::Relaxed);
        if let Some(tap) = raw_tap.as_ref() {
            let _ = tap.send(line.clone());
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            emit_event(
                &app,
                &session_id,
                "notification",
                None,
                json!({ "raw": line }),
            );
            continue;
        };

        // JSON-RPC response to one of our requests
        if message.get("method").is_none() {
            if let Some(id) = message.get("id").and_then(json_rpc_id_as_u64) {
                let result = if let Some(error) = message.get("error") {
                    Err(error.to_string())
                } else {
                    Ok(message.get("result").cloned().unwrap_or(Value::Null))
                };
                // Also surface on the event bus so the UI/debug log can see turn completion.
                emit_event(
                    &app,
                    &session_id,
                    if result.is_ok() { "response" } else { "error" },
                    Some("rpc/response"),
                    message.clone(),
                );
                if let Ok(mut pending) = pending.lock() {
                    if let Some(sender) = pending.remove(&id) {
                        let _ = sender.send(result);
                    }
                }
                continue;
            }
        }

        // Agent → client request that needs a response
        if let Some(id) = message.get("id").cloned() {
            if let Some(method) = message.get("method").and_then(Value::as_str) {
                let params = message.get("params").cloned().unwrap_or(Value::Null);
                emit_event(
                    &app,
                    &session_id,
                    "request",
                    Some(method),
                    message.clone(),
                );
                handle_agent_request(
                    &process,
                    &app,
                    &session_id,
                    id,
                    method,
                    params,
                    &permissions,
                    &permission_seq,
                );
                continue;
            }
        }

        // Notification
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .map(str::to_string);
        emit_event(
            &app,
            &session_id,
            "notification",
            method.as_deref(),
            message,
        );
    }

    // Stdout closed (agent crash/exit/EOF). Fail any pending RPC waiters and tell the UI
    // so status does not stay "Working" forever with no stream updates.
    if let Ok(mut pending_map) = pending.lock() {
        for (_, sender) in pending_map.drain() {
            let _ = sender.send(Err(
                "Agent process ended (stdout closed) before a response arrived".to_string(),
            ));
        }
    }
    let _ = process.child.lock().map(|mut child| {
        let _ = child.try_wait();
    });
    if process.intentional_stop.load(Ordering::SeqCst) {
        emit_event(
            &app,
            &session_id,
            "system",
            Some("process/stopped"),
            json!({
                "message": "Agent process stopped by AgentShell.",
                "sessionId": session_id,
            }),
        );
        return;
    }
    emit_event(
        &app,
        &session_id,
        "error",
        Some("process/ended"),
        json!({
            "message": "Agent process stream ended (process exited or stdout closed).",
            "sessionId": session_id,
        }),
    );
}

/// Client methods the agent may call. Permission is gated by the UI (no silent allow).
fn handle_agent_request(
    process: &Arc<AcpProcess>,
    app: &AppHandle,
    session_id: &str,
    id: Value,
    method: &str,
    params: Value,
    permissions: &Arc<Mutex<HashMap<String, PendingPermission>>>,
    permission_seq: &Arc<AtomicU64>,
) {
    match method {
        "session/request_permission" => {
            let seq = permission_seq.fetch_add(1, Ordering::Relaxed) + 1;
            let request_id = format!("{session_id}:perm:{seq}");
            let options = extract_permission_options(&params);
            let (title, detail) = permission_summary(&params);
            if let Ok(mut map) = permissions.lock() {
                map.insert(
                    request_id.clone(),
                    PendingPermission {
                        process: Arc::clone(process),
                        rpc_id: id.clone(),
                        options: Value::Array(options.clone()),
                    },
                );
            }
            emit_event(
                app,
                session_id,
                "request",
                Some("permission/prompt"),
                json!({
                    "requestId": request_id.clone(),
                    "sessionId": session_id,
                    "title": title,
                    "detail": detail,
                    "options": options,
                    "params": params,
                }),
            );
            // Auto-deny if UI never answers (avoids hung agents).
            let timeout_app = app.clone();
            let timeout_session = session_id.to_string();
            let timeout_req = request_id;
            let timeout_perms = Arc::clone(permissions);
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(120));
                let pending = {
                    let Ok(mut map) = timeout_perms.lock() else {
                        return;
                    };
                    map.remove(&timeout_req)
                };
                if let Some(pending) = pending {
                    let option_id = pick_reject_option(&pending.options)
                        .unwrap_or_else(|| "cancel".to_string());
                    write_response(
                        &pending.process,
                        pending.rpc_id,
                        json!({
                            "outcome": {
                                "outcome": "selected",
                                "optionId": option_id
                            }
                        }),
                    );
                    emit_event(
                        &timeout_app,
                        &timeout_session,
                        "system",
                        Some("permission/timeout"),
                        json!({ "requestId": timeout_req, "optionId": option_id }),
                    );
                }
            });
        }
        // Off-thread for the same reason dispatch is: this is disk I/O, and the
        // reader it would run on is the only thing draining the agent's stdout.
        // Responses are matched by id, so answering out of order is fine.
        "fs/read_text_file" | "fs/write_text_file" => {
            let process = Arc::clone(process);
            let method = method.to_string();
            thread::spawn(move || {
                let outcome = if method == "fs/read_text_file" {
                    read_text_file_for_agent(&params).map(|content| json!({ "content": content }))
                } else {
                    write_text_file_for_agent(&params).map(|()| json!({}))
                };
                match outcome {
                    Ok(result) => write_response(&process, id, result),
                    Err(error) => write_error_response(&process, id, -32000, &error),
                }
            });
        }
        other => {
            // Unknown client methods: reject so the agent can recover instead of hang
            write_error_response(
                process,
                id,
                -32601,
                &format!("Method not implemented by AgentShell: {other}"),
            );
            emit_event(
                app,
                session_id,
                "system",
                Some("request/unimplemented"),
                json!({ "method": other }),
            );
        }
    }
}

fn extract_permission_options(params: &Value) -> Vec<Value> {
    params
        .get("options")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|opt| {
                    let option_id = opt
                        .get("optionId")
                        .or_else(|| opt.get("option_id"))
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if option_id.is_empty() {
                        return None;
                    }
                    let name = opt
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(option_id.as_str())
                        .to_string();
                    let kind = opt
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    Some(json!({
                        "optionId": option_id,
                        "name": name,
                        "kind": kind,
                    }))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn permission_summary(params: &Value) -> (String, Option<String>) {
    // Prefer tool call title / path from common ACP shapes.
    if let Some(tool) = params.get("toolCall").or_else(|| params.get("tool_call")) {
        let title = tool
            .get("title")
            .and_then(Value::as_str)
            .or_else(|| tool.get("kind").and_then(Value::as_str))
            .unwrap_or("Tool permission required")
            .to_string();
        let detail = tool
            .get("rawInput")
            .or_else(|| tool.get("raw_input"))
            .map(|v| {
                if let Some(s) = v.as_str() {
                    s.to_string()
                } else {
                    v.to_string()
                }
            })
            .or_else(|| {
                tool.get("locations")
                    .and_then(Value::as_array)
                    .map(|locs| {
                        locs.iter()
                            .filter_map(|l| l.get("path").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .filter(|s| !s.is_empty())
            });
        return (title, detail);
    }
    if let Some(title) = params.get("title").and_then(Value::as_str) {
        return (title.to_string(), None);
    }
    (
        "Permission required".to_string(),
        Some("The agent wants to run a tool or access a resource.".to_string()),
    )
}

fn pick_reject_option(options: &Value) -> Option<String> {
    let arr = options.as_array()?;
    let mut reject_once = None;
    let mut reject_named = None;
    for opt in arr {
        let kind = opt.get("kind").and_then(Value::as_str).unwrap_or("");
        let option_id = opt
            .get("optionId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if option_id.is_empty() {
            continue;
        }
        let name = opt.get("name").and_then(Value::as_str).unwrap_or("");
        match kind {
            "reject_once" | "reject_always" | "cancel" => {
                return Some(option_id);
            }
            _ => {}
        }
        let lower = format!("{option_id} {name}").to_ascii_lowercase();
        if reject_named.is_none()
            && (lower.contains("reject") || lower.contains("deny") || lower.contains("cancel"))
        {
            reject_named = Some(option_id.clone());
        }
        if reject_once.is_none() && kind.contains("reject") {
            reject_once = Some(option_id);
        }
    }
    reject_once.or(reject_named)
}

#[allow(dead_code)]
fn pick_allow_option(params: &Value) -> Option<String> {
    let options = params.get("options")?.as_array()?;
    // Prefer allow_always, then allow_once, then any option whose id/name suggests allow
    let mut allow_once = None;
    let mut allow_always = None;
    let mut allow_named = None;
    for opt in options {
        let kind = opt.get("kind").and_then(Value::as_str).unwrap_or("");
        let option_id = opt
            .get("optionId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let name = opt.get("name").and_then(Value::as_str).unwrap_or("");
        match kind {
            "allow_always" => allow_always = Some(option_id),
            "allow_once" => allow_once = Some(option_id),
            _ => {
                if allow_named.is_none()
                    && (option_id.to_lowercase().contains("allow")
                        || name.to_lowercase().contains("allow"))
                {
                    allow_named = Some(option_id);
                }
            }
        }
    }
    allow_always.or(allow_once).or(allow_named).or_else(|| {
        options
            .first()
            .and_then(|o| o.get("optionId"))
            .and_then(Value::as_str)
            .map(str::to_string)
    })
}

fn read_text_file_for_agent(params: &Value) -> Result<String, String> {
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "fs/read_text_file missing path".to_string())?;
    let path = PathBuf::from(path);
    let content =
        fs::read_to_string(&path).map_err(|error| format!("Read {path:?} failed: {error}"))?;
    let line = params.get("line").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
    let limit = params.get("limit").and_then(Value::as_u64).map(|n| n as usize);
    if line <= 1 && limit.is_none() {
        return Ok(content);
    }
    let lines: Vec<&str> = content.lines().collect();
    let start = line.saturating_sub(1).min(lines.len());
    let end = match limit {
        Some(n) => (start + n).min(lines.len()),
        None => lines.len(),
    };
    Ok(lines[start..end].join("\n"))
}

fn write_text_file_for_agent(params: &Value) -> Result<(), String> {
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| "fs/write_text_file missing path".to_string())?;
    let content = params
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| "fs/write_text_file missing content".to_string())?;
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("Create parent for {path:?} failed: {error}"))?;
        }
    }
    fs::write(&path, content).map_err(|error| format!("Write {path:?} failed: {error}"))
}

fn read_stderr(app: AppHandle, session_id: String, stderr: impl std::io::Read) {
    for line in BufReader::new(stderr).lines().flatten() {
        emit_event(&app, &session_id, "stderr", None, json!({ "text": line }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::Receiver;

    /// Stands in for an agent that stopped draining its stdin: every write parks.
    struct WedgedPipe(Receiver<()>);

    impl Write for WedgedPipe {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            let _ = self.0.recv(); // never signalled — parks like a full pipe
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn enqueue_never_blocks_on_a_wedged_pipe() {
        // Held, not dropped: dropping the sender would let `recv` return.
        let (_hold_gate_open, gate) = channel::<()>();
        let tx = spawn_stdin_writer(WedgedPipe(gate), "test-session".to_string(), |_| {});

        let start = std::time::Instant::now();
        for _ in 0..64 {
            tx.send(json!({ "jsonrpc": "2.0", "method": "session/cancel" }))
                .expect("enqueue must succeed while the writer is stuck");
        }
        // The old code took the stdin mutex across the write, so the second
        // caller — the cancel trying to rescue the turn — parked with it.
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "enqueueing blocked behind the stuck pipe write"
        );
    }

    fn chunk(update_kind: &str, message_id: &str, text: &str) -> UiEvent {
        UiEvent {
            kind: "notification".to_string(),
            method: Some("session/update".to_string()),
            data: json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": {
                    "sessionId": "ses_test",
                    "update": {
                        "sessionUpdate": update_kind,
                        "messageId": message_id,
                        "content": { "type": "text", "text": text }
                    }
                }
            }),
        }
    }

    /// The invariant that matters: the user must read exactly what the model
    /// wrote. Merging may change how many events carry it, never the text.
    #[test]
    fn coalescing_preserves_the_text_stream_exactly() {
        let batch = vec![
            chunk("agent_thought_chunk", "m1", "Let"),
            chunk("agent_thought_chunk", "m1", " me"),
            chunk("agent_thought_chunk", "m1", " read"),
            chunk("agent_message_chunk", "m1", "Reading"),
            chunk("agent_message_chunk", "m1", " now"),
            chunk("agent_message_chunk", "m2", "next turn"),
        ];
        let before: String = batch.iter().filter_map(chunk_text).collect();

        let out = coalesce_stream_chunks(batch);

        let after: String = out.iter().filter_map(chunk_text).collect();
        assert_eq!(before, after, "coalescing must not alter a single character");
        assert_eq!(
            out.len(),
            3,
            "one event per (updateKind, messageId) run, got {:?}",
            out.iter().filter_map(chunk_text).collect::<Vec<_>>()
        );
    }

    #[test]
    fn coalescing_never_reorders_or_merges_across_other_events() {
        let tool_call = UiEvent {
            kind: "notification".to_string(),
            method: Some("session/update".to_string()),
            data: json!({
                "params": { "update": { "sessionUpdate": "tool_call", "status": "pending" } }
            }),
        };
        let batch = vec![
            chunk("agent_thought_chunk", "m1", "a"),
            chunk("agent_thought_chunk", "m1", "b"),
            tool_call,
            chunk("agent_thought_chunk", "m1", "c"),
        ];

        let out = coalesce_stream_chunks(batch);

        assert_eq!(out.len(), 3);
        assert_eq!(chunk_text(&out[0]), Some("ab"));
        assert_eq!(
            out[1].data.pointer("/params/update/sessionUpdate").unwrap(),
            "tool_call",
            "a tool call must stay put — merging across it would reorder the transcript"
        );
        assert_eq!(
            chunk_text(&out[2]),
            Some("c"),
            "text after an interruption must not fold back into text before it"
        );
    }

    #[test]
    fn coalescing_leaves_a_keeping_up_stream_untouched() {
        // One event at a time is the normal case; it must pass through as-is.
        let out = coalesce_stream_chunks(vec![chunk("agent_message_chunk", "m1", "solo")]);
        assert_eq!(out.len(), 1);
        assert_eq!(chunk_text(&out[0]), Some("solo"));
    }

    /// Stands in for a killed agent: the pipe is closed under us.
    struct BrokenPipe;

    impl Write for BrokenPipe {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "child gone",
            ))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_broken_pipe_is_reported_through_on_fail() {
        let (failed_tx, failed_rx) = channel::<String>();
        let tx = spawn_stdin_writer(BrokenPipe, "test-session".to_string(), move |error| {
            let _ = failed_tx.send(error);
        });

        // Enqueue succeeds — the write has not been attempted yet. That is
        // exactly why the caller cannot be the one to learn it failed.
        tx.send(json!({ "jsonrpc": "2.0", "method": "session/prompt" }))
            .expect("first enqueue is accepted");

        let reported = failed_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("a dropped prompt must be reported, not silently swallowed");
        assert!(!reported.is_empty(), "failure reason should be populated");
    }

    #[test]
    fn enqueue_reports_a_dead_writer() {
        let tx = spawn_stdin_writer(BrokenPipe, "test-session".to_string(), |_| {});

        // Queueing is decoupled from writing, so the failure shows up on a
        // later send — but it must show up, not swallow outbound traffic.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while tx.send(json!({ "jsonrpc": "2.0", "method": "session/cancel" })).is_ok() {
            assert!(
                std::time::Instant::now() < deadline,
                "a dead writer thread never surfaced as a send error"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

// ── UI event dispatch ──────────────────────────────────────────────────────
//
// Dispatch runs on its own thread, and that is load-bearing rather than tidy.
//
// The stdout reader used to log to disk and `app.emit` inline for every line it
// read. At streaming rates (measured: ~93 events/s, one per token) it spent
// most of its time not reading, the agent's stdout pipe filled up, and the
// agent's writer blocked on a full pipe — while its own internals carried on,
// so it looked "hung" from the UI and healthy from its own logs. That is a
// backpressure deadlock: the agent's flow control was coupled to how fast React
// could render. The reader must never do anything slower than a queue push.

struct UiEvent {
    kind: String,
    method: Option<String>,
    data: Value,
}

/// Per-session dispatch queues, so `emit_event` keeps its existing signature at
/// the ~10 call sites that only hold an `AppHandle`.
static EMITTERS: OnceLock<Mutex<HashMap<String, Sender<UiEvent>>>> = OnceLock::new();

fn emitters() -> &'static Mutex<HashMap<String, Sender<UiEvent>>> {
    EMITTERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn register_emitter(session_id: &str, app: &AppHandle) {
    let tx = spawn_event_emitter(app.clone(), session_id.to_string());
    if let Ok(mut map) = emitters().lock() {
        // Replacing a previous session's sender drops it, which drains and ends
        // that emitter thread once its backlog is flushed.
        map.insert(session_id.to_string(), tx);
    }
}

fn unregister_emitter(session_id: &str) {
    if let Ok(mut map) = emitters().lock() {
        map.remove(session_id);
    }
}

fn queue_event(session_id: &str, event: UiEvent) -> Result<(), UiEvent> {
    let Ok(map) = emitters().lock() else {
        return Err(event);
    };
    let Some(tx) = map.get(session_id) else {
        return Err(event);
    };
    let result = tx.send(event).map_err(|err| err.0);
    if result.is_ok() {
        QUEUE_DEPTH.fetch_add(1, Ordering::Relaxed);
    }
    result
}

/// Hand an event to the session's dispatch thread. Never blocks.
fn emit_event(app: &AppHandle, session_id: &str, kind: &str, method: Option<&str>, data: Value) {
    let event = UiEvent {
        kind: kind.to_string(),
        method: method.map(str::to_string),
        data,
    };
    // Before the session registers (or after it is gone) there is no queue to
    // use — those are rare and low-volume, so going direct is fine.
    if let Err(event) = queue_event(session_id, event) {
        emit_now(app, session_id, event);
    }
}

// ── Pipeline liveness ──────────────────────────────────────────────────────
//
// Three things in a row can each stop delivering without any error surfacing:
// the stdout reader, the dispatch queue, and the `app.emit` call itself. Timing
// a call *after* it returns cannot catch a call that never returns, which is
// the failure being hunted — so progress is recorded before each step, and a
// watchdog reports what is outstanding.

static READER_LINES: AtomicU64 = AtomicU64::new(0);
static EMITS_DONE: AtomicU64 = AtomicU64::new(0);
static QUEUE_DEPTH: AtomicU64 = AtomicU64::new(0);
static EMIT_INFLIGHT: OnceLock<Mutex<Option<(String, Instant)>>> = OnceLock::new();

fn emit_inflight() -> &'static Mutex<Option<(String, Instant)>> {
    EMIT_INFLIGHT.get_or_init(|| Mutex::new(None))
}

pub fn pipeline_summary() -> String {
    let inflight = match emit_inflight().lock() {
        Ok(slot) => match slot.as_ref() {
            Some((label, since)) => format!("{label} for {}ms", since.elapsed().as_millis()),
            None => "idle".to_string(),
        },
        Err(_) => "<poisoned>".to_string(),
    };
    format!(
        "readerLines={} emitsDone={} queueDepth={} emitting={}",
        READER_LINES.load(Ordering::Relaxed),
        EMITS_DONE.load(Ordering::Relaxed),
        QUEUE_DEPTH.load(Ordering::Relaxed),
        inflight
    )
}

/// Reports an `app.emit` that has not returned, or a queue nobody is draining.
///
/// Also emits a periodic heartbeat while a session is live. Without it the
/// counters are only ever printed when something looks stuck — which is useless
/// for the opposite failure, where every component is healthy and idle and the
/// bytes simply never arrive. The heartbeat makes "readerLines stopped moving"
/// visible as a fact instead of an inference.
pub fn spawn_pipeline_watchdog() {
    thread::spawn(|| {
        let mut last_beat = Instant::now();
        let mut last_seen = (0u64, 0u64);
        let mut reported = false;
        loop {
            thread::sleep(Duration::from_secs(3));

            let live = emitters().lock().map(|m| !m.is_empty()).unwrap_or(false);
            let now = (
                READER_LINES.load(Ordering::Relaxed),
                EMITS_DONE.load(Ordering::Relaxed),
            );
            if live && last_beat.elapsed() >= Duration::from_secs(10) {
                last_beat = Instant::now();
                let moved = now != last_seen;
                last_seen = now;
                crate::debug_log::append(
                    "watchdog",
                    if moved { "debug" } else { "info" },
                    "",
                    if moved { "pipeline heartbeat" } else { "pipeline heartbeat (no movement)" },
                    Some(&pipeline_summary()),
                );
            }
            let stuck_emit = emit_inflight()
                .lock()
                .ok()
                .and_then(|slot| slot.as_ref().map(|(l, s)| (l.clone(), s.elapsed())))
                .filter(|(_, age)| *age > Duration::from_secs(3));
            let idle_backlog = stuck_emit.is_none() && QUEUE_DEPTH.load(Ordering::Relaxed) > 0;

            if stuck_emit.is_none() && !idle_backlog {
                reported = false;
                continue;
            }
            if reported {
                continue; // one line per episode, not one every 3s
            }
            reported = true;
            let what = match &stuck_emit {
                Some((label, age)) => {
                    format!("app.emit has not returned for {}s on {label}", age.as_secs())
                }
                None => "events queued but the dispatch thread is not draining them".to_string(),
            };
            crate::debug_log::append(
                "watchdog",
                "error",
                "",
                "ACP DELIVERY STALLED — UI will stop updating",
                Some(&format!("{what} | {}", pipeline_summary())),
            );
        }
    });
}

/// `AGENTSHELL_RAW_LOG=1` mirrors every stdout line the reader yields to
/// `~/.agentshell/logs/raw-<session>.log`, unabridged. dev.log clips detail to
/// 4000 chars, which is smaller than a `read` tool result — so it cannot answer
/// "did we receive this line" on its own.
fn raw_stdout_tap(session_id: &str) -> Option<Sender<String>> {
    if std::env::var("AGENTSHELL_RAW_LOG").ok().as_deref() != Some("1") {
        return None;
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let dir = PathBuf::from(home).join(".agentshell").join("logs");
    fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("raw-{session_id}.log"));

    let (tx, rx) = channel::<String>();
    thread::spawn(move || {
        let Ok(mut file) = fs::File::create(&path) else {
            return;
        };
        for line in rx {
            let _ = writeln!(file, "{line}");
            let _ = file.flush();
        }
    });
    Some(tx)
}

const SLOW_DISPATCH: Duration = Duration::from_millis(250);
/// Bounded so one merge cannot build an unbounded payload.
const MAX_BATCH: usize = 256;
const MAX_MERGED_TEXT: usize = 64 * 1024;

fn spawn_event_emitter(app: AppHandle, session_id: String) -> Sender<UiEvent> {
    let (tx, rx) = channel::<UiEvent>();
    thread::spawn(move || {
        let mut last_backlog_report: Option<Instant> = None;
        // Ends when the sender is dropped, after draining what is queued.
        while let Ok(first) = rx.recv() {
            let mut batch = vec![first];
            // Only ever batches what is *already* waiting. If the UI is keeping
            // up, `try_recv` is empty on the first poll and behaviour is
            // identical to emitting one at a time.
            while batch.len() < MAX_BATCH {
                match rx.try_recv() {
                    Ok(next) => batch.push(next),
                    Err(_) => break,
                }
            }

            if batch.len() >= 32 {
                let report = last_backlog_report
                    .map(|at| at.elapsed() >= Duration::from_secs(1))
                    .unwrap_or(true);
                if report {
                    last_backlog_report = Some(Instant::now());
                    crate::debug_log::append(
                        "acp",
                        "warn",
                        &session_id,
                        "UI dispatch is behind — coalescing stream chunks",
                        Some(&format!("batch={}", batch.len())),
                    );
                }
            }

            let queued = batch.len() as u64;
            for event in coalesce_stream_chunks(batch) {
                // A panic here used to end the session's UI updates for good,
                // with no error anywhere. Drop the offending event instead.
                let label = event.method.clone().unwrap_or_else(|| event.kind.clone());
                let app = &app;
                let session = session_id.as_str();
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                    emit_now(app, session, event)
                }))
                .is_err()
                {
                    crate::debug_log::append(
                        "acp",
                        "error",
                        &session_id,
                        "dropped an event that panicked during dispatch",
                        Some(&label),
                    );
                }
            }
            QUEUE_DEPTH.fetch_sub(queued, Ordering::Relaxed);
        }
    });
    tx
}

/// Text chunks arrive one token at a time; when we are behind, adjacent chunks
/// of the same message can be delivered as one without the UI noticing — it
/// appends the text either way.
fn coalesce_stream_chunks(batch: Vec<UiEvent>) -> Vec<UiEvent> {
    let mut out: Vec<UiEvent> = Vec::with_capacity(batch.len());
    for event in batch {
        let key = chunk_key(&event);
        let merged = match (key, out.last_mut()) {
            (Some(key), Some(previous)) if chunk_key(previous).as_ref() == Some(&key) => {
                match (chunk_text(&event), chunk_text(previous)) {
                    (Some(extra), Some(existing)) if existing.len() + extra.len() <= MAX_MERGED_TEXT => {
                        let combined = format!("{existing}{extra}");
                        set_chunk_text(previous, combined)
                    }
                    _ => false,
                }
            }
            _ => false,
        };
        if !merged {
            out.push(event);
        }
    }
    out
}

/// `Some((updateKind, messageId))` for a mergeable text chunk, else `None`.
fn chunk_key(event: &UiEvent) -> Option<(String, String)> {
    if event.method.as_deref() != Some("session/update") {
        return None;
    }
    let update = event.data.pointer("/params/update")?;
    let kind = update.get("sessionUpdate")?.as_str()?;
    if kind != "agent_message_chunk" && kind != "agent_thought_chunk" {
        return None;
    }
    if update.pointer("/content/type")?.as_str()? != "text" {
        return None;
    }
    let message_id = update
        .get("messageId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some((kind.to_string(), message_id.to_string()))
}

fn chunk_text(event: &UiEvent) -> Option<&str> {
    event.data.pointer("/params/update/content/text")?.as_str()
}

fn set_chunk_text(event: &mut UiEvent, text: String) -> bool {
    match event.data.pointer_mut("/params/update/content/text") {
        Some(slot) => {
            *slot = Value::String(text);
            true
        }
        None => false,
    }
}

/// Takes the event by value: this runs once per streamed chunk, so the payload
/// is moved into the emit rather than cloned.
fn emit_now(app: &AppHandle, session_id: &str, event: UiEvent) {
    // Developer diary on disk — not a product UI surface.
    let summary = event
        .method
        .clone()
        .unwrap_or_else(|| event.kind.clone());
    let detail = serde_json::to_string(&event.data).ok();
    let level = if event.kind == "error" { "error" } else { "info" };

    let log_started = Instant::now();
    crate::debug_log::append("acp", level, session_id, &summary, detail.as_deref());
    let log_took = log_started.elapsed();

    // Marked *before* the call: an emit that never returns is the failure mode
    // this is here to catch, and it would leave no trace if only timed after.
    let emit_started = Instant::now();
    if let Ok(mut slot) = emit_inflight().lock() {
        *slot = Some((format!("{session_id}:{summary}"), emit_started));
    }
    let _ = app.emit(
        ACP_EVENT,
        AcpEvent {
            session_id: session_id.to_string(),
            kind: event.kind,
            method: event.method,
            data: event.data,
        },
    );
    if let Ok(mut slot) = emit_inflight().lock() {
        *slot = None;
    }
    EMITS_DONE.fetch_add(1, Ordering::Relaxed);
    let emit_took = emit_started.elapsed();

    // Names which half is slow, so the next stall does not need to be guessed at.
    if log_took > SLOW_DISPATCH || emit_took > SLOW_DISPATCH {
        crate::debug_log::append(
            "acp",
            "warn",
            session_id,
            "slow UI event dispatch",
            Some(&format!(
                "log={}ms emit={}ms method={}",
                log_took.as_millis(),
                emit_took.as_millis(),
                summary
            )),
        );
    }
}
