# T3 Code 借鉴实施路线图（2026-11-27）

> **状态**：待开工。本稿把 [`research/t3code-feature-audit.md`](../research/t3code-feature-audit.md)（下称「审计」）
> 的四桶决策收敛成**可以在家按批次推进**的工程计划：每个阶段有清单、有文件落点、有验收命令、有失败路径。
>
> **上游依据**：审计 §0–§6（判断与四桶）、§2（工程纪律）、§5（adopt/adapt/defer/reject）、§8（未验证清单）。
> **现行契约**：行为一旦落地，写进 [`docs/CURRENT.md`](../CURRENT.md)；本文件只保留计划与批次边界。
> **工作量口径**：S ≈ 半天内，M ≈ 1–2 天，L ≈ 一个迭代以上（单人、熟悉本仓）。

---

## 0. 一页速览

| 问题 | 结论 | 证据 |
|---|---|---|
| T3 值得抄什么 | **不是功能条数，是四条骨架纪律**：能力=服务方法、副作用后置、能力声明诚实、线缆有预算 | 审计 §0、§2.2、§2.3、§2.8 |
| 最贵的债在哪 | 前端 `performSend` 与 `chief/runner.rs` 两套发送/回合管线必须手工对齐 | 审计 §5.1 A1、§7.4 |
| 最先做哪件 | **Phase 0：子代理可观测性**（四个 worker 真跑了，界面只看到约 1 条 thought） | 审计 §0.1 |
| 最容易白费劲的 | 追 T3 的远程/多端/PR 平台/Workflows——形态不同，做了就错 | 审计 §5.4 |
| 护城河 | 跨用引擎、贴边常驻、Computer Use（+ 圆桌、插件层、派活账本） | 审计 §0、§4 第 24/31/40 行 |
| First user-visible wave | S0, then P1-3/P1-1, then S1-S4, then P1-2/P1-8; see section 14 | Sections 11 and 14 |

**一句话**：先把「多 agent 干活时我看得见」修好，再把每天要按的四个键（发送、排队、审批、撤销）修顺，
然后才动回合服务与 ACP 连续性这种结构性大改。

---

## 1. 目标、产品定位与不做的事

### 1.1 产品定位（本路线图的前提）

Marionette 是 **Windows 单机、本地驻留的多 Agent 驾驶舱**：一个 Tauri 2 + Rust 壳，加一层跨 agent 的
模型/密钥/订阅互通引擎。它不承载 server、不做多端，执行永远发生在这台机器上，在用户已经登好的 agent CLI 里。

T3 是**控制平面**：执行发生在持有工作区的 environment，Web/桌面/手机是认证客户端。两边不是同类替代品，
是两个正交的轴。所以本路线图只借「工程骨架」，不借「产品形态」。

### 1.2 目标（按优先级，不是愿望清单）

1. **多 agent 的工作过程可信、可见、可恢复**——worker 跑过什么，用户必须能看见（Phase 0）。
2. **每天按的那几个键不添堵**——发送/排队/审批/撤销（Phase 1）。
3. **回合生命周期只有一份实现**——UI、Telegram、员工模式、AI 派活不再各自拼 ACP 调用（Phase 2）。
4. **代码交付从「让 agent 尽量做对」升级成确定性工具链**——终端、定位、批注、提交、按轮回退（Phase 3）。
5. **保留并强化三个核心差异**：跨用模型、贴边常驻、Computer Use。
6. **Daily workflow friction is a product defect.** Once observability is trustworthy, prioritize the
   T3-only UX gaps catalogued in section 14: triage, attachments, paste folding, references, startup,
   onboarding, browser/diff controls, focus, and usage-aware delivery.

### 1.3 明确不做（做了就是错，不是「暂时不做」）

- **不做云端身份与中继**：T3 Connect / Clerk / relay / Guest links / peer links / 多机负载均衡。
  远程的正确答案已经是 Telegram（纯出站、零新依赖）。
- **不做多端**：iOS / Android / 托管 Web App / 远程推流浏览器 / preview tunnel。
- **不做 PR 平台层**：9 家托管商、Issue、PR watching、Review guide、栈式合并。
  我们的交付语义是「本轮改了什么 + 能不能退」，不是「把改动送去评审」。
- **不做耐久编排平台**：Workflows、Webhook、跨 environment 编排、企业 managed policy、产品遥测 ingest。
- **不做多仓 worktree 扇出**（`worktreeLayout` 脚本一类），只做单仓、可选、一个（Phase 5A）。
- **不复制技术栈**：Effect / Atom / 三端共享 runtime / SQLite 事件溯源。抄原则，不抄代码。
- **不把仓库里声明的脚本自动跑起来**：项目动作只读 `~/.marionette` 下的项目配置，不读仓库内 checked-in 脚本。

### 1.4 关于「roster」：T3 没有员工产品面

T3 的代码与文档里会出现 `roster`，它指的是**后台任务 / 子代理 / pending work 的运行时列表**，
不是招聘、职责、承诺、员工记忆。T3 **没有**员工模式、员工花名册、招聘、职级、承诺与员工记忆这一整套产品面。

本路线图沿用这个区分：**我们在 T3 侧借的「roster」只指后台任务与子代理的可观测性**（Phase 0），
Marionette 自己的员工模式（`docs/plans/plan-employee-mode.md`）继续作为差异化能力维护，
不因为「对齐 T3」被稀释或改造。

---

## 2. 工程原则（先于一切条目）

这六条不是口号，每条都有本仓已经付过学费的反面案例。

1. **先观测，再并行。**
   在扩大并行派活之前，worker 的工具调用、正文进展、当前步骤、最终摘要必须先在对话里看得见。
   反面案例：四个 worker 各跑了 127/492/84/89 次工具调用，界面只有约 1 条 thought——我们连「哪里丢了」都说不出。

2. **先转发，后收口（forward-only）。**
   统一回合服务**第一步只做转发层**：把调用原样送到现有实现，不新增第二份判断、不做行为改写、不设双开关。
   入口按会话逐个迁移，迁完一个删一个旧路径。反面案例：桥接档位「假成功」、账号模型「静默降级」，
   都是同一件事有两份判断的结果。

3. **能力必须诚实。**
   agent 不支持的 steer / rollback / 权限档 / resume，直接禁用并**写清原因**；不模拟、不代答、不假装成功。
   反面案例：Grok 无菜单模型发档位被 -32602、Codex 裸内建名落回订阅源烧额度。

4. **一处写者。**
   `src/app/App.tsx`、`src/components/Composer.tsx`、`src-tauri/src/acp.rs`、`src-tauri/src/main.rs`
   同一时间只允许一个实现代理修改；共享的 `storage.rs`、`git_service.rs` 同一阶段也归一个代理。
   其余文件按目录边界并行。

5. **文档先行。**
   每个阶段先写「行为契约 + 失败路径 + 用户看到什么」，再写代码。契约进 `docs/CURRENT.md`（现行）
   或 `docs/guides/`（规范）；**改动是重写那一段，不是追加第二份说明**（审计 §2.9 的 rewrite-not-append）。

6. **证据化验收。**
   新增能力要么有纯函数脚本（`scripts/verify-*.mts`），要么有回放 fixture，要么有 Rust 单测。
   「我看了一眼没问题」不算验收——本仓历史上「看起来能用」翻车过不止一次（隐藏面板 4 fps、24 MB 转录全量重写）。

---

## 3. 阶段总览

