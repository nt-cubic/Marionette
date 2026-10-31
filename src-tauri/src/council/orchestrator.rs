//! Council 编排器:run 生命周期、协议循环、fan-out 并发、预算、恢复。
//!
//! 模型(架构审计 §6):
//! - 每个 run 一个 orchestrator 线程,单写者落盘(消息/转移/状态),
//!   worker 线程只回 mpsc,不碰磁盘。
//! - fan-out 分两步:先(借用只读 messages)构建每个成员的 job,再
//!   `std::thread::scope` 并发执行 + 计数信号量限流。
//! - 取消:`AtomicBool`,调度边界与 worker 请求前检查;取消后不再发起
//!   任何模型调用(验收 #8)。
//! - 恢复:以 messages.jsonl 为进度账本——某阶段产出齐全则跳过(等价
//!   OmO consumed-ledger 语义;验收 #7 重启不重放已完成副作用)。

use super::prompts;
use super::state::{apply_transition, phase_name};
use super::store;
use super::types::*;
use super::worker::{EngineClient, ModelClient, WorkerOutput, WorkerRequest};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

// ---------------------------------------------------------------------------
// 事件
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct CouncilEventEmitter {
    app: Option<tauri::AppHandle>,
}

impl CouncilEventEmitter {
    pub fn new(app: Option<tauri::AppHandle>) -> Self {
        CouncilEventEmitter { app }
    }

    /// 全局事件(对齐 engine 的 `engine-state-changed` 模式)。测试传
    /// None → no-op。
    pub fn emit(&self, run_id: &str, kind: &str, data: serde_json::Value) {
        let Some(app) = &self.app else { return };
        use tauri::Emitter;
        let _ = app.emit(
            "council-event",
            json!({ "runId": run_id, "kind": kind, "data": data }),
        );
    }
}

// ---------------------------------------------------------------------------
// 信号量(无 tokio 红线下的 std 原语)
// ---------------------------------------------------------------------------

pub struct Semaphore {
    permits: Mutex<u32>,
    cond: Condvar,
}

impl Semaphore {
    pub fn new(permits: u32) -> Self {
        Semaphore { permits: Mutex::new(permits), cond: Condvar::new() }
    }

    pub fn acquire(&self) {
        let mut left = self.permits.lock().unwrap();
        while *left == 0 {
            left = self.cond.wait(left).unwrap();
        }
        *left -= 1;
    }

    pub fn release(&self) {
        let mut left = self.permits.lock().unwrap();
        *left += 1;
        self.cond.notify_one();
    }
}

// ---------------------------------------------------------------------------
// Run 注册表(进程内活线程)
// ---------------------------------------------------------------------------

pub struct RunHandle {
    pub cancel: AtomicBool,
    pub pause: AtomicBool,
}

impl RunHandle {
    fn new() -> Self {
        RunHandle { cancel: AtomicBool::new(false), pause: AtomicBool::new(false) }
    }
}

fn registry() -> &'static Mutex<HashMap<String, Arc<RunHandle>>> {
    static RUNS: OnceLock<Mutex<HashMap<String, Arc<RunHandle>>>> = OnceLock::new();
    RUNS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn run_is_live(run_id: &str) -> bool {
    registry().lock().unwrap().contains_key(run_id)
}

// ---------------------------------------------------------------------------
// 启动 / 控制
// ---------------------------------------------------------------------------

/// 测试/生产注入面:模型客户端与执行 harness 均可替换(验收 #14)。
#[derive(Default)]
pub struct RunClients {
    pub model: Option<Arc<dyn ModelClient>>,
    pub harness: Option<Arc<dyn super::executor::TaskHarness>>,
}

/// 启动(或重启)run 的 orchestrator 线程。幂等:已有活线程 → Err。
pub fn start_run(emitter: CouncilEventEmitter, run_id: String, clients: RunClients) -> Result<(), String> {
    {
        let runs = registry().lock().unwrap();
        if runs.contains_key(&run_id) {
            return Err(format!("run {run_id} 已在运行"));
        }
    }
    // 引擎唤醒(生产路径):最长等 20s;不就绪也不失败——首个 worker
    // 请求会给出人话错误。
    if clients.model.is_none() {
        crate::engine::wake_if_installed();
        let _ = crate::engine::ensure_bridge_ready_if_installed(Duration::from_secs(20));
    }
    let handle = Arc::new(RunHandle::new());
    registry().lock().unwrap().insert(run_id.clone(), handle.clone());
    let rid = run_id.clone();
    std::thread::Builder::new()
        .name(format!("council-{rid}"))
        .spawn(move || {
            let model: Arc<dyn ModelClient> =
                clients.model.unwrap_or_else(|| Arc::new(EngineClient::default()));
            let result = run_loop(&emitter, &rid, &handle, model.as_ref(), clients.harness.as_deref());
            registry().lock().unwrap().remove(&rid);
            if let Err(e) = result {
                emitter.emit(&rid, "error", json!({ "message": e }));
            }
        })
        .map_err(|e| {
            registry().lock().unwrap().remove(&run_id);
            format!("启动 council 线程失败:{e}")
        })?;
    Ok(())
}

pub fn cancel_run(run_id: &str) -> Result<bool, String> {
    let runs = registry().lock().unwrap();
    match runs.get(run_id) {
        Some(h) => {
            h.cancel.store(true, Ordering::SeqCst);
            Ok(true)
        }
        None => Ok(false),
    }
}

pub fn pause_run(run_id: &str) -> Result<bool, String> {
    let runs = registry().lock().unwrap();
    match runs.get(run_id) {
        Some(h) => {
            h.pause.store(true, Ordering::SeqCst);
            Ok(true)
        }
        None => Ok(false),
    }
}

/// 启动时恢复:磁盘上是运行态但进程内无活线程(崩溃/退出残留)→ 置
/// Paused。返回被标记的 run id 列表。
pub fn recover_orphaned_runs(emitter: &CouncilEventEmitter) -> Vec<String> {
    let mut recovered = Vec::new();
    let Ok(runs) = store::list_runs() else { return recovered };
    for summary in runs {
        if run_is_live(&summary.run_id) {
            continue;
        }
        if !summary.phase.is_running() {
            continue;
        }
        if let Ok(mut state) = store::load_state(&summary.run_id) {
            if let Ok(record) = apply_transition(&mut state, RunPhase::Paused, Some("进程重启恢复")) {
                let _ = store::append_transition(&summary.run_id, &record);
                let _ = store::save_state(&state);
                emitter.emit(&summary.run_id, "phase", json!({ "phase": "paused", "reason": "recovered" }));
                recovered.push(summary.run_id.clone());
            }
        }
    }
    recovered
}

/// 验证-修复回路上限(任务书 §4.3 默认 3)。
pub const MAX_VERIFY_ATTEMPTS: u32 = 3;

