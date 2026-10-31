//! Council 审议协议的 prompt 构造(纯函数,信息隔离的强制点)。
//!
//! 阶段输入可见性(任务书 §3.3,验收 #3 的实现位置):
//! - Briefing:任务 + 主席角色。**不含任何成员输出**。
//! - Proposal:任务 + 本人角色。**绝不含他人提案**(独立提案)。
//! - Critique:任务 + 本人角色 + 他人提案/修订(署名)。不含本人未公开的草稿。
//! - Revision:任务 + 本人角色 + 本人最新版本 + 针对自己的全部质询。
//! - Synthesis:主席 + 全部消息(提案/质询/修订/反驳的署名流)。
//! - Plan:synthesis 产出 → 结构化 PlanDoc JSON。
//!
//! 可见性由 orchestrator 在调用这些函数前决定(它们只渲染给定输入),
//! 但函数签名刻意做成"只收该看的消息",配合隔离测试双向锁死。

use super::types::{CouncilMessage, CouncilSpec, MemberSpec, MemberTier, MessageKind, PlanDoc};

/// 所有阶段共用的角色前缀(任务书 §3.2:模型身份显式披露,同一模型
/// 多成员必须知道自己是谁)。
fn role_header(spec: &CouncilSpec, member: &MemberSpec) -> String {
    format!(
        "你是多模型审议团(Council)的成员「{}」(id: {}),由模型 {} 驱动,角色:{}。\n当前审议任务:「{}」\n",
        member.display_name, member.id, member.model, member.role, spec.title
    )
}

const OUTPUT_DISCIPLINE: &str = "\n输出纪律:直接输出正文(无需寒暄/复述任务),使用简体中文,可用 Markdown 结构化。你的回复将作为署名消息进入审议记录。\n";

/// Analyzing 阶段:主席任务简报(议程/关键决策点/给成员的提示)。
pub fn briefing_prompt(spec: &CouncilSpec, chair: &MemberSpec) -> (String, String) {
    let system = format!(
        "{}你是本次审议的主席(Chair),负责开场简报与最终综合。{}",
        role_header(spec, chair),
        OUTPUT_DISCIPLINE
    );
    let user = format!(
        "请为以下任务产出开场简报(Briefing),供全体成员在各自独立分析前阅读:\n\n\
         ## 任务\n{}\n\n\
         要求输出四节:\n\
         1. **任务重述**:用你自己的话精确复述目标与边界(1-3 句)。\n\
         2. **关键决策点**:这个任务里真正需要权衡、容易出错的地方(列表)。\n\
         3. **评审重点**:请各成员在提案与互评时优先覆盖的角度(列表)。\n\
         4. **澄清说明**:对任务里模糊处的统一解释(若有)。\n\n\
         注意:简报不预设任何技术方案,不暗示倾向;各成员仍须独立提案。",
        spec.task
    );
    (system, user)
}

/// Phase A:独立提案。输入刻意只有任务与本人角色——隔离测试断言
/// 输出不含其他成员的任何文本。
pub fn proposal_prompt(spec: &CouncilSpec, member: &MemberSpec, briefing: Option<&CouncilMessage>) -> (String, String) {
    let system = format!(
        "{}你是提案人。本轮你的提案**不会**被其他成员看到,直到所有提案完成;他们同样独立提案,你看不到他们的。{}",
        role_header(spec, member),
        OUTPUT_DISCIPLINE
    );
    let briefing_section = match briefing {
        Some(b) => format!("\n## 主席开场简报\n{}\n", b.content),
        None => String::new(),
    };
    let user = format!(
        "请独立完成对以下任务的技术提案(Proposal):\n\n## 任务\n{}\n{}\
         \n提案必须包含七节(缺一不可):\n1. **主张**:一句话核心方案。\n2. **假设**:你依赖的前提(环境/需求/外部行为)。\n3. **架构/算法**:具体设计——模块划分、数据结构、关键算法;能写伪代码就写。\n4. **实施提纲**:分步骤的落地顺序。\n5. **预期收益**:为什么这样做。\n6. **风险**:最可能失败的三件事。\n7. **验证策略**:如何证明它是对的(测试/实验/度量)。",
        spec.task, briefing_section
    );
    (system, user)
}