| 阶段 | 主题 | 规模 | 前置 | 主要收益 | 可独立发布 |
|---|---|---|---|---|---|
| Phase 0 | 子代理可观测性 | S–M | 无 | 多 agent 过程可信 | 是（S0） |
| Phase 1 | Composer 与审批体验 | M | Phase 0 | 高频输入路径明显改善 | 是（S1–S4） |
| Phase 2 | 回合服务与 ACP 连续性 | L | Phase 0 | 重启/续接/错误恢复更可靠 | 是（S5–S6，带保守回退） |
| Phase 3 | 代码交付工作流 | M–L | Phase 1 | 终端、Diff、提交、回退可用 | 是（S7–S9） |
| Phase 4 | 搜索、无障碍与快捷键 | M | Phase 1 | 找得快、操作一致、可访问 | 是（S10） |
| Phase 5 | 本地工作区与系统能力 | L | Phase 2/3 | worktree、唤醒、备份与诊断 | 需单独 beta（S11） |
| P1 annex | T3-only user-experience gaps | M | S0 for refs; otherwise independent | faster daily use without changing product shape | yes (P1 slices, see section 14) |

**顺序不可倒**：Phase 2 的 DOM/协议改动风险最高，必须建立在 Phase 0 的可观测基础上，且不得与 Phase 1
共用同一个实现代理。Phase 5 全部是「可选增强」，任何一项失败都不该拖住前面。

---

## 4. Phase 0 · 子代理可观测性

### 4.1 目标

打开任何一个派活 worker，都能看见它**在读什么、跑什么、卡在哪、已经产出了什么**；
关掉应用期间跑完的 worker，下次打开也能补齐；父对话上的子代理卡要能一眼看出它在干哪一步。

### 4.2 已观测到的事实（这就是本阶段的动因）

- 四个 OpenCode Go / DeepSeek V4.1 Flash 的派活 worker 都真的跑了：
  它们各自的 session JSONL 里有 **127 / 492 / 84 / 89** 条 `tool_call`，thought / assistant 流
  有数万到 15 万+ 字符，文件也确实被改动。
- 但在 Marionette 里打开对应子对话，**只看到大约「1 条 thought」**，没有可用的工具步骤和阶段正文。
- 结论：这是**可观测性缺口**，不是 worker 什么都没干。

### 4.3 先测量，再修（本阶段第一条纪律）

worker 的事件要经过五段才到眼睛，**先给每段装计数器**，别先猜：

| # | 段 | 位置 | 怎么量 |
|---|---|---|---|
| 1 | agent 发出 | agent 自己的 session 库 | 已知：127/492/84/89 条 `tool_call`（外部核对） |
| 2 | ACP 解析 | `src-tauri/src/acp.rs` 的 `send_prompt` 读流路径 | 按 `sessionUpdate` 种类计数（tool_call / tool_call_update / agent_message_chunk / agent_thought_chunk） |
| 3 | tee 落盘 | `src-tauri/src/chief/tee.rs` 的 `observe` / `spawn_writer` | 已写条数 + `dropped`（有界队列丢弃计数） + 文件行数 |
| 4 | 转录落盘 | `src-tauri/src/storage.rs` 的 append 路径 / 前端 `useSessionRails` | 该 worker 会话 JSONL 的行数与 kind 分布 |
| 5 | 回放与渲染 | `src/app/App.tsx` 的 `backfillChiefTee` → `extractAcpUpdateText` → `applyStreamPart` | 实际应用的条数、被 `uncoveredTeeEvents` / `railGrewPastBaseline` 挡掉的条数 |

- 计数走已有的 `appendDebugLog`（`source=subagent-observe`），或临时落在 `~/.marionette/logs/dev.log`；
  **不要为观测新建并行的状态存储**。
- 判定标准：某一跳的数目从 127 掉到个位数，那一跳就是断点；若是「每段都对、只有渲染少」，问题在 kind 分类或折叠逻辑。
- 已知的三类可疑点（都要被数据证实或排除，不许直接改）：
  1. tee 是**有界丢弃**队列（背压红线：绝不阻塞 ACP 读线程），高频 `tool_call_update` 可能被整段丢掉；
  2. 回放只在「转录未覆盖的 ts 跨度」生效（`uncoveredTeeEvents`），worker 事件缺 `ts` 或 ts 乱序时可能整段被过滤；
  3. 回放遇到活路写入就整体放弃且**不推进游标**（`railGrewPastBaseline`），频繁开关会话时可能反复放弃。

### 4.4 实施清单

- [ ] **P0-1**（S）五段链路计数器落地，输出一份「同一次派活，五段各多少条」的对照。
- [ ] **P0-2**（M）worker 会话正文：确认 worker 的转录 JSONL **真的被写**（tee 回放后落盘，还是 broker 直接 append），
      定下唯一写入路径；不允许「tee 里有、转录里没有」长期并存。
- [ ] **P0-3**（M）tee 丢弃可见：`dropped > 0` 时在打开会话处给出可理解提示，并给出补救路径
      （分段回填 / 延长回放窗口），而不是静默少事件。
- [ ] **P0-4**（M）worker 主视图显示 `tool_call`、`tool_call_update`、thought 与 assistant 正文，
      与普通会话**同一套渲染**（`applyStreamPart`），不新写第二套卡片。
- [ ] **P0-5**（M）父对话的子代理卡显示**当前动作**（最后一条工具标题 + 状态 + 步数），
      替代「只有 thought 计数」；卡片数据源与 worker 会话同源。
- [ ] **P0-6**（S）六态区分：`starting / running / waiting_user / completed / failed / cancelled`，
      与后台任务面板的状态投影一致（`src/lib/backgroundTasks.ts` 的口径为准）。
- [ ] **P0-7**（M）worker 结束后保留**最终摘要卡**：末条正文 + 工具调用数 + 耗时；
      完整过程仍可展开，不把过程压成一张 thought。
- [ ] **P0-8**（M）从真实 worker JSONL 录制**脱敏回放 fixture**（保留形状、去掉正文与路径敏感段），
      放 `scripts/fixtures/`；同一份 fixture 必须能驱动「解析 → 回放 → 渲染」全链。
- [ ] **P0-9**（S）冷启动补齐：应用关闭期间跑完的 worker，下次打开必须补全（游标 + 未覆盖跨度），
      且**不许重复**（You 卡与工具卡各只出现一次）。
- [ ] **P0-10**（S）新增 `scripts/verify-subagent-observability.mts`：拿 fixture 断言
      「127 条工具调用不会被折叠成 1 条」，且步数与父卡计数一致。

### 4.5 关键文件

- `src/lib/acpTranscript.ts`（`extractAcpUpdateText` / `applyAcpPartToEvents` / `applyStreamPart`）
- `src/lib/subagentAdapters.ts`（各家的嵌套 agent 识别，**别在这里加按 agent 名的特判**）
- `src/lib/liveSubagents.ts` + `scripts/verify-live-subagents.mts`（已有的活体判定）
- `src/components/SessionView.tsx`、`src/components/ActivityPanel.tsx`（`LiveActivityTimeline` 在这里）
- `src/app/App.tsx`（`backfillChiefTee`，只允许一个写者）
- `src-tauri/src/chief/tee.rs`（`observe` / `spawn_writer` / `replay`）、`src-tauri/src/chief/broker.rs`（注册点）
- `src/locales/parts/fe-delegate.*`、`src/locales/parts/fe-v2transcript.*`

### 4.6 验收

