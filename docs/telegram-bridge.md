# Telegram 远程桥

> 状态：**MVP + 总控面板 + 回复全文 Markdown 已实现（2026-10-18）**。通知四类
> + 文本回复 + 权限/ask/plan 内联按钮 + 配对 + 到期前 30s 提醒 + **置顶总控面板
> （多会话状态一目了然）/ `/status` / 远程 ⏹ / 面板点选切换绑定** + **跑完推送带
> 全文正文（MarkdownV2，多消息分片，解析失败回退纯文本）** 已落地。
> 实现与设计的差异见文末「实现差异记录」。

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
- ~~`/status` 活点单 + 置顶实时状态消息~~ **已落地（2026-10-18，见下「总控面板」节）**。
- ~~远程 ⏹ 停止按钮~~ **已落地**（走 `AcpService::cancel`，与 UI 停止键同一条 `session/cancel` 路，比原稿计划的 `stop_acp_session` 杀进程更轻）。
- ~~`/sessions` 多会话切换~~ **已落地**（面板按钮点选即切换绑定）。
- 通知去噪（仅窗口失焦时推 / 仅后台会话推）。
- `/last`（拉最近一条回复全文）。

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

## 实现差异记录（2026-10-18 MVP）

按设计落地，四处偏差，都比原稿更省或更稳：

1. **事件钩子收口到 `emit_now` 单点**（而非每类事件各挂一处）：`acp.rs` 所有 UI 事件
   （排队与直发两条路）都经 `emit_now` 落地，telegram 侧 `observe_ui_event` 只观察这一个
   点，按 `(kind, method)` 过滤五类事件。回合时长/末条回复摘要在观察侧从
   `session/update` 流里自维护（`user_message_chunk` 钉回合起点、`agent_message*`
   进尾部滚动缓冲，多字节安全截断），不动 acp.rs 的任何回合状态。
2. **文本回复走前端管线而非 Rust 直发**：设计原稿计划把 `send_acp_prompt` 内核提为
   普通函数，但「挂起重连 + 历史注入 + 排队 + 配置回推」整条管线住在 App.tsx 的
   `performSend` 里（约 400 行 TS）。Rust 侧收到手机文本后 `emit("telegram-remote-inbound")`
   → 前端切到目标会话并调 `performSend` → 完成后 `telegram_remote_ack` 回执（✓ 或错误
   摘要推回手机）。本地打字与远程回复从此走同一条路，行为永远一致。
3. **按钮解答在 Rust 直连 waiter map**（与设计一致），另补了 `permission/resolved` /
   `question/resolved` / `plan/resolved` 三个事件：UI 与 Telegram 任一方解答后，
   另一方的卡片（和提醒计时）立即收掉，先答先得不残留。
4. **token 存 `~/.marionette/telegram.json`**（设计原稿是 credentials.json 新条目种类）：
   复用 `credentials.rs` 的 DPAPI 加解密出口（同 `dpapi:` 前缀格式），但独立成文件——
   避免给 `CredentialKind` 加变体而波及模型桥的凭据枚举/去重逻辑。明文红线不变：
   落盘只密文，日志只记事件。

其他实现要点：

- 配对：保存 token 后 bot 的第一条私聊消息挂起为待配对请求（TTL 5 分钟），左下角浮层
  1Hz 轮询展示并确认；轮询线程只要有 token 就跑（否则配对死锁），发送/通知额外要求
  已配对且开关打开。
- 引用回复路由靠 bot 自己发出的 message_id → session 表（FIFO 256 条）；裸文本去
  最近收到 `user_message_chunk` 的会话。getUpdates 24h 保留：Marionette 重启后，
  停机期间手机上发的消息照常送进会话。
- ask 按钮：仅「单问题 + 单选」映射内联按钮（+「跳过」）；多问题/多选推送文本提示
  回电脑答。应答按问题原文匹配 wire key，绝不截断。
- 命令（`/` 开头）不进会话，MVP 没有命令面（`/status`、`/last` 属二期）。
- 409（同 token 第二 poller）从 15s 起退避；断线 1s 起、60s 封顶。
- 验证：`telegram_remote` 11 个单测（payload/路由纯函数、密文落盘不含明文、损坏
  留档）+ 全套 280 过 + `npm run build`（tsc strict）；本机到 api.telegram.org 的
  TLS 与 API 行为已用假 token 探活（401 予期）。真机端到端（真实 bot token 收发）
  留给用户首次配对时按浮层指引完成。

## 总控面板（2026-10-18 二期首批，已落地）

多会话可见性是纯推送形态的最大短板（用户实测反馈：多开时不知道谁在跑）。调研
grinev / artickc / RemoClaw 三个同类项目后收敛为共识形态，全部落进 Rust 侧：

- **置顶面板**：一条 `pinChatMessage` 的消息，会话状态一变（开跑 / 跑完 / 等你 /
  等待解除 / 绑定切换）就 `editMessageText` 原地重画——头部计数（▶N ⏸N）+ 每会话
  一行（状态图标、标签、agent、时长 / 等待）+ `⏎ 当前绑定` 标记 + 未列出的空闲数。
  面板被删会在下次 edit 失败时自动重建并重置顶；「message is not modified」视为成功。
- **状态机**：`SessionTrack.waiting`（挂起的权限/ask/plan 计数，prompt 时 +1、六类
  `*/resolved|*/timeout` 事件时 -1）+ `turn_started` 推导 Running / Waiting / Idle，
  等待优先于跑着。与 `notify_wait` 偏好解耦——面板永远反映真实状态。
