# Council Mode — 架构(M1–M3 定稿)

> 状态:M1(审议)+ M2(执行)+ M3(验证回路)已实现(2026-10-31)。
> 对比审计与决策依据见 [`architecture-audit.md`](./architecture-audit.md);
> 本文描述**实际落地**的结构。与代码冲突时以代码为准。

## 1. 总览

Council 是 Marionette 内建的多模型审议编排器:多个模型各自独立分析
同一任务、互相质询、修订、由主席综合,产出经评审的结构化实施计划
(Plan Gate);用户批准后由真实 coding agent(ACP)按计划落码——并行
任务用 git worktree 隔离、冲突显式上报;执行完成后跑计划声明的验证
命令,失败触发主席修订计划的受限修复回路(≤3 次)。

```
┌────────────────────────── 前端 (React) ──────────────────────────┐
│ CouncilPage(setup / timeline / plan / history)                   │
│   └─ src/lib/council.ts(invoke 封装 + listen("council-event"))   │
└────────────────────────────────┬──────────────────────────────────┘
                    Tauri commands(council_*×9)+ 全局事件
┌────────────────────────────────┴──────────────────────────────────┐
│ src-tauri/src/council/                                           │
│  orchestrator.rs  run 线程:协议循环 / fan-out / 预算 / 取消 /     │
│    │              执行-验证-修复回路                                │
│    ├─ prompts.rs   各阶段 prompt 构造(信息隔离强制点)             │
│    ├─ worker.rs    ModelClient:EngineClient(引擎 HTTP)/ Mock     │
│    ├─ state.rs     状态机转移表(硬编码 + 审计)                   │
│    ├─ store.rs     ~/.marionette/council/runs/{id}/ 持久化        │
│    ├─ tasks.rs     PlanDoc→任务图(frontier 调度/失败不级联)      │
│    ├─ executor.rs  执行 harness(ACP 生产 + 脚本测试)+ worktree 合并│
│    ├─ verify.rs    验证命令执行(超时/输出截断/危险拦截)           │
│    └─ types.rs     领域类型(spec/消息/状态/计划)                  │
└───────────────────────────────────────────────────────────────────┘
          复用(不重造):engine.rs(Magpie 网关)、storage::write_atomic、
          http_client(ureq schannel)、acp.rs(M2 执行阶段)
```

## 2. 审议协议(Phase A–E)

| 阶段 | 状态机值 | 参与者 | 输入可见性 | 产出消息 |
|---|---|---|---|---|
| 任务分析 | `analyzing` | 主席 | 任务 + 角色(无成员输出) | `briefing` |
| A 独立提案 | `proposing` | 全体 | 任务 + 角色 + 简报(**互相不可见提案**) | `proposal` |
| B 交叉质询 | `reviewing` | 全体 | 他人最新方案(署名) | `critique` |
| C 修订回应 | `revising` | 被质询的作者 | 自己最新版 + 针对自己的质询 | `revision` |
| D 主席综合 | `synthesizing` | 主席 | 全部消息流 | `synthesis` |
| E 计划门 | `synthesizing`→`plan_ready` | 主席 | synthesis | `plan`(JSON)+ plan.json |

- 轮次:round N 的 B/C 之后仍有未回应质询且 `round < rounds` → 回
  `reviewing`(看 round N-1 的修订);否则进 D。
- 不做多数投票:分歧靠证据裁决,裁决不了的进 `openQuestions`(每条
  附消解实验),不接受和稀泥。
- 计划 JSON 解析失败 → 一次修复调用;两次失败 → run `failed`。

## 3. 状态机(`state.rs`)

```
created → analyzing → proposing → reviewing ⇄ revising → synthesizing → plan_ready
plan_ready --用户批准--> executing → verifying → completed(执行链,M2 启用)
plan_ready --接受(council-only)--> completed
任意运行态 → paused / cancelled / failed / budget_exhausted(终态)
paused → paused_from 指向的原状态(或 cancelled)
```

- `can_transition` 硬编码表;非法转移 `Err`(编译期 enum 穷尽 + 运行
  期表校验)。
