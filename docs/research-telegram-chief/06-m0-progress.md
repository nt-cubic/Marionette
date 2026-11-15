# 施工台账（M0 + M1 垂直切片）

> 逐 PR 记录落地状态与偏差。设计契约见 [02-architecture.md](./02-architecture.md) 与
> [05-mvp-plan.md](./05-mvp-plan.md)；本文不是契约，是施工台账。
>
> 状态：**代码已落地，回归全绿；真机 M0 验证项未跑**（见文末清单）。

## 分支与提交（chief-m0）

| PR | 提交 | 内容 |
|---|---|---|
| M0-A | `a3a12bb1` | durable event tee（`chief/tee.rs`，挂 `emit_now`）+ 后端 ACP 驱动探针（`chief/probe.rs`，`window.__chiefProbe`） |
| M0 基线 | `301d5484` | 调研文档六篇入库 |
| M1-PR2 | `000b0284` | types / store（config·tasks·events 账本）/ policy（白名单裁决） |
| M1-PR3+4 | `b3972cc4` | 10 个控制工具 + session-aware MCP + broker/runner（任务生命周期·幂等·并发·心跳） |
| M1-PR5 | `216104a8` | Telegram chief 模式（固定路由·聚合·急停）+ update_id 游标持久化 + 启动恢复接线 |
| M1-PR6 | `acefc0f1` | 前端：tee 回填、右栏「远程助理」卡、任务区、侧栏徽标、i18n |
| M1-PR7 | （本提交） | waiting_user 状态机补全 + 文档四件套 |

## 与设计稿的偏差记录

1. **`task_started` 事件口径**：设计稿事件表把 `task.started` 定义为「worker ACP
   ready」——实现只在 Starting→Running 时发一次 `task.started`（Starting 中间态
   不发独立事件）。
2. **resume 语义**：`task_resume` 只接受 `interrupted`（failed 按设计稿建议
   明确拒绝并提示重新发起），复用同一 spawn-and-watch 观测循环。
3. **read_only 项目的执行边界**：M1 用「提示词明示边界 + 写走权限卡」实现，
   **没有**按 agent 改各自的权限旋钮（那是 per-agent 配置矩阵，M2）。权限卡
   fail-closed 兜底，符合 R2/R3 的「每次审批」语义，但不等价于强只读。
4. **任务卡路由**：任务卡登记的是 chief 会话路由（引用任务卡 → 后端直送
   Chief），没有实现设计稿的 `messageId → taskId` 精确映射；引用 worker 的
   旧通知仍走 relay 前端管线（M1 允许）。
5. **tee 是 Chief/worker 的事件真相**：这类会话的转录由前端打开时从 tee
   回填（与活路同一转换管线），storage transcript 的写入仍走 rails 的常规
   持久化；游标（`.consumed.json`）保证不重放。崩溃窗口最坏是「回放过但没
   consume」= 重放一次（重复卡），宁可重复不可丢。
6. **waiting_user**：挂在 `emit_now` 的事件收口上（`observe_chief_event`），
   permission/question/plan 的请求与解除直接驱动任务状态机——没有让 Chief
   参与批准决策（设计红线）。

## 回归状态（2026-11-15）

- `cargo test --bin marionette`：**573 passed / 0 failed**（含 chief 32 项：
  tee 往返/续 seq/坏 id/幂等/队列满不阻塞、store 恢复、policy 裁决、broker
  纯逻辑、工具 schema 红线）。council orchestrator 在并行全量下偶发 flaky
  （stash 基线对照确认与本专题无关，单独跑必过）。
- `npm run build`（tsc + vite）：干净。
- `npx tsx scripts/verify-chief-tee.mts`：游标过滤 / maxSeq / 来源判定 /
  user echo 合并与 flush 全绿。
- dev.log 零污染、tee/store 沙箱不写真实 `~/.marionette/chief/`。
- **M0 出口标准（05 §2.2）全部达成**（真机证据见下节）；探针回合里
  `late JSON-RPC response dropped (no waiter)` 是已知形态——send_prompt
  发射后即返回、回合等待走 `wait_turn_idle` 轮询，最终 response 无等待者
  属预期，非缺陷。

## 真机验证清单（M0 闸门）

### 已通过（2026-11-15，本机实测）

- [x] **探针 1：WebView 挂起下后端驱动完整回合**。入口 =
      `MARIONETTE_CHIEF_PROBE="agentId|cwd|prompt"` +
      `MARIONETTE_CHIEF_PROBE_MINIMIZE=1`（`chief::probe::maybe_run_env_probe`，
      release exe 启动 12s 后自动最小化主窗再驱动；devtools 里的
      `window.__chiefProbe.drive` 仍可用）。证据（dev.log `[chief]` 行）：
  - 主窗最小化 → `[window] main window minimized: webview suspended (TrySuspend)`；
  - **codex**（codex-acp 2.6.0 / gpt-5.6-luna）：`turnIdle=true`，7.2s，tee 11 events / 0 坏行；
  - **opencode**（1.19.5 / opencode/big-pickle）：`turnIdle=true`，17.7s，tee 6 events；
  - **claude-code**（claude-agent-acp / default）：`turnIdle=true`，6.0s，tee 21 events。
  - 三份 tee 文件都在 `~/.marionette/chief/sessions/`，seq 连续、含
    initialize → session/new → prompt 流 → turn/complete → process/stopped 全程。
- [x] **探针 2（多 agent 矩阵，三项达标）**：codex / opencode / claude-code
  全部在挂起状态下完成 prompt→complete（05 §2.2 要求「至少一个完整跑完」
  「至少两个确认 MCP 可用」——实测三个）。
- [x] **重启可恢复**：真重启（进程杀死 → 新 PID）后 tee 文件 md5 逐字节
  一致（`dddcc3c5…` 前后相同）；`tee::replay` 消费该文件（单测覆盖）。
- [x] **未授权 fail-closed**：chief 工具对普通会话不可见/不可调、项目白名单
  拒绝、schema 无提权字段（单测盖住）。

### 待跑（需要用户本人手机 / 凭据，代码已备好）

- [ ] **M1 端到端**：右栏「远程助理」切 Chief 模式 → 选允许项目 →
      Telegram 发一句自然语言 → Chief 启 worker → 手机上批权限卡 →
      收任务卡 → 右栏任务区看状态 → 本地打开 worker 会话验证 tee 回填。
      （入站模拟不可行：bot 收不到自己发的消息，必须真人发。）
- [ ] **8h soak**（M2 验收口径）：主窗隐藏，混合任务并发/取消/重启/断网。
- [ ] **deepseek / 其余 agent 探针**（矩阵扩展，非闸门项）。

## M2 待办（按设计稿）

- 成本 / token / 墙钟预算（软上限）；审计页（`chief/audits/` 月度文件设计了未落）
- Telegram 图片 / 文件附件进 worker（设计稿建议 M2）
- worker 通知聚合的「内容预览」偏好接入（chief 模式下 task 卡摘要当前不受该偏好门控）
- 空闲 worker 自动挂起（复用前端 trim 巡逻需要识别 chief-worker）
- `messageId → taskId` 精确路由、Telegram topics / 任务 thread