/// Phase B:交叉质询。`others` = 除本人外所有成员最近一轮的
/// Proposal/Revision(署名渲染)。
pub fn critique_prompt(
    spec: &CouncilSpec,
    member: &MemberSpec,
    round: u32,
    briefing: Option<&CouncilMessage>,
    others: &[CouncilMessage],
) -> (String, String) {
    let system = format!(
        "{}你是批判性评审人。逐份审视其他成员的方案,找出真实缺陷——事实错误、逻辑漏洞、数学/复杂度问题、工程隐患、验证策略缺失。**不做礼节性肯定**,不搞折中和稀泥;但每条批评必须具体可检验(引用原文、指出位置、说明为什么错或有什么风险)。证据不足时明确说「这一点我无法验证,建议实验 X」而不是猜。{}",
        role_header(spec, member),
        OUTPUT_DISCIPLINE
    );
    let briefing_section = match briefing {
        Some(b) => format!("\n## 主席开场简报\n{}\n", b.content),
        None => String::new(),
    };
    let mut proposals = String::new();
    for m in others {
        proposals.push_str(&format!("\n---\n### 来自 `{}` 的方案(第 {} 轮)\n{}\n", m.from, m.round, m.content));
    }
    let user = format!(
        "第 {round} 轮评审。以下是其他成员的当前方案,请逐份质询(每份至少一条,除非真的无懈可击——那种情况明确说「未发现实质缺陷」并给出你检查过的角度):\n{proposals}\
         \n## 任务原文\n{}\n{briefing_section}\
         \n输出格式:对每个作者一节,标题用 `### 对 <author-id> 的质询`,下面按条列出:\n- `[事实|逻辑|数学|工程|验证] 问题描述`(引用其原文关键句,说明问题与影响;可提出改进建议或要求其澄清的 `question`)。",
        spec.task
    );
    (system, user)
}

/// Phase C:修订。输入 = 本人最新版本 + 针对自己的全部质询(含 rebuttal 目标)。
pub fn revision_prompt(
    spec: &CouncilSpec,
    member: &MemberSpec,
    round: u32,
    own_latest: &CouncilMessage,
    critiques: &[CouncilMessage],
) -> (String, String) {
    let system = format!(
        "{}你是提案作者,现在回应收到的质询并修订方案。规则:**逐条回应**每条批评——接受并修改、或给出反驳理由(反驳必须有据,不为面子硬顶)。修订产生**新版本**,不覆盖旧版本(系统已保留历史)。若某条批评确实成立但需要更多信息,列为 open question。{}",
        role_header(spec, member),
        OUTPUT_DISCIPLINE
    );
    let mut critique_text = String::new();
    for c in critiques {
        critique_text.push_str(&format!("\n---\n来自 `{}`:\n{}\n", c.from, c.content));
    }
    let user = format!(
        "第 {round} 轮修订。\n\n## 你的当前方案(第 {} 轮版本)\n{}\n\n## 收到的全部质询\n{critique_text}\n\
         ## 任务原文\n{}\n\n\
         输出两节:\n1. **逐条回应**:对每条质询,`- 回应 <reviewer-id>`:接受(说明怎么改)/反驳(给出理由)/存疑(列为 open question)。\n2. **修订版方案**:完整的最新版本(保持七节结构;接受批评的部分实质修改,不是只改措辞)。",
        own_latest.round, own_latest.content, spec.task
    );
    (system, user)
}

