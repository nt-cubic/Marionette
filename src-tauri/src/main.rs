#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod acp;
mod agent_registry;
mod agent_update;
mod app_paths;
mod elicitation;
mod app_update;
mod commands;
mod context_inventory;
mod custom_agents;
mod debug_log;
mod git_service;
mod handoff;
mod http_client;
mod models;
mod open_target;
mod preflight;
mod process_util;
mod provider_usage;
mod session_manager;
mod storage;
mod terminal_runtime;
mod webview2_gate;

use acp::AcpService;
use session_manager::SessionManager;
use std::sync::Mutex;
use storage::StorageService;

pub struct AppState {
    pub storage: Mutex<StorageService>,
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
///
/// Callers must `api.prevent_close()` first so default WebView teardown never
/// starts while we are still on the event-handler stack.
fn shutdown_and_exit(app: &tauri::AppHandle) -> ! {
    use tauri::Manager;

    // Kill agents best-effort, then leave immediately. Do not touch the window
    // (hide/destroy/emit) — any Win32 re-entry here can hit the tao paint assert.
    if let Some(state) = app.try_state::<AppState>() {
        let acp = state.acp.stop_all();
        debug_log::append(
            "shutdown",
            "info",
            "",
            &format!("stopped {acp} acp"),
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
    // Before any WebView host is created — native dialog if runtime missing.
    webview2_gate::ensure_or_exit();

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
            use tauri::{Emitter, Manager};
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    // NEVER let WebView2 tear down through the normal close path
                    // on Windows — tao 0.35.3 asserts in flush_paint_messages
                    // (tao#1180 / tauri#14088) and the process dies with 101.
                    //
                    // Main / last window → stop agents and process::exit.
                    // Detached dialog windows → hide only (reuse on next pop-out).
                    // Destroy is deferred until process exit, which skips orderly
                    // WebView teardown and avoids the paint assert.
                    api.prevent_close();
                    let label = window.label().to_string();
                    let is_main = label == "main";
                    // Count windows that are still visible — hidden detached
                    // shells should not keep the app "alive" alone.
                    let visible = window
                        .app_handle()
                        .webview_windows()
                        .values()
                        .filter(|w| w.is_visible().unwrap_or(true))
                        .count();
                    if is_main || visible <= 1 {
                        shutdown_and_exit(window.app_handle());
                    } else {
                        let _ = window.hide();
                        // Frontend reaper blanks excess hidden shells (memory).
                        let _ = window.app_handle().emit("marionette-detached-hidden", &label);
                        crate::debug_log::append(
                            "window",
                            "info",
                            "",
                            &format!("hid detached window `{label}` (no WebView destroy)"),
                            Some("avoids tao#1180 paint assert on secondary close"),
                        );
                    }
                }
                // Safety net if something still destroys a window without
                // CloseRequested. Never try to paint/teardown here — exit.
                tauri::WindowEvent::Destroyed => {
                    let app = window.app_handle();
                    if window.label() == "main" {
                        std::process::exit(0);
                    }
                    let any_visible = app
                        .webview_windows()
                        .values()
                        .any(|w| w.is_visible().unwrap_or(false));
                    if !any_visible {
                        std::process::exit(0);
                    }
                }
                _ => {}
            }
        })
        .manage(AppState {
            storage: Mutex::new(
                StorageService::new().expect("failed to initialize Marionette storage"),
            ),
            acp: AcpService::new(),
            sessions: SessionManager::new(),
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_projects,
            commands::list_providers,
            commands::navigate_webview,
            commands::add_project,
            commands::pick_folder,
            commands::pick_files,
            commands::delete_project,
            commands::reorder_projects,
            commands::list_agents,
            commands::list_custom_agents,
            commands::add_custom_agent,
            commands::remove_custom_agent,
            commands::test_agent_command,
            commands::list_agent_commands,
            commands::agent_preflight,
            commands::list_agent_preflights,
            commands::install_agent,
            commands::agent_versions,
            commands::list_sessions,
            commands::create_session,
            commands::create_child_session,
            commands::list_child_sessions,
            commands::update_session_agent,
            commands::update_session_prefs,
            commands::update_session_label,
            commands::update_session_status,
            commands::delete_session,
            commands::write_transcript,
            commands::load_transcript,
            commands::search_sessions,
            commands::probe_agent_auth,
            commands::start_agent_login,
            commands::start_acp_session,
            commands::send_acp_prompt,
            commands::read_image_data_url,
            commands::save_pasted_image,
            commands::cancel_acp_session,
            commands::stop_acp_session,
            commands::get_session_capabilities,
            commands::update_acp_session,
            commands::append_debug_log,
            commands::debug_log_path,
            commands::probe_provider_usage,
            commands::probe_acp_billing,
            commands::generate_handoff,
            commands::get_changed_files,
            commands::get_file_diff,
            commands::respond_acp_permission,
            commands::respond_acp_question,
            commands::respond_acp_plan_approval,
            commands::check_app_update,
            commands::download_app_update,
            commands::apply_app_update_and_relaunch,
            commands::scan_project_context,
            commands::set_project_context_enabled,
            commands::project_context_prompt,
            commands::list_todos,
            commands::save_todos,
            commands::check_outside_project_paths,
            commands::grant_workspace_root,
            commands::revoke_workspace_root,
            commands::save_provider_key,
            commands::delete_provider_key,
            commands::upsert_provider_meta,
            commands::delete_provider_meta,
            open_target::resolve_link_target,
            open_target::open_external,
            open_target::reveal_in_file_manager
        ])
        .run(tauri::generate_context!())
        .expect("error while running Marionette");
}
