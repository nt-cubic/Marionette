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
pub const MEMORY_RECALL: &str = "marionette_memory_recall";
pub const MEMORY_PROPOSE: &str = "marionette_memory_propose";
pub const COMMITMENT_ADD: &str = "marionette_commitment_add";
pub const COMMITMENT_LIST: &str = "marionette_commitment_list";
pub const COMMITMENT_COMPLETE: &str = "marionette_commitment_complete";
pub const STAFF_STATUS: &str = "marionette_staff_status";

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
    MEMORY_RECALL,
    MEMORY_PROPOSE,
    COMMITMENT_ADD,
    COMMITMENT_LIST,
    COMMITMENT_COMPLETE,
    STAFF_STATUS,
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
        tool(
            MEMORY_RECALL,
            "Read YOUR layered staff memory: pinned facts, current state, recent journals and pending proposals. Current user instructions outrank old memory.",
            &["recentDays"],
            json!({
                "recentDays": { "type": "integer", "minimum": 0, "maximum": 30 },
            }),
        ),
        tool(
            MEMORY_PROPOSE,
            "Propose a durable memory candidate from a real user statement or task fact. This only appends to YOUR proposal area; it never edits pinned/state directly.",
            &["kind", "text"],
            json!({
                "kind": { "type": "string", "enum": ["preference", "decision", "commitment", "lesson"] },
                "text": { "type": "string", "maxLength": 16000 },
                "taskId": { "type": "string" },
                "eventSeq": { "type": "integer", "minimum": 1 },
            }),
        ),
        tool(
            COMMITMENT_ADD,
            "Record a future follow-up, reminder, promise or condition for yourself. Task completion does not automatically complete a commitment.",
            &["text", "kind"],
            json!({
                "text": { "type": "string", "maxLength": 4000 },
                "kind": { "type": "string", "enum": ["follow_up", "reminder", "promise", "condition"] },
                "dueAt": { "type": "integer", "description": "Unix milliseconds; 0/omitted = no timed deadline." },
                "condition": { "type": "string" },
                "sourceRef": { "type": "string" },
            }),
        ),
        tool(
            COMMITMENT_LIST,
            "List YOUR open/due commitments and recent terminal ones.",
            &[],
            json!({ "includeTerminal": { "type": "boolean" } }),
        ),
        tool(
            COMMITMENT_COMPLETE,
            "Mark one of YOUR commitments done or cancelled.",
            &["commitmentId"],
            json!({
                "commitmentId": { "type": "string" },
                "cancelled": { "type": "boolean" },
            }),
        ),
        tool(
            STAFF_STATUS,
            "Get YOUR derived status, task counts, waiting reason, paused/budget policy and due commitments. Status is a projection of Host facts, not a model-authored state.",
            &[],
            json!({}),
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
        MEMORY_RECALL => {
            let Some(profile) = super::staff_commands::profile_for_session(session_id) else {
                return Ok(json!({ "ok": false, "error": "staff_missing", "note": "当前会话没有员工档案。" }));
            };
            let days = args.get("recentDays").and_then(Value::as_u64).unwrap_or(7) as usize;
            match super::memory::recall(&profile.staff_id, days) {
                Ok(snapshot) => Ok(json!({ "ok": true, "memory": snapshot })),
                Err(error) => Ok(json!({ "ok": false, "error": "memory_read_failed", "note": error })),
            }
        }
        MEMORY_PROPOSE => {
            let Some(profile) = super::staff_commands::profile_for_session(session_id) else {
                return Ok(json!({ "ok": false, "error": "staff_missing", "note": "当前会话没有员工档案。" }));
            };
            let kind = args.get("kind").and_then(Value::as_str).unwrap_or("lesson");
            let text = args.get("text").and_then(Value::as_str).unwrap_or("");
            let mut refs = Vec::new();
            if let Some(task_id) = args.get("taskId").and_then(Value::as_str) {
                refs.push(super::memory::MemorySourceRef::task(task_id));
            }
            if let Some(seq) = args.get("eventSeq").and_then(Value::as_u64) {
                if let Some(source) = refs.first_mut() {
                    source.event_seq = Some(seq);
                }
            }
            match super::memory::append_proposal(&profile.staff_id, kind, text, refs) {
                Ok(proposal) => Ok(json!({ "ok": true, "proposal": proposal })),
                Err(error) => Ok(json!({ "ok": false, "error": "memory_propose_failed", "note": error })),
            }
        }
        COMMITMENT_ADD => {
            let Some(profile) = super::staff_commands::profile_for_session(session_id) else {
                return Ok(json!({ "ok": false, "error": "staff_missing", "note": "当前会话没有员工档案。" }));
            };
            let text = args.get("text").and_then(Value::as_str).unwrap_or("").trim();
            if text.is_empty() {
                return Ok(json!({ "ok": false, "error": "bad_request", "note": "text 必填。" }));
            }
            let kind = match args.get("kind").and_then(Value::as_str).unwrap_or("follow_up") {
                "reminder" => super::commitments::CommitmentKind::Reminder,
                "promise" => super::commitments::CommitmentKind::Promise,
                "condition" => super::commitments::CommitmentKind::Condition,
                _ => super::commitments::CommitmentKind::FollowUp,
            };
            let row = super::commitments::Commitment {
                staff_id: profile.staff_id,
                text: text.to_string(),
                kind,
                due_at: args.get("dueAt").and_then(Value::as_u64).unwrap_or(0),
                condition: args.get("condition").and_then(Value::as_str).map(str::to_string),
                source_ref: args.get("sourceRef").and_then(Value::as_str).map(str::to_string),
                ..Default::default()
            };
            match super::commitments::with_ledger(|ledger| {
                ledger.add(row.clone())?;
                Ok(row)
            }) {
                Ok(commitment) => Ok(json!({ "ok": true, "commitment": commitment })),
                Err(error) => Ok(json!({ "ok": false, "error": "commitment_add_failed", "note": error })),
            }
        }
        COMMITMENT_LIST => {
            let Some(profile) = super::staff_commands::profile_for_session(session_id) else {
                return Ok(json!({ "ok": false, "error": "staff_missing", "note": "当前会话没有员工档案。" }));
            };
            let include_terminal = args.get("includeTerminal").and_then(Value::as_bool).unwrap_or(false);
            let rows = super::commitments::load()
                .commitments
                .into_iter()
                .filter(|item| item.staff_id == profile.staff_id)
                .filter(|item| include_terminal || item.status.is_open())
                .collect::<Vec<_>>();
            Ok(json!({ "ok": true, "commitments": rows }))
        }
        COMMITMENT_COMPLETE => {
            let Some(profile) = super::staff_commands::profile_for_session(session_id) else {
                return Ok(json!({ "ok": false, "error": "staff_missing", "note": "当前会话没有员工档案。" }));
            };
            let commitment_id = args.get("commitmentId").and_then(Value::as_str).unwrap_or("");
            if commitment_id.is_empty() {
                return Ok(json!({ "ok": false, "error": "bad_request", "note": "commitmentId 必填。" }));
            }
            let cancelled = args.get("cancelled").and_then(Value::as_bool).unwrap_or(false);
            match super::commitments::with_ledger(|ledger| {
                let current = ledger
                    .get(commitment_id)
                    .ok_or_else(|| "承诺不存在".to_string())?;
                if current.staff_id != profile.staff_id {
                    return Err("承诺不属于这个员工".to_string());
                }
                if cancelled {
                    ledger.cancel(commitment_id, super::types::now_ms())
                } else {
                    ledger.complete(commitment_id, super::types::now_ms())
                }
            }) {
                Ok(commitment) => Ok(json!({ "ok": true, "commitment": commitment })),
                Err(error) => Ok(json!({ "ok": false, "error": "commitment_complete_failed", "note": error })),
            }
        }
        STAFF_STATUS => {
            let Some(profile) = super::staff_commands::profile_for_session(session_id) else {
                return Ok(json!({ "ok": false, "error": "staff_missing", "note": "当前会话没有员工档案。" }));
            };
            let tasks = super::store::load_tasks()
                .into_iter()
                .filter(|task| {
                    task.chief_session_id == session_id
                        || task.staff_id.as_deref() == Some(profile.staff_id.as_str())
                })
                .collect::<Vec<_>>();
            let status = super::staff_status::project_status(
                &profile.staff_id,
                &tasks,
                super::types::now_ms(),
            );
            Ok(json!({
                "ok": true,
                "status": status,
                "policy": profile.policy,
            }))
        }
        _ => Err(format!("unknown chief tool: {name}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schemas_cover_the_documented_control_surface() {
        let schemas = chief_tool_schemas();
        let list = schemas.as_array().expect("array");
        assert_eq!(list.len(), ALL.len());
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