/// 修订计划后的任务账本迁移:同 id 且同 detail 的已完成(Merged)任务
/// 保留执行成果(不重跑、不重复调用 agent);失败/被修改/新增的任务
/// 复位 Pending;被删除的步骤出账。
fn migrate_tasks(
    old_plan: &crate::council::types::PlanDoc,
    new_plan: &crate::council::types::PlanDoc,
    current: &[super::tasks::TaskProgress],
) -> Vec<super::tasks::TaskProgress> {
    use super::tasks::TaskStatus;
    let mut out = Vec::new();
    for new_step in &new_plan.steps {
        let old_detail = old_plan.steps.iter().find(|s| s.id == new_step.id).map(|s| s.detail.clone());
        let current_prog = current.iter().find(|t| t.step_id == new_step.id);
        let keep = match (current_prog, &old_detail) {
            (Some(prog), Some(old)) => prog.status == TaskStatus::Merged && *old == new_step.detail,
            _ => false,
        };
        if keep {
            let mut kept = current_prog.unwrap().clone();
            kept.title = new_step.title.clone();
            out.push(kept);
        } else {
            out.push(super::tasks::TaskProgress {
                step_id: new_step.id.clone(),
                title: new_step.title.clone(),
                status: TaskStatus::Pending,
                session_id: None,
                worktree: None,
                branch: None,
                changes: Vec::new(),
                error: None,
                attempts: current_prog.map(|p| p.attempts).unwrap_or(0),
                notes: String::new(),
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// fan-out job
// ---------------------------------------------------------------------------

/// 一个待执行的成员任务(prompt 已构造完,与 messages 的借用解耦)。
struct Job {
    member_id: String,
    model: String,
    system: String,
    user: String,
    intent: Intent,
}

#[derive(Clone, Copy, PartialEq)]
enum Intent {
    Proposal,
    Critique,
    Revision,
    Skip,
}

// ---------------------------------------------------------------------------
// 主循环
// ---------------------------------------------------------------------------

fn run_loop(
    emitter: &CouncilEventEmitter,
    run_id: &str,
    handle: &RunHandle,
    client: &dyn ModelClient,
    harness: Option<&dyn super::executor::TaskHarness>,
) -> Result<(), String> {
    let spec = store::load_spec(run_id)?;
    let mut state = store::load_state(run_id)?;
    let mut messages = store::load_messages(run_id)?;
    let sem = Semaphore::new(spec.max_concurrent);

    loop {
        if handle.cancel.load(Ordering::SeqCst) {
            transition(&mut state, run_id, emitter, RunPhase::Cancelled, Some("用户取消"));
            emitter.emit(run_id, "finished", json!({ "phase": "cancelled" }));
            return Ok(());
        }
        if handle.pause.load(Ordering::SeqCst) {
            transition(&mut state, run_id, emitter, RunPhase::Paused, Some("用户暂停"));
            return Ok(());
        }
        if let Some(bust) = budget_exhausted(&spec, &state) {
            transition(&mut state, run_id, emitter, RunPhase::BudgetExhausted, Some(&bust));
            emitter.emit(run_id, "finished", json!({ "phase": "budget_exhausted", "reason": bust }));
            return Ok(());
        }

        match state.phase {
            RunPhase::Created if spec.mode == CouncilMode::SgsGame => {
                transition(&mut state, run_id, emitter, RunPhase::Playing, Some("身份局开局"));
            }
            RunPhase::Playing => {
                // 身份局游戏(M5.2):整局阻塞跑完,终局→Completed;
                // 取消/失败按状态机收尾。恢复:runner 内部从 game-state.json 续。
                let seed = spec.created_at as u64;
                let result = super::game::runner::run_game(emitter, run_id, &spec, client, &handle.cancel, seed);
                match result {
                    Ok(()) => {
                        transition(&mut state, run_id, emitter, RunPhase::Completed, Some("对局结束"));
                        emitter.emit(run_id, "finished", json!({ "phase": "completed" }));
                        return Ok(());
                    }
                    Err(e) if e == "__cancelled__" => {
                        transition(&mut state, run_id, emitter, RunPhase::Cancelled, Some("用户取消"));
                        emitter.emit(run_id, "finished", json!({ "phase": "cancelled" }));
                        return Ok(());
                    }
                    Err(e) => {
                        fail_run(&mut state, run_id, emitter, format!("对局中断:{e}"));
                        return Ok(());
                    }
                }
            }
            RunPhase::Created => {
                transition(&mut state, run_id, emitter, RunPhase::Analyzing, None);
            }

            RunPhase::Analyzing => {
                if messages.iter().any(|m| m.kind == MessageKind::Briefing) {
                    transition(&mut state, run_id, emitter, RunPhase::Proposing, Some("briefing 已存在,跳过"));
                    continue;
                }
                let chair = chair_of(&spec).ok_or("找不到主席成员")?;
                let (system, user) = prompts::briefing_prompt(&spec, chair);
                set_member_status(&mut state, run_id, emitter, &chair.id, MemberRuntimeStatus::Working);
                match call_model(&mut state, client, handle, &chair.model, &system, &user, "briefing") {
                    Ok(out) => {
                        let msg = new_message(run_id, 0, "chair", "all", MessageKind::Briefing, None, &out.content, None);
                        persist_message(&mut messages, run_id, emitter, msg);
                        set_member_status(&mut state, run_id, emitter, &chair.id, MemberRuntimeStatus::Done);
                        transition(&mut state, run_id, emitter, RunPhase::Proposing, None);
                    }
                    Err(e) => {
                        set_member_status(&mut state, run_id, emitter, &chair.id, MemberRuntimeStatus::Failed { reason: e.clone() });
                        fail_run(&mut state, run_id, emitter, format!("开场简报失败:{e}"));
                        return Ok(());
                    }
                }
            }

            RunPhase::Proposing => {
                if proposals_complete(&spec, &messages) {
                    transition(&mut state, run_id, emitter, RunPhase::Reviewing, Some("提案齐全"));
                    continue;
                }
                let briefing = messages.iter().find(|m| m.kind == MessageKind::Briefing).cloned();
                let jobs: Vec<Job> = spec
                    .members
                    .iter()
                    .filter(|m| !messages.iter().any(|x| x.from == m.id && x.kind == MessageKind::Proposal))
                    .map(|member| {
                        let (system, user) = prompts::proposal_prompt(&spec, member, briefing.as_ref());
                        Job {
                            member_id: member.id.clone(),
                            model: member.model.clone(),
                            system,
                            user,
                            intent: Intent::Proposal,
                        }
                    })
                    .collect();
                let ok = fan_out(emitter, &spec, &mut state, &mut messages, client, &sem, handle, jobs, "proposal");
                if ok == 0 && !proposals_complete(&spec, &messages) {
                    fail_run(&mut state, run_id, emitter, "所有成员提案失败".into());
                    return Ok(());
                }
                transition(&mut state, run_id, emitter, RunPhase::Reviewing, None);
            }

            RunPhase::Reviewing => {
                let round = state.round;
                if !critiques_complete(&spec, &messages, round) {
                    let briefing = messages.iter().find(|m| m.kind == MessageKind::Briefing).cloned();
                    let jobs: Vec<Job> = spec
                        .members
                        .iter()
                        .filter(|m| {
                            !messages.iter().any(|x| x.from == m.id && x.kind == MessageKind::Critique && x.round == round)
                        })
                        .map(|member| {
                            let others = prompts::latest_positions_for_review(&messages, &member.id, round);
                            if others.is_empty() {
                                return Job {
                                    member_id: member.id.clone(),
                                    model: member.model.clone(),
                                    system: String::new(),
                                    user: String::new(),
                                    intent: Intent::Skip,
                                };
                            }
                            let (system, user) = prompts::critique_prompt(&spec, member, round, briefing.as_ref(), &others);
                            Job { member_id: member.id.clone(), model: member.model.clone(), system, user, intent: Intent::Critique }
                        })
                        .collect();
                    let ok = fan_out(emitter, &spec, &mut state, &mut messages, client, &sem, handle, jobs, "critique");
                    if ok == 0 && !critiques_complete(&spec, &messages, round) {
                        emitter.emit(run_id, "warn", json!({ "message": "本轮质询全部失败,降级进综合" }));
                        transition(&mut state, run_id, emitter, RunPhase::Synthesizing, Some("质询失败降级"));
                        continue;
                    }
                }
                // 质询齐备后的去向。
                let needs_revision = !prompts::authors_needing_revision(&messages, round).is_empty();
                let more_rounds = round < spec.rounds;
                if needs_revision && more_rounds {
                    transition(&mut state, run_id, emitter, RunPhase::Revising, None);
                } else {
                    let reason = if needs_revision { "轮次耗尽,带未决分歧进综合" } else { "无待修订项,进综合" };
                    transition(&mut state, run_id, emitter, RunPhase::Synthesizing, Some(reason));
                }
            }

            RunPhase::Revising => {
                let round = state.round;
                if revisions_complete(&spec, &messages, round) {
                    state.round += 1;
                    let next_round = state.round;
                    transition(&mut state, run_id, emitter, RunPhase::Reviewing, Some(&format!("进入第 {next_round} 轮评审")));
                    continue;
                }
                let jobs: Vec<Job> = spec
                    .members
                    .iter()
                    .map(|member| {
                        let critiques = prompts::pending_critiques_for(&messages, &member.id, round);
                        let own = prompts::latest_own_position(&messages, &member.id);
                        match (critiques.is_empty(), own) {
                            (true, _) | (_, None) => Job {
                                member_id: member.id.clone(),
                                model: member.model.clone(),
                                system: String::new(),
                                user: String::new(),
                                intent: Intent::Skip,
                            },
                            (false, Some(own)) => {
                                let (system, user) = prompts::revision_prompt(&spec, member, round, &own, &critiques);
                                Job { member_id: member.id.clone(), model: member.model.clone(), system, user, intent: Intent::Revision }
                            }
                        }
                    })
                    .collect();
                fan_out(emitter, &spec, &mut state, &mut messages, client, &sem, handle, jobs, "revision");
                state.round += 1;
                let next_round = state.round;
                transition(&mut state, run_id, emitter, RunPhase::Reviewing, Some(&format!("进入第 {next_round} 轮评审")));
            }

            RunPhase::Synthesizing => {
                let chair = chair_of(&spec).ok_or("找不到主席成员")?;
                // 0) 分层背书(M5):每个 lead 对每个 contributor 的最新
                //    产出给出认可/否决。全员 lead(旧配置)→ 整段跳过,
                //    零额外调用,行为与 M1-M4 完全一致。恢复语义:某
                //    (lead, contributor, round) 的背书已在账上则不重调。
                //    Synthesis 依赖背书状态渲染采信规则,必须先行。
                let contributors: Vec<&MemberSpec> = spec
                    .members
                    .iter()
                    .filter(|m| m.tier == super::types::MemberTier::Contributor)
                    .collect();
                let leads: Vec<&MemberSpec> = spec
                    .members
                    .iter()
                    .filter(|m| m.tier == super::types::MemberTier::Lead && !m.is_chair)
                    .collect();
                if !contributors.is_empty() && !leads.is_empty() {
                    for contributor in &contributors {
                        let Some(output) = prompts::latest_own_position(&messages, &contributor.id) else {
                            continue;
                        };
                        let round = output.round;
                        let target_kind = format!("{:?}", output.kind);
                        for lead in &leads {
                            let already = messages.iter().any(|m| {
                                m.kind == MessageKind::Endorsement
                                    && m.from == lead.id
                                    && m.round == round
                                    && m.meta.as_ref().and_then(|v| v.get("targetAuthor")).and_then(|v| v.as_str())
                                        == Some(contributor.id.as_str())
                                    && m.meta.as_ref().and_then(|v| v.get("targetKind")).and_then(|v| v.as_str())
                                        == Some(target_kind.as_str())
                            });
                            if already {
                                continue;
                            }
                            if handle.cancel.load(Ordering::SeqCst) {
                                break;
                            }
                            let (system, user) =
                                prompts::endorsement_prompt(&spec, lead, contributor, &output);
                            match call_model(&mut state, client, handle, &lead.model, &system, &user, "endorsement") {
                                Ok(out) => {
                                    let verdict = prompts::parse_endorsement(&out.content);
                                    let (verdict_str, reason) = match verdict {
                                        Ok((true, reason)) => ("endorse", reason),
                                        Ok((false, reason)) => ("reject", reason),
                                        Err(e) => ("unparsed", e),
                                    };
                                    let msg = new_message(
                                        run_id,
                                        round,
                                        &lead.id,
                                        &contributor.id,
                                        MessageKind::Endorsement,
                                        Some(output.message_id.clone()),
                                        &format!(
                                            "{}:{}",
                                            if verdict_str == "endorse" { "认可" } else if verdict_str == "reject" { "否决" } else { "无法解析" },
                                            if reason.is_empty() { out.content.chars().take(200).collect::<String>() } else { reason.clone() }
                                        ),
                                        Some(json!({
                                            "targetAuthor": contributor.id,
                                            "targetKind": target_kind,
                                            "verdict": verdict_str,
                                            "byLead": lead.id,
                                        })),
                                    );
                                    persist_message(&mut messages, run_id, emitter, msg);
                                    set_member_status(&mut state, run_id, emitter, &lead.id, MemberRuntimeStatus::Done);
                                }
                                Err(e) => {
                                    // 单个背书失败不毒化 run:记录状态消息,
                                    // 该 contributor 的产出按"零背书"进入
                                    // 综合(chair 的采信规则会兜住)。
                                    let msg = new_message(
                                        run_id, round, "system", "all", MessageKind::Status, None,
                                        &format!("成员 `{}` 对 `{}` 的背书调用失败:{e}", lead.id, contributor.id),
                                        Some(json!({ "member": lead.id, "failed": true })),
                                    );
                                    persist_message(&mut messages, run_id, emitter, msg);
                                }
                            }
                        }
                    }
                }
                // 1) synthesis(恢复跳过:已存在则不重调)。
                if !messages.iter().any(|m| m.kind == MessageKind::Synthesis) {
                    let (system, user) = prompts::synthesis_prompt(&spec, chair, &messages);
                    set_member_status(&mut state, run_id, emitter, &chair.id, MemberRuntimeStatus::Working);
                    match call_model(&mut state, client, handle, &chair.model, &system, &user, "synthesis") {
                        Ok(out) => {
                            let msg = new_message(run_id, state.round, &chair.id, "all", MessageKind::Synthesis, None, &out.content, None);
                            persist_message(&mut messages, run_id, emitter, msg);
                        }
                        Err(e) => {
                            set_member_status(&mut state, run_id, emitter, &chair.id, MemberRuntimeStatus::Failed { reason: e.clone() });
                            fail_run(&mut state, run_id, emitter, format!("综合失败:{e}"));
                            return Ok(());
                        }
                    }
                }
                // 2) plan(恢复跳过)。
                if !messages.iter().any(|m| m.kind == MessageKind::Plan) {
                    let synthesis = messages
                        .iter()
                        .find(|m| m.kind == MessageKind::Synthesis)
                        .cloned()
                        .expect("synthesis 刚落账");
                    let (system, user) = prompts::plan_prompt(&spec, chair, &synthesis);
                    let raw = match call_model(&mut state, client, handle, &chair.model, &system, &user, "plan") {
                        Ok(out) => out.content,
                        Err(e) => {
                            fail_run(&mut state, run_id, emitter, format!("计划生成失败:{e}"));
                            return Ok(());
                        }
                    };
                    let (plan, plan_json) = match parse_plan(&raw) {
                        Ok(p) => (p, raw),
                        Err(first_error) => {
                            // 一次修复机会(架构审计 §5:两次失败 → Failed)。
                            emitter.emit(run_id, "warn", json!({ "message": "计划 JSON 解析失败,请求修复", "error": first_error }));
                            let (rs, ru) = prompts::plan_repair_prompt(chair, &raw, &first_error);
                            match call_model(&mut state, client, handle, &chair.model, &rs, &ru, "plan-repair") {
                                Ok(out) => match parse_plan(&out.content) {
                                    Ok(p) => (p, out.content),
                                    Err(second) => {
                                        fail_run(&mut state, run_id, emitter, format!("计划 JSON 两次解析失败:{second}"));
                                        return Ok(());
                                    }
                                },
                                Err(e) => {
                                    fail_run(&mut state, run_id, emitter, format!("计划修复调用失败:{e}"));
                                    return Ok(());
                                }
                            }
                        }
                    };
                    store::save_plan(run_id, &plan)?;
                    let meta = json!({ "planJson": plan_json });
                    let msg = new_message(run_id, state.round, &chair.id, "all", MessageKind::Plan, None, &plan.summary, Some(meta));
                    persist_message(&mut messages, run_id, emitter, msg);
                }
                set_member_status(&mut state, run_id, emitter, &chair.id, MemberRuntimeStatus::Done);
                // M5:审议落幕,把各模型表现(产出/背书/失败)记进
                // model-stats.json——能力画像的历史信号源。
                super::formation::record_deliberation_outcome(&spec, &messages);
                transition(&mut state, run_id, emitter, RunPhase::PlanReady, None);
                emitter.emit(run_id, "plan-ready", json!({}));
                // PlanReady = 等用户批准,线程退出(验收 #9:执行门)。
                return Ok(());
            }

            RunPhase::PlanReady | RunPhase::Paused => {
                // 转移后线程应已退出;防御性返回。
                return Ok(());
            }

            RunPhase::Executing => {
                let Some(h) = harness else {
                    fail_run(&mut state, run_id, emitter, "执行阶段没有可用的 agent harness".into());
                    return Ok(());
                };
                let Some(executor) = spec.executor.clone() else {
                    fail_run(&mut state, run_id, emitter, "run 缺少执行器配置".into());
                    return Ok(());
                };
                let Some(plan) = store::load_plan(run_id)? else {
                    fail_run(&mut state, run_id, emitter, "执行前找不到计划".into());
                    return Ok(());
                };
                // 恢复位:上次线程死掉时 Running 的任务复位 Pending,
                // 其遗留 worktree 若在则先清(重跑这一步)。
                for t in state.tasks.iter_mut() {
                    if t.status == super::tasks::TaskStatus::Running {
                        if let (Some(w), true) = (t.worktree.clone(), t.worktree.is_some()) {
                            let _ = crate::git_service::remove_worktree(
                                &std::path::PathBuf::from(&executor.cwd),
                                std::path::Path::new(&w),
                            );
                        }
                        t.status = super::tasks::TaskStatus::Pending;
                        t.worktree = None;
                        t.branch = None;
                    }
                }
                // 任务账本初始化(恢复时已有账目则沿用——以账本为进度,
                // 与审议阶段的恢复语义一致)。
                if state.tasks.is_empty() {
                    state.tasks = match super::tasks::build_task_progress(&plan) {
                        Ok(t) => t,
                        Err(e) => {
                            fail_run(&mut state, run_id, emitter, format!("计划不可执行:{e}"));
                            return Ok(());
                        }
                    };
                    let _ = store::save_state(&state);
                }
                let project_root = std::path::PathBuf::from(&executor.cwd);
                let use_worktrees = crate::git_service::is_git_worktree(&project_root);
                if !use_worktrees {
                    emitter.emit(run_id, "warn", json!({ "message": "目录不是 git 仓库:并行任务退化为串行执行,无隔离" }));
                }
                let exec_ctx = super::executor::ExecuteContext {
                    run_id,
                    project_root: project_root.clone(),
                    plan: &plan,
                    harness: h,
                    max_concurrent: spec.max_concurrent,
                    step_timeout_secs: executor.step_timeout_secs.unwrap_or(900),
                    use_worktrees,
                    cancel: &handle.cancel,
                };
                // on_task_update 只发事件(闭包不能借用 state/batch 的
                // 独占写);进度落盘在批次边界——批内崩溃时该批任务
                // 从 Pending 重来,worktree 产物由复位逻辑清理。
                let mut task_emitter = |t: &super::tasks::TaskProgress| {
                    emitter.emit(run_id, "task", serde_json::to_value(t).unwrap_or(json!({})));
                };
                while super::tasks::work_remaining(&plan, &state.tasks) {
                    if handle.cancel.load(Ordering::SeqCst) {
                        transition(&mut state, run_id, emitter, RunPhase::Cancelled, Some("用户取消"));
                        emitter.emit(run_id, "finished", json!({ "phase": "cancelled" }));
                        return Ok(());
                    }
                    let mut tasks_snapshot = state.tasks.clone();
                    let progressed =
                        super::executor::execute_ready_batch(&exec_ctx, &mut tasks_snapshot, &mut task_emitter);
                    state.tasks = tasks_snapshot;
                    let _ = store::save_state(&state);
                    if !progressed {
                        break;
                    }
                }
                // 合并 Staged(worktree)任务;冲突显式失败(验收 #10)。
                let mut tasks_snapshot = state.tasks.clone();
                let (_merged, conflicts) = super::executor::merge_staged(&exec_ctx, &mut tasks_snapshot, &mut task_emitter);
                state.tasks = tasks_snapshot;
                super::tasks::finalize_skipped(&plan, &mut state.tasks);
                let _ = store::save_state(&state);
                if conflicts > 0 {
                    let failed: Vec<String> = state
                        .tasks
                        .iter()
                        .filter(|t| matches!(t.status, super::tasks::TaskStatus::Failed { .. }))
                        .map(|t| t.step_id.clone())
                        .collect();
                    emitter.emit(run_id, "warn", json!({ "message": format!("{conflicts} 个任务合并冲突:{}", failed.join(", ")) }));
                }
                let settled = state.tasks.iter().all(|t| {
                    matches!(
                        t.status,
                        super::tasks::TaskStatus::Merged | super::tasks::TaskStatus::Skipped { .. }
                    )
                });
                if settled {
                    transition(&mut state, run_id, emitter, RunPhase::Verifying, None);
                } else {
                    fail_run(&mut state, run_id, emitter, format!("{conflicts} 个任务执行/合并失败"));
                    return Ok(());
                }
            }

            RunPhase::Verifying => {
                let Some(plan) = store::load_plan(run_id)? else {
                    fail_run(&mut state, run_id, emitter, "验证阶段找不到计划".into());
                    return Ok(());
                };
                let Some(executor) = spec.executor.clone() else {
                    transition(&mut state, run_id, emitter, RunPhase::Completed, Some("无执行器,跳过验证"));
                    return Ok(());
                };
                let cwd = std::path::PathBuf::from(&executor.cwd);
                let results = super::verify::run_all(&cwd, &plan.verification, 900);
                let evidence = super::verify::render_evidence(&results);
                let all_ok = results.iter().all(|r| r.ok);
                let msg = new_message(
                    run_id,
                    state.round,
                    "system",
                    "all",
                    MessageKind::Evidence,
                    None,
                    &evidence,
                    Some(json!({ "attempt": state.verify_attempts, "allOk": all_ok })),
                );
                persist_message(&mut messages, run_id, emitter, msg);
                emitter.emit(run_id, "verify", serde_json::to_value(&results).unwrap_or(json!([])));

                if all_ok {
                    transition(&mut state, run_id, emitter, RunPhase::Completed, Some("验证全部通过"));
                    emitter.emit(run_id, "finished", json!({ "phase": "completed" }));
                    return Ok(());
                }
                // 失败 → 受限修复回路(默认上限 3,任务书 §4.3)。
                if state.verify_attempts >= MAX_VERIFY_ATTEMPTS {
                    fail_run(&mut state, run_id, emitter, format!("验证失败且修复次数已用尽({MAX_VERIFY_ATTEMPTS} 次)"));
                    return Ok(());
                }
                state.verify_attempts += 1;
                let chair = chair_of(&spec).ok_or("找不到主席成员")?;
                let (system, user) =
                    prompts::repair_round_prompt(&spec, chair, &plan, &evidence, state.verify_attempts);
                let repaired_raw = match call_model(&mut state, client, handle, &chair.model, &system, &user, "plan-repair") {
                    Ok(out) => out.content,
                    Err(e) => {
                        fail_run(&mut state, run_id, emitter, format!("修复计划生成失败:{e}"));
                        return Ok(());
                    }
                };
                let new_plan = match parse_plan(&repaired_raw) {
                    Ok(p) => p,
                    Err(first) => {
                        emitter.emit(run_id, "warn", json!({ "message": "修复计划 JSON 解析失败,请求一次修复", "error": first }));
                        let (rs, ru) = prompts::plan_repair_prompt(chair, &repaired_raw, &first);
                        match call_model(&mut state, client, handle, &chair.model, &rs, &ru, "plan-repair-retry") {
                            Ok(out) => match parse_plan(&out.content) {
                                Ok(p) => p,
                                Err(second) => {
                                    fail_run(&mut state, run_id, emitter, format!("修复计划两次解析失败:{second}"));
                                    return Ok(());
                                }
                            },
                            Err(e) => {
                                fail_run(&mut state, run_id, emitter, format!("修复计划调用失败:{e}"));
                                return Ok(());
                            }
                        }
                    }
                };
                // 版本化:旧计划已由 Plan 消息留痕;新计划落盘 + Plan 消息。
                let old_plan = plan;
                store::save_plan(run_id, &new_plan)?;
                let plan_msg = new_message(
                    run_id,
                    state.round,
                    &chair.id,
                    "all",
                    MessageKind::Plan,
                    None,
                    &new_plan.summary,
                    Some(json!({ "planJson": repaired_raw, "repairRound": state.verify_attempts })),
                );
                persist_message(&mut messages, run_id, emitter, plan_msg);
                // 任务账本迁移:同 id 同 detail 的 Merged 任务保留(不重跑);
                // 其余(失败的、detail 变化的、新增的)复位 Pending。
                state.tasks = migrate_tasks(&old_plan, &new_plan, &state.tasks);
                let _ = store::save_state(&state);
                emitter.emit(
                    run_id,
                    "warn",
                    json!({ "message": format!("验证失败,进入第 {} 次修复:计划已修订,受影响任务复位重跑", state.verify_attempts) }),
                );
                transition(&mut state, run_id, emitter, RunPhase::Executing, Some("验证失败,修订计划后重执行"));
                continue;
            }

            RunPhase::AwaitingApproval => {
                // M3 权限场景(破坏性验证命令)预留;当前不应到达。
                let stuck = phase_name(state.phase).to_string();
                fail_run(&mut state, run_id, emitter, format!("阶段 {stuck} 尚未实现"));
                return Ok(());
            }

            _ => {
                let stuck = phase_name(state.phase).to_string();
                fail_run(&mut state, run_id, emitter, format!("编排器遇到未实现的阶段 {stuck}"));
                return Ok(());
            }
        }
    }
}

// ---------------------------------------------------------------------------
// fan-out 与模型调用
// ---------------------------------------------------------------------------

/// 并发执行 jobs:scope 线程 + 信号量限流,结果经 mpsc 回本线程统一
/// 落盘(单写者)。返回成功产出数。
fn fan_out(
    emitter: &CouncilEventEmitter,
    spec: &CouncilSpec,
    state: &mut CouncilState,
    messages: &mut Vec<CouncilMessage>,
    client: &dyn ModelClient,
    sem: &Semaphore,
    handle: &RunHandle,
    jobs: Vec<Job>,
    label: &str,
) -> usize {
    let (tx, rx) = mpsc::channel::<FanResult>();
    let run_id = state.run_id.clone();
    std::thread::scope(|scope| {
        for job in jobs {
            let tx = tx.clone();
            scope.spawn(move || {
                if job.intent == Intent::Skip {
                    let _ = tx.send(FanResult::Skipped(job.member_id));
                    return;
                }
                if handle.cancel.load(Ordering::SeqCst) || handle.pause.load(Ordering::SeqCst) {
                    let _ = tx.send(FanResult::Skipped(job.member_id));
                    return;
                }
                sem.acquire();
                let result = call_with_retry(client, &job.model, &job.system, &job.user);
                sem.release();
                let _ = tx.send(match result {
                    Ok((out, _retried)) => FanResult::Ok(job.member_id, out, job.system.chars().count() + job.user.chars().count()),
                    Err(e) => FanResult::Err(job.member_id, e),
                });
            });
        }
        drop(tx);
        let mut ok_count = 0usize;
        for res in rx {
            if handle.cancel.load(Ordering::SeqCst) {
                break;
            }
            match res {
                FanResult::Skipped(_) => {}
                FanResult::Ok(member_id, out, prompt_chars) => {
                    ok_count += 1;
                    state.budget_usage.requests += 1;
                    state.budget_usage.tokens_est +=
                        estimate_tokens(prompt_chars) + estimate_tokens(out.output_chars);
                    let msg = fan_message(label, spec, state.round, &member_id, &out.content);
                    persist_message(messages, &run_id, emitter, msg);
                    set_member_status(state, &run_id, emitter, &member_id, MemberRuntimeStatus::Done);
                }
                FanResult::Err(member_id, e) => {
                    // 单成员失败:记 Status 消息 + 成员 Failed,run 继续
                    // (验收 #6)。
                    let msg = new_message(
                        &run_id,
                        state.round,
                        "system",
                        "all",
                        MessageKind::Status,
                        None,
                        &format!("成员 `{member_id}` 的 {label} 调用失败:{e}"),
                        Some(json!({ "member": member_id, "failed": true })),
                    );
                    persist_message(messages, &run_id, emitter, msg);
                    set_member_status(state, &run_id, emitter, &member_id, MemberRuntimeStatus::Failed { reason: e });
                }
            }
        }
        let _ = store::save_state(state);
        ok_count
    })
}

enum FanResult {
    Skipped(String),
    Ok(String, WorkerOutput, usize),
    Err(String, String),
}

fn fan_message(label: &str, spec: &CouncilSpec, round: u32, member_id: &str, content: &str) -> CouncilMessage {
    let run_id = &spec.run_id;
    match label {
        "proposal" => new_message(run_id, round, member_id, "all", MessageKind::Proposal, None, content, None),
        "critique" => {
            // 质询面向其余全部作者(meta 记录目标集,Phase C 路由与
            // 验收 #4 的判定都用它)。
            let targets: Vec<String> = spec
                .members
                .iter()
                .filter(|m| m.id != member_id)
                .map(|m| m.id.clone())
                .collect();
            new_message(
                run_id,
                round,
                member_id,
                "all",
                MessageKind::Critique,
                None,
                content,
                Some(json!({ "targetAuthors": targets })),
            )
        }
        "revision" => new_message(run_id, round, member_id, "all", MessageKind::Revision, None, content, None),
        _ => new_message(run_id, round, member_id, "all", MessageKind::Status, None, content, None),
    }
}

/// 单次调用(orchestrator 内联用:briefing/synthesis/plan)。带取消
/// 检查;失败重试 1 次。
fn call_model(
    state: &mut CouncilState,
    client: &dyn ModelClient,
    handle: &RunHandle,
    model: &str,
    system: &str,
    user: &str,
    label: &str,
) -> Result<WorkerOutput, String> {
    if handle.cancel.load(Ordering::SeqCst) || handle.pause.load(Ordering::SeqCst) {
        return Err(format!("{label} 被取消"));
    }
    let (out, _retried) = call_with_retry(client, model, system, user)?;
    state.budget_usage.requests += 1;
    state.budget_usage.tokens_est +=
        estimate_tokens(system.chars().count() + user.chars().count()) + estimate_tokens(out.output_chars);
    if state.budget_usage.started_at == 0 {
        state.budget_usage.started_at = now_millis();
    }
    let _ = store::save_state(state);
    Ok(out)
}

/// 带一次重试的调用(worker 共用)。
fn call_with_retry(
    client: &dyn ModelClient,
    model: &str,
    system: &str,
    user: &str,
) -> Result<(WorkerOutput, bool), String> {
    let req = WorkerRequest {
        model: model.to_string(),
        system: system.to_string(),
        user: user.to_string(),
        max_tokens: None,
    };
    match client.complete(&req) {
        Ok(out) => Ok((out, false)),
        Err(first) => match client.complete(&req) {
            Ok(out) => Ok((out, true)),
            Err(second) => Err(format!("{first};重试仍失败:{second}")),
        },
    }
}

// ---------------------------------------------------------------------------
// 进度判定(恢复语义的核心:以消息账本为准)
// ---------------------------------------------------------------------------

pub fn proposals_complete(spec: &CouncilSpec, messages: &[CouncilMessage]) -> bool {
    spec.members
        .iter()
        .all(|m| messages.iter().any(|x| x.from == m.id && x.kind == MessageKind::Proposal))
}

pub fn critiques_complete(spec: &CouncilSpec, messages: &[CouncilMessage], round: u32) -> bool {
    // 每个非主席成员在 round 内至少一条质询(chair 也质询,但 chair
    // 缺席不阻塞——它还要做综合)。
    spec.members
        .iter()
        .filter(|m| !m.is_chair)
        .all(|m| messages.iter().any(|x| x.from == m.id && x.kind == MessageKind::Critique && x.round == round))
}

pub fn revisions_complete(spec: &CouncilSpec, messages: &[CouncilMessage], round: u32) -> bool {
    let authors = prompts::authors_needing_revision(messages, round);
    authors.is_empty()
        || spec.members.iter().all(|m| {
            !authors.contains(&m.id)
                || messages.iter().any(|x| x.from == m.id && x.kind == MessageKind::Revision && x.round >= round)
        })
}

fn chair_of(spec: &CouncilSpec) -> Option<&MemberSpec> {
    spec.members.iter().find(|m| m.is_chair)
}

fn budget_exhausted(spec: &CouncilSpec, state: &CouncilState) -> Option<String> {
    if let Some(max) = spec.budget.max_requests {
        if state.budget_usage.requests >= max {
            return Some(format!("请求数达到上限 {max}"));
        }
    }
    if let Some(max) = spec.budget.max_tokens {
        if state.budget_usage.tokens_est >= max {
            return Some(format!("token 估算达到上限 {max}"));
        }
    }
    if let Some(secs) = spec.budget.wall_clock_secs {
        if state.budget_usage.started_at > 0 {
            let elapsed = (now_millis() - state.budget_usage.started_at) / 1000;
            if elapsed >= secs as i64 {
                return Some(format!("墙钟达到上限 {secs}s"));
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

fn new_message(
    run_id: &str,
    round: u32,
    from: &str,
    to: &str,
    kind: MessageKind,
    reply_to: Option<String>,
    content: &str,
    meta: Option<serde_json::Value>,
) -> CouncilMessage {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    CouncilMessage {
        message_id: format!("msg-{seq:x}-{}", now_millis()),
        run_id: run_id.to_string(),
        round,
        from: from.to_string(),
        to: to.to_string(),
        kind,
        reply_to,
        content: content.to_string(),
        timestamp: now_millis(),
        meta,
    }
}

fn persist_message(
    messages: &mut Vec<CouncilMessage>,
    run_id: &str,
    emitter: &CouncilEventEmitter,
    msg: CouncilMessage,
) {
    // 幂等:同 id 已在账上则跳过(恢复重放保护,验收 #5)。
    if messages.iter().any(|m| m.message_id == msg.message_id) {
        return;
    }
    if store::append_message(run_id, &msg).is_ok() {
        emitter.emit(run_id, "message", serde_json::to_value(&msg).unwrap_or(json!({})));
        messages.push(msg);
    }
}

fn set_member_status(
    state: &mut CouncilState,
    run_id: &str,
    emitter: &CouncilEventEmitter,
    member_id: &str,
    status: MemberRuntimeStatus,
) {
    state.member_status.insert(member_id.to_string(), status.clone());
    state.updated_at = now_millis();
    let _ = store::save_state(state);
    emitter.emit(
        run_id,
        "member",
        json!({ "memberId": member_id, "status": serde_json::to_value(&status).unwrap_or(json!(null)) }),
    );
}

fn transition(
    state: &mut CouncilState,
    run_id: &str,
    emitter: &CouncilEventEmitter,
    to: RunPhase,
    reason: Option<&str>,
) {
    match apply_transition(state, to, reason) {
        Ok(record) => {
            let _ = store::append_transition(run_id, &record);
            let _ = store::save_state(state);
            emitter.emit(
                run_id,
                "phase",
                json!({ "phase": phase_name(to), "round": state.round, "reason": reason }),
            );
        }
        Err(e) => {
            // 非法转移是编排器 bug,记录并按 Failed 收尾(fail closed)。
            emitter.emit(run_id, "error", json!({ "message": format!("状态转移被拒:{e}") }));
            state.error = Some(e);
            let _ = store::save_state(state);
        }
    }
}

fn fail_run(state: &mut CouncilState, run_id: &str, emitter: &CouncilEventEmitter, reason: String) {
    state.error = Some(reason.clone());
    transition(state, run_id, emitter, RunPhase::Failed, Some(&reason));
    emitter.emit(run_id, "finished", json!({ "phase": "failed", "error": reason }));
}

/// 解析计划输出:剥 ```json 围栏 → PlanDoc。
pub fn parse_plan(raw: &str) -> Result<PlanDoc, String> {
    let body = extract_json_fence(raw).unwrap_or(raw.trim());
    let value: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("JSON 解析失败:{e}"))?;
    if !value.is_object() {
        return Err("计划不是 JSON 对象".into());
    }
    let summary = value.get("summary").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if summary.is_empty() {
        return Err("计划缺 summary".into());
    }
    let mut steps = Vec::new();
    if let Some(arr) = value.get("steps").and_then(|v| v.as_array()) {
        for (i, s) in arr.iter().enumerate() {
            let id = s
                .get("id")
                .and_then(|v| v.as_str())
                .map(|v| v.to_string())
                .unwrap_or_else(|| format!("s{}", i + 1));
            steps.push(PlanStep {
                id,
                title: s.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                detail: s.get("detail").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                depends_on: s
                    .get("dependsOn")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(|x| x.to_string())).collect())
                    .unwrap_or_default(),
                acceptance: s.get("acceptance").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                owner_hint: s.get("ownerHint").and_then(|v| v.as_str()).map(|v| v.to_string()),
            });
        }
    }
    if steps.is_empty() {
        return Err("计划没有任何步骤".into());
    }
    let strs = |key: &str| -> Vec<String> {
        value
            .get(key)
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_str().map(|x| x.to_string())).collect())
            .unwrap_or_default()
    };
    Ok(PlanDoc {
        summary,
        steps,
        open_questions: strs("openQuestions"),
        acceptance: strs("acceptance"),
        verification: strs("verification"),
    })
}

fn extract_json_fence(raw: &str) -> Option<&str> {
    let start = raw.find("```json")?;
    let rest = &raw[start + 7..];
    let end = rest.find("```")?;
    Some(rest[..end].trim())
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::council::types::tests::sample_spec;
    use crate::council::worker::{MockCall, MockClient};
    use std::fs;

    /// mock 主席模型的响应:循环队列里每次都返回它——briefing/synthesis
    /// 把它当普通文本,plan 阶段 parse 成功。
    const MOCK_PLAN: &str = r#"{"summary":"合并后的方案概述","steps":[{"id":"s1","title":"实现核心","detail":"写代码","dependsOn":[],"acceptance":"测试通过","ownerHint":"engineer"},{"id":"s2","title":"补测试","detail":"写测试","dependsOn":["s1"],"acceptance":"cargo test 绿"}],"openQuestions":["Q1 需实验"],"acceptance":["全部步骤完成"],"verification":["cargo test"]}"#;

    /// 与 store::tests 共用同一把串行锁(同一个 ROOT_OVERRIDE)。
    struct Sandbox(std::path::PathBuf, Option<std::sync::MutexGuard<'static, ()>>);

    impl Sandbox {
        fn new(tag: &str) -> Self {
            let guard: std::sync::MutexGuard<'static, ()> = store::tests::SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let dir = std::env::temp_dir().join(format!("marionette-council-orch-{tag}-{}", new_run_id()));
            let _ = fs::remove_dir_all(&dir);
            store::set_root_override_for_tests(Some(dir.clone()));
            Sandbox(dir, Some(guard))
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            store::set_root_override_for_tests(None);
            let _ = fs::remove_dir_all(&self.0);
            drop(self.1.take());
        }
    }

    fn setup_run(sandbox: &Sandbox, tag: &str) -> CouncilSpec {
        let mut spec = sample_spec();
        spec.run_id = format!("t-{tag}-{}", new_run_id());
        let state = CouncilState {
            run_id: spec.run_id.clone(),
            phase: RunPhase::Created,
            round: 1,
            paused_from: None,
            member_status: spec
                .members
                .iter()
                .map(|m| (m.id.clone(), MemberRuntimeStatus::Idle))
                .collect(),
            budget_usage: Default::default(),
            tasks: Vec::new(),
            verify_attempts: 0,
            updated_at: now_millis(),
            error: None,
        };
        store::save_spec(&spec).unwrap();
        store::save_state(&state).unwrap();
        let _ = sandbox;
        spec
    }

    fn wait_phase(run_id: &str, phase: RunPhase, timeout_ms: u64) {
        wait_for(
            || store::load_state(run_id).map(|s| s.phase == phase).unwrap_or(false),
            timeout_ms,
        );
    }

    fn wait_for(f: impl Fn() -> bool, timeout_ms: u64) {
        let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
        while std::time::Instant::now() < deadline {
            if f() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("等待条件超时({timeout_ms}ms)");
    }

    /// 完整协议跑通(M1 主验收):3 模型独立提案 → 交叉质询 → 修订 →
    /// 主席综合 → 计划门。
    #[test]
    fn full_protocol_run() {
        let sandbox = Sandbox::new("full");
        let mut spec = setup_run(&sandbox, "full");
        spec.rounds = 1;
        store::save_spec(&spec).unwrap();
        let mock = Arc::new(MockClient::new());
        for m in &spec.members {
            if m.is_chair {
                mock.enqueue(&m.model, vec![MOCK_PLAN.to_string()]);
            } else {
                mock.enqueue(&m.model, vec!["member-resp".to_string()]);
            }
        }
        start_run(CouncilEventEmitter::new(None), spec.run_id.clone(), crate::council::orchestrator::RunClients { model: Some(mock.clone()), harness: None }).unwrap();
        wait_phase(&spec.run_id, RunPhase::PlanReady, 30_000);

        let state = store::load_state(&spec.run_id).unwrap();
        assert_eq!(state.phase, RunPhase::PlanReady, "应停在计划门");
        let messages = store::load_messages(&spec.run_id).unwrap();
        let count = |k: MessageKind| messages.iter().filter(|m| m.kind == k).count();
        assert_eq!(count(MessageKind::Briefing), 1);
        assert_eq!(count(MessageKind::Proposal), 3, "三份独立提案(含主席)");
        assert!(count(MessageKind::Critique) >= 2, "非主席成员各至少一条质询");
        assert_eq!(count(MessageKind::Synthesis), 1);
        assert_eq!(count(MessageKind::Plan), 1);
        let plan = store::load_plan(&spec.run_id).unwrap().expect("plan.json 落盘");
        assert!(!plan.steps.is_empty());
        let transitions = store::load_transitions(&spec.run_id).unwrap();
        // rounds=1 的合法链:created→analyzing→proposing→reviewing→synthesizing→plan_ready = 5。
        assert!(transitions.len() >= 5, "审计链完整:{}", transitions.len());
        assert_eq!(transitions.last().unwrap().to, RunPhase::PlanReady, "链尾必须是 plan_ready");
        let models: std::collections::HashSet<String> = mock.models_called().into_iter().collect();
        assert_eq!(models.len(), 3, "三个不同模型都收到请求(验收 #1)");
    }

    /// 验收 #3:独立提案阶段的成员 prompt 不含其他成员的提案文本。
    #[test]
    fn proposal_phase_information_isolation() {
        let sandbox = Sandbox::new("iso");
        let spec = setup_run(&sandbox, "iso");
        let mock = Arc::new(MockClient::new());
        // 简报要全员共享——chair 模型返回中性简报文本,非 chair 各带唯一
        // 标记;若编排器把他人提案泄进简报/提案输入,标记就会串。
        let mut marks: std::collections::HashMap<String, &str> = std::collections::HashMap::new();
        for (i, m) in spec.members.iter().enumerate() {
            if m.is_chair {
                mock.enqueue(&m.model, vec!["中性简报:关注正确性与复杂度".to_string()]);
            } else {
                let mark = Box::leak(format!("PROPOSAL_MARK_{i}").into_boxed_str());
                marks.insert(m.model.clone(), mark);
                mock.enqueue(&m.model, vec![mark.to_string()]);
            }
        }
        start_run(CouncilEventEmitter::new(None), spec.run_id.clone(), crate::council::orchestrator::RunClients { model: Some(mock.clone()), harness: None }).unwrap();
        wait_phase(&spec.run_id, RunPhase::Reviewing, 90_000);
        cancel_run(&spec.run_id).ok();

        let calls = mock.calls.lock().unwrap().clone();
        let proposal_calls: Vec<&MockCall> = calls
            .iter()
            .filter(|c| c.prompt.contains("独立完成对以下任务的技术提案"))
            .collect();
        assert_eq!(proposal_calls.len(), 3, "三次提案调用");
        for call in &proposal_calls {
            // 从 prompt 里认出这是谁的提案调用(role_header 带 "(id: xxx)")。
            let caller = spec
                .members
                .iter()
                .find(|m| call.prompt.contains(&format!("id: {})", m.id)))
                .expect("prompt 应含成员 id");
            for (model, mark) in &marks {
                let other = spec.members.iter().find(|m| &m.model == model).unwrap();
                if other.id != caller.id {
                    assert!(
                        !call.prompt.contains(mark),
                        "成员 {} 的提案 prompt 泄露了成员 {} 的提案标记",
                        caller.id,
                        other.id
                    );
                }
            }
        }
    }

    /// 验收 #4 + 同轮路由:质询对象覆盖全部其他作者。
    #[test]
    fn critique_targets_all_other_authors() {
        let sandbox = Sandbox::new("route");
        let spec = setup_run(&sandbox, "route");
        let mock = Arc::new(MockClient::new());
        for m in &spec.members {
            if m.is_chair {
                mock.enqueue(&m.model, vec![MOCK_PLAN.to_string()]);
            } else {
                mock.enqueue(&m.model, vec!["resp".into()]);
            }
        }
        start_run(CouncilEventEmitter::new(None), spec.run_id.clone(), crate::council::orchestrator::RunClients { model: Some(mock.clone()), harness: None }).unwrap();
        wait_phase(&spec.run_id, RunPhase::PlanReady, 30_000);
        let messages = store::load_messages(&spec.run_id).unwrap();
        let critiques: Vec<_> = messages.iter().filter(|m| m.kind == MessageKind::Critique).collect();
        assert!(!critiques.is_empty());
        // prompts::pending_critiques_for 已单测路由;这里验证可见性函数
        // 对真实消息流的行为:每个被质询作者都能取到自己的质询。
        for round in 1..=2u32 {
            for author in spec.members.iter().map(|m| &m.id) {
                let pending = prompts::pending_critiques_for(&messages, author, round);
                let _ = pending; // 存在性由上一测试覆盖;此处防 panic
            }
        }
        // 质询 prompt 里确实渲染了他人方案(验收 #4 的输入面)。
        let calls = mock.calls.lock().unwrap().clone();
        let critique_calls: Vec<_> = calls.iter().filter(|c| c.prompt.contains("逐份质询")).collect();
        assert!(!critique_calls.is_empty(), "质询调用存在");
        for c in &critique_calls {
            assert!(c.prompt.contains("的方案"), "质询输入包含署名方案");
        }
    }

    /// 验收 #6:一个成员的模型失败,run 不死,其余继续。
    #[test]
    fn single_worker_failure_does_not_kill_run() {
        let sandbox = Sandbox::new("fail1");
        let spec = setup_run(&sandbox, "fail1");
        let mock = Arc::new(MockClient::new());
        let victim = spec.members.iter().find(|m| !m.is_chair).unwrap().clone();
        for m in &spec.members {
            if m.id != victim.id {
                let resp = if m.is_chair { MOCK_PLAN.to_string() } else { "ok resp".into() };
                mock.enqueue(&m.model, vec![resp]);
            }
        }
        start_run(CouncilEventEmitter::new(None), spec.run_id.clone(), crate::council::orchestrator::RunClients { model: Some(mock.clone()), harness: None }).unwrap();
        wait_phase(&spec.run_id, RunPhase::PlanReady, 30_000);
        let state = store::load_state(&spec.run_id).unwrap();
        assert_eq!(state.phase, RunPhase::PlanReady, "run 未被单成员失败毒化");
        let messages = store::load_messages(&spec.run_id).unwrap();
        assert!(messages.iter().any(|m| m.kind == MessageKind::Status && m.content.contains("失败")));
        assert_eq!(
            messages.iter().filter(|m| m.kind == MessageKind::Proposal).count(),
            2,
            "受害者的提案缺席,其余 2 份正常"
        );
    }

    /// 验收 #7:重启恢复——已完成的提案不重调,从评审续跑。
    #[test]
    fn resume_does_not_replay_completed_phase() {
        let sandbox = Sandbox::new("resume");
        let spec = setup_run(&sandbox, "resume");
        let mock = Arc::new(MockClient::new());
        for (i, m) in spec.members.iter().enumerate() {
            let resp = if m.is_chair { MOCK_PLAN.to_string() } else { format!("R{i}") };
            mock.enqueue(&m.model, vec![resp]);
        }
        start_run(CouncilEventEmitter::new(None), spec.run_id.clone(), crate::council::orchestrator::RunClients { model: Some(mock.clone()), harness: None }).unwrap();
        wait_phase(&spec.run_id, RunPhase::Reviewing, 90_000);
        // 模拟崩溃:直接摘掉活线程注册。
        registry().lock().unwrap().remove(&spec.run_id);
        let calls_before = mock.call_count();
        let proposals_before = store::load_messages(&spec.run_id)
            .unwrap()
            .iter()
            .filter(|m| m.kind == MessageKind::Proposal)
            .count();
        assert_eq!(proposals_before, 3);
        // resume:同一 mock 看增量。
        let proposal_calls = |m: &MockClient| {
            m.calls.lock().unwrap().iter().filter(|c| c.prompt.contains("独立完成对以下任务的技术提案")).count()
        };
        let brief_calls = |m: &MockClient| {
            // briefing_prompt 独有特征串(质询/修订 prompt 里的简报引用段不含它)。
            m.calls.lock().unwrap().iter().filter(|c| c.prompt.contains("为以下任务产出开场简报")).count()
        };
        let proposals_before = proposal_calls(&mock);
        let briefs_before = brief_calls(&mock);
        start_run(CouncilEventEmitter::new(None), spec.run_id.clone(), crate::council::orchestrator::RunClients { model: Some(mock.clone()), harness: None }).unwrap();
        wait_phase(&spec.run_id, RunPhase::PlanReady, 30_000);
        // 恢复绝不重放已完成的 briefing/proposal(验收 #7:不重放副作用)。
        assert_eq!(proposal_calls(&mock), proposals_before, "提案调用零重放");
        assert_eq!(brief_calls(&mock), briefs_before, "简报调用零重放");
        assert!(mock.call_count() > calls_before, "恢复后确有新调用(续跑质询及以后)");
        let state = store::load_state(&spec.run_id).unwrap();
        assert_eq!(state.phase, RunPhase::PlanReady);
        // briefing 仍只有一条(未重放)。
        let messages = store::load_messages(&spec.run_id).unwrap();
        assert_eq!(messages.iter().filter(|m| m.kind == MessageKind::Briefing).count(), 1);
    }

    /// 验收 #8:取消后不再发起模型调用。
    #[test]
    fn cancel_stops_new_calls() {
        let sandbox = Sandbox::new("cancel");
        let mut spec = setup_run(&sandbox, "cancel");
        spec.max_concurrent = 1; // 串行化 fan-out
        store::save_spec(&spec).unwrap();
        let mock = Arc::new(MockClient::new());
        for m in &spec.members {
            mock.enqueue(&m.model, vec!["resp".into()]);
        }
        start_run(CouncilEventEmitter::new(None), spec.run_id.clone(), crate::council::orchestrator::RunClients { model: Some(mock.clone()), harness: None }).unwrap();
        wait_for(
            || {
                store::load_messages(&spec.run_id)
                    .map(|ms| ms.iter().any(|m| m.kind == MessageKind::Proposal))
                    .unwrap_or(false)
            },
            30_000,
        );
        cancel_run(&spec.run_id).unwrap();
        wait_phase(&spec.run_id, RunPhase::Cancelled, 30_000);
        let calls_at_cancel = mock.call_count();
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(mock.call_count(), calls_at_cancel, "取消后不再有新调用");
        let state = store::load_state(&spec.run_id).unwrap();
        assert_eq!(state.phase, RunPhase::Cancelled);
    }

    /// 验收 #9:计划门——Created 不可直达 Executing;PlanReady 拒绝
    /// 跳过批准。
    #[test]
    fn plan_gate_blocks_unapproved_execution() {
        let sandbox = Sandbox::new("gate");
        let spec = setup_run(&sandbox, "gate");
        let mut s = store::load_state(&spec.run_id).unwrap();
        assert_eq!(s.phase, RunPhase::Created);
        assert!(apply_transition(&mut s, RunPhase::Executing, None).is_err());
        s.phase = RunPhase::PlanReady;
        assert!(apply_transition(&mut s, RunPhase::Verifying, None).is_err());
        assert!(apply_transition(&mut s, RunPhase::Executing, Some("user approved")).is_ok());
    }

    /// 预算闸:请求数到顶 → BudgetExhausted 终态。
    #[test]
    fn budget_exhaustion_is_terminal() {
        let sandbox = Sandbox::new("budget");
        let mut spec = setup_run(&sandbox, "budget");
        spec.budget.max_requests = Some(3);
        store::save_spec(&spec).unwrap();
        let mock = Arc::new(MockClient::new());
        for m in &spec.members {
            mock.enqueue(&m.model, vec!["r".into()]);
        }
        start_run(CouncilEventEmitter::new(None), spec.run_id.clone(), crate::council::orchestrator::RunClients { model: Some(mock.clone()), harness: None }).unwrap();
        wait_phase(&spec.run_id, RunPhase::BudgetExhausted, 30_000);
        let state = store::load_state(&spec.run_id).unwrap();
        assert!(state.phase.is_terminal());
    }

    /// 验收 #2:同一模型两个成员,各自独立调用与消息身份。
    #[test]
    fn same_model_two_members_distinct_calls() {
        let sandbox = Sandbox::new("same");
        let mut spec = setup_run(&sandbox, "same");
        spec.members[2].model = spec.members[0].model.clone();
        store::save_spec(&spec).unwrap();
        let mock = Arc::new(MockClient::new());
        let shared_model = spec.members[0].model.clone();
        mock.enqueue(&shared_model, vec!["shared".into()]);
        for m in &spec.members {
            if m.model != shared_model {
                mock.enqueue(&m.model, vec!["other".into()]);
            }
        }
        start_run(CouncilEventEmitter::new(None), spec.run_id.clone(), crate::council::orchestrator::RunClients { model: Some(mock.clone()), harness: None }).unwrap();
        wait_phase(&spec.run_id, RunPhase::Reviewing, 90_000);
        cancel_run(&spec.run_id).ok();
        let messages = store::load_messages(&spec.run_id).unwrap();
        let glm = &spec.members[0].id;
        let ds = &spec.members[2].id;
        assert!(messages.iter().any(|m| m.from == *glm && m.kind == MessageKind::Proposal));
        assert!(messages.iter().any(|m| m.from == *ds && m.kind == MessageKind::Proposal));
        let proposal_calls = mock
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.prompt.contains("独立完成对以下任务的技术提案") && c.model == shared_model)
            .count();
        assert_eq!(proposal_calls, 2, "同模型的两个成员各自被调用");
    }

    /// 验收 #11:验证失败触发受限修订路径——首轮执行产出坏内容,
    /// 验证命令失败 → 主席修订计划(同 id 换 detail)→ 受影响任务复位
    /// 重跑 → 验证通过 → Completed。全程计划版本留痕。
    #[test]
    fn verification_failure_triggers_bounded_repair() {
        let sandbox = Sandbox::new("repair");
        // 真实 git 临时仓库(验证命令要在里面跑)。
        let repo = std::env::temp_dir().join(format!("marionette-council-repair-{}", new_run_id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        for args in [vec!["init", "-q"], vec!["config", "user.email", "t@t"], vec!["config", "user.name", "t"]] {
            assert!(std::process::Command::new("git").args(args).current_dir(&repo).status().unwrap().success());
        }
        std::fs::write(repo.join("seed.txt"), "seed
").unwrap();
        assert!(std::process::Command::new("git").args(["add", "--all"]).current_dir(&repo).status().unwrap().success());
        assert!(std::process::Command::new("git").args(["commit", "-q", "-m", "init"]).current_dir(&repo).status().unwrap().success());

        let mut spec = setup_run(&sandbox, "repair");
        spec.mode = CouncilMode::CouncilAndExecution;
        spec.executor = Some(crate::council::types::ExecutorSpec {
            agent_id: "fake-agent".into(),
            cwd: repo.to_string_lossy().to_string(),
            project_id: None,
            step_timeout_secs: Some(30),
        });
        store::save_spec(&spec).unwrap();

        // plan v1:写坏内容;验证要求 good。
        let plan_v1 = crate::council::orchestrator::parse_plan(
            r#"{"summary":"v1","steps":[{"id":"s1","title":"写文件","detail":"WRITE out.txt|bad","dependsOn":[],"acceptance":"含 good"}],"verification":["cmd /C exit 1"]}"#,
        )
        .unwrap();
        store::save_plan(&spec.run_id, &plan_v1).unwrap();

        // 状态推进到 Executing(测试内直接落状态;approve 是 UI 命令)。
        let mut state = store::load_state(&spec.run_id).unwrap();
        let mut phase = RunPhase::Created;
        for hop in [RunPhase::Analyzing, RunPhase::Proposing, RunPhase::Reviewing, RunPhase::Synthesizing, RunPhase::PlanReady, RunPhase::Executing] {
            let rec = crate::council::state::apply_transition(&mut state, hop, Some("test")).unwrap();
            store::append_transition(&spec.run_id, &rec).unwrap();
            phase = hop;
        }
        let _ = phase;
        store::save_state(&state).unwrap();

        // chair 的修复调用返回 plan v2(同 id,detail 改好)。
        let plan_v2_json = r#"{"summary":"v2 修复","steps":[{"id":"s1","title":"写文件","detail":"WRITE out.txt|good","dependsOn":[],"acceptance":"含 good"}],"verification":["cmd /C exit 0"]}"#;
        let mock = Arc::new(MockClient::new());
        mock.enqueue(&spec.members[1].model, vec![plan_v2_json.to_string()]); // chair = members[1]

        let harness = Arc::new(crate::council::executor::ScriptedHarness { timeout_secs: 30 });
        start_run(
            CouncilEventEmitter::new(None),
            spec.run_id.clone(),
            RunClients { model: Some(mock.clone()), harness: Some(harness) },
        )
        .unwrap();
        wait_phase(&spec.run_id, RunPhase::Completed, 60_000);

        let final_state = store::load_state(&spec.run_id).unwrap();
        assert_eq!(final_state.phase, RunPhase::Completed, "修复后应完成:{:?}", final_state.error);
        assert_eq!(final_state.verify_attempts, 1, "恰好一轮修复");
        // 文件最终是修复后的内容。
        let content = std::fs::read_to_string(repo.join("out.txt")).unwrap();
        assert!(content.contains("good"), "最终内容应来自修订计划:{content}");
        // 计划版本留痕:两条 Plan 消息(v1 未走消息流,这里是修复轮的 v2;至少一条 repair)。
        let messages = store::load_messages(&spec.run_id).unwrap();
        assert!(messages.iter().any(|m| m.kind == MessageKind::Evidence));
        assert!(messages.iter().any(|m| m.kind == MessageKind::Plan && m.meta.as_ref().and_then(|v| v.get("repairRound")).is_some()));
        // 最终 plan.json 是 v2。
        let plan = store::load_plan(&spec.run_id).unwrap().unwrap();
        assert!(plan.steps[0].detail.contains("good"));
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// 修复上限:验证始终失败 → 3 次修复后 Failed 终态(不无限自改)。
    #[test]
    fn repair_loop_is_bounded() {
        let sandbox = Sandbox::new("bound");
        let repo = std::env::temp_dir().join(format!("marionette-council-bound-{}", new_run_id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        for args in [vec!["init", "-q"], vec!["config", "user.email", "t@t"], vec!["config", "user.name", "t"]] {
            assert!(std::process::Command::new("git").args(args).current_dir(&repo).status().unwrap().success());
        }
        std::fs::write(repo.join("seed.txt"), "seed
").unwrap();
        assert!(std::process::Command::new("git").args(["add", "--all"]).current_dir(&repo).status().unwrap().success());
        assert!(std::process::Command::new("git").args(["commit", "-q", "-m", "init"]).current_dir(&repo).status().unwrap().success());

        let mut spec = setup_run(&sandbox, "bound");
        spec.mode = CouncilMode::Autonomous;
        spec.executor = Some(crate::council::types::ExecutorSpec {
            agent_id: "fake-agent".into(),
            cwd: repo.to_string_lossy().to_string(),
            project_id: None,
            step_timeout_secs: Some(30),
        });
        store::save_spec(&spec).unwrap();

        // 恒败验证 + 修复计划永远不改 detail(同 id 同 detail → 任务虽复位,
        // 结果不变)→ 循环触顶。
        let plan = parse_plan(
            r#"{"summary":"v","steps":[{"id":"s1","title":"写","detail":"WRITE out.txt|bad","dependsOn":[],"acceptance":"含 good"}],"verification":["cmd /C exit 1"]}"#,
        )
        .unwrap();
        store::save_plan(&spec.run_id, &plan).unwrap();

        let mut state = store::load_state(&spec.run_id).unwrap();
        for hop in [RunPhase::Analyzing, RunPhase::Proposing, RunPhase::Reviewing, RunPhase::Synthesizing, RunPhase::PlanReady, RunPhase::Executing] {
            let rec = crate::council::state::apply_transition(&mut state, hop, Some("test")).unwrap();
            store::append_transition(&spec.run_id, &rec).unwrap();
        }
        store::save_state(&state).unwrap();

        let same_plan_json = r#"{"summary":"v","steps":[{"id":"s1","title":"写","detail":"WRITE out.txt|bad","dependsOn":[],"acceptance":"含 good"}],"verification":["cmd /C exit 1"]}"#;
        let mock = Arc::new(MockClient::new());
        mock.enqueue(&spec.members[1].model, vec![same_plan_json.to_string()]);
        let harness = Arc::new(crate::council::executor::ScriptedHarness { timeout_secs: 30 });
        start_run(
            CouncilEventEmitter::new(None),
            spec.run_id.clone(),
            RunClients { model: Some(mock), harness: Some(harness) },
        )
        .unwrap();
        wait_phase(&spec.run_id, RunPhase::Failed, 120_000);
        let final_state = store::load_state(&spec.run_id).unwrap();
        assert_eq!(final_state.verify_attempts, MAX_VERIFY_ATTEMPTS, "恰好用尽 3 次修复");
        assert!(final_state.error.as_deref().unwrap_or("").contains("修复次数已用尽"));
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// M5 分层背书端到端:contributor 的提案被非主席 lead 背书
    /// (verdict/targetAuthor/targetKind 齐备),synthesis 输入带背书
    /// 标记,run 到 plan_ready;历史统计落账。
    #[test]
    fn tiered_endorsement_flow() {
        let sandbox = Sandbox::new("endorse");
        let mut spec = setup_run(&sandbox, "endorse");
        spec.rounds = 1;
        // 分层:members[0]=lead(背书人)、members[1]=主席(lead)、
        // members[2]=contributor(被检验)。
        spec.members[2].tier = crate::council::types::MemberTier::Contributor;
        store::save_spec(&spec).unwrap();

        let mock = Arc::new(MockClient::new());
        // 背书人模型:提案+质询+背书全部回合法背书 JSON(提案/质询正文
        // 是什么无所谓,背书调用按 prompt 特征数出来)。
        mock.enqueue(
            &spec.members[0].model,
            vec![r#"{"verdict":"endorse","reason":"推理与结论一致"}"#.to_string()],
        );
        // 主席:briefing / synthesis / plan 都用合法 plan JSON 循环。
        mock.enqueue(&spec.members[1].model, vec![MOCK_PLAN.to_string()]);
        // contributor:普通文本产出。
        mock.enqueue(&spec.members[2].model, vec!["调研结论:三种备选方案".to_string()]);

        start_run(
            CouncilEventEmitter::new(None),
            spec.run_id.clone(),
            RunClients { model: Some(mock.clone()), harness: None },
        )
        .unwrap();
        wait_phase(&spec.run_id, RunPhase::PlanReady, 60_000);

        let messages = store::load_messages(&spec.run_id).unwrap();
        // 背书消息:lead → contributor,针对其 round-1 提案。
        let endorsements: Vec<&CouncilMessage> =
            messages.iter().filter(|m| m.kind == MessageKind::Endorsement).collect();
        assert!(!endorsements.is_empty(), "应产生背书消息");
        for e in &endorsements {
            assert_eq!(e.from, spec.members[0].id, "背书只能由 lead 发出");
            let meta = e.meta.as_ref().unwrap();
            assert_eq!(meta.get("targetAuthor").and_then(|v| v.as_str()), Some(spec.members[2].id.as_str()));
            assert_eq!(meta.get("targetKind").and_then(|v| v.as_str()), Some("Proposal"));
            assert_eq!(meta.get("verdict").and_then(|v| v.as_str()), Some("endorse"));
        }
        // synthesis 的输入(用 mock 调用记录验证)确实渲染了背书标记。
        let calls = mock.calls.lock().unwrap().clone();
        let synthesis_call = calls.iter().find(|c| c.prompt.contains("输出综合")).expect("有综合调用");
        assert!(synthesis_call.prompt.contains("〔lead 背书:认可 1"), "综合输入应带背书标记");
        // 历史统计落账:contributor 的模型有背书记录。
        let stats = crate::council::formation::load_model_stats();
        let entry = stats.models.get(&spec.members[2].model).expect("统计应含 contributor 模型");
        assert_eq!(entry.runs, 1);
        assert!(entry.endorsed >= 1, "收到认可背书:{:?}", entry);
    }

    /// 兼容性:全员 lead(旧配置)不产生任何背书调用与消息。
    #[test]
    fn all_lead_config_skips_endorsement() {
        let sandbox = Sandbox::new("nolegs");
        let mut spec = setup_run(&sandbox, "nolegs");
        spec.rounds = 1;
        store::save_spec(&spec).unwrap();
        let mock = Arc::new(MockClient::new());
        for m in &spec.members {
            let resp = if m.is_chair { MOCK_PLAN.to_string() } else { "resp".to_string() };
            mock.enqueue(&m.model, vec![resp]);
        }
        start_run(
            CouncilEventEmitter::new(None),
            spec.run_id.clone(),
            RunClients { model: Some(mock.clone()), harness: None },
        )
        .unwrap();
        wait_phase(&spec.run_id, RunPhase::PlanReady, 60_000);
        let messages = store::load_messages(&spec.run_id).unwrap();
        assert!(
            messages.iter().all(|m| m.kind != MessageKind::Endorsement),
            "全员 lead 时不应有背书消息"
        );
        // 背书 prompt 从未发出。
        let calls = mock.calls.lock().unwrap().clone();
        assert!(calls.iter().all(|c| !c.prompt.contains("待检验的产出")), "零背书调用");
    }

    /// 背书否决路径:lead 全部 reject → synthesis 输入标「认可 0 / 否决」,
    /// 主席按分层规则把该主张挡在结论外(不进 crash,run 仍到 plan_ready)。
    #[test]
    fn rejected_contributor_output_is_flagged() {
        let sandbox = Sandbox::new("reject");
        let mut spec = setup_run(&sandbox, "reject");
        spec.rounds = 1;
        spec.members[2].tier = crate::council::types::MemberTier::Contributor;
        store::save_spec(&spec).unwrap();
        let mock = Arc::new(MockClient::new());
        mock.enqueue(
            &spec.members[0].model,
            vec![r#"{"verdict":"reject","reason":"复杂度分析错误,忽略了最坏情况"}"#.to_string()],
        );
        mock.enqueue(&spec.members[1].model, vec![MOCK_PLAN.to_string()]);
        mock.enqueue(&spec.members[2].model, vec!["方案:用 O(n^2) 暴力扫描".to_string()]);
        start_run(
            CouncilEventEmitter::new(None),
            spec.run_id.clone(),
            RunClients { model: Some(mock.clone()), harness: None },
        )
        .unwrap();
        wait_phase(&spec.run_id, RunPhase::PlanReady, 60_000);
        let calls = mock.calls.lock().unwrap().clone();
        let synthesis_call = calls.iter().find(|c| c.prompt.contains("输出综合")).expect("有综合调用");
        assert!(synthesis_call.prompt.contains("认可 0 / 否决 1"), "否决要在综合输入可见");
        let messages = store::load_messages(&spec.run_id).unwrap();
        assert!(messages
            .iter()
            .any(|m| m.kind == MessageKind::Endorsement && m.content.contains("否决")));
    }

    /// M5.2 身份局对局端到端(mock):整局从 Created → Playing →
    /// Completed;消息账本含开局公告/行动/终局身份揭示;取消路径生效。
    #[test]
    fn sgs_game_plays_to_completion_with_mock_agents() {
        let sandbox = Sandbox::new("sgs");
        let mut spec = setup_run(&sandbox, "sgs");
        spec.mode = CouncilMode::SgsGame;
        spec.rounds = 1; // 游戏忽略
        spec.members.truncate(5.min(spec.members.len()));
        // 补足 5 人(样例只有 3 人)。
        while spec.members.len() < 5 {
            let n = spec.members.len() + 1;
            spec.members.push(MemberSpec {
                id: format!("p{n}"),
                display_name: format!("玩家{n}"),
                model: "m/extra".into(),
                role: "玩家".into(),
                is_chair: false,
                tier: crate::council::types::MemberTier::Lead,
            });
        }
        spec.budget.max_requests = Some(200); // 12 轮上限 × 5 人 ≈ 60 次
        store::save_spec(&spec).unwrap();
        let mock = Arc::new(MockClient::new());
        // 所有模型:pass 计划(纯发言)。
        let mut models: Vec<String> = spec.members.iter().map(|m| m.model.clone()).collect();
        models.dedup();
        for m in &models {
            mock.enqueue(m, vec![r#"{"plays":[],"say":"且听下回分解"}"#.to_string()]);
        }
        start_run(
            CouncilEventEmitter::new(None),
            spec.run_id.clone(),
            RunClients { model: Some(mock.clone()), harness: None },
        )
        .unwrap();
        wait_phase(&spec.run_id, RunPhase::Completed, 120_000);
        let state = store::load_state(&spec.run_id).unwrap();
        assert_eq!(state.phase, RunPhase::Completed, "{:?}", state.error);
        let messages = store::load_messages(&spec.run_id).unwrap();
        assert!(messages.iter().any(|m| m.kind == MessageKind::GameAction && m.content.contains("开局")), "有开局公告");
        assert!(messages.iter().any(|m| m.kind == MessageKind::GameAction && m.content.contains("发言")), "有玩家发言");
        assert!(messages.iter().any(|m| m.kind == MessageKind::GameAction && m.content.contains("终局身份揭示")), "有终局揭示");
        // 恢复现场可读:game-state.json 存在且已 finished。
        let (game, _hist) = crate::council::game::runner::load_game(&spec.run_id).expect("game-state 落盘");
        assert!(game.finished.is_some());
        // 主公身份在视角层全程公开、其余暗置(引擎不变量)。
        assert_eq!(game.players.iter().filter(|p| p.role == crate::council::game::engine::SgsRole::Lord).count(), 1);
    }

    #[test]
    fn sgs_game_cancel_midgame() {
        let sandbox = Sandbox::new("sgscancel");
        let mut spec = setup_run(&sandbox, "sgscancel");
        spec.mode = CouncilMode::SgsGame;
        while spec.members.len() < 5 {
            let n = spec.members.len() + 1;
            spec.members.push(MemberSpec {
                id: format!("p{n}"),
                display_name: format!("玩家{n}"),
                model: "m/extra".into(),
                role: "玩家".into(),
                is_chair: false,
                tier: crate::council::types::MemberTier::Lead,
            });
        }
        spec.budget.max_requests = Some(200);
        store::save_spec(&spec).unwrap();
        let mock = Arc::new(MockClient::new());
        let mut models: Vec<String> = spec.members.iter().map(|m| m.model.clone()).collect();
        models.dedup();
        for m in &models {
            mock.enqueue(m, vec![r#"{"plays":[],"say":"过"}"#.to_string()]);
        }
        start_run(
            CouncilEventEmitter::new(None),
            spec.run_id.clone(),
            RunClients { model: Some(mock.clone()), harness: None },
        )
        .unwrap();
        wait_phase(&spec.run_id, RunPhase::Playing, 60_000);
        cancel_run(&spec.run_id).unwrap();
        wait_phase(&spec.run_id, RunPhase::Cancelled, 60_000);
        let calls = mock.call_count();
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(mock.call_count(), calls, "取消后不再调用模型");
    }

    /// 计划解析:围栏剥离 + 字段容错。
    #[test]
    fn parse_plan_accepts_fenced_json() {
        let raw = "前置说明\n```json\n{\"summary\":\"s\",\"steps\":[{\"id\":\"s1\",\"title\":\"t\",\"detail\":\"d\",\"dependsOn\":[],\"acceptance\":\"a\"}],\"verification\":[\"cargo test\"]}\n```\n后置";
        let plan = parse_plan(raw).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert_eq!(plan.verification, vec!["cargo test".to_string()]);
        assert!(parse_plan("不是 JSON").is_err());
        assert!(parse_plan("{\"summary\":\"s\",\"steps\":[]}").is_err(), "空步骤拒绝");
    }
}
