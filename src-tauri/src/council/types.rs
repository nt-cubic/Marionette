//! Council Mode — 领域类型。
//!
//! 设计定稿见 `docs/council/architecture-audit.md`。三层概念严格区分
//! (任务书 §3.2):模型 provider(引擎内的 id)、模型 worker(本模块直接
//! HTTP 调引擎的审议参与者)、agent harness worker(M2 起经 ACP 执行计划
//! 的真实 coding agent)。M1 只有模型 worker。
//!
//! serde 全部 camelCase,与 `models.rs::Session` 的前端契约一致。

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 审议消息种类。任务书 §3.4 清单 + Briefing(Analyzing 阶段主席议程)
/// + Plan(Plan Gate 产出)。
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Hash)]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    /// Analyzing 阶段主席产出的任务简报/议程。
    Briefing,
    /// Phase A:独立提案(或轮次内新提案)。
    Proposal,
    /// Phase B:交叉质询。
    Critique,
    /// Phase B/C:对质询的反驳(不修订原文时)。
    Rebuttal,
    /// Phase C:作者修订版(旧版本保留为独立消息,append-only)。
    Revision,
    Question,
    Answer,
    /// 分层背书:lead 成员对 contributor 产出的认可/否决判定
    /// (M5 能力分层协议;verdict 在 meta.endorse)。
    Endorsement,
    /// Phase D:主席综合。
    Synthesis,
    /// Phase E:结构化实施计划(PlanDoc 的 JSON 正文)。
    Plan,
    Task,
    /// 运行状态通告(worker 失败、跳过原因等系统侧记录)。
    Status,
    Evidence,
}

/// 一条审议消息。`message_id` 是幂等键:store 拒绝重复 append。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CouncilMessage {
    pub message_id: String,
    pub run_id: String,
    pub round: u32,
    /// 发送者成员 id,系统消息用 `system`。
    pub from: String,
    /// 接收者成员 id、`all`(受控广播)或 `chair`。
    pub to: String,
    pub kind: MessageKind,
    pub reply_to: Option<String>,
    pub content: String,
    /// epoch 毫秒。
    pub timestamp: i64,
    /// 附加元数据(critique 的目标提案 id / 作者、失败原因等),前端与
    /// 协议判断共用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<serde_json::Value>,
}

/// 能力分层(M5,任务书「能力感知组队」):lead = 强模型,主导综合、
/// 背书他人产出;contributor = 较弱模型,承担调研/简单设计,其产出
/// 须经至少一个 lead 背书才被主席采信。默认 lead = 与旧 spec 行为
/// 完全一致(全员同层,背书跳过)。
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemberTier {
    #[default]
    Lead,
    Contributor,
}

/// 审议成员(模型 worker)。同一 model 可以配置多个成员(验收 #2),
/// 每个成员有独立的角色 prompt 与消息身份。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct MemberSpec {
    /// kebab-case 唯一 id,如 `glm-architect`。
    pub id: String,
    pub display_name: String,
    /// 引擎模型 id(`engine::catalog_direct_entries` 的 `CatalogModelRow.id`)。
    pub model: String,
    /// 角色描述,进各阶段 system prompt。
    pub role: String,
    /// 是否主席。恰好一个成员为 chair(校验在 spec 校验处)。
    #[serde(default)]
    pub is_chair: bool,
    /// 能力分层(M5)。缺省 lead(旧 run 兼容)。
    #[serde(default)]
    pub tier: MemberTier,
}

/// 运行模式(任务书 §3.1)。M1 全部模式都止步于 `PlanReady`;
/// 执行型模式在 M2 接 ACP。
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum CouncilMode {
    CouncilOnly,
    CouncilAndExecution,
    Autonomous,
}

/// 执行器配置(M2):计划批准后由哪个 agent harness 在哪个目录落地。
/// `None` 或 mode=council_only → 计划批准即交付完成(不执行)。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExecutorSpec {
    /// agent id(`custom_agents::all_agents_merged()` 里的一条)。
    pub agent_id: String,
    /// 项目根(git 仓库;worktree 隔离的基)。
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// 单步 turn 等待上限(秒;默认 900)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_timeout_secs: Option<u64>,
}

/// 资源预算。创建时固化进 spec.json,运行期不可改(OmO store.ts:148
/// 验证过的决策)。全部 `None` = 不设限(仍受取消与失败终止)。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Budget {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_requests: Option<u64>,
    /// token 估算上限(字符数/4,prompt+output 双向累加)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wall_clock_secs: Option<u64>,
}

