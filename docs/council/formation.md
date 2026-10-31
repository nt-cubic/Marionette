# 能力感知组队与分层背书(M5)

> 状态:已实现(2026-10-31)。让 AI 参与团队设置:先画像、再组队、
> 再分层检验。本文记录方法依据、机制与边界。

## 1. 方法依据(相关工作)

本功能是四个已验证方向的组合,不是发明新协议:

| 方向 | 代表工作 | 采借的机制 |
|---|---|---|
| 动态组队 | [DyLAN](https://arxiv.org/abs/2310.02170)(Dynamic LLM-Powered Agent Network);[AgentVerse](https://arxiv.org/abs/2308.10848)(ICLR 2024)的 expert recruitment | 按任务与 agent 能力**选团队**而非固定名单;组队本身就是一次模型调用 |
| 分层稳健性 | 多智能体韧性研究:层级结构(boss+peers)在扰动下损失 ~5%,链式 ~24% | 强模型当 lead/chair 主导综合,弱模型不做关键裁决 |
| 强弱路由 | [RouteLLM](https://arxiv.org/abs/2406.18665) | 难任务升给强模型;**简单可检验**的任务(调研/枚举)留给弱模型 |
| 背书/验证 | 层级化 multi-agent debate 的 verifier 模式;Google MAD 的多 verifier 评估 | 弱模型(contributor)的产出须经 lead **显式背书**才被主席采信 |

与"多数投票"的区别:背书是**层级化的质量闸**,不是计数——一个 lead 的
有据否决足以把 contributor 的主张挡在结论外,并把理由留痕。

## 2. 机制

### 2.1 模型能力画像(三信号源,成本递增)

`src-tauri/src/council/formation.rs`:

1. **静态启发** `model_family_rank`:模型 id 的家族/档位关键词
   (opus/ultra/max/reasoner → 9;mini/flash/haiku/lite → 4;pro/plus/v4
   → 7)。识别不了返回**未知**——不猜,UI 显示证据来源。
2. **本地探针** `probe_model`(用户显式触发):每模型 **1 次**小请求
   (一道短推理 + 严格 JSON 格式遵从),评分 0-7(答对 +4、格式 +3、
   错答但格式合法 +1)。结果存 `council/probes.json`(最近一次语义
   ——模型会升级,旧探针不永久粘滞)。
3. **历史统计** `model-stats.json`(反馈闭环):每个 run 到
   `plan_ready` 时落账各模型的 产出数 / 收到背书(认可·否决)/ 失败
   数。经验分需 ≥2 个 run 才生效(样本太少不算),背书率主导、失败
   率惩罚。**越用越准**,上限 200 条 LRU。

综合分 = 三信号加权(启发 0.5 / 探针 1.0 / 历史 1.5),每条证据
以人话进 UI(「家族档位启发:9/10(未经实测)」「本地探针:7/7
(推理正确)」「历史:5 run · 背书认可 8/9」)。

### 2.2 自动组队(AI 参与设置)

`council_auto_formation` 命令:候选池中**画像分最高**的模型担任
「组队 agent」(Team Formation Agent),读任务 + 全部画像,产出
FormationPlan JSON(成员/角色/tier/主席/具体职责/理由/轮次)。
经 `parse_formation` 硬校验(模型必须在池内、成员 2-8、恰好一个
主席且为 lead、至少一个 lead),**UI 审阅可改后才应用**——人在环,
AI 只建议。

配置原则写进组队 prompt:强模型当主席/综合/架构评审;弱模型承担
简单、可检验的工作(调研、备选枚举、边界清单);contributor 的
职责必须具体可检验(因为下一步就是被背书检验)。

### 2.3 分层背书协议(tiered endorsement)

`MemberTier { lead, contributor }`(缺省 lead——**旧 run 与旧配置
行为完全不变**,全员 lead 时背书整段跳过、零额外调用)。

流程:进入 `synthesizing` 后、主席综合**之前**——

```
for 每个 contributor 的最新产出(Proposal/Revision):
    for 每个非主席 lead:
        endorsement_prompt(待检验产出 + 判定标准)
        → {"verdict":"endorse|reject","reason":"..."}
        → Endorsement 消息(meta: targetAuthor/targetKind/verdict/byLead)
```

- 背书失败(单条调用挂)不毒化 run:记 Status 消息,该产出按
  「零背书」进综合——主席的采信规则兜底。
- 恢复语义:(lead, contributor, round) 的背书已在消息账上则不重调。
- **主席综合的输入**给每份 contributor 产出标注
  「〔lead 背书:认可 N / 否决 M〕」,并注入分层采信规则:
  **认可 ≥1 才可直接采纳;全部被否决或零背书的主张不得进入结论**,
  其有价值但未获认可的部分进 open questions 并注明原因。
- 审计:每条背书是 append-only 消息,UI 时间线新增「背书」分组,
  `model-stats.json` 据此滚动更新画像——**检验结果反馈回组队**,
  闭环。

## 3. 使用路径

设置页新增「AI 智能组队」区:勾选候选池 →(可选)「本地探针」→
「AI 组队」→ 审阅建议(每人 tier/职责/理由 + 画像证据)→「应用此
组队」→ 仍可手动微调(每行有 分层 下拉)。运行页成员条显示
tier 徽标;时间线出现「背书」分组。

## 4. 边界与诚实标注

- 画像三信号都是**建议性证据**:静态启发未经实测、探针是单次小样本
  (1 道题)、历史统计要 ≥2 run 才参与计分。UI 全程展示证据来源,
  用户可全部手改。
- 背书不改变信息隔离:Phase A 独立提案互相不可见的语义不变。
- tier 不影响执行阶段(harness 无模型之分);只作用于审议协议。
- 同一模型可同时作为 A run 的 lead 与 B run 的 contributor——分层是
  **成员属性**不是模型属性(与"同模型多成员"的正交性保持)。

## 5. 测试

`cargo test --bin marionette council`(53 项,M5 新增 10):

- `formation::family_rank_covers_known_tiers` — 启发表命中与诚实未知
- `formation::probe_scoring_correct_and_malformed` — 探针评分三档
- `formation::formation_parse_validates` — 池外模型/主席非 lead/无主席/成员数全拒
- `formation::profile_blends_signals_with_evidence` — 加权与证据文案
- `formation::auto_formation_uses_strongest_as_organizer` — mock 端到端
- `orchestrator::tiered_endorsement_flow` — 端到端:背书消息 meta 齐备、
  综合输入带「认可 1」标记、历史统计落账
- `orchestrator::all_lead_config_skips_endorsement` — 旧配置零背书(兼容)
- `orchestrator::rejected_contributor_output_is_flagged` — 否决路径可见
- `prompts::parse_endorsement_verdicts` / `prompts::endorsement_prompt_addresses_contributor_output`


## 6. 身份局真对局(M5.2)

让身份局模板的成员**真打一局**简化三国杀,而不只是角色扮演审议。

### 方法依据

LLM 玩隐藏身份博弈已有成熟范式:[Werewolf 实证研究](https://arxiv.org/abs/2309.04658)、
[AvalonBench](https://arxiv.org/abs/2310.21678)、Avalon 多轮涌现研究等——
**确定性引擎持有全部真相,LLM 玩家只拿私有视角观测(自己的手牌与身份 +
公开信息),决策以结构化 JSON 回传,引擎逐张校验结算,非法行动跳过留痕**。
我们的实现与圆桌的信息隔离思想同源。

### 机制(`council/game/`)

- `engine.rs`:纯逻辑引擎(零 IO,单测 11 项)——75 张标准比例牌堆
  (杀30/闪15/桃8/决斗3/无中4/南蛮3/万箭1/拆6/顺5)、环状座次与距离
  (杀/顺手限相邻)、回合结算(摸2→出牌→弃至体力)、濒死自动桃、
  杀反贼奖摸3、胜负判定(主公死→内奸独活则内奸胜否则反贼胜;反贼内奸
  全灭→主忠胜)、回合上限 12 与弃牌堆重洗;
- `player.rs`:私有视角构造(**他人手牌只有数量、身份只有主公亮明**,
  有专项测试锁死信息隔离)+ 决策 prompt(整回合计划 JSON:plays/discard/
  say,发言可演戏但出牌要服务真实胜利条件)+ 解析(plays 非数组判格式
  错误,缺 plays 合法=过牌);
- `runner.rs`:对局循环(复用 ModelClient seam / council-event / 取消 /
  预算),每回合一次模型调用,决策失败自动过(游戏永不卡死);
  game-state.json 全量落盘,进程重启后从当前局面续打。

### v1 规则简化(诚实清单)

无武将技能(白板)、无装备、无延时锦囊、无无懈可击;**防御响应
(闪/决斗出杀/濒死桃)由系统自动打出**(有牌即出);桃仅自救;
只支持 4-5 人局。这些简化让每局模型调用稳定在「每玩家每回合 1 次」,
成本可预算;后续版本可逐项引入交互式响应。

### 状态机与集成

`RunPhase::Playing`(Created→Playing→Completed/Cancelled/Failed/
BudgetExhausted/Paused);`CouncilMode::SgsGame`(校验:4-5 成员);
`MessageKind::GameAction`(开局公告/行动/发言/终局身份揭示全进
append-only 消息账本);RunDetail 附带游戏快照(体力/手牌数/身份亮暗)。

### 测试

引擎 11 项(发牌/距离/杀规则/闪桃链/身份胜负/AOE/非法行动留痕/重洗/
回合上限)+ 玩家 3 项(信息隔离/视角/解析)+ 端到端 2 项(mock 整局到
Completed 且账本含开局·发言·终局揭示;对局中取消冻结调用)。
