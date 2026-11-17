# 员工模式(首席助理产品化)实现对账与设计稿

日期:2026-11-17。状态:**已施工 + 设计中**。当前工作树已完成员工骨架(E1 的主要部分),
记忆/承诺/预算/通知处于部分实现,E4 仍待办。前置 chief-m0(M1 已落地、三 agent 真机 E2E
通过,设计见 `docs/research/telegram-chief/`)保持不变。

代码模块、路径、工具名沿用 `chief`(与「圆桌 = council」同款约定:UI 名与代码名分离)。

本稿 2026-11-17 追加吸收了 BigFatFishWife / dsh-room 的可迁移机制。完整源码观察、证据索引、
细碎行为和明确不借项见 [research-bigfatfishwife.md](../research/research-bigfatfishwife.md)。

本稿现在同时承担三件事:**记录当前实际行为**、保留仍未施工的目标契约、并明确两者之间的偏差。
未标注“已施工”的内容默认仍是待办或设计目标。代码与本文冲突时先以代码为准,再回来修文档。

## 0.1 施工对账(2026-11-17 工作树)

以下以当前工作树源码为准;其中部分文件尚未提交,不能写成“已发布”。

### 已施工

| 能力 | 当前实现 |
|---|---|
| 员工注册表/迁移 | `staff.json`(原子写、坏档 fail-open、上限 128);旧 `config.chiefSessionId` 一次性迁移成一位「总管」,保留旧项目策略、agent/model 与 Telegram 行为。迁移完成标记落盘后,即使最后一位员工被删,旧 config session 也不再获得 Chief/MCP 门禁 |
| 员工会话 | 每位员工绑定普通 Chat 会话(`origin="chief"`),可修复/重建 sessionId;控制工具只对员工会话开放 |
| 员工组与档案 UI | `ProjectShelf` 顶部员工组、5 秒轮询、搜索、雇佣/编辑弹窗、暂停全部、右栏档案、删除/清空记忆 |
| 任务与 worker | 原 chief 任务账本/tee/权限卡继续使用;worker 是 `origin="chief-worker"` 的项目会话,不写 `parent_session_id`,侧栏有来源徽标和「回到员工」面包屑 |
| 状态投影 | `staff_status.rs` 与 `staffStatus.ts` 把 task 投影成 `waiting_user > failed/interrupted > running > thinking > idle`;failed/interrupted 保留 5 分钟,5 分钟无事件标 stale 但不改 running |
| Host 急停 | 注册表级 `pausedUntilUserResumes` 是总闸,新员工继承;新 task、task_send、task_resume、条件唤醒和非安全出站受影响;全部停止会连 `Proposed` 任务一起取消、关闭 worker 浏览器 tabs 并撤销 Computer Use 共享窗口 |
| 项目/并发/轮次/墙钟 | 项目白名单 fail-closed;员工档案可按项目配置只读/审批写/预授权写与 Computer Use/自我修改;全局/项目并发上限;`Proposed` 计入占用与停止;requestId、并发预留和插入在同一任务账本锁内;task send/resume 重新校验当前项目策略 |
| 只读硬门禁 | 员工 ACP 会话强制切到 Agent 自报的只读/plan 权限;worker 的 read_only 任务同样通过 permission/mode 落地,Agent 不支持时 fail-closed,不再把一句 prompt 当成沙箱 |
| 分层记忆底座 | `pinned.md`、`state.md`、`history.jsonl`、`journal/`、`index.json`、`proposals.jsonl`;原子写、索引重建、来源引用、tombstone、分文件大小上限;task 终态由 Host 追加真实 journal |
| 记忆 UI | 右栏编辑 pinned/state、查看近期 journal/proposal;proposal 可接受进 pinned/state 或拒绝;确认清空全部记忆并留下 tombstone |
| 承诺账本 | `commitments.json`;add/list/complete/cancel、到期 tick、task 完成/失败条件触发、员工工具与右栏 UI 均可管理;暂停/manual/禁后台时不消费到期事件,恢复后仍可补唤醒;员工行显示 due 计数但不改显示状态 |
| Telegram 绑定 | 档案可绑配对私聊;显式绑定即可路由,不再硬依赖全局 `mode=chief`,旧单员工 chief 回落仍兼容;任务卡通知与回复路由优先按稳定 `staffId` 找当前 session,员工 ACP 会话重建后不会回到旧 session |
| 通知闸门 | `StaffPolicy.notification_allowed` 已是 channel-neutral;`allowTelegram` 只在 Telegram 渠道检查,桌面通知不被它误伤。前端 `staffStatus.ts` 的优先级表已与 Rust 对齐。普通通知冷却/小时额度持久写员工档案,写盘失败 fail-closed;task.started 不再消耗报告预算 |
| bootstrap | 员工会话空闲时注入人设、Host 策略、项目、记忆摘要、open commitments;后端有 `bootstrapVersion`。当前 UI save 不发送该字段,Rust `serde(default)` 使每次 UI save 为 0 并重新排队,详见 §5.3 |
| 控制工具 | 当前 16 个 `marionette_*` 工具:原任务/项目/agent 工具 + memory_recall/propose、commitment_add/list/complete、staff_status、task_resume;没有 hire 或 approve 工具 |
| 事实/派生分离 | task/event/tee 仍是事实;员工行、任务条和右栏是投影;模型报告不能写回 task 状态 |
| 辞退收尾 | 辞退先确认取消该员工的活动/Proposed worker task,停止/按选项删除会话,再清理注册表并取消所有 open/due commitments(终态保留审计);同时清 wake/user/host/bootstrap 内存队列与 tee 注册 |
| worker 策略映射 | worker session id 在 ACP 启动前写入 task;写失败拒绝启动。Computer Use schema/dispatch 均按稳定 staffId + 当前项目策略硬校验,员工/task 映射丢失时 fail-closed |

### 已施工但和原设计有偏差

- **雇佣 UI 是一张分节表单,不是分步向导**;档案没有头像字段,侧栏统一使用 `UserRound` 图标。
- **`autonomy` 当前几乎只是策略元数据**:`manual` 只阻止条件承诺自动唤醒;assisted/autonomous
  没有独立的候选规划、backlog 审批或自主频率差异。
- **`maxModelCallsPerTask` 是保守下界而非精确 provider 调用数**:后端取 usage ledger 行数与
  task.turn_count 的较大值,主窗挂起时仍能靠 turn 计数兜底;但一次 agent turn 内部的多次模型
  请求没有逐次预占。
- **`maxCostPerTaskUsd` 依赖 Agent 上报成本**:send/resume 前和 turn 完成时会检查已报告 USD,
  但没有请求前预占;不上报 `reportedUsd` 的 Agent 无法触发成本止损。
- **`maxWallMinutesPerTask` 是真实启动 deadline**,但心跳还会把 `lastEventAt` 超过上限解释成
  “无进展”;恢复会重新开一段墙钟。两者语义需在 UI/账本里写清。