/// 不可变的 run 配置,创建后落盘不再修改。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CouncilSpec {
    pub run_id: String,
    pub title: String,
    pub task: String,
    pub mode: CouncilMode,
    /// ≥2 成员(默认 3);恰好一个 chair(缺省第一个)。
    pub members: Vec<MemberSpec>,
    /// 审议轮次(≥1)。round 1 = 提案+质询+修订;round N = 带修订版再质询。
    pub rounds: u32,
    /// 并发 worker 上限(1..=8)。
    pub max_concurrent: u32,
    pub budget: Budget,
    /// epoch 毫秒。
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// 执行配置(M2;M1 创建的 run 无此字段,serde 默认兼容)。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<ExecutorSpec>,
}

/// 预算用量快照,随 state.json 持久化。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct BudgetUsage {
    pub requests: u64,
    pub tokens_est: u64,
    /// epoch 毫秒;0 = 尚未发起任何调用。
    pub started_at: i64,
}

/// 成员运行时状态。
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum MemberRuntimeStatus {
    Idle,
    Working,
    Done,
    Failed { reason: String },
}

/// run 的可变快照。状态机转移全部经 `state.rs::apply_transition` 落账。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CouncilState {
    pub run_id: String,
    pub phase: RunPhase,
    /// 当前协议轮次,1-based。
    pub round: u32,
    /// `Paused` 时记录恢复目标阶段。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused_from: Option<RunPhase>,
    pub member_status: HashMap<String, MemberRuntimeStatus>,
    pub budget_usage: BudgetUsage,
    /// 执行阶段的任务账目(Executing 起;M1 run 为空数组)。
    #[serde(default)]
    pub tasks: Vec<crate::council::tasks::TaskProgress>,
    /// 验证-修复回路已用次数(上限 3,任务书 §4.3)。
    #[serde(default)]
    pub verify_attempts: u32,
    /// epoch 毫秒。
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Plan Gate 的结构化产物。`PlanDoc` 解析失败会触发一次修复调用
/// (见 orchestrator),两次失败则 run Failed——不静默降级成自由文本计划。
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct PlanDoc {
    pub summary: String,
    pub steps: Vec<PlanStep>,
    /// 显式遗留不确定性:主席必须列出,不许和稀泥(任务书 §3.3 Phase D)。
    #[serde(default)]
    pub open_questions: Vec<String>,
    /// 整体验收标准。
    #[serde(default)]
    pub acceptance: Vec<String>,
    /// 验证命令(build/test/lint…,M3 验证循环直接消费)。
    #[serde(default)]
    pub verification: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlanStep {
    pub id: String,
    pub title: String,
    pub detail: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub acceptance: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_hint: Option<String>,
}

/// 状态转移审计记录(transitions.jsonl 每行一条)。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TransitionRecord {
    /// epoch 毫秒。
    pub at: i64,
    pub from: RunPhase,
    pub to: RunPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// 给前端的 run 摘要(列表用)。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RunSummary {
    pub run_id: String,
    pub title: String,
    pub phase: RunPhase,
    pub mode: CouncilMode,
    pub member_count: usize,
    pub rounds: u32,
    pub message_count: usize,
    pub created_at: i64,
    pub updated_at: i64,
}

/// 给前端的 run 全量详情。
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RunDetail {
    pub spec: CouncilSpec,
    pub state: CouncilState,
    pub messages: Vec<CouncilMessage>,
    pub transitions: Vec<TransitionRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<PlanDoc>,
    /// 进程内是否有活 orchestrator 线程。
    pub live: bool,
}

/// run 生命周期状态机(任务书 §3.5)。转移合法性硬编码在
/// `state.rs::can_transition`;`transitions.jsonl` 记录每次转移。
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum RunPhase {
    Created,
    Analyzing,
    Proposing,
    Reviewing,
    Revising,
    Synthesizing,
    /// 计划就绪,等待用户批准(执行门,验收 #9)。
    PlanReady,
    /// 执行期需要人工确认时的挂起态(M2 权限场景)。
    AwaitingApproval,
    Executing,
    Verifying,
    Completed,
    Paused,
    Cancelled,
    Failed,
    BudgetExhausted,
}

impl RunPhase {
    pub fn is_terminal(self) -> bool {
        matches!(self, RunPhase::Completed | RunPhase::Cancelled | RunPhase::Failed | RunPhase::BudgetExhausted)
    }

