//! 后端 ACP 驱动探针（M0 可行性闸门，docs/research-telegram-chief/05-mvp-plan.md §2）。
//!
//! 从 Rust 直接驱动一次完整 ACP 回合：start → prompt → wait_turn_idle →
//! stop，全程不经过前端 `performSend` 管线。事件经 [`crate::chief::tee`]
//! 落盘，回放读回即 M0 的"重启可恢复"证据。
//!
//! 这些命令是**诊断面不是产品面**：等 ChiefBroker（broker/runner/policy）
//! 落地后，探针会被真正的驱动路径替换，命令保留用于真机回归。

use serde_json::{json, Value};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, State};

use crate::agent_registry;
use crate::AppState;

/// 探针等待回合完成的默认上限。真实任务可以远超这个值；探针只证明
/// "能完整跑完"，长 soak 用更小的 prompt 手工验证。
const DEFAULT_TURN_TIMEOUT_SECS: u64 = 900;
/// 回合空闲后再给事件 tee 一点收尾时间，然后才读回。
const TEE_DRAIN_QUIET_MS: u64 = 600;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 从 Rust 驱动一个完整 ACP 回合（无前端参与）。
///
/// 返回：caps 摘要、prompt 是否被接受、回合是否在超时内空闲、tee 路径
/// 与回放事件统计。探针会话不进 storage（不是产品会话），跑完即 stop。
#[tauri::command(async)]
pub async fn chief_probe_drive(
    app: AppHandle,
    state: State<'_, AppState>,
    agent_id: String,
    cwd: String,
    prompt: String,
    timeout_secs: Option<u64>,
) -> Result<Value, String> {
    let acp = state.acp.clone();
    tauri::async_runtime::spawn_blocking(move || {
        drive_sync(app, &acp, &agent_id, &cwd, &prompt, timeout_secs)
    })
    .await
    .map_err(|e| format!("chief probe task failed: {e}"))?
}

/// 手动中止一个探针会话（卡死时的逃生口，与产品停止键同路 `AcpService::stop`）。
#[tauri::command(async)]
pub fn chief_probe_stop(session_id: String, state: State<'_, AppState>) -> Result<Value, String> {
    let result = state.acp.stop(&session_id);
    crate::chief::tee::unregister(&session_id);
    result?;
    Ok(json!({ "stopped": session_id }))
}

/// 读回探针会话的 tee 事件（重启后回放验证）。
#[tauri::command(async)]
pub fn chief_probe_replay(session_id: String) -> Result<Value, String> {
    let (events, bad) = crate::chief::tee::replay(&session_id)?;
    let mut kinds: std::collections::BTreeMap<String, u64> = Default::default();
    for event in &events {
        let kind = event
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string();
        *kinds.entry(kind).or_insert(0) += 1;
    }
    Ok(json!({
        "sessionId": session_id,
        "events": events.len(),
        "badLines": bad,
        "kinds": kinds,
        "last": events.last().cloned(),
    }))
}