/// Phase D:主席综合。输入 = 全部审议消息(署名流)。
pub fn synthesis_prompt(spec: &CouncilSpec, chair: &MemberSpec, all: &[CouncilMessage]) -> (String, String) {
    let system = format!(
        "{}你是主席,现在做最终综合(Synthesis)。职责:\n- 采信**有依据**的发现(谁的方案在证据上站得住,就采谁,与人气无关)。\n- 逐项裁决分歧:每个分歧给出裁决与理由;裁决靠证据,不靠投票。\n- 证据不足的分歧**不许强行统一**:归入 open questions,并给出针对性的实验/测试建议。\n- 明确列出仍然成立的风险。{}",
        role_header(spec, chair),
        OUTPUT_DISCIPLINE
    );
    // 分层背书状态(M5):contributor 的提案/修订附 lead 判定计数;
    // 采信规则同步写进任务指令(见下方"分层规则")。
    let tier_of = |id: &str| spec.members.iter().find(|m| m.id == id).map(|m| m.tier).unwrap_or(MemberTier::Lead);
    let endorsement_state = |author: &str, round: u32, kind: &MessageKind| -> Option<(usize, usize)> {
        if tier_of(author) != MemberTier::Contributor {
            return None;
        }
        if !matches!(kind, MessageKind::Proposal | MessageKind::Revision) {
            return None;
        }
        let mut endorsed = 0;
        let mut rejected = 0;
        for e in all.iter().filter(|m| m.kind == MessageKind::Endorsement) {
            let meta = match &e.meta { Some(v) => v, None => continue };
            let targets_author = meta.get("targetAuthor").and_then(|v| v.as_str()) == Some(author);
            let same_round = e.round == round;
            let same_kind = meta
                .get("targetKind")
                .and_then(|v| v.as_str())
                .map(|k| k == format!("{:?}", kind))
                .unwrap_or(true);
            if targets_author && same_round && same_kind {
                if meta.get("verdict").and_then(|v| v.as_str()) == Some("endorse") {
                    endorsed += 1;
                } else {
                    rejected += 1;
                }
            }
        }
        Some((endorsed, rejected))
    };
    let mut flow = String::new();
    for m in all {
        let mut label = match m.kind {
            MessageKind::Proposal => format!("提案(第 {} 轮)", m.round),
            MessageKind::Critique => format!("质询(第 {} 轮)", m.round),
            MessageKind::Revision => format!("修订(第 {} 轮)", m.round),
            MessageKind::Rebuttal => format!("反驳(第 {} 轮)", m.round),
            MessageKind::Briefing => "开场简报".to_string(),
            MessageKind::Endorsement => format!("背书(第 {} 轮)", m.round),
            _ => "发言".to_string(),
        };
        if let Some((endorsed, rejected)) = endorsement_state(&m.from, m.round, &m.kind) {
            label.push_str(&format!("〔lead 背书:认可 {endorsed} / 否决 {rejected}〕"));
        }
        flow.push_str(&format!("\n---\n#### {} · {} · `{}`\n{}\n", label, kind_name(m.kind), m.from, m.content));
    }
    let user = format!(
        "以下是全部审议记录,请输出综合:\n{flow}\n## 任务原文\n{}\n\n\
         输出四节:\n1. **采纳的结论**:各方方案中站得住的部分,以及最终合并出的技术路线(具体,可执行)。〔分层规则:lead 的产出按证据采信;contributor 的产出只有带「认可 ≥1」背书标记的才可直接采纳——全部被否决或零背书的 contributor 主张不得进入结论,其中有价值但未获认可的部分列入 open questions 并注明原因。〕\n2. **已裁决的分歧**:每条 `分歧 → 裁决 → 依据`。\n3. **遗留不确定性(open questions)**:无法裁决的,每条附一个能消解它的具体实验/测试。\n4. **风险清单**:仍然成立的风险与缓解。",
        spec.task
    );
    (system, user)
}

