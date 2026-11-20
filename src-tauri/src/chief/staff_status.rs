//! 员工状态投影。
//!
//! 这里只读 `Task` 快照，不写数据库、不发事件，也不把 UI 状态反向写成
//! 业务事实。worker/task 是临时工作卡，员工行只是这些事实的聚合视图。

use super::types::{Task, TaskStatus};
use serde::{Deserialize, Serialize};

/// running 状态超过该时长没有新事件时标为 stale；不自动伪造失败。
pub const DEFAULT_STALE_AFTER_MS: u64 = 5 * 60 * 1000;
pub const TERMINAL_RETENTION_MS: u64 = 5 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StaffDisplayState {
    WaitingUser,
    Failed,
    Interrupted,
    Running,
    Thinking,
    #[default]
    Idle,
}

impl StaffDisplayState {
    /// 数值越大越需要用户注意。
    pub fn priority(self) -> u8 {
        match self {
            Self::WaitingUser => 5,
            Self::Failed => 4,
            Self::Interrupted => 4,
            Self::Running => 3,
            Self::Thinking => 2,
            Self::Idle => 0,
        }
    }

    pub fn is_active(self) -> bool {
        !matches!(self, Self::Idle)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct StaffTaskCounts {
    pub proposed: u32,
    pub starting: u32,
    pub running: u32,
    pub waiting_user: u32,
    pub completed: u32,
    pub failed: u32,
    pub cancelled: u32,
    pub interrupted: u32,
    pub total: u32,
}

#[allow(dead_code)]
impl StaffTaskCounts {
    pub fn active(self) -> u32 {
        self.proposed
            .saturating_add(self.starting)
            .saturating_add(self.running)
            .saturating_add(self.waiting_user)
    }

    pub fn needs_user(self) -> u32 {
        self.waiting_user
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct StaffStatus {
    pub staff_id: String,
    pub display_state: StaffDisplayState,
    pub active_task_id: Option<String>,
    pub active_task_title: Option<String>,
    pub waiting_reason: Option<String>,
    pub task_counts: StaffTaskCounts,
    pub last_event_at: u64,
    pub last_progress_at: u64,
    pub last_report_at: Option<u64>,
    /// 基于 `DEFAULT_STALE_AFTER_MS` 的便捷投影；复杂场景用 `is_stale_at`。
    pub stale: bool,
}

#[allow(dead_code)]
impl StaffStatus {
    pub fn is_stale_at(&self, now: u64, after_ms: u64) -> bool {
        self.display_state.is_active()
            && self.last_event_at > 0
            && now.saturating_sub(self.last_event_at) >= after_ms
    }

    pub fn waiting_reason_text(&self) -> Option<&str> {
        self.waiting_reason.as_deref()
    }
}

/// 将一组 task 投影为员工行。调用方应只传入该员工的 task；不做 DB 查询。
pub fn project_status(staff_id: &str, tasks: &[Task], now: u64) -> StaffStatus {
    project_status_with_report(staff_id, tasks, now, None)
}

/// 兼容更明确的调用命名。
#[allow(dead_code)]
pub fn project_staff_status(staff_id: &str, tasks: &[Task], now: u64) -> StaffStatus {
    project_status(staff_id, tasks, now)
}

pub fn project_status_with_report(
    staff_id: &str,
    tasks: &[Task],
    now: u64,
    last_report_at: Option<u64>,
) -> StaffStatus {
    let mut counts = StaffTaskCounts::default();
    let mut chosen: Option<(&Task, StaffDisplayState)> = None;
    let mut last_event_at = 0;
    let mut last_progress_at = 0;

    for task in tasks {
        counts.total = counts.total.saturating_add(1);
        match task.status {
            TaskStatus::Proposed => counts.proposed = counts.proposed.saturating_add(1),
            TaskStatus::Starting => counts.starting = counts.starting.saturating_add(1),
            TaskStatus::Running => counts.running = counts.running.saturating_add(1),
            TaskStatus::WaitingUser => counts.waiting_user = counts.waiting_user.saturating_add(1),
            TaskStatus::Completed => counts.completed = counts.completed.saturating_add(1),
            TaskStatus::Failed => counts.failed = counts.failed.saturating_add(1),
            TaskStatus::Cancelled => counts.cancelled = counts.cancelled.saturating_add(1),
            TaskStatus::Interrupted => counts.interrupted = counts.interrupted.saturating_add(1),
        }

        let task_last_event = task.last_event_at.max(task.updated_at);
        last_event_at = last_event_at.max(task_last_event);
        let Some(state) = display_state_for(task.status) else {
            continue;
        };
        if matches!(state, StaffDisplayState::Failed | StaffDisplayState::Interrupted)
            && task_last_event > 0
            && now.saturating_sub(task_last_event) >= TERMINAL_RETENTION_MS
        {
            continue;
        }
        last_progress_at = last_progress_at.max(task.updated_at);
        let replace = chosen
            .as_ref()
            .map(|(current, current_state)| {
                let task_key = (state.priority(), task_last_event, task.updated_at);
                let current_key = (
                    current_state.priority(),
                    current.last_event_at.max(current.updated_at),
                    current.updated_at,
                );
                task_key > current_key
                    || (task_key == current_key && task.task_id < current.task_id)
            })
            .unwrap_or(true);
        if replace {
            chosen = Some((task, state));
        }
    }

    let (active_task_id, active_task_title, waiting_reason, display_state) = match chosen {
        Some((task, state)) => (
            Some(task.task_id.clone()),
            Some(task.title.clone()),
            if state == StaffDisplayState::WaitingUser {
                waiting_reason(task)
            } else {
                None
            },
            state,
        ),
        None => (None, None, None, StaffDisplayState::Idle),
    };

    let stale = StaffDisplayState::is_active(display_state)
        && last_event_at > 0
        && now.saturating_sub(last_event_at) >= DEFAULT_STALE_AFTER_MS;
    StaffStatus {
        staff_id: staff_id.to_string(),
        display_state,
        active_task_id,
        active_task_title,
        waiting_reason,
        task_counts: counts,
        last_event_at,
        last_progress_at,
        last_report_at,
        stale,
    }
}

fn display_state_for(status: TaskStatus) -> Option<StaffDisplayState> {
    match status {
        TaskStatus::WaitingUser => Some(StaffDisplayState::WaitingUser),
        TaskStatus::Failed => Some(StaffDisplayState::Failed),
        TaskStatus::Interrupted => Some(StaffDisplayState::Interrupted),
        TaskStatus::Running => Some(StaffDisplayState::Running),
        TaskStatus::Proposed | TaskStatus::Starting => Some(StaffDisplayState::Thinking),
        TaskStatus::Completed | TaskStatus::Cancelled => None,
    }
}

fn waiting_reason(task: &Task) -> Option<String> {
    task.waiting_reason
        .clone()
        .or_else(|| task.stop_reason.clone())
        .or_else(|| task.error.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, status: TaskStatus, at: u64) -> Task {
        Task {
            task_id: id.into(),
            title: format!("task {id}"),
            status,
            updated_at: at,
            last_event_at: at,
            ..Default::default()
        }
    }

    #[test]
    fn waiting_user_has_highest_priority() {
        let tasks = vec![
            task("running", TaskStatus::Running, 2_000),
            task("failed", TaskStatus::Failed, 2_000),
            task("waiting", TaskStatus::WaitingUser, 1_000),
        ];
        let status = project_status("staff_a", &tasks, 2_100);
        assert_eq!(status.display_state, StaffDisplayState::WaitingUser);
        assert_eq!(status.active_task_id.as_deref(), Some("waiting"));
        assert_eq!(status.task_counts.waiting_user, 1);
        assert_eq!(status.task_counts.total, 3);
    }

    #[test]
    fn failed_and_interrupted_are_distinct_and_beat_running() {
        let failed = project_status(
            "staff_a",
            &[
                task("running", TaskStatus::Running, 100),
                task("failed", TaskStatus::Failed, 50),
            ],
            200,
        );
        assert_eq!(failed.display_state, StaffDisplayState::Failed);
        let interrupted = project_status(
            "staff_a",
            &[
                task("running", TaskStatus::Running, 100),
                task("interrupted", TaskStatus::Interrupted, 50),
            ],
            200,
        );
        assert_eq!(interrupted.display_state, StaffDisplayState::Interrupted);
    }

    #[test]
    fn starting_is_thinking_and_empty_is_idle() {
        let status = project_status("staff_a", &[task("proposed", TaskStatus::Starting, 1)], 2);
        assert_eq!(status.display_state, StaffDisplayState::Thinking);
        assert_eq!(
            project_status("staff_a", &[], 2).display_state,
            StaffDisplayState::Idle
        );
    }

    #[test]
    fn stale_is_a_projection_not_a_status_rewrite() {
        let task = task("running", TaskStatus::Running, 1_000);
        let status = project_status("staff_a", &[task], 1_000 + DEFAULT_STALE_AFTER_MS);
        assert!(status.stale);
        assert!(status.is_stale_at(1_000 + DEFAULT_STALE_AFTER_MS, DEFAULT_STALE_AFTER_MS));
        assert_eq!(status.display_state, StaffDisplayState::Running);
    }
}
