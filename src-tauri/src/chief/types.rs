//! Chief 数据类型：任务、配置、项目策略、事件。
//!
//! 状态机（docs/research-telegram-chief/02-architecture.md §4.3）：
//!
//! ```text
//! proposed ─▶ starting ─▶ running ─┬─▶ waiting_user ─▶ running
//!   │                              ├─▶ completed / failed / cancelled
//!   └─▶ cancelled                  └─▶ interrupted ─(resume)─▶ starting
//! ```
//!
//! 终态无出边（`completed/failed/cancelled`）；`interrupted` 表示进程丢失
//! 或应用重启，不等于用户取消，可被显式 resume。

use serde::{Deserialize, Serialize};

/// 远程工作模式。`relay` 是现行 Telegram 桥（通知 + 引用回复 + 批准）；
/// `chief` 把裸文本固定送 Chief 会话。二者不能同时抢同一条裸文本。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RemoteWorkMode {
    #[default]
    Off,
    Relay,
    Chief,
}

/// 项目级远程策略。能力由 Host 配置决定，Chief 不能靠工具参数扩展
/// （05-mvp-plan.md §6.2：schema 里永远没有 cwd / permissionMode / key）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectPolicy {
    pub project_id: String,
    pub enabled: bool,
    /// read_only / approve_writes / write_preauthorized / blocked
    pub remote_mode: RemoteMode,
    /// 空表 = 允许全部已安装 Agent。
    pub allowed_agents: Vec<String>,
    pub default_agent_id: Option<String>,
    pub default_model: Option<String>,
    pub max_concurrent_workers: u32,
    pub max_task_minutes: u32,
    pub allow_computer_use: bool,
    /// 「让 AI 改 Marionette 自己」类仓库：默认独立 worktree，发布仍本地确认。
    pub self_modification: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RemoteMode {
    #[default]
    ReadOnly,
    ApproveWrites,
    WritePreauthorized,
    Blocked,
}

impl Default for ProjectPolicy {
    fn default() -> Self {
        Self {
            project_id: String::new(),
            enabled: true,
            remote_mode: RemoteMode::ApproveWrites,
            allowed_agents: Vec::new(),
            default_agent_id: None,
            default_model: None,
            max_concurrent_workers: 1,
            max_task_minutes: 120,
            allow_computer_use: false,
            self_modification: false,
        }
    }
}

/// Chief 配置（`~/.marionette/chief/config.json`，无凭据）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChiefConfig {
    pub version: u32,
    pub mode: RemoteWorkMode,
    /// Chief 的 ACP 会话（普通会话，`origin="chief"`）。
    pub chief_session_id: Option<String>,
    pub chief_agent_id: Option<String>,
    pub chief_model: Option<String>,
    pub allowed_projects: Vec<ProjectPolicy>,
    /// 全局并发上限（默认 1，最大 3；docs 决策表）。
    pub max_concurrent_workers: u32,
}

impl Default for ChiefConfig {
    fn default() -> Self {
        Self {
            version: 1,
            mode: RemoteWorkMode::Off,
            chief_session_id: None,
            chief_agent_id: None,
            chief_model: None,
            allowed_projects: Vec::new(),
            max_concurrent_workers: 1,
        }
    }
}

impl ChiefConfig {
    /// 全局并发上限的硬顶（05-mvp-plan.md §1：配置可到 3，不接受更多）。
    pub fn clamped_max_workers(&self) -> u32 {
        self.max_concurrent_workers.clamp(1, 3)
    }

    pub fn policy_for(&self, project_id: &str) -> Option<&ProjectPolicy> {
        self.allowed_projects
            .iter()
            .find(|p| p.project_id == project_id)
    }
}

/// 任务状态。终态（`is_terminal`）无出边；转移表在 [`Self::can_transition`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Proposed,
    Starting,
    Running,
    WaitingUser,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Cancelled
        )
    }

    /// 硬编码转移表（02-architecture.md §4.3）。所有转移必须走这里，
    /// 拒绝是返回值不是 panic。
    pub fn can_transition(self, next: TaskStatus) -> bool {
        use TaskStatus::*;
        match (self, next) {
            (Proposed, Starting | Cancelled) => true,
            (Starting, Running | Failed | Cancelled | Interrupted) => true,
            (Running, WaitingUser | Completed | Failed | Cancelled | Interrupted) => true,
            (WaitingUser, Running | Completed | Failed | Cancelled | Interrupted) => true,
            (Interrupted, Starting | Cancelled) => true,
            // 终态无出边；同状态幂等转移允许（重复事件不报错）。
            (a, b) if a == b => a.is_terminal() || matches!(a, Proposed | Starting | Running | WaitingUser | Interrupted),
            _ => false,
        }
    }
}

