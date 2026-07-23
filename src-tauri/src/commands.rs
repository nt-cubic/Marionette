use crate::models::{AgentCommandStatus, AgentConfig, Project, Session};
use crate::AppState;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;
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
pub fn list_agents() -> Vec<AgentConfig> {
    AgentConfig::defaults()
}

#[tauri::command]
pub fn test_agent_command(agent_id: String) -> Result<AgentCommandStatus, String> {
    let agent = AgentConfig::defaults()
        .into_iter()
        .find(|agent| agent.id == agent_id)
        .ok_or_else(|| format!("Unknown agent: {agent_id}"))?;

    match resolve_command(&agent.command) {
        Ok(Some(path)) => Ok(AgentCommandStatus {
            id: agent.id,
            status: "installed".to_string(),
            path: Some(path),
            message: format!("{} is available", agent.command),
        }),
        Ok(None) => Ok(AgentCommandStatus {
            id: agent.id,
            status: "missing".to_string(),
            path: None,
            message: format!("{} was not found on PATH", agent.command),
        }),
        Err(error) => Ok(AgentCommandStatus {
            id: agent.id,
            status: "failed".to_string(),
            path: None,
            message: format!("Command lookup failed: {error}"),
        }),
    }
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
    storage.list_sessions(&project_id)
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

#[tauri::command]
pub fn start_acp_session(
    app: AppHandle,
    session_id: String,
    command: String,
    args: Vec<String>,
    cwd: String,
    state: State<'_, AppState>,
) -> Result<crate::acp::CapabilitySnapshot, String> {
    let caps = state
        .acp
        .start(app, session_id.clone(), command, args, cwd)?;
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    storage.update_session_status(&session_id, "running")?;
    Ok(caps)
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
    state: State<'_, AppState>,
) -> Result<(), String> {
    state
        .pty
        .start(app, session_id.clone(), cwd, state.sessions.clone())?;
    let storage = state
        .storage
        .lock()
        .map_err(|_| "Storage lock poisoned".to_string())?;
    storage.update_session_status(&session_id, "running")
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

fn resolve_command(command: &str) -> Result<Option<String>, String> {
    crate::process_util::resolve_command_display_path(command)
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