- **通知不是完整渠道矩阵**:普通通知预算已持久化,但桌面/Telegram 仍共享同一份普通频率额度,
  没有邮件/媒体渠道、持久抑制审计或完整“为什么没发”UI;`staffStatus.ts` 的通知投影仍主要是
  纯函数/测试工具。
- **承诺条件不是通用表达式**:现有 scheduler 只识别 Host 产生的
  `task:<taskId>:completed|failed` 精确字符串;模型创建其他 condition 不会自动求值。
- **到期唤醒队列只在进程内去重**:每 tick 会重新消费所有 due 中尚未领取的承诺,暂停期间不再
  先标 due 后丢失;但如果进程在 host queue 入队后、真正发送前退出,内存队列仍可能丢这一拍。
- **员工行已接 due commitment 计数**,但仍不展示完整 waiting reason、lastEventAt、stale 与抑制原因。
- **任务 journal 是 Host 事实记录,不是模型复盘**:终态会自动追加 taskId/项目/状态/摘要;更丰富的
  自动反思、来源展示与 proposal 批量整理仍待做。proposal 已有逐条 accept/reject。
- **记忆注入没有总预算**:bootstrap 读取完整 pinned/state 和最多 3 份 journal;recall 最多
  7 天(工具最多 30/60 天),只按文件字节上限,未实现 7/60 天 detail/gist/dormant 和 3500
  字式总预算。
- **清空记忆是永久 tombstone**:pinned/state/journal/proposals 均被标记,当前没有“忘记后重新
  启用记忆”的恢复入口;用户再次编辑 pinned/state 或模型 propose 会被 tombstone 拒绝。
- **员工搜索只按 name/title/主任务过滤**,状态 chip 不计入员工行;员工行不显示 waiting reason、
  lastEventAt、stale 或通知抑制原因。
- **档案里的 `memoryRoot`/`commitmentLedger` 不是可重定位开关**:当前 memory 固定走
  `chief/memory/<staffId>/`,commitments 固定走 `chief/commitments.json`;字段会被规范化和保留,
  但改路径不会迁移数据。
- **Telegram transport 仍是单 chat**:显式员工绑定已解耦 `mode=chief`,但底层设置仍只有一个
  已配对 chat_id,因此多位员工不能同时绑定不同私聊;出站目标仍受当前 live chat 限制。
- **STAFF_STATUS 工具描述略超前**:实现返回投影和 policy,不直接带 due commitments 或预算余额;
  员工需要另调 `commitment_list`。
- **自我修改仍是“Host prompt + 独立权限卡”而非 git 沙箱**:任务会要求 worktree/diff/tests、禁止
  push/merge/tag/release,但没有在 Host 侧强制每个 git 命令;这部分不能当成已硬隔离。
- **员工直接会话依赖 Agent 提供只读/plan 档**:支持该档的 Agent 会被强制只读;不支持的 Agent
  会拒绝启动员工会话。员工自己仍拥有 Agent 原生只读工具,项目写入只能派 worker。

### 未施工 / 待办

- 空闲候选规划、已批准 backlog、重试条件、no-op 评分和循环/重复进度保护;
- 真正的 provider 级模型调用计数、成本请求前预占、预算耗尽状态、可恢复续期和运行中即时止损;
- 按渠道的桌面/Telegram/邮件/媒体矩阵、持久抑制审计、“为什么没发”UI 与去重;
- 更丰富的自动复盘、proposal 批量整理、来源与更新时间展示、单条删除、导入/导出;
- 健康/审计页:ledger、memory、scheduler、heartbeat、worker、Telegram、预算和最近抑制;
- 通用承诺条件求值、host queue 持久化和“入队后崩溃”恢复;
- E4 的长期常驻验收、成本/图片附件/worker 运维和真机 soak;
- 与 `CURRENT.md` 同步的正式产品入口/发布说明;代码尚未合入时索引不能写成“已发布”。

### 明确不照搬

BigFatFishWife 的恋爱/陪伴人格、精力/睡眠债/孤独/心情、固定问候文案、正则推断关键状态;
dsh-room 的 2.5D 房间、Live2D、家具/天气/世界协议和 AGPL 角色素材。只借事实与投影、
闸门与预算、可检视数据和现有 Marionette 视觉语言。详见 §10 与调研稿 §13。

当前读代码入口:

- 后端注册表/策略/记忆/承诺:`src-tauri/src/chief/{staff,staff_commands,memory,commitments}.rs`;
- 状态投影与控制工具:`src-tauri/src/chief/{staff_status,tools,broker}.rs`;
- 前端 roster/档案/雇佣:`src/components/{StaffRoster,StaffSection,StaffHireDialog}.tsx`、
  `src/app/useStaffController.ts`;
- 前端状态与通知纯函数:`src/lib/{staffStatus,staffUi}.ts`;
- 回归:Rust `cargo test --bin marionette chief::` 当前 55 项通过;前端
  `npx tsx scripts/verify-staff-status.mts` 当前 18 项通过。二者都不是完整 UI/真 agent E2E。

## 0. 一句话定位

> **Marionette:本地多 Agent 工作台,自带可雇的员工,替你把活儿派给车间里所有 Agent。**

员工是前台,车间(会话树/项目/浏览器/Computer Use)是机器间。**没有模式切换**——和员工聊天
就是在现有界面里聊天:中栏是那段对话,侧栏照常在,它派的 worker 以普通会话形式落在项目里。
"切换回现在的界面"这个概念不存在,因为用户从没离开过它。

与收纳盒/圆桌的区别:那些是「你去拜访的地方」,员工是「一直坐在那儿的人」——后者要的是
**常驻存在感**(侧栏置顶、首页可达),不是一个新界面壳。

## 1. 要解决的问题

chief-m0 把编排后端建好了(broker/runner/store/tee/policy/控制工具/waiting_user),但原本
被 Telegram 框住:`chief_inbound_text` 只被 Telegram poller 调用,`mode=chief` 是 telegram
的模式位,本地 UI 里的入口是右栏一张「远程助理」配置卡。用户在桌面端打开 Marionette,
感觉这是一个"Telegram 功能的设置页",而不是"我有一位助理"。

Varin(见 §2)恰好从外部验证了反转:**assistant-first 是产品本体,远程只是渠道**。它根本没有
Telegram——主 Bot 就是产品的门面,用户打开应用第一眼就是和 lead bot 对话、旁边看着进行中
的工作。chief-m0 的架构其实已经把正确的缝留好了(telegram_remote 只做路由,
broker/store/tee/policy/tools 全是渠道无关的)。当前工作树已经把这层产品化到员工注册表、
侧栏员工组和本地档案 UI;剩余问题从“有没有员工对象”变成“长期行为契约是否真的兑现”。

## 2. 业界对标(2026-11-17 调研)

### 2.1 Varin(github.com/Astrmira/Varin,AGPL-3.0)

