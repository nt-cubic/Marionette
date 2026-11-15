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

- [x] **Telegram 线路段 E2E（本机侧，2026-11-15）**：`MARIONETTE_TELEGRAM_API_BASE`
      诊断钩子（只放行 loopback，未设=官方云）+ 本地假 Bot API fixture +
      DPAPI 密文的临时凭据，把「云端中继」以外**全部真代码**跑了一遍：
  - 入站：poller 长轮询取 update → 配对门（chat 42）→ chief 路由 →
    `chief_inbound_text` → Chief 决策派工（真 MCP dispatch）→ worker 完成；
  - 出站（发送队列逐一捕获）：`🛑 Chief 急停：已停止 0 个进行中任务。`、
    `🫡 已交给 Chief。`、`▶️ 任务开始`、`✅ 任务完成 + worker 摘要`、
    Chief 回复走 `sendRichMessage`，总控面板 `pinChatMessage`/
    `editMessageText` 全程实时重画；
  - 游标：`telegram-cursor.json` 重启后首个 getUpdates 请求即带
    `offset=101`（不重放旧消息），处理 update 200 后推进为 201。
  - fixture 未覆盖的只有「物理手机 → Telegram 云」那一跳——用户配对
    真 bot 后（右栏 Telegram 图标 → token → 手机发一条消息确认配对），
    切 Chief 模式发一句自然语言即为全真链路。
- [ ] **8h soak**（M2 验收口径）：主窗隐藏，混合任务并发/取消/重启/断网。
- [ ] **deepseek / 其余 agent 探针**（矩阵扩展，非闸门项）。

### M1 管线端到端（本机实测通过，2026-11-15）

Telegram 线路之外的全部环节，用 `MARIONETTE_CHIEF_E2E_TASK` / `E2E_INBOUND`
（release exe，真实 agent、真实任务账本）驱动：

- **Stage A（确定性派工）**：`chief_task_start` → worker 会话（origin=
  chief-worker）spawn → codex 读文件完成 → `proposed→starting→running→
  completed` 全程落 tasks.json / events.jsonl → 唤醒收件箱 → 心跳 flush
  （`delivered: true`）→ Chief 收 HOST EVENT 并回复（turn/complete 实锤）。
- **Stage B（Chief 决策腿）**：`chief_inbound_text` → Chief（codex）经
  **真实 localhost MCP dispatch** 依次调用 `marionette_projects_list →
  marionette_task_start → marionette_task_status×4 → marionette_task_collect`
  （chief tee 192 行完整记录）→ 最终交付与 worker 读到的内容逐字一致。
- **实测修回的 3 个 bug**：
  1. 心跳 flush 前没有懒启动 Chief——`turn_idle` 对缺席进程返回 true，
     send_prompt 报 not running 并每 2s 无限重试。修：有待办才懒启动
     （空转不养进程），失败 warn 一次退避重试。
  2. Chief 会话本身没挂 tee（worker 有账本、Chief 没有）——修复前创建的
     会话复用时也补挂（幂等）。
  3. tee 回填的 baseline 守卫会让「UI 正常用过的 Chief 会话」卡死在永远
     abort——改为按「未覆盖跨度」过滤（`tee.ts > 转录最新 createdAt`，
     `uncoveredTeeEvents` + verify 用例）。
- 修复后全量回归 573/573 + tsc + verify-chief-tee 全绿；测试配置已回退
  `mode=off`，E2E 产生的 worker 会话留在 Marionette 项目组（带「远程」
  徽标，可删）。

## M2 待办（按设计稿）

- 成本 / token / 墙钟预算（软上限）；审计页（`chief/audits/` 月度文件设计了未落）
- Telegram 图片 / 文件附件进 worker（设计稿建议 M2）
- worker 通知聚合的「内容预览」偏好接入（chief 模式下 task 卡摘要当前不受该偏好门控）
- 空闲 worker 自动挂起（复用前端 trim 巡逻需要识别 chief-worker）
- `messageId → taskId` 精确路由、Telegram topics / 任务 thread
