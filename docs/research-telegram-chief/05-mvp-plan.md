# MVP 与施工计划

## 1. 目标与范围

第一版必须证明下面这条完整链路，而不是先做一堆 Telegram 命令：

> Telegram 自然语言 → 固定 Chief 会话 → Chief 调用控制 MCP → Host 创建普通 worker →
> worker 在 ACP Agent 中执行 → 审批 / 状态回到手机 → 最终结果回到 Chief 和本地 UI。

第一版范围：

- 单机、单 owner、单 Telegram 私聊；
- 一个持久 Chief 会话；
- 用户可配置 Chief Agent / 模型 / Provider 绑定；
- Chief 可在允许项目里创建普通 worker；
- 默认最多 1 个 active worker，配置上限 3；
- 任务 start / send / status / cancel / collect / resume；
- worker 权限 / ask / plan 复用现有审批卡；
- 主窗隐藏、最小化、应用重启后的任务恢复；
- worker 在本地 Marionette 中可见；
- 任务、事件、审批有本地持久化和审计；
- 自我修改只在允许仓库和独立 worktree 中进行，发布仍本地确认。

第一版不做：

- 群聊多 Bot；
- Bot 市场、人格、长期记忆仓库同步；
- worker 再创建 worker；
- 自动 push / release / deploy；
- 远程新增项目根目录；
- 远程新建整屏 Computer Use 共享；
- 多 IM 同时在线；
- Council 作为 Chief 子任务；
- 语音和大附件工作流。

## 2. M0：可行性闸门

M0 不是写 UI，而是证明技术底座不会在无人值守时失效。M0 失败时不得进入 M1。

### 2.1 必做探针

1. **后台 ACP 驱动**
   - 从 Rust 内部启动一个普通 ACP session；
   - 发送 prompt；
   - 收到 agent message / tool / permission / complete；
   - 停止与恢复；
   - 在主窗隐藏和最小化时持续工作 30 分钟。

2. **多 Agent 认证矩阵**
   - 至少验证 OpenCode、Codex、Claude Code；
   - 有能力时验证 Grok、ZCode / DSH；
   - 检查每个 Agent 是否支持：
     - localhost HTTP MCP；
     - model / permission options；
     - 长任务；
     - cancel；
     - 历史恢复；
     - 在 backend 无 Composer 状态下发送 prompt。
   - 对没通过的 Agent 标 `experimental` 或不进入首发可选列表。

3. **durable event tee**
   - 在 ACP 统一事件出口把 Chief / worker 事件写 JSONL；
   - 应用重启或 WebView 恢复后能回放大致完整的工具 / 回复视图；
   - UI 不因回放产生重复卡片。

4. **动态 MCP 能力**
   - `tools/list` 能按 `sessionId` 返回不同工具；
   - Chief 看到控制工具；
   - 普通 worker 看不到控制工具；
   - 跨 Agent 均能正确处理拒绝返回值。

5. **Telegram 后台**
   - 主窗隐藏、最小化、贴边时均能接收文本与 callback；
   - 长轮询断线 30 分钟再恢复；
   - 重复 `update_id` 不重复创建任务。

6. **最小权限探针**
   - Chief 尝试请求未授权项目、cwd、改权限，Host 全部拒绝；
   - 普通 worker 无法调用 `marionette_task_start`。

### 2.2 M0 退出标准

- 至少一个 ACP Agent 在 WebView 暂停时完整跑完一次远程 prompt；
- 至少两个 Agent 确认 localhost MCP 工具可用；
- 事件可在重启后恢复；
- 未授权工具和项目访问 fail-closed；
- 有明确证据说明不需要前端 `performSend` 来驱动 Chief worker。

没有达到这些条件就先修基础设施，不以 Telegram demo 掩盖问题。

## 3. M1：单 worker 垂直切片

### 3.1 用户流程

1. 用户在 Marionette 配置：
   - 启用 Chief；
   - 选择 Chief Agent / 模型；
   - 选择允许项目；
   - 选择默认 worker Agent / 模型；
   - 设为 read-only 或 approve-writes；
   - 确认 Telegram 配对。
2. 手机发：

   > 帮我看一下 Marionette 的构建失败，找到原因；如果可以修，就修掉并跑测试。别发布。

3. Host 将消息送入 Chief。
4. Chief 调用 `marionette_projects_list` 与 `marionette_agents_list`。
5. Chief 调用 `marionette_task_start`。
6. Telegram 收到“任务已开始”卡片。
7. worker 需要写文件时发权限卡；用户在手机上允许一次。
8. worker 完成，Host 记录 worker session、diff、测试与最终正文。
9. Chief 被 completed event 唤醒，生成简洁交付。
10. 用户回“把 diff 贴出来”，Chief 调用 `task_collect` 或直接汇总。

### 3.2 配置面