开源「个人超级智能体」:常驻主 Bot + 派工给外部 Agent(Codex/Claude Code/Pi/ACP 兼容)
+ 三视图(Varin 对话 / Workbench 任务 / IDE)+ Rust 内核耐久执行(Thread/Run/Operation 账本、
checkpoint/resume 契约、消息队列、重启恢复、后台上下文压缩)。pre-1.0,Electron + Web + 移动端。

**验证了什么**(与我们方向的交集,可直接抄模式):

- 常驻 lead session 经 `dispatch/wait_child/child_status` 控制子任务,worker 是外部 Agent、
  保留自己的账号/模型/工具——与 chief-m0 的「Chief 会话 + marionette_* 工具 + 普通 worker
  会话」同构;
- 主视图 = 对话在前台、进行中工作并排、可点开进任务详情——这是它最大的 UI 贡献;
- 任务账本比聊天消息长命,重启可恢复。

**IA 教训**(不抄的部分):它把 Thread(对话)挂在 Project 下面,Project 又同时是工作区——
「跟谁聊」和「活儿落在哪」搅在一起,定位显得混乱。反推出我们的硬规则(§8 红线 2):
**人和事分开放**——侧栏先人(员工组)后事(聊天/项目),员工对话永远不进项目。

**明确不取**:耐久执行做到它的深度(Run/Operation/checkpoint 契约——tee + 账本 +
interrupted/resume 对单机产品够用)、扩展 SDK、多设备/手机端、IDE 视图(Monaco——Marionette
是壳不是 IDE,文档/浏览器 surface 已覆盖大半场景)、AGPL 代码(**一行都不能看进来**,只借鉴
模式;chief-m0 本就是独立设计,与当年对 DS Bot 的论证同款,见 04-reference-products.md §6)。

### 2.2 Grok Bot(产品价值基准,已有调研)

04-reference-products.md §2 已下结论:"always-on agent + own execution environment +
natural-language delegation + approval loop" 是完整产品而不是聊天壳;内部形态就是
chief of staff 管各领域 specialist;记忆随长期合作增强。

本设计新增的拆解——**助理感 = 提示词承诺 × 宿主兑现**(§4 行为契约表)。Grok bot 独一档
是因为它每一行承诺的右列(宿主机制)都真做了;DS Bot / Tobs 这些只有交互皮肤,右列是空的,
所以"聊起来像助理,用起来是 agent"。

### 2.3 DS Bot / 其它 Telegram assistant

交互像,本质还是「agent + AI」:单轮对话驱动、无事件驱动唤醒、无任务账本、无车间可见性。
它们的定位恰好从反面说明:**没有右列,人设 prompt 写得再好也会塌**。

### 2.4 BigFatFishWife / dsh-room(2026-11-17 本地代码复核)

BigFatFishWife 是 AstrBot 上的长期陪伴实现,另有 dsh-room 用会话事件驱动可见角色。它的产品玩法
不属于 Marionette,但宿主机制提供了十个直接可借的结构:

1. **原始流水与记忆分层**:append-only JSONL、固定事实 pinned、短状态、状态历史、日记和召回索引
   分开;最近记忆全文注入,旧记忆只给概要,超旧内容按需检索;
2. **纯本地时间线**:墙钟推进、活动 due 只触发一次、重启恢复、预计时间与硬超时分离,不靠模型
   每分钟刷新状态;
3. **承诺账本**:promises/alarms 与 task 分开,到点可唤醒员工,完成后写回,避免答应过的事消失;
4. **两道闸门**:允许后台做事不等于允许主动打扰;消息、语音、图片和邮件分别过出站政策;
5. **明确事实才生成需求**:用户没说勿扰就不猜勿扰,沉默不被解释成生气或没吃饭;状态带到期时间;
6. **中央策略档位**:一个模式同时投影调用数、工具轮数、墙钟、通知数、媒体权限,避免散落开关漏改;
7. **预算与循环保护**:请求前预占额度、短窗口异常检测、turn 与墙钟双上限、预算耗尽可解释;
8. **候选计划与休息**:未完成工作有优先级,重复进度/卡住会扣分,「不做事」永远是合法候选;
9. **事件到状态投影**:waiting > error > running > thinking > idle,终态自动衰减,陈旧态清理;
10. **事实与复盘分离**:任务账本是客观真相,员工复盘和模型摘要只是派生文本,不能改写完成状态。

直接结论:

- 员工不只是「会话 + 档案 + 工具」,还需要**分层记忆、承诺账本、Host 策略**;
- 员工行显示的是任务/worker 的聚合投影,不是主会话单点状态;
- 主动汇报必须经过通知闸门和预算,不能因为员工「想说」就出站;
- 空闲时只能处理有据可查的 backlog/承诺/重试事项,没有活就待机;
- 不做 2.5D 房间和拟人生活模拟,只借状态与车间信息结构;
- dsh-room 为 AGPL-3.0,角色素材另有限制;只借模式,不复制代码或素材。

参考项目的纯规则验证:生活插件 58/58、记忆与调度脚本 33/33、dsh-room 所选核心逻辑 33/33。

## 3. 产品形状

### 3.1 侧栏 IA

```
┌──────────────┐
│ 🧑‍💼 阿珍 · 总管    │ ← 员工组(置顶、常驻;显示名字+职务)
│ 🧑‍🔬 老王 · 调研    │
│ ＋ 雇一位          │
├──────────────┤
│ 聊天              │ ← 现有一切原样
│ 项目 / 收纳盒…     │
└──────────────┘
```

- 员工组渲染在「聊天」分组之上(与收纳盒分组同款机制,方向相反:收纳盒在下、员工在上);
- 员工行 = 名字/职务 + **聚合运行状态**。状态由其 task/worker 事件派生,
  优先级为 `waiting_user > failed/interrupted > running > thinking > idle`;等待用户的待办比普通运行更醒目;
- 贴边态:员工入口进 ⋯ 菜单或窄条列表,DockPage 整页承载员工对话(与收纳盒同款)。

当前对账:普通窗口员工组和命令面板跳转已施工;侧栏使用通用 `UserRound` 图标,没有头像;
贴边态只显示员工计数/暂停入口,完整员工 roster 尚未搬进 Dock 菜单或 DockPage。

### 3.2 员工的定义

一位「员工」是六项稳定资产的绑定,另有两项派生视图:

1. **一段持久对话**——就是个 session(`origin:"chief"`,rails/转录/恢复/权限卡全免费)。
   雇了就在,随时回来继续聊,"它记得你们之间的事"(转录即短期记忆);
2. **一份档案**——名字、头像、职务描述(人设 prompt)、绑哪个 agent、模型偏好、
   能碰哪些项目(项目策略沿用 chief-m0 的 remoteMode 白名单)、并发上限;
3. **一套控制工具**——当前 16 个 `marionette_*` MCP 工具,门禁从
   「唯一 chiefSessionId」改成「员工注册表」(§7 delta 1);