- 用 fixture 回放时：工具卡有标题、路径、状态；阶段正文按顺序出现；结束时出摘要卡。
- 父卡显示的步数 / 工具数 / 状态与 worker 会话**一致**。
- 完成后仍可展开完整过程；重复打开不重复追加。
- `npx tsc --noEmit`、`npm run build`、`npx tsx scripts/verify-subagent-observability.mts` 通过。
- 真机：派一个 worker 干 30+ 次工具调用的活，全程盯着看——不空窗、不闪退、卡不卡在「running」。

### 4.7 风险与回退

| 风险 | 应对 |
|---|---|
| 加大 tee 队列 = 内存与背压风险 | **不加队列上限**；改为「分段回填 + 每次打开重放」，队列继续拒绝阻塞 |
| 为让界面有东西而伪造进度 | 只渲染真事件；缺事件就如实显示「该段输出未记录」 |
| 转录双写（broker + 前端 rails 都写） | 先定唯一写入者（P0-2），另一个改成只读回放 |
| 回放把 You 卡重复插入 | 去重按「最大已覆盖 ts + 内容指纹」，fixture 里必须有一个双开场景 |

---

## 5. Phase 1 · Composer 与审批体验

### 5.1 目标

把「我有话说」「我想排后」「我被打断了」「我删错了」四条高频路径做顺，并且**不偷偷改变用户既有预期**。

### 5.2 1A · 队列 vs 打断（S1）

- [ ] Agent 忙碌时，发送按钮提供两个明确动作：**立即打断并发送** / **等当前回合结束后发送**。
- [ ] **默认保持现有的「立即打断并发送」行为**（`cancelLiveTurn` 现路径），不静默改默认。
- [ ] 队列条区分「插话」与「排队」两种来源，支持取消全部；`pendingSendsRef` 仍是唯一真相。
- [ ] 第一条 queue-only 消息支持「立即发送」，执行一次中断后按原顺序 flush。
- [ ] 不支持真 steer 的 agent **不显示**「插进当前回合」，改显示「等回合结束」并写原因。
- [ ] 新增 `scripts/verify-queued-delivery.mts`：三条路径（打断、排队、排队后立即发）不丢消息、不重复 You 卡。

### 5.3 1B · 草稿持久化（S2）

- [ ] Composer 草稿按 sessionId 持久化（当前 `draftBySessionId` 只在内存 `Map`，刷新即失）。
- [ ] 键形如 `marionette-composer-draft:<sessionId>`；正文 + 附件路径；单条上限（建议 64 KiB，超了不存并提示）。
- [ ] 删除会话时清理对应草稿；**撤销删除时保留**（否则 undo 只恢复一半）。
- [ ] 切会话、关标签、贴边收回再展开，草稿都在。
- [ ] 新增 `scripts/verify-prompt-draft.mts`：写入/恢复/删除清理/撤销保留/超限降级。

### 5.4 1C · 历史召回（S2）

- [ ] 空输入框 `ArrowUp` 召回最近一条用户消息，`ArrowDown` 回到较新的一条。
- [ ] slash、delegate、模型菜单打开中，以及 IME 组合中**不拦截**方向键（这是最容易出的输入事故）。
- [ ] 发送、切会话、手动输入后重置召回游标。
- [ ] 新增 `scripts/verify-prompt-history.mts`（纯函数：按会话取历史、游标推进、边界钳制）。

### 5.5 1D · 审批不抢焦点（S3）

- [ ] Composer 正在输入且 1.5 秒内有键入时，新的权限 / Ask / Plan 请求**不切换当前 session**（现在 `permissionPrompt` 是单槽 state，且会顶替）。
- [ ] 侧栏行出「等你处理」徽标，点击进入；`waiting_user` 进标题、侧栏与贴边「进行中」列表。
- [ ] Composer 上方显示**非阻塞**提示条（不抢焦点、不弹窗），明确说明「agent 正在等你」。
- [ ] 空闲时（无键入、无聚焦交互）保持现有的自动聚焦行为。
- [ ] 贴边窄条、整页、分屏、detached 四种形态分别手测。

### 5.6 1E · 破坏性操作可撤销（S4）

- [ ] 删除会话先进 5–6 秒 Undo 窗口（移到 `~/.marionette/trash/<id>/`），到点才真删；启动时清扫超期项。
- [ ] 清空队列、取消定时发送、关闭保留草稿的标签，各给一条可点 Undo 的 toast。
- [ ] 输入框聚焦时 `Ctrl+Z` **永远优先**执行文本撤销（不要和全局 undo 抢）。
- [ ] 崩溃重启不会「复活」已超时的删除。

### 5.7 关键文件

- `src/app/App.tsx`（会话删除、权限单槽、队列 drain——独占写者）
- `src/components/Composer.tsx`（草稿缓存 `draftBySessionId`、菜单、发送路径——独占写者）
- `src/components/ProjectShelf.tsx`（会话行菜单、「等你处理」徽标）
- `src/lib/toast.ts`、`src/lib/backgroundTasks.ts`、`src/lib/sessionOrder.ts`
- 新增：`src/lib/promptDraft.ts`、`src/lib/promptHistory.ts`
- 样式：`src/styles/stream-cards.css`、`src/styles/v2/shelf.css`（走 `docs/guides/ui-language.md`）
- 文案：`src/locales/parts/fe-composer.*`、`fe-app.*`、`fe-shelf.*`

### 5.8 验收

- 队列、打断、queue-only 三条路径不丢消息、不重复 You 卡。
- 重启后草稿恢复；历史召回不覆盖非空草稿。
- A 会话正在输入时收到 B 会话权限请求，**不切走**；徽标可点进入。
- 删除撤销后转录、草稿、会话元数据**完整**恢复。
- 四个形态手测通过；`npx tsc --noEmit` + `npm run build` 干净。

### 5.9 风险

- `App.tsx` 已万行以上，任何改动都要开新函数而不是继续堆在渲染路径里。
- localStorage 草稿与多窗口：撕出的 detached 窗也会写同一键——按 window 维度只写自己的 session，接受最后写入者胜。
- 「不抢焦点」若做成「永不提醒」，用户会漏掉审批 → 必须同时提供徽标与标题状态。

---

## 6. Phase 2 · 统一回合服务与 ACP 连续性

### 6.1 目标

把 UI、Telegram、员工模式、AI 派活工具的**发送 / 取消 / 恢复**语义收敛到同一套后端服务；
并让「重启后接着聊」尽量走 ACP 原生续接，而不是每次灌一段本地历史。

### 6.2 2A · 统一回合服务（先转发）（S5）

- [ ] 新建 `src-tauri/src/turn_service.rs`，**第一版只做转发层**：包装现有 `AcpService::send_prompt` / `cancel` / `stop`，不改行为。
- [ ] 统一事件枚举：`queued / dispatched / turn-started / waiting-user / completed / failed / cancelled`。
- [ ] 统一承载：模型配置回推、权限天花板、cancel watchdog、历史注入、turn baseline。
- [ ] 调用点逐个改道：`src-tauri/src/delegation.rs`、`src-tauri/src/chief/runner.rs`、`src-tauri/src/telegram_remote.rs`，最后是前端 `performSend`；
      每改一个，行为差异**在转发层对齐**，不在各入口打补丁。
- [ ] 新增 `scripts/verify-turn-service.mts`：同一 goal 经 UI / 派活 / Telegram 三条入口产生**相同阶段序列**。

### 6.3 2B · ACP `session/load` → `session/resume` → `session/new`（S5）

现状核实：本仓**没有** `session/load` 调用路径（只有 mock JSON 里出现过 `loadSession` 字样）。

