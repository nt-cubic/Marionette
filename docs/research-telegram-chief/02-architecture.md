# 架构方案：Chief 会话与控制面

## 1. 设计约束

本方案继续遵守 Marionette 的现行底线：

- 不自研 Agent runtime；
- ACP 优先，会话绑定 Agent；
- Agent、模型、Provider、项目与会话仍是 Marionette 现有对象；
- localhost MCP 是工具进入 Host 的统一通道；
- 浏览器与 Computer Use 的授权仍由 Host 和本地用户掌握；
- Telegram 只是远程消息面，不是第二套会话数据库。

新增的只有一个东西：**Host 侧 Chief 编排器**。它不推理、不生成内容，负责把 Chief 的模型意图翻译成
对普通 Marionette 会话的安全、持久、可审计操作。

## 2. 总体数据流

`````````text
Phone / Telegram
        │  natural language, approvals
        ▼
Telegram Bot API
        │  long poll / send queue
        ▼
telegram_remote.rs
        │  fixed chat → chiefSessionId
        ▼
ChiefBroker (Rust)
        │                         ┌─────────────────────────────┐
        ├── prompt/events ───────▶│ Chief ACP session            │
        │                         │ user-chosen agent + model    │
        │                         │ control MCP tools only        │
        │                         └──────────────┬──────────────┘
        │                                        │ tools/call
        │                                        ▼
        │                         marionette_* control tools
        │                                        │
        │                         validate + persist + schedule
        │                                        ▼
        │                         TaskBroker / WorkerRunner
        │                                        │
        │                         ordinary ACP worker sessions
        │                         project cwd + agent + model + tools
        │                                        │
        └──────── events / summaries ◀───────────┘
                        │
                        ├── local Marionette UI
                        └── Telegram messages / approval cards
`````````

关键点：Telegram 消息先进入 Chief，而不是“自动送到最近活跃会话”。Chief 通过 MCP 工具请求 Host
创建或操作 worker；worker 不是另一套 runtime，而是现有 `Session`。

## 3. 现有基础设施盘点

| 现有能力 | 现状 | Chief 如何复用 | 仍需新增 |
|---|---|---|---|
| ACP 会话 | `AcpService::start` / `send_prompt` / `cancel` 已存在，事件经统一 `emit_now` 出口 | Chief 与 worker 都用它 | Rust 侧可恢复的 runner 与 durable event tee |
| 项目 / 会话存储 | `storage.rs` 已有 project、session、transcript、native session id | worker 使用普通 Session；Chief 也是普通 Session | 任务账本与 Chief/worker 关系 |
| 模型 / Provider | 跨用引擎与会话 `preferredModel` 已存在 | 用户预先绑定 Chief / worker 可用模型 | 只读能力查询与 Host 校验 |
| localhost MCP | `tools_mcp.rs` 已有每会话 Bearer token、`tools/list`、`tools/call` | 在同一通道加入 Chief 专用工具 | session-aware schema、能力隔离、token 生命周期 |
| Telegram | 长轮询、配对、发送队列、rich message、权限 waiter 共享、总控面板 | 复用连接、审批和投递；新增 Chief 模式 | 持久 update cursor、固定 Chief 路由、任务聚合 |
| Browser | 既有 read / control 分级与权限卡 | worker 可直接使用 | Chief 不能替用户创建无限授权 |
| Computer Use | 既有窗口共享与工具白名单 | 可操作已共享窗口 | 远程新建共享、远程接管仍是后续问题 |
| Council | 有持久状态机、恢复、审计 | 后续可当作一种 worker 类型 | 不在第一版接入 |

## 4. 核心对象

### 4.1 Chief session

Chief 是普通 ACP 会话，满足：

- 有一个稳定的 `sessionId`；
- 属于 Chat 逻辑组，或一个专门的内部 Host 项目；建议第一版使用 Chat，避免伪造用户项目；
- `origin = "chief"`；
- 绑定用户选择的 `agentId`；
- 模型与 Provider 走现有会话偏好与跨用引擎，不另建 provider 配置；
- 用户可在本地 UI 打开并继续同一段对话；
- Agent 进程挂起后，下一条 Chief 消息按现有历史恢复路径重连；
- 控制工具只注入这一会话，普通会话的工具列表不出现它们。