4. **一套分层记忆**——固定事实、当前状态、任务/合作复盘、历史审计分开,模型只能提交候选,
   不能直接改固定事实(§6);
5. **一份承诺账本**——用户设置或员工明确答应的跟进、提醒、条件和承诺,与 worker task 分开(§6.4);
6. **一份 Host 强制策略**——自主档位、项目白名单、并发、turn/墙钟/成本/通知预算、勿扰和持久急停
   (§5.1、§7)。

派生视图:

- **员工状态投影**——从 task/worker/approval 事件派生,不单独写事实(§3.7);
- **员工汇报/复盘文本**——可以引用事实,但不能替代 task ledger 或 change/test 证据。

### 3.3 雇 / 改 / 辞

- **雇**:四组信息——它是谁(名字+职务+人设,可从出厂模板起步)/背后是谁(agent+模型)/
  能碰哪些项目(白名单+权限档)/怎么工作(StaffPolicy、通知与预算)。提交即创建
  `origin:"chief"` 会话并注入 bootstrap(§5);
- **改**:档案页编辑人设/模型/白名单/策略;人设变更后可「重新生效」(重注入 bootstrap prompt);
  记忆与承诺在各自视图管理,不塞回人事表单;
- **辞**:确认后删会话、注册表清行;任务账本默认保留供审计,记忆默认保留并可单独导出/删除;
  不把"辞退"和"清除记忆"合并成同一个不可恢复动作。

当前对账:已施工为**单个分节弹窗**,不是多步向导;新建时由 Host 创建/复用 Chat 会话,编辑时
保留 staffId/sessionId/记忆路径。辞退先取消该员工的活动 worker task 和所有 open/due
commitments(终态保留审计),再停止/按选项删除员工会话;任务和记忆默认保留,另有权二次确认的
“清空记忆”动作。“记忆可单独导出”尚未实现。

### 3.4 聊天体验(Grok bot 感)

中栏就是普通会话视图。体验差异全部来自「普通会话 + 控制工具 + 任务回话」三件套:

| 用户动作 | 背后机制(chief-m0 已有) |
|---|---|
| 随时聊想法,聊着聊着派活 | 自然语言进 Chief 会话,它自己决定派不派(`task_start`) |
| "已交给 codex 在处理了" | `task_start` 返回任务卡,员工回话 |
| "那个活儿干完了吗" | `task_status` / `task_collect`;完成事件经唤醒收件箱送回对话 |
| 干完/挂了主动回来汇报 | 唤醒收件箱:worker 事件 → 员工醒来 → 回话(合并 ≤5 条/tick) |
| 随时叫停 | 急停/`task_cancel` 已有;员工模式补持久 `pausedUntilUserResumes`,阻断后续任务启动和非安全出站 |
| 它来要授权 | 权限卡出现在会话流里(waiting_user 直连任务状态) |
| 它自己上手干活 | 员工是完整 agent 会话,可以不派工直接干("直接去车间") |
| "你还记得吗" | `memory_recall` 返回 pinned/state/近期复盘及来源;没找到就说没找到 |
| "周五提醒我" / "CI 绿了再继续" | commitment ledger 建 due/condition 事项,到点唤醒员工 |
| "为什么没动静" | 员工状态投影显示 lastEventAt、lastProgressAt、waiting 原因和通知是否被抑制 |

### 3.5 车间与 worker

- worker(`origin:"chief-worker"`)是员工派出去的一次性活儿:普通会话、顶层可见、带「远程」
  徽标、落在目标项目里——**这是事,不是人**;
- worker 永远不进 staff registry;任务完成后工作卡可以衰减/归档,员工仍长期存在;
- 从员工对话的任务条点任务 → 跳 worker 会话;worker 会话头部带「来自 XX」面包屑 → 回员工;
- 分屏/撕窗/merge 对 worker 会话照常适用,零特判。

### 3.6 首启引导

- 新装:首屏是员工组的「雇第一位」引导卡(推荐出厂总管模板,一键创建),不强制——
  跳过后侧栏仍留「＋雇一位」;
- 老用户升级:一次性提示卡,不自动创建任何会话。

当前对账:员工组为空时显示“雇第一位员工”按钮,顶部始终有 `+`;尚无老用户升级提示卡,
也不会自动创建员工。旧单 chief 迁移在读取注册表时静默完成。

### 3.7 员工状态投影

员工状态必须是投影,不是第二份可写业务状态。输入来自:

- task status;
- worker ACP 事件;
- permission/question/plan approval;
- staff registry;
- commitment due;
- Host notification policy。

首版显示档位:

~~~text
waiting_user > failed/interrupted > running > thinking > idle
~~~

规则:

- 同一 task 的 waiting 原因从现有权限/问题/计划审批链路读取;
- `failed` 与 `interrupted` 在 UI 上区分,不能合并成「出错了」;
- 终态保留一小段时间供用户看到,随后回落到下一条或 idle;
- 长时间无新事件的 running 标记为 stale,但不自动伪造失败;
- 多个 worker 时,员工行优先显示最需要用户处理的一个,并显示其余计数;
- 点击员工状态进入对应 task/worker,不是打开新的详情数据库。

当前对账:`StaffDisplayState`、优先级、failed/interrupted 区分、5 分钟终态保留和 5 分钟 stale
标记已在 Rust/TS 两端实现;Rust 投影含 waiting reason、last event/progress/report 和 counts,
但员工行目前只消费状态/主任务/计数,右栏未完整展示这些解释字段。

## 4. 行为契约(助理感)

**每一行都是「提示词怎么写 × 宿主拿什么保证」。提示词只承诺宿主能兑现的行为**(红线 7)。

