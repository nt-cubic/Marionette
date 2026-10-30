# Council Mode — Architecture Audit (M0)

> 状态:定稿 (2026-10-31)。本文件是 Council Mode(多模型审议与自治执行)实施前的架构对比审计。
> 参考仓库:oh-my-openagent (OmO) `dev` 分支 pinned commit `e9c5af30d957c9dbde673e24cbb71a05fdb16881`(克隆于本地 `G:\Zcode\omo-ref`,仅作架构参考,不拷贝代码)。
> Marionette 基线:`master` 分支 `d8783d8`(v0.5.4),开发分支 `feature/council-mode`。

## 0. 审计结论(TL;DR)

Marionette 已具备 Council 需要的全部**基础设施**:模型网关(Magpie 引擎)、持久化(`write_atomic` JSON)、事件管线(全局 emit)、ACP 会话生命周期、git 快照。缺失的是**编排层**:结构化审议协议、显式消息总线、状态机、恢复语义。这些全部新建,以 `src-tauri/src/council/` 独立模块落地,不触碰现有 ACP/引擎/存储的语义。

与 OmO 的根本差异决定了借鉴方式:**OmO team-mode 是自治协作团队**(成员是黑盒 LLM session,fire-and-forget,靠文件系统协议跨进程协调);**Council 是结构化审议编排**(orchestrator 是进程内确定性 Rust 状态机,按协议轮次调度模型调用,信息流由 orchestrator 显式构造)。因此 OmO 的跨进程锁、maildir 三态投递、`.delivering-` 预留、pid+instanceId 陈锁回收等机制在 Marionette 单进程模型下**不需要整体移植**,取其语义(幂等、at-least-once、审计)用 `Mutex` + append-only 日志实现。

**明确不做**:第二个 ACP 运行时、第二个模型网关、第二个 transcript 管线。Council 的模型调用直连引擎 HTTP,执行阶段(M2)复用 `AcpService`。

## 1. 能力对照表

