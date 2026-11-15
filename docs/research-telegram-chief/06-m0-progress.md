# M0 施工记录（进行中）

> 逐 PR 记录落地状态与偏差。设计契约见 [02-architecture.md](./02-architecture.md) 与
> [05-mvp-plan.md](./05-mvp-plan.md)；本文不是契约，是施工台账。

## PR-A：durable event tee + 后端 ACP 驱动探针（2026-11-15）

### 已落地

| 件 | 位置 | 说明 |
|---|---|---|
| `chief` 模块 | `src-tauri/src/chief/` | `mod.rs`（模块文档）+ `tee.rs` + `probe.rs` |
| durable event tee | `chief/tee.rs` | 挂在 `acp.rs::emit_now`（与 telegram 远程桥同一收口，`observe` 只做 map miss 检查 + sender clone，锁内零 IO） |
| 事件落盘 | `~/.marionette/chief/sessions/<sessionId>.events.jsonl` | 每行 `{seq, ts, sessionId, kind, method, data}`；重新注册时 seq 从现有行数续数（断点续记） |
| 回放 | `chief::tee::replay` | 读回全部行，坏行跳过并计数 |
| 后端驱动探针 | `chief/probe.rs` | `AcpService::start → send_prompt → wait_turn_idle → stop` 全链在 Rust，不经前端 `performSend` |
| 前端入口 | `main.tsx` 的 `window.__chiefProbe` + `lib/api.ts` 三个 wrapper | 诊断面，非产品 UI |

### tee 的纪律（写代码时定下的，改之前先读）

1. **锁内零 IO、零等待**：`register` 的 mkdir/数行/开文件、`unregister` 的
   drain 等待（最长 5s）都在 registry 锁外——`observe` 是**每个** UI 事件
   都要过的一道闸（所有会话共享一把 registry 锁），锁内卡 1ms 全体事件
   线程陪卡。
2. **有界队列 + 丢事件计数**：容量 2048，满则丢并计数、unregister 时记
   `dev.log`。绝不反压 ACP 读线程（acp.rs 背压死锁事故的教训）。
3. **fail-closed 会话 id**：集合外字符（含 `..`、空格）拒绝注册，不做
   静默替换——静默替换会让两个 id 折叠到同一文件串台。
4. **writer 线程每会话一个**，逐行 flush；写失败记一次 error 后继续收干
   队列（不再逐条刷屏）。

### 探针用法（真机 M0 验证）

1. `npm run tauri dev` 启动，devtools 控制台：
   ````js
   await window.__chiefProbe.drive({
     agentId: "codex",            // 或 opencode / claude-code / deepseek…
     cwd: "D:\\Some\\Real\\Project",
     prompt: "列出你工作目录下的文件名，然后说 done。",
     timeoutSecs: 600,
   });
   ````
2. 回合进行中把主窗**最小化/隐藏**——回合照跑（这正是 M0 探针 1 的验证点）。
3. 返回报告含 `caps / promptAccepted / turnIdle / elapsedMs / tee{kinds,events}`。
4. 重启 Marionette 后 `await window.__chiefProbe.replay("<sessionId>")`——
   能读回完整事件流即 M0「事件可在重启后恢复」的证据。
5. 卡死逃生口：`await window.__chiefProbe.stop(sessionId)`（与产品停止键同路 `AcpService::stop`）。

### 探针会话的边界

- 不进 storage（无 Session 行，UI 永远看不到）；跑完即 `stop()` 收尸。
- `send_prompt` 是**发射后即返回**（`accepted`），不是等回合结束——等待
  靠现成的 `wait_turn_idle`（200ms 轮询 in-flight 计数）。
- **前端 start 后会做的「历史重注入」（`acpNeedsHistoryRef`）探针没有复刻**
  ——探针是全新会话没有历史；backend runner（PR-B 之后）必须补这一课，
  这是「Chief 挂起重连」的关键差异点。

### 回归

- `cargo test --bin marionette chief`：7 项全绿（tee 往返/续 seq/坏 id/幂等/
  队列满不阻塞 + 探针 id 校验/harness 元数据形状）。
- tee 测试用 `set_root_override_for_tests` 沙箱（对齐 council store 模式），
  **绝不写真实 `~/.marionette/chief/`**。

## M0 剩余清单（照 05-mvp-plan.md §2.1）

- [ ] 探针 1：主窗隐藏/最小化下 30 分钟持续工作（真机）
- [ ] 探针 2：多 Agent 认证矩阵（OpenCode / Codex / Claude Code 起码各一轮探针）
- [ ] 探针 4：session-aware MCP（`tools/list` 按 session 过滤 + 假控制工具拒绝路径）
- [ ] 探针 5：Telegram update_id 持久化（现版 offset 在轮询循环局部变量里，重启即丢——telegram_remote.rs:2201）
- [ ] 探针 6：最小权限探针（未授权 fail-closed）
