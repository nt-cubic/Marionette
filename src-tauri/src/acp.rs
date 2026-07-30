use agent_client_protocol_schema::v1 as acp_schema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
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

// ── Pending UI timeouts (Codeg-aligned: cancel/decline, never auto-approve) ──
/// Permission card: auto-deny if user never answers.
const TIMEOUT_PERMISSION_SECS: u64 = 120;
/// Ask / elicitation questions.
const TIMEOUT_QUESTION_SECS: u64 = 300;
/// Plan approval (human-paced review).
const TIMEOUT_PLAN_APPROVAL_SECS: u64 = 600;

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
    fn strip_noise(s: &str) -> String {
        let head = s
            .split(['·', '•', '●', '|'])
            .next()
            .unwrap_or(s)
            .split(" $")
            .next()
            .unwrap_or(s)
            .trim();
        // Drop trailing context / price marketing so Codex/Grok names stay short.
        let mut out = head.to_string();
        for pat in [
            " with ", // "with 1M context"
        ] {
            if let Some(i) = out.to_ascii_lowercase().find(pat) {
                // Only cut when it looks like a context/spec tail
                let tail = out[i..].to_ascii_lowercase();
                if tail.contains("context") || tail.contains("token") {
                    out.truncate(i);
                    break;
                }
            }
        }
        out.trim().to_string()
    }

    let name_clean = strip_noise(name);
    let Some(desc) = description.map(str::trim).filter(|s| !s.is_empty()) else {
        return name_clean;
    };
    let head = strip_noise(desc);
    if head.is_empty() {
        return name_clean;
    }
    let name_l = name_clean.to_ascii_lowercase();
    let head_l = head.to_ascii_lowercase();
    if head_l == name_l || head_l.starts_with(&format!("{name_l} ")) {
        return head;
    }
    if name_l.contains("default") || name_l.contains("recommend") {
        if let Some(inner) = head
            .find("currently ")
            .map(|i| &head[i + "currently ".len()..])
        {
            let ver = inner.trim_end_matches(')').trim();
            if !ver.is_empty() {
                return format!("Default · {ver}");
            }
        }
        return format!("{name_clean} · {head}");
    }
    // Short family name + richer head (Claude opus/sonnet)
    if name_clean.len() <= 16 && head.len() > name_clean.len() {
        return format!("{name_clean} · {head}");
    }
    name_clean
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
    /// Agent accepts `ContentBlock::Image` in session/prompt (from initialize).
    /// Defaults true so vision-capable CLIs work before we re-parse caps.
    pub prompt_image: bool,
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
    /// Set when Marionette intentionally stops the process (agent switch / delete).
    /// Suppresses false "process/ended" crash UX on the UI.
    intentional_stop: AtomicBool,
    /// Request ids whose *failure* is an expected answer, not a fault.
    ///
    /// Capability probes live here: a -32601 for `session/set_config_option` is
    /// how a pre-v2 agent says "use the older RPC", and the UI must not render
    /// that as "Agent error: Method not found" when we went on to succeed.
    quiet_ids: Arc<Mutex<HashSet<u64>>>,
    /// Latched once an agent answers -32601 to `session/set_config_option`, so
    /// later changes skip the doomed probe entirely.
    config_option_unsupported: AtomicBool,
    /// JSON-RPC id of the outstanding `session/prompt` (`0` = none).
    ///
    /// Turn busy/idle is bound **only** to this id (Codeg-style). Responses to
    /// `set_config_option` / model / mode / effort / probes must never clear
    /// the UI "Working" state — that was the false "turn complete" bug.
    in_flight_prompt_id: Arc<AtomicU64>,
}

struct PendingPermission {
    process: Arc<AcpProcess>,
    rpc_id: Value,
    options: Value,
}

/// Grok (`_x.ai/ask_user_question`) / future multi-choice prompts parked for UI.
struct PendingQuestion {
    process: Arc<AcpProcess>,
    rpc_id: Value,
}

/// Codex `elicitation/create` parked for UI (approval → permission card, questions → ask card).
struct PendingElicitation {
    process: Arc<AcpProcess>,
    rpc_id: Value,
    /// `"approval"` | `"questions"`
    kind: String,
    /// Serialized plan for response rebuild (`approval_to_stored` / `questions_to_stored`).
    stored: Value,
}

/// Grok `_x.ai/exit_plan_mode` approval parked until the user picks an action.
/// Wire format confirmed in Codeg against Grok 0.2.111 — see
/// `build_exit_plan_mode_response`.
struct PendingPlanApproval {
    process: Arc<AcpProcess>,
    rpc_id: Value,
}

#[derive(Clone, Default)]
pub struct AcpService {
    sessions: Arc<Mutex<HashMap<String, Arc<AcpProcess>>>>,
    capabilities: Arc<Mutex<HashMap<String, CapabilitySnapshot>>>,
    /// UI-gated `session/request_permission` waiters, keyed by request_id.
    permissions: Arc<Mutex<HashMap<String, PendingPermission>>>,
    permission_seq: Arc<AtomicU64>,
    /// UI-gated ask-user-question waiters, keyed by request_id.
    questions: Arc<Mutex<HashMap<String, PendingQuestion>>>,
    question_seq: Arc<AtomicU64>,
    /// UI-gated Grok exit-plan-mode approvals, keyed by request_id.
    plan_approvals: Arc<Mutex<HashMap<String, PendingPlanApproval>>>,
    plan_approval_seq: Arc<AtomicU64>,
    /// Codex form elicitation waiters, keyed by request_id (shared UI cards).
    elicitations: Arc<Mutex<HashMap<String, PendingElicitation>>>,
    elicitation_seq: Arc<AtomicU64>,
    /// ACP client `terminal/*` host (shared across sessions).
    terminals: Arc<crate::terminal_runtime::TerminalRuntime>,
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

        // Fallback cwd when agent omits `cwd` on `terminal/create`.
        self.terminals
            .set_session_cwd(&session_id, PathBuf::from(&cwd));

