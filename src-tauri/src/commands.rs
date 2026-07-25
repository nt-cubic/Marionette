use crate::models::{AgentCommandStatus, AgentConfig, Project, Session};
use crate::AppState;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, State};

#[tauri::command]
pub fn list_projects(state: State<'_, AppState>) -> Result<Vec<Project>, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    storage.list_projects()
}

#[tauri::command]
pub fn add_project(path: String, state: State<'_, AppState>) -> Result<Project, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    storage.add_project(path)
}

#[tauri::command]
pub fn delete_project(project_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    storage.delete_project(&project_id)
}

#[tauri::command]
pub fn list_agents() -> Vec<AgentConfig> {
    AgentConfig::defaults()
}

#[tauri::command]
pub fn test_agent_command(agent_id: String) -> Result<AgentCommandStatus, String> {
    let agent = AgentConfig::defaults()
        .into_iter()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| format!("Unknown agent: {agent_id}"))?;
    Ok(agent_command_status(&agent))
}

/// Status for every agent in one round-trip (the agent menu asks on open).
#[tauri::command]
pub fn list_agent_commands() -> Vec<AgentCommandStatus> {
    AgentConfig::defaults()
        .iter()
        .map(agent_command_status)
        .collect()
}

/// Is the agent's own command on PATH, and are the CLIs it drives present too?
fn agent_command_status(agent: &AgentConfig) -> AgentCommandStatus {
    let bridge = resolve_command(&agent.command);
    let mut missing: Vec<String> = Vec::new();
    let mut installable = false;

    let (status, path, mut message) = match bridge {
        Ok(Some(path)) => (
            "installed".to_string(),
            Some(path),
            format!("{} is available", agent.command),
        ),
        Ok(None) => {
            missing.push(format!("{} (`{}`)", agent.label, agent.command));
            installable = agent.install.package.is_some();
            (
                "missing".to_string(),
                None,
                format!("{} was not found on PATH", agent.command),
            )
        }
        Err(error) => (
            "failed".to_string(),
            None,
            format!("Command lookup failed: {error}"),
        ),
    };

    // A present bridge with an absent CLI still cannot answer a prompt.
    for dependency in &agent.install.requires {
        if matches!(resolve_command(&dependency.command), Ok(None)) {
            missing.push(format!("{} (`{}`)", dependency.label, dependency.command));
            installable = installable || dependency.package.is_some();
        }
    }

    if status == "installed" && !missing.is_empty() {
        message = format!("{} needs: {}", agent.label, missing.join(", "));
    }
    if let Some(note) = &agent.install.note {
        if !missing.is_empty() {
            message = format!("{message}. {note}");
        }
    }

    AgentCommandStatus {
        id: agent.id.clone(),
        // "installed" only when the whole chain can actually run.
        status: if status == "installed" && !missing.is_empty() {
            "incomplete".to_string()
        } else {
            status
        },
        path,
        message,
        installable,
        missing,
    }
}

/// Install the agent's ACP command (and optionally the CLIs it drives) with npm.
///
/// Packages come from the built-in table only — never from the UI — so this
/// cannot be talked into installing something the app does not ship support for.
#[tauri::command]
pub async fn install_agent(
    agent_id: String,
    include_dependencies: bool,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || install_agent_blocking(&agent_id, include_dependencies))
        .await
        .map_err(|error| format!("Install task failed: {error}"))?
}

