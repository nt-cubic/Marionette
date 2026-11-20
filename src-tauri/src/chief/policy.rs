//! Host 侧策略裁决（03-security-permissions.md §5–6）。
//!
//! 能力由 Host 配置决定，**不接受 Chief 在工具参数里自称安全**。
//! 拒绝是可读的返回值（`{error, note}` 形状由 tools 层包装），这里
//! 返回结构化的 [`Rejection`]，绝不 panic、绝不静默放行。

use super::types::{ChiefConfig, RemoteMode, TaskStatus};
use serde_json::{json, Value};

/// 一次策略裁决结果。`Ok(Grant)` 携带 Host 校验后的参数（Chief 给的字面值
/// 不直接采用——默认值、允许表在这里定型）。
#[derive(Debug, Clone)]
pub enum Decision {
    Grant {
        project_id: String,
        agent_id: String,
        model: Option<String>,
        remote_mode: RemoteMode,
        max_task_minutes: u32,
        self_modification: bool,
    },
    Reject {
        error: &'static str,
        note: String,
    },
}

impl Decision {
    #[allow(dead_code)]
    pub fn into_value(self) -> Value {
        match self {
            Decision::Grant { .. } => json!({ "ok": true }),
            Decision::Reject { error, note } => json!({ "ok": false, "error": error, "note": note }),
        }
    }
}

/// `marionette_task_start` 的入口校验。`requested_agent` / `requested_model`
/// 为 None 时用项目默认（或已安装第一个可用 Agent 由调用方解析）。
pub fn validate_task_start(
    config: &ChiefConfig,
    project_id: &str,
    requested_agent: Option<&str>,
    requested_model: Option<&str>,
    known_agents: &[String],
) -> Decision {
    let Some(policy) = config.policy_for(project_id) else {
        return Decision::Reject {
            error: "project_not_allowed",
            note: format!(
                "项目 {project_id} 不在 Chief 允许列表里。请在本地「远程助理」设置中添加该项目的授权。"
            ),
        };
    };
    if !policy.enabled || policy.remote_mode == RemoteMode::Blocked {
        return Decision::Reject {
            error: "project_blocked",
            note: format!("项目 {project_id} 当前被禁用（remoteMode={}）。", format!("{:?}", policy.remote_mode).to_lowercase()),
        };
    }

    let agent_id = match requested_agent.or(policy.default_agent_id.as_deref()) {
        Some(agent) => agent.to_string(),
        None => {
            // 无显式指定、无项目默认：取允许表外第一个已安装 Agent（调用方
            // 传入 known_agents；空表按 policy.allowed_agents 空表语义放行）。
            match known_agents.first() {
                Some(first) => first.clone(),
                None => {
                    return Decision::Reject {
                        error: "agent_unavailable",
                        note: "没有可用 Agent。请确认目标 Agent 已安装并登录。".to_string(),
                    }
                }
            }
        }
    };
    if !policy.allowed_agents.is_empty() && !policy.allowed_agents.contains(&agent_id) {
        return Decision::Reject {
            error: "agent_not_allowed",
            note: format!(
                "Agent {agent_id} 不在项目 {project_id} 的允许列表里（允许：{}）。",
                policy.allowed_agents.join(", ")
            ),
        };
    }
    if !known_agents.is_empty() && !known_agents.contains(&agent_id) {
        return Decision::Reject {
            error: "agent_unavailable",
            note: format!("Agent {agent_id} 未安装或未启用。"),
        };
    }

    let model = requested_model.or(policy.default_model.as_deref()).map(str::to_string);

    Decision::Grant {
        project_id: project_id.to_string(),
        agent_id,
        model,
        remote_mode: policy.remote_mode,
        max_task_minutes: policy.max_task_minutes,
        self_modification: policy.self_modification,
    }
}

/// `remote_mode` 能否承载「可能写文件/跑命令」的任务。read_only 项目里
/// Chief 仍可发起任务，worker 起在只读权限档；写预授权完全交给本地配置。
#[allow(dead_code)]
pub fn can_write(remote_mode: RemoteMode) -> bool {
    matches!(
        remote_mode,
        RemoteMode::ApproveWrites | RemoteMode::WritePreauthorized
    )
}

/// `marionette_task_send` / `task_cancel` / `task_collect` 的所有权校验：
/// 只允许本 Chief 的任务、终态任务拒绝 send、cancelled 幂等放行。
#[allow(dead_code)]
pub fn validate_task_access(
    task: &super::types::Task,
    chief_session_id: &str,
    action: &str,
) -> Result<(), (&'static str, String)> {
    validate_task_access_for_staff(task, chief_session_id, None, action)
}

