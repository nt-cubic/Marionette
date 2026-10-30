//! Council 任务图:PlanDoc → 可执行 TaskNode 集合 + frontier 调度。
//!
//! 语义参考 OmO `senpi-task/src/dag/scheduler-frontier.ts`(frontier 而非
//! 严格波次 barrier):依赖全部完成的任务即刻就绪。失败**不级联**:下游
//! 任务的依赖里有 failed 时按 skipped 收尾(等所有可跑的跑完再判定,
//! 避免"兄弟还在飞时错判搁浅"的回归)。

use super::types::{PlanDoc, PlanStep};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    Running,
    Completed,
    Failed { reason: String },
    Skipped { reason: String },
    /// worktree 里的工作完成,等待合并回主工作区。
    Staged,
    Merged,
}

/// 单个步骤的执行账目(随 state.json 持久化,恢复时以此为进度)。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgress {
    pub step_id: String,
    pub title: String,
    pub status: TaskStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// 专属 worktree 路径(并行任务隔离;串行/无 git 时为空)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default)]
    pub changes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 执行次数(修复回路限次用)。
    #[serde(default)]
    pub attempts: u32,
    /// harness 侧备注(变更摘要/超时说明)。
    #[serde(default)]
    pub notes: String,
}

/// PlanDoc → TaskProgress 初始化(依赖校验:未知依赖剔除并记 skipped
/// 候选;环检测——PlanDoc 来自模型输出,不可信)。
pub fn build_task_progress(plan: &PlanDoc) -> Result<Vec<TaskProgress>, String> {
    if plan.steps.is_empty() {
        return Err("计划没有步骤".into());
    }
    let ids: Vec<&str> = plan.steps.iter().map(|s| s.id.as_str()).collect();
    // 环检测:反复剥无依赖节点,剥不完 = 有环。
    let mut remaining: Vec<&PlanStep> = plan.steps.iter().collect();
    let mut resolved: Vec<String> = Vec::new();
    let mut guard = 0;
    while !remaining.is_empty() {
        guard += 1;
        if guard > plan.steps.len() + 1 {
            let stuck: Vec<&str> = remaining.iter().map(|s| s.id.as_str()).collect();
            return Err(format!("计划步骤存在依赖环:{}", stuck.join(", ")));
        }
        let mut progressed = false;
        let mut next_remaining = Vec::new();
        for step in remaining {
            // 只看图内依赖:图外依赖不是环(由 finalize_skipped 收 skipped),
            // 否则模型输出引用不存在的步骤 id 会被误报成环。
            let deps_ok = step
                .depends_on
                .iter()
                .filter(|d| plan.steps.iter().any(|s| &s.id == *d))
                .all(|d| resolved.iter().any(|r| r == d));
            if deps_ok {
                resolved.push(step.id.clone());
                progressed = true;
            } else {
                next_remaining.push(step);
            }
        }
        if !progressed {
            let stuck: Vec<&str> = next_remaining.iter().map(|s| s.id.as_str()).collect();
            return Err(format!("计划步骤存在依赖环:{}", stuck.join(", ")));
        }
        remaining = next_remaining;
    }
    let _ = ids;
    Ok(plan
        .steps
        .iter()
        .map(|s| TaskProgress {
            step_id: s.id.clone(),
            title: s.title.clone(),
            status: TaskStatus::Pending,
            session_id: None,
            worktree: None,
            branch: None,
            changes: Vec::new(),
            error: None,
            attempts: 0,
            notes: String::new(),
        })
        .collect())
}

/// frontier:依赖(按 plan 的 depends_on)全部 completed/merged 且自身
/// pending 的任务。failed/skipped 依赖**不解锁**下游(由
/// `finalize_skipped` 统一收尾);未知依赖视为不满足(freeze 防死锁由
/// `work_remaining` + finalize 兜底)。
pub fn ready_steps<'a>(plan: &'a PlanDoc, tasks: &'a [TaskProgress]) -> Vec<&'a PlanStep> {
    plan.steps
        .iter()
        .filter(|step| {
            let Some(prog) = tasks.iter().find(|t| t.step_id == step.id) else { return false };
            prog.status == TaskStatus::Pending
        })
        .filter(|step| {
            step.depends_on.iter().all(|d| {
                tasks.iter().any(|t| {
                    t.step_id == *d && matches!(t.status, TaskStatus::Completed | TaskStatus::Merged)
                })
            })
        })
        .collect()
}

