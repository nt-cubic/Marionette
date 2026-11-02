# 圆桌：收敛终止与幻觉治理 — 实施方案（草案）

> 依据 [`research-council-convergence-hallucination.md`](./research-council-convergence-hallucination.md)
> 的现状审计与文献调研。**本文档是方案，不是已实现行为的描述**；落地时按 P0→P1 分批，
> 每批带回归（`cargo test --bin marionette council` + `npx tsx scripts/verify-council.mts`）。

## 现状一句话

辩论本身是真的（独立提案 → 交叉质询 → 修订 → 主席综合，star 拓扑）。
断点有二：**Rebuttal 通道定义了没接线**（少数派异议无处留痕）、
**计划步骤不带来源字段**（主席采纳的断言无法追溯）。

## P0-1 接线 Rebuttal（异议留痕）

- `MessageKind::Rebuttal` 已存在，语义是"对质询的反驳（不修订原文时）"。
- 编排器当前零引用 → 在 `Revising` 分支里，若成员对某条质询既不修订也不撤回，
  落一条 `Rebuttal` 消息（`meta.targetAuthors` 沿用 `critique_targets` 的形状）。
- 关键约束：**Rebuttal 必须进 synthesis 的输入流**，并在 prompt 里点明
  "以下异议未被作者接受，你必须在综合结论中显式处置（采纳 / 保留为反对意见 / 列为未决）"。
- 验收：新增测试——成员反驳后不修订时账本里有 Rebuttal，且综合输入包含它。

## P0-2 综合三段式 + 强制溯源

- `synthesis_prompt` 的输出格式从四节改为**三段式 + 溯源**：
  1. `[已采纳]` 每条带 `← 成员 id` 与依据出处；
  2. `[未采纳但保留的反对意见]` 每条带提出者 id 与未采纳理由；
  3. `[未决分歧]` 每条附消解实验。
- 硬规则写进 prompt：**主席不得引入任何成员提案中未出现的断言**（切断
  Genesis→Elaboration→Citation Chain 的传播链）。
- 事后检查（轻量，纯字符串）：综合里的文件路径/符号名若从未出现在任何 Proposal/Revision 的
  正文里 → 标 `unverified` 并在 UI 提示。不做语义判断，只做**是否出现过**的可枚举检查。

## P0-3 收益衰减提前终止

- 纯字符串实现（零新依赖）：每轮结束算 `positions[r]`（各成员规范化结论串：去空白、小写）。
- 若 `jaccard(positions[r], positions[r-1]) > 0.9` **且** 本轮无新 `Rebuttal`/`Endorsement`
  → 判定无信息增量，直接转 `Synthesizing`（并在 transitions 里记原因，保持可审计）。
- 位置：`orchestrator.rs` 现有 `needs_revision && more_rounds` 判据处加一个
  `has_new_information(&messages, round)`。
- 注意：**不要**用它去"追求共识"——它只回答"还有没有新东西"，答"没有"就停，
  而不是答"大家一致了"才停（文献：收敛到一致 ≠ 收敛到正确）。

## P1-1 确定性符号核验（grounding）

- 新 `council/grounding.rs`：对计划步骤/综合结论里出现的文件路径、`fn`/符号名、命令，
  先用文件系统与 `rg` 硬查存在性。
- 查不到 → 标 `unverified` 进 open question，**不让模型自己判真假**
  （LLM 引用捏造率 78–90%，而确定性工具在桌面环境是免费的）。
- 只覆盖可枚举断言；语义类断言仍走质询/背书流程，不强行工具化。

## P1-2 / P1-3 / P2（见调研文档表格）

- Isolation guard（孤立立场者获"重申或放弃均需理由"注入）——直击 conformity。
- 置信度字段 + ReConcile 阶梯加权 `f(p)=[1.0,0.8,0.5,0.3,0.1]`。
- P2：质询改「构造反命题」；ABSTAIN 弃权通道（弃权不惩罚，否则逼出幻觉）。

## 明确不做

- 不把"轮次"当质量开关（加轮次常见地降低精度；默认 2 正确，UI 上第 3/4 轮标"实验性"）。
- 不改 star 拓扑为 full mesh（成本 5 倍而精度相当）。
- 不用 LLM 作为符号/API 存在性的唯一裁判。

## 分批与验收

每批独立可合：P0-1 + P0-2 一个 PR（prompt + 编排器 + 测试），P0-3 一个 PR（终止判据），
P1-1 单独一个 PR（新模块，改动面最大）。验收口径：
`cargo test --bin marionette council`（现有 61 项不得回归）
+ `npx tsx scripts/verify-council.mts` + 每批补自己的用例。