- [ ] initialize 后记录能力：`loadSession`、`sessionCapabilities.resume`、`additionalDirectories`。
- [ ] 有 native session id 时依次尝试 `session/load` → `session/resume` → `session/new`。
- [ ] 原生续接成功时，**不再注入**本地历史文本（现在是一整段注入）。
- [ ] load 超时或 replay 卡死时安全回退历史注入，并向 UI 说明「这次是历史接续，不是原生会话」。
- [ ] 第一版**不实现** synthetic replay-idle response（T3 的取巧），保持保守超时。
- [ ] ACP fixture 覆盖四种：load 成功、method-not-found、超时、resume 成功。

### 6.4 · 员工模式与普通 AI 派活共用执行层

现状的父会话语义仍按入口分裂：普通 AI 派活以当前普通对话为父，经 `marionette_delegate_task` 启动 `agent-worker`，worker 随任务结束而销毁；员工模式以带 `staffId` 的持久员工 Chat 为父，由 `chief-worker` 承载项目策略、记忆、承诺、通知策略和 wake inbox。目标是把执行与展示收口到同一层，同时保留“员工持久、worker 一次性”的产品语义。

- [ ] 普通派活与员工派发统一经 `turn_service` 执行，只产生一套 worker 事件流和一条 worker 卡片渲染路径。
- [ ] 员工可以派发 worker；worker 不得递归派活，员工也不得招聘新员工。
- [ ] 员工记忆、承诺、通知和项目策略只属于员工层，不混入通用 worker 执行逻辑。
- [ ] 实现落点：`src-tauri/src/turn_service.rs`、`src-tauri/src/delegation.rs`、`src-tauri/src/chief/runner.rs`、`src-tauri/src/chief/broker.rs`、`src/app/App.tsx`。
- [ ] 验收：同一 goal 经普通派活和员工派发产生等价的 phase/event 序列；worker UI 卡、步骤数、当前动作和最终摘要保持一致；员工独有治理能力不丢失。

### 6.5 2C · 中断回合续接（S6）

- [ ] 冷启动识别「进程退出但上一回合没有终态」的 session（与 `agentReentry.ts` 的既有判定不要混）。
- [ ] 显示「上次回合被中断」卡片，提供**继续 / 忽略**；继续时明确提示「上一条命令可能已经执行，重复前先检查」。
- [ ] 权限/Ask/Plan 未答、用户主动停止、parked、worker 子会话**不自动提供继续**。
- [ ] 默认手动，绝不自动花钱重试。

### 6.6 2D · 能力诚实（S5/S6）

- [ ] 新增 `docs/guides/honest-capabilities.md`：把桥接模型档位、账号模型漂移、权限档继承、steer、resume 的
      fail-closed 规则收成**一篇通则**（现散落在 `AGENTS.md` 各批次）。
- [ ] `src/lib/capabilityDrift.ts` 与 `delegation.rs` 各加一层「声明 vs 实测」断言。
- [ ] 新增 `scripts/verify-honest-capabilities.mts`：三类场景（桥接档位、账号漂移、派活档位继承）钉死。

### 6.7 关键文件

- 新增：`src-tauri/src/turn_service.rs`、`docs/guides/honest-capabilities.md`
- `src-tauri/src/acp.rs`（独占写者：initialize 能力、send_prompt、cancel、生命周期）
- `src-tauri/src/chief/runner.rs`、`src-tauri/src/delegation.rs`、`src-tauri/src/telegram_remote.rs`
- `src/app/App.tsx`（`performSend` 只保留 UI 状态，不再自己拼 ACP 调用）
- `src/lib/capabilityDrift.ts`、`src/lib/sessionHistory.ts`、`src/lib/agentReentry.ts`

### 6.8 验收

- `cargo test --bin marionette` 全绿（现有 475+ 项口径）。
- load 成功时 agent 能直接接着原生历史；不支持时仍能靠历史注入继续，且 UI 说明清楚。
- UI / Telegram / 员工模式 / 派活四条入口的回合事件序列一致。
- 中断续接卡只在「真中断」时出现；不重复、不自动重试。

### 6.9 风险

- **最大的一条**：`acp.rs` 是全局热点，本阶段必须独占；任何「顺手改一点」都要另开批次。
- 转发层跑成第三份实现 = 债更重。判据：转发层里出现业务分支，就说明迁错了。
- 不同 agent 对 load/resume 的实现差异大：**按能力降级**，不要按 agent 名字特判。

---

## 7. Phase 3 · 代码交付工作流

### 7.1 目标

让用户能看清终端、看懂本轮 diff、直接批注、确定性提交，并且**只撤需要撤的那一轮**。

### 7.2 3A · 终端输出抽屉（S7）

现状：`listAcpTerminalOutputs` 已存在，但只被 dev-server URL 扫描使用（`src/app/App.tsx` 的 4s tick），没有给用户看的界面。

- [ ] 后台任务里的 ACP terminal 行增加「查看输出」。
- [ ] 抽屉显示运行状态、尾部输出、刷新与复制；长输出分页（**不要**一次性拉全量）。
- [ ] 解析 Windows / POSIX 路径与 `path:line:column`，点击走现有链接行为。
- [ ] 不做完整 PTY，不执行新 shell（这是只读观察面）。
- [ ] 新增 `scripts/verify-terminal-links.mts`（纯函数：路径与行列解析）。

### 7.3 3B · `file:line:column` 外部编辑器定位（S7）

现状：`open_target.rs::resolve_link_target` 解析时会剥掉 `file.ts:42` 后缀，但返回里没有行号。

- [ ] `resolve_link_target` 增加 `line / column`（保留现有 `kind/target/risky` 字段，前端不破）。
- [ ] 新增 `open_in_editor`：检测 VS Code / Cursor / Windsurf，支持 `MARIONETTE_EDITOR` 覆盖。
- [ ] 链接菜单、Changed Files、Turn Delivery Card 增加「在编辑器中打开」。
- [ ] 没有编辑器时**明确提示**，不静默用系统默认应用打开文件（会丢行号语义）。
- [ ] Rust 单测覆盖 `file:42`、`file:42:7`、相对路径、Windows 盘符与中文路径。

### 7.4 3C · Diff 区间批注（S8）

- [ ] 复用 `src/lib/annotations.ts` 已有的 range / line 模型，不新造一套。
- [ ] `UnifiedDiffView` 支持拖选连续行；`FileChangeCard`、`TurnDeliveryCard`、`DiffPage` 共用同一个 draft。
- [ ] 发送格式为「路径 + 行号 + 引用块」，可删可编辑（进 Composer 后仍是普通文本）。
- [ ] 新增 `scripts/verify-annotations.mts`。

### 7.5 3D · 确定性 Commit / Push（S8）

- [ ] 后端 `commit_paths` 只 stage **明确路径**，拒绝越界与符号链接逃逸。
- [ ] 提交信息由用户编辑（默认可由 Phase 2 之外的文本生成填一版，但**必须可改**）；可选同时 push。
- [ ] 提交对话框展示 hook 输出与失败尾部；hook 失败不吞错。
- [ ] 保留「让 Agent 提交」作为备选（现在的形态）。
- [ ] 不存 GitHub token，不引入 PR 平台层。
- [ ] 新增 `scripts/verify-commit-paths.mts` + Rust 单测（白名单、越界、符号链接、空清单拒绝）。

### 7.6 3E · 按轮检查点（S9）

现状：`git_service` 有 `begin/end_workspace_snapshot`（内容树差异）与 `restore_turn_files`，但**只恢复最后一轮**。