fn install_agent_blocking(
    agent_id: &str,
    include_dependencies: bool,
) -> Result<serde_json::Value, String> {
    let agent = AgentConfig::defaults()
        .into_iter()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| format!("Unknown agent: {agent_id}"))?;

    if agent.install.manager != "npm" {
        return Err(agent
            .install
            .note
            .clone()
            .unwrap_or_else(|| format!("{} has no automatic installer", agent.label)));
    }

    let mut packages: Vec<String> = Vec::new();
    if let Some(package) = &agent.install.package {
        if matches!(resolve_command(&agent.command), Ok(None)) {
            packages.push(package.clone());
        }
    }
    if include_dependencies {
        for dependency in &agent.install.requires {
            if let Some(package) = &dependency.package {
                if matches!(resolve_command(&dependency.command), Ok(None)) {
                    packages.push(package.clone());
                }
            }
        }
    }

    if packages.is_empty() {
        return Ok(serde_json::json!({
            "agentId": agent.id,
            "installed": [],
            "message": format!("{} is already installed", agent.label),
            "status": agent_command_status(&agent),
        }));
    }

    let mut installed: Vec<String> = Vec::new();
    for package in &packages {
        npm_install_global(&agent.id, package)?;
        installed.push(package.clone());
    }

    let status = agent_command_status(&agent);
    Ok(serde_json::json!({
        "agentId": agent.id,
        "installed": installed,
        "message": format!("Installed {}", installed.join(", ")),
        "status": status,
    }))
}

fn npm_install_global(agent_id: &str, package: &str) -> Result<(), String> {
    use std::process::Command;

    let resolved = crate::process_util::resolve_spawn_command("npm")
        .map_err(|error| format!("`npm` not found — install Node.js first ({error})"))?;
    crate::debug_log::append(
        "install",
        "info",
        agent_id,
        &format!("npm install -g {package}"),
        Some(&resolved.resolved_path),
    );

    let mut command = Command::new(&resolved.program);
    resolved.apply_to(&mut command);
    command.args(["install", "-g", package]);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    let output = command
        .output()
        .map_err(|error| format!("Run npm install -g {package} failed: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    crate::debug_log::append(
        "install",
        if output.status.success() { "info" } else { "error" },
        agent_id,
        &format!("npm install -g {package} → {}", output.status),
        Some(&format!("{stdout}\n{stderr}")),
    );
    if output.status.success() {
        return Ok(());
    }
    // npm's last stderr lines carry the actual reason (EACCES, 404, proxy…).
    let reason = stderr
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("npm install failed");
    Err(format!("npm install -g {package} failed: {reason}"))
}

#[tauri::command]
pub fn list_sessions(
    project_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<Session>, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    // Only this process can own a live agent — anything else on disk is stale.
    storage.list_sessions_healed(&project_id, |session_id| {
        state.acp.is_live(session_id) || state.sessions.is_live(session_id).unwrap_or(false)
    })
}

#[tauri::command]
pub fn create_session(
    project_id: String,
    agent_id: String,
    label: String,
    state: State<'_, AppState>,
) -> Result<Session, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    storage.create_session(&project_id, agent_id, label)
}

#[tauri::command]
pub fn update_session_agent(
    session_id: String,
    agent_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    storage.update_session_agent(&session_id, &agent_id)
}

/// Persist per-dialog Composer prefs (model / mode / effort).
#[tauri::command]
pub fn update_session_prefs(
    session_id: String,
    preferred_model: Option<String>,
    preferred_mode: Option<String>,
    preferred_effort: Option<f64>,
    preferred_effort_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    storage.update_session_prefs(
        &session_id,
        preferred_model,
        preferred_mode,
        preferred_effort,
        preferred_effort_id,
    )
}

#[tauri::command]
pub fn delete_session(
    project_id: String,
    session_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    storage.delete_session(&project_id, &session_id)
}

#[tauri::command]
pub fn update_session_label(
    session_id: String,
    label: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    storage.update_session_label(&session_id, &label)
}

#[tauri::command]
pub fn write_transcript(
    session_id: String,
    events: Vec<serde_json::Value>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    storage.write_transcript(&session_id, &events)
}

#[tauri::command]
pub fn load_transcript(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    storage.load_transcript(&session_id)
}

#[tauri::command]
pub fn search_sessions(
    query: String,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    storage.search_sessions(&query)
}

