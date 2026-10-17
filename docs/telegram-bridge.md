# Telegram 远程桥（设计稿）

> 状态：**设计定稿，未实现**（2026-10-17 调研 + 设计；实现排期见「分期」）。
> 落地后现行行为并入 [`CURRENT.md`](./CURRENT.md)，本文件挪进 `archive/`。

## 一句话

给 Marionette 内置一个 Telegram 通知/回复通道：会话跑完、等你批准/回答时推送到手机；
在 Telegram 里回一句话就注入会话继续跑。纯出站 HTTPS，零新依赖，零新进程。

## 场景

- 人在家但离开电脑（沙发/厨房/厕所）：手机收到「跑完了 / 在等你」，回「继续」。
- 人出门（超市/散步）：电脑开着 Marionette 继续跑任务，手机走流量照样收发。
- 电脑关机 = 通道消失（Marionette 是壳，不做常驻服务；NAS 上那套 OpenClaw 覆盖常驻场景，两者用各自的 bot token，互不干扰）。

## 选型结论

**Telegram Bot API 直连（getUpdates 长轮询 + sendMessage）**，在 Rust 壳内置：

- 纯出站 HTTPS：无端口、无 webhook、无公网暴露；PC 和手机两头都只要能连 `api.telegram.org`，与彼此所在网络无关。
- 手机零安装、零账号体系：用户已有 Telegram（日本网络直连可达）。
- 体积增量 ≈ 0：HTTP 用现有 `ureq`（阻塞、native-tls），序列化用 `serde_json`；Cargo.toml 体积红线（禁 tokio/reqwest）完全兼容——长轮询就是一个 std 线程上的阻塞 GET（`getUpdates?timeout=50`）。

局域网方案需要同网段 + 手机端保活，出不了门；蓝牙影响音频设备且无可靠推送。两者放弃。

## 调研参考（GitHub，2026-10-17）

| 仓库 | ★ | 验证了什么 |
|---|---|---|
| `overwirehq/claude-code-telegram` | 2814 | Python 常驻 + headless Claude；功能天花板（会话持久化、白名单、目录沙箱） |
| `grinev/opencode-telegram-bot` | 1259 | 连本地 OpenCode server；**后台会话通知**（reply/ask/permission 三种等待都推）、**内联按钮批权限/答问**、忙时消息排队、置顶实时状态 |
| `artickc/opencode-telegram-bot` | 11 | 用 **ACP over stdio** 驱动 OpenCode；Inline approvals（Approve/Deny 按钮）+ 远程 `/cancel` 停回合 |
| `gohyperdev/hdcd-telegram` | 6 | Rust 单二进制（3.5MB / ~5MB RAM）；Permission relay 内联键盘；证明 Rust 小体积做全功能 bot 可行 |
| `Davasny/opencode-telegram-notification-plugin` | 22 | 只发完成通知的最小形态（hosted bot + revocable key） |
| `gene1wood/claude-code-ntfy-notification-hook` | — | ntfy 替代通道（自托管要额外服务，较重，不做） |

共识形状：出站长轮询 + 用户 ID 白名单 + 内联按钮回调解答。

## 架构

```
                        ┌────────────────────────── Marionette (Rust) ──────────────────────────┐
 agent stdout ──▶ acp.rs 事件收口 ──┬─▶ emit_event → 前端 UI（现状，不变）
   (ACP JSON-RPC)                  └─▶ telegram_remote::enqueue(事件) ──┐
                        │                                                │ mpsc（fire-and-forget）
                        │   waiter maps（permissions/questions/          ▼
                        │   plan_approvals/elicitations）         telegram 线程（std thread）
                        │      ▲                                    · getUpdates 长轮询(timeout=50s)
                        │      │ resolve(request_id, outcome)      · sendMessage / answerCallbackQuery
                        │      └──────────────────────────────────  · 失败退避重试（1s→60s 封顶）
                        └────────────────────────────────────────────────────────────────────┘
                                    手机 Telegram ⇆ api.telegram.org（双向，全出站）
```

### 通知钩子（发送侧）

全部是现有单一收口点，加旁路调用即可：

| 事件 | 锚点 | 推送内容 |
|---|---|---|
| 回合结束 | `acp.rs` `emit_turn_complete()`（~5578）/ `emit_turn_complete_for_session()` | 会话名 + 耗时 + stop_reason + 末条回复摘要（超长截断；代码块可作 monospace 段） |
| 等权限 | `session/request_permission`（`acp.rs:3638`，事件 `permission/prompt`） | 标题/详情（复用 `permission_summary`）+ options 内联按钮 |
| 等 ask | `question/prompt`（`acp.rs:3738`） | 问题 + 选项按钮（开放式问题则提示用文字回复） |
| 等 plan 审批 | `plan/approval`（`acp.rs:3940`） | 摘要 + 批准/拒绝按钮 |
| 进程死亡/报错 | 现有 process 退出路径 | 「会话挂了」+ 原因 |

通知一律经 mpsc 队列交给 telegram 线程串行发送，**绝不在事件收口里同步做网络 IO**；发送失败只记 `dev.log`（`[telegram]` 标签），不影响 agent 流程。

### 解答/回复（接收侧）

