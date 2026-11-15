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
use tauri::{AppHandle, State};

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

    let accepted = acp.send_prompt(&sid, prompt.to_string(), Vec::new(), Vec::new());
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
