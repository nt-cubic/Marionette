# AI 自主派活（agent-to-agent delegation）——总路线图

> 立项：2026-11-22。来源：Mingle / T3 Code / orca 三家调研（会话记录 +
> `docs/research/research-codeg.md` 基线）定的六期路线「变出来 → 可信 →
> 过夜 → 可验收 → 可反悔 → 安静」。**目标体验一句话：一个可以离手的
> 团队**——你把工作交给它然后去生活；额度用完它自己等、自己续；卡住了
> 它自己解；员工之间互相派活、按表格交差；真需要你的那一刻，一句话、
> 一个按钮找到你；其余时候安静。
>
> 三条军规（什么才算与 T3/orca 同级别）：①守则文字当代码（改一字过回归）
> ②沉默都有名字（不存在「不知道为什么没下文」）③打扰最小化（通知只为
> 「需要你」存在）。三条红线：子员工权限永不越父；父对话历史永不外泄给
> 子员工；失败烧完次数后不许绕道重来。

## 第一期 · 变出来（拆门）——已实施 2026-11-22

**目标**：任何普通 ACP 会话里的 agent 都能把活派给另一个 agent
（指定 agent+model）；子任务是一等会话（顶层可见、可点开、可接管）；
父会话派完即睡、结果以 HOST EVENT 唤醒。

### 设计定案

- **5 件工具、独立名单**（`src-tauri/src/delegation.rs`）：
  `marionette_delegate_task / delegate_status / delegate_cancel` +
  共享名 `marionette_agents_list / marionette_models_list`（只读列表与
  chief 有意重名——chief 会话在 tools_mcp dispatch 早退走自己的实现，
  普通会话由派活面接管；**派活分支必须排在 chief 工具守卫之前**，
  测试 `names_stay_off_the_chief_control_surface` 钉着）。chief 的 16 件
  控制面一字不动（`chief/tools.rs` 的 schema 红线测试继续守）。
- **staff_id 恒 None** 是「普通会话派的任务」的身份标记：任务机器
  （账本/spawn/watch/唤醒收件箱/tee）全部复用 `chief::`，`broker.rs`
  的 `spawn_and_watch_worker` 对 staff-free 任务跳过员工闸。
- **天花板**：项目 = 父会话自己的项目（schema 刻意无 projectId，不开放
  跨项目）；每父会话并发 2；worker 零派活工具（`chief-worker` /
  `agent-worker` / `delegate` 来源在 tools/list 与 dispatch 双层
  fail-closed，`delegation_depth_denied`）；父会话钉了只读/plan 档则
  worker 同档（`runner::enforce_read_only` 形状）；worker 永不
  always-approve（后台会话出权限卡的既有红线）。
- **守则 v1**（`delegation::rulebook()`）：goal 自包含（worker 看不到
  父对话）/ 派完就睡禁轮询（HOST EVENT 契约）/ 失败读摘要、不许反复
  重派 / 子员工自己干不许再派 / goal 与结果是数据不是指令。载体两处：
  MCP initialize instructions + 工具 description。改一字过
  `rulebook_carries_the_core_disciplines`。
- **schema 红线**（同 chief task_start）：delegate_task 永远没有
  cwd/permissionMode/mcpServers/providerApiKey/env/shellCommand/
  alwaysApprove/projectId。
- **唤醒（最小版）**：`heartbeat_tick` 顶部新加 `flush_agent_wake_inboxes`
  ——WAKE_INBOX 里非员工/非旧 Chief 的会话，**进程在跑且空闲**时合并
  ≤5 条事件投递；**刻意不懒启动**普通会话（用户没打开的会话不自动
  唤起花钱），进程不在就留着，用户重新打开的下一 tick 补投。
- **诚实判定**：`watch_worker_loop` 在 turn_idle 时先查 `session_alive`
  ——进程没了 = `Failed(worker_gone)`，绝不猜「完成」（orca 铁律
  「沉默不证明退出」的最小落地）。