/// Phase E:结构化计划。输入 = synthesis 产出,要求输出 PlanDoc JSON。
pub fn plan_prompt(spec: &CouncilSpec, chair: &MemberSpec, synthesis: &CouncilMessage) -> (String, String) {
    let system = format!(
        "{}你是主席,现在把综合结论结构化为实施计划(Plan)。计划将由真实的 coding agent 执行,必须自足、可验证、依赖关系明确。{}",
        role_header(spec, chair),
        OUTPUT_DISCIPLINE
    );
    let user = format!(
        "基于以下综合结论,输出实施计划:\n\n## 综合结论\n{}\n\n## 任务\n{}\n\n\
         只输出一个 ```json 围栏(不要其他文字),schema:\n```json\n{{\n  \"summary\": \"一段话计划概述\",\n  \"steps\": [\n    {{\"id\": \"s1\", \"title\": \"步骤标题\", \"detail\": \"具体做什么(文件/模块/伪代码级)\", \"dependsOn\": [], \"acceptance\": \"怎样算完成(可检验)\", \"ownerHint\": \"建议负责的成员角色\"}}\n  ],\n  \"openQuestions\": [\"遗留不确定性,每条附消解实验\"],\n  \"acceptance\": [\"整体验收标准\"],\n  \"verification\": [\"验证命令,如 npm run build / cargo test …\"]\n}}\n```\n\
         步骤要求:粒度适中(一个步骤 = 一个可独立验收的变更);dependsOn 只引用已定义的 id;verification 里的命令必须在当前仓库环境可执行。",
        synthesis.content, spec.task
    );
    (system, user)
}

/// Plan JSON 解析失败的修复调用(一次机会;两次失败 run Failed)。
pub fn plan_repair_prompt(chair: &MemberSpec, broken: &str, error: &str) -> (String, String) {
    let system = format!("你是主席 {}。你上一轮输出的计划 JSON 无法解析,请修复后重新输出**只含一个 json 围栏**的结果。", chair.display_name);
    let user = format!("解析错误:{error}\n\n你上一轮的输出:\n```\n{broken}\n```\n\n请输出修复后的完整 JSON(保持约定 schema,只输出 ```json 围栏)。");
    (system, user)
}