- **文本回复 → 注入会话**：复用 `commands.rs:1777` `send_acp_prompt` 的内核（提为普通函数，Tauri command 变薄壳，telegram 线程同调）。已挂起会话的「下次发消息自动接上历史」行为自动获得。
- **路由**：Telegram「引用回复」某条通知 → 该通知所属会话（bot 记住自己发出的 message_id → session 映射）；裸发文字 → 当前绑定会话（最近活跃者）。
- **权限/ask/plan 按钮**：`callback_query` → 与 UI 相同的 resolve 命令族 `respond_acp_permission`（`commands.rs:2181`）/ `respond_acp_question`（2191）/ `respond_acp_plan_approval`（2205）同内核。按钮即 ACP options 的直接映射（allow once / always / deny…）。
- **先答先得**：UI 和 Telegram 消费同一批 waiter map（`request_id` 为键，`map.remove` 语义），后到者收到「已处理」。telegram 线程无 State，同样把三个 respond 的内核提为可无 UI 调用的函数。

### telegram 线程本身（新文件 `src-tauri/src/telegram_remote.rs`）

- std 线程 ×1；`ureq` 阻塞调用；`getUpdates` `timeout=50`，断线指数退避（1s 起，60s 封顶），409（同 token 第二个 poller）按退避重试并记 dev.log。
- 发送节流：单 chat 串行足够（Telegram 限速 ~30 msg/s 用不到）。
- 退出：跟随进程；`panic = "abort"` 下线程随进程消亡，无清理负担。
- 白名单：只处理配对过的 `chat.id`（单值），其余静默丢弃并记一条 warn。

### 配置与凭据

- bot token：`~/.marionette/credentials.json`（DPAPI，`credentials.rs` 现有 store）新增 `telegram-bot` 类型条目，`source: manual`；沿用「明文绝不进日志/agent 配置」红线。
- 其余（启用开关、chat id、通知偏好）：并入 `~/.marionette` 现有配置。

## 用户体验

### 首次配对（入口：左下角浮层 `ProjectShelf.tsx:257` 处加「远程通知」块，或设置页）

1. @BotFather `/newbot` 建新 bot（一分钟；与 NAS 的 `@NewtronNewsbot` 各用各的 token）。
2. Marionette 设置里粘 token → 启用。
3. 用户给 bot 发 `/start` → Marionette 弹「检测到配对请求（@用户名）→ 接受」→ 固化 chat id 白名单。
4. 完事。bot 只认这个 chat。

### 日常形态（Telegram 里一个私聊会话）

```
[Marionette 主线] ✅ 跑完（4分12秒）
  重构完成，改了 3 个文件，测试全绿——……
  💬 回复这条消息 = 继续这个会话

[NAS 脚本] ⏸ 在等你：agent 想执行 rm -rf build/tmp
  [允许一次] [总是允许] [拒绝]        ← 内联按钮

你: 跑一下测试
Bot: 已送进 [Marionette 主线] ✓
```

多会话靠前缀区分；引用回复定向路由；通知偏好开关：跑完推 / 等你推 / 错误推 / 内容过云端的隐私开关（只推「✅」，`/last` 拉摘要）。

## 权限超时策略（已定）

现状 `TIMEOUT_PERMISSION_SECS = 120`（`acp.rs:20`）到点自动选拒绝，防 agent 挂死。**保持 120 秒**，
开启 telegram 后到期前 ~30 秒追加一条「快回来，要自动拒了」提醒推送。agent 不为没被看到的请求干等。

## 分期

**MVP（本次实现）**
1. `telegram_remote.rs`：配置读取、轮询线程、发送队列、配对状态机、退避。
2. 通知：回合结束 / 等权限 / 等 ask / 等 plan / 进程死亡。
3. 文本回复注入（引用路由 + 最近活跃绑定）。
4. 权限/ask/plan 内联按钮（resolve 内核复用）。
5. 设置 UI：token 粘贴、启用、配对确认、通知偏好。
6. 到期前 30s 提醒。

**二期（按需）**
- `/status` 活点单 + 置顶实时状态消息（学 grinev）。
- 远程 ⏹ 停止按钮（复用 `stop_acp_session`，`commands.rs:1997`）。
- `/sessions` `/switch` 多会话切换。
- 通知去噪（仅窗口失焦时推 / 仅后台会话推）。

**明确不做**
- 常驻服务/开机自启的独立 notifier 进程；Telegram 端会话管理器（建会话、切模型——那是把 Marionette 搬进 Telegram，违背壳的定位）；ntfy/Bark 等第二通道；语音输入。

## 验证

- Rust 单测：payload 构造、白名单过滤、reply_to → session 路由决策（纯函数化）。
- 端到端 smoke：真实 token 手动跑，`dev.log` 过滤 `[telegram]` 标签（与 AGENTS.md 排查习惯一致）。
- 409 场景：同 token 起第二实例确认退避与恢复。
- 回归：无 token / 未启用时零行为变化（钩子短路）。

## 工程量预估

`telegram_remote.rs` 400–600 行 + acp.rs 旁路钩子若干 + commands 内核提取 + 左下角浮层一小块 React；
依赖零新增，便携包体积不变。