/// Best-effort local auth probe for agents that expose a CLI status command.
#[tauri::command]
pub fn probe_agent_auth(agent_id: String) -> Result<serde_json::Value, String> {
    match agent_id.as_str() {
        "claude-code" | "claude" => probe_claude_auth(),
        _ => Ok(serde_json::json!({
            "agentId": agent_id,
            "status": "unknown",
            "message": "No local auth probe for this agent"
        })),
    }
}

fn probe_claude_auth() -> Result<serde_json::Value, String> {
    use std::process::Command;
    let resolved = crate::process_util::resolve_spawn_command("claude")
        .map_err(|error| format!("`claude` not found on PATH: {error}"))?;
    let mut command = Command::new(&resolved.program);
    resolved.apply_to(&mut command);
    command.args(["auth", "status"]);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .map_err(|error| format!("Run `claude auth status` failed: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    let text = if !stdout.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    // Prefer JSON if the CLI prints it.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim()) {
        let logged_in = value
            .get("loggedIn")
            .and_then(|v| v.as_bool())
            .or_else(|| value.get("logged_in").and_then(|v| v.as_bool()))
            .unwrap_or(false);
        return Ok(serde_json::json!({
            "agentId": "claude-code",
            "status": if logged_in { "logged_in" } else { "logged_out" },
            "loggedIn": logged_in,
            "raw": value,
            "message": if logged_in {
                "Claude is logged in"
            } else {
                "Claude is not logged in"
            }
        }));
    }
    let lower = text.to_ascii_lowercase();
    let logged_in = lower.contains("\"loggedin\": true")
        || lower.contains("logged in")
        || (lower.contains("loggedin") && lower.contains("true"));
    let logged_out = lower.contains("\"loggedin\": false")
        || lower.contains("not logged")
        || lower.contains("loggedin\":false");
    Ok(serde_json::json!({
        "agentId": "claude-code",
        "status": if logged_in && !logged_out {
            "logged_in"
        } else if logged_out || lower.contains("false") {
            "logged_out"
        } else {
            "unknown"
        },
        "loggedIn": logged_in && !logged_out,
        "message": text.chars().take(240).collect::<String>(),
    }))
}

/// Kick off agent login (opens browser / CLI flow). Non-blocking spawn.
#[tauri::command]
pub fn start_agent_login(agent_id: String) -> Result<serde_json::Value, String> {
    match agent_id.as_str() {
        "claude-code" | "claude" => start_claude_login(),
        _ => Err(format!("No in-app login flow for agent `{agent_id}`")),
    }
}

fn start_claude_login() -> Result<serde_json::Value, String> {
    use std::process::{Command, Stdio};
    let resolved = crate::process_util::resolve_spawn_command("claude")
        .map_err(|error| format!("`claude` not found on PATH: {error}"))?;

    // Prefer a visible console so the user can complete any CLI prompts if the
    // browser handoff needs confirmation. On Windows this uses CREATE_NEW_CONSOLE.
    let mut command = Command::new(&resolved.program);
    resolved.apply_to(&mut command);
    command
        .args(["auth", "login"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        // New console is more reliable for interactive OAuth CLIs than NO_WINDOW.
        command.creation_flags(CREATE_NEW_CONSOLE | DETACHED_PROCESS);
    }

    command
        .spawn()
        .map_err(|error| format!("Start `claude auth login` failed: {error}"))?;

    crate::debug_log::append(
        "auth",
        "info",
        "",
        "started claude auth login",
        Some("browser/CLI login flow"),
    );

    Ok(serde_json::json!({
        "agentId": "claude-code",
        "started": true,
        "message": "Opened Claude login — complete sign-in in the browser/terminal window, then return here."
    }))
}

#[tauri::command]
pub fn read_terminal_snapshot(session_id: String, cwd: String) -> Result<String, String> {
    if !Path::new(&cwd).is_dir() {
        return Err(format!("Terminal cwd is not a directory: {cwd}"));
    }
    let safe_id = session_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let path = Path::new(&cwd)
        .join(".agentshell")
        .join("sessions")
        .join(format!("{safe_id}.raw.log"));
    if !path.exists() {
        return Ok(String::new());
    }
    let bytes =
        fs::read(path).map_err(|error| format!("Read terminal snapshot failed: {error}"))?;
    let start = bytes.len().saturating_sub(1024 * 1024);
    Ok(String::from_utf8_lossy(&bytes[start..]).into_owned())
}

/// Async so the handshake runs on a blocking pool and does not stall the webview
/// (IME / typing) while OpenCode/Claude spawn + initialize.
#[tauri::command]
pub async fn start_acp_session(
    app: AppHandle,
    session_id: String,
    command: String,
    args: Vec<String>,
    cwd: String,
    state: State<'_, AppState>,
) -> Result<crate::acp::CapabilitySnapshot, String> {
    crate::debug_log::append(
        "acp",
        "info",
        &session_id,
        &format!("start_acp_session {command}"),
        Some(&cwd),
    );

    // session.agentId is the source of truth for which harness owns this dialog,
    // and it decides what project context gets lent to it.
    let agent_id = {
        let storage = state
            .storage
            .lock()
            .map_err(|_| "Storage lock poisoned".to_string())?;
        storage
            .find_session(&session_id)
            .ok()
            .flatten()
            .map(|session| session.agent_id)
    };

    let acp = state.acp.clone();
    let app_for_start = app.clone();
    let sid = session_id.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        acp.start(app_for_start, sid, command, args, cwd, agent_id)
    })
    .await
    .map_err(|error| format!("ACP start task failed: {error}"))?;

    match result {
        Ok(caps) => {
            {
                let storage = state
                    .storage
                    .lock()
                    .map_err(|_| "Storage lock poisoned".to_string())?;
                storage.update_session_status(&session_id, "running")?;
            }
            crate::debug_log::append(
                "acp",
                "info",
                &session_id,
                &format!(
                    "ready models={} modes={}",
                    caps.models.len(),
                    caps.modes.len()
                ),
                caps.current_model.as_deref(),
            );
            Ok(caps)
        }
        Err(error) => {
            crate::debug_log::append("acp", "error", &session_id, "start failed", Some(&error));
            Err(error)
        }
    }
}