同一个已配对 Telegram 私聊只能映射一个 `chiefSessionId`。重新初始化 Chief 时必须显式“换一段 Chief”，
不能静默把旧聊天接到新会话。

### 4.2 Worker session

Worker 是普通可见会话：

- 属于真实项目目录，或用户明确允许的 Chat 工作目录；
- `origin = "chief-worker"`；
- `agentId`、cwd、模型、模式与权限都来自 Host 校验后的任务配置；
- 有正常的工具轨迹、审批、diff、测试结果与停止语义；
- 用户在本地 UI 可以打开、接管、停止或继续发消息；
- 任务账本记录 `taskId ↔ workerSessionId` 关系。

第一版**不要**把 worker 的 `parent_session_id` 指向 Chief 来“顺便建立关系”。现有语义中非空
`parent_session_id` 会把会话从普通列表过滤掉，并与 `@` delegate 子会话共用一套 UI 假设。
worker 应保持顶层可见；关系放任务账本里。以后若要统一“父子会话树”，再单独升级存储和 UI 语义。

### 4.3 Task

建议的最小任务记录：

`````````json
{
  "taskId": "task_...",
  "requestId": "chief-generated idempotency key",
  "chiefSessionId": "session_...",
  "title": "修复远程构建失败",
  "goal": "完整自然语言目标",
  "projectId": "project_...",
  "workerSessionId": "session_...",
  "agentId": "codex",
  "model": "provider/model or agent-native option",
  "status": "running",
  "createdAt": "...",
  "updatedAt": "...",
  "lastEventAt": "...",
  "lastSummary": "...",
  "approvalState": null,
  "artifacts": [],
  "telegramMessageId": 123,
  "error": null
}
`````````

状态建议：

`````````text
proposed
  ├─▶ starting ─▶ running ─┬─▶ waiting_user ─▶ running
  │                        ├─▶ completed
  │                        ├─▶ failed
  │                        ├─▶ cancelled
  │                        └─▶ interrupted
  └─▶ cancelled
`````````

约束：

- `completed / failed / cancelled` 是终态；
- `interrupted` 表示进程丢失、应用重启或 heartbeat 过期，不等于用户取消；
- 同一 worker 只能有一个 active task；
- 同一 Chief 默认最多一个 active task，可配置到三个；
- `task_cancel` 幂等，重复调用不会二次破坏；
- `task_resume` 只能恢复 `interrupted` 或明确可继续的 `failed`，不能把 `completed` 重跑。

### 4.4 Agent / 模型 / Provider 绑定

用户所谓“指定提供商”不新增第四套配置，而是引用现有跨用引擎的 Provider / 模型绑定：

- Chief 配置保存 `agentId`、模型或别名、以及现有 Provider binding id；
- Host 在启动前解析成该 Agent 真正接受的 model / config option；
- Chief 可以在项目允许列表内请求换模型，但不能新增 / 修改 Provider key 或全局默认；
- 不同 Agent 的 model id 不保证可移植，Host 必须按该 Agent 的 caps 校验；
- Provider 认证失效、余额不足、限流与模型不存在要分类回报；
- UI 与 Telegram 状态卡显示“最终实际生效”的 Agent / 模型 / Provider，而不是用户输入的字面值。

可为常见选择配置别名，例如“便宜模型 / 主力模型 / 高推理模型”。Chief 可以按任务风险与预算选择别名，
但别名只能映射到用户已经批准的 binding。

## 5. Chief 控制 MCP

### 5.1 注入方式

继续使用 `tools_mcp.rs` 的 localhost HTTP MCP：

1. 每个 ACP 会话仍有独立 Bearer token；
2. `tools/list` 从静态 `tool_schemas()` 改为按 `session_id` 返回能力；
3. `dispatch` 已带 `session_id`，在入口再次校验会话确实是 Chief；
4. 普通 worker 即使猜到工具名，也因 schema 和能力校验不可见 / 不可调用；
5. 控制工具的所有拒绝使用可读返回值，例如 `{"error":"project_not_allowed","note":"..."}`，
   不让 Chief 因一次拒绝丢掉整轮。

建议模块边界：