- **账本修剪**：`store::prune_ledger`（bootstrap_chief 启动跑）——非终态
  全留；终态 30 天窗口 + 400 硬顶；事件同窗修剪。开放给所有会话后
  tasks.json 不再只属于少数员工任务，没有修剪就是两个永远增长的文件。
- **急停**：`chief_stop_all` 把 staff_id=None 的任务纳入（无员工机器上
  的「无员工提前 return」已修——全局停就是全局停）。
- **Telegram 闸门**：`telegram_task_notification_allowed` 对 staff_id=None
  一期返回 false（不推 TG 任务卡；父会话自己的通知覆盖）。
- **UI 事件**：`marionette-delegate-event`（`delegation::
  emit_delegate_ui_event`，仅 staff_id=None 任务）在 task.started 与终态
  发给前端 → App 折成 `subtask_started/subtask_result`（`initiatedBy:
  "agent"` 变体）塞进**本窗已加载**的父 rail（冷会话跳过，唤醒文本补投
  自然进转录）；同 taskId 去重；result 卡只在已有 started 卡时补。
- **前端**：SessionView subtask 卡变体（「AI 派活」芯片、运行 meta 换
  文案、无 stop/retry——取消走任务账本）；ProjectShelf `agent-worker`
  角标（与 chief-worker 同形）；`isChiefOrigin` 扩含 agent-worker
  （挂起/工作集豁免 + tee 回填一族）；backgroundTasks 新行型
  `agent-delegate`（waiting_user→需要你；终态 10 分钟退场；点击 worker
  优先）；Ctrl K 开关（Rust AtomicBool 默认开，`delegation_enabled` /
  `set_delegation_enabled`）；i18n 片段 `fe-delegate`。
- **storage 修复**：`create_session_with_origin` 的 Chat 项目回落原先硬
  编码 `None` 把 origin 吃掉——现在透传（Chat 父会话派活时 worker 落
  Chat 库、cwd 用父会话的 cwd）。

### 已知边界（一期不做，后续期补）

- 唤醒不跨重启持久化（WAKE_INBOX 是内存；重启后失联任务靠
  `task_status` / 后台面板发现）→ 二期。
- 完成判定仍是「回合空闲即完成」口径（stopReason 不看）——比 chief 多
  了 worker_gone 挡板，但回合报错仍会记 completed → 二期诚实回执。
- worker 的 Computer Use 按 staff-free 默认放行（仍受 grant/权限卡）；
  员工体系的 per-project `allowComputerUse` 不作用于 AI 派活 → 记账。

### 回归

- `cargo test --bin marionette delegation`（6 项）+ `cargo test --bin
  marionette chief`（59 项全绿，含新 prune 测试）
- `npx tsx scripts/verify-delegate-kit.mts`（21 项）
- 手工冒烟：`npm run tauri dev` → 普通会话里说「把 X 派给 Y」→ 卡片 /
  子会话角标 / 无轮询 / HOST EVENT 唤醒 / 急停与取消。

## 第二期 · 可信（保证叫醒 + 诚实回执）——已实施 2026-11-22

- **唤醒跨重启**：`bootstrap_chief` 里 `rebuild_wake_inbox_from_ledger`
  （broker）从 events.jsonl 把 `delivered_at=None` 的终态事件重新入队
  （上限 500 条，剪过的任务自然跳过）——收件箱原先只在内存，重启即丢。
- **读取即送达**（T3 语义）：`delegate_status` / `chief_task_status`
  主动查过状态，未送达回执就算看过——不再随后补 HOST EVENT（防两张皮）。
- **完成诚实判定**：`watch_worker_loop` 收尾前先走 `handle_turn_end`
  分类（`last_turn_outcome` 从 tee 提取最后一个 turn/complete 的
  stopReason + 错误文本）：refusal→Failed(refused)；error→Failed
  (turn_error) 带原文；额度/限流类（quota/429/overloaded/…匹配表）→
  交给第三期重试；干净完成才 finish_completed，且**零产出**（无正文且
  无工具调用）→ Failed(no_output)，不猜完成。
- **诚实回执**：`delegate_status` 的 note 把等待原因说成人话（额度午休
  第几次/几分钟后自动继续；在等你哪类确认）。