- [ ] 每轮开始记录 tree 并挂 `refs/marionette/turn/<turnId>`（隐藏 ref，不给用户分支加提交）。
- [ ] Diff 使用**本轮 base tree → 当前 worktree**，而不是无条件对 HEAD。
- [ ] Undo 使用同一 base tree，不再无条件回 HEAD。
- [ ] 只保留最近 N 轮的轻量 ref，过期清理；`doctor` 报孤儿 ref 数量。
- [ ] 老会话没有 tree 时，退回当前 HEAD 行为并**说明**。
- [ ] 数据迁移测试：旧会话、无 git 仓库、起点就脏的仓库三种都要过。

### 7.7 关键文件

- `src/components/TasksSection.tsx`、新增 `src/components/TerminalOutputDrawer.tsx`、新增 `src/lib/terminalLinks.ts`
- `src/components/LinkedText.tsx`、`src-tauri/src/open_target.rs`
- `src/components/UnifiedDiffView.tsx`、`src/components/DiffPage.tsx`、`src/components/TurnDeliveryCard.tsx`
- 新增 `src/components/CommitDialog.tsx`、`src-tauri/src/git_service.rs`（独占写者）
- `src/lib/turnDelivery.ts`（归属规则：起点脏只算被本轮工具碰过的）

### 7.8 验收

- 长命令输出可以在抽屉里查看，并点路径跳到正确位置。
- 编辑器打开到正确行列；无编辑器时提示明确。
- 多行 diff 批注进同一条 Composer 上下文，也进 same draft。
- 空清单不能提交；越界路径被拒；hook 失败可读。
- 撤第 2 轮不会吃掉第 3 轮；老会话行为不退化。
- `npx tsx scripts/verify-turn-delivery.mts` + `cargo test --bin marionette git_service` 不退。

### 7.9 风险

- `git_service.rs` 同时被交付卡、圆桌 worktree、检查点使用：本阶段独占，禁止并行。
- 隐藏 ref 与用户自己 `git fetch --prune` / GC 的交互要写清（`doctor` 能重建，不能丢数据）。
- Commit 对话框要防「一键提交别人的改动」：默认只勾选本轮路径。

---

## 8. Phase 4 · 搜索、无障碍与快捷键

### 8.1 4A · 跨会话正文搜索（S10）

现状：会话只按标题过滤，对话内 `Ctrl+F` 只覆盖**已加载范围**（`src/lib/conversationFind.ts`）。

- [ ] 新增 Rust 模块对 transcript JSONL 做增量索引（SQLite FTS5 或 trigram 倒排，二选一，先做能重建的那版）。
- [ ] 首版允许**全量重建**和「删索引后重建」，不做复杂迁移。
- [ ] `Ctrl K` 结果返回 session / event / snippet / 跳转定位（复用 `findSeenEventIndex` 那套定位）。
- [ ] 中文按子串命中，英文大小写不敏感。
- [ ] 单会话 24 MB 不阻塞 UI（索引在后台线程，查询有上限）。
- [ ] 新增 `scripts/verify-transcript-search.mts`（分词、大小写、中文子串、删除会话后清理）。

### 8.2 4B · 无障碍播报（S10）

- [ ] 新增 polite live region，播报「回合完成 / 需要你 / 失败 / 已停止」。
- [ ] 同来源去重；重复文案可重读；焦点稳定后才播报（避免和焦点移动抢读）。
- [ ] **不朗读** thought 正文与敏感字段。
- [ ] `Ctrl K` 设置：关闭 / 仅状态 / 含工具步骤。

### 8.3 4C · 快捷键注册表（S10）

- [ ] 先建**只读**的 keybinding 清单，覆盖现有快捷键（Ctrl K / Ctrl F / Ctrl W / Ctrl Tab / Ctrl 1..9 等）。
- [ ] `Ctrl K` 增加「快捷键」页只做展示。
- [ ] 第二步再接 `when` 条件（输入框、终端、iframe、贴边、浮层各自的优先级），**不要一次重写全部监听**。

### 8.4 4D · Side chat 评估（不排期）

- [ ] 先做用户访谈或自用记录，确认「问一句但不想脏主历史」是高频需求。
- [ ] 若成立，复用现有 fork 通道创建隐藏 `origin=side-chat` 会话，默认开在右侧面板，不进普通侧栏。
- [ ] 支持 Add to chat 后显式提升。
- [ ] 结论不稳定就**暂缓**，不硬做完整线程生命周期（审计里 side chat 属 defer）。

---

## 9. Phase 5 · 本地工作区与系统能力（全部可选）

任何一项失败都不该拖住前面；不达标就单独关掉。

### 9.1 5A · 单仓可选 worktree（S11，需单独 beta）

- [ ] 新会话可选「本目录 / 新 worktree」；路径 `~/.marionette/worktrees/<project>/<sessionId>`。
- [ ] 复用 `git_service::create_worktree / remove_worktree / merge_worktree_branch`（圆桌执行器已在用，别新写一套）。
- [ ] 会话 cwd、权限根、终端、浏览器、文档相对路径**全部同步**到 worktree。
- [ ] 删除会话时检查本地改动，提供保留 / 合并 / 丢弃；不做多仓与自定义布局脚本。
- [ ] `doctor` 检查孤儿 worktree。
- [ ] 不做「共享 checkout 时禁止切分支」这类多 agent 编排约束（我们是单进程单用户）。

### 9.2 5B · agent 运行时保持唤醒

- [ ] 任一 session 处于 starting / running / 队列待发时持 `ES_SYSTEM_REQUIRED`。
- [ ] 最后一轮结束后 60 秒释放；电池供电默认关闭、可配置。
- [ ] 4 小时硬上限，避免永久占用。

### 9.3 5C · 备份与更新回滚

- [ ] `Ctrl K` 导出 `~/.marionette` 的关键数据（索引 / 账本 / 绑定 / 设置），**不复制转录正文**（体积不可控）。
- [ ] 引入数据 schemaVersion：迁移前把元数据打包进 `backups/`；迁移失败自动还原并留日志。
- [ ] 更新前保留旧 exe（`app_update.rs` 既有路径），启动后健康检查失败可提示回滚。
- [ ] `doctor` 报「最近备份时间 + 可回滚性」。

### 9.4 5D · 诊断与存储面板

- [ ] 只在面板打开时高频采样（默认不采）。
- [ ] 展示 Agent Job、内存、CPU、最近主线程停顿（`[watchdog]` 日志）、最近错误（`[panic]` 日志索引）。
- [ ] 清理缓存 / 日志 / 旧引擎镜像；**不自动删对话**。
- [ ] 与 `diagnostics.rs` 的导出报告边界一致（脱敏规则同一套）。

---

## 10. 多代理并行规则

### 10.1 可以并行

- Phase 0：前端 transcript 展示、fixture 录制、父卡文案，三件互不重叠。
- Phase 1：5.2（队列）/ 5.3（草稿）/ 5.4（历史）/ 5.6（Undo）可拆给不同代理；**5.5（审批焦点）与 App.tsx 串行**。
- Phase 3：终端抽屉、编辑器定位、Diff 批注、Git 后端可按文件边界并行。
- 文档、验证脚本、代码实现可以分给不同代理（但验证脚本必须由**没写实现的人**写，否则等于自证）。

### 10.2 必须串行（独占文件）

