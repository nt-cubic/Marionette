#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod acp;
mod agent_update;
mod commands;
mod context_inventory;
mod debug_log;
mod git_service;
mod handoff;
mod models;
mod open_target;
mod process_util;
mod provider_usage;
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

/// Shut down our own state, then leave — deliberately without unwinding.
///
/// Closing the window makes tao tear down the WebView2 host, which pumps a
/// paint while a redraw is already in flight and trips
/// `assert!(flush_paint_messages(..))` in tao 0.35.3
/// (`platform_impl/windows/event_loop.rs:2344`, tao#1180 / tauri#14088).
/// A panic hook cannot save us there: tao stashes the payload and re-raises it
/// with `panic::resume_unwind` on the message loop, so the process dies with
/// 101 no matter what the hook prints — and our agent processes get orphaned
/// with it. So we do the cleanup ourselves and exit before that path runs.
fn shutdown_and_exit(app: &tauri::AppHandle) -> ! {
    use tauri::Manager;

    if let Some(state) = app.try_state::<AppState>() {
        let acp = state.acp.stop_all();
        let pty = state.pty.stop_all();
        debug_log::append(
            "shutdown",
            "info",
            "",
            &format!("stopped {acp} acp · {pty} pty"),
            Some("exiting before tao window teardown (tao#1180)"),
        );
    }
    std::process::exit(0);
}

/// Notice when the window stops pumping messages, and say what it was doing.
///
/// A frozen Tauri window reports `Responding: False` and nothing else — no
/// stack, no culprit, and a `--release` build has nothing useful to attach to.
/// This pings the main thread on an interval; a ping that does not come back
/// means the UI is frozen right now, so we write that fact plus the set of
/// commands still executing. The stall is timestamped in the same dev.log as
/// the ACP traffic, which is what makes the two correlatable after the fact.
fn spawn_main_thread_watchdog(app: tauri::AppHandle) {
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};

    const PING_EVERY: Duration = Duration::from_secs(2);
    const STALL_AFTER: Duration = Duration::from_secs(3);

    std::thread::spawn(move || loop {
        std::thread::sleep(PING_EVERY);

        let (tx, rx) = channel::<()>();
        // Queued behind whatever the main thread is doing — that is the point.
        if app
            .run_on_main_thread(move || {
                let _ = tx.send(());
            })
            .is_err()
        {
            return; // app is shutting down
        }

        let started = Instant::now();
        if rx.recv_timeout(STALL_AFTER).is_ok() {
            continue;
        }

        debug_log::append(
            "watchdog",
            "error",
            "",
            "MAIN THREAD STALLED — window is frozen",
            Some(&format!(
                "in-flight: {} | acp: {}",
                debug_log::inflight_summary(),
                acp::pipeline_summary()
            )),
        );

        // Keep holding the ping so we can report the real duration rather than
        // just "at least 3s", and so one stall logs twice, not every 2s.
        let recovered = rx.recv_timeout(Duration::from_secs(600)).is_ok();
        debug_log::append(
            "watchdog",
            if recovered { "warn" } else { "error" },
            "",
            &format!(
                "main thread {} after {}s",
                if recovered { "recovered" } else { "STILL frozen" },
                started.elapsed().as_secs()
            ),
            Some(&format!("in-flight: {}", debug_log::inflight_summary())),
        );
    });
}

fn main() {
    // A panic anywhere else should still leave a trace in the dev diary.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Non-blocking: the panicking thread may already hold the log lock.
        debug_log::append_from_panic_hook(&info.to_string());
        previous_hook(info);
    }));

    tauri::Builder::default()
        .setup(|app| {
            spawn_main_thread_watchdog(app.handle().clone());
            acp::spawn_pipeline_watchdog();
            // Startup is the only safe moment: no session exists yet, so an
            // upgrade cannot replace a binary an agent is mid-turn on.
            commands::auto_update_agents_in_background();
            Ok(())
        })
        .on_window_event(|window, event| {
            use tauri::Manager;
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                shutdown_and_exit(window.app_handle());
            }
        })
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
            commands::list_providers,
            commands::add_project,
            commands::delete_project,
            commands::list_agents,
            commands::test_agent_command,
            commands::list_agent_commands,
            commands::install_agent,
            commands::agent_versions,
            commands::list_sessions,
            commands::create_session,
            commands::update_session_agent,
            commands::update_session_prefs,
            commands::update_session_label,
            commands::delete_session,
            commands::write_transcript,
            commands::load_transcript,
            commands::search_sessions,
            commands::probe_agent_auth,
            commands::start_agent_login,
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
            commands::stop_terminal,
            commands::append_debug_log,
            commands::debug_log_path,
            commands::probe_provider_usage,
            commands::generate_handoff,
            commands::get_changed_files,
            commands::get_file_diff,
            commands::respond_acp_permission,
            commands::scan_project_context,
            commands::set_project_context_enabled,
            commands::project_context_prompt,
            commands::check_outside_project_paths,
            commands::grant_workspace_root,
            commands::revoke_workspace_root,
            commands::save_provider_key,
            open_target::resolve_link_target,
            open_target::open_external,
            open_target::reveal_in_file_manager
        ])
        .run(tauri::generate_context!())
        .expect("error while running AgentShell");
}