#[tauri::command]
pub fn send_acp_prompt(
    session_id: String,
    text: String,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    state.acp.send_prompt(&session_id, text)
}

#[tauri::command]
pub fn cancel_acp_session(session_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.acp.cancel(&session_id)
}

#[tauri::command]
pub fn get_session_capabilities(
    session_id: String,
    state: State<'_, AppState>,
) -> Result<Option<crate::acp::CapabilitySnapshot>, String> {
    Ok(state.acp.get_capabilities(&session_id))
}

#[tauri::command]
pub fn update_acp_session(
    session_id: String,
    config: serde_json::Value,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    state.acp.update_session(&session_id, config)
}

#[tauri::command]
pub fn stop_acp_session(session_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.acp.stop(&session_id)?;
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    storage.update_session_status(&session_id, "exited")
}

#[tauri::command]
pub fn start_terminal(
    app: AppHandle,
    session_id: String,
    cwd: String,
    command: String,
    args: Vec<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    crate::debug_log::append(
        "pty",
        "info",
        &session_id,
        &format!("start {command} {}", args.join(" ")),
        Some(&cwd),
    );
    match state.pty.start(
        app,
        session_id.clone(),
        cwd,
        command,
        args,
        state.sessions.clone(),
    ) {
        Ok(()) => {
            let storage = state
                .storage
                .lock()
                .map_err(|_| "Storage lock poisoned".to_string())?;
            storage.update_session_status(&session_id, "running")
        }
        Err(error) => {
            crate::debug_log::append("pty", "error", &session_id, "start failed", Some(&error));
            Err(error)
        }
    }
}

#[tauri::command]
pub fn write_terminal(
    session_id: String,
    data: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.pty.write(&session_id, &data)
}