| 助理感 | 提示词侧 | 宿主侧 | 当前工作树 |
|---|---|---|---|
| 接单即负责 | 把目标当自己的事:一句话复述计划 + 说清派给谁、为什么 | 任务账本(状态/归属/幂等/可恢复) | ✅ |
| 主动汇报 | 承诺"完成/失败我都会回来"——对话不靠用户下一轮续命 | 唤醒收件箱(worker 事件 → 员工醒来) | ✅ 最核心的一条 |
| 汇报像人话 | 三行摘要;结论带证据(文件/测试/diff);给选项必带推荐 | `task_collect` 提供摘要/工具计数/文件/失败;员工仍可能自由表述 | 部分:事实可查,格式未强制 |
| 知道何时不问 | 只在答案会改变结果时问;能推断的不烦人 | 权限卡/waiting_user 只在真判断点触发 | ✅ |
| 记得你 | 引用偏好与前科("上次你说别动 CI") | 档案级持久记忆,可查可改可忘(§6) | 部分:存储/UI 已有,自动写入与晋升未接 |
| 诚实 | 永不无证据地说"完成";区分 完成/失败/我停了/不知道 | 任务状态是宿主真相,不是模型嘴里的 | ✅ |
| 有分寸 | 知道能碰哪些项目、什么必须请示 | 项目白名单 fail-closed | ✅ |
| 答应的事有账 | 不把"稍后提醒你"停在聊天里 | commitment ledger + due/condition 唤醒(§6.4) | 部分:定时/精确 task 条件已有,通用条件和持久唤醒队列未完成 |
| 不刷存在感 | 普通进展不汇报;没有新事实就不发消息 | 事件驱动唤醒 + channel-neutral 通知闸门 + 冷却(§7.2) | 部分:闸门已完成,持久账本/抑制 UI 待补 |
| 有预算 | 长任务知道会何时收尾,预算耗尽如实停下 | turn/墙钟/成本/通知预算,请求前预占(§5.1、§7.2) | 部分:turn/墙钟已卡,成本只在 send/resume 前查,无请求前预占 |
| 可解释地停 | 能说清在等什么、停了多久、是否被静默 | 员工状态投影 + waitingReason + notification gate(§3.7) | 部分:后端有投影,员工行/右栏未展示完整原因 |
| 记忆有证据 | 区分用户确认、任务事实和模型推测 | pinned/state/journal/history + sourceRefs(§6) | 部分:数据有来源,UI 未展示,proposal 不能晋升 |
| 不制造工作 | 没活就待机;空闲规划只从 backlog/承诺/重试中选 | candidate planner + no-op/休止(§7.2) | ❌ |

当前已经越过了“只有人设提示词”的阶段,但仍不能把部分实现写成行为契约已完成。尤其要区分
纯函数投影/测试通过、后端命令可调用、用户真的在 UI 上看见和恢复——这三层验收必须分别过。

## 5. 员工档案与出厂人设

### 5.1 档案(staff registry 条目)

```jsonc
{
  "staffId": "staff_…",
  "name": "阿珍",
  "title": "总管",
  "persona": "…(人设 prompt,见 §5.2)",
  "agentId": "codex",
  "model": "provider/model | null(= agent 默认)",
  "allowedProjects": [
    { "projectId": "project_…", "remoteMode": "read_only", "allowedAgents": [],
      "maxConcurrentWorkers": 1, "maxTaskMinutes": 120,
      "allowComputerUse": false, "selfModification": false }
  ],                                      // 实际序列化是 ProjectPolicy 对象数组;空表 = 无权访问
  "maxActiveTasks": 1,
  "sessionId": "session_…",           // 持久对话
  "telegramChatId": null,             // 可选:某位员工绑定某个私聊(§7 delta 4)
  "autonomy": "assisted",             // manual / assisted / autonomous
  "maxTurnsPerTask": 30,
  "maxWallMinutesPerTask": 120,
  "maxModelCallsPerTask": 0,          // 0 = 不设独立上限,仍受 Host 总预算约束
  "maxCostPerTaskUsd": 0,
  "maxReportsPerHour": 4,
  "notificationCooldownMinutes": 30,
  "quietHours": "23:00-08:00",
  "allowBackgroundWork": true,
  "allowTelegram": false,
  "pausedUntilUserResumes": false,
  "memoryRoot": "memory/staff_…",
  "commitmentLedger": "commitments.json",
  "bootstrapVersion": 1,
  "notificationWindowStartedAt": 0,
  "notificationsInWindow": 0,
  "lastNotificationAt": 0,
  "createdAt": 0,
  "updatedAt": 0
}
```

存放:`~/.marionette/chief/staff.json`(沿用 chief store 的原子写/测试 root-override 纪律,
员工数上限 128)。`StaffPolicy` 的字段在 JSON 中扁平展开;注册表顶层另有全局
`pausedUntilUserResumes`。注册表是控制工具门禁的事实,旧 `config.chief_session_id` 只用于一次性
迁移。正常读取不重写文件;坏档保留原文件并进入 fail-closed sentinel,不会被 legacy 配置静默覆盖。

策略原则:

- 所有执行/通知入口读取同一份 StaffPolicy,不能在多个配置面各存一套;
- 0 值必须有统一语义,UI 中显示"继承 Host 默认"还是"无限制";
- `pausedUntilUserResumes` 属于 Host,不由员工工具修改;
- 员工可读取自己的策略摘要,但不能写策略;
- 预算或急停变化要写审计事件,重启后仍生效。

当前对账:注册表、策略字段、唯一 staffId/sessionId/Telegram 绑定、注册表级持久暂停和坏档
fail-closed 已施工;策略变更的**审计事件**尚未真正落账,UI 主要靠注册表 `updatedAt`/debug log。
`autonomy` 的实际执行面见 §0.1,不要仅凭表单选项认为三档已有不同调度器。

### 5.2 出厂人设(产品资产,不是用户配置)

内置一份打磨好的「总管」模板,雇第一位员工时默认填充。骨架(行为契约 = §4 表的 prompt 化):

- **角色**:用户的本地首席助理,管理一群 CLI Agent 与项目;对话风格少而明确;
- **决策循环**:理解目标 → 判断自己干还是派工(说理由)→ 报告进展 → 交付证据;
- **汇报格式**:三行摘要 + 证据(文件/测试结果/diff 路径);选项必带推荐;
- **请示分寸**:只在答案会改变结果时问(列正反例,沿 01-product.md §6.2);
- **诚实条款**:任务状态以工具查询为准,永不无证据声称完成;
- **边界条款**:只碰白名单项目;危险操作走权限卡;不扩自己的权限;
- **不制造工作**:没有用户任务、到期承诺、已批准 backlog 或明确重试条件时,应该待机而不是找事做;
- **承诺条款**:答应提醒/跟进/条件触发时写入 commitment ledger,不能只停在聊天里;
- **预算条款**:接近 turn/墙钟/成本上限时主动收尾;预算耗尽如实报告,不假装仍在执行;
- **记忆条款**:当前用户指令高于旧记忆;记忆没找到说没找到;模型推测不能写成用户确认的事实;
- **通知条款**:普通进展不刷屏;完成、失败、等待授权和到期承诺才进入通知闸门。

用户雇的第二位起自由写职务描述(「调研员:只读项目、产出对比表」之类)。
模板随产品版本迭代——**这是要像 UI 一样认真打磨的产品资产**(Grok 的"聪明感"大半在此)。

当前对账:雇佣弹窗只内置一段短 `DEFAULT_PERSONA`(本地助理 + 短汇报 + 只在该提问时提问 +
不制造工作),上面的完整出厂总管模板尚未成为独立产品资产;员工档案也没有头像/展示图片字段。
这不阻断骨架,但意味着“助理感”仍偏工具型,不应宣称出厂模板打磨完成。

### 5.3 注入点(已施工;刷新由当前 save 语义触发)

当前实现已有 **bootstrap prompt**:员工会话空闲时,Host 注入人设、Host 策略、项目策略、
pinned/state/最近 journal、open commitments 和行为契约;用档案里的 `bootstrapVersion` 做幂等标记。

