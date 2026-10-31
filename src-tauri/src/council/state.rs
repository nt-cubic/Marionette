//! Council 状态机:硬编码转移表 + 审计日志语义。
//!
//! 非法转移直接 `Err`(对齐 OmO `InvalidTransitionError`,但穷尽性由
//! Rust enum 的编译期保证背书)。`Paused` 的出边是动态的(回到
//! `paused_from`),在 `can_resume` 单独判。

use super::types::{now_millis, CouncilState, RunPhase, TransitionRecord};

/// 合法转移表。数据驱动而非 match 大表,便于单测逐条对账。
pub fn can_transition(from: RunPhase, to: RunPhase) -> bool {
    use RunPhase::*;
    matches!(
        (from, to),
        (Created, Analyzing)
            | (Created, Cancelled)
            | (Created, Failed)
            | (Analyzing, Proposing)
            | (Proposing, Reviewing)
            | (Reviewing, Revising)
            | (Reviewing, Synthesizing)
            | (Revising, Reviewing)
            | (Revising, Synthesizing)
            | (Synthesizing, PlanReady)
            // 执行门:PlanReady 只能经用户批准进 Executing(approve_plan
            // 是唯一入口,orchestrator 自己永远不走这条边——验收 #9)。
            // PlanReady→Completed 是 council-only 交付专用(用户接受计划、
            // 无代码执行),与假执行无关——transitions.jsonl 记录原因。
            | (PlanReady, Executing)
            | (PlanReady, Completed)
            | (Executing, Verifying)
            | (Executing, AwaitingApproval)
            | (AwaitingApproval, Executing)
            | (Verifying, Executing)
            | (Verifying, Completed)
            // 任意运行态 → 暂停/取消/失败/超支(终态无出边)。
            | (Analyzing, Paused)
            | (Proposing, Paused)
            | (Reviewing, Paused)
            | (Revising, Paused)
            | (Synthesizing, Paused)
            | (PlanReady, Paused)
            | (AwaitingApproval, Paused)
            | (Executing, Paused)
            | (Verifying, Paused)
            | (Analyzing, Cancelled)
            | (Proposing, Cancelled)
            | (Reviewing, Cancelled)
            | (Revising, Cancelled)
            | (Synthesizing, Cancelled)
            | (PlanReady, Cancelled)
            | (AwaitingApproval, Cancelled)
            | (Executing, Cancelled)
            | (Verifying, Cancelled)
            | (Analyzing, Failed)
            | (Proposing, Failed)
            | (Reviewing, Failed)
            | (Revising, Failed)
            | (Synthesizing, Failed)
            | (Executing, Failed)
            | (Verifying, Failed)
            | (AwaitingApproval, Failed)
            | (Created, Playing)
            | (Playing, Completed)
            | (Playing, Cancelled)
            | (Playing, Failed)
            | (Playing, BudgetExhausted)
            | (Playing, Paused)
            | (Analyzing, BudgetExhausted)
            | (Proposing, BudgetExhausted)
            | (Reviewing, BudgetExhausted)
            | (Revising, BudgetExhausted)
            | (Synthesizing, BudgetExhausted)
            | (Executing, BudgetExhausted)
            | (Verifying, BudgetExhausted)
    )
}

/// Paused → 之前的阶段(`paused_from` 必须是可恢复的运行态)。
pub fn can_resume(state: &CouncilState) -> Result<RunPhase, String> {
    if state.phase != RunPhase::Paused {
        return Err(format!("当前状态 {} 不可恢复", phase_name(state.phase)));
    }
    match state.paused_from {
        Some(target) if target.is_running() => Ok(target),
        Some(target) => Err(format!("暂停记录的目标状态 {} 不可恢复", phase_name(target))),
        None => Err("暂停记录缺失恢复目标".into()),
    }
}

/// 状态机转移:改 phase、留 paused_from、刷 updated_at,产出审计记录。
/// 调用方负责持久化 state.json 与 append transitions.jsonl。
pub fn apply_transition(
    state: &mut CouncilState,
    to: RunPhase,
    reason: Option<&str>,
) -> Result<TransitionRecord, String> {
    let from = state.phase;
    if from == RunPhase::Paused {
        // 暂停的动态出边:恢复到 paused_from,或取消。
        if to == RunPhase::Cancelled {
            // 允许:暂停中的 run 可以直接取消。
        } else {
            let target = can_resume(state)?;
            if to != target {
                return Err(format!("暂停只能恢复到 {}(请求 {})", phase_name(target), phase_name(to)));
            }
        }
    } else if from.is_terminal() {
        return Err(format!("终态 {} 不可再转移", phase_name(from)));
    } else if !can_transition(from, to) {
        return Err(format!("非法转移:{} → {}", phase_name(from), phase_name(to)));
    }
    state.paused_from = if to == RunPhase::Paused { Some(from) } else { None };
    state.phase = to;
    state.updated_at = now_millis();
    Ok(TransitionRecord { at: state.updated_at, from, to, reason: reason.map(|s| s.to_string()) })
}