建议新增一个最小“远程助理”区块：

- 当前模式：off / relay / chief；
- Chief 状态与本地打开按钮；
- Chief Agent / 模型 / Provider 绑定；
- Telegram 配对与 owner；
- 允许项目列表；
- 每个项目的：
  - 默认 worker Agent；
  - 默认模型；
  - read-only / approve-writes；
  - 最大并发；
  - 最大任务时长；
  - 是否允许 Browser / Computer Use；
  - 是否为自我修改仓库；
- “测试 Chief”按钮。

不要在第一版做完整 Bot 编辑器、人格卡或角色市场。

### 3.3 本地 UI

至少需要：

- Chief 会话出现在聊天列表，`origin = "chief"`；
- Chief 发起的 worker 是顶层普通会话，`origin = "chief-worker"`；
- 能从一个 task 跳到对应 worker；
- 能看 task 状态、当前 Agent、最后摘要、等待事项；
- 可手动停止 / 接管 / 打开 worker；
- 不重做一套 IDE 面板；优先复用现有 transcript、diff、permission、usage 组件。

任务页可以是侧栏里的一个小型“远程任务”区，后续再升级为完整体。

## 4. M2：可靠性与有限并发

M2 把“能跑一次”变成“离开电脑也敢用”：

- 可配置 1–3 个 worker；
- 同一任务事件合并，避免每个 token 都唤醒 Chief；
- completed / failed / interrupted 自动唤醒；
- 任务心跳和 stale 检测；
- Telegram 断线 / 应用重启恢复；
- 任务附件、最终文件、diff 和测试摘要；
- 远程消息排队 / steer；
- provider 限流、余额不足、认证失效的明确分类；
- task cost / token / wall-clock 预算；
- 本地审计页或任务详情中的审计摘要；
- 任务结束后自动挂起空闲 worker，但保留会话和关系；
- 清理和保留策略。

### 4.1 并发语义

- 任务并发与 Agent 进程并发分开计数；
- 一个 worker 只能有一个 active task；
- Chief 本身不算 worker 并发；
- 达到上限时 `task_start` 返回 `queued` 或明确拒绝，不能悄悄丢弃；
- 默认总成本达到上限时停止新任务，不停止已有 read-only 任务；
- 全局急停优先于所有队列和预算逻辑。

## 5. M3：扩展能力

按用户需求再开：

- Browser 远程任务完整流程；
- 已共享窗口的 Computer Use；
- 从手机发起“请求新共享”但本地确认；
- Council 作为任务类型；
- 定时任务 / morning brief；
- Telegram topics 或任务 thread；
- 多 IM；
- 多 Chief（工作 / 生活 / 写作）但每个都是独立 route，不做一个人格市场；
- worker 之间的受控 handoff；
- 可选 sandbox / worktree profile。

任何一项都不能绕过 Host policy 和审批矩阵。

## 6. 控制工具定义

### 6.1 最小 schema

`marionette_task_start`：

`````````json
{
  "type": "object",
  "required": ["requestId", "title", "goal", "projectId"],
  "properties": {
    "requestId": {"type": "string"},
    "title": {"type": "string", "maxLength": 120},
    "goal": {"type": "string", "maxLength": 20000},
    "projectId": {"type": "string"},
    "agentId": {"type": "string"},
    "model": {"type": "string"},
    "mode": {"type": "string"},
    "maxRuntimeMinutes": {"type": "integer", "minimum": 1, "maximum": 480}
  }
}
`````````

`marionette_task_send`：

`````````json
{
  "type": "object",
  "required": ["commandId", "taskId", "message"],
  "properties": {
    "commandId": {"type": "string"},
    "taskId": {"type": "string"},
    "message": {"type": "string", "maxLength": 10000},
    "urgency": {"enum": ["normal", "interrupt"]}
  }
}
`````````

`marionette_task_collect`：

`````````json
{
  "type": "object",
  "required": ["taskId"],
  "properties": {
    "taskId": {"type": "string"},
    "includeDiff": {"type": "boolean"},
    "includeTestOutput": {"type": "boolean"},
    "artifactLimit": {"type": "integer", "minimum": 0, "maximum": 20}
  }
}
`````````

### 6.2 明确不在 schema 的字段

不要提供：

- `cwd`；
- `permissionMode` / `alwaysApprove`；
- `mcpServers`；
- `providerApiKey`；
- `env`；
- `shellCommand`；
- `release` / `publish` / `push`；
- `addProjectRoot`；
- `chiefPolicy`。

Chief 的可用能力通过 Host 配置决定，不能靠调用参数扩展。

### 6.3 返回格式

建议统一 envelope：

`````````json
{
  "ok": true,
  "task": {},
  "error": null,
  "note": null
}
`````````

可操作拒绝：