当前 UI 的 `StaffProfile` 不带 `bootstrapVersion`,而 Rust 的 `StaffProfile` 对该字段使用
`serde(default)`;因此每次 UI 保存都会反序列化成 0,`chief_staff_save` 随后排队 bootstrap。
也就是说“保存 agent/model/项目/预算/通知策略”在当前 UI 路径上会重新注入;persona 或新建
session 也会重置。仍需明确的是:这不是字段级/策略版本契约,后台调用方若显式发送旧版本仍可能
跳过注入;策略/审计变更没有独立事件账本,后续应决定“哪些变化必须刷新、刷新版本从哪里取”。
不写 agent 自家配置文件的红线不变,agent/模型继续走会话偏好和 runner 更新。

## 6. 分层记忆、承诺与长期合作

### 6.1 存储结构

每位员工使用独立目录,不搞一个无限增长的单文件,第一版仍不使用向量/RAG:

~~~text
~/.marionette/chief/memory/<staffId>/
  pinned.md              # 用户确认的长期事实/硬偏好,普通整理不可覆盖
  state.md               # 近期合作上下文、当前关注点、待确认事项
  history.jsonl           # 每次状态修改的来源、摘要、操作者和时间
  journal/
    YYYY-MM-DD.md         # 合作日或任务级复盘
  index.json               # 文件元数据、更新时间和可重建索引
~~~

- pinned/state/journal 全部纯文本,可读、可 diff、可编辑;
- 原始事实仍来自 session transcript、task ledger 和 chief tee,不另建第二套会话库;
- index 损坏时可从目录扫描重建,不能因此丢正文;
- 删除采用 tombstone/删除账本,避免整理或同步把已删记忆自动补回。

当前对账:目录、原子写、索引重建、来源引用、history 和 tombstone 已施工;
`pinned.md` 64 KiB、`state.md` 32 KiB、单 journal 128 KiB、history 1 MiB/2000 行、
proposal 128 KiB 等是当前硬上限。`proposals.jsonl` 已加入目录,但见 §6.3 的晋升缺口。

### 6.2 分层与注入预算

记忆按以下优先级注入,总量和分层各自有字符预算:

~~~text
Host 硬策略
> 当前用户消息 / 当前审批
> 任务账本最新状态
> pinned
> state
> 最近 journal
> 更早 journal 的概要
> 模型推断
~~~

- pinned 不可被自动整理覆盖;冲突写入"待确认",等用户决定;
- 最近任务/合作复盘可整篇注入,较早内容只给概要;
- 更旧内容按需全文搜索,不默认塞进上下文;
- 当前用户明确推翻旧偏好时,当前轮以用户为准,并把冲突记录下来;
- 记忆没找到时只能说"没找到",不能说"从未发生"。

当前对账:**未按这一节的 detail/gist/dormant 与总预算实现**。`memory::recall` 会读完整
pinned/state,再按日期取最近 N 天 journal;bootstrap 取最近 3 份完整 journal。当前只有单文件
字节上限,没有总上下文预算、7–60 天概要或 60 天以上休眠档,也没有 UI 的“当前是否注入”标记。

### 6.3 写入规则

新增控制工具:

- `memory_recall`:读取本员工 pinned/state/近期复盘和来源;
- `memory_propose`:提交候选事实、偏好、经验或决策,默认写入候选区;
- `memory_forget`:默认不开放给员工;删除由用户在档案页明确操作。

规则:

- 只有用户明确确认,或 Host 能引用真实事件证据时,proposal 才能晋升 pinned;
- 任务完成、失败、审批和文件变更以账本为准,记忆不能覆盖;
- 模型摘要失败不影响任务本身,可之后重试;
- 自动整理只消费上次游标后的新事件,失败时保留原文和失败输出;
- 日记/复盘只写真实发生的事,不写工具残骸、瞬时状态和重复通知;
- sourceRefs 记录 sessionId/taskId/eventSeq/transcript 时间窗等来源。

当前对账:`memory_recall`、`memory_propose` 工具已施工,员工不能直接编辑 pinned/state;
proposals 带来源和 pending 状态;员工档案页可 accept 到 pinned/state 或 reject;task 终态会由
Host 追加真实 journal。**未施工**:模型摘要失败重试、“只消费游标后的新事件”的整理器和更丰富
的自动复盘。`memory_forget` 没有开放给员工,删除只走 Host UI。

### 6.4 承诺账本

承诺与 worker task 分开,用于 Chief of Staff 的"答应过的事":

~~~jsonc
{
  "commitmentId": "cmt_…",
  "staffId": "staff_…",
  "text": "CI 通过后继续合并",
  "kind": "follow_up | reminder | promise | condition",
  "dueAt": 0,
  "condition": "ci_green",
  "sourceRef": "session:…/event:…",
  "status": "open | due | done | cancelled | expired",
  "createdAt": 0,
  "completedAt": 0
}
~~~

- 到期/条件满足通过 scheduler 唤醒员工,由员工生成自然语言回复;
- 承诺完成写回账本,避免重复提醒;
- 用户取消、员工无法兑现或条件永远不满足时要显式结束;
- task completed 不等于 commitment done;
- 用户明确要求和员工明确答应的承诺都应可查、可改、可取消。

当前对账:账本、原子写、add/list/complete/cancel、2 秒 heartbeat 到期检查、task completed/failed
条件触发、右栏可查/完成/取消已施工。**偏差**:只识别 Host 生成的
`task:<taskId>:completed|failed` 字符串;任意 condition 不会求值。暂停/manual/禁后台时保留
due 供恢复后补唤醒;领取去重仍是进程内存,host queue 入队后崩溃可能漏一拍。

### 6.5 档案页与隐私

- 档案页展示 pinned/state/近期 journal/待确认 proposal/commitments;
- 所有条目显示来源、更新时间、作用域和状态;
- 提供编辑、删除、清空三类动作,清空是独立危险操作;
- 「可检视、可改、可忘」:删除后保留 tombstone,自动整理和同步不得复活;
- 记忆默认不进 transcript、不进 token 台账、不上 Telegram;员工在回复中引用属于用户可见输出。

当前对账:可编辑 pinned/state、查看 journal/proposal、accept/reject proposal、清空全部并留
tombstone 已施工;没有逐条删除、单条编辑、来源/更新时间展开、导入/导出。更需注意:清空后
没有“重新启用记忆”的 Host 操作,而 tombstone 会阻止后续 pinned/state/proposal 写入;这是
当前实现风险,不应包装成完成的隐私模型。

## 7. 架构:chief-m0 资产与 delta

### 7.1 资产盘点(复用面 + 新增层)