| 能力 | OmO 实现(参考) | Marionette 现状 | 决定 | 依据 |
|---|---|---|---|---|
| Team registry | `team-registry/paths.ts:93` scope 解析 + Zod spec 校验 (`loader.ts:102`),成员 eligibility parse 期三档裁决 (`types.ts:190`) | 无;session 由 `storage.rs::create_session` 管理,agent 由 `agent_registry.rs` 静态表 + `custom_agents.rs` 动态发现 | **新建(轻量)** `council/spec.rs`:run 级 spec 校验(成员数 ≥3、模型 id 必须在引擎目录、角色名唯一),不做跨 run 的命名 registry——Council 的"团队"是一次 run 的配置,不是常驻实体 | OmO 的 registry 服务于"团队常驻、多次运行";Council 按任务临时组建 |
| Agent spawning | `team-runtime/create.ts:124` BackgroundManager 池化 spawn + 25ms 轮询等 sessionId + 内存 registry 防 spawn 竞态 | `acp.rs:379 AcpService::start`:完整 spawn/initialize/session/new 管线,`in_flight_prompt_id` 保证一会话一 turn;懒启动 | **复用(仅执行阶段)** M1 不经 ACP——模型 worker 是引擎 HTTP 直调,无进程;M2 执行任务时每 task 临时 spawn ACP session(`create_child_session` + `start_acp_session` 现成) | 后端报告 §11 结论:"council 直连 HTTP,别复用 AcpService"(避免事件进 transcript 管线、避免一会话一 turn 限制) |
| Mailbox messaging | `team-mailbox/send.ts:135`:maildir 三态(`.json`/`.delivering-`/`processed/`),messageId=文件名幂等,32KB 消息上限,256KB 未读背压,lead-only 广播 | 无;前后端有 `acp-event` 流(背压合并管线 acp.rs:5982)但语义是 UI 事件,不是 agent 间消息 | **新建** `council/types.rs CouncilMessage` + `messages.jsonl` append-only 日志。单进程内存即真相源,磁盘日志做 UI 回放 + 崩溃恢复 + 审计。幂等键 `message_id`,去重在 append 前查内存索引 | OmO 的跨进程投递协议(pending/requeue/ambiguous-failure)是为"黑盒 session 无法确认送达"打的补丁;Council 的 orchestrator 自己就是收发双方的管理者,送达是内存事实 |
| Shared task list | `team-tasklist/claim.ts:45`:per-task 文件锁 + 锁内双检 claim,watermark 分配 id,owner-only complete,`blockedBy` 依赖,无 failed 态(失败靠消息) | 无;前端 `lib/todos.ts` + ContextPanel Tasks 卡是 UI 层,后端 `todo.rs` 是 agent 工具宿主 | **新建** `council/tasks.rs TaskGraph`:`Vec<TaskNode>`(id/dependencies/owner/status/artifacts/acceptance),进程内 `Mutex` 保护;`claim` = 状态 CAS(`Pending→Scheduled→Running→Completed/Failed`),依赖就绪检查在调度器内(参考 OmO DAG `senpi-task/src/dag/scheduler-frontier.ts:9` frontier 语义:无上游未完成即可入队,而非严格波次 barrier) | 单进程下文件锁退化为内存锁;OmO "失败的任务只删不标" 的选择对 Council 不适用——我们需要失败任务携带证据进修订回路(M3) |
| State persistence | `team-state-store/store.ts:182 transitionRuntimeState` = state.lock 内 read-modify-write + Zod 再验 + 硬编码转移表;`atomicWrite` = tmp+rename;tolerant-fsync 容忍 Windows EINVAL | `storage.rs:1147 write_atomic`(tmp+rename+.bak 回退)是全库标准;session 存 `sessions/index.json` | **复用 write_atomic 语义** `council/store.rs`:每 run 一个目录 `~/.marionette/council/runs/{run_id}/`,`state.json`(快照,原子写)+ `messages.jsonl` + `transitions.jsonl`(append-only 审计) | Omni state.json 全量重写对 8 成员够用,Council 同量级;消息走 append-only(OmO DAG 引擎的方向)避免重写大文件 |
| Recovery | `team-core/src/resume.ts:16`:creating 30min 超时→failed;active+lead 消失→orphaned;`.delivering-` TTL 10min 回收;worker 全灭→orphaned,部分死→errored | session 恢复 = 冷启动读 index + transcript 重放(`acp.rs` 无 resume RPC);引擎空闲回收有 idle watchdog | **新建** `council/orchestrator.rs resume`:启动时扫 runs 目录,状态为运行态但无活线程 → `Paused`;resume 以**消息日志为准**——某阶段产出消息已存在则跳过该阶段不重调模型(等价 OmO "ack 过的不重投") | "重启不重放已完成副作用"验收项 #7 的直接实现;单进程崩溃场景 = 整进程死,不存在部分 worker 死而 orchestrator 活的中间态 |
| Model routing | `agent-model-matching.md`:四条 profile 泳道、category 模型门、显式 pin 永不覆盖、per-委派 fallback 链、订阅>API 优先 | `engine.rs` Magpie 网关:`catalog_direct_entries()`(engine.rs:2549)全量模型目录(含 contextWindow/images),`model_sources.rs` 来源发现,`model_bridge.rs` 绑定注入 | **复用** 引擎目录即 Council 的模型选择数据源;成员显式 pin 模型 id,不做自动路由(M4 再考虑 capability matching)。同模型多成员合法(验收项 #2) | Marionette 的跨用引擎已解决"哪个 key 能供哪个模型";Council 只需要在目录里选。OmO 的 profile 泳道概念记为 M4 优化项 |
| Planning | `orchestration.md`:planner 并行只读研究、plan-consultant 盲区分析、plan-reviewer 高准确率评审、plan-gate 防越权、worker done 需独立 reviewer 证实 | 无;`acp_plan.rs` 只是 plan 权限 UI 卡片 | **新建** `council/prompts.rs` 五阶段协议(见 §2)——这正是 Council 的核心增量 | OmO 的 plan-gate/verification-gate 概念直接映射为状态机 `PlanReady` 门与 M3 verify 循环 |
| Git isolation | `team-worktree/manager.ts:42` `git worktree add --detach`(但适配层实际只 mkdir);wave 完成后 orchestrator 才合并 | `git_service.rs`:变更追踪 + 私有 index 快照(`begin/end_workspace_snapshot` git_service.rs:523/662),**无 worktree 创建** | **M2 扩展** `git_service.rs` 加 `create_worktree`(git CLI 子进程,零新依赖);M1 不涉及文件修改 | Marionette 已有最难的"内容树快照"部分;worktree add 是薄封装。OmO 的教训(create.ts:76 实际没接上 git worktree)提示 M2 必须端到端实测 |
| Debate protocol | 无直接对应物(team-mode 是协作不是审议;mass-ulw 是 DAG 执行) | 无 | **新建** Phase A–E 结构化协议(独立提案→交叉质询→修订→综合→计划门),orchestrator 强制执行信息隔离与轮次边界 | 这是本任务的独有需求,两个参考系都没有现成答案;OmO 借鉴点是"prompt 层行为契约"(`member-guidance.ts`)——把输出格式约束写进 worker prompt |
| Concurrency bounds | `config.ts:6 max_parallel_members`(1..8 默认 4);bounds 创建时固化进 state(`store.ts:148`) | 无 council 概念;engine 目录查询有"并发线程+总预算"先例(model_bridge.rs:1319) | **新建** `Semaphore`(Mutex+Condvar)+ bounds 固化进 run spec(OmO 的正确决策:运行期不可被配置扰动) | 无 tokio 红线(Cargo.toml:35)下 std 原语是唯一选择 |
| Cost/budget | `max_messages_per_run/max_wall_clock_minutes/max_member_turns` | `provider_usage.rs` 只读用量;引擎 quota 查询(engine.rs:1495) | **新建** `council/types.rs Budget`:请求数/token 估算(字符/4)/墙钟三层,每次模型调用前检查,超限→`BudgetExhausted` 终态 | 引擎 quota 是只读面板数据,不是执行预算;Council 需要自己的硬闸 |
| UI | tmux 布局(Windows 不适用)+ omo-desktop 读文件 | 完整 React 体系:消息时间线(SessionView)、模型菜单(Composer)、活动泳道(ActivityPanel)、生成卡(SpecCard) | **复用** memo-tab-page 模式加全屏 Council 视图(前端报告 §10 方案 A);MarkdownBody 渲染消息;模型选择复用 `libraryGroups` 数据层 | tmux 路线直接排除;Marionette 的会话流 UI 不动,Council 自己的视图避免污染 Clean 主界面 |

## 2. Council 审议协议(新设计,OmO 概念借鉴处标注)

五阶段一轮,`rounds` 可配(默认 2):

```
Phase A PROPOSING    每个成员独立收到任务+自己的角色卡,产出结构化提案
                     (主张/假设/架构/提纲/收益/风险/验证策略)。
                     信息隔离:prompt 只含任务与角色,绝不含他人提案 ← orchestrator 显式构造上下文,天然保证
Phase B REVIEWING    每个成员收到全部提案(署名),对**每个非己提案**产出质询
                     (事实/逻辑/数学/工程缺陷,逐条引用原文)。允许 question 型往返
Phase C REVISING     原作者收到针对自己提案的全部质询,产出修订版(reply_to 逐条回应);
                     旧版本保留为独立消息,永不覆盖 ← OmO "位置即状态"精神的等价物:消息不可变、append-only
Phase D SYNTHESIZING  Chair(可配置,默认第一成员之外的独立角色)读全部消息,
                     产出:采纳的发现、已裁决的分歧(含理由)、**遗留不确定性**(显式列表,不许和稀泥)
Phase E PLAN_GATE     Chair 把综合产出结构化为 PlanDoc:
                     步骤/依赖/验收标准/验证命令/风险。状态机停在 PlanReady 等用户批准
                     ← OmO plan-gate(orchestration.md:136)+ Marionette 权限卡的门语义
```

关键规则:
- **不做多数投票**。分歧靠证据裁决;证据不足 → 计划里写"需要实验 X"而不是制造同意(任务书 §3.3)。
- 轮次:round N 的 Phase C 之后,若还有未裁决分歧且 `round < rounds`,回到 Phase B(带修订版);否则进 Phase D。
- 每条消息是 `CouncilMessage`,`reply_to` 形成回应链;UI 按成员×轮次分栏。

## 3. 状态机

```
Created → Analyzing → Proposing → Reviewing → Revising ─┐
     ▲                                                 │(还有轮次)
     └─────────────────────────────────────────────────┘
Reviewing/Revising(轮次耗尽)→ Synthesizing → PlanReady
PlanReady --用户批准--> Executing → Verifying → Completed
任意运行态 → Paused(恢复回原态) | Cancelled | Failed | BudgetExhausted
```

- 硬编码转移表(`council/state.rs`,Rust enum + `can_transition`,非法转移返回 `Err`——比 OmO 的运行时 Zod 校验更强,编译期穷尽)。
- 每次转移 append `transitions.jsonl`(时间戳+from+to+原因)。
- **执行门**:PLAN 未经用户批准(`PlanReady` 状态)时任何执行调度器入口直接拒绝——验收项 #9。

## 4. 数据布局

```
~/.marionette/council/
├── runs/
│   └── {run_id}/
│       ├── spec.json          # 不可变 run 配置(成员/模型/轮次/预算,创建时固化)
│       ├── state.json         # 可变快照(phase/成员状态/budget usage/task graph),write_atomic
│       ├── messages.jsonl     # append-only,每行一条 CouncilMessage
│       ├── transitions.jsonl  # append-only,状态转移审计
│       └── plan.json          # PlanReady 后的最终计划(M1 终点)
└── index.json                 # run 列表(标签/创建时间/终态)
```

恢复协议:进程启动(或前端请求)时扫描 `runs/`,`state.json` 为运行态但进程内无活 orchestrator 线程 → 置 `Paused` 并 emit 事件;用户点继续 → resume 从 phase 边界重入,已完成 phase 的判据 = 该 phase 的产出消息在 `messages.jsonl` 中齐全(等价 OmO consumed-ledger 语义)。

## 5. 模型 Worker(M1 核心)

- `council/worker.rs`:`trait ModelClient { fn complete(&self, model: &str, system: &str, user: &str) -> Result<WorkerOutput, WorkerError> }`
- 生产实现 `EngineClient`:ureq POST `http://127.0.0.1:{port}/v1/chat/completions`(端口现查 `engine::running_base()`,禁缓存——端口会漂移),`Bearer magpie`,非流式,90s 超时(比 memory.rs:650 的 60s 放宽,审议回复长)。调用前 `engine::ensure_bridge_ready_if_installed` 唤醒。模板 = `memory.rs::distill_with_deepseek`(后端报告确认的唯一后端直调先例)。
- 测试实现 `MockClient`:注入脚本化响应(按 model id 区分——验收项 #1 断言不同模型收到不同请求)。
- Worker 失败:单成员失败重试 1 次后标记 `Failed` 并继续其他成员(验收项 #6:单个 worker 崩溃不毒化整个 run);全员失败 → run `Failed`。

## 6. 并发与预算

- fan-out:每阶段 N 成员 → `std::thread::spawn` × N,`Semaphore`(max_concurrent,默认 4)闸门,`mpsc` 收结果。
- 取消:`run.cancel = AtomicBool`;orchestrator 在每次调度前、worker 在 HTTP 超时后检查;取消后不再发起任何模型调用(验收项 #8),状态 → `Cancelled`。
- 预算:`Budget { max_requests, max_tokens_est, wall_clock }` 固化进 spec.json;usage 每次 call 后累加进 state.json;超限 → `BudgetExhausted`(终态,不重试)。

## 7. 前端(复用清单)

| 需求 | 复用 | 新建 |
|---|---|---|
| 入口 | ProjectShelf 侧栏收纳盒入口旁(App.tsx:8386 prop 模式)、CommandPalette action | `councilOpen` 状态 + 图标 |
| 视图容器 | memo-tab-page 模式(App.tsx:8654 条件链) | `CouncilPage.tsx` |
| 消息渲染 | `MarkdownBody`、`ClippedBody` | Council 消息卡(成员色条+模型徽标+轮次) |
| 模型选择 | `useModelLibrary` + `libraryGroups` + 引擎目录 | 成员配置行 |
| 事件 | `listen("council-event")` 范式 | — |
| 任务图(M2) | mermaid 懒加载管线(`MdMermaid`)或手写 SVG(仿 ActivityTimeline) | — |

## 8. 许可与归属

- OmO(`Sustainable Use License`)仅作架构参考,零代码拷贝。本审计中所有"借鉴"均为语义/协议层面(幂等键、append-only、bounds 固化、frontier 调度),实现从零按 Marionette 的 Rust/std-thread 模型写。
- Marionette 保持 MIT。Council 不引入任何新 crate(零新依赖,Cargo.toml 体积红线不动)。

## 9. 测试策略(M1)

1. Rust 单测(`cargo test --bin marionette council`):
   - 状态机:合法/非法转移全表、执行门(PlanReady 前无执行调度)
   - 消息:append 幂等(重复 message_id 不重复入账——验收项 #5)、reply_to 链完整
   - 信息隔离:Phase A prompt 构造函数对成员 X 的输入绝不含成员 Y 的提案文本(验收项 #3)
   - 路由:Phase B 质询消息 from→to 与提案作者映射正确(验收项 #4)
   - Mock fan-out:3 个不同 model id 各自收到请求、响应归位(验收项 #1);同模型 2 成员各自独立请求(验收项 #2)
   - 恢复:写盘 → drop 内存 → 重新加载 → phase/消息/预算一致,已产消息不触发重调(验收项 #7)
   - 取消:fan-out 中途置 cancel → 不再发起新调用(验收项 #8)
   - worker 失败注入:1/3 失败 → run 继续,2 提案 + 综合仍成(验收项 #6)
2. 前端回归:`scripts/verify-council.mts`(进 `verify-all.mjs` 聚合):纯逻辑模块(phase 顺序、消息分组、成员配色)离线可测。
3. 真模型冒烟(有引擎环境时手动):3 模型小任务跑通全协议。
4. 既有回归不动:`npm run verify` 全量 + `cargo test --bin marionette` 全量必须保持绿(验收项 #12)。

## 10. 决策记录(ADR 摘要)

| # | 决策 | 备选 | 理由 |
|---|---|---|---|
| 1 | 模型 worker 直连引擎 HTTP | 经 ACP agent 中转 | 后端报告:ACP 事件会进 transcript 管线、一会话一 turn 限制、需 spawn 进程;直调零开销且隔离 |
| 2 | orchestrator = 进程内状态机线程 | OmO 式文件系统协调 | 单进程;文件协议的复杂度全部来自跨进程,不需要 |
| 3 | 消息 append-only JSONL | maildir 三态目录 | 进程内内存是真相,磁盘只需回放/恢复/审计;OmO 三态是为不可靠投递设计 |
| 4 | bounds/spec 创建时固化 | 运行期可改 | OmO store.ts:148 的验证过的正确决策 |
| 5 | 零新 crate | 引入 uuid/chrono 等 | Cargo.toml 体积红线;uuid 用已有引擎目录 id/时间戳组合足够 |
| 6 | Council 视图独立成页 | 混入会话流 | 不污染 Clean 主界面;memo-tab-page 模式已验证 |
| 7 | M1 终点 = PlanReady(不动文件) | 一路做到执行 | 任务书 M1 边界;执行(M2)有独立验收项 |