/// 全部可执行工作结束(无 running、无 pending-ready)后,把依赖含
/// failed/skipped(或引用了图外步骤、永远无法满足)的 pending 任务收为
/// skipped(失败不级联成假成功)。
pub fn finalize_skipped(plan: &PlanDoc, tasks: &mut [TaskProgress]) {
    loop {
        let mut changed = false;
        for step in &plan.steps {
            let dep_blocked = |d: &str| -> bool {
                if !plan.steps.iter().any(|s| s.id == *d) {
                    return true; // 图外依赖:永不满足
                }
                tasks.iter().any(|t| {
                    t.step_id == *d && matches!(t.status, TaskStatus::Failed { .. } | TaskStatus::Skipped { .. })
                })
            };
            if step.depends_on.iter().any(|d| dep_blocked(d)) {
                if let Some(t) = tasks.iter_mut().find(|t| t.step_id == step.id) {
                    if t.status == TaskStatus::Pending {
                        t.status = TaskStatus::Skipped { reason: "上游任务失败或依赖缺失".into() };
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            break;
        }
    }
}

/// 是否还有可推进的执行工作(running 或 ready 非空)。
pub fn work_remaining(plan: &PlanDoc, tasks: &[TaskProgress]) -> bool {
    let running = tasks.iter().any(|t| t.status == TaskStatus::Running);
    running || !ready_steps(plan, tasks).is_empty()
}

/// 合并顺序:按 plan 声明顺序(拓扑序已在 build 时验证)。
pub fn merge_order(plan: &PlanDoc) -> Vec<String> {
    plan.steps.iter().map(|s| s.id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::council::types::PlanStep;

    fn step(id: &str, deps: Vec<&str>) -> PlanStep {
        PlanStep {
            id: id.into(),
            title: format!("step {id}"),
            detail: "d".into(),
            depends_on: deps.into_iter().map(|s| s.to_string()).collect(),
            acceptance: "a".into(),
            owner_hint: None,
        }
    }

    fn doc(steps: Vec<PlanStep>) -> PlanDoc {
        PlanDoc { summary: "s".into(), steps, open_questions: vec![], acceptance: vec![], verification: vec![] }
    }

    fn status_of(tasks: &[TaskProgress], id: &str) -> TaskStatus {
        tasks.iter().find(|t| t.step_id == id).unwrap().status.clone()
    }

    #[test]
    fn cycle_detection_rejects_circular_plan() {
        let plan = doc(vec![step("a", vec!["b"]), step("b", vec!["a"])]);
        assert!(build_task_progress(&plan).is_err());
        // 自依赖同样拒绝。
        let plan2 = doc(vec![step("a", vec!["a"])]);
        assert!(build_task_progress(&plan2).is_err());
    }

    #[test]
    fn frontier_unlocks_only_when_deps_complete() {
        let plan = doc(vec![step("s1", vec![]), step("s2", vec!["s1"]), step("s3", vec![])]);
        let mut tasks = build_task_progress(&plan).unwrap();
        // 初始:s1、s3 就绪;s2 被 s1 挡。
        let ready: Vec<&str> = ready_steps(&plan, &tasks).iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ready, vec!["s1", "s3"]);
        // s1 完成 → s2 解锁。
        tasks[0].status = TaskStatus::Completed;
        let ready2: Vec<&str> = ready_steps(&plan, &tasks).iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ready2, vec!["s2", "s3"]);
        // s1 运行中不阻塞判定自身,但 s2 仍等 completed。
        tasks[0].status = TaskStatus::Running;
        let ready3: Vec<&str> = ready_steps(&plan, &tasks).iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ready3, vec!["s3"]);
    }

    #[test]
    fn failure_does_not_cascade_but_finalizes_skipped() {
        let plan = doc(vec![step("s1", vec![]), step("s2", vec!["s1"]), step("s3", vec![])]);
        let mut tasks = build_task_progress(&plan).unwrap();
        tasks[0].status = TaskStatus::Failed { reason: "boom".into() };
        // 失败不级联:s3 仍可跑(还在飞时 s2 不提前判死)。
        let ready: Vec<&str> = ready_steps(&plan, &tasks).iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ready, vec!["s3"]);
        // 全部停当后,s2 收为 skipped。
        tasks[2].status = TaskStatus::Completed;
        finalize_skipped(&plan, &mut tasks);
        assert!(matches!(status_of(&tasks, "s2"), TaskStatus::Skipped { .. }));
        assert_eq!(status_of(&tasks, "s3"), TaskStatus::Completed);
        assert!(!work_remaining(&plan, &tasks));
    }

    #[test]
    fn unknown_dependency_does_not_deadlock() {
        // 模型输出引用了不存在的步骤 id:该依赖视为永远不满足 →
        // finalize 阶段……unknown 不在 tasks 里,failed 判定不命中,
        // 这里验证不会 panic 且 build 成功(剔依赖交给 plan 解析容错)。
        let plan = doc(vec![step("a", vec!["ghost"]), step("ghost2", vec![])]);
        let tasks = build_task_progress(&plan).unwrap();
        assert_eq!(tasks.len(), 2);
    }
}