| 资产 | 位置 | 状态 |
|---|---|---|
| 编排核心(生命周期/幂等/并发/心跳/唤醒收件箱/用户队列) | `chief/broker.rs` | 已复用,`chief_session_id` 参数语义变「派单员工」 |
| ACP 后端驱动(start/prompt/cancel/收尸,主窗挂起可用) | `chief/runner.rs` | 已复用 |
| 任务账本 + 事件账本(tasks.json / events.jsonl,重启恢复) | `chief/store.rs` | 已复用 |
| durable event tee(会话事件真相,前端回填) | `chief/tee.rs` | 已复用 |
| 项目策略白名单(fail-closed) | `chief/policy.rs` | 已复用,按 staffId 取档案里的白名单 |
| 控制工具(session-aware MCP 门禁) | `chief/tools.rs` | 当前 16 个;员工扩展 6 个,门禁查注册表 |
| waiting_user 状态机(权限/提问/计划卡直连任务状态) | broker + acp.rs 挂点 | 已复用,UI 显示简化为状态 chip |
| Telegram chief 模式(路由/聚合/急停/游标) | `telegram_remote.rs` | 已降级为渠道并支持员工私聊绑定 |
| 前端任务区/配置卡 | `ChiefSection.tsx` / `StaffSection.tsx` | 任务区与档案 UI 已接;配置卡仍保留远程助理入口 |
| staff 状态投影 | `staff_status.rs` + `staffStatus.ts` | 已新增只读投影;部分字段尚未上屏 |
| StaffPolicy + 通知闸门 | `staff.rs` + `staff_commands.rs` + 出站路径 | 部分:持久暂停、channel-neutral gate、Telegram 独立开关已做;持久通知账本/抑制 UI/完整渠道审计仍待完善 |
| 分层记忆 | `chief/memory.rs` + `memory/<staffId>/` | 已新增,不改 tee/transcript 语义 |
| commitment ledger | `commitments.rs` + scheduler | 已新增,和 task ledger 分离 |
| 空闲候选规划 | broker scheduler + 纯规则 | 未施工,只保留设计 |

### 7.2 delta 清单(按施工顺序)

1. **员工注册表(已施工)**:`config.chief_session_id`(单数)→ `staff.json`(§5.1);
   `is_chief_session` → `is_staff_session`(注册表查询,运行时注册表 ∪ 档案持久表);
   `ensure_chief_session` → `ensure_staff_session(staffId)`(档案存在则复用,否则按 §5.3 创建)。
   Rust 侧先完成身份门禁,broker 其余按 staffId 参数传导;迁移完成标记是权限屏障——最后一位
   员工被辞退后,旧 `config.chiefSessionId` 不再获得控制工具;
2. **员工状态投影 + 持久急停(已施工,展示仍有偏差)**:新增只读 selector/命令,把 task、worker
   事件、approval 和 commitment 事实投影成 §3.7 的对象;commitment due 已进入员工行计数;
   `chief_stop_all` 之外增加注册表级
   `pausedUntilUserResumes`,阻断新 task、自动唤醒和非安全出站。恢复需要明确操作;
3. **侧栏员工组 + 雇佣 UI(已施工,非分步)**:`ProjectShelf` 顶部新分组(收纳盒分组同款机制,方向相反);
   单弹窗分四节 = 员工档案、Agent/模型、项目权限、StaffPolicy;UI 仍沿用现有视觉语言;
4. **与 Telegram 解耦(已完成入站/策略路由,transport 仍单 chat)**:员工注册表和本地 UI
   不要求 Telegram;显式 `telegramChatId` 可直接路由,不再要求全局 `mode=chief`;relay 模式照旧;
   多员工同时绑定多个私聊仍待 transport 升级;
5. **bootstrap 人设注入(已施工,刷新待补)**(§5.3):注入人设、行为契约、策略摘要、可用项目和 agent、记忆摘要;
   幂等标记写账本,不重放;
6. **任务条 + 承诺条 + 面包屑(部分)**:员工会话激活时右栏显示 task、记忆和 open commitments;
   worker 会话头部「来自 XX」回跳;点击任务/状态直达真实 worker;
7. **StaffPolicy + 通知闸门(已施工,精确 provider 预算待补)**:集中实现并发、turn、墙钟、
   模型调用下界、成本检查、持久通知频率、勿扰、Telegram 例外和持久急停;所有出站路径读取
   同一决策,生成或写盘失败时宁可不发;
8. **分层记忆存储 + recall/propose(已施工)**:实现 §6.1–§6.3;第一版不做向量,
   档案页支持查看/编辑/清空、proposal accept/reject;pinned 只能由用户或 Host 明确晋升;
9. **commitment ledger + scheduler(已施工,queue 持久化待补)**:实现 add/list/complete/cancel;
   到期或精确 task 条件满足时唤醒员工,暂停/manual/禁后台时留到恢复后补唤醒;task 完成不能自动
   勾掉 commitment;
10. **空闲候选规划(未施工)**:只从 due commitments、用户批准 backlog、满足条件的重试和明确周期检查中
    生成候选;no-op 永远是合法结果;重复进度/无新证据时禁止继续唤醒;
11. **健康与审计(未施工)**:显示 ledger/memory 可写、scheduler/Telegram/worker 状态、预算剩余、
    最近被抑制通知和持久急停;恢复时先核对账本再启动自动跟进;
12. **词汇(已施工)**:UI 员工面板为「员工/任务/承诺/车间」;旧「远程助理」配置面仍保留。
    代码/路径/事件名继续用 `chief`/`task`/`commitment`/`worker`。

### 7.3 前端挂点(已施工 / 偏差)

- 员工对话 = `renderConversation` 的普通路径,右栏 ContextPanel 显示任务小节
  (会话属于员工注册表时);已施工,但右栏仍混有通用任务/记忆卡;
- 员工行读取状态投影,当前上屏 waiting/failed/interrupted/running/thinking/idle、主 task、
  多任务计数;**未上屏** lastEventAt、stale、waiting reason、通知抑制原因;
- 右栏已有 open commitments、pinned/state、journal/proposal 和任务摘要;点击 task/worker 回现有
  会话;预算和来源仅部分显示;
- 员工搜索按 name/title/主任务过滤;状态 chip 不计入员工行,与“并入状态过滤”的目标不同;
- 命令面板:员工列表可搜索、可跳转;已施工;
- ui-language:员工组沿用现有侧栏卡片语言(1px 边、3px 角、无阴影),不新造视觉体系。
- 不做 2.5D 办公室、Live2D、家具或角色动画;状态表现只借信息层级,不借游戏化视觉。

## 8. 红线(改前先读)

1. **员工是普通会话**:顶层可见,绝不写 `parent_session_id`(chief 研究已定;父子树是另一个话题);
2. **人和事分离**:员工对话永远不进项目;worker 会话进项目,因为它是"落在那个目录里的活儿"。
   侧栏先人后事(Varin 的 IA 教训,§2.1);
3. **员工不能雇员工**:员工可随意派 worker;雇新员工(拿到控制工具的人)必须用户确认——
   员工可以"起草招聘启事"(建议人设+配置发进对话),用户点头才真雇。否则"Chief 不能给自己
   扩权"会被绕着违反(一位员工雇十位 = 自己扩十倍编制);