`````````text
src-tauri/src/chief/
  mod.rs         生命周期与启动
  types.rs       Task / Event / Config / Tool DTO
  store.rs       tasks.json、events.jsonl、原子写与恢复
  tools.rs       Chief 专用 MCP schema / dispatch
  broker.rs      命令校验、并发、调度、幂等
  runner.rs      ACP Chief / worker 进程驱动与事件收口
  telegram.rs    Chief 消息渲染、审批、聚合
  policy.rs      项目白名单、能力上限、自我修改保护
`````````

### 5.2 工具集合

第一版控制在 10 个：

| 工具 | 输入 | 行为 | 关键限制 |
|---|---|---|---|
| `marionette_projects_list` | 无或查询词 | 返回允许 Chief 使用的项目、别名、最近活跃状态 | 不返回未授权根目录的敏感元数据 |
| `marionette_agents_list` | 可选 `projectId` | 返回已安装、已启用、可认证的 Agent | 不可用的 Agent 明确标原因 |
| `marionette_models_list` | `agentId`、可选项目 | 返回可用模型、模式与 effort | 复用现有 ACP caps / 跨用引擎 |
| `marionette_tasks_list` | 状态、数量 | 返回当前 Chief 的任务摘要 | 只列本 Chief 创建的任务 |
| `marionette_task_start` | `requestId`、目标、项目、可选 Agent/模型/模式 | 立即返回 `taskId`，后台启动 worker | 不接受 cwd、任意权限、provider key、MCP 配置 |
| `marionette_task_send` | `taskId`、补充指令 | 向 running / waiting worker 注入后续指令 | 不绕过 worker 自己的权限 |
| `marionette_task_status` | `taskId` | 返回状态、最后摘要、当前等待事项、耗时 | 不返回未脱敏 token |
| `marionette_task_cancel` | `taskId`、原因 | 走现有 `session/cancel` | 幂等 |
| `marionette_task_collect` | `taskId` | 返回最终正文、changed files、diff 摘要、测试结果、附件 | 未完成时返回 interim，不假装最终 |
| `marionette_task_resume` | `taskId`、可选补充指令 | 重连挂起 worker 或新开 continuation | 不自动重放高风险副作用 |

不建议第一版加入：

- `task_approve`：Chief 不能替用户批准；
- `project_add`：新根目录只能本地确认；
- `settings_update`：权限、Provider、凭据、模型默认值只能本地修改；
- `shell_exec`：不要让 Chief 绕过 worker 的工具与审批模型；
- `task_spawn_child`：先不做递归委派。

### 5.3 为什么任务启动立即返回

`task_start` 必须快速返回，不能同步等任务完成：

- MCP call 不应挂几十分钟；
- Telegram 不能因为 worker 长时间运行而失去“已接单”反馈；
- Chief 可以继续回答用户、查询状态或再启动第二个任务；
- 应用重启和网络断开后，任务仍由任务账本表示。

## 6. 执行面：不要让前端 WebView 成为唯一 orchestrator

这是一个必须写进施工红线的问题。

现有 Telegram 入站文本通过 `telegram-remote-inbound` 交给 `src/app/App.tsx` 的 `performSend`，
因为“挂起重连、历史注入、排队、配置回推”目前主要在前端。作为人工远程回复路径，它工作得很好。

但远程 Chief 是无人值守路径，而主窗最小化 / 隐藏会触发 WebView2 的 `TrySuspend`。若 Chief 或 worker
完全依赖前端闭包、React state 和当前 Composer 快照：

- 最小化时可能不处理任务；
- 页面重载会让在途命令丢失；
- 多个异步任务会与 UI 焦点、分屏状态耦合；
- 无法证明“电脑开着但 Marionette 在后台”时仍可靠。

因此建议：

1. **M1 就使用 Rust 侧 `ChiefBroker / WorkerRunner` 驱动 Chief 与 worker 的 ACP 生命周期。**
2. 复用 `AcpService`，不复制 Agent runtime；把目前 Tauri command 里的 start / send / cancel / config
   内核提为 Rust 可直接调用的普通函数。
3. 在 `emit_now` 旁增加 durable event tee：Chief 创建的会话把标准化 `AcpEvent` 追加到
   `~/.marionette/chief/sessions/<sessionId>.events.jsonl`。
4. WebView 恢复时既接收实时事件，也能从事件文件重建 worker 视图；避免“后台跑完但本地 UI 空白”。
5. 现有前端 `performSend` 继续服务人工输入；远期再把它和 backend runner 共用的逻辑收口，
   不在第一版冒险重写整个 UI 管线。