/// 修复轮(§4.3 targeted revision):验证失败后,主席根据失败证据修订
/// 计划。要求:只改有问题的步骤(未受影响的保持原 id 原样输出,使已
/// 完成的任务可以保留);每处修改附失败证据的对应关系。
pub fn repair_round_prompt(
    spec: &CouncilSpec,
    chair: &MemberSpec,
    plan: &PlanDoc,
    evidence: &str,
    attempt: u32,
) -> (String, String) {
    let system = format!(
        "{}你是主席。执行验证失败了,你需要**修订实施计划**。规则:\n- 只修改与失败证据相关的步骤;无关步骤保持 id 与内容不变(系统会保留它们已完成的执行结果)。\n- 修改/新增的步骤要针对证据里具体的失败原因。\n- 若失败源于验证命令本身不合理,可以修正 verification 清单并说明理由。{}",
        role_header(spec, chair),
        OUTPUT_DISCIPLINE
    );
    let steps = plan
        .steps
        .iter()
        .map(|s| {
            format!(
                "- [{}] {}(detail: {}; dependsOn: {:?}; acceptance: {})",
                s.id, s.title, s.detail, s.depends_on, s.acceptance
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let user = format!(
        "## 第 {attempt} 次修复尝试\n\n## 当前计划\n{steps}\n\n## 验证失败证据\n{evidence}\n\n## 任务原文\n{}\n\n只输出一个 ```json 围栏,内容为修订后的完整计划(与原 schema 相同)。",
        spec.task
    );
    (system, user)
}

// ---------------------------------------------------------------------------
// M5:能力感知组队 + 分层背书
// ---------------------------------------------------------------------------

/// 自动组队(组队 agent = 候选池中画像分最高者)。输出 FormationPlan
/// JSON;经 parse_formation 校验、用户 UI 审阅后才生效(人在环)。
pub fn formation_prompt(task: &str, profiles: &[crate::council::formation::ModelProfile], organizer: &str) -> (String, String) {
    let system = "你是软件工程团队的组建顾问(Team Formation Agent)。根据任务需要与各模型的**能力画像**配置一个多模型审议团。画像分数只是建议(注意其证据来源:启发式未经实测、探针是单次小样本、历史统计最可信),你要结合任务特征做判断,不要机械按分数排序。".to_string();
    let mut roster = String::from("## 候选模型(画像分 0-10,附证据)\n");
    for p in profiles {
        roster.push_str(&format!(
            "- `{}` — 建议 {} 分\n  - {}\n",
            p.model,
            p.score,
            p.evidence.iter().map(|e| format!("·{e}")).collect::<Vec<_>>().join(" ")
        ));
    }
    let user = format!(
        "{roster}\n## 任务\n{task}\n\n你自己(`{organizer}`)也会被分配角色,可以当主席。\n\n配置原则:\n1. **分层**:能力强的模型(tier=\"lead\")担任主席/综合/架构评审等主导角色;能力较弱的(tier=\"contributor\")承担**简单、可检验**的工作(资料调研、备选方案枚举、边界情况清单等)。\n2. **检验**:contributor 的产出之后会由 lead 逐一背书,只有被认可的才进最终综合——给 contributor 的职责要具体、可检验。\n3. 恰好一个 isChair=true 的 lead 当主席(做最终综合)。\n4. 成员 3-5 个为宜;每个成员的 role(身份)与 assignment(具体职责)写清楚,rationale 说明为什么这样分。\n\n只输出一个 json 围栏:\n```json\n{{\n  \"members\": [\n    {{\"model\": \"...\", \"role\": \"...\", \"tier\": \"lead|contributor\", \"isChair\": false, \"assignment\": \"...\", \"rationale\": \"...\"}}\n  ],\n  \"rounds\": 2,\n  \"notes\": \"组队思路一句话\"\n}}\n```"
    );
    (system, user)
}

/// 分层背书:lead 成员对某个 contributor 的最新产出做认可/否决判定。
pub fn endorsement_prompt(
    spec: &CouncilSpec,
    lead: &MemberSpec,
    contributor: &MemberSpec,
    contributor_output: &CouncilMessage,
) -> (String, String) {
    let system = format!(
        "{}你是 lead 成员,负责**检验**较弱势成员的产出。判断标准:事实与逻辑是否成立、工程上是否可行、是否遗漏关键风险。认可要有依据,否决要给出必须解决的具体问题——不是礼节性盖章,也不是苛刻挑剔。",
        role_header(spec, lead),
    );
    let kind_cn = match contributor_output.kind {
        MessageKind::Proposal => "提案".to_string(),
        MessageKind::Revision => "修订(回应质询后)".to_string(),
        other => format!("{:?}", other),
    };
    let user = format!(
        "## 待检验的产出\n来自 contributor `{}`(角色:{}):第 {} 轮的{}\n\n{}\n\n## 任务原文\n{}\n\n只输出一个 json 围栏:\n```json\n{{\"verdict\": \"endorse|reject\", \"reason\": \"一句话依据或必须解决的问题\"}}\n```",
        contributor.id, contributor.role, contributor_output.round, kind_cn, contributor_output.content, spec.task
    );
    (system, user)
}

/// 解析背书输出:{"verdict":"endorse|reject","reason":...}。
pub fn parse_endorsement(raw: &str) -> Result<(bool, String), String> {
    let fence = raw
        .find("```json")
        .and_then(|start| raw[start + 7..].find("```").map(|end| &raw[start + 7..start + 7 + end]))
        .map(|s| s.trim())
        .unwrap_or(raw.trim());
    let v: serde_json::Value = serde_json::from_str(fence).map_err(|e| format!("背书 JSON 解析失败:{e}"))?;
    let verdict = v.get("verdict").and_then(|x| x.as_str()).ok_or("背书缺 verdict")?;
    let reason = v.get("reason").and_then(|x| x.as_str()).unwrap_or("").to_string();
    match verdict {
        "endorse" => Ok((true, reason)),
        "reject" => Ok((false, reason)),
        other => Err(format!("背书 verdict 非法:{other}")),
    }
}

pub fn kind_name(kind: MessageKind) -> &'static str {
    match kind {
        MessageKind::Briefing => "briefing",
        MessageKind::Proposal => "proposal",
        MessageKind::Critique => "critique",
        MessageKind::Rebuttal => "rebuttal",
        MessageKind::Revision => "revision",
        MessageKind::Question => "question",
        MessageKind::Answer => "answer",
        MessageKind::Endorsement => "endorsement",
        MessageKind::Synthesis => "synthesis",
        MessageKind::Plan => "plan",
        MessageKind::Task => "task",
        MessageKind::Status => "status",
        MessageKind::Evidence => "evidence",
    }
}

// ---------------------------------------------------------------------------
// 可见性计算(orchestrator 用;独立纯函数便于隔离测试)
// ---------------------------------------------------------------------------

/// Phase B 的输入:其他成员**最近一轮**的方案(round 内先看 Revision,
/// 没有则 Proposal)。
pub fn latest_positions_for_review(messages: &[CouncilMessage], reviewer: &str, round: u32) -> Vec<CouncilMessage> {
    let mut out = Vec::new();
    let authors: std::collections::HashSet<&str> = messages
        .iter()
        .filter(|m| matches!(m.kind, MessageKind::Proposal | MessageKind::Revision) && m.from != reviewer)
        .map(|m| m.from.as_str())
        .collect();
    for author in authors {
        let position_round = if round <= 1 { 1 } else { round - 1 };
        // 找该作者 ≤ position_round 的最新 Proposal/Revision。
        let latest = messages
            .iter()
            .filter(|m| {
                m.from == author
                    && matches!(m.kind, MessageKind::Proposal | MessageKind::Revision)
                    && m.round <= position_round
            })
            .max_by_key(|m| (m.round, m.timestamp));
        if let Some(m) = latest {
            out.push(m.clone());
        }
    }
    out
}

/// 一条质询的目标作者集合:`meta.targetAuthors`(数组,编排器写入)
/// → `meta.targetAuthor`(单数,兼容手写)→ `to`。
pub fn critique_targets(m: &CouncilMessage) -> Vec<String> {
    if let Some(arr) = m.meta.as_ref().and_then(|v| v.get("targetAuthors")).and_then(|v| v.as_array()) {
        return arr.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect();
    }
    if let Some(single) = m.meta.as_ref().and_then(|v| v.get("targetAuthor")).and_then(|v| v.as_str()) {
        return vec![single.to_string()];
    }
    if m.to == "all" || m.to == "chair" {
        return Vec::new(); // 广播型没有具体目标
    }
    vec![m.to.clone()]
}

/// Phase C 的输入:针对某作者、某轮评审后尚未回应的全部质询。
/// 「尚未回应」= 该作者在**之后**没有对应 round 的 Revision。
pub fn pending_critiques_for(messages: &[CouncilMessage], author: &str, round: u32) -> Vec<CouncilMessage> {
    let has_revision = messages
        .iter()
        .any(|m| m.kind == MessageKind::Revision && m.from == author && m.round >= round);
    if has_revision {
        return Vec::new();
    }
    messages
        .iter()
        .filter(|m| m.kind == MessageKind::Critique && m.round == round && critique_targets(m).iter().any(|t| t == author))
        .cloned()
        .collect()
}

/// Phase C 需要修订的作者集合(有未回应质询的人)。
pub fn authors_needing_revision(messages: &[CouncilMessage], round: u32) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in messages.iter().filter(|m| m.kind == MessageKind::Critique && m.round == round) {
        for target in critique_targets(m) {
            if !out.contains(&target) {
                out.push(target);
            }
        }
    }
    out.retain(|author| !pending_critiques_for(messages, author, round).is_empty());
    out
}

/// 作者当前最新版本(Phase C 的 own_latest)。
pub fn latest_own_position(messages: &[CouncilMessage], author: &str) -> Option<CouncilMessage> {
    messages
        .iter()
        .filter(|m| m.from == author && matches!(m.kind, MessageKind::Proposal | MessageKind::Revision))
        .max_by_key(|m| (m.round, m.timestamp))
        .cloned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::council::types::{now_millis, tests::sample_spec};

    fn msg(from: &str, kind: MessageKind, round: u32, content: &str) -> CouncilMessage {
        CouncilMessage {
            message_id: format!("m-{from}-{kind:?}-{round}"),
            run_id: "t".into(),
            round,
            from: from.into(),
            to: "all".into(),
            kind,
            reply_to: None,
            content: content.into(),
            timestamp: now_millis(),
            meta: None,
        }
    }

    /// 验收 #3:Phase A prompt 绝不泄露其他成员的提案。
    #[test]
    fn proposal_prompt_isolated_from_other_proposals() {
        let spec = sample_spec();
        let member = &spec.members[0];
        let secret_texts = ["GPT 的秘密提案内容 UNIQUE_MARKER_ALPHA", "DeepSeek 的秘密提案 UNIQUE_MARKER_BETA"];
        let others = vec![
            msg("gpt-reviewer", MessageKind::Proposal, 1, secret_texts[0]),
            msg("ds-math", MessageKind::Proposal, 1, secret_texts[1]),
        ];
        // 即便错误地把他人提案传进来,proposal_prompt 的签名也不接受;
        // 正确调用(不传)时输出必然干净:
        let (system, user) = proposal_prompt(&spec, member, None);
        for t in secret_texts {
            assert!(!system.contains(t) && !user.contains(t));
        }
        assert!(user.contains("任务"));
        // 隔离的另一半:critique 输入只含他人方案,reviewer 自己的方案不混入。
        let reviewer = &spec.members[1];
        let (_, cu) = critique_prompt(&spec, reviewer, 1, None, &others);
        assert!(cu.contains(secret_texts[0]));
        assert!(!cu.contains("我自己的提案")); // 不应出现本人提案字段
    }

    #[test]
    fn latest_positions_prefers_revision() {
        let messages = vec![
            msg("a", MessageKind::Proposal, 1, "a v1"),
            msg("b", MessageKind::Proposal, 1, "b v1"),
            msg("a", MessageKind::Revision, 1, "a v2"),
        ];
        let got = latest_positions_for_review(&messages, "b", 1);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].content, "a v2");
        // round 2 评审看 round 1 的位置(修订发生在 round 1 内)。
        let got2 = latest_positions_for_review(&messages, "b", 2);
        assert_eq!(got2[0].content, "a v2");
    }

    #[test]
    fn pending_critiques_clear_after_revision() {
        let mut c = msg("b", MessageKind::Critique, 1, "问题");
        c.to = "a".into();
        let messages = vec![c.clone()];
        assert_eq!(pending_critiques_for(&messages, "a", 1).len(), 1);
        assert_eq!(authors_needing_revision(&messages, 1), vec!["a".to_string()]);
        let rev = msg("a", MessageKind::Revision, 1, "修订");
        let messages2 = vec![c, rev];
        assert!(pending_critiques_for(&messages2, "a", 1).is_empty());
        assert!(authors_needing_revision(&messages2, 1).is_empty());
    }

    #[test]
    fn parse_endorsement_verdicts() {
        let (ok, reason) = parse_endorsement("```json\n{\"verdict\":\"endorse\",\"reason\":\"依据充分\"}\n```").unwrap();
        assert!(ok);
        assert_eq!(reason, "依据充分");
        let (bad, _) = parse_endorsement("{\"verdict\":\"reject\",\"reason\":\"缺边界分析\"}").unwrap();
        assert!(!bad);
        assert!(parse_endorsement("{\"verdict\":\"maybe\"}").is_err());
        assert!(parse_endorsement("认可").is_err());
    }

    #[test]
    fn endorsement_prompt_addresses_contributor_output() {
        let spec = sample_spec();
        let lead = &spec.members[0];
        let contributor = &spec.members[2];
        let output = msg("ds-math", MessageKind::Proposal, 1, "轻量方案");
        let (system, user) = endorsement_prompt(&spec, lead, contributor, &output);
        assert!(system.contains("检验"));
        assert!(user.contains("轻量方案"));
        assert!(user.contains(&contributor.id));
        assert!(user.contains("endorse|reject"));
    }

    #[test]
    fn plan_prompt_includes_synthesis_and_schema() {
        let spec = sample_spec();
        let chair = spec.members.iter().find(|m| m.is_chair).unwrap();
        let synthesis = msg("chair", MessageKind::Synthesis, 1, "综合:采用 A 方案");
        let (_, user) = plan_prompt(&spec, chair, &synthesis);
        assert!(user.contains("综合:采用 A 方案"));
        assert!(user.contains("dependsOn"));
        assert!(user.contains("verification"));
    }
}
