//! ACP 进程驱动层：Chief 与 worker 的 start / prompt / config / 收尸。
//!
//! 全部函数需要 `AppHandle`（拿 `AppState` 里的 `AcpService` 与 storage）；
//! 纯决策逻辑不在这层（见 broker.rs / policy.rs）。红线（02-architecture.md
//! §6）：这是**后端驱动路径**，绝不借用前端 `performSend` —— 主窗最小化 /
//! WebView 挂起时这里必须照常工作。

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use crate::acp::AcpService;
use crate::AppState;

/// 解析一个 Agent 的启动命令。优先用户配置（custom_agents 合并表，与
/// UI 同源），缺失时回落内置 harness 表。
pub fn resolve_launch(agent_id: &str) -> Result<(String, Vec<String>), String> {
    for agent in crate::custom_agents::all_agents_merged() {
        if agent.id == agent_id && agent.enabled {
            if agent.command.trim().is_empty() {
                return Err(format!("agent `{agent_id}` has no launch command"));
            }
            return Ok((agent.command.clone(), agent.args.clone()));
        }
    }
    let meta = crate::agent_registry::harness_meta(agent_id);
    if meta.cmd.is_empty() {
        return Err(format!("unknown agent `{agent_id}`"));
    }
    Ok((meta.cmd.to_string(), meta.args.iter().map(|s| s.to_string()).collect()))
}

fn acp_of(app: &AppHandle) -> AcpService {
    app.state::<AppState>().acp.clone()
}

/// 确保一个 ACP 会话活着。幂等：`AcpService::start` 对活会话直接返回
/// 现有 caps；死进程会先 stop 再重启（挂起重连同一条路）。
pub fn ensure_session(
    app: &AppHandle,
    session_id: &str,
    agent_id: &str,
    cwd: &str,
) -> Result<Value, String> {
    let (command, args) = resolve_launch(agent_id)?;
    let acp = acp_of(app);
    let app_for_start = app.clone();
    let sid = session_id.to_string();
    let aid = agent_id.to_string();
    let cwd = cwd.to_string();
    // start 是阻塞握手（spawn + initialize），放独立线程，调用方等结果。
    // 调用方本身就在 broker 的后台线程上，同步等待是预期行为。
    let result = std::thread::Builder::new()
        .name(format!("chief-start-{session_id}"))
        .spawn(move || acp.start(app_for_start, sid, command, args, cwd, Some(aid)))
        .map_err(|e| format!("chief: spawn start task failed: {e}"))?
        .join()
        .map_err(|_| "chief: start task panicked".to_string())?;
    match result {
        Ok(caps) => Ok(json!({
            "models": caps.models.len(),
            "modes": caps.modes.len(),
            "currentModel": caps.current_model,
            "currentMode": caps.current_mode,
        })),
        Err(error) => Err(error),
    }
}

/// 应用模型偏好（Agent 自报的 live 菜单 + 跨用桥都走 session/set_config_option；
/// 失败只记日志不致命——模型不生效但任务照跑）。
pub fn apply_model(app: &AppHandle, session_id: &str, model: &str) {
    let acp = acp_of(app);
    match acp.update_session(session_id, json!({ "model": model })) {
        Ok(_) => crate::debug_log::append(
            "chief",
            "info",
            session_id,
            "model applied",
            Some(model),
        ),
        Err(error) => crate::debug_log::append(
            "chief",
            "warn",
            session_id,
            "model apply failed; agent default stays",
            Some(&format!("model={model} error={error}")),
        ),
    }
}

fn find_read_only_option(options: &[crate::acp::ModeDef]) -> Option<String> {
    options.iter().find_map(|option| {
        let haystack = format!("{} {}", option.id, option.label).to_ascii_lowercase();
        (haystack.contains("read-only")
            || haystack.contains("readonly")
            || haystack.contains("read only")
            || haystack.contains("只读")
            || option.id.eq_ignore_ascii_case("plan"))
        .then_some(option.id.clone())
    })
}

/// Enforce a read-only project ceiling through the Agent's advertised
/// permission select or read-only/plan mode. If the Agent exposes no such
/// control, fail closed rather than pretending a prompt is a sandbox.
pub fn enforce_read_only(app: &AppHandle, session_id: &str) -> Result<(), String> {
    let acp = acp_of(app);
    let caps = acp
        .get_capabilities(session_id)
        .ok_or_else(|| "Agent capabilities unavailable for read-only enforcement".to_string())?;
    if let Some(option) = find_read_only_option(&caps.permission_options) {
        if caps.current_permission.as_deref() == Some(option.as_str()) {
            return Ok(());
        }
        acp.update_session(session_id, json!({ "permission": option }))?;
        return Ok(());
    }
    if let Some(option) = find_read_only_option(&caps.modes) {
        if caps.current_mode.as_deref() == Some(option.as_str()) {
            return Ok(());
        }
        acp.update_session(session_id, json!({ "mode": option }))?;
        return Ok(());
    }
    Err("Agent does not advertise a read-only permission or mode".to_string())
}

/// 发一条 prompt（发射后即返回 accepted；等待回合用 [`wait_turn_idle`]）。
pub fn send_prompt(
    app: &AppHandle,
    session_id: &str,
    text: &str,
) -> Result<Value, String> {
    acp_of(app).send_prompt(session_id, text.to_string(), Vec::new(), Vec::new())
}

/// 阻塞等待回合空闲；`slice` 轮询一次任务状态（返回 false = 调用方要求停）。
/// 返回值：`Ok(true)` = 空闲了；`Ok(false)` = deadline 到或调用方要求停。
pub fn wait_turn_idle_checked(
    app: &AppHandle,
    session_id: &str,
    deadline_ms: u64,
    mut check: impl FnMut() -> bool,
) -> bool {
    let acp = acp_of(app);
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(deadline_ms);
    while std::time::Instant::now() < deadline {
        if acp.turn_idle(session_id) {
            return true;
        }
        if !check() {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(2000));
    }
    acp.turn_idle(session_id)
}

/// 会话是否真的还有进程（区别于 turn_idle 的"缺席视为空闲"）。
pub fn session_alive(app: &AppHandle, session_id: &str) -> bool {
    app.state::<AppState>().acp.agent_session_id(session_id).is_some()
}

/// 当前是否无进行中的回合（进程缺席视为空闲）。
pub fn turn_idle_now(app: &AppHandle, session_id: &str) -> bool {
    app.state::<AppState>().acp.turn_idle(session_id)
}

/// 取消当前回合（幂等；不发起新回合）。
pub fn cancel_turn(app: &AppHandle, session_id: &str) {
    let _ = acp_of(app).cancel(session_id);
}

/// 停止整个会话进程（任务收尸 / 急停路径；幂等）。
pub fn stop_session(app: &AppHandle, session_id: &str) {
    let _ = acp_of(app).stop(session_id);
}

/// 会话还活着但失联（进程没了）时的状态判定辅助：running 任务在心跳里
/// 用它转 interrupted。
pub fn process_gone(app: &AppHandle, session_id: &str) -> bool {
    !session_alive(app, session_id)
}