    pub fn is_running(self) -> bool {
        !self.is_terminal() && self != RunPhase::Paused && self != RunPhase::PlanReady
    }
}

/// 校验 spec(创建入口调用)。返回 Err(人话) 而不是让 orchestrator
/// 运行期炸——OmO 的 parse 期 eligibility 裁决思路。
pub fn validate_spec(spec: &CouncilSpec) -> Result<(), String> {
    if spec.task.trim().is_empty() {
        return Err("任务描述不能为空".into());
    }
    if spec.members.len() < 2 {
        return Err("至少需要 2 个成员(建议 3 个不同模型)".into());
    }
    if spec.members.len() > 8 {
        return Err("成员数上限 8".into());
    }
    let chair_count = spec.members.iter().filter(|m| m.is_chair).count();
    match chair_count {
        0 => return Err("需要恰好一个主席成员(isChair)".into()),
        1 => {}
        _ => return Err("主席只能有一个".into()),
    }
    let mut ids = std::collections::HashSet::new();
    for m in &spec.members {
        if m.id.trim().is_empty() || !m.id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(format!("成员 id 只能是小写字母/数字/横线:{}", m.id));
        }
        if !ids.insert(m.id.clone()) {
            return Err(format!("成员 id 重复:{}", m.id));
        }
        if m.model.trim().is_empty() {
            return Err(format!("成员 {} 未选择模型", m.display_name));
        }
        if m.role.trim().is_empty() {
            return Err(format!("成员 {} 缺少角色描述", m.display_name));
        }
    }
    if spec.rounds < 1 {
        return Err("审议轮次至少 1".into());
    }
    if spec.rounds > 4 {
        return Err("审议轮次上限 4".into());
    }
    if !(1..=8).contains(&spec.max_concurrent) {
        return Err("并发上限须在 1..=8".into());
    }
    Ok(())
}

/// 生成 run id:时间戳+进程内计数器,零新依赖(体积红线)。
pub fn new_run_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    format!("c{millis:x}-{seq}")
}

/// 当前 epoch 毫秒。
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 简易 token 估算(字符/4)。中文偏保守(1 token≈1.5 字符,按 4 算
/// 高估),宁可提前触顶也不放行超支。
pub fn estimate_tokens(chars: usize) -> u64 {
    (chars as u64 + 3) / 4
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn sample_spec() -> CouncilSpec {
        CouncilSpec {
            run_id: new_run_id(),
            title: "test".into(),
            task: "设计一个 LRU 缓存".into(),
            mode: CouncilMode::CouncilOnly,
            members: vec![
                MemberSpec {
                    id: "glm-architect".into(),
                    display_name: "架构师".into(),
                    model: "glm/glm-5.3".into(),
                    role: "系统架构".into(),
                    is_chair: false,
                    tier: MemberTier::Lead,
                },
                MemberSpec {
                    id: "gpt-reviewer".into(),
                    display_name: "评审".into(),
                    model: "openai/gpt-5.2".into(),
                    role: "批判性评审".into(),
                    is_chair: true,
                    tier: MemberTier::Lead,
                },
                MemberSpec {
                    id: "ds-math".into(),
                    display_name: "算法".into(),
                    model: "deepseek/deepseek-v4".into(),
                    role: "算法与数学分析".into(),
                    is_chair: false,
                    tier: MemberTier::Lead,
                },
            ],
            rounds: 2,
            max_concurrent: 4,
            budget: Budget { max_requests: Some(40), max_tokens: Some(200_000), wall_clock_secs: None },
            created_at: now_millis(),
            project_id: None,
            cwd: None,
            executor: None,
        }
    }

    #[test]
    fn spec_validation_rejects_bad_input() {
        let mut spec = sample_spec();
        assert!(validate_spec(&spec).is_ok());

        spec.members.truncate(1);
        assert!(validate_spec(&spec).is_err());

        let mut two_chairs = sample_spec();
        two_chairs.members[2].is_chair = true;
        assert!(validate_spec(&two_chairs).is_err());

        let mut no_chair = sample_spec();
        for m in &mut no_chair.members {
            m.is_chair = false;
        }
        assert!(validate_spec(&no_chair).is_err());

        let mut dup = sample_spec();
        dup.members[2].id = "gpt-reviewer".into();
        assert!(validate_spec(&dup).is_err());
    }

    #[test]
    fn run_ids_unique() {
        assert_ne!(new_run_id(), new_run_id());
    }
}