#[tauri::command]
pub fn resize_terminal(
    session_id: String,
    cols: u16,
    rows: u16,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.pty.resize(&session_id, cols, rows)
}

#[tauri::command]
pub fn stop_terminal(session_id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.pty.stop(&session_id)?;
    state.sessions.mark_exited(&session_id)?;
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    storage.update_session_status(&session_id, "exited")
}

#[tauri::command]
pub fn append_debug_log(
    source: String,
    level: String,
    session_id: String,
    summary: String,
    detail: Option<String>,
) {
    crate::debug_log::append(
        &source,
        &level,
        &session_id,
        &summary,
        detail.as_deref(),
    );
}

#[tauri::command]
pub fn debug_log_path() -> String {
    crate::debug_log::log_path_display()
}

/// Probe billing/balance for the active OpenCode-style model id (`provider/model`).
/// Uses keys from OpenCode `auth.json`; never returns secrets.
#[tauri::command]
pub fn probe_provider_usage(
    model_id: Option<String>,
) -> crate::provider_usage::ProviderUsageSnapshot {
    crate::provider_usage::probe_provider_usage(model_id)
}

/// Generate `.agentshell/handoff.md` and a composer prefill prompt. Does not send.
/// Pass `source_agent_id` when the session may already be rebound to the target.
#[tauri::command]
pub fn generate_handoff(
    project_id: String,
    session_id: String,
    target_agent_id: String,
    source_agent_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<crate::models::HandoffResult, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    let project = storage
        .list_projects()?
        .into_iter()
        .find(|p| p.id == project_id)
        .ok_or_else(|| format!("Unknown project: {project_id}"))?;
    let session = storage
        .find_session(&session_id)?
        .ok_or_else(|| format!("Unknown session: {session_id}"))?;

    let agents = crate::models::AgentConfig::defaults();
    let agent_label = |id: &str| {
        agents
            .iter()
            .find(|a| a.id == id)
            .map(|a| a.label.clone())
            .unwrap_or_else(|| id.to_string())
    };

    let source_id = source_agent_id
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| session.agent_id.clone());
    let source_label = agent_label(&source_id);
    let target_label = agent_label(&target_agent_id);

    crate::handoff::generate_handoff(
        &project.id,
        Path::new(&project.root_path),
        &project.name,
        &session.id,
        &session.label,
        &source_id,
        &source_label,
        &target_agent_id,
        &target_label,
        Path::new(&session.transcript_path),
    )
}

#[tauri::command]
pub fn get_changed_files(
    project_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<crate::models::ChangedFile>, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    let project = storage
        .list_projects()?
        .into_iter()
        .find(|p| p.id == project_id)
        .ok_or_else(|| format!("Unknown project: {project_id}"))?;
    crate::git_service::get_changed_files(Path::new(&project.root_path))
}

#[tauri::command]
pub fn get_file_diff(
    project_id: String,
    path: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    let project = storage
        .list_projects()?
        .into_iter()
        .find(|p| p.id == project_id)
        .ok_or_else(|| format!("Unknown project: {project_id}"))?;
    crate::git_service::get_file_diff(Path::new(&project.root_path), &path)
}

/// Answer a pending ACP `session/request_permission` prompt.
#[tauri::command]
pub fn respond_acp_permission(
    request_id: String,
    option_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    state.acp.respond_permission(&request_id, &option_id)
}

fn resolve_command(command: &str) -> Result<Option<String>, String> {
    crate::process_util::resolve_command_display_path(command)
}

// ─── Project context: MCP servers + skills lent to agents that lack them ────

fn project_root_of(project_id: &str, state: &State<'_, AppState>) -> Result<PathBuf, String> {
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    let project = storage
        .list_projects()?
        .into_iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| format!("Unknown project: {project_id}"))?;
    Ok(PathBuf::from(project.root_path))
}

