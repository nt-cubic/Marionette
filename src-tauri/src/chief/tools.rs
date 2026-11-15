//! Chief 控制 MCP 工具：schema 与 dispatch（M1 恰好 10 个）。
//!
//! 注入方式（02-architecture.md §5.1）：继续走 `tools_mcp.rs` 的
//! localhost HTTP MCP——`tools/list` 按会话身份返回（Chief 只看到这 10 个，
//! 普通会话永远看不到它们）；dispatch 入口再做一次身份校验（双保险：
//! 猜到工具名的普通 worker 一样被拒）。
//!
//! 纪律：所有拒绝都是可读返回值（`{ok:false, error, note}` envelope），
//! 不让 Chief 因一次拒绝丢整轮；schema 里**永远没有** cwd / permissionMode /
//! mcpServers / providerApiKey / env / shellCommand / release / addProjectRoot
//! （05-mvp-plan.md §6.2——能力靠 Host 配置，不靠调用参数扩展）。

use serde_json::{json, Value};
use tauri::AppHandle;

use super::broker;

pub const PROJECTS_LIST: &str = "marionette_projects_list";
pub const AGENTS_LIST: &str = "marionette_agents_list";
pub const MODELS_LIST: &str = "marionette_models_list";
pub const TASKS_LIST: &str = "marionette_tasks_list";
pub const TASK_START: &str = "marionette_task_start";
pub const TASK_SEND: &str = "marionette_task_send";
pub const TASK_STATUS: &str = "marionette_task_status";
pub const TASK_CANCEL: &str = "marionette_task_cancel";
pub const TASK_COLLECT: &str = "marionette_task_collect";
pub const TASK_RESUME: &str = "marionette_task_resume";

const ALL: &[&str] = &[
    PROJECTS_LIST,
    AGENTS_LIST,
    MODELS_LIST,
    TASKS_LIST,
    TASK_START,
    TASK_SEND,
    TASK_STATUS,
    TASK_CANCEL,
    TASK_COLLECT,
    TASK_RESUME,
];

/// 工具名是否属于 Chief 控制面（普通会话的 dispatch 用它 fail-closed）。
pub fn is_chief_tool(name: &str) -> bool {
    ALL.contains(&name)
}

fn tool(name: &str, description: &str, required: &[&str], properties: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": { "type": "object", "required": required, "properties": properties },
    })
}

pub fn chief_tool_schemas() -> Value {
    json!([
        tool(
            PROJECTS_LIST,
            "List the projects the Host has allowed Chief to work with (id, remote mode, allowed agents, per-project concurrency/time limits). Unlisted projects are NOT accessible — do not guess paths.",
            &[],
            json!({}),
        ),
        tool(
            AGENTS_LIST,
            "List installed/enabled ACP agents. With projectId, marks which agents that project allows. Unavailable agents are listed with enabled=false.",
            &[],
            json!({ "projectId": { "type": "string" } }),
        ),
        tool(
            MODELS_LIST,
            "List models the cross-use engine can route for a given agent (context window / max output included). Agents' native models follow their own defaults — omit `model` on task_start to use the default.",
            &["agentId"],
            json!({ "agentId": { "type": "string" } }),
        ),
        tool(
            TASKS_LIST,
            "List YOUR tasks (newest first) with status and last summary. Optional status filter (proposed/starting/running/waiting_user/completed/failed/cancelled/interrupted).",
            &[],
            json!({
                "status": { "type": "string" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 50 },
            }),
        ),
        tool(
            TASK_START,
            "Start a task: the Host creates an ordinary worker session in the allowed project and runs your goal there. Returns the taskId IMMEDIATELY (does not wait for completion) — poll marionette_task_status or wait for a HOST EVENT. Repeating the same requestId returns the same task (idempotent).",
            &["requestId", "title", "goal", "projectId"],
            json!({
                "requestId": { "type": "string", "description": "Idempotency key you generate (e.g. random uuid). Same key = same task." },
                "title": { "type": "string", "maxLength": 120 },
                "goal": { "type": "string", "maxLength": 20000, "description": "The full natural-language goal for the worker. Be complete — the worker sees THIS text, not the chat." },
                "projectId": { "type": "string", "description": "From marionette_projects_list." },
                "agentId": { "type": "string", "description": "Optional; defaults to the project's configured default agent." },
                "model": { "type": "string", "description": "Optional; defaults to the project's configured default model." },
                "maxRuntimeMinutes": { "type": "integer", "minimum": 1, "maximum": 480 },
            }),
        ),
        tool(
            TASK_SEND,
            "Send a follow-up instruction to a running task's worker. If the worker is mid-turn this fails with worker_busy — retry later or use urgency=interrupt.",
            &["taskId", "message"],
            json!({
                "taskId": { "type": "string" },
                "commandId": { "type": "string", "description": "Idempotency key for this send; repeating it is a no-op." },
                "message": { "type": "string", "maxLength": 10000 },
                "urgency": { "type": "string", "enum": ["normal", "interrupt"] },
            }),
        ),
        tool(
            TASK_STATUS,
            "Get one task's status, last summary, error and whether it is waiting for the user.",
            &["taskId"],
            json!({ "taskId": { "type": "string" } }),
        ),
        tool(
            TASK_CANCEL,
            "Cancel a task (stops the worker's current turn). Idempotent: cancelling an already-cancelled task succeeds.",
            &["taskId"],
            json!({
                "taskId": { "type": "string" },
                "reason": { "type": "string" },
            }),
        ),
        tool(
            TASK_COLLECT,
            "Collect a task's result: summary text, tool-call counts, touched files. Before completion this is an interim snapshot — never presented as final.",
            &["taskId"],
            json!({
                "taskId": { "type": "string" },
                "includeDiff": { "type": "boolean" },
                "includeTestOutput": { "type": "boolean" },
                "artifactLimit": { "type": "integer", "minimum": 0, "maximum": 20 },
            }),
        ),
        tool(
            TASK_RESUME,
            "Resume an interrupted task (e.g. after an app restart): restarts the worker session and re-issues the goal. Completed/failed tasks cannot be resumed.",
            &["taskId"],
            json!({
                "taskId": { "type": "string" },
                "note": { "type": "string", "description": "Optional user note appended to the resume prompt." },
            }),
        ),
    ])
}

