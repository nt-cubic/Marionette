#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod acp;
mod commands;
mod models;
mod process_util;
mod pty;
mod session_manager;
mod storage;

use acp::AcpService;
use pty::PtyService;
use session_manager::SessionManager;
use std::sync::Mutex;
use storage::StorageService;

pub struct AppState {
    pub storage: Mutex<StorageService>,
    pub pty: PtyService,
    pub acp: AcpService,
    pub sessions: SessionManager,
}

fn main() {
    tauri::Builder::default()
        .manage(AppState {
            storage: Mutex::new(
                StorageService::new().expect("failed to initialize AgentShell storage"),
            ),
            pty: PtyService::new(),
            acp: AcpService::new(),
            sessions: SessionManager::new(),
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_projects,
            commands::add_project,
            commands::list_agents,
            commands::test_agent_command,
            commands::list_sessions,
            commands::create_session,
            commands::delete_session,
            commands::read_terminal_snapshot,
            commands::start_acp_session,
            commands::send_acp_prompt,
            commands::cancel_acp_session,
            commands::stop_acp_session,
            commands::get_session_capabilities,
            commands::update_acp_session,
            commands::start_terminal,
            commands::write_terminal,
            commands::resize_terminal,
            commands::stop_terminal
        ])
        .run(tauri::generate_context!())
        .expect("error while running AgentShell");
}