- **行内按钮**：每个会话一行 `[标签][⏹]`——点标签 = 把裸发文字的默认去向切过去
  （原来隐式的「最近活跃绑定」变成看得见点得动）；⏹ = `AcpService::cancel`（与 UI
  停止键同路：`session/cancel` + in-flight 清理 + `turn/complete(cancelled)`）。
  末行 🔄 刷新。面板消息的键盘不会被逐按钮摘除（由下一次整画重写），权限卡片的
  摘按钮行为不变。
- **`/status`（别名 `/sessions`）**：全量清单（含空闲，按最近动静排序，上限 20 行），
  reply 到用户那条命令。
- **心跳**：单独线程，忙时每 60s 重画一次面板（「跑了 N 分钟」保鲜）；全闲零流量。
- **权限推送格式修复**：detail 里的工具入参原始 JSON（`{"file_path":…}`）渲染成
  `key: value` 行（`render_tool_detail`）；非 JSON（plan markdown 等）原样保留。
- 锁序红线：`inner`（telegram 可变态）与 storage/acp 的锁绝不嵌套——快照先取
  inner 后查 storage，回调先摘动作再进 acp。
- 验证：模块单测 11 → 17（面板/清单渲染、JSON 人话化、waiting 状态推导、六类
  清理事件匹配），全套 289 过，`cargo check` 零警告。

## 回复全文 + MarkdownV2（2026-10-18 二批，已落地）

用户实测（11:02–11:28 的推送）暴露：跑完推送既无时长也无正文。根因两层——

1. **Grok 不回显用户消息**（探针实测 2026-10-18：`session/update` 里只有
   `agent_message_chunk` / `agent_thought_chunk`，没有 `user_message_chunk`），
   而回合起点原本钉在用户回显上 → Grok 会话永远 `turn_started=None`。
2. MVP 的取值链 `started.map(|t| (…, tail)).unwrap_or((None, ""))` 把正文缓冲的
   存在性与回合起点焊死——起点是 None 时攒好的正文也被一起丢掉。

修复与增强：

- **回合起点改锚「首个活动」**（agent 文本 / 工具 / thought 首次出现时钉住，
  仅当当前为 None）；首个活动同时赢得裸文本路由（Grok 会话从此也能当绑定目标）。
  正文缓冲与起点彻底解耦。缓冲上限 8K → 32K（约等于全文）。
- **MarkdownV2 全文推送**：回合回复以 `parse_mode: MarkdownV2` 发送，转换器
  （`md_to_markdownv2`）处理围栏代码块（丢语言标记、内容仅转义反引号）、行内
  code、**bold**、*italic*（CommonMark 词内下划线规则：`file_name_x` 字面量、
  `2*3*4` 斜体）、[链接](url)（URL 只转义 `)` 与 `\`）、标题行加粗；其余保留字
  逐字转义。**Telegram 拒绝解析时自动回退纯文本重发**——格式化是增益，投递是
  底线。
- **分片**：先在原文上按空行切块（围栏块原子、单块超限按换行硬切、多字节安全），
  每片独立转换成文（无跨片实体），每条 ≤3300 字符，最多 4 条（约 13K 字符），
  更长截断提示回电脑看。主消息带 `[会话] ✅ 跑完 · 时长` 头部，末条带页脚与
  引用路由提示。
- **settings 加固**：`telegram.json` 解析失败（截断/瞬态）改为原地保留 + 限频
  告警（浮层 1Hz 轮询曾把归档告警刷成串，且归档会把 token 一起带走）；只有
  「未来版本」才留档重置。
- 验证：模块单测 17 → 22（转义、bold 闭合成对、词内下划线、围栏、链接、分片
  原子性、settings 两分支），全套 294 过。

## 面板 400 事故（2026-10-18 三批，已修）

面板上线后一条都发不出去（`dashboard send failed | HTTP 400 · sendMessage` 反复刷，
纯文本推送不受影响）。根因：`send_message_inner` / `edit_message` 里
`reply_markup` 直接塞了**行数组的裸 JSON**，而 Bot API 要求对象壳
`{"inline_keyboard": [[…]]}`——带键盘的消息（面板、权限卡片）全部被 400 拒。
真实 bot 探针双向验证：裸数组 ok=false，包壳 ok=true。修复 `keyboard_json` →
`reply_markup()`（返回对象壳），回归测试钉住形状。

同批加固：

- `tg_call` 的 `ureq::Error::Status` 分支原先丢弃响应体，只剩 `HTTP 400 · 方法名`
  ——现在把错误体里的 `description`（400 的唯一线索）带进错误串与 `last_error`。
- MarkdownV2 失败回退纯文本的条件从「仅 can't parse」放宽到**任何失败**
  （转义后超长等）：投递优先，格式化是增益。
- 分片加转义膨胀护栏 `fit_markdown_chunk`：片转 MarkdownV2 后超 3400 就在原文
  上对半重切（换行优先），避免「原文 ≤3300、转义后 >4096」的静默降级。
- `send_message_inner` 的 text 双闸：`clip`（3800 chars）后再按 UTF-16 码元闸
  4000（emoji 占 2 码元，chars 计数会低估 4096 硬顶）。
- 验证：模块单测 22 → 25（reply_markup 形状、膨胀重切、UTF-16 闸），全套 297 过；
  真实 bot 端到端确认两种形态的接受/拒绝。
