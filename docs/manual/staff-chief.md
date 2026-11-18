# 远程助理与员工模式手册（staff-chief）

一句话：Telegram 远程首席助理（chief）+ 员工（staff）子系统—— chief 经控制
MCP 工具让 Host 创建 worker 会话执行任务；staff 是「档案绑普通 ACP 会话」的
常设身份层。

## 文件地图（改 X 找 Y）

| 想改什么 | 去哪 | 备注 |
|---|---|---|
| chief 驱动（任务推进） | `chief/runner.rs` | **直接调 `AcpService` pub 方法**，绝不借前端 performSend |
| 控制工具（恰好 10 个 `marionette_*`） | `chief/tools.rs` + `tools_mcp.rs` 分流 | 门禁事实源 = `broker::is_chief_session` |
| 心跳/收件箱 | `chief/broker.rs`（`ensure_heartbeat` 2s tick） | 唤醒合并 flush/FIFO/超时取消/失联标 interrupted |
| 任务账本（关系真相） | `chief/store.rs` | tasks.json / events.jsonl / config.json；`STORE_LOCK` 非重入 |
| tee（前端回填） | `chief/tee.rs` + `src/lib/chiefTee.ts` | observe 挂 emit_now；游标回放 |
| 员工注册表 | `chief/staff.rs` | `~/.marionette/chief/staff.json`；策略 `StaffPolicy` 扁平存档 JSON |
| 员工命令/状态投影 | `chief/staff_commands.rs` / `staff_status.rs` | 投影只读 Task 快照，**不把 UI 状态反向写成业务事实** |
| 承诺/记忆/策略 | `chief/{commitments,memory,policy,probe}.rs` | |
| 设计稿/台账 | `docs/research/telegram-chief/`（README + 06-m0-progress.md） | 员工模式设计：`docs/plans/plan-employee-mode.md` |

## 红线（绝不能做）

- **Chief/worker 的驱动全在后端**——主窗隐藏/最小化（TrySuspend）时任务必须照跑；
  前端只在会话打开时做 tee 回填（rail 被抢先写入就弃回放且**不推游标**——宁可重放
  不丢不重）。
- **tee 纪律**：锁内零 IO 零等待（register 文件 IO/unregister drain 全在 registry 锁外）；
  有界队列满丢事件计数**不反压** ACP 读线程（背压死锁红线）；会话 id fail-closed
  不静默替换（两 id 折叠同文件 = 串台）。
- **控制工具门禁只有一处事实**（`broker::is_chief_session`）；dispatch 入口再挡一次。
  **schema 永远不得出现** cwd/permissionMode/mcpServers/providerApiKey/env/
  shellCommand/release/addProjectRoot；**没有 `task_approve` 工具**——写走权限卡
  （waiter 先答先得）。
- **worker 关系只进账本，绝不写 `parent_session_id`**（那是 `@` delegate 的隐藏语义）。
- **启动恢复把非终态任务标 `interrupted`**（绝不标 completed）。
- **STORE_LOCK 非重入**——`append_events` 不能在 `with_tasks` 闭包里调（自锁实锤）。
- `task_start` 必须立即返回（spawn 在后台线程）；requestId/commandId 幂等；
  并发上限全局 clamp 1–3 + 项目级，超限明确拒绝。
- **Telegram chief 模式**：裸文本后端直送；worker 回合推送被 `is_chief_worker`
  聚合掉（任务卡替代）；急停 = 「急停」/`/stop`/右栏全部停止。update_id 游标在
  `telegram-cursor.json`（poller 唯一写者，独立文件防互踩）。
- **mode 快照**（`store::mode_cached`，AtomicU8）热路径零读盘；fail-open 默认 Off。

## 坑

- 测试沙箱：`store::set_root_override_for_tests` + `store::SANDBOX_LOCK`
  （broker/store 共用一把锁）；tee 测试有自己的锁。
- M0 探针 1/2 与 8h soak **还没在真机跑**（施工台账）——动 Telegram 链路前先看台账。

## 验证

```bash
cargo test --bin marionette chief      # 32 项
npx tsx scripts/verify-chief-tee.mts
npm run build
```
真机清单：`docs/research/telegram-chief/06-m0-progress.md`。

## 已知差距与源流

- 蒸馏自：AGENTS.md「远程助理 / Chief（M1 垂直切片）」段 + chief/ 模块头注实读。
- 分支落差：master 的 `e99d9de3`（staff 首启就绪）不在 engine-rust 分支——
  合并时以 master 为准。