`````````json
{
  "ok": false,
  "error": "project_not_allowed",
  "note": "Marionette 当前未授权给远程任务。请让用户在本地选择允许项目。"
}
`````````

拒绝是工具结果而不是 MCP 协议错误，保留 Chief 继续向用户解释的能力。

## 7. 建议施工顺序

### PR 1：后端 ACP runner

- 从 `commands.rs` 抽出可无 UI 调用的 start / send / cancel / config 内核；
- 保持现有 Tauri command 行为不变；
- 添加 durable event tee 的接口；
- 加主窗隐藏 / 最小化集成测试。

### PR 2：Task store + broker

- 新增 `src-tauri/src/chief/{types,store,broker}.rs`；
- schema version；
- 原子 tasks 快照、append-only events；
- requestId / commandId 幂等；
- 状态机单测。

### PR 3：session-aware MCP

- `tools_mcp.rs` 改为按 session 返回工具；
- Chief token → 控制工具；
- worker token → 普通 Browser / Computer / 插件工具；
- 未授权会话直接拒绝；
- 加 schema / dispatch 单测。

### PR 4：Telegram Chief mode

- Telegram settings 增 `remoteWorkMode`；
- 固定 chat → Chief；
- 持久 `update_id`；
- 入站消息送 backend Chief；
- 任务状态卡与 messageId→taskId 路由；
- 与 relay 模式二选一，禁止重复消费。

### PR 5：审批与结果交付

- worker permission / ask / plan 复用现有 waiter；
- 任务卡显示可读影响；
- completed / failed 事件进入任务 store；
- Telegram rich message 或分段文本交付；
- 本地 UI 可见 worker 与 task。

### PR 6：恢复、安全与 soak

- 启动恢复；
- stale / heartbeat；
- 全局急停；
- 路径 / prompt injection / unauthorized 测试；
- 主窗隐藏、最小化、重启、网络断线；
- 8 小时 soak。

### PR 7：文档和发布

- 实现后再更新 `docs/CURRENT.md`；
- 增加用户配置指南；
- 更新 Telegram 桥文档说明 relay / chief 差异；
- 增加故障排查标签 `[chief]`、`[chief-tools]`、`[chief-telegram]`。

## 8. 代码影响面

| 位置 | 预计改动 | 风险 |
|---|---|---|
| `src-tauri/src/acp.rs` | runner 内核、durable event hook、后台会话状态 | 高：不能破坏现有 UI 流 |
| `src-tauri/src/commands.rs` | 提为可复用函数，保持 command 薄壳 | 中 |
| `src-tauri/src/tools_mcp.rs` | session-aware tools、控制工具 dispatch | 高：安全边界 |
| `src-tauri/src/telegram_remote.rs` | Chief 模式、offset、聚合、审批 | 高：现有桥不可回归 |
| `src-tauri/src/chief/*` | 新 broker / store / runner / policy | 核心新增 |
| `src-tauri/src/models.rs` | 可能新增 origin / task metadata，或尽量不改 Session | 中：兼容旧数据 |
| `src-tauri/src/storage.rs` | Chief / worker 关系、恢复、清理 | 中 |
| `src/app/App.tsx` / sidebars | Chief 入口、任务视图、worker 跳转 | 中 |
| `src/lib/api.ts` | 新命令 / 事件 / DTO | 低 |
| `docs/*` | 实现后写 CURRENT 与用户指南 | 低 |

建议尽量把 task relation 放 `chief/tasks.json`，不要在 Session 上堆十几个专用字段。
Session 只保留通用的 `origin` 与必要状态；关系与任务生命周期由 Chief 自己的 store 管。

## 9. 测试矩阵

| # | 场景 | 预期 |
|---|---|---|
| 1 | 自然语言“修最近构建问题” | Chief 选对唯一项目并创建单 worker |
| 2 | 两个相似项目 | Chief 问一个明确问题，不瞎选 |
| 3 | 用户指定 Agent / 模型 | Host 校验可用后使用 |
| 4 | Agent 不存在 / 未登录 | task_start 拒绝并给可理解原因 |
| 5 | worker 要求 shell 写文件 | Telegram 与本地同时出现权限卡，先答先得 |
| 6 | 重复点允许按钮 | 只生效一次 |
| 7 | 用户说“停止” | 当前 worker 取消，state=cancelled |
| 8 | 全局急停 | 新任务拒绝，所有 worker / browser / computer 停止 |
| 9 | 主窗最小化 2 小时 | 任务继续，Telegram 可查询 |
| 10 | 主窗隐藏 / WebView suspend | backend runner 不受影响 |
| 11 | 应用重启 | running 失联任务变 interrupted，不标 completed |
| 12 | Telegram 断线重连 | 不重复 task，重复 update 被忽略 |
| 13 | task_start 重试同一 requestId | 返回同一 taskId |
| 14 | 两个 worker 并行且未超过上限 | 状态不串台 |
| 15 | 超过并发上限 | 明确 queued / rejected |
| 16 | worker 输出恶意“扩大权限” | Chief 调用仍被 Host 拒绝 |
| 17 | 网页包含 prompt injection | 不得直接产生项目外或发布动作 |
| 18 | 请求项目外路径 | canonical path 校验拒绝 |
| 19 | Computer Use 未共享窗口 | 返回 grant_required，不自动接管 |
| 20 | 自我修改任务 | 只写独立 worktree；不 push / release / 重启 |
| 21 | provider 429 / 401 / 断网 | 分类失败，可 resume，不伪装完成 |
| 22 | 长回复 / diff / 文件附件 | 分片或文件交付，手机不截断到无信息 |
| 23 | Chief Agent 不可用 | Host 仍发 worker 完成状态，用户不丢结果 |
| 24 | 未配对 user / chat | 静默拒绝 + 审计，不泄漏项目名 |