/// Same ownership check, with the stable staff id as a fallback when an
/// employee session had to be recreated.
pub fn validate_task_access_for_staff(
    task: &super::types::Task,
    chief_session_id: &str,
    staff_id: Option<&str>,
    action: &str,
) -> Result<(), (&'static str, String)> {
    let owns = task.chief_session_id == chief_session_id
        || staff_id.is_some_and(|staff_id| task.staff_id.as_deref() == Some(staff_id));
    if !owns {
        return Err((
            "task_not_owned",
            format!("任务 {} 不属于这个 Chief。", task.task_id),
        ));
    }
    match action {
        "send" => {
            if task.status.is_terminal() {
                return Err((
                    "task_finished",
                    format!("任务 {} 已结束（{}），不能再发补充指令。", task.task_id, format!("{:?}", task.status).to_lowercase()),
                ));
            }
            if task.status == TaskStatus::Proposed || task.status == TaskStatus::Starting {
                return Err((
                    "task_not_running",
                    format!("任务 {} 还没进入运行态，等启动完成后才能追加指令。", task.task_id),
                ));
            }
        }
        "cancel" => {
            if task.status == TaskStatus::Cancelled {
                return Ok(()); // 幂等：已取消的任务重复取消直接成功
            }
            if task.status.is_terminal() {
                return Err((
                    "task_finished",
                    format!("任务 {} 已是终态（{}），无需取消。", task.task_id, format!("{:?}", task.status).to_lowercase()),
                ));
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chief::types::{ChiefConfig, ProjectPolicy, RemoteMode, RemoteWorkMode};

    fn config_with(project: &str, remote_mode: RemoteMode, allowed: Vec<String>) -> ChiefConfig {
        ChiefConfig {
            mode: RemoteWorkMode::Chief,
            allowed_projects: vec![ProjectPolicy {
                project_id: project.into(),
                remote_mode,
                allowed_agents: allowed,
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn rejects_projects_outside_allowlist() {
        let config = config_with("project_ok", RemoteMode::ApproveWrites, vec![]);
        let decision = validate_task_start(&config, "project_other", None, None, &["codex".into()]);
        match decision {
            Decision::Reject { error, .. } => assert_eq!(error, "project_not_allowed"),
            _ => panic!("must reject"),
        }
    }

    #[test]
    fn rejects_blocked_projects() {
        let config = config_with("project_a", RemoteMode::Blocked, vec![]);
        let decision = validate_task_start(&config, "project_a", None, None, &["codex".into()]);
        match decision {
            Decision::Reject { error, .. } => assert_eq!(error, "project_blocked"),
            _ => panic!("must reject"),
        }
    }

    #[test]
    fn agent_allowlist_is_enforced() {
        let config = config_with("project_a", RemoteMode::ApproveWrites, vec!["claude-code".into()]);
        let decision = validate_task_start(&config, "project_a", Some("codex"), None, &["codex".into()]);
        match decision {
            Decision::Reject { error, .. } => assert_eq!(error, "agent_not_allowed"),
            _ => panic!("must reject"),
        }
    }

    #[test]
    fn unknown_agent_fails_closed() {
        let config = config_with("project_a", RemoteMode::ApproveWrites, vec![]);
        let decision = validate_task_start(&config, "project_a", Some("ghost-agent"), None, &["codex".into()]);
        match decision {
            Decision::Reject { error, .. } => assert_eq!(error, "agent_unavailable"),
            _ => panic!("must reject"),
        }
    }

    #[test]
    fn grant_uses_project_defaults() {
        let mut config = config_with("project_a", RemoteMode::ReadOnly, vec![]);
        config.allowed_projects[0].default_agent_id = Some("codex".into());
        config.allowed_projects[0].default_model = Some("provider/model-x".into());
        match validate_task_start(&config, "project_a", None, None, &["codex".into()]) {
            Decision::Grant {
                agent_id,
                model,
                remote_mode,
                max_task_minutes,
                self_modification,
                ..
            } => {
                assert_eq!(agent_id, "codex");
                assert_eq!(model.as_deref(), Some("provider/model-x"));
                assert_eq!(remote_mode, RemoteMode::ReadOnly);
                assert_eq!(max_task_minutes, 120);
                assert!(!self_modification);
            }
            _ => panic!("must grant"),
        }
    }

    #[test]
    fn task_access_ownership_and_lifecycle() {
        use crate::chief::types::Task;
        let mut task = Task {
            chief_session_id: "session_c1".into(),
            status: TaskStatus::Running,
            ..Default::default()
        };
        assert!(validate_task_access(&task, "session_c1", "send").is_ok());
        // 别的 Chief 不能碰
        assert!(validate_task_access(&task, "session_c2", "send").is_err());
        // 终态拒绝 send，cancel 幂等
        task.status = TaskStatus::Completed;
        assert!(validate_task_access(&task, "session_c1", "send").is_err());
        task.status = TaskStatus::Cancelled;
        assert!(validate_task_access(&task, "session_c1", "cancel").is_ok());
        // 未运行不能 send
        task.status = TaskStatus::Proposed;
        assert!(validate_task_access(&task, "session_c1", "send").is_err());
    }
}