pub fn phase_name(p: RunPhase) -> &'static str {
    use RunPhase::*;
    match p {
        Created => "created",
        Analyzing => "analyzing",
        Proposing => "proposing",
        Reviewing => "reviewing",
        Revising => "revising",
        Synthesizing => "synthesizing",
        PlanReady => "plan_ready",
        AwaitingApproval => "awaiting_approval",
        Executing => "executing",
        Verifying => "verifying",
        Completed => "completed",
        Playing => "playing",
        Paused => "paused",
        Cancelled => "cancelled",
        Failed => "failed",
        BudgetExhausted => "budget_exhausted",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::council::types::MemberRuntimeStatus;
    use std::collections::HashMap;

    fn state_at(phase: RunPhase) -> CouncilState {
        CouncilState {
            run_id: "t".into(),
            phase,
            round: 1,
            paused_from: None,
            member_status: HashMap::from([("a".into(), MemberRuntimeStatus::Idle)]),
            budget_usage: Default::default(),
            tasks: Vec::new(),
            verify_attempts: 0,
            updated_at: 0,
            error: None,
        }
    }

    #[test]
    fn happy_path_transitions() {
        let mut s = state_at(RunPhase::Created);
        for to in [RunPhase::Analyzing, RunPhase::Proposing, RunPhase::Reviewing, RunPhase::Revising, RunPhase::Revising] {
            // Revising→Revising 非法,单独跳过
            if to == RunPhase::Revising && s.phase == RunPhase::Revising {
                let rec = apply_transition(&mut s, RunPhase::Reviewing, None).unwrap();
                assert_eq!(rec.from, RunPhase::Revising);
                continue;
            }
            apply_transition(&mut s, to, None).unwrap();
        }
        for to in [RunPhase::Synthesizing, RunPhase::PlanReady, RunPhase::Executing, RunPhase::Verifying, RunPhase::Completed] {
            apply_transition(&mut s, to, None).unwrap();
        }
        assert!(s.phase.is_terminal());
    }

    #[test]
    fn execution_gate_blocks_orchestrator_paths() {
        // PlanReady 不能跳过用户批准直上 Executing 之外的任何执行态;
        // Verifying 未经 Executing 不可达(不能从 PlanReady 直达)。
        assert!(!can_transition(RunPhase::PlanReady, RunPhase::Verifying));
        assert!(!can_transition(RunPhase::PlanReady, RunPhase::Synthesizing));
        assert!(can_transition(RunPhase::PlanReady, RunPhase::Executing));
        // council-only 交付边存在(用户接受计划,无代码执行)。
        assert!(can_transition(RunPhase::PlanReady, RunPhase::Completed));
    }

    #[test]
    fn paused_can_be_cancelled() {
        let mut s = state_at(RunPhase::Reviewing);
        apply_transition(&mut s, RunPhase::Paused, None).unwrap();
        apply_transition(&mut s, RunPhase::Cancelled, Some("用户取消")).unwrap();
        assert_eq!(s.phase, RunPhase::Cancelled);
    }

    #[test]
    fn terminal_states_frozen() {
        for t in [RunPhase::Completed, RunPhase::Cancelled, RunPhase::Failed, RunPhase::BudgetExhausted] {
            let mut s = state_at(t);
            assert!(apply_transition(&mut s, RunPhase::Paused, None).is_err());
            assert!(apply_transition(&mut s, RunPhase::Analyzing, None).is_err());
        }
    }

    #[test]
    fn pause_resume_round_trip() {
        let mut s = state_at(RunPhase::Reviewing);
        apply_transition(&mut s, RunPhase::Paused, Some("user")).unwrap();
        assert_eq!(s.paused_from, Some(RunPhase::Reviewing));
        // 恢复到别的状态被拒
        assert!(apply_transition(&mut s, RunPhase::Proposing, None).is_err());
        apply_transition(&mut s, RunPhase::Reviewing, Some("resume")).unwrap();
        assert_eq!(s.phase, RunPhase::Reviewing);
        assert_eq!(s.paused_from, None);
    }

    #[test]
    fn revision_loop_allowed() {
        assert!(can_transition(RunPhase::Reviewing, RunPhase::Revising));
        assert!(can_transition(RunPhase::Revising, RunPhase::Reviewing));
        assert!(can_transition(RunPhase::Revising, RunPhase::Synthesizing));
    }
}