如果 M0 证明后端 ACP 驱动无法在所有目标 Agent 上稳定工作，就应缩小首发 Agent 支持矩阵，
而不是退回“靠前端碰运气”的伪无人值守。

## 7. 事件与唤醒

### 7.1 事件分类

每个任务至少产出以下 durable event：

| 事件 | 触发 | 是否唤醒 Chief | 是否通知手机 |
|---|---|---|---|
| `task.created` | Host 创建任务 | 否 | 发简短“已接单” |
| `task.started` | worker ACP ready | 可合并 | 发项目 / Agent |
| `task.progress` | 关键工具里程碑 | 否 | 按用户偏好，默认不逐条发 |
| `task.waiting_user` | permission / ask / plan | 直接发批准或问题卡 | 是 |
| `task.completed` | 回合正常完成 | 是，合并后唤醒一次 | 是 |
| `task.failed` | Agent / Provider / 进程失败 | 是，合并后唤醒一次 | 是 |
| `task.interrupted` | heartbeat 丢失或重启 | 是 | 是 |
| `task.cancelled` | 用户或 Host 取消 | 视来源 | 是 |

### 7.2 唤醒规则

不能让 worker 的每一条更新都作为一轮新 prompt 打入 Chief；会造成：

- token 浪费；
- Chief 自己打断用户；
- 事件风暴和 Telegram 重复；
- 模型在“没完成”时抢着下结论。

建议：

- Host 直接处理 `waiting_user` 的交互卡，不先让 Chief 决定批准；
- `completed / failed / interrupted` 进入 Chief 的 event inbox；
- 同一任务在 5–15 秒窗口内的事件合并成一条 Host Event；
- 每次 Chief 回合结束后再消费下一批事件；
- Chief 不在线或正在忙时，Host 仍发确定性状态，不让事件丢失；
- 每个事件有 `eventId` 与 `sequence`，消费状态落盘，防止重启后重复汇报。

Host Event 应是结构化、带信任标签的文本，例如：

`````````text
[HOST EVENT: task.completed]
taskId: task_...
status: completed
summary: 修复完成，2 个测试通过
artifacts: 3 files changed; test output available
untrustedContent: false
`````````

worker 的正文与网页内容必须标为不可信数据，不得被 Chief 当作系统指令执行。

## 8. Telegram 路由

### 8.1 模式

建议增加 Host 级 `remoteWorkMode`：

`````````text
off
relay       // 现有“通知 + 回复 + 批准”
chief       // 所有普通文本进入 Chief
`````````

默认继续 `relay`，用户显式配置并测试后才切 `chief`。切换时：

- Telegram 发一条状态卡；
- 本地 UI 显示当前模式；
- 未完成的旧会话通知仍可保留，但裸文本不再路由到它们；
- 不删除旧桥的引用回复与审批能力。

在 `chief` 模式下，`/new`、`/model` 等旧控制命令不再用于主流程；只保留少量 Host 处理的确定性逃生口
（例如停止、状态、帮助）。未知 slash 文本可原样交给 Chief，由 Chief 用自然语言解释，不另造一套命令语义。

### 8.2 入站

1. 校验 Telegram user / chat / update_id；
2. 把 `update_id` 持久化为 cursor，重复 update 忽略；
3. 把文本、图片、文件等转成 Host inbound message；
4. 发送给 Chief：如果 Chief 正忙，进入每 Chief 的 FIFO；
5. 用户引用某个任务卡时，在消息 metadata 中附带 `taskId`；裸文本不附默认 worker。

Telegram 的 24 小时 update 保留不能代替本地 offset 持久化，否则应用重启后可能重复处理旧消息。

### 8.3 出站

出站分两类：

- **确定性 Host 卡**：任务开始、进度、审批、失败。格式固定，不依赖 Chief 在线；
- **Chief 自然语言回复**：解释、综合、追问和最终交付。

每条任务卡注册 `messageId → taskId` 路由，便于引用回复；与现有通知路由相同的 FIFO + 过期策略。

### 8.4 审批

复用 ACP waiter：

