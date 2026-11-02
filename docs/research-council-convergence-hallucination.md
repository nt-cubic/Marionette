# 圆桌会议的收敛与幻觉治理：现状审计 + 调研方案（2026-11-02）

> 结论先说：**多模型辩论确实已实现**（不是串讲），但有两处真实断点——
> ①「反驳但不修订」这条异议通道定义了却从未接线；②计划步骤不带来源/证据字段，
> 主席综合出的断言无法追溯是谁说的、依据是什么。
> 本文是**调研与方案**，改动建议按 P0/P1/P2 排序，未实施。

## 一、现状审计（对 `src-tauri/src/council/` 的实读结论）

### 确实已实现的「agent 之间讨论」

`orchestrator.rs::run_loop` 是一个真正的辩论状态机，不是"各说各话再拼接"：

- **Phase A 独立提案**：`proposal_prompt` 的签名只收自己该看的内容，编排器强制信息隔离
  （测试 `proposal_prompt_isolated_from_other_proposals` 双向锁死）。
- **Phase B 交叉质询**：每个成员拿到**其他成员的最新立场**（`latest_positions_for_review`）
  逐份挑错——这是 agent 读 agent 的输出，是讨论的定义性特征。
- **Phase C 修订**：被质询者拿到针对自己的质询逐条回应并出新版本（`revision_prompt`），
  旧版本以独立消息保留（append-only，历史不覆盖）。
- **轮次推进**：`Reviewing → Revising → Reviewing(round+1)`，直到 `round >= rounds`。
- **Phase D 主席综合**：输入是全部署名发言流。

拓扑是 **star-like**（每人评审他人最新立场），而不是 full mesh——正好符合文献结论：
N=5/R=3 时 full-mesh 需要 60 次 critique 而 star 只需 12，精度相当。**不要改成两两互评。**

### 已有的防护（写得比预期扎实，别重复造）

| 防护 | 位置 |
|---|---|
| `rounds ≤ 4` 硬上限（默认 2） | `types.rs:377` |
| 预算三层封顶（requests / tokens / wall clock），触顶即停 | `orchestrator::budget_exhausted` |
| 质询必须**引用原文**、「证据不足时明确说无法验证」而非猜 | `critique_prompt` |
| 主席**不许强行统一**分歧，证据不足进 open questions | `synthesis_prompt` |
| 提示注入红线：外部内容一律是数据 | `INJECTION_GUARD`（每阶段 system 都带） |
| 分层背书（contributor 产出需 lead 认可才可直接采纳） | M5 `Endorsement` |
| 提前收敛判据：无人需修订 → 直接进综合 | `authors_needing_revision` |

### 两处真实断点

1. **`MessageKind::Rebuttal` 是死通道**：类型已定义（"对质询的反驳，不修订原文时"），
   但 `orchestrator.rs` 中**零引用**（`prompts.rs` 只在文案里出现 2 次）。
   后果：成员若不同意质询但也不改立场，这条异议**进不了账本**，主席综合时看不到——
   这正是文献里 consensus collapse 的温床（少数派没处留痕，就被当成"已解决"）。
2. **`PlanStep` 没有来源/证据字段**：`{id, title, detail, depends_on, acceptance, owner_hint}`
   里没有一个字段记录"这条是谁说的、依据是什么"。合成阶段是自由文本
   （`MessageKind::Synthesis` 无 schema 校验，只有 `Plan` 走 `PlanDoc` 解析）。
   后果：幻觉一旦被主席采纳，就**带着权威外衣进了执行计划**，且事后无法审计。

## 二、用户担心的两个问题，文献怎么说

### 问题 A：「AI 会不会不自觉一直聊」

**不会失控（硬边界已存在），但会"空转"且可能越聊越差。**