## 10. 验收指标

第一版以可通过的 journey 和故障恢复为主，不拿模型“看起来很聪明”当验收。

### 功能

- 24/24 测试矩阵通过；
- 自然语言核心路径不需要任何 slash command；
- 至少 3 个目标 Agent 完成 start / permission / complete / cancel；
- 本地 UI 能看到所有 Chief worker；
- 任务最终结果包含 changed files、验证结果和剩余风险中的至少两项。

### 可靠性

- 8 小时 soak 无丢任务、无重复 task、无 worker 状态串台；
- 主窗隐藏 / 最小化期间至少完成 10 个任务；
- 网络断开 30 分钟可恢复；
- 应用重启后所有 task 有确定状态；
- stale 检测在配置时限内触发。

### 安全

- 0 个未授权项目被访问；
- 0 个 remote R6 操作成功；
- 0 个重复审批产生二次执行；
- 0 个 token / key 进入日志或 Telegram；
- 自我修改任务 100% 走 worktree 且不自动发布。

### 体验

- 普通任务默认通知不超过：
  - 开始 1 条；
  - 审批按需；
  - 完成 1 条；
  - Chief 可合并成 1 条最终交付。
- 用户无需知道 session id、MCP 名称或 Agent 启动命令。
- 本地 / 手机看到同一个任务的事实状态。

## 11. 发布策略

- 先在开发版 / 实验开关开放；
- 只给一个测试项目启用；
- 记录本地 `[chief]` 日志；
- 观察至少一周的真实 dogfood（包括远程修本仓）；
- 再开放多项目、多 worker；
- Browser / Computer Use 单独开关；
- 公开版本默认 `remoteWorkMode=off`；
- 发布说明明确“电脑必须开机、Marionette 必须运行、Windows 睡眠会暂停”。

## 12. 开放问题

需要用户或实现前 spike 拍板：

1. **Chief 默认 Agent**：第一版推荐哪种作为官方配置？可能是 Codex / Claude / OpenCode，而不是所有 Agent 同权。
2. **远程审批超时**：现有 120 秒是否延长到 5–10 分钟？超时后是 failed 还是等待恢复？
3. **Chief event wake 频率**：只在完成 / 失败 / 需人时唤醒，还是允许阶段性主动汇报？
4. **worker 在侧栏如何分组**：平铺加 origin，还是新“Chief 任务”分组？不要复用 `@` child 隐藏语义。
5. **附件**：第一版是否支持 Telegram 图片 / 文件进入 worker？建议 M2，而不是 M1。
6. **自动启动**：Chief 开启时是否建议 Marionette 开机启动 / 常驻 tray？这是用户偏好，不默认打开。
7. **成本预算**：按 provider 真实金额、估算 token 还是任务时长？建议先用现有 usage 估算并只做软上限。
8. **多模型**：Chief 是否可在一个任务里自动换 provider？建议第一版不允许中途切换，避免上下文漂移。
9. **Computer Use**：第一版只操作本地已共享窗口，还是支持手机上请求共享？推荐前者。

## 13. Definition of Done

以下全部满足，才算“Marionette Chief MVP 完成”：

- 用户本地配置自己的 Chief Agent / 模型 / Provider；
- Telegram 固定进 Chief，自然语言完成远程代码任务；
- Chief 通过 Host 工具创建一个普通可见 worker；
- worker 权限、ask、plan 可以从手机和本地 UI 任一处解决；
- 任务 store、event、审批、worker transcript 可恢复；
- 主窗隐藏 / 最小化时 backend runner 继续工作；
- 应用重启、Telegram 重连、provider 失败都有确定状态；
- 项目白名单、远程能力分级和急停通过安全测试；
- 自我修改只产出 worktree / diff / tests，不自动发布；
- 文档更新完成，默认开关仍为关闭。