/// M0 真机探针入口（不依赖 devtools）：
///
/// `MARIONETTE_CHIEF_PROBE="agentId|cwd|prompt"` → 启动 12s 后在后台线程
/// 跑一轮完整后端驱动探针（start → prompt → wait_turn_idle → stop），
/// 报告整份写 dev.log（`[chief] env probe report`）。可选
/// `MARIONETTE_CHIEF_PROBE_MINIMIZE=1`：先最小化主窗再驱动——主窗最小化
/// 会触发 WebView2 TrySuspend（main.rs MINIMIZE_SUSPENDED），这正是
/// 「WebView 挂起时后端照常工作」的 M0 闸门条件。
///
/// env 未设 = 零行为。探针超时固定 300s（探针只证明"能完整跑完"，
/// 长 soak 手工用窗口做）。
pub fn maybe_run_env_probe(app: AppHandle) {
    let Ok(spec) = std::env::var("MARIONETTE_CHIEF_PROBE") else {
        return;
    };
    let spec = spec.trim().to_string();
    if spec.is_empty() {
        return;
    }
    let parts: Vec<&str> = spec.splitn(3, '|').collect();
    if parts.len() != 3 {
        crate::debug_log::append(
            "chief",
            "error",
            "",
            "env probe: bad MARIONETTE_CHIEF_PROBE spec (want agentId|cwd|prompt)",
            Some(&spec),
        );
        return;
    }
    let agent_id = parts[0].to_string();
    let cwd = parts[1].to_string();
    let prompt = parts[2].to_string();
    let minimize = std::env::var("MARIONETTE_CHIEF_PROBE_MINIMIZE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    let spawned = std::thread::Builder::new()
        .name("chief-env-probe".to_string())
        .spawn(move || {
            // 等 UI / 托盘 / 引擎初始化稳定，再动主窗。
            std::thread::sleep(Duration::from_secs(12));
            if minimize {
                if let Some(window) = app.get_webview_window("main") {
                    let _ = window.minimize();
                    crate::debug_log::append(
                        "chief",
                        "info",
                        "",
                        "env probe: main window minimized (webview suspend expected)",
                        None,
                    );
                    std::thread::sleep(Duration::from_secs(3));
                }
            }
            crate::debug_log::append(
                "chief",
                "info",
                "",
                "env probe: begin",
                Some(&format!("agent={agent_id} cwd={cwd} minimize={minimize}")),
            );
            let state = app.state::<crate::AppState>();
            let report = drive_sync(app.clone(), &state.acp, &agent_id, &cwd, &prompt, Some(300));
            match report {
                Ok(value) => crate::debug_log::append(
                    "chief",
                    "info",
                    "",
                    "env probe report",
                    Some(&serde_json::to_string(&value).unwrap_or_default()),
                ),
                Err(error) => crate::debug_log::append(
                    "chief",
                    "error",
                    "",
                    "env probe failed",
                    Some(&error),
                ),
            }
        });
    if spawned.is_err() {
        crate::debug_log::append("chief", "error", "", "env probe thread spawn failed", None);
    }
}

/// M1 管线端到端真机入口（Telegram 入站腿之外的整条链路）：
///
/// - `MARIONETTE_CHIEF_E2E_TASK="projectId|goal"`：12s 后直接走
///   `chief_task_start`（确定性覆盖：worker spawn → tee → 任务卡/权限卡
///   状态机 → 完成 → 唤醒收件箱 → 心跳 flush → Chief 回复出站）。
/// - `MARIONETTE_CHIEF_E2E_INBOUND="<text>"`：12s 后走
///   `chief_inbound_text`（覆盖：Chief 会话引导 + 模型经 MCP 真调
///   `marionette_projects_list` / `marionette_task_start` 的决策腿；
///   模型不派工则如实记录，不算缺陷——chief 可以自己答琐碎问题）。
///
/// 前者要求 projectId 已在允许列表；后者要求 `mode=chief`。报告写
/// dev.log（`[chief] e2e report`）。env 未设 = 零行为。
pub fn maybe_run_env_e2e(app: AppHandle) {
    let task_spec = std::env::var("MARIONETTE_CHIEF_E2E_TASK").ok();
    let inbound_text = std::env::var("MARIONETTE_CHIEF_E2E_INBOUND").ok();
    let (task_spec, inbound_text) = match (task_spec.filter(|s| !s.trim().is_empty()), inbound_text.filter(|s| !s.trim().is_empty())) {
        (None, None) => return,
        pair => pair,
    };
    let spawned = std::thread::Builder::new()
        .name("chief-env-e2e".to_string())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(12));
            if let Some(spec) = task_spec {
                let parts: Vec<&str> = spec.splitn(2, '|').collect();
                if parts.len() != 2 {
                    crate::debug_log::append("chief", "error", "", "e2e: bad E2E_TASK spec (want projectId|goal)", Some(&spec));
                    return;
                }
                let (project_id, goal) = (parts[0].to_string(), parts[1].to_string());
                let chief_sid = match super::broker::chief_session_ensure(&app) {
                    Ok(sid) => sid,
                    Err(error) => {
                        crate::debug_log::append("chief", "error", "", "e2e: chief session bootstrap failed", Some(&error));
                        return;
                    }
                };
                let args = json!({
                    "requestId": format!("e2e_{}", now_ms()),
                    "title": "E2E 管线验证",
                    "goal": goal,
                    "projectId": project_id,
                });
                let outcome = super::broker::chief_task_start(&app, &chief_sid, &args);
                crate::debug_log::append(
                    "chief",
                    "info",
                    "",
                    "e2e report (task_start)",
                    Some(&serde_json::to_string(&outcome).unwrap_or_default()),
                );
            }
            if let Some(text) = inbound_text {
                let outcome = super::broker::chief_inbound_text(&app, &text);
                let line = match outcome {
                    Ok(super::broker::InboundOutcome::Sent) => "inbound sent to chief".to_string(),
                    Ok(super::broker::InboundOutcome::Queued) => "inbound queued (chief busy)".to_string(),
                    Err(error) => format!("inbound failed: {error}"),
                };
                crate::debug_log::append("chief", "info", "", "e2e report (inbound)", Some(&line));
            }
        });
    if spawned.is_err() {
        crate::debug_log::append("chief", "error", "", "e2e thread spawn failed", None);
    }
}