- 收益集中在第 1–2 轮：Du et al. (ICML 2024, [arXiv:2305.14325](https://arxiv.org/abs/2305.14325))
  的规范配置就是 3 agents × 2 rounds；MADIAVE（EACL Findings 2026）观察到**到第 3 轮
  "变差的案例数开始超过变好的"**；熵视角研究显示 R1→R2 骤降、R2→R5 几乎持平。
- **最该警惕的不是"聊太久"，是"收敛到一致 ≠ 收敛到正确"**：
  *Talk Isn't Always Cheap*（[arXiv:2509.05396](https://arxiv.org/abs/2509.05396)）实测
  3×Mistral 在 MMLU 上两轮后 **33.6% → 24.4%**；机制是 correct→incorrect 的翻转压倒
  incorrect→correct，且**孤立无援的成员最容易放弃正确答案**。
- 对我们有利的一点：该研究同时发现 **centralized（强编排器 + 主席综合）架构的抗退化能力
  明显强于 peer-based**——圆桌正是这个形状。

→ 所以方向不是"限制轮次"（已有），而是**检测无信息增量就提前停** + **保护少数派意见**。

### 问题 B：「有些模型容易出幻觉」

**共识会放大幻觉，而不是抑制它。** 2025–2026 的工作几乎一致放弃了"同伴同意 = 正确"：

- ReputAgent 的四阶段传播模型：*Genesis*（一个 agent 编造）→ *Acceptance*（下游当既定事实）
  → *Elaboration*（细节被添油加醋）→ *Citation Chain*（多人引用这个"已确立"的事实）。
- MARCH 的关键发现：**把结论和文档同时给 verifier，verifier 会陷 confirmation bias**，
  优先内部自洽而非来源接地——**不要让验证者先看到结论再验证**。
- LLM 引用捏造率高达 78–90%（CiteGuard），而 LLM-judge 在词汇不匹配时召回率低至 16%
  → **绝不能用 LLM 当唯一 verifier 去查 API/符号是否存在**。

→ 对桌面应用的我们有个天然优势：**证据源是免费的**（grep / 符号索引 / 文件系统），
  代码类断言可以降级成**确定性检查**，把概率问题变成可枚举问题。

## 三、方案（按性价比排序）

| 优先级 | 改动 | 落点 | 成本 |
|---|---|---|---|
| **P0-1** | **接线 Rebuttal 通道**：成员可"不接受质询也不改立场"，该异议进账本并在综合 prompt 中显式呈现 | `orchestrator.rs` + `prompts.rs` | 低（类型已存在） |
| **P0-2** | **综合三段式 + 强制溯源**：输出 `[已采纳(带 from_member_id)] / [未采纳但保留的反对意见 + 提出者] / [未决分歧]`；主席不得引入任何成员提案中未出现的断言 | `synthesis_prompt` + 事后检查 | 极低 |
| **P0-3** | **收益衰减提前终止**：连续两轮立场签名 Jaccard > 0.9 且无新 Rebuttal/Endorsement → 直接进综合（纯字符串，零新依赖） | `orchestrator.rs` 终止判据处 | 低 |
| **P1-1** | **确定性符号核验**：计划步骤里的文件路径 / 符号 / 命令，先用 `rg`/文件系统硬查；查不到标红进 open question，不让模型自己判真假 | 新 `council/grounding.rs` | 中 |
| **P1-2** | **Isolation guard**：某立场支持数为 0 时，下一轮给该成员注入"你是唯一持此立场者，重申或放弃都需给出理由"——直击 conformity | `critique_prompt` | 极低 |
| **P1-3** | **置信度字段 + ReConcile 阶梯加权**：成员输出 confidence，聚合用 `f(p)=[1.0, 0.8, 0.5, 0.3, 0.1]` 重标定（治 LLM 普遍虚高），替代朴素多数投票 | `types.rs` + 聚合函数 | 低 |
| P2-1 | 质询改为「构造反命题」：无法构造可证伪反命题的断言标 `unverifiable`，在计划里降级为待人工确认 | `critique_prompt` | 低 |
| P2-2 | ABSTAIN 弃权通道：全员弃权 → 主席必须输出 `OPEN_QUESTION` 而非猜；弃权不惩罚（否则会逼出幻觉） | `types.rs` / 编排器 | 低 |

### 两个明确不要做的

- **不要把"轮次"当质量开关**——文献显示加轮次常见地**降低**精度；默认值 2 是对的，别鼓励用户开到 4。
- **不要改成 full mesh / 两两互评**——star 拓扑成本 1/5 而精度相当。

## 四、证据强度说明

稳固锚点：Du et al. (ICML 2024)、ReConcile (ACL 2024)、*Talk Isn't Always Cheap*、
*Stop Overvaluing Multi-Agent Debate*。
2026 年的几条（熵视角、M3MAD-Bench、MARCH、CSMAD）多为预印本或二手聚合页，
**尚未完整同行评议**，据此做 P0 决策时应以"低成本可回退"为前提——上表 P0 三项都满足。