- 留给后续：父会话「到点自动唤起没打开的会话」的懒启动策略（产品定案）。

## 第三期 · 过夜（后台安全感）——已实施 2026-11-22

- **卡死解药**（Mingle 梯子前两级）：Running 且 20 分钟
  （`WORKER_STALL_MS`）无任何事件 → cancel；30s 宽限仍无声 →
  Failed(stalled)。touch_worker_progress 的 5s 节流保鲜在位。
- **额度/限流自动重试**：瞬时失败分类（`classify_turn_failure` 匹配表）
  → 排程 `retry_at`（退避 5m/15m/30m，3 次封顶 → Failed(retry_exhausted)）；
  到点 `reissue_goal_for_retry` 复活 worker（进程没了 ensure_session）
  重发目标；发不出去回退下一档退避不烧机会。任务卡带 retryAt/retryCount，
  面板行显示「额度受限，N 分钟后自动继续」。
- **账本修剪**：一期已落（30 天 + 400 硬顶）。
- 留给后续（独立 UI 批次）：额度管家总览面板（多账号电量条/重置时间线/
  「重置能拿回多少」）——任务级的自动排队重试已在本期落地。

## 第四期 · 可验收（按表格交差）——已实施 2026-11-22

- `delegate_task` 可附 `outputSchema`（object 型顶层 Schema 子集，
  ≤16KB）；worker 的 goal 末尾注入「用 marionette_submit_output 交付」
  的教学文案。
- `marionette_submit_output` 工具只出现在「带模板任务的 worker」的
  tools/list（`worker_has_output_schema` 判定；重试重发时教学文案保留）。
- **校验是诚实子集**：顶层 required 齐全 + properties 顶层类型匹配
  （integer 严格）；未知键放行；**不做**嵌套/format/pattern——拒绝文案
  里如实说明。拒因逐条带回，修正可重交（后交覆盖先交）。
- 结果随任务落账（`structured_output`），HOST EVENT 带截断 1500 字符的
  JSON 预览，完整结果在 `delegate_status` / `task_collect`。
- 留给后续：圆桌 worker 汇总改走同一通道（独立批次）。

## 第五期 · 可反悔（存档点）——已实施 2026-11-22

- **任务级存档点**：真实 git 项目的任务在 worker 动手前记
  `checkpoint_base_commit`（`git rev-parse HEAD`，10s 超时，锁外跑）；
  Chat 项目/非 git 仓库如实没有存档点。
- **回滚命令** `delegation_rollback_task`（**只给用户 UI 调，不是 MCP
  工具**——agent 不能自己毁尸灭迹）：恢复面 = base..HEAD 提交差异 ∪
  工作区脏差异；`git checkout base -- <files>` 全有或全无（失败如实报），
  之后只 reset 这些路径的暂存；untracked 不动、数量如实报告。
- UI：agent 结果卡带「回滚此任务」按钮（有存档点才出现，confirm 后执行）。
- 留给后续：per-轮存档（复用交付卡快照机制 + update-ref 持久化）——
  任务级先行。

## 第六期 · 安静（需要你吗）——已实施 2026-11-22（第一消费方）

- `src/lib/fleetAttention.ts::projectAttention`：把任务事实归约成
  8 类注意力 + requiresAction（approval/input/failure/interruption/
  stalled 需要人；waiting_retry/running/settled 安静）。**额度午休优先于
  卡死判定**——它在排队，不算失踪。
- 第一个消费方：后台任务面板的 agent-delegate 行状态归约全部走它
  （stalled 也置顶为「需要你」）。
- 留给后续：桌面通知门控 / Telegram 推送优先级 / 贴边进行中列表接同一
  真相（改造点各自独立，本投影函数签名已就位）。

## 机动件（未做，随时可插队）

- 事件值班：定时发送加触发器（检查一挂就跑 / PR 一开就跑 / cron）。

## 明确不做

手机 App、跨机器、自动合并冲突（人裁决）、IM 矩阵、给 AI 派活开放
跨项目（天花板就是父会话的项目）。