- **执行门(验收 #9)**:`plan_ready → executing` 是唯一执行入口,只
  由 `council_approve_plan` 命令走;orchestrator 线程到 `plan_ready`
  即退出,不存在绕过路径。
- 每次转移 append `transitions.jsonl`(时间戳/起止/原因),全链可审计。

## 4. 消息协议(`types.rs::CouncilMessage`)

```json
{
  "messageId": "msg-<seq>-<millis>",
  "runId": "c<millis-hex>-<seq>",
  "round": 2,
  "from": "glm-architect",
  "to": "all",
  "kind": "proposal | critique | rebuttal | revision | question | answer
        | synthesis | plan | briefing | task | status | evidence",
  "replyTo": null,
  "content": "…",
  "timestamp": 1790000000000,
  "meta": { "targetAuthors": ["gpt-reviewer"], "failed": false }
}
```

- `messageId` 是幂等键:`store::append_message` 拒绝重复 id(验收 #5),
  `persist_message` 内存再挡一层。
- 消息 append-only,修订是**新消息**,旧版本永不覆盖。
- 送达确认在 Marionette 单进程模型下是内存事实(worker 结果经 mpsc
  回 orchestrator 唯一写者),不需要 OmO 的 maildir 三态协议。

## 5. 并发与控制(`orchestrator.rs`)

- 每 run 一个 `std::thread`(命名 `council-{run_id}`),注册进进程内
  `registry()`(HashMap<run_id, Arc<RunHandle>>);`RunHandle` 只有两
  个 `AtomicBool`:cancel / pause。
- fan-out 两步:先在主循环(借用只读 messages)构建 `Job`(model +
  双 prompt + 意图),再 `std::thread::scope` 并发执行;计数信号量
  (`Mutex<u32> + Condvar`)限并发,默认 4,spec 固化。
- worker 失败自动重试 1 次;仍失败 → `status` 消息 + 成员标记
  `failed`,run 继续(验收 #6);某阶段**全员**失败才是 run 失败
  (质询阶段全败降级进综合,链路可见)。
- 取消(验收 #8):调度边界 + worker 请求前检查 cancel 标志;取消后
  不再发起任何模型调用。
- 预算三层(请求数 / token 估算 chars÷4 / 墙钟),每次循环边界检
  查;超限 → `budget_exhausted` 终态,不重试。

## 6. 持久化与恢复(`store.rs`)

```
~/.marionette/council/runs/{run_id}/
  spec.json          不可变配置(创建时固化,含预算与并发)
  state.json         可变快照(phase/round/成员状态/预算用量)
  messages.jsonl     append-only 消息账本
  transitions.jsonl  append-only 转移审计
  plan.json          PlanDoc(plan_ready 后)
```

- 写入用 `storage::write_atomic`(tmp+rename+.bak);JSONL 追加用
  append 句柄,单写者(orchestrator 线程)。
- **恢复协议**:进程启动时 `recover_orphaned_runs` 把「磁盘运行态 +
  无活线程」的 run 置 `paused`;`council_resume_run` 重新拉起线程。
  阶段进度以消息账本为准——briefing/各成员 proposal/质询/synthesis/
  plan 的存在性判定阶段是否已完成,**已完成的阶段不再发起模型调用**
  (验收 #7:重启不重放已完成副作用)。
- 测试经 `set_root_override_for_tests` 沙箱隔离,不碰真实用户目录。

## 7. 模型 Worker(`worker.rs`)

- `trait ModelClient`:生产 `EngineClient` / 测试 `MockClient`(脚本
  队列 + 调用记录,验收 #1/#14 的 mock/live 分界)。
- `EngineClient` 直连引擎:`POST http://127.0.0.1:{port}/v1/chat/completions`
  (`Bearer magpie`,120s 超时,非流式)。端口每次现查
  `engine::running_base()`(引擎端口会漂移);引擎唤醒由
  `start_run` 统一做一次(`ensure_bridge_ready_if_installed`,最多
  20s)。
- 不经 ACP:避免事件进 transcript 管线、绕开一会话一 turn 限制
  (审计决策 #1)。

## 8. 前端(§5 任务书的 M1 覆盖)

- 入口:侧栏「圆桌」(ProjectShelf)+ Ctrl+K「打开圆桌会议」
  (代码模块仍叫 council)。
- 视图:`.council-tab-page` 整页(收纳盒伪标签同模式),三态:
  **列表**(run 历史)、**设置**(任务/模式/成员/轮次/并发/预算)、
  **运行**(成员状态条 + 分组时间线 + 计划面板 + 暂停/取消/批准)。
  版式跟产品其余界面同一套:居中栏(列表 720 / 召集 640 / 运行 800)、
  1px 线、`var(--radius)` 3px 角;中栏宽度用容器查询适配
  (双侧栏窄列 ≈ 466 起)。不另搞圆角卡片 / 插画 / 毛玻璃。
- 事件驱动:后端 `council-event`(message/phase/member/plan-ready/
  finished/warn/error)→ 当前 run 全量重拉(M1 量级 <百条消息)。
- 复用:`MarkdownBody`(消息渲染)、`showToast`、模型目录经
  `council_list_models`(引擎目录直通)。

## 9. 命令面(`commands.rs`)

| 命令 | async | 说明 |
|---|---|---|
| `council_create` | ✓ | 重写 runId/createdAt → 校验 → 落盘 → 启动线程 |
| `council_list_runs` | | run 摘要列表 |
| `council_get_run` | | 全量详情(spec/state/messages/transitions/plan/live) |
| `council_cancel_run` | | 活线程置标志等落账;非活线程直接转移 |
| `council_pause_run` | | 置 pause 标志(当前批次完成后停在阶段边界) |
| `council_resume_run` | ✓ | paused → paused_from,重启线程 |
| `council_approve_plan` | | plan_ready 才可裁决;接受→completed(拒绝→留 plan_ready) |
| `council_delete_run` | | 运行中拒绝;删目录 |
| `council_list_models` | ✓ | 引擎目录 + ready 状态(顺带唤醒 5s) |

## 10. 执行与验证(M2/M3 已实现)

### 10.1 任务图(`tasks.rs`)

PlanDoc → `TaskProgress` 账本:编译期环检测(只看图内依赖;图外依赖
由 finalize 收 skipped,不误报环);frontier 调度(依赖全部
Merged/Completed 即就绪,非严格波次);**失败不级联**——上游失败的
下游在全部可执行工作结束后统一收 `skipped`,不假成功。

### 10.2 执行(`executor.rs`)

- `TaskHarness` trait:生产 `AcpHarness`(AcpService spawn → prompt →
  `wait_turn_idle` → stop;成功判据 = turn 完成 + 工作区变更,而非
  agent 的自述);测试 `ScriptedHarness`(prompt 约定的 WRITE/FAIL 指令)。
- **隔离(验收 #10)**:ready 批次 >1 且是 git 仓库 → 每任务
  `git worktree add -b council/<run>/<step>`;完成后按拓扑序**顺序合并**
  (--no-ff --no-commit,冲突先 `--diff-filter=U` 收集文件再 abort),
  冲突任务显式 `Failed(冲突文件清单)`,绝不静默覆盖。非 git 目录退
  化串行 + 显著警告。
- 恢复:进入 Executing 时把账本里遗留 Running 的任务复位 Pending(其
  worktree 清理后重跑)。

### 10.3 验证与修复回路(`verify.rs` + orchestrator Verifying 分支)

- 验证命令(`plan.verification`)逐条 `cmd /C` 执行:900s 超时、8KB
  尾部保留输出、CREATE_NO_WINDOW;**危险模式**(git push/publish/
  rm -rf/shutdown…)拦截为 needs-human,清单在 Plan Gate 冻结,运行
  期提示注入无法扩它。
- 失败 → `Evidence` 消息(逐命令 ✓/✗/⚠)→ 主席 `repair_round_prompt`
  (只改受影响步骤、无关步骤保持 id 使已完成工作保留)→ 计划版本化
  (Plan 消息带 repairRound)→ 账本迁移(同 id 同 detail 的 Merged 保
  留,变更/新增复位)→ Executing 重跑 → 再验证;**上限 3 次**,触顶
  `Failed`(带证据),不无限自改(验收 #11)。