| 文件 | 独占原因 |
|---|---|
| `src/app/App.tsx` | 会话生命周期、队列、权限单槽、回放全在这里；并行必冲突 |
| `src/components/Composer.tsx` | 草稿/菜单/发送三条路径互锁 |
| `src-tauri/src/acp.rs` | ACP 读写与生命周期唯一入口 |
| `src-tauri/src/main.rs` | 窗口/关闭/挂起/命令注册 |
| `src-tauri/src/storage.rs` | 转录写入唯一真相 |
| `src-tauri/src/git_service.rs` | 交付卡 + 圆桌 + 检查点共用 |

### 10.3 交接格式（每个代理交付时必须有）

1. 改了哪些文件、为什么。
2. 行为差异（before / after）。
3. 失败路径与用户看到什么文案。
4. 实际跑过的测试命令（原样粘贴）。
5. 未验证项（诚实写，不许留空）。

主代理负责冲突审查、最终构建与真机验证。

---

## 11. 发布切片与第一批

| Slice | 内容 | 可单独发布 | 依赖 |
|---|---|---|---|
| **S0** | 子代理工具与正文可见（Phase 0） | 是 | — |
| S1 | 队列「立即打断 / 等本轮结束」（1A） | 是 | S0 |
| S2 | 草稿持久化 + 历史召回（1B/1C） | 是 | S0 |
| S3 | 非阻塞审批 + 「等你处理」（1D） | 是 | S1 |
| S4 | 删除与队列 Undo（1E） | 是 | S1 |
| S5 | 统一回合服务转发层 + ACP load/resume（2A/2B/2D） | 是，带保守 fallback | S0 |
| S6 | 中断回合续接卡（2C） | 是 | S5 |
| S7 | 终端输出抽屉 + 编辑器定位（3A/3B） | 是 | S1 |
| S8 | Diff 批注 + 确定性提交（3C/3D） | 可合并发布 | S7 |
| S9 | 按轮检查点（3E） | 是，需数据迁移测试 | S8 |
| S10 | 正文搜索 + 无障碍 + 快捷键（Phase 4） | 是 | S1 |
| S11 | 单仓 worktree（5A） | 需单独 beta | S9 |
| **P1-3** | Large-paste folding | Yes | independent after S0 write freeze |
| P1-1 | Diagnostic triage chat | Yes | diagnostics schema |
| P1-2 | Question-answer attachments | Yes | P1-3 attachment intake |
| P1-4 | Explicit bounded session references | Yes | S0 |
| P1-6 | Recent-project discovery only (no clone/default overrides) | Yes | P1-1/P1-3 not required |
| P1-7 | Browser find/viewport + Diff find/whitespace/colorblind | Yes | Phase 3 browser/diff surfaces |
| P1-8 | T3-style Usage redesign + overrides/reset-aware scheduled send | Yes | existing ledger + scheduled sends |
| P1-9 | Prompt links + F6 area focus | Yes | versioned launch handoff |
| P1-10 | Window capture into the current draft | Yes | P1-3 attachment intake + Computer Use |
| P1-11 | Working/settled/snoozed/archived session presentation states | Yes | S0 + scheduled/background projections |
| P1-12 | On-demand tool schema search | Yes | S0 tool catalog |
| P1-13 | Cheap text generation for titles/commit messages | Yes | A2 + credentials/source reuse |
| P1-14 | Local project actions | Yes | P1-11 + worktree setup hooks |

### 第一批（回家就做）

**Revised first wave: S0, then P1-3/P1-1, then S1-S4, then P1-2/P1-4.** Section 14 supersedes the older S0-S4-only wording.

1. 它们不依赖 ACP 协议层重写，风险可控。
2. 直接改善每天的使用体验（看得见 worker、草稿不丢、审批不抢输入、删错能退）。
3. 完成后，Phase 2 的大改可以在「可观测、可回退」的地基上进行。
4. 完成后，多代理并行才真的安全（现在看不见 worker，等于并行等于赌博）。

After the revised wave in section 14 is green, decide whether to enter S5 (`session/load`) and the unified turn service.

---

## 12. 全局验证与完成定义

### 12.1 每个 Slice 合并前

- [ ] `npx tsc --noEmit`
- [ ] `npm run build`
- [ ] `cargo check`（在 `src-tauri/`）
- [ ] 相关 `cargo test --bin marionette <module>`
- [ ] `node scripts/verify-all.mjs`（或写清哪几个脚本没跑、为什么）
- [ ] 普通窗口 / 贴边 / 分屏 / detached 四形态手测
- [ ] 失败时用户看到明确文案（**不允许静默成功**）
- [ ] `docs/CURRENT.md` 只更新现行行为段落，不追加批次流水账

### 12.2 完成定义（Definition of Done）

一个切片算完成，必须同时满足：

1. 行为契约写进 `docs/CURRENT.md`（或 `guides/`），并说明了失败路径。
2. 至少一条自动化证据：verify 脚本 / 回放 fixture / Rust 单测。
3. 四种窗口形态下都能用，且不闪、不卡、不丢输入焦点。
4. 能力不支持时禁用并写原因，而不是假装成功。
5. 用户可回退（Undo 或保守 fallback 二者之一）。
6. `AGENTS.md` 只新增「必须记住的红线」，不写流水账。

### 12.3 批次收尾

- 每个 Phase 结束跑一次全量：`npm run build` + `node scripts/verify-all.mjs` + `cargo test --bin marionette`。
- 记一份「本批实测」（内存、启动、24 MB 转录打开时间、worker 步数），放 `docs/manual/`，供下批对比。

---

## 13. 回家从这里开始（按顺序，不要跳）

1. **第一天：读与量。**
   读本文件 §1–§4、审计 §0.1 与 §5.1。跑一遍基线：`npm run build`、`node scripts/verify-all.mjs`、
   `cargo test --bin marionette`，把结果贴进本次会话笔记（不通过就先修环境，别开工）。
2. **第一天下午：造证据。**
   派一个真实 worker 干 30+ 次工具调用的活，同时打开 `~/.marionette/logs/dev.log`。
   按 §4.3 的五段表人工数一遍（agent 库 / acp 日志 / tee 文件 / 转录 JSONL / 界面卡片）。
   **先写出「哪一跳掉了多少」这一句话**，再动代码。
3. **第二天：最小闭环。**
   只做 P0-1（计数器）+ P0-8（fixture）。fixture 能驱动「解析 → 回放 → 渲染」即可，不求界面好看。
4. **第三至六天：S0 主体。**
   P0-4 / P0-5 / P0-6 / P0-7 按顺序做；每做完一条，拿同一份 fixture 验一次，并真机看一眼。
5. **第七天：S0 收口。**
   P0-2（唯一写入路径）、P0-3（丢弃可见）、P0-9（冷启动补齐）、P0-10（verify 脚本）。
   写 `docs/CURRENT.md` 的「子代理可观测性」段。
6. **第二周：S1–S4。**
   S1（队列双动作）→ S2（草稿/历史）→ S3（审批不抢焦点）→ S4（Undo）。
   每片单独可发；任何一片卡住就发前面的，别攒大版本。
7. **Decision gate for S5.**
   Proceed only after the revised wave in section 14 is green and has one week without regressions;
   only then should `acp.rs` be touched.

> Core sequence: make process visible, remove daily workflow friction, then make the runtime faster.

## 14. Second-pass UX implementation queue (2026-11-27)