/// 任务记录（`tasks.json` 快照行）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Task {
    pub task_id: String,
    /// Chief 生成的幂等键：重试同一 `requestId` 返回同一 `taskId`。
    pub request_id: String,
    /// Stable employee ownership, independent from the current ACP session id.
    pub staff_id: Option<String>,
    pub chief_session_id: String,
    pub title: String,
    pub goal: String,
    pub project_id: String,
    pub worker_session_id: Option<String>,
    pub agent_id: String,
    pub model: Option<String>,
    pub status: TaskStatus,
    /// permission / question / plan_approval while waiting_user.
    pub waiting_reason: Option<String>,
    /// Worker prompts issued for this task (initial prompt + follow-ups/resumes).
    pub turn_count: u32,
    /// Agent-reported USD cost across this task's worker turns.
    pub cost_usd: f64,
    /// Unix 毫秒。
    pub created_at: u64,
    pub updated_at: u64,
    pub last_event_at: u64,
    pub last_summary: Option<String>,
    pub stop_reason: Option<String>,
    pub error: Option<String>,
    /// 最近一条任务卡在 Telegram 的 message id（引用回复路由用）。
    pub telegram_message_id: Option<i64>,
    /// 完成时的产物摘要（changed files / 测试结果行）。
    pub artifacts: Vec<String>,
}

impl Default for Task {
    fn default() -> Self {
        let now = now_ms();
        Self {
            task_id: String::new(),
            request_id: String::new(),
            staff_id: None,
            chief_session_id: String::new(),
            title: String::new(),
            goal: String::new(),
            project_id: String::new(),
            worker_session_id: None,
            agent_id: String::new(),
            model: None,
            status: TaskStatus::Proposed,
            waiting_reason: None,
            turn_count: 0,
            cost_usd: 0.0,
            created_at: now,
            updated_at: now,
            last_event_at: now,
            last_summary: None,
            stop_reason: None,
            error: None,
            telegram_message_id: None,
            artifacts: Vec::new(),
        }
    }
}

/// Host 事件（`events.jsonl` 追加行）。这是任务账本的事实记录；
/// Chief 的唤醒（event inbox）从这些事件派生。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub event_id: String,
    /// 每 task 单调递增，从 1 起。
    pub seq: u64,
    pub task_id: String,
    /// task.created / task.started / task.progress / task.waiting_user /
    /// task.completed / task.failed / task.interrupted / task.cancelled /
    /// task.wake_delivered
    pub kind: String,
    pub status: Option<TaskStatus>,
    pub summary: Option<String>,
    /// 消费时间戳（唤醒送达 Chief 后回填；None = 尚未唤醒）。
    pub delivered_at: Option<u64>,
    pub ts: u64,
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 短随机后缀（任务/事件 id 用；不追求密码学强度，只要求不可猜且唯一）。
pub fn short_id(prefix: &str) -> String {
    let time = now_ms();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{prefix}_{time}_{nonce:06x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_machine_follows_the_doc_diagram() {
        use TaskStatus::*;
        // 主路径
        assert!(Proposed.can_transition(Starting));
        assert!(Starting.can_transition(Running));
        assert!(Running.can_transition(WaitingUser));
        assert!(WaitingUser.can_transition(Running));
        assert!(Running.can_transition(Completed));
        // 终态无出边
        assert!(!Completed.can_transition(Running));
        assert!(!Failed.can_transition(Starting));
        assert!(!Cancelled.can_transition(Proposed));
        // interrupted 可显式恢复 / 取消
        assert!(Interrupted.can_transition(Starting));
        assert!(Interrupted.can_transition(Cancelled));
        assert!(!Interrupted.can_transition(Completed));
        // 幂等转移（重复事件）
        assert!(Running.can_transition(Running));
        assert!(Completed.can_transition(Completed));
    }

    #[test]
    fn config_defaults_are_conservative() {
        let config = ChiefConfig::default();
        assert_eq!(config.mode, RemoteWorkMode::Off);
        assert_eq!(config.clamped_max_workers(), 1);
        let loose = ChiefConfig {
            max_concurrent_workers: 99,
            ..Default::default()
        };
        assert_eq!(loose.clamped_max_workers(), 3);
    }

    #[test]
    fn serde_roundtrip_uses_camel_case() {
        let task = Task {
            task_id: "task_1".into(),
            request_id: "req_1".into(),
            chief_session_id: "session_c".into(),
            title: "t".into(),
            goal: "g".into(),
            project_id: "project_x".into(),
            worker_session_id: Some("session_w".into()),
            agent_id: "codex".into(),
            status: TaskStatus::Running,
            ..Default::default()
        };
        let json = serde_json::to_string(&task).unwrap();
        assert!(json.contains("\"taskId\":\"task_1\""));
        assert!(json.contains("\"chiefSessionId\":\"session_c\""));
        assert!(json.contains("\"status\":\"running\""));
        let back: Task = serde_json::from_str(&json).unwrap();
        assert_eq!(back.task_id, "task_1");
        assert_eq!(back.status, TaskStatus::Running);
    }

    #[test]
    fn old_status_strings_still_deserialize() {
        let status: TaskStatus = serde_json::from_str("\"waiting_user\"").unwrap();
        assert_eq!(status, TaskStatus::WaitingUser);
    }
}