4. **Host 管权限**:能力来自档案与白名单,不来自员工自己的声明;fail-closed;
5. **worker 必须可见**:任何写文件/跑命令/开浏览器的活儿都能在车间找到、接管、停止;
6. **Telegram 是渠道不是产品**:员工体系零 Telegram 也完整;远程消息只是喂进某位员工对话的
   另一个入口,与本地 UI 共享同一段对话;
7. **提示词只承诺宿主能兑现的行为**(§4);出厂人设随版本打磨,但承诺项永远落后于(而非超前于)
   宿主机制;
8. **记忆可检视可改可忘**(§6.5);
9. **不引入第二套会话数据库**:tee 仍是员工/worker 会话的事件真相,storage transcript 照走
   rails 常规持久化(chief-m0 偏差记录 5 的口径不变)。
10. **状态是投影,不是事实源**:员工行/waiting/进度只能从 task、worker、approval 和 commitment
    派生;前端动画、卡片状态和模型自述不能写回业务状态;
11. **客观事实高于模型叙述**:任务完成/失败、文件变更、测试结果和审批结论以 ledger/tee 为准;
    员工报告和记忆只能引用,不能覆盖;
12. **主动出站是独立能力**:后台执行、task_start、worker 继续运行不等于允许通知用户;所有消息、
    桌面通知、Telegram、邮件和媒体都必须经过通知闸门与预算;
13. **不做无依据工作**:没有用户任务、due commitment、已批准 backlog 或明确重试条件时,员工应待机;
    「展示主动性」不是任务来源;
14. **急停必须持久**:`pausedUntilUserResumes` 是 Host 状态,重启后仍生效;员工不能自行解除;
15. **高风险能力 Host 硬校验**:项目白名单、预算、审批、记忆固定事实和员工注册表不能由提示词或
    员工工具直接修改;自然语言推断只能作为结构化账本的辅助入口。

## 9. 里程碑

### M1.5(前置,不含本设计):合并 chief-m0 → master

它是绿的(573 测试、三 agent 真机 E2E、tee 重启逐字节复现),只落后 master 数个 browser
提交。先合,Telegram 框定的 M1 立即可用;员工化在其上施工,避免长分支漂移。

### E1 员工化第一批(主体已施工,待端到端验收)

delta 1–6:注册表、状态投影、持久急停、侧栏员工组、雇佣向导、Telegram 解耦、
bootstrap、任务/承诺入口和面包屑。

验收:

- 零 Telegram 配置下雇一位员工、派一次活、收到主动汇报;
- awaiting permission/question/plan 时员工行显示 waiting_user 并能跳回真实 worker;
- `stop all` 后新任务不能启动、非安全通知停止、重启后仍暂停;
- worker 可在车间找到、接管、停止;删除/归档工作卡不影响员工;
- 原有单 chief 配置自动迁移成一位员工(账本无缝)。

当前对账:骨架和本地入口已具备;上述条目仍需按当前工作树跑真实 UI/权限卡/重启迁移验收,
尤其“零 Telegram 下完成一次主动汇报”和“员工行 waiting_user 能跳回 worker”不能只靠单元测试。

### E2 行为契约(部分施工)

delta 7、9 + 出厂总管模板打磨:StaffPolicy、通知闸门、commitment ledger。

验收:

- 新员工开箱即有 §4 表左列行为;普通 running 不产生周期性消息;
- 完成/失败/waiting/承诺到期有汇报,同一事件去重并有冷却;
- 用户设置勿扰后普通通知延迟,审批/安全例外仍可出站;
- "周五提醒我" / "CI 绿了继续" 能落账、可查、可取消,到点/满足条件会唤醒;
- 模型生成失败时不发固定 fallback;预算耗尽和超时如实停在账本里。

当前对账:StaffPolicy、持久暂停、承诺账本、部分通知闸门已落地;通知账本持久化、渠道矩阵、
真实成本/模型调用预占和“同一事件去重”仍待补,因此 E2 不能整体打勾。

### E3 分层记忆(存储/编辑部分施工)

delta 8 + 档案页记忆视图:§6.1–§6.5。

验收:

- 偏好二次生效("上次你说…"),回答能给出记忆来源;
- pinned/state/journal/history 分层;旧日记超预算只给概要,当前用户消息永远优先;
- 模型只能 propose,不能直接改 pinned;用户能编辑/删除/清空;
- 删除记忆后整理、同步和重连不会自动补回;
- 日记只写真实事件,工具残骸/重复通知/临时进度不会入库。

当前对账:文件分层、tombstone、工具 recall/propose 和 pinned/state 编辑可用;自动 journal、
proposal 晋升、来源 UI、单条删除/导入导出和 detail/gist/dormant 注入尚未完成。

### E4 长期常驻与运维(待施工)

delta 10–11 + telegram per-staff 绑定 + M2 遗留(成本预算、审计页、图片附件进 worker,
沿 chief-m0 的 M2 待办清单)。

验收:

- 空闲员工只处理 due commitment、已批准 backlog、满足条件的重试;没有活时保持 idle;
- 重复同一进度、连续相同工具调用、连续无进展会有速率/循环保护;
- 员工页能看到调用、成本、最后活动、最近通知被抑制原因;
- ledger/memory/scheduler/Telegram/worker 健康可诊断,恢复时不会重放已完成 task;
- Telegram 消息进指定员工对话且与本地同段,不影响本地/远程同源事实。

当前对账:未达到本节验收;不要把 per-staff Telegram 绑定或 16 个工具存在本身当成 E4 完成。

## 10. 明确不做(非目标)

沿 01-product.md §8 全部保留,另加:

本节是“不做”,不是待办清单;§0.1 的未施工项仍可能在后续实现,二者不要混写。

- 不做 Varin 式扩展 SDK / IDE 视图 / 多设备 / Run-Operation-checkpoint 级耐久(§2.1);
- 不做多用户/多租户;
- 不做员工人格市场、角色扮演、群聊;
- 不做向量记忆/RAG(纯文本起步,够用再议);
- 不自动创建员工(老用户零打扰);
- 不把员工对话做成独立窗口模式/应用壳——它就是会话,前台感来自置顶与常驻,不来自新界面;
- 不复制 BigFatFishWife 的恋爱/伴侣人格、精力、睡眠债、孤独、心情、口吃检测和拟人生活循环;
- 不做 2.5D 办公室、Live2D、家具、天气、完整 AI 世界或员工社交广场;
- 不用正则承担关键任务状态、权限、承诺创建或审批;自然语言推断只是辅助;
- 不让员工为了显得主动而创造工作;没有有据可查的候选就待机;
- 不把 dsh-room 的 AGPL 代码或受限制角色素材直接搬进 Marionette;
- 不做通用多人世界协议、跨设备员工记忆同步和员工市场;
- 不把每个 worker 变成永久员工;worker 是任务,不是新人物;
- 不把状态动画或前端卡片当作事实源。