fn drive_sync(
    app: AppHandle,
    acp: &crate::acp::AcpService,
    agent_id: &str,
    cwd: &str,
    prompt: &str,
    timeout_secs: Option<u64>,
) -> Result<Value, String> {
    let meta = agent_registry::harness_meta(agent_id);
    let sid = format!("session_chiefprobe_{}", now_ms());
    let tee_path = crate::chief::tee::register(&sid)?;
    let started = Instant::now();
    crate::debug_log::append(
        "chief",
        "info",
        &sid,
        &format!("probe drive begin agent={agent_id} cmd={}", meta.cmd),
        Some(cwd),
    );

    let args: Vec<String> = meta.args.iter().map(|s| s.to_string()).collect();
    let start_result = acp.start(
        app,
        sid.clone(),
        meta.cmd.to_string(),
        args,
        cwd.to_string(),
        Some(agent_id.to_string()),
    );
    let caps = match start_result {
        Ok(caps) => caps,
        Err(error) => {
            let _ = acp.stop(&sid);
            crate::chief::tee::unregister(&sid);
            return Err(format!("ACP start failed: {error}"));
        }
    };

    crate::desktop_notify::mark_silent_turn(&sid);
    let accepted = acp.send_prompt(&sid, prompt.to_string(), Vec::new(), Vec::new());
    if accepted.is_err() {
        crate::desktop_notify::clear_turn_marks(&sid);
    }
    let mut prompt_accepted = false;
    let mut turn_idle = false;
    let mut prompt_error = None;
    match accepted {
        Ok(_) => {
            prompt_accepted = true;
            let timeout = Duration::from_secs(timeout_secs.unwrap_or(DEFAULT_TURN_TIMEOUT_SECS).min(3600));
            turn_idle = acp.wait_turn_idle(&sid, timeout);
            if !turn_idle {
                // 超时先取消当前回合再收尸，避免进程挂着等下一次交互。
                let _ = acp.cancel(&sid);
                let _ = acp.wait_turn_idle(&sid, Duration::from_secs(10));
            }
        }
        Err(error) => {
            prompt_error = Some(error);
        }
    }

    std::thread::sleep(Duration::from_millis(TEE_DRAIN_QUIET_MS));
    let replay = crate::chief::tee::replay(&sid)
        .map(|(events, bad)| {
            let mut kinds: std::collections::BTreeMap<String, u64> = Default::default();
            for event in &events {
                let kind = event
                    .get("kind")
                    .and_then(Value::as_str)
                    .unwrap_or("?")
                    .to_string();
                *kinds.entry(kind).or_insert(0) += 1;
            }
            json!({
                "path": tee_path.to_string_lossy(),
                "events": events.len(),
                "badLines": bad,
                "kinds": kinds,
            })
        })
        .unwrap_or_else(|e| json!({ "error": e }));

    let stopped = acp.stop(&sid);
    crate::chief::tee::unregister(&sid);
    let elapsed_ms = started.elapsed().as_millis() as u64;

    let report = json!({
        "sessionId": sid,
        "agentId": agent_id,
        "command": format!("{} {}", meta.cmd, meta.args.join(" ")).trim(),
        "cwd": cwd,
        "caps": {
            "models": caps.models.len(),
            "modes": caps.modes.len(),
            "currentModel": caps.current_model,
        },
        "promptAccepted": prompt_accepted,
        "promptError": prompt_error,
        "turnIdle": turn_idle,
        "elapsedMs": elapsed_ms,
        "tee": replay,
        "stopped": stopped.is_ok(),
    });
    crate::debug_log::append(
        "chief",
        "info",
        &sid,
        &format!(
            "probe drive done accepted={prompt_accepted} turnIdle={turn_idle} elapsed={elapsed_ms}ms"
        ),
        Some(&serde_json::to_string(&report).unwrap_or_default()),
    );
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_meta_resolves_known_agents() {
        // 探针的启动参数解析完全走 agent_registry；这里钉住形状，
        // 避免 probe 与 registry 的字段约定漂移。
        let codex = agent_registry::harness_meta("codex");
        assert_eq!(codex.cmd, "codex-acp");
        // codex 适配器命令行无默认参数（args 留空），别假设非空。
        assert!(codex.args.is_empty());
        let deepseek = agent_registry::harness_meta("deepseek");
        assert_eq!(deepseek.cmd, "dsh-acp");
    }

    #[test]
    fn probe_session_ids_pass_tee_validation() {
        // 探针生成的会话 id 必须能过 tee 的 fail-closed 校验。
        let sid = format!("session_chiefprobe_{}", now_ms());
        assert!(crate::chief::tee::validate_session_id(&sid).is_ok());
    }
}
