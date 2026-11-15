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

## 真机验证清单（M0 闸门，待跑）

以下需要真机 + 已登录 agent，代码已备好入口：

- [ ] **探针 1**：`npm run tauri dev` → devtools `await window.__chiefProbe.drive({ agentId: "codex", cwd: "...", prompt: "..." })`，回合进行中把主窗**最小化/隐藏**，确认回合照跑、报告 `turnIdle: true`；重启后 `replay(sessionId)` 能读回完整事件流。
- [ ] **探针 2（多 agent 矩阵）**：对 opencode / claude-code / deepseek 各跑一轮探针，确认 localhost MCP 注入、cancel、历史恢复；不通过的标 experimental。
- [ ] **M1 端到端**：右栏「远程助理」切 Chief 模式 → 选允许项目 → Telegram 发一句自然语言 → Chief 启 worker → 手机上批权限卡 → 收任务卡 → 右栏任务区看状态 → 本地打开 worker 会话验证 tee 回填。
- [ ] **8h soak**（M2 验收口径）：主窗隐藏，混合任务并发/取消/重启/断网。
- [ ] **安全路径**：未授权项目 task_start 被拒（单测已盖）、worker 输出诱导扩权、重复 callback 不二次执行。

## M2 待办（按设计稿）

- 成本 / token / 墙钟预算（软上限）；审计页（`chief/audits/` 月度文件设计了未落）
- Telegram 图片 / 文件附件进 worker（设计稿建议 M2）
- worker 通知聚合的「内容预览」偏好接入（chief 模式下 task 卡摘要当前不受该偏好门控）
- 空闲 worker 自动挂起（复用前端 trim 巡逻需要识别 chief-worker）
- `messageId → taskId` 精确路由、Telegram topics / 任务 thread
