# 关窗卡死（主线程死锁）— 诊断与修复记录

> 2026-09-20 定位并修复。记录死锁结构、日志证据链与以后必须遵守的规则。
> 现行产品说明见 [`CURRENT.md`](./CURRENT.md)。

## 现象

点 X 关窗 → 窗口「未响应」，只能任务管理器杀进程。dev.log 特征：

- `[watchdog] MAIN THREAD STALLED — window is frozen | in-flight: <none — main thread is not stuck in a command>`
- STALLED 之后该会话的 ACP 流量仍在继续（agent 进程没死）
- 600 秒后没有 `main thread STILL frozen` 行 → 进程在十分钟内被外部杀掉

## 死锁结构（根因）

两方互等同一把锁：

| 线程 | 路径 | 行为 |
|------|------|------|
| A：terminal 等待线程 | agent 发 `terminal/wait_for_exit` → `acp.rs` 为每个 terminal 请求 spawn 一个线程 → `TerminalInstance::wait_for_exit()` | 旧实现里 `child.lock()` 的 guard 一直活到 `child.wait()` 返回，即**持锁阻塞等待 shell 退出** |
| B：主线程（关窗） | `CloseRequested` → `shutdown_and_exit` → `stop_all` → `stop`（先杀 agent 拥有的终端）→ `release_all_for_agent` → `kill_command()` | 要拿同一把 `child` 锁去 kill 正被等待的进程 |

能解开等待的 kill 恰好被等待者占的锁挡住 → 互等 → 主线程永久冻结。

**为什么每次都复现**：只要关窗时会话里留有一个未返回的 `terminal/wait_for_exit`（agent 跑过 dev server、watcher、任何还没退出的 shell 命令），必然触发。

## 日志证据链（2026-09-19 那次）

| 时间戳(ms) | 日志 | 推断 |
|------|------|------|
| …830242 | `turn/complete` | 活跃会话正常结束一轮 |
| …838101 | `process/stopped`（旧会话） | 关窗已触发 `stop_all`，主线程当时还活着，且先停掉了别的会话 |
| …839953 | （推断）watchdog ping | 主线程此后再未响应 |
| …842953 | `MAIN THREAD STALLED … in-flight: none` | 关窗走窗口事件、不是 command，所以 in-flight 注册表为空——**`none` 不代表没事发生** |
| …847421 | agent 生成会话标题 | agent 进程还活着 → 主线程卡在 kill 之前的锁上（不是卡在 kill 之后的 `wait()`） |
| …863870 | 最后一条日志 | 用户随后任务管理器杀进程 |

`in-flight: none` + agent 未死 + reader 线程仍在读行，三条同时成立，只有「卡在等锁」自洽。

## 修复（2026-09-20）

1. **`terminal_runtime.rs::wait_for_exit`** — 不再持锁阻塞：持锁瞬时 `try_wait` + 每 100ms（`WAIT_EXIT_POLL`）轮询一次，锁每次只握几毫秒，`kill_command` 任何时刻都能进入。
2. **`terminal_runtime.rs::kill_command`** — kill 后经 `record_exit` 把退出状态写进 snapshot（首个收割者记录，后者不覆盖）。并发的 `wait_for_exit` 轮询方只看到 `child == None`，从 snapshot 拿到真实退出码。
3. **`main.rs::shutdown_and_exit`** — `stop_all()` 挪到工作线程，主线程最多等 5 秒（`SHUTDOWN_KILL_TIMEOUT_SECS`），超时照样退出并写 `[shutdown] agent cleanup timed out` 警告。任何未来清理挂起最多让关窗慢 5 秒，不再永久冻结（孤儿 agent 好过卡死）。
4. **回归测试** `terminal_runtime::tests::kill_proceeds_while_wait_for_exit_is_parked` — 一个线程停在 `wait_for_exit` 等长驻 shell，另一线程同时 `kill_command`；修复前该测试死锁挂死，修复后 kill 立即返回且等待方拿到 kill 的退出码。

## 现行规则（写代码必须守）

- 终端 `child` 锁——以及任何「kill 路径需要拿的锁」——**绝不允许跨阻塞调用持有**。等进程退出 = 轮询 `try_wait`，或在锁外 `wait()`。
- 收割子进程必须经 `record_exit` 记录状态，保持「首个收割者记录、不覆盖」的约定。
- 关窗路径主线程上不许有无界等待：所有清理必须秒级可放弃。
- 排查关窗类卡死时的三个信号：`in-flight: none`（只说明不是 command，窗口事件不进注册表）；agent 流量还在动（= 进程没死 = 卡在锁上，不是卡在 `wait()`）；`still frozen` 行缺失（= 进程被外部杀掉的时间点）。
