use agent_client_protocol_schema::v1 as acp_schema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
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
    pub supports_cancel: bool,
    pub current_mode: Option<String>,
    pub current_model: Option<String>,
    pub current_effort: Option<f64>,
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
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, Sender<Result<Value, String>>>>>,
    agent_session_id: Mutex<Option<String>>,
}

#[derive(Clone, Default)]
pub struct AcpService {
    sessions: Arc<Mutex<HashMap<String, Arc<AcpProcess>>>>,
    capabilities: Arc<Mutex<HashMap<String, CapabilitySnapshot>>>,
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
    ) -> Result<CapabilitySnapshot, String> {
        if !std::path::Path::new(&cwd).is_dir() {
            return Err(format!("ACP cwd is not a directory: {cwd}"));
        }

        {
            let sessions = self
                .sessions
                .lock()
                .map_err(|_| "ACP session lock poisoned".to_string())?;
            if sessions.contains_key(&session_id) {
                if let Some(caps) = self.get_capabilities(&session_id) {
                    return Ok(caps);
                }
            }
        }
        // Stale process without capabilities — kill and restart
        let _ = self.stop(&session_id);

        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| "ACP session lock poisoned".to_string())?;

        // Windows npm shims (`opencode` without .exe) fail with "program not found"
        // under CreateProcess. Resolve to a real executable (or cmd.exe /C .cmd).
        let resolved = crate::process_util::resolve_spawn_command(&command).map_err(|error| {
            format!("Start ACP agent failed: {error}")
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
        let process = Arc::new(AcpProcess {
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
            next_id: AtomicU64::new(1),
            pending: Arc::clone(&pending),
            agent_session_id: Mutex::new(None),
        });
        sessions.insert(session_id.clone(), Arc::clone(&process));
        drop(sessions);

        let reader_app = app.clone();
        let reader_session = session_id.clone();
        let reader_process = Arc::clone(&process);
        thread::spawn(move || {
            read_stdout(reader_app, reader_session, stdout, reader_process, pending)
        });
        let stderr_app = app.clone();
        let stderr_session = session_id.clone();
        thread::spawn(move || read_stderr(stderr_app, stderr_session, stderr));

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
        if let Err(error) = initialized {
            let _ = self.stop(&session_id);
            return Err(format!("ACP initialize failed: {error}"));
        }

        // ── Phase 2: session/new ────────────────────────────────────────
        let new_session = match request(
            &process,
            "session/new",
            json!({ "cwd": cwd, "mcpServers": [] }),
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

    pub fn send_prompt(&self, session_id: &str, text: String) -> Result<Value, String> {
        let process = self.process(session_id)?;
        let agent_session_id = process
            .agent_session_id
            .lock()
            .map_err(|_| "ACP session id lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| "ACP session is not initialized".to_string())?;
        request(
            &process,
            "session/prompt",
            json!({
                "sessionId": agent_session_id,
                "prompt": [{ "type": "text", "text": text }]
            }),
        )
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
            let mut child = process
                .child
                .lock()
                .map_err(|_| "ACP child lock poisoned".to_string())?;
            let _ = child.kill();
            let _ = child.wait();
        }
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
    if Some(config_id) == caps.effort_config_id.as_deref() {
        if let Some(n) = value.as_f64() {
            caps.current_effort = Some(n);
        } else if let Some(text) = text {
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
    let mut current_mode: Option<String> = None;
    let mut current_model: Option<String> = None;
    let mut current_effort: Option<f64> = None;
    let mut thinking_effort: Option<ThinkingEffort> = None;
    let mut model_config_id: Option<String> = None;
    let mut mode_config_id: Option<String> = None;
    let mut effort_config_id: Option<String> = None;

    fn extract_options(options: &acp_schema::SessionConfigSelectOptions) -> Vec<(String, String)> {
        match options {
            acp_schema::SessionConfigSelectOptions::Ungrouped(list) => list
                .iter()
                .map(|o| (o.value.0.to_string(), o.name.clone()))
                .collect(),
            acp_schema::SessionConfigSelectOptions::Grouped(groups) => groups
                .iter()
                .flat_map(|g| &g.options)
                .map(|o| (o.value.0.to_string(), o.name.clone()))
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
                                .map(|(id, label)| ModeDef { id, label })
                                .collect();
                        }
                        current_mode = Some(select.current_value.0.to_string());
                    }
                }
                Some(Cat::Model) => {
                    model_config_id = Some(id_str.clone());
                    if let Kind::Select(select) = &opt.kind {
                        models = extract_options(&select.options)
                            .into_iter()
                            .map(|(id, label)| ModelDef { id, label })
                            .collect();
                        current_model = Some(select.current_value.0.to_string());
                    }
                }
                Some(Cat::ThoughtLevel) => {
                    effort_config_id = Some(id_str.clone());
                    if let Kind::Select(select) = &opt.kind {
                        let vals: Vec<f64> = extract_options(&select.options)
                            .iter()
                            .filter_map(|(v, _)| v.parse::<f64>().ok())
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
                        current_effort = select.current_value.0.parse::<f64>().ok();
                    }
                }
                _ => {
                    let lower = id_str.to_lowercase();
                    if lower.contains("effort") || lower.contains("thinking") || lower.contains("thought")
                    {
                        effort_config_id = Some(id_str);
                        if let Kind::Select(select) = &opt.kind {
                            let vals: Vec<f64> = extract_options(&select.options)
                                .iter()
                                .filter_map(|(v, _)| v.parse::<f64>().ok())
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
                            current_effort = select.current_value.0.parse::<f64>().ok();
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
                },
            );
        }
    }

    CapabilitySnapshot {
        modes,
        models,
        thinking_effort,
        supports_cancel: true,
        current_mode,
        current_model,
        current_effort,
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
    let mut thinking_effort: Option<ThinkingEffort> = None;
    let mut current_mode: Option<String> = None;
    let mut current_model: Option<String> = None;
    let mut current_effort: Option<f64> = None;
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

            let select_options = opt
                .get("options")
                .and_then(|o| o.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|item| {
                            let value = item.get("value")?.as_str()?.to_string();
                            let label = item
                                .get("name")
                                .and_then(Value::as_str)
                                .unwrap_or(value.as_str())
                                .to_string();
                            Some((value, label))
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            match category.as_str() {
                "mode" => {
                    mode_config_id = Some(id);
                    if !select_options.is_empty() {
                        modes = select_options
                            .into_iter()
                            .map(|(id, label)| ModeDef { id, label })
                            .collect();
                    }
                    current_mode = current_value;
                }
                "model" => {
                    model_config_id = Some(id);
                    if !select_options.is_empty() {
                        models = select_options
                            .into_iter()
                            .map(|(id, label)| ModelDef { id, label })
                            .collect();
                    }
                    current_model = current_value;
                }
                "thought_level" | "effort" | "thinking" => {
                    effort_config_id = Some(id);
                    if !select_options.is_empty() {
                        let vals: Vec<f64> = select_options
                            .iter()
                            .filter_map(|(v, _)| v.parse::<f64>().ok())
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
                    current_effort = current_value.and_then(|v| v.parse().ok());
                }
                _ => {
                    let lower = id.to_lowercase();
                    if lower.contains("model") {
                        model_config_id = Some(id);
                        if !select_options.is_empty() {
                            models = select_options
                                .into_iter()
                                .map(|(id, label)| ModelDef { id, label })
                                .collect();
                        }
                        current_model = current_value;
                    } else if lower.contains("mode") {
                        mode_config_id = Some(id);
                        if !select_options.is_empty() {
                            modes = select_options
                                .into_iter()
                                .map(|(id, label)| ModeDef { id, label })
                                .collect();
                        }
                        current_mode = current_value;
                    } else if lower.contains("effort")
                        || lower.contains("thinking")
                        || lower.contains("thought")
                    {
                        effort_config_id = Some(id);
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
                },
            );
        }
    }

    CapabilitySnapshot {
        modes,
        models,
        thinking_effort,
        supports_cancel: true,
        current_mode,
        current_model,
        current_effort,
        model_config_id,
        mode_config_id,
        effort_config_id,
    }
}

fn request(process: &Arc<AcpProcess>, method: &str, params: Value) -> Result<Value, String> {
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
    match receiver.recv_timeout(Duration::from_secs(60)) {
        Ok(result) => result,
        Err(error) => {
            let _ = process
                .pending
                .lock()
                .map(|mut pending| pending.remove(&id));
            Err(format!("ACP request `{method}` timed out: {error}"))
        }
    }
}

fn notify(process: &Arc<AcpProcess>, method: &str, params: Value) -> Result<(), String> {
    write_message(
        process,
        json!({ "jsonrpc": "2.0", "method": method, "params": params }),
    )
}

fn write_message(process: &Arc<AcpProcess>, message: Value) -> Result<(), String> {
    let mut stdin = process
        .stdin
        .lock()
        .map_err(|_| "ACP stdin lock poisoned".to_string())?;
    writeln!(
        stdin,
        "{}",
        serde_json::to_string(&message)
            .map_err(|error| format!("Serialize ACP message failed: {error}"))?
    )
    .map_err(|error| format!("Write ACP message failed: {error}"))?;
    stdin
        .flush()
        .map_err(|error| format!("Flush ACP message failed: {error}"))
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
) {
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
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
            if let Some(id) = message.get("id").and_then(Value::as_u64) {
                let result = if let Some(error) = message.get("error") {
                    Err(error.to_string())
                } else {
                    Ok(message.get("result").cloned().unwrap_or(Value::Null))
                };
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
                handle_agent_request(&process, &app, &session_id, id, method, params);
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
}

/// Minimal auto-handlers so OpenCode can actually run tools without hanging.
fn handle_agent_request(
    process: &Arc<AcpProcess>,
    app: &AppHandle,
    session_id: &str,
    id: Value,
    method: &str,
    params: Value,
) {
    match method {
        "session/request_permission" => {
            let option_id = pick_allow_option(&params).unwrap_or_else(|| "allow".to_string());
            write_response(
                process,
                id,
                json!({
                    "outcome": {
                        "outcome": "selected",
                        "optionId": option_id
                    }
                }),
            );
            emit_event(
                app,
                session_id,
                "system",
                Some("permission/auto-allowed"),
                json!({ "optionId": pick_allow_option(&params) }),
            );
        }
        "fs/read_text_file" => match read_text_file_for_agent(&params) {
            Ok(content) => write_response(process, id, json!({ "content": content })),
            Err(error) => write_error_response(process, id, -32000, &error),
        },
        "fs/write_text_file" => match write_text_file_for_agent(&params) {
            Ok(()) => write_response(process, id, json!({})),
            Err(error) => write_error_response(process, id, -32000, &error),
        },
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

fn emit_event(app: &AppHandle, session_id: &str, kind: &str, method: Option<&str>, data: Value) {
    let _ = app.emit(
        ACP_EVENT,
        AcpEvent {
            session_id: session_id.to_string(),
            kind: kind.to_string(),
            method: method.map(str::to_string),
            data,
        },
    );
}