/// dispatch 入口的身份与工具名校验（不依赖 AppHandle，可单测）。
/// Ok(()) = 是 Chief 会话且工具名合法。
pub(crate) fn check_access(session_id: &str, name: &str) -> Result<(), String> {
    if !broker::is_chief_session(session_id) {
        return Err(format!(
            "tool `{name}` is a Chief control tool and is only available to the Chief session"
        ));
    }
    if !is_chief_tool(name) {
        return Err(format!("unknown chief tool: {name}"));
    }
    Ok(())
}

/// Chief 控制工具的 dispatch。拒绝一路是 envelope 返回值；Err 只留给
/// host 侧意外（MCP 层会把 Err 变成 isError=true 的工具结果）。
pub fn dispatch(app: &AppHandle, session_id: &str, name: &str, args: &Value) -> Result<Value, String> {
    check_access(session_id, name)?;
    match name {
        PROJECTS_LIST => Ok(broker::chief_projects_list(session_id)),
        AGENTS_LIST => Ok(broker::chief_agents_list(
            session_id,
            args.get("projectId").and_then(Value::as_str),
        )),
        MODELS_LIST => {
            let agent_id = args.get("agentId").and_then(Value::as_str).unwrap_or("");
            if agent_id.is_empty() {
                return Ok(json!({ "ok": false, "error": "bad_request", "note": "agentId 必填。" }));
            }
            Ok(broker::chief_models_list(agent_id))
        }
        TASKS_LIST => Ok(broker::chief_tasks_list(
            session_id,
            args.get("status").and_then(Value::as_str),
            args.get("limit").and_then(Value::as_u64).unwrap_or(20) as usize,
        )),
        TASK_START => Ok(broker::chief_task_start(app, session_id, args)),
        TASK_SEND => Ok(broker::chief_task_send(app, session_id, args)),
        TASK_STATUS => {
            let task_id = args.get("taskId").and_then(Value::as_str).unwrap_or("");
            Ok(broker::chief_task_status(session_id, task_id))
        }
        TASK_CANCEL => Ok(broker::chief_task_cancel(app, session_id, args)),
        TASK_COLLECT => Ok(broker::chief_task_collect(session_id, args)),
        TASK_RESUME => Ok(broker::chief_task_resume(app, session_id, args)),
        _ => Err(format!("unknown chief tool: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_cover_exactly_the_documented_ten() {
        let schemas = chief_tool_schemas();
        let list = schemas.as_array().expect("array");
        assert_eq!(list.len(), 10);
        for name in ALL {
            let found = list.iter().any(|t| t.get("name").and_then(Value::as_str) == Some(*name));
            assert!(found, "missing schema for {name}");
        }
        // 关键 schema 红线：task_start 必填四个字段，且**没有**任何提权字段。
        let start = list
            .iter()
            .find(|t| t.get("name").and_then(Value::as_str) == Some(TASK_START))
            .unwrap();
        let required = start.pointer("/inputSchema/required").unwrap().as_array().unwrap();
        assert_eq!(required.len(), 4);
        let props = start.pointer("/inputSchema/properties").unwrap().as_object().unwrap();
        for forbidden in ["cwd", "permissionMode", "mcpServers", "providerApiKey", "env", "shellCommand", "addProjectRoot", "alwaysApprove"] {
            assert!(!props.contains_key(forbidden), "task_start must not offer `{forbidden}`");
        }
    }

    #[test]
    fn unknown_names_fail_closed() {
        assert!(is_chief_tool(TASK_START));
        assert!(!is_chief_tool("browser_click"));
        assert!(!is_chief_tool("marionette_task_approve"), "Chief 永远不该有 approve 工具");
        assert!(!is_chief_tool("shell_exec"));
    }
}