- `permission` 卡片按钮直接映射 ACP options；
- ask / plan 按钮按现有结构映射；
- 手机与本地 UI 先答先得；
- 120 秒超时仍按拒绝；
- Chief 不参与“是否批准”的决策；
- 危险动作卡片必须展示命令、目标路径、窗口名或 diff 范围，而不只是“允许 agent？”。

## 9. 持久化

建议布局：

`````````text
~/.marionette/chief/
  config.json                  Chief 配置与允许项目（无凭据）
  tasks.json                   任务快照，原子写 + schema version
  events.jsonl                 全量 Host 事件，append-only
  sessions/
    <sessionId>.events.jsonl   Chief / worker 的 durable ACP 事件
  cursors.json                 Telegram update offset、事件消费位置
  audits/
    <yyyy-mm>.jsonl            审批与安全高审计记录
`````````

要求：

- `tasks.json` 使用临时文件 + rename，单 writer，带 `version`；
- `events.jsonl` 是追加式事实记录，失败恢复时以它校验快照；
- 敏感 token 不进入这些文件；Telegram token 继续留现有 DPAPI 文件；
- 日志中的文件内容与 command output 按长度和敏感模式截断；
- 所有路径先 canonicalize，再做项目根目录包含判断；
- 保留期可配置，默认任务与审计至少 90 天。

## 10. 启动、恢复与幂等

### 10.1 启动恢复

Marionette 启动时，`ChiefBroker` 应：

1. 读取 config、tasks、cursor；
2. 校验 Chief session 与 worker session 仍存在；
3. 对每个 `running` 任务检查 ACP 进程与 freshness；
4. 没有活进程的任务标 `interrupted`，不标 `completed`；
5. 重建 Telegram 总控 / 当前任务卡；
6. 将未消费事件送入 Chief event inbox；
7. 不自动重放有副作用的命令；需要继续时生成明确 continuation。

### 10.2 幂等

- Telegram：`(chatId, updateId)` 去重；
- 任务启动：`requestId` 去重，同一 Chief 重试返回同一 `taskId`；
- 发送：`(taskId, commandId)` 去重；
- 取消：任务进入终态后直接返回成功；
- 事件消费：`eventId` / sequence 去重；
- 出站消息：保存每个任务的最后已发送状态与 cursor，重启后只补未确认的关键消息。

不要承诺“端到端 exactly once”。正确目标是：**传输可重复，业务动作幂等**。

## 11. 自我修改任务

用户明确希望以后能说“帮我改 Marionette”。这可以作为高价值 dogfood 场景，但要特殊保护：

- 仅在明确允许的 Marionette 仓库执行；
- 默认创建独立 git worktree / branch，不直接改主工作树；
- 允许读、改、build、test；
- 在手机与本地都显示“这是 Marionette 自身修改”；
- push、创建 PR、合并主分支、打 tag、发布 Release、重启正在运行的版本必须本地确认；
- 如果任务会替换当前 exe，必须禁止自动执行，只生成可审阅产物；
- 完成后提供 diff、测试、可选的版本号建议和回滚方法。

这条路径同时是“产品能远程迭代自己”的演示，也是最高风险的供应链入口，不能按普通项目任务处理。

## 12. 与 Council 的关系

第一版不做 `Chief → Council → workers` 的嵌套编排。

后续可把 Council 当作一种任务类型：

- Chief 调用 `task_start(type="council")`；
- Host 使用现有 council 状态机，不绕开 Plan Gate；
- Council 的裁决仍回本地 / Telegram 审批；
- Chief 只负责把用户目标写清楚并汇总结果。

在这之前，Chief 不应绕过 Council 的 Plan Gate，也不应把 Council 当成一个无限可调用的子 Agent。

## 13. 建议实施顺序

1. 抽出 backend ACP start / send / cancel 内核；
2. 加 Chief / worker session role 与 durable event tee；
3. 建任务 store 与 broker；
4. 在 `tools_mcp.rs` 注入 session-aware 控制工具；
5. 把 Telegram 普通文本在 Chief 模式下固定送 Chief；
6. 复用权限 waiter 发 worker 审批；
7. 完成恢复、幂等和本地 UI 任务视图；
8. 最后再开放多 worker、附件、Browser / Computer Use 的更高等级。

安全细节见 [03-security-permissions.md](./03-security-permissions.md)，逐阶段验收见
[05-mvp-plan.md](./05-mvp-plan.md)。