        // Prefer global ACP bins over `npx -y …` (npx cold start can hang UI for minutes).
        let (command, mut args) =
            crate::process_util::prefer_fast_acp_launch(&command, &args);
        // Grok: project-scoped MCP is gated by folder trust. Marionette opens
        // the project on the user's behalf — grant trust for this cwd and make
        // sure `--trust` is on the argv even if an older agent row lacked it.
        if command_looks_like_grok(&command) {
            crate::context_inventory::ensure_grok_folder_trust(std::path::Path::new(&cwd));
            args = with_grok_trust_flag(args);
            crate::debug_log::append(
                "context",
                "info",
                &session_id,
                "grok folder trust ensured for project MCP",
                Some(&cwd),
            );
        }

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
                 (or @agentclientprotocol/claude-agent-acp) so Marionette can skip slow npx."
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
        // Registry launch_env (e.g. Pi embedded context) — does not override OpenCode/Grok specials.
        if let Some(id) = agent_id.as_deref() {
            for (k, v) in crate::agent_registry::launch_env_for(id) {
                child_command.env(*k, *v);
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
        let in_flight_prompt_id = Arc::new(AtomicU64::new(0));
        let stdin_tx = {
            // A failed write happens after its caller already got `Ok`, so the
            // only way it reaches anyone is from in here.
            let fail_app = app.clone();
            let fail_session = session_id.clone();
            let fail_pending: Arc<Mutex<HashMap<u64, Sender<Result<Value, String>>>>> =
                Arc::clone(&pending);
            let fail_inflight = Arc::clone(&in_flight_prompt_id);
            spawn_stdin_writer(stdin, session_id.clone(), move |error| {
                if let Ok(mut waiters) = fail_pending.lock() {
                    for (_, sender) in waiters.drain() {
                        let _ = sender.send(Err(format!("ACP agent stdin closed: {error}")));
                    }
                }
                // Drop any in-flight turn before process/ended so the UI does
                // not stay "Working" if it only listens for turn/complete.
                if take_in_flight_prompt_id(&fail_inflight).is_some() {
                    emit_turn_complete(
                        &fail_app,
                        &fail_session,
                        "error",
                        "error",
                        json!({
                            "message": format!("Agent stopped reading input ({error}). The turn was not delivered."),
                            "reason": "stdin-closed",
                        }),
                    );
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
            quiet_ids: Arc::new(Mutex::new(HashSet::new())),
            config_option_unsupported: AtomicBool::new(false),
            in_flight_prompt_id,
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
        let questions = Arc::clone(&self.questions);
        let question_seq = Arc::clone(&self.question_seq);
        let plan_approvals = Arc::clone(&self.plan_approvals);
        let plan_approval_seq = Arc::clone(&self.plan_approval_seq);
        let elicitations = Arc::clone(&self.elicitations);
        let elicitation_seq = Arc::clone(&self.elicitation_seq);
        let terminals = Arc::clone(&self.terminals);
        thread::spawn(move || {
            read_stdout(
                reader_app,
                reader_session,
                stdout,
                reader_process,
                pending,
                permissions,
                permission_seq,
                questions,
                question_seq,
                plan_approvals,
                plan_approval_seq,
                elicitations,
                elicitation_seq,
                terminals,
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

        // ── Phase 1: initialize (per-agent capabilities — Codeg gates) ──
        let client_capabilities =
            crate::agent_registry::build_client_capabilities_json(agent_id.as_deref());
        let initialized = request(
            &process,
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": client_capabilities,
                "clientInfo": { "name": "Marionette", "version": "0.1.0" }
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
        // Project-level Skill/MCP lend (P0). Policy may skip inject (OpenClaw/Pi)
        // without clearing the shared selection UI.
        let (mcp_servers, mcp_skipped) =
            if crate::agent_registry::should_inject_mcp(agent_id.as_deref()) {
                match agent_id.as_deref() {
                    Some(agent) => crate::context_inventory::mcp_payload_for_agent(
                        std::path::Path::new(&cwd),
                        agent,
                        supports_http,
                        supports_sse,
                    ),
                    None => (Vec::new(), Vec::new()),
                }
            } else {
                let reason = agent_id
                    .as_deref()
                    .unwrap_or("(none)")
                    .to_string();
                (
                    Vec::new(),
                    vec![format!("{reason} (mcp wire policy: skip/never)")],
                )
            };
        if !mcp_servers.is_empty() || !mcp_skipped.is_empty() {
            // Names + header *counts* only — values are secrets (Bearer tokens).
            let injected: Vec<String> = mcp_servers
                .iter()
                .filter_map(|server| {
                    let name = server.get("name").and_then(Value::as_str)?;
                    let n = server
                        .get("headers")
                        .and_then(Value::as_array)
                        .map(|a| a.len())
                        .unwrap_or(0);
                    Some(if n > 0 {
                        format!("{name}(headers={n})")
                    } else {
                        name.to_string()
                    })
                })
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
                &format!(
                    "mcp inject: {}",
                    if injected.is_empty() {
                        "(none)".to_string()
                    } else {
                        injected.join(", ")
                    }
                ),
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
                    // Seeds the Usage panel before the first usage_update lands
                    // (and is the only size Grok ever reports).
                    "contextSize": advertised_context_size(&new_session),
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
        for update in updates {
            let skip_config_option = process
                .config_option_unsupported
                .load(Ordering::Relaxed);
            let attempted = match &update.config_id {
                // Already know this agent has no set_config_option — don't probe again.
                Some(_) if skip_config_option => self.legacy_set_config(
                    &process,
                    &agent_session_id,
                    session_id,
                    &update,
                ),
                Some(config_id) => {
                    let params = json!({
                        "sessionId": agent_session_id,
                        "configId": config_id,
                        "value": update.value,
                    });
                    // Quiet: a -32601 here is an expected capability answer, not a
                    // fault. Without this the UI renders "Agent error: Method not
                    // found" even when the legacy fallback succeeds.
                    match request_quiet(&process, "session/set_config_option", params) {
                        Ok(value) => Ok(value),
                        // Pre-v2 agents (Grok) have no set_config_option at all;
                        // the per-knob RPCs are the only way in. A value rejection
                        // is a different thing and must still surface.
                        Err(error) if is_method_not_found(&error) => {
                            process
                                .config_option_unsupported
                                .store(true, Ordering::Relaxed);
                            self.legacy_set_config(
                                &process,
                                &agent_session_id,
                                session_id,
                                &update,
                            )
                        }
                        Err(error) => Err(error),
                    }
                }
                // No advertised config id: legacy RPC is the only transport.
                None => self.legacy_set_config(&process, &agent_session_id, session_id, &update),
            };
            last_result = attempted?;

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
                    let id = update.config_id.clone().unwrap_or_else(|| {
                        match update.target {
                            ConfigTarget::Model => "model",
                            ConfigTarget::Mode => "mode",
                            ConfigTarget::Effort => "effort",
                            ConfigTarget::Other => "",
                        }
                        .to_string()
                    });
                    apply_local_config_change(caps, &id, &update.value);
                }
            }
        }
        Ok(last_result)
    }

    /// Pre-v2 ACP per-knob RPCs, used when `session/set_config_option` is absent.
    ///
    /// Grok exposes exactly these: `session/set_mode` for plan/build, and
    /// `session/set_model` for both the model and (via `_meta.reasoningEffort`)
    /// the reasoning level.
    fn legacy_set_config(
        &self,
        process: &Arc<AcpProcess>,
        agent_session_id: &str,
        session_id: &str,
        update: &ConfigUpdate,
    ) -> Result<Value, String> {
        match update.target {
            ConfigTarget::Mode => {
                let mode_id = update
                    .value
                    .as_str()
                    .ok_or_else(|| "Mode id must be a string".to_string())?;
                request(
                    process,
                    "session/set_mode",
                    json!({ "sessionId": agent_session_id, "modeId": mode_id }),
                )
            }
            ConfigTarget::Model => {
                let model_id = update
                    .value
                    .as_str()
                    .ok_or_else(|| "Model id must be a string".to_string())?;
                let mut params = json!({
                    "sessionId": agent_session_id,
                    "modelId": model_id,
                });
                // A bare set_model snaps effort back to the model default, which
                // would leave the Effort chip claiming a level the agent dropped.
                // Re-assert whatever the session is on.
                if let Some(effort) = self
                    .get_capabilities(session_id)
                    .and_then(|caps| caps.current_effort_id)
                    .filter(|id| matches!(id.as_str(), "low" | "medium" | "high"))
                {
                    params["_meta"] = json!({ "reasoningEffort": effort });
                }
                request(process, "session/set_model", params)
            }
            ConfigTarget::Effort => {
                // Effort rides along with the model on this transport, so we need
                // whatever model the session is on right now.
                let caps = self.get_capabilities(session_id);
                let model_id = caps
                    .as_ref()
                    .and_then(|c| c.current_model.clone())
                    .or_else(|| {
                        caps.as_ref()
                            .and_then(|c| c.models.first().map(|m| m.id.clone()))
                    })
                    .ok_or_else(|| {
                        "Cannot set effort: no current model known for this session".to_string()
                    })?;
                let effort = update
                    .value
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| update.value.as_f64().map(numeric_effort_to_level))
                    .ok_or_else(|| "Effort must be a string level or number".to_string())?;
                request(
                    process,
                    "session/set_model",
                    json!({
                        "sessionId": agent_session_id,
                        "modelId": model_id,
                        "_meta": { "reasoningEffort": effort },
                    }),
                )
            }
            ConfigTarget::Other => Err(format!(
                "Agent does not support session/set_config_option, and \"{}\" has no legacy equivalent",
                update.config_id.as_deref().unwrap_or("(unknown)")
            )),
        }
    }

    /// Resolve a UI-gated `session/request_permission` **or** elicitation approval.
    pub fn respond_permission(
        &self,
        request_id: &str,
        option_id: &str,
    ) -> Result<(), String> {
        // Prefer classic ACP permission waiters.
        let classic = {
            let mut map = self
                .permissions
                .lock()
                .map_err(|_| "Permission lock poisoned".to_string())?;
            map.remove(request_id)
        };
        if let Some(pending) = classic {
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
            return Ok(());
        }

        // Codex elicitation approval-style (same permission card UI).
        let elic = {
            let mut map = self
                .elicitations
                .lock()
                .map_err(|_| "Elicitation lock poisoned".to_string())?;
            map.remove(request_id)
        };
        let Some(pending) = elic else {
            return Err(format!(
                "Unknown or expired permission request: {request_id}"
            ));
        };
        if pending.kind != "approval" {
            // Put back if user hit wrong responder — re-insert
            if let Ok(mut map) = self.elicitations.lock() {
                map.insert(request_id.to_string(), pending);
            }
            return Err(format!(
                "Elicitation {request_id} is not approval-style; use respond_question"
            ));
        }
        let approval = crate::elicitation::approval_from_stored(&pending.stored)
            .ok_or_else(|| "Corrupt elicitation approval state".to_string())?;
        write_response(
            &pending.process,
            pending.rpc_id,
            crate::elicitation::build_approval_response(&approval, option_id),
        );
        Ok(())
    }

    /// Resolve `_x.ai/ask_user_question` **or** elicitation questions form.
    ///
    /// Grok: internally-tagged `outcome`. Elicitation: `{ action, content }`.
    /// UI sends `answers` as `[{ question, selected: string[] }]`.
    pub fn respond_question(
        &self,
        request_id: &str,
        answers: Value,
        declined: bool,
    ) -> Result<(), String> {
        let classic = {
            let mut map = self
                .questions
                .lock()
                .map_err(|_| "Question lock poisoned".to_string())?;
            map.remove(request_id)
        };
        if let Some(pending) = classic {
            write_response(
                &pending.process,
                pending.rpc_id,
                build_ask_user_question_result(answers, declined),
            );
            return Ok(());
        }

        let elic = {
            let mut map = self
                .elicitations
                .lock()
                .map_err(|_| "Elicitation lock poisoned".to_string())?;
            map.remove(request_id)
        };
        let Some(pending) = elic else {
            return Err(format!(
                "Unknown or expired question request: {request_id}"
            ));
        };
        if pending.kind != "questions" {
            if let Ok(mut map) = self.elicitations.lock() {
                map.insert(request_id.to_string(), pending);
            }
            return Err(format!(
                "Elicitation {request_id} is not questions-style; use respond_permission"
            ));
        }
        let questions = crate::elicitation::questions_from_stored(&pending.stored)
            .ok_or_else(|| "Corrupt elicitation questions state".to_string())?;
        write_response(
            &pending.process,
            pending.rpc_id,
            crate::elicitation::build_questions_response(&questions, &answers, declined),
        );
        Ok(())
    }

    /// Resolve Grok `_x.ai/exit_plan_mode` (plan approval card).
    ///
    /// `decision`: `"approve"` | `"request_changes"` | `"abandon"`.
    /// Response wire (Codeg / Grok 0.2.111):
    /// `{ "outcome": "approved"|"keep_planning"|"abandoned", "feedback": "…" }`.
    /// Field is `outcome` (NOT `decision`). Unknown outcomes keep plan mode.
    pub fn respond_plan_approval(
        &self,
        request_id: &str,
        decision: &str,
        feedback: Option<String>,
    ) -> Result<(), String> {
        let pending = {
            let mut map = self
                .plan_approvals
                .lock()
                .map_err(|_| "Plan approval lock poisoned".to_string())?;
            map.remove(request_id).ok_or_else(|| {
                format!("Unknown or expired plan approval request: {request_id}")
            })?
        };
        write_response(
            &pending.process,
            pending.rpc_id,
            build_exit_plan_mode_response(decision, feedback.as_deref()),
        );
        Ok(())
    }

    /// Grok account billing / weekly credit usage (`_x.ai/billing`).
    ///
    /// This is what the TUI `/usage` panel shows (creditUsagePercent + period end).
    /// It is a pager/shell extension, not `session/set_config_option`, and is
    /// only available while a live ACP process is up.
    pub fn probe_billing(&self, session_id: &str) -> Result<Value, String> {
        let process = self.process(session_id)?;
        // Quiet: missing method on non-Grok agents must not flash "Agent error".
        request_quiet(&process, "_x.ai/billing", json!({}))
    }

    /// Fire `session/prompt` and return immediately so the UI can stream
    /// `session/update` events (thinking / tool_call / message chunks) live.
    /// Turn completion arrives later as a dedicated `turn/complete` acp-event
    /// (only for this prompt's JSON-RPC id — never for set_config echoes).
    ///
    /// `image_paths` are absolute filesystem paths; each is read, base64-encoded,
    /// and sent as an ACP `ContentBlock::Image` before the text block (when the
    /// agent advertised image prompt capability; otherwise images are skipped
    /// with a log line and the text still goes through).
    pub fn send_prompt(
        &self,
        session_id: &str,
        text: String,
        image_paths: Vec<String>,
    ) -> Result<Value, String> {
        let process = self.process(session_id)?;
        let agent_session_id = process
            .agent_session_id
            .lock()
            .map_err(|_| "ACP session id lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| "ACP session is not initialized".to_string())?;

        // Codeg concurrency gate: one prompt turn per connection. A second
        // send while the first is still open would race responses and make
        // "running" untrustworthy again.
        if process.in_flight_prompt_id.load(Ordering::SeqCst) != 0 {
            return Err(
                "A turn is already in flight on this session — interrupt it first".into(),
            );
        }

        let caps = self.get_capabilities(session_id);
        let images_ok = caps
            .as_ref()
            .map(|c| c.prompt_image)
            .unwrap_or(false);

        let mut prompt_blocks: Vec<Value> = Vec::new();
        let mut image_count = 0usize;
        if !image_paths.is_empty() {
            if images_ok {
                for path in &image_paths {
                    match load_image_content_block(path) {
                        Ok(block) => {
                            prompt_blocks.push(block);
                            image_count += 1;
                        }
                        Err(err) => {
                            crate::debug_log::append(
                                "acp",
                                "warn",
                                session_id,
                                "skip image in prompt",
                                Some(&format!("{path}: {err}")),
                            );
                        }
                    }
                }
            } else {
                crate::debug_log::append(
                    "acp",
                    "warn",
                    session_id,
                    "agent has no image prompt capability — sending text only",
                    Some(&format!("droppedImages={}", image_paths.len())),
                );
            }
        }
        if !text.is_empty() {
            prompt_blocks.push(json!({ "type": "text", "text": text }));
        }
        if prompt_blocks.is_empty() {
            return Err("Empty prompt (no text and no images)".into());
        }

        let id = process.next_id.fetch_add(1, Ordering::Relaxed);
        // Latch before write so a fast agent response cannot race past an
        // empty in_flight and be misclassified as a non-turn RPC. CAS so a
        // concurrent send loses cleanly instead of overwriting.
        if process
            .in_flight_prompt_id
            .compare_exchange(0, id, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(
                "A turn is already in flight on this session — interrupt it first".into(),
            );
        }
        // No pending waiter: response is still emitted on the event bus in read_stdout.
        if let Err(err) = write_message(
            &process,
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "session/prompt",
                "params": {
                    "sessionId": agent_session_id,
                    "prompt": prompt_blocks
                }
            }),
        ) {
            // Only clear if we still own this turn (cancel may have taken it).
            let _ = process.in_flight_prompt_id.compare_exchange(
                id,
                0,
                Ordering::SeqCst,
                Ordering::SeqCst,
            );
            return Err(err);
        }
        // Cancel raced after CAS but before/during write: UI already has
        // turn/complete(cancelled); re-issue cancel so the agent does not keep
        // a zombie turn we no longer track.
        if process.in_flight_prompt_id.load(Ordering::SeqCst) != id {
            let _ = notify(
                &process,
                "session/cancel",
                json!({ "sessionId": agent_session_id }),
            );
            crate::debug_log::append(
                "acp",
                "warn",
                session_id,
                "session/prompt wrote after cancel — re-issued session/cancel",
                Some(&format!("rpcId={id}")),
            );
            return Ok(json!({
                "accepted": true,
                "id": id,
                "images": image_count,
                "cancelledDuringSend": true
            }));
        }
        crate::debug_log::append(
            "acp",
            "info",
            session_id,
            "session/prompt sent (async stream)",
            Some(&format!(
                "rpcId={id} chars={} images={image_count}",
                text.len()
            )),
        );
        Ok(json!({ "accepted": true, "id": id, "images": image_count }))
    }

    pub fn cancel(&self, session_id: &str) -> Result<(), String> {
        let process = self.process(session_id)?;
        let agent_session_id = process
            .agent_session_id
            .lock()
            .map_err(|_| "ACP session id lock poisoned".to_string())?
            .clone()
            .ok_or_else(|| "ACP session is not initialized".to_string())?;
        // Codeg: free the client immediately — do not wait for the agent to
        // honour session/cancel (it may hang inside a tool). Clear in-flight
        // so a late prompt response is ignored; emit turn/complete so UI
        // leaves "Working" without needing a second signal.
        let had_turn = take_in_flight_prompt_id(&process.in_flight_prompt_id).is_some();
        notify(
            &process,
            "session/cancel",
            json!({ "sessionId": agent_session_id }),
        )?;
        if had_turn {
            // Best-effort: we need the AppHandle to emit. cancel is invoked
            // from commands without app in AcpService — use a deferred path
            // via the registered emitter if present.
            emit_turn_complete_for_session(
                session_id,
                "response",
                "cancelled",
                json!({ "source": "session/cancel" }),
            );
        }
        Ok(())
    }

    pub fn stop(&self, session_id: &str) -> Result<(), String> {
        // Unpark exit_plan_mode / questions / permissions BEFORE killing the
        // process so keep_planning can still flush on stdin (Codeg: disconnect
        // mid-approval must not look like approve).
        self.drop_pending_for_session(session_id);

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
            // End any open turn before kill so listeners do not wait forever.
            if take_in_flight_prompt_id(&process.in_flight_prompt_id).is_some() {
                emit_turn_complete_for_session(
                    session_id,
                    "response",
                    "cancelled",
                    json!({ "source": "process/stop" }),
                );
            }
            // Kill any agent-owned shells before the agent process itself.
            if let Ok(guard) = process.agent_session_id.lock() {
                if let Some(agent_sid) = guard.as_deref() {
                    self.terminals.release_all_for_agent(agent_sid);
                }
            }
            let mut child = process
                .child
                .lock()
                .map_err(|_| "ACP child lock poisoned".to_string())?;
            let _ = child.kill();
            let _ = child.wait();
        }
        self.terminals.clear_session(session_id);
        // Dropping the sender lets the dispatch thread flush its backlog and
        // exit. Done here rather than in the reader's teardown so a restart
        // cannot race and unregister the *new* session's queue.
        unregister_emitter(session_id);
        Ok(())
    }

    /// Drop UI-gated waiters for this session (see `drop_ui_pending_for_session`).
    fn drop_pending_for_session(&self, session_id: &str) {
        drop_ui_pending_for_session(
            session_id,
            &self.plan_approvals,
            &self.questions,
            &self.permissions,
            &self.elicitations,
        );
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

/// Which session knob an update is aiming at.
///
/// Needed because the pre-v2 ACP methods (`session/set_mode`, `session/set_model`)
/// are *per-knob* RPCs, so a `session/set_config_option` rejection can only be
/// retried if we still know what the config id meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigTarget {
    Model,
    Mode,
    Effort,
    Other,
}

#[derive(Debug, Clone)]
struct ConfigUpdate {
    target: ConfigTarget,
    /// `None` = the agent advertised no config option for this knob, so only the
    /// legacy per-knob RPC can carry it. Never invent an id here: sending a
    /// fabricated `effort` makes Claude answer "Unknown config option: effort".
    config_id: Option<String>,
    value: Value,
}

fn expand_config_updates(
    config: &Value,
    caps: Option<&CapabilitySnapshot>,
) -> Result<Vec<ConfigUpdate>, String> {
    let obj = config
        .as_object()
        .ok_or_else(|| "Config update must be a JSON object".to_string())?;

    if let (Some(config_id), Some(value)) = (obj.get("configId"), obj.get("value")) {
        let id = config_id
            .as_str()
            .ok_or_else(|| "configId must be a string".to_string())?
            .to_string();
        let target = match id.as_str() {
            "model" => ConfigTarget::Model,
            "mode" | "approval-policy" | "approvalPolicy" => ConfigTarget::Mode,
            "effort" | "reasoning" | "reasoning-effort" | "thought_level" => ConfigTarget::Effort,
            _ => ConfigTarget::Other,
        };
        return Ok(vec![ConfigUpdate {
            target,
            config_id: Some(id),
            value: value.clone(),
        }]);
    }

    let mut updates = Vec::new();
    if let Some(model) = obj.get("model") {
        updates.push(ConfigUpdate {
            target: ConfigTarget::Model,
            config_id: Some(
                caps.and_then(|c| c.model_config_id.clone())
                    .unwrap_or_else(|| "model".to_string()),
            ),
            value: model.clone(),
        });
    }
    if let Some(mode) = obj.get("mode") {
        updates.push(ConfigUpdate {
            target: ConfigTarget::Mode,
            config_id: Some(
                caps.and_then(|c| c.mode_config_id.clone())
                    .unwrap_or_else(|| "mode".to_string()),
            ),
            value: mode.clone(),
        });
    }
    // `effortId` (string level) and `thinkingEffort` (0..1) are the same knob.
    if let Some(effort) = obj.get("effortId").or_else(|| obj.get("thinkingEffort")) {
        updates.push(ConfigUpdate {
            target: ConfigTarget::Effort,
            // Absent id is legal: Grok carries effort on session/set_model instead.
            config_id: caps.and_then(|c| c.effort_config_id.clone()),
            value: effort.clone(),
        });
    }
    Ok(updates)
}

/// True when the agent does not implement the method at all (JSON-RPC -32601),
/// as opposed to rejecting the value we sent.
fn is_method_not_found(error: &str) -> bool {
    error.contains("-32601") || error.to_lowercase().contains("method not found")
}

/// Collapse a 0..1 UI strength onto the discrete levels legacy agents accept.
fn numeric_effort_to_level(n: f64) -> String {
    if n <= 0.25 {
        "low".to_string()
    } else if n >= 0.75 {
        "high".to_string()
    } else {
        "medium".to_string()
    }
}

/// Context window size the agent advertised for the model the session is on.
///
/// Standard ACP only reports context size inside `usage_update`, which some
/// agents (Grok) never send — but they do publish the ceiling per model at
/// session setup, so the UI can show `used / size` from turn one.
fn advertised_context_size(session_response: &Value) -> Option<u64> {
    let models = session_response.get("models")?;
    let available = models.get("availableModels")?.as_array()?;
    let current = models.get("currentModelId").and_then(Value::as_str);
    let pick = available
        .iter()
        .find(|m| {
            current.is_some() && m.get("modelId").and_then(Value::as_str) == current
        })
        .or_else(|| available.first())?;

    for key in ["totalContextTokens", "contextWindow", "contextWindowTokens"] {
        if let Some(n) = pick
            .get("_meta")
            .and_then(|meta| meta.get(key))
            .and_then(Value::as_u64)
        {
            return Some(n);
        }
        if let Some(n) = pick.get(key).and_then(Value::as_u64) {
            return Some(n);
        }
    }
    None
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
    let mut caps = if let Ok(response) =
        serde_json::from_value::<acp_schema::NewSessionResponse>(session_response.clone())
    {
        // Typed ACP NewSessionResponse has no `models` field — Grok (and other
        // pre-v2 agents) put the list under top-level `models.currentModelId` /
        // `availableModels`. Ignoring that left current_model=None and made
        // effort changes fail with "no current model known for this session".
        parse_capabilities_from_typed(response)
    } else {
        parse_capabilities_fallback(session_response)
    };
    merge_legacy_models_field(session_response, &mut caps);
    caps
}

/// Grok / pre-v2 ACP: `session/new` returns
/// `{ models: { currentModelId, availableModels: [{ modelId, name, _meta }] } }`.
fn merge_legacy_models_field(session_response: &Value, caps: &mut CapabilitySnapshot) {
    let Some(models_obj) = session_response.get("models") else {
        return;
    };

    if caps.current_model.is_none() {
        if let Some(id) = models_obj
            .get("currentModelId")
            .or_else(|| models_obj.get("current_model_id"))
            .and_then(Value::as_str)
        {
            caps.current_model = Some(id.to_string());
        }
    }

    let available = models_obj
        .get("availableModels")
        .or_else(|| models_obj.get("available_models"))
        .and_then(Value::as_array);

    if let Some(list) = available {
        if caps.models.is_empty() {
            for entry in list {
                let id = entry
                    .get("modelId")
                    .or_else(|| entry.get("model_id"))
                    .or_else(|| entry.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if id.is_empty() {
                    continue;
                }
                let name = entry
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or(id.as_str())
                    .to_string();
                let description = entry
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                caps.models.push(ModelDef {
                    id: id.clone(),
                    label: model_display_label(&name, description.as_deref()),
                    description,
                });
            }
        }

        // Effort levels live on the current model's `_meta` for Grok.
        let current = caps.current_model.clone();
        let pick = list.iter().find(|m| {
            let id = m
                .get("modelId")
                .or_else(|| m.get("model_id"))
                .or_else(|| m.get("id"))
                .and_then(Value::as_str);
            current.as_deref().zip(id).is_some_and(|(c, i)| c == i)
        }).or_else(|| list.first());

        if let Some(entry) = pick {
            if let Some(meta) = entry.get("_meta") {
                if caps.current_effort_id.is_none() {
                    if let Some(effort) = meta
                        .get("reasoningEffort")
                        .or_else(|| meta.get("reasoning_effort"))
                        .and_then(Value::as_str)
                    {
                        caps.current_effort_id = Some(effort.to_string());
                    }
                }
                if caps.effort_options.is_empty() {
                    if let Some(levels) = meta
                        .get("reasoningEfforts")
                        .or_else(|| meta.get("reasoning_efforts"))
                        .and_then(Value::as_array)
                    {
                        for level in levels {
                            let id = level
                                .get("id")
                                .or_else(|| level.get("value"))
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string();
                            if id.is_empty() {
                                continue;
                            }
                            let label = level
                                .get("label")
                                .or_else(|| level.get("name"))
                                .and_then(Value::as_str)
                                .unwrap_or(id.as_str())
                                .to_string();
                            caps.effort_options.push(ModeDef { id, label });
                        }
                    }
                }
            }
        }
    }

    // Ensure current is in the list for the UI / set_model path.
    if let Some(current) = &caps.current_model {
        if !caps.models.iter().any(|m| &m.id == current) {
            caps.models.insert(
                0,
                ModelDef {
                    id: current.clone(),
                    label: current.clone(),
                    description: None,
                },
            );
        }
    }
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
        prompt_image: true,
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
        prompt_image: true,
    }
}

/// Read an image file into an ACP image content block (base64 + mimeType).
fn load_image_content_block(path: &str) -> Result<Value, String> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    use std::fs;
    use std::path::Path;

    let p = Path::new(path);
    if !p.is_file() {
        return Err(format!("not a file: {path}"));
    }
    let meta = fs::metadata(p).map_err(|e| format!("stat failed: {e}"))?;
    // ~12MB raw → ~16MB base64; keep IPC and agent stdin sane.
    const MAX_BYTES: u64 = 12 * 1024 * 1024;
    if meta.len() > MAX_BYTES {
        return Err(format!(
            "image too large ({} MB > 12 MB): {path}",
            meta.len() / (1024 * 1024)
        ));
    }
    let bytes = fs::read(p).map_err(|e| format!("read failed: {e}"))?;
    let mime = mime_for_image_path(path);
    let data = B64.encode(&bytes);
    Ok(json!({
        "type": "image",
        "data": data,
        "mimeType": mime,
        "uri": format!("file://{}", path.replace('\\', "/")),
    }))
}

fn mime_for_image_path(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".bmp") {
        "image/bmp"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else {
        "application/octet-stream"
    }
}

fn command_looks_like_grok(command: &str) -> bool {
    let base = std::path::Path::new(command)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(command)
        .to_ascii_lowercase();
    base == "grok" || base.starts_with("grok-")
}

/// `grok --trust agent stdio` — `--trust` is a top-level flag, before `agent`.
fn with_grok_trust_flag(args: Vec<String>) -> Vec<String> {
    if args.iter().any(|a| a == "--trust") {
        return args;
    }
    let mut out = Vec::with_capacity(args.len() + 1);
    out.push("--trust".to_string());
    out.extend(args);
    out
}

fn request(process: &Arc<AcpProcess>, method: &str, params: Value) -> Result<Value, String> {
    request_with_timeout(process, method, params, Duration::from_secs(60), false)
}

/// Like `request`, but a failure is not emitted as a user-visible ACP error.
///
/// Used for capability probes: the caller inspects the Err and either retries
/// on a different transport or surfaces a real error itself.
fn request_quiet(process: &Arc<AcpProcess>, method: &str, params: Value) -> Result<Value, String> {
    request_with_timeout(process, method, params, Duration::from_secs(60), true)
}

fn request_with_timeout(
    process: &Arc<AcpProcess>,
    method: &str,
    params: Value,
    timeout: Duration,
    quiet: bool,
) -> Result<Value, String> {
    let id = process.next_id.fetch_add(1, Ordering::Relaxed);
    if quiet {
        if let Ok(mut quiet_ids) = process.quiet_ids.lock() {
            quiet_ids.insert(id);
        }
    }
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
        let _ = process
            .quiet_ids
            .lock()
            .map(|mut quiet_ids| quiet_ids.remove(&id));
        return Err(error);
    }
    match receiver.recv_timeout(timeout) {
        Ok(result) => {
            // Successful quiet probes still clean up; failures are cleaned in
            // read_stdout when deciding whether to emit the error.
            let _ = process
                .quiet_ids
                .lock()
                .map(|mut quiet_ids| quiet_ids.remove(&id));
            result
        }
        Err(error) => {
            let _ = process
                .pending
                .lock()
                .map(|mut pending| pending.remove(&id));
            let _ = process
                .quiet_ids
                .lock()
                .map(|mut quiet_ids| quiet_ids.remove(&id));
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
    questions: Arc<Mutex<HashMap<String, PendingQuestion>>>,
    question_seq: Arc<AtomicU64>,
    plan_approvals: Arc<Mutex<HashMap<String, PendingPlanApproval>>>,
    plan_approval_seq: Arc<AtomicU64>,
    elicitations: Arc<Mutex<HashMap<String, PendingElicitation>>>,
    elicitation_seq: Arc<AtomicU64>,
    terminals: Arc<crate::terminal_runtime::TerminalRuntime>,
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
                // Capability probes register here before the write. Their -32601
                // is how a pre-v2 agent declines set_config_option; the caller
                // recovers via legacy RPCs. Emitting that as kind="error" made
                // the UI scream "Agent error: Method not found" on every model
                // / mode / effort change against Grok.
                let quiet_failure = result.is_err()
                    && process
                        .quiet_ids
                        .lock()
                        .map(|mut set| set.remove(&id))
                        .unwrap_or(false);
                // Only the outstanding session/prompt response ends a turn.
                // set_config / set_mode / probes share the same JSON-RPC channel
                // and must not flip the UI out of "Working" (Codeg: TurnComplete
                // is prompt-path exclusive).
                let is_prompt_turn = {
                    let current = process.in_flight_prompt_id.load(Ordering::SeqCst);
                    current != 0 && current == id
                };
                if is_prompt_turn {
                    let _ = take_in_flight_prompt_id(&process.in_flight_prompt_id);
                    let stop_reason = if result.is_ok() {
                        extract_stop_reason(message.get("result")).unwrap_or("end_turn")
                    } else {
                        "error"
                    };
                    let kind = if result.is_ok() { "response" } else { "error" };
                    emit_turn_complete(
                        &app,
                        &session_id,
                        kind,
                        stop_reason,
                        message.clone(),
                    );
                } else if !quiet_failure {
                    // Non-turn RPCs (config/model/mode): debug-visible only.
                    emit_event(
                        &app,
                        &session_id,
                        if result.is_ok() { "response" } else { "error" },
                        Some("rpc/response"),
                        message.clone(),
                    );
                } else {
                    // Keep a low-noise trail for debug without tripping App.tsx
                    // error → "Agent error" rendering.
                    emit_event(
                        &app,
                        &session_id,
                        "response",
                        Some("rpc/probe"),
                        message.clone(),
                    );
                }
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
                    &questions,
                    &question_seq,
                    &plan_approvals,
                    &plan_approval_seq,
                    &elicitations,
                    &elicitation_seq,
                    &terminals,
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
    // Close any open prompt turn before process/* so status cannot stay running.
    if take_in_flight_prompt_id(&process.in_flight_prompt_id).is_some() {
        let reason = if process.intentional_stop.load(Ordering::SeqCst) {
            "cancelled"
        } else {
            "error"
        };
        emit_turn_complete(
            &app,
            &session_id,
            if reason == "cancelled" {
                "response"
            } else {
                "error"
            },
            reason,
            json!({
                "message": "Agent process stream ended before the prompt response arrived.",
                "source": "stdout-eof",
            }),
        );
    }
    // Also drop Ask / Plan / Permission waiters so the UI is not stuck on a
    // dead process (write_response is best-effort if the pipe is already gone).
    drop_ui_pending_for_session(
        &session_id,
        &plan_approvals,
        &questions,
        &permissions,
        &elicitations,
    );
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
                "message": "Agent process stopped by Marionette.",
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

/// Clear UI-gated waiters for one session (shared by `stop` and stdout EOF).
fn drop_ui_pending_for_session(
    session_id: &str,
    plan_approvals: &Mutex<HashMap<String, PendingPlanApproval>>,
    questions: &Mutex<HashMap<String, PendingQuestion>>,
    permissions: &Mutex<HashMap<String, PendingPermission>>,
    elicitations: &Mutex<HashMap<String, PendingElicitation>>,
) {
    let prefix = format!("{session_id}:");
    if let Ok(mut map) = plan_approvals.lock() {
        let keys: Vec<String> = map
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for key in keys {
            if let Some(pending) = map.remove(&key) {
                write_response(
                    &pending.process,
                    pending.rpc_id,
                    exit_plan_mode_disconnect_response(),
                );
            }
        }
    }
    if let Ok(mut map) = questions.lock() {
        let keys: Vec<String> = map
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for key in keys {
            if let Some(pending) = map.remove(&key) {
                write_response(
                    &pending.process,
                    pending.rpc_id,
                    json!({ "outcome": "cancelled" }),
                );
            }
        }
    }
    if let Ok(mut map) = permissions.lock() {
        let keys: Vec<String> = map
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for key in keys {
            if let Some(pending) = map.remove(&key) {
                let option_id =
                    pick_reject_option(&pending.options).unwrap_or_else(|| "cancel".to_string());
                write_response(
                    &pending.process,
                    pending.rpc_id,
                    json!({
                        "outcome": {
                            "outcome": "selected",
                            "optionId": option_id,
                        }
                    }),
                );
            }
        }
    }
    if let Ok(mut map) = elicitations.lock() {
        let keys: Vec<String> = map
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect();
        for key in keys {
            if let Some(pending) = map.remove(&key) {
                write_response(
                    &pending.process,
                    pending.rpc_id,
                    crate::elicitation::elicitation_cancel(),
                );
            }
        }
    }
}

/// Client methods the agent may call. Permission / questions / plan exit are
/// gated by the UI; terminal + fs run off-thread.
fn handle_agent_request(
    process: &Arc<AcpProcess>,
    app: &AppHandle,
    session_id: &str,
    id: Value,
    method: &str,
    params: Value,
    permissions: &Arc<Mutex<HashMap<String, PendingPermission>>>,
    permission_seq: &Arc<AtomicU64>,
    questions: &Arc<Mutex<HashMap<String, PendingQuestion>>>,
    question_seq: &Arc<AtomicU64>,
    plan_approvals: &Arc<Mutex<HashMap<String, PendingPlanApproval>>>,
    plan_approval_seq: &Arc<AtomicU64>,
    elicitations: &Arc<Mutex<HashMap<String, PendingElicitation>>>,
    elicitation_seq: &Arc<AtomicU64>,
    terminals: &Arc<crate::terminal_runtime::TerminalRuntime>,
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
                thread::sleep(Duration::from_secs(TIMEOUT_PERMISSION_SECS));
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
        // Grok Build mid-turn multi-choice (also used heavily in Plan mode).
        // Wire shape: method `x.ai/ask_user_question` (also `_x.ai/` prefix),
        // params.questions[] with { question, options[{label,description}], multiSelect }.
        // Response must be an internally-tagged enum on `outcome` (see
        // `build_ask_user_question_result`) — bare `{}` fails with
        // "missing field `outcome`".
        "x.ai/ask_user_question" | "_x.ai/ask_user_question" | "ask_user_question" => {
            let seq = question_seq.fetch_add(1, Ordering::Relaxed) + 1;
            let request_id = format!("{session_id}:ask:{seq}");
            let tool_call_id = params
                .get("toolCallId")
                .or_else(|| params.get("tool_call_id"))
                .and_then(Value::as_str)
                .map(str::to_string);
            let mode = params
                .get("mode")
                .and_then(Value::as_str)
                .map(str::to_string);
            let qs = params
                .get("questions")
                .cloned()
                .unwrap_or_else(|| Value::Array(vec![]));
            if let Ok(mut map) = questions.lock() {
                map.insert(
                    request_id.clone(),
                    PendingQuestion {
                        process: Arc::clone(process),
                        rpc_id: id.clone(),
                    },
                );
            }
            emit_event(
                app,
                session_id,
                "request",
                Some("question/prompt"),
                json!({
                    "requestId": request_id.clone(),
                    "sessionId": session_id,
                    "toolCallId": tool_call_id,
                    "mode": mode,
                    "questions": qs,
                }),
            );
            // Auto-decline if the user never answers (agent must not hang forever).
            let timeout_app = app.clone();
            let timeout_session = session_id.to_string();
            let timeout_req = request_id;
            let timeout_qs = Arc::clone(questions);
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(TIMEOUT_QUESTION_SECS));
                let pending = {
                    let Ok(mut map) = timeout_qs.lock() else {
                        return;
                    };
                    map.remove(&timeout_req)
                };
                if let Some(pending) = pending {
                    // Same shape as user Skip — Grok requires `outcome`.
                    write_response(
                        &pending.process,
                        pending.rpc_id,
                        json!({ "outcome": "cancelled" }),
                    );
                    emit_event(
                        &timeout_app,
                        &timeout_session,
                        "system",
                        Some("question/timeout"),
                        json!({ "requestId": timeout_req }),
                    );
                }
            });
        }
        // Codex form elicitation (Plan request_user_input, MCP approvals, …).
        // Only advertised for Codex via clientCapabilities.elicitation.form.
        // Approval-style → permission/prompt; questions → question/prompt.
        "elicitation/create" => {
            let plan = match crate::elicitation::classify_elicitation(&params) {
                Ok(p) => p,
                Err(e) => {
                    write_response(
                        process,
                        id,
                        crate::elicitation::elicitation_decline(),
                    );
                    emit_event(
                        app,
                        session_id,
                        "system",
                        Some("elicitation/declined"),
                        json!({ "reason": e }),
                    );
                    return;
                }
            };
            let seq = elicitation_seq.fetch_add(1, Ordering::Relaxed) + 1;
            let request_id = format!("{session_id}:elic:{seq}");
            match plan {
                crate::elicitation::ElicitationPlan::Approval(approval) => {
                    let options = crate::elicitation::approval_options_for_ui(&approval);
                    if let Ok(mut map) = elicitations.lock() {
                        map.insert(
                            request_id.clone(),
                            PendingElicitation {
                                process: Arc::clone(process),
                                rpc_id: id.clone(),
                                kind: "approval".into(),
                                stored: crate::elicitation::approval_to_stored(&approval),
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
                            "title": if approval.message.is_empty() {
                                "Approval required".to_string()
                            } else {
                                approval.message.clone()
                            },
                            "detail": approval.tool_call_id.clone(),
                            "options": options,
                            "source": "elicitation",
                        }),
                    );
                    let timeout_app = app.clone();
                    let timeout_session = session_id.to_string();
                    let timeout_req = request_id;
                    let timeout_el = Arc::clone(elicitations);
                    thread::spawn(move || {
                        thread::sleep(Duration::from_secs(TIMEOUT_QUESTION_SECS));
                        let pending = {
                            let Ok(mut map) = timeout_el.lock() else {
                                return;
                            };
                            map.remove(&timeout_req)
                        };
                        if let Some(pending) = pending {
                            write_response(
                                &pending.process,
                                pending.rpc_id,
                                crate::elicitation::elicitation_decline(),
                            );
                            emit_event(
                                &timeout_app,
                                &timeout_session,
                                "system",
                                Some("permission/timeout"),
                                json!({ "requestId": timeout_req }),
                            );
                        }
                    });
                }
                crate::elicitation::ElicitationPlan::Questions(questions) => {
                    let qs = crate::elicitation::questions_for_ui(&questions);
                    if let Ok(mut map) = elicitations.lock() {
                        map.insert(
                            request_id.clone(),
                            PendingElicitation {
                                process: Arc::clone(process),
                                rpc_id: id.clone(),
                                kind: "questions".into(),
                                stored: crate::elicitation::questions_to_stored(&questions),
                            },
                        );
                    }
                    emit_event(
                        app,
                        session_id,
                        "request",
                        Some("question/prompt"),
                        json!({
                            "requestId": request_id.clone(),
                            "sessionId": session_id,
                            "toolCallId": questions.tool_call_id,
                            "mode": "elicitation",
                            "questions": qs,
                        }),
                    );
                    let timeout_app = app.clone();
                    let timeout_session = session_id.to_string();
                    let timeout_req = request_id;
                    let timeout_el = Arc::clone(elicitations);
                    thread::spawn(move || {
                        thread::sleep(Duration::from_secs(TIMEOUT_QUESTION_SECS));
                        let pending = {
                            let Ok(mut map) = timeout_el.lock() else {
                                return;
                            };
                            map.remove(&timeout_req)
                        };
                        if let Some(pending) = pending {
                            write_response(
                                &pending.process,
                                pending.rpc_id,
                                crate::elicitation::elicitation_decline(),
                            );
                            emit_event(
                                &timeout_app,
                                &timeout_session,
                                "system",
                                Some("question/timeout"),
                                json!({ "requestId": timeout_req }),
                            );
                        }
                    });
                }
            }
        }
        // Grok plan mode: agent finished planning and BLOCKS on user approval.
        // Wire (Codeg, confirmed Grok 0.2.111):
        //   method: `_x.ai/exit_plan_mode` (also `x.ai/` / bare)
        //   params: { sessionId, toolCallId, planContent }
        //   reply:  { outcome: "approved"|"keep_planning"|"abandoned", feedback }
        // `planContent` is often null — Grok reads plan.md itself after approve.
        "x.ai/exit_plan_mode" | "_x.ai/exit_plan_mode" | "exit_plan_mode" => {
            let seq = plan_approval_seq.fetch_add(1, Ordering::Relaxed) + 1;
            let request_id = format!("{session_id}:plan:{seq}");
            let (plan_markdown, tool_call_id) = parse_exit_plan_mode_request(&params);
            if let Ok(mut map) = plan_approvals.lock() {
                map.insert(
                    request_id.clone(),
                    PendingPlanApproval {
                        process: Arc::clone(process),
                        rpc_id: id.clone(),
                    },
                );
            }
            emit_event(
                app,
                session_id,
                "request",
                Some("plan/approval"),
                json!({
                    "requestId": request_id.clone(),
                    "sessionId": session_id,
                    "toolCallId": tool_call_id,
                    "planMarkdown": plan_markdown,
                }),
            );
            // Long timeout: planning review is human-paced. Keep plan mode
            // active (do NOT auto-approve) so reconnect can re-surface.
            let timeout_app = app.clone();
            let timeout_session = session_id.to_string();
            let timeout_req = request_id;
            let timeout_plans = Arc::clone(plan_approvals);
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(TIMEOUT_PLAN_APPROVAL_SECS));
                let pending = {
                    let Ok(mut map) = timeout_plans.lock() else {
                        return;
                    };
                    map.remove(&timeout_req)
                };
                if let Some(pending) = pending {
                    write_response(
                        &pending.process,
                        pending.rpc_id,
                        exit_plan_mode_disconnect_response(),
                    );
                    emit_event(
                        &timeout_app,
                        &timeout_session,
                        "system",
                        Some("plan/timeout"),
                        json!({ "requestId": timeout_req }),
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
        // ACP terminal host (create / output / wait / kill / release).
        // `wait_for_exit` blocks until the child exits — never run on the
        // stdout reader thread or the agent pipeline stalls.
        "terminal/create"
        | "terminal/output"
        | "terminal/wait_for_exit"
        | "terminal/kill"
        | "terminal/release" => {
            let process = Arc::clone(process);
            let terminals = Arc::clone(terminals);
            let marionette_session = session_id.to_string();
            let method = method.to_string();
            let agent_sid = process
                .agent_session_id
                .lock()
                .ok()
                .and_then(|g| g.clone());
            thread::spawn(move || {
                match terminals.handle(
                    &method,
                    &params,
                    &marionette_session,
                    agent_sid.as_deref(),
                ) {
                    Ok(result) => write_response(&process, id, result),
                    Err(err) => write_error_response(
                        &process,
                        id,
                        err.code() as i64,
                        err.message(),
                    ),
                }
            });
        }
        other => {
            // Unknown client methods: reject so the agent can recover instead of hang
            write_error_response(
                process,
                id,
                -32601,
                &format!("Method not implemented by Marionette: {other}"),
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

const MAX_PLAN_MARKDOWN_CHARS: usize = 262_144;
const MAX_PLAN_FEEDBACK_CHARS: usize = 16_384;

/// Parse Grok `_x.ai/exit_plan_mode` params → (plan_markdown, tool_call_id).
/// Shape: `{ sessionId, toolCallId, planContent }`. Empty/missing plan is valid.
fn parse_exit_plan_mode_request(params: &Value) -> (String, Option<String>) {
    let plan_markdown: String = params
        .get("planContent")
        .or_else(|| params.get("plan_content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(MAX_PLAN_MARKDOWN_CHARS)
        .collect();
    let tool_call_id = params
        .get("toolCallId")
        .or_else(|| params.get("tool_call_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|s| !s.is_empty());
    (plan_markdown, tool_call_id)
}

/// Serialize the user's plan decision into Grok's `ExitPlanModeExtResponse`.
///
/// Confirmed against Grok 0.2.111 (via Codeg): field is `outcome`, NOT
/// `decision`. Only `"approved"` and `"abandoned"` leave plan mode; everything
/// else (we send `"keep_planning"`) keeps plan mode. Grok discards `feedback`
/// on the keep-planning path — the UI must also send revision notes as a
/// follow-up prompt.
fn build_exit_plan_mode_response(decision: &str, feedback: Option<&str>) -> Value {
    let outcome = match decision.trim().to_ascii_lowercase().as_str() {
        "approve" | "approved" => "approved",
        "abandon" | "abandoned" => "abandoned",
        // request_changes / keep_planning / anything else
        _ => "keep_planning",
    };
    let feedback: String = feedback
        .unwrap_or("")
        .trim()
        .chars()
        .take(MAX_PLAN_FEEDBACK_CHARS)
        .collect();
    json!({
        "outcome": outcome,
        "feedback": feedback,
    })
}

/// Reply when the client disconnects mid-approval: stay in plan mode.
fn exit_plan_mode_disconnect_response() -> Value {
    json!({ "outcome": "keep_planning", "feedback": "" })
}

/// Build the JSON-RPC result for `_x.ai/ask_user_question`.
///
/// Grok deserializes this as an **internally tagged** enum on field `outcome`.
/// Accepted shape (from grok-build-vscode research / binary strings):
/// ```json
/// { "outcome": "accepted", "answers": { "<question>": "<label>" }, "annotations": {} }
/// { "outcome": "cancelled" }
/// ```
/// UI sends `answers` as `[{ "question", "selected": string[] }]`; multi-select
/// labels are joined with `", "`.
fn build_ask_user_question_result(answers: Value, declined: bool) -> Value {
    if declined {
        return json!({ "outcome": "cancelled" });
    }

    let mut map = serde_json::Map::new();
    if let Some(arr) = answers.as_array() {
        for item in arr {
            let question = item
                .get("question")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if question.is_empty() {
                continue;
            }
            let selected: Vec<&str> = item
                .get("selected")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            map.insert(question.to_string(), Value::String(selected.join(", ")));
        }
    }

    json!({
        "outcome": "accepted",
        "answers": Value::Object(map),
        "annotations": {},
    })
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

    /// Effort must survive an agent that advertises no effort config option —
    /// that is exactly Grok, whose only transport is session/set_model `_meta`.
    #[test]
    fn effort_expands_without_a_config_id() {
        let updates = expand_config_updates(&json!({ "effortId": "low" }), None).unwrap();
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].target, ConfigTarget::Effort);
        assert_eq!(updates[0].config_id, None);
        assert_eq!(updates[0].value, json!("low"));
    }

    /// A mode change must stay recognisable as a *mode* after expansion, or the
    /// -32601 retry has no way to pick session/set_mode.
    #[test]
    fn mode_keeps_its_target_through_expansion() {
        let updates = expand_config_updates(&json!({ "mode": "plan" }), None).unwrap();
        assert_eq!(updates[0].target, ConfigTarget::Mode);
        assert_eq!(updates[0].config_id.as_deref(), Some("mode"));

        // Passthrough form must classify too (Composer sends this shape).
        let passthrough =
            expand_config_updates(&json!({ "configId": "mode", "value": "build" }), None).unwrap();
        assert_eq!(passthrough[0].target, ConfigTarget::Mode);
    }

    #[test]
    fn method_not_found_is_distinguished_from_a_rejected_value() {
        assert!(is_method_not_found(
            r#"{"code":-32601,"message":"Method not found"}"#
        ));
        // A rejected *value* must not trigger the legacy retry.
        assert!(!is_method_not_found(
            r#"{"code":-32602,"message":"Unknown config option: effort"}"#
        ));
    }

    /// Grok reports its ceiling only here, so this is the whole reason the
    /// Usage panel can show `used / size` for it at all.
    #[test]
    fn context_size_comes_from_the_current_model_entry() {
        let response = json!({
            "sessionId": "s1",
            "models": {
                "currentModelId": "grok-4.5",
                "availableModels": [
                    { "modelId": "other", "_meta": { "totalContextTokens": 111 } },
                    { "modelId": "grok-4.5", "_meta": { "totalContextTokens": 500000 } }
                ]
            }
        });
        assert_eq!(advertised_context_size(&response), Some(500000));

        // Agents that never advertise a ceiling must yield None, not a guess.
        assert_eq!(advertised_context_size(&json!({ "sessionId": "s1" })), None);
    }

    /// Grok puts model + effort on `models.*`, not configOptions — typed ACP
    /// parse alone must not leave current_model empty.
    #[test]
    fn grok_models_field_fills_current_model_and_effort() {
        let response = json!({
            "sessionId": "s1",
            "models": {
                "currentModelId": "grok-4.5",
                "availableModels": [{
                    "modelId": "grok-4.5",
                    "name": "Grok 4.5",
                    "_meta": {
                        "reasoningEffort": "high",
                        "reasoningEfforts": [
                            { "id": "high", "label": "High Effort" },
                            { "id": "medium", "label": "Medium Effort" },
                            { "id": "low", "label": "Low Effort" }
                        ]
                    }
                }]
            }
        });
        let caps = parse_session_capabilities(&response);
        assert_eq!(caps.current_model.as_deref(), Some("grok-4.5"));
        assert_eq!(caps.current_effort_id.as_deref(), Some("high"));
        assert_eq!(caps.models.len(), 1);
        assert_eq!(caps.effort_options.len(), 3);
        assert!(caps.effort_options.iter().any(|o| o.id == "low"));
    }

    #[test]
    fn exit_plan_mode_response_maps_each_decision() {
        let approve = build_exit_plan_mode_response("approve", None);
        assert_eq!(approve["outcome"], json!("approved"));
        assert_eq!(approve["feedback"], json!(""));
        assert!(approve.get("decision").is_none());

        let changes = build_exit_plan_mode_response("request_changes", Some("  use SSE  "));
        assert_eq!(changes["outcome"], json!("keep_planning"));
        assert_eq!(changes["feedback"], json!("use SSE"));

        let abandon = build_exit_plan_mode_response("abandon", None);
        assert_eq!(abandon["outcome"], json!("abandoned"));

        let disconnect = exit_plan_mode_disconnect_response();
        assert_eq!(disconnect["outcome"], json!("keep_planning"));
    }

    #[test]
    fn exit_plan_mode_parse_accepts_empty_plan() {
        let (plan, tc) = parse_exit_plan_mode_request(&json!({
            "sessionId": "s",
            "toolCallId": "call-1",
            "planContent": "# Plan\n- a"
        }));
        assert_eq!(plan, "# Plan\n- a");
        assert_eq!(tc.as_deref(), Some("call-1"));

        let (plan, tc) = parse_exit_plan_mode_request(&json!({ "sessionId": "s" }));
        assert!(plan.is_empty());
        assert!(tc.is_none());
    }

    /// Grok rejects any result missing the internally-tagged `outcome` field.
    #[test]
    fn ask_user_question_result_has_outcome_tag() {
        let declined = build_ask_user_question_result(json!([]), true);
        assert_eq!(declined, json!({ "outcome": "cancelled" }));

        let accepted = build_ask_user_question_result(
            json!([
                {
                    "question": "Which approach?",
                    "selected": ["Plan first", "Also ship"]
                },
                {
                    "question": "Color?",
                    "selected": ["Blue"]
                }
            ]),
            false,
        );
        assert_eq!(accepted["outcome"], json!("accepted"));
        assert_eq!(
            accepted["answers"],
            json!({
                "Which approach?": "Plan first, Also ship",
                "Color?": "Blue",
            })
        );
        assert_eq!(accepted["annotations"], json!({}));
    }

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

    #[test]
    fn take_in_flight_prompt_id_is_single_shot() {
        let slot = AtomicU64::new(42);
        assert_eq!(take_in_flight_prompt_id(&slot), Some(42));
        assert_eq!(take_in_flight_prompt_id(&slot), None);
        assert_eq!(slot.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn extract_stop_reason_normalizes_variants() {
        assert_eq!(
            extract_stop_reason(Some(&json!({ "stopReason": "end_turn" }))),
            Some("end_turn")
        );
        assert_eq!(
            extract_stop_reason(Some(&json!({ "stop_reason": "cancelled" }))),
            Some("cancelled")
        );
        assert_eq!(
            extract_stop_reason(Some(&json!({ "stopReason": "maxTokens" }))),
            Some("max_tokens")
        );
        assert_eq!(extract_stop_reason(Some(&json!({}))), None);
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

/// `0` means idle. Atomically take the in-flight prompt id if any.
fn take_in_flight_prompt_id(slot: &AtomicU64) -> Option<u64> {
    let id = slot.swap(0, Ordering::SeqCst);
    if id == 0 {
        None
    } else {
        Some(id)
    }
}

/// Pull ACP `stopReason` from a `session/prompt` result object.
fn extract_stop_reason(result: Option<&Value>) -> Option<&'static str> {
    let obj = result?;
    let raw = obj
        .get("stopReason")
        .or_else(|| obj.get("stop_reason"))
        .and_then(Value::as_str)?;
    Some(match raw {
        "end_turn" | "endTurn" => "end_turn",
        "cancelled" | "canceled" => "cancelled",
        "refusal" => "refusal",
        "max_tokens" | "maxTokens" => "max_tokens",
        "max_turn_requests" | "maxTurnRequests" => "max_turn_requests",
        "error" => "error",
        _ => "end_turn",
    })
}

/// Dedicated turn-end event. Frontend must only leave `running` on this
/// (or process death) — never on generic `rpc/response`.
fn emit_turn_complete(
    app: &AppHandle,
    session_id: &str,
    kind: &str,
    stop_reason: &str,
    data: Value,
) {
    let payload = match data {
        Value::Object(mut map) => {
            map.insert("stopReason".into(), json!(stop_reason));
            Value::Object(map)
        }
        other => json!({
            "stopReason": stop_reason,
            "data": other,
        }),
    };
    crate::debug_log::append(
        "acp",
        "info",
        session_id,
        "turn/complete",
        Some(&format!("kind={kind} stopReason={stop_reason}")),
    );
    emit_event(app, session_id, kind, Some("turn/complete"), payload);
}

/// Queue `turn/complete` when the caller has no `AppHandle` (cancel/stop).
/// Relies on the session still having a registered emitter.
fn emit_turn_complete_for_session(
    session_id: &str,
    kind: &str,
    stop_reason: &str,
    data: Value,
) {
    let payload = match data {
        Value::Object(mut map) => {
            map.insert("stopReason".into(), json!(stop_reason));
            Value::Object(map)
        }
        other => json!({
            "stopReason": stop_reason,
            "data": other,
        }),
    };
    crate::debug_log::append(
        "acp",
        "info",
        session_id,
        "turn/complete",
        Some(&format!("kind={kind} stopReason={stop_reason} (no-app)")),
    );
    let event = UiEvent {
        kind: kind.to_string(),
        method: Some("turn/complete".to_string()),
        data: payload,
    };
    // Session gone → drop; stop() already tears the process down.
    let _ = queue_event(session_id, event);
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

/// `Marionette_RAW_LOG=1` mirrors every stdout line the reader yields to
/// `~/.marionette/logs/raw-<session>.log`, unabridged. dev.log clips detail to
/// 4000 chars, which is smaller than a `read` tool result — so it cannot answer
/// "did we receive this line" on its own.
fn raw_stdout_tap(session_id: &str) -> Option<Sender<String>> {
    if std::env::var("Marionette_RAW_LOG").ok().as_deref() != Some("1") {
        return None;
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    let dir = PathBuf::from(home).join(".marionette").join("logs");
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