/// Scan this machine + project for MCP servers and skills, with what the user
/// has already decided to lend. Cheap enough to call whenever the panel opens.
#[tauri::command]
pub fn scan_project_context(
    project_id: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let root = project_root_of(&project_id, &state)?;
    let inventory = crate::context_inventory::scan(&root);
    let selection = crate::context_inventory::load_selection(&root);
    Ok(serde_json::json!({
        "projectId": project_id,
        "inventory": inventory,
        "selection": selection,
    }))
}

#[tauri::command]
pub fn set_project_context_enabled(
    project_id: String,
    kind: String,
    id: String,
    enabled: bool,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let root = project_root_of(&project_id, &state)?;
    let selection = crate::context_inventory::set_enabled(&root, &kind, &id, enabled)?;
    crate::debug_log::append(
        "context",
        "info",
        "",
        &format!("{kind} {id} → {}", if enabled { "lend" } else { "off" }),
        Some(&root.display().to_string()),
    );
    Ok(serde_json::to_value(selection).unwrap_or_default())
}

/// Which paths in a draft message point outside the project and are not granted.
#[tauri::command]
pub fn check_outside_project_paths(
    project_id: String,
    paths: Vec<String>,
    state: State<'_, AppState>,
) -> Result<Vec<serde_json::Value>, String> {
    let root = project_root_of(&project_id, &state)?;
    Ok(crate::context_inventory::outside_project_paths(&root, &paths))
}

/// Grant a folder outside the project to this project's agents.
///
/// Returns `restartNeeded` when a live ACP session already exists: the scope is
/// fixed at `session/new`, so it only applies from the next connection.
#[tauri::command]
pub fn grant_workspace_root(
    project_id: String,
    dir: String,
    session_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let root = project_root_of(&project_id, &state)?;
    let roots = crate::context_inventory::grant_workspace_root(&root, &dir)?;
    let restart_needed = session_id
        .as_deref()
        .map(|id| state.acp.is_live(id))
        .unwrap_or(false);
    crate::debug_log::append(
        "context",
        "info",
        session_id.as_deref().unwrap_or(""),
        &format!("granted workspace root: {dir}"),
        Some(if restart_needed {
            "live session will be restarted to apply it"
        } else {
            "applies on next connect"
        }),
    );
    Ok(serde_json::json!({ "workspaceRoots": roots, "restartNeeded": restart_needed }))
}

#[tauri::command]
pub fn revoke_workspace_root(
    project_id: String,
    dir: String,
    state: State<'_, AppState>,
) -> Result<Vec<String>, String> {
    let root = project_root_of(&project_id, &state)?;
    crate::context_inventory::revoke_workspace_root(&root, &dir)
}

/// Skills preamble for an agent that has no skill system of its own.
/// Returns null when the agent already ships everything that is enabled.
#[tauri::command]
pub fn project_context_prompt(
    project_id: String,
    agent_id: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let root = project_root_of(&project_id, &state)?;
    Ok(crate::context_inventory::skills_prompt_for_agent(&root, &agent_id))
}

#[cfg(test)]
mod tests {
    use super::resolve_command;
    use crate::process_util::resolve_spawn_command;

    #[test]
    fn detects_a_command_available_on_path() {
        let command = if cfg!(target_os = "windows") {
            "where.exe"
        } else {
            "sh"
        };
        let path = resolve_command(command).unwrap();
        assert!(path.is_some(), "expected {command} to be available on PATH");
    }

    #[test]
    fn resolves_opencode_when_installed() {
        // Soft check: only assert structure when opencode is present on the machine
        if let Ok(resolved) = resolve_spawn_command("opencode") {
            assert!(
                resolved.program.to_ascii_lowercase().ends_with(".exe")
                    || resolved.program.eq_ignore_ascii_case("cmd.exe")
                    || resolved.program.eq_ignore_ascii_case("powershell.exe"),
                "unexpected program: {}",
                resolved.program
            );
            assert!(
                !resolved.resolved_path.is_empty(),
                "resolved path should not be empty"
            );
        }
    }
}