> Source: [T3 second-pass audit, section 9](../research/t3code-feature-audit.md#9-second-pass-review-t3-ux-gaps-worth-adopting-2026-11-27).
> This annex is authoritative where it conflicts with earlier wording in this plan. It does not replace
> Phase 0 or the existing S0-S11 structural work; it inserts user-visible P1 slices where they can ship
> independently and changes the release order so useful improvements do not wait behind ACP rewrites.

### 14.1 UX ordering principles

1. Finish S0 observability before expanding parallel-agent work. A visible process is still the first
   trust requirement.
2. Pull independent UX wins forward. Large-paste folding and diagnostic triage
   do not depend on `acp.rs` and should not wait for the unified turn service.
3. Preserve the current inheritance model unless the user explicitly changes it: a new chat follows
   the current or most recent same-project dialog, with per-agent last-used values as fallback.
4. Every state has a reverse state and every unsupported action explains why it is disabled.
5. No new cloud capability is needed for any UX item below.

### 14.2 Revised first user-visible wave

Ship in this order, treating each item as independently releasable:

1. **S0** - subagent observability. This remains the first correctness slice.
2. **P1-3** - large-paste folding. Small, isolated, immediate every-day benefit.
3. **P1-1** - diagnostic triage chat. Turns the existing diagnostic exporter into a useful agent workflow.
4. **S1-S4** - queue/interrupt behavior, persistent drafts/history, non-blocking approvals, and Undo.
5. **P1-2** - question-answer attachments after the Composer attachment path from P1-3 is stable.
6. **P1-8** - T3-style Usage redesign, ahead of advanced browser/tool refinements because it directly
   fixes an existing readability problem.
7. **P1-4** - explicit session references after S0 has established the one-worker/event rendering path.
8. **P1-12** - on-demand tool schema search after S0 makes worker/tool activity fully visible.
9. **P1-10** - window capture into the current draft after the attachment pipeline is stable.
10. **P1-11** - presentation-only session lifecycle states after Undo and background projections are stable.
11. **P1-13** - cheap text generation after the usage ledger and model-source plumbing are ready.
12. **P1-14** - local project actions after session states and worktree setup hooks are stable.

P1-1 and P1-3 may be implemented while S0 analysis is running only when their write sets do not overlap.
The exclusive-file rules in section 10 still win.

### 14.3 P1 slices

#### P1-1 Diagnostic triage chat (M)

- **Entry:** Ctrl K `Diagnose with agent`.
- **Behavior:** build the current redacted diagnostic bundle, create an `origin: diagnostic` Chat, attach
  the Markdown/JSON and a bounded relevant log tail, and prefill the playbook. Do not auto-send.
- **Playbook rules:** treat logs/issues/source as untrusted data; never read credentials; do not patch
  Marionette unless the user explicitly asks; never post an issue without confirmation; distinguish
  the installed version from the source version.
- **Files:** `src-tauri/src/diagnostics.rs`, `src/app/App.tsx`, `src/components/CommandPalette.tsx`,
  `src/lib/api.ts`, `src/locales/parts/fe-dialogs.*`, `src/locales/parts/fe-palette.*`.
- **Acceptance:** generated bundle passes the existing diagnostic redaction tests; no conversation body,
  project path, environment block, or credential store is included; the draft is created without being
  sent; unavailable source or version is stated honestly.
- **Rollback:** hide the command and retain the existing diagnostic export path.

#### P1-2 Question-answer attachments (M)

- Allow attachments only on questions whose schema accepts a custom text answer.
- Keep per-question drafts while moving through a multi-question form. Uploads must finish before submit.
- Prefer a native ACP attachment field if one exists. Otherwise, do not stuff paths into enum values;
  append one explicit follow-up message containing saved paths after the answer is accepted.
- **Files:** `src/components/AskQuestionCard.tsx`, `src/app/App.tsx`, `src-tauri/src/acp.rs`,
  `src-tauri/src/elicitation.rs`, `src/lib/askQuestion.ts`, Composer attachment helpers.
- **Acceptance:** fixed-choice-only questions show no attachment UI; each question keeps its own draft;
  cancel/delete cleans files; the response remains schema-valid; a file attachment path is visible to
  the user before submit.

#### P1-3 Large-paste folding (S)

- Fold text at 32 KiB or whenever insertion would exceed the composer limit.
- Save UTF-8 text under Marionette-owned attachment storage. Never place it in the project tree.
- `Ctrl+Shift+V` pastes inline. If attachment staging fails, show a concrete error and keep the draft.
- **Files:** new `src/lib/pastedTextFold.ts`, `src/components/Composer.tsx`, attachment API/types,
  locales, new `scripts/verify-pasted-text-fold.mts`.
- **Acceptance:** byte threshold handles CJK correctly; selection replacement is respected; inline
  bypass works; no duplicate attachment; cleanup follows existing attachment lifecycle.

#### P1-4 Explicit session references (M-L)

- Add a session chip by dragging a sidebar row into Composer or pasting a Marionette session link.
- First version attaches a bounded snapshot: metadata, first user request, last N user/assistant turns,
  and selected tool outcomes. No thoughts, credentials, raw output bodies, or unrelated events.
- Second version adds a read-only MCP tool with hard event and byte limits. Only explicitly referenced
  sessions are readable.
- **Files:** new `src/lib/sessionReference.ts`, snapshot builder, Composer chip support, `src-tauri`
  storage read API, later `src-tauri/src/tools_mcp.rs`.
- **Acceptance:** 24 MB source transcript produces a bounded reference without blocking UI; no reference
  can widen filesystem or credential access; deleting/redacting the source is reflected honestly.


#### P1-6 Recent-project discovery only (M)

- Scan existing agent session stores for recent project roots. Present candidate roots with activity
  counts; add only user-selected projects and then reuse per-project session import.
- Do not add Git URL cloning. Existing local projects remain the normal entry path.
- Do not add project-level agent/model/mode/effort/permission defaults. The current behavior already
  inherits the current or most recent same-project dialog, with per-agent last-used values as fallback.
- **Files:** `src-tauri/src/parsers/`, `src/components/GettingStartedGuide.tsx`, project shelf/onboarding UI.
- **Acceptance:** no project is added without explicit selection; discovery failure does not block normal
  Add Project; no existing project or session preference is overwritten.

#### P1-7 Browser and diff usability (M)

- Browser find-on-page first; then responsive viewport presets, rotate, and zoom.
- Diff find over loaded files, hide-whitespace mode, and a colorblind-friendly palette.
- **Files:** `BrowserPage.tsx`, browser surface/CDP APIs, `UnifiedDiffView.tsx`, `DiffPage.tsx`, styles,
  locales, pure helper scripts.
- **Acceptance:** Ctrl F routes to the focused surface instead of hijacking the transcript; viewport
  changes are labeled as emulation; whitespace mode never changes the underlying patch or commit data.

#### P1-8 T3-style Usage page redesign + overrides/reset-aware send (L)

- **Design transplant:** replace the current long-dashboard default with T3's hierarchy: Usage header
  with environment/provider filters, period and Cost/Tokens/Limits tabs; one large total and session
  count; provider shares; one trend chart; then an expandable secondary detail layer.
- Keep Marionette's exact ledger data, time/tool/file/context breakdowns, session rows, per-turn detail,
  backfill and CSV export, but do not show all of them simultaneously on first paint.
- Apply Marionette's existing 3px/1px/no-shadow visual language. Preserve responsive controls and
  skeleton/loading behavior; no horizontal overflow in dock or narrow panes.
- Add custom price-per-million, alias/model mapping, and include/exclude filters. Provider-reported cost
  remains authoritative; API estimate and coverage stay explicitly labeled.
- When a usage-limit failure has a trustworthy reset time, offer `Schedule after reset` through the
  existing scheduled-send queue. Do not add a second scheduler or auto-retry.
- **Files:** `UsageAnalysisDialog.tsx`, `UsagePage.tsx`, `UsageV2Body.tsx`, `usage/*`, usage styles,
  locales, `usageLedger.ts`/`usageInsights.ts`, existing CSV and scheduler paths.
- **Acceptance:** first screen answers total, largest provider/model, period and limit state in under five
  seconds; Cost/Tokens/Limits tabs persist; all advanced analytics and CSV remain reachable; custom
  price never masquerades as billed cost; scheduled reset delivery is explicit and cancellable.

#### P1-9 Prompt links and F6 area focus (M)

- Add a versioned launch payload for prompt links. Replace the raw pending-open-path file only after
  backward-compatible parsing is in place.
- Parameters: prompt, project/session, agent, model. Do not include permissions or auto-send.
- Strip control/hidden characters, cap prompt length, warn on removed characters, and confirm before
  replacing a non-empty draft.
- Add F6/Shift+F6 focus cycling across visible workspace areas and restore the prior focus after closing
  palettes/menus.
- **Acceptance:** links cannot inject hidden instructions or change permission state; second-instance
  routing works from edge dock and normal windows; F6 skips hidden areas and never traps focus.

#### P1-10 Window capture into the current draft (M)

- Add a configurable user shortcut for capture-window-into-draft.
- Reuse Computer Use window/screenshot primitives and accessibility-tree summarization; do not widen
  agent-side Computer Use permissions.
- Attach the PNG and a bounded text summary to the current or new draft. Exclude Marionette itself and
  existing blacklisted windows; never auto-send.
- **Acceptance:** works without sharing the window to an agent first; capture failure is visible; the
  resulting attachment follows normal cleanup; keyboard shortcut is configurable or conflicts clearly
  with an existing chord.

#### P1-11 Session working/settled/snoozed/archived states (M)

- Keep process state and presentation state separate. Do not make `settled`, `snoozed`, or `archived`
  kill an agent process by implication.
- Working groups running, delegated, and attention-waiting sessions. Settled auto-collapses after the
  configured quiet period only when there is no unread attention, active background work, approval,
  question, or scheduled send.
- Snooze hides until a chosen time or an attention event. Archive removes the row from the normal shelf
  and search while preserving restore.
- **Files:** `sessionGroups.ts`, `ProjectShelf.tsx`, session metadata/storage, scheduled/background task
  projections, locales, `scripts/verify-session-states.mts`.
- **Acceptance:** no auto-settle while attention is pending; status filters/counts agree; Undo or Restore
  works; process-park behavior and tests remain unchanged.

#### P1-12 On-demand tool schema search (M)

- Keep core tools resident. Put browser, computer, and optional plugin schemas behind one bounded
  tool-search entry instead of injecting every schema into every session.
- Preserve explicit direct calls for user-enabled tools; discovery is for reducing baseline context tax,
  not for hiding capabilities.
- **Files:** `src-tauri/src/tools_mcp.rs`, tool-help/catalog registration, model-facing help tests.
- **Acceptance:** schema bytes at session start decrease with recorded before/after numbers; browser and
  computer tools are discoverable in at most two calls; existing permission and ref-lease tests remain green.


- `turn_service` accepts a durable/client request id and deduplicates UI, Telegram, scheduled, and
  delegated sends. A retry returns the existing command result rather than starting another turn.
- One connection owner probes before replacing a live transport or engine session. Foregrounding, proxy
  changes, and network recovery must not create parallel retry loops.
- Health and data freshness are separate UI states.
- Starting/running/waiting phases come from actual protocol/process signals, never timers.
- Recovery retires effects tied to a lost process; it does not replay prompts or checkpoints automatically.

#### P1-13 Cheap text-generation service (M)

- Configure one low-cost text model for semantic title regeneration, branch names, and editable commit
  messages. Keep it independent from coding-agent turns and never block sending.
- No key/network means the existing first-message title remains. Label these calls as a separate usage
  source and do not mix them into agent quota totals.
- Reuse existing encrypted credential/source storage. A failed generation is silent but logged without
  prompt or response bodies.

#### P1-14 Local project actions (M)

- Add user-owned setup/dev/test/cleanup actions under Marionette project metadata. Do not execute
  checked-in scripts from the repository.
- Show the exact command before first run. Actions use an ACP terminal, expose output/stop, and never
  inherit a higher permission mode.
- Setup may run after worktree creation; other actions are manual. Failure does not block opening the
  session unless the user explicitly chose wait-for-setup.

### 14.4 Structural requirements added to S5


### 14.5 Quality gates added

- Long renderer tasks and backend main-loop stalls above two seconds write a bounded local trace with
  the same redaction boundary as diagnostics; provide a small offline summary script.
- UI changes attach before/after captures. Motion/timing changes also record a short capture when
  practical.
- Every new state is reviewed with its reverse transition and recovery path.
- Large-component handler review: only callbacks crossing props/effect/dependency boundaries need stable
  wrappers; ordinary render-local logic stays uncached.

### 14.6 Corrections that override earlier plan text

- Scheduled send is already implemented. Extend it; do not add the old proposed `scheduler.rs`.
- First-prompt auto-title is already implemented. A2 now means semantic model generation, regeneration,
  and optional title-style instructions.
- `--open-path`, bare-path launch, and single-instance handoff already exist. P1-9 extends that contract.
- Dev-server discovery, quote/annotation capture, MemoBox draft saving, browser handoff, and explicit
  browser sharing are existing features and must not be re-planned.

### 14.7 Explicit defer / reject

- **Defer:** provider instances, MCP OAuth UI, private secret-request cards, rich Markdown composer,
  Activity Zen modes, side chat, custom keybinding `when` engine, browser recording/cookie import.
- **Defer until the named dependency:** model-predicted next prompt after a cheap text generator exists;
  same-prompt multi-model runs after worktree and observability are stable.
- **Keep rejected:** cloud identity/relay, mobile clients, guest/peer links, PR/Issue platform,
  workflows/webhooks, enterprise managed policy, product telemetry, custom CSS themes, automatic
  crash-continuation that may spend money.
- **User declined:** Windows startup registration, Git URL clone in Add Project, and project-level
  default agent/model/permission overrides. Do not schedule those.

### 14.8 Release hardening

- Authenticode signing needs a publisher/certificate decision and should be tracked separately from UI
  slicing.
- Add a generated third-party notice inventory and an in-app viewer.
- Keep the Phase 5 backup/trial-migration rule: a failed migration must leave the old data recoverable.
### 14.9 Remaining audit quality backlog

These were present in the first audit but had no dedicated execution slice in this plan:

- **Q1 Performance budgets.** Add startup, transcript, and dock A/A scripts with machine-readable JSON.
  Start by recording the A/A noise floor; do not enforce a threshold until two identical builds are
  reproducibly comparable.
- **Q2 Model-facing evals.** Add fixed scenarios for delegation goals, tool descriptions, and generated
  titles. Run them with the cheapest usable model and store pass rates and cost, not natural-language
  assertions.
- **Q3 Real-flow proof.** Add deterministic replay fixtures for the most failure-prone UX paths: worker
  cards, queue/steer, question attachments, session references, and triage bundles. A visual change also
  gets before/after captures.
- **Q4 Documentation governance.** Rewrite current behavior in CURRENT.md or guides when a decision
  changes. Keep historical implementation narration in docs/archive; do not append a second account
  of current behavior.
- **Q5 Remaining optional items.** D3 bounded windows is already directionally implemented; D5 honest
  capabilities stays in S5; D10 backup/migration and D12 sandboxed visual replies remain explicitly
  deferred or optional.
