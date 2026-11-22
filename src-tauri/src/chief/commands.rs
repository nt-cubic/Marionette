//! Chief 的 UI 命令层（前端「远程助理」面与 tee 回填）。
//!
//! 与 broker 的关系：这些命令是**本地 UI** 的入口（ Ctrl K / 设置面 /
//! 任务区），不经过 MCP 门禁（UI 是 Host，Host 有全部权力）；broker 里
//! 的工具 dispatch 才是 Chief（模型）的门禁入口。

use serde_json::{json, Value};
use tauri::{AppHandle, Manager};

use super::store;
use super::tee;
use crate::AppState;

/// 读 Chief 配置（无凭据）。
#[tauri::command(async)]
pub fn chief_config_load() -> Value {
    let config = store::load_config();
    serde_json::to_value(&config).unwrap_or_else(|_| json!({ "mode": "off" }))
}

/// 保存 Chief 配置。mode / 项目白名单在这里定型；chief_session_id 由
/// broker 引导时维护，UI 传 None 表示保持现状。
#[tauri::command]
pub fn chief_config_save(config: Value) -> Result<Value, String> {
    let mut parsed: super::types::ChiefConfig =
        serde_json::from_value(config).map_err(|e| format!("配置格式错误：{e}"))?;
    // 版本与并发上限的硬顶在 types::ChiefConfig（clamp 用时再收）；
    // 这里只做结构性校验，策略细节交给 policy 层。
    if parsed.max_concurrent_workers == 0 {
        parsed.max_concurrent_workers = 1;
    }
    // 保持既有 chief 会话引用（UI 不改它）。
    if parsed.chief_session_id.is_none() {
        parsed.chief_session_id = store::load_config().chief_session_id;
    }
    store::save_config(&parsed)?;
    Ok(serde_json::to_value(&parsed).unwrap_or(json!({})))
}

/// 全量任务清单（本地 UI 用：跨 chief、不过 MCP 门禁）。
#[tauri::command(async)]
pub fn chief_tasks_list_all() -> Value {
    json!({ "tasks": store::load_tasks() })
}

/// 本地急停（与 Telegram 急停同路）。
#[tauri::command(async)]
pub fn chief_stop_all(app: AppHandle) -> Result<usize, String> {
    super::broker::chief_stop_all(&app)
}

/// tee 回放（前端把离线期间的事件喂进会话流）。
#[tauri::command(async)]
pub fn chief_tee_replay(session_id: String) -> Result<Value, String> {
    let (events, bad) = tee::replay(&session_id)?;
    Ok(json!({ "sessionId": session_id, "events": events, "badLines": bad }))
}

/// tee 消费游标推进（前端成功回填后调用；下次只回放 seq 更大的事件）。
#[tauri::command]
pub fn chief_tee_consume(session_id: String, upto_seq: u64) -> Result<(), String> {
    tee::set_consumed_seq(&session_id, upto_seq)
}

#[tauri::command(async)]
pub fn chief_tee_cursor(session_id: String) -> u64 {
    tee::consumed_seq(&session_id)
}

/// 唤醒心跳线程 + 启动恢复：main.rs setup 调用。
pub fn bootstrap_chief(app: &AppHandle) {
    let _ = super::staff_commands::chief_staff_list(app.clone());
    // 启动恢复（02-architecture.md §10.1）：失联任务标 interrupted，
    // 绝不标 completed；事件账本留痕。
    match store::mark_interrupted_on_startup() {
        Ok(marked) if !marked.is_empty() => {
            for task in &marked {
                crate::debug_log::append(
                    "chief",
                    "info",
                    &task.task_id,
                    "startup recovery: interrupted",
                    Some(&format!("worker={:?}", task.worker_session_id)),
                );
            }
        }
        Ok(_) => {}
        Err(error) => crate::debug_log::append(
            "chief",
            "warn",
            "",
            "startup recovery failed",
            Some(&error),
        ),
    }
    // 账本修剪（AI 派活开放后任务量不再只属于员工——30 天窗口 + 终态
    // 硬顶，防 tasks.json / events.jsonl 无界增长）。
    match store::prune_ledger(crate::delegation::TASK_RETENTION_MS) {
        Ok(dropped) if dropped > 0 => {
            crate::debug_log::append(
                "chief",
                "info",
                "",
                "ledger pruned",
                Some(&format!("dropped {dropped} terminal tasks")),
            );
        }
        _ => {}
    }
    // （第二期「可信」）唤醒收件箱跨重启：从事件账本把未送达的终态回执
    // 重新入队——重启前「没被答的结果」不再蒸发。
    super::broker::rebuild_wake_inbox_from_ledger();
    // 心跳（幂等）：唤醒 flush / 用户队列 / 超时 / 失联检测。
    super::broker::ensure_heartbeat(app.clone());
    let _ = &app.state::<AppState>(); // 确保 state 就绪（setup 时序自检）
}
