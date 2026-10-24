#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod acp;
mod acp_catalog;
mod agent_registry;
mod agent_update;
mod app_paths;
mod elicitation;
mod app_update;
mod commands;
mod context_inventory;
mod credentials;
mod custom_agents;
mod data_dir_gate;
mod debug_log;
mod dock_support;
mod dsh_bridge;
mod dsh_plugins;
mod engine;
mod git_service;
mod grok_dialect;
mod handoff;
mod http_client;
mod memory;
mod models;
mod model_bridge;
mod model_sources;
mod open_target;
mod parsers;
mod preflight;
mod process_util;
mod provider_usage;
mod proxy;
mod shell_integration;
mod storage;
mod telegram_remote;
mod terminal_runtime;
mod webview2_gate;

use acp::AcpService;
use std::sync::Mutex;
use storage::StorageService;

pub struct AppState {
    pub storage: Mutex<StorageService>,
    pub acp: AcpService,
}

/// Longest to wait for agent kills on close before exiting anyway.
const SHUTDOWN_KILL_TIMEOUT_SECS: u64 = 5;

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
pub(crate) fn shutdown_and_exit(app: &tauri::AppHandle) -> ! {
    use tauri::Manager;

    // Kill our embedded translation engine first (fast TerminateProcess; the
    // reused user instance is not ours and stays). magpie's JSON writes are
    // all tmp+rename atomic, so a hard kill is safe (S1).
    crate::engine::kill_engine();

    // Kill agents best-effort, then leave. Do not touch the window
    // (hide/destroy/emit) — any Win32 re-entry here can hit the tao paint assert.
    //
    // stop_all runs on a helper thread with a bounded wait: every kill is a
    // lock-then-block pair, so a hang in that chain must cost the close a few
    // seconds, never freeze the window (an orphaned agent beats a dead UI).
    if let Some(state) = app.try_state::<AppState>() {
        let acp = state.acp.clone();
        let (tx, rx) = std::sync::mpsc::channel::<usize>();
        std::thread::spawn(move || {
            let _ = tx.send(acp.stop_all());
        });
        match rx.recv_timeout(std::time::Duration::from_secs(SHUTDOWN_KILL_TIMEOUT_SECS)) {
            Ok(count) => debug_log::append(
                "shutdown",
                "info",
                "",
                &format!("stopped {count} acp"),
                Some("exiting before tao window teardown (tao#1180)"),
            ),
            Err(_) => debug_log::append(
                "shutdown",
                "warn",
                "",
                &format!(
                    "agent cleanup timed out after {SHUTDOWN_KILL_TIMEOUT_SECS}s — exiting anyway"
                ),
                Some("exiting before tao window teardown (tao#1180)"),
            ),
        }
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

/// Recognizes the tao 0.35.x paint-flush assert (tao#1140 / tauri#14088).
///
/// Matches the message text recorded in the dev diary for the crash:
/// "assertion failed: flush_paint_messages(None, &subclass_input.event_loop_runner)"
/// at `tao-0.35.3/src/platform_impl/windows/event_loop.rs:2344`.
fn is_tao_paint_assert(panic_message: &str) -> bool {
    panic_message.contains("flush_paint_messages")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tao_paint_assert_is_recognized() {
        let real = concat!(
            "panicked at D:\\Dev\\Cargo\\registry\\src\\index.crates.io-1949cf8c6b5b557f\\",
            "tao-0.35.3\\src\\platform_impl\\windows\\event_loop.rs:2344:11: ",
            "assertion failed: flush_paint_messages(None, &subclass_input.event_loop_runner)"
        );
        assert!(is_tao_paint_assert(real));
    }

    #[test]
    fn ordinary_panics_pass_through() {
        assert!(!is_tao_paint_assert(
            "panicked at src/main.rs:99: index out of bounds: the len is 1 but the index is 5"
        ));
        assert!(!is_tao_paint_assert(
            "panicked at src/storage.rs:41: failed to initialize storage"
        ));
        assert!(!is_tao_paint_assert(""));
    }
}

fn main() {
    // Before any WebView host is created — native dialog if runtime missing.
    webview2_gate::ensure_or_exit();

    // The global data dir holds the instance lock, the dev diary and every
    // session. When it is not writable the single-instance check below cannot
    // open the lock, the app concludes another instance owns it and exits 0
    // with no window, no log line and no dialog — indistinguishable from "the
    // exe does nothing". That is what Windows file permissions produce when the
    // process runs with restricted rights (a sandbox token, or an image whose
    // mandatory integrity label is Low): writes to %USERPROFILE%\.marionette are
    // denied while reads still succeed, so only a write probe can see it.
    if let Err(err) = app_paths::ensure_global_dir_writable() {
        data_dir_gate::prompt_and_exit(&err);
    }

    // Explorer "Open Marionette here" / CLI: resolve folder before UI starts.
    let open_path = shell_integration::parse_open_path_from_args();
    if let Some(ref p) = open_path {
        debug_log::append(
            "shell",
            "info",
            "",
            "launch with --open-path",
            Some(p),
        );
    }

    // Single instance: a second start hands the path to the running UI and exits.
    if !shell_integration::acquire_single_instance_or_handoff(open_path.as_deref()) {
        std::process::exit(0);
    }
    shell_integration::set_launch_open_path(open_path);

    // A panic anywhere else should still leave a trace in the dev diary.
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Non-blocking: the panicking thread may already hold the log lock.
        debug_log::append_from_panic_hook(&info.to_string());

        // tao#1140 / tauri#14088: WebView2 redraw races the thread-event-target
        // window's WM_PAINT flush and trips
        // `assert!(flush_paint_messages(..))` in tao 0.35.x
        // (`platform_impl/windows/event_loop.rs`, "assertion failed:
        // flush_paint_messages"). The upstream fix landed in tao 0.36.0
        // (window subclassing removed, #1231) but tauri 2.11.5 still pins
        // tao 0.35, so this can still fire on release builds — where
        // `panic = "abort"` would otherwise turn it into a hard crash
        // (0xc0000409 in the Application event log) instead of an exit.
        //
        // If we are at that assert, the WebView2 host is already tearing
        // down and tao re-raises the payload via `panic::resume_unwind` on
        // the message loop, after which unwinding crosses the `extern
        // "system"` callback boundary and aborts the process regardless of
        // this hook. Exiting here, before that second raise, is the only
        // chance to go out clean: same visible outcome (window closes),
        // no crash dialog, no WER bucket, no "stopped responding" report.
        // Deliberately nothing else in here — no state to flush, no locks
        // to take: the panicking thread may hold them, and any Win32 or
        // process work can re-enter the very paint race that just tripped.
        let message = info.to_string();
        if is_tao_paint_assert(&message) {
            std::process::exit(0);
        }

        previous_hook(info);
    }));

    tauri::Builder::default()
        .setup(|app| {
            use tauri::Manager;
            spawn_main_thread_watchdog(app.handle().clone());
            acp::spawn_pipeline_watchdog();
            // Engine progress/state events are emitted from its background
            // threads — they need an app handle kept for the process lifetime.
            engine::set_app_handle(app.handle().clone());
            // Telegram 远程桥：只读配置 + 起 worker 线程（无 token 时全空转）。
            // try_state：状态缺失时跳过（不 panic——release 是 panic=abort）。
            if let Some(state) = app.try_state::<AppState>() {
                telegram_remote::start(app.handle().clone(), state.acp.clone());
            } else {
                crate::debug_log::append(
                    "telegram",
                    "warn",
                    "",
                    "AppState missing at setup — telegram remote not started",
                    None,
                );
            }
            // Keep Explorer menu pointing at this portable exe (moves / rebuilds).
            if let Err(err) = shell_integration::register_context_menu() {
                debug_log::append(
                    "shell",
                    "warn",
                    "",
                    "register Explorer context menu failed",
                    Some(&err),
                );
            }
            shell_integration::spawn_pending_open_watcher(app.handle().clone());
            // DSH 跨用桥已改成 per-spawn bundle（--bundle），不再写
            // ~/.dsh/cordis.patch.yml。旧版本留下的 llm-pi-ai 托管块必须一次性
            // 清掉：它在 home 层，会整条盖住用户在 dsh Web 里配的 provider，并
            // 让 Web 的保存校验一直报 "overridden by a home patch"。删前备份。
            match dsh_bridge::migrate_legacy_home_patch() {
                Ok(true) => debug_log::append(
                    "bridge",
                    "info",
                    "",
                    "removed legacy llm-pi-ai block from dsh home patch",
                    None,
                ),
                Ok(false) => {}
                Err(err) => debug_log::append(
                    "bridge",
                    "warn",
                    "",
                    "legacy dsh home-patch cleanup failed",
                    Some(&err),
                ),
            }
            // Startup is the only safe moment: no session exists yet, so an
            // upgrade cannot replace a binary an agent is mid-turn on.
            commands::auto_update_agents_in_background();
            // 无脑可用：翻译引擎随应用启动自动拉起。便携版（embedded-engine）
            // 离线落位内嵌 magpie 再 serve；开发构建未装则后台下载。
            // enable 幂等；失败只落 Failed 状态（左下角可见/可重试），不阻塞启动。
            if let Err(error) = engine::enable() {
                debug_log::append(
                    "engine",
                    "warn",
                    "",
                    "engine auto-start at launch failed",
                    Some(&error),
                );
            }
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
                    // Background mode (edge dock, stage 0): close never exits —
                    // the window hides, engine/sessions/TG bridge keep running.
                    // True exit is footer 退出 → quit_app → shutdown_and_exit.
                    let background = dock_support::background_mode_enabled();
                    if (is_main || visible <= 1) && !background {
                        shutdown_and_exit(window.app_handle());
                    } else {
                        if is_main {
                            // Background mode + edge dock (stage 1): hand the
                            // close to the frontend — dockMode retracts the
                            // window to the edge sliver (or hides itself as
                            // fallback when docking is unavailable). No hide
                            // here; the window is never hidden while docking.
                            use tauri::Emitter as _;
                            let _ = window.emit_to(label.as_str(), "marionette-main-close", ());
                            crate::debug_log::append(
                                "window",
                                "info",
                                "",
                                "main close handed to frontend (background mode)",
                                Some("dockMode retracts to edge; fallback hide is its own"),
                            );
                        } else {
                            let _ = window.hide();
                            // Frontend reaper blanks excess hidden shells (memory).
                            // Detached SPA flushes transcript then releases ACP
                            // ownership — do not steal the stream here or main
                            // would rewrite JSONL from an empty in-memory rail.
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
                }
                // Safety net if something still destroys a window without
                // CloseRequested. Never try to paint/teardown here — hand the
                // exit to the shared shutdown path so agents don't orphan
                // (a bare exit(0) here used to skip stop_all entirely).
                tauri::WindowEvent::Destroyed => {
                    let app = window.app_handle();
                    let any_visible = app
                        .webview_windows()
                        .values()
                        .any(|w| w.is_visible().unwrap_or(false));
                    if window.label() == "main" || !any_visible {
                        shutdown_and_exit(&app);
                    }
                }
                _ => {}
            }
        })
        .manage(AppState {
            storage: Mutex::new(StorageService::new().unwrap_or_else(|error| {
                // panic=abort 下 expect 是「无对话框静默退出」；与 data_dir_gate
                // 同款哲学：写 dev.log 再弹原生对话框，用户看得见失败。
                crate::debug_log::append(
                    "bootstrap",
                    "error",
                    "",
                    "failed to initialize Marionette storage",
                    Some(&error),
                );
                crate::data_dir_gate::prompt_and_exit(&format!(
                    "Marionette 无法初始化存储：{error}"
                ));
            })),
            acp: AcpService::new(),
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_projects,
            commands::list_providers,
            commands::navigate_webview,
            commands::add_project,
            commands::take_launch_open_path,
            commands::get_default_folder,
            commands::open_here,
            commands::create_chat_session,
            commands::register_shell_integration,
            commands::unregister_shell_integration,
            commands::shell_integration_registered,
            commands::pick_folder,
            commands::pick_files,
            commands::delete_project,
            commands::reorder_projects,
            commands::list_agents,
            commands::list_custom_agents,
            commands::add_custom_agent,
            commands::remove_custom_agent,
            commands::fetch_acp_registry,
            commands::test_agent_command,
            commands::list_agent_commands,
            commands::agent_preflight,
            commands::list_agent_preflights,
            commands::install_agent,
            commands::agent_versions,
            commands::list_sessions,
            commands::refresh_external_sessions,
            commands::create_session,
            commands::create_child_session,
            commands::list_child_sessions,
            commands::update_session_agent,
            commands::update_session_prefs,
            commands::update_session_label,
            commands::set_session_pinned,
            commands::update_session_status,
            commands::set_detached_session_owner,
            commands::delete_session,
            commands::write_transcript,
            commands::load_transcript,
            commands::probe_agent_auth,
            commands::memory_get,
            commands::memory_set_enabled,
            commands::memory_add,
            commands::memory_update,
            commands::memory_delete,
            commands::memory_set_entry_enabled,
            commands::memory_import_preview,
            commands::memory_import_run,
            commands::memory_distill,
            commands::dsh_plugins_list,
            commands::dsh_plugin_add,
            commands::dsh_plugin_remove,
            commands::start_agent_login,
            commands::start_grok_update,
            commands::start_acp_session,
            commands::send_acp_prompt,
            commands::set_stream_interests,
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
            commands::get_current_branch,
            commands::get_file_diff,
            commands::restore_turn_files,
            commands::begin_workspace_snapshot,
            commands::end_workspace_snapshot,
            commands::discard_workspace_snapshot,
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
            commands::bridge_scan,
            commands::bridge_list_entries,
            commands::bridge_add_key,
            commands::bridge_delete_entry,
            commands::bridge_list_models,
            commands::bridge_add_custom_provider,
            commands::model_library_scan,
            commands::model_library_check_source,
            commands::model_library_set_environment,
            commands::model_library_preferences,
            commands::model_library_save_preferences,
            commands::bridge_bind,
            commands::bridge_get_binding,
            commands::engine_status,
            commands::engine_enable,
            commands::engine_quota,
            commands::engine_stop,
            commands::engine_check_update,
            commands::engine_update,
            commands::engine_restore_factory,
            commands::get_proxy_config,
            commands::set_proxy_config,
            commands::test_proxy,
            commands::telegram_remote_status,
            commands::telegram_remote_save,
            commands::telegram_remote_accept_pairing,
            commands::telegram_remote_dismiss_pairing,
            commands::telegram_remote_reset_pairing,
            commands::telegram_remote_ack,
            commands::telegram_remote_result,
            open_target::resolve_link_target,
            open_target::open_external,
            open_target::reveal_in_file_manager,
            dock_support::set_background_mode,
            dock_support::quit_app,
            dock_support::fullscreen_active
        ])
        .run(tauri::generate_context!())
        .unwrap_or_else(|error| {
            crate::debug_log::append(
                "bootstrap",
                "error",
                "",
                "tauri run failed",
                Some(&error.to_string()),
            );
            crate::data_dir_gate::prompt_and_exit(&format!("Marionette 启动失败：{error}"));
        });
}
