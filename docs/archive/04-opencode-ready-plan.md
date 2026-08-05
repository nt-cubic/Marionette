# AgentShell OpenCode 可用化计划

## 0. 为什么是这个顺序

当前项目状态：M1-M4（PTY/Multi-Session）基本稳固，M5（ACP 传输）也通了。但用户实际体验与 Zed 的差距不在"能不能连接"，而在：

1. Composer 上的模型/模式/强度控件是假控件——点完了没任何效果。
2. Agent 请求权限时没有响应——Agent 会卡住。
3. 出错了只有模糊状态——用户不知道该重新登录还是重试。

**一个 Agent 打透，胜过四个 Agent 半残。** 先让 OpenCode 在 AgentShell 里真正能用，再扩展其他 Agent。

## 1. Phase A：能力协商（Composer "知道自己是谁"）

### 目标

Composer 不再显示假数据，而是如实反映当前 Agent 的实际能力。

### 用户可见变化

| 当前 | Phase A 之后 |
|------|-------------|
| 模型名显示 "Agent default" | 显示 OpenCode 实际返回的第一个模型名 |
| 强度选择 "Auto/Low/Medium/High" 始终显示 | OpenCode 不支持则隐藏 |
| "Build" 模式按钮固定显示 | 根据 OpenCode 返回的 modes 列表动态渲染 |
| 不可用的功能显示为灰色 | 不可用的功能直接隐藏 |

### 要改什么

#### 1.1 后端：保存 Agent 能力快照

**位置**：`src-tauri/src/acp.rs`

- `initialize` 响应中 Agent 会返回 `modes`、`configOptions`（模型列表、思考强度范围等） → 需要保存到 session 相关的数据结构
- `session/new` 响应中 Agent 可能返回 session 级别的配置覆盖 → 也需要保存
- 新增 `AgentCapabilitySnapshot` 结构体，包含：
  - `modes: Vec<ModeDef>`（id + label + description）
  - `models: Vec<ModelDef>`（id + label）
  - `thinkingEffort: { min, max, default } | null`
  - `supportsCancel: bool`
  - `sessionConfig: Value`（原始 session config，用于 session/update）

#### 1.2 后端：暴露能力给前端

- 新增 Tauri command：`get_session_capabilities(sessionId) -> CapabilitySnapshot | null`
- 前端在 session 切换时直接调用获取

#### 1.3 前端：Composer 动态渲染

- Composer 读取 `CapabilitySnapshot`
- 按以下规则渲染：
  - 如果有 `modes` 且长度 > 1 → 显示 mode 切换下拉
  - 如果有 `models` 且长度 > 1 → 显示 model 选择菜单
  - 如果有 `thinkingEffort` → 显示强度选择器（值来自 Agent 返回的范围，不是硬编码 Auto/Low/Medium/High）
  - 以上三项全部为空时 → Composer 最简模式（仅输入框 + 发送按钮）
- mode/model/effort 切换时，暂时存为 UI state，还未发送 session/update（Phase B 做）

### 验收标准

1. 启动 OpenCode session 后，Composer 显示的模型名与 OpenCode 实际可用模型一致
2. 如果 OpenCode 返回了 modes，Composer 出现 mode 切换下拉
3. 如果 OpenCode 不返回 thinkingEffort，强度选择器不出现
4. 点开 model 菜单看到的是实际模型名，不是硬编码数组

---

## 2. Phase B：真实切换（Composer 选择"真的生效"）

### 目标

用户在 Composer 做的每一次选择，都转化为真实的 ACP `session/update` 请求发给 Agent。

### 用户可见变化

- 切换 mode → Agent 收到 `session/update` → 下一轮请求使用新模式
- 切换 model → Agent 收到 `session/update` → 使用新模型
- 切换 effort → Agent 收到 `session/update` → 调整推理深度

### 要改什么

#### 2.1 后端：实现 session/update 发送

- 在 `acp.rs` 中新增 `update_session(sessionId, config: Value) -> Result`
- 构造 `session/update` 的 JSON-RPC 参数，包含 sessionId 和 config
- 如果 Agent 不支持更新（返回错误），前端要能捕获并回退

#### 2.2 前端：Composer 切换时触发更新

- mode 切换 → 调用 `update_session` 传入新的 `mode` 值
- model 切换 → 调用 `update_session` 传入新的 `model` 值
- effort 切换 → 调用 `update_session` 传入新的 `thinkingEffort` 值
- 所有切换都可以合并到一次 `session/update`，前端可以在值变化后将多个变更合并
- 失败时 UI 回滚到上一个有效值并显示 Toast 错误

### 验收标准

1. 切换 mode 后发送一条 prompt，Agent 响应风格明显不同（如 Build vs Plan）
2. 切换 model 后发送一条 prompt，确认回复来自新模型
3. 切换失败时 UI 回退且提示错误

---

## 3. Phase C：权限不走丢（Agent 请求→用户响应闭环）

### 目标

Agent 请求权限 / 读文件 / 写文件 / 执行终端命令时，用户能看到确认提示，选择允许或拒绝，Agent 不会卡死。

### 用户可见变化

- Agent 请求权限时 → Composer 上方出现确认卡片
- 用户点 "允许" → Agent 继续执行
- 用户点 "拒绝" → Agent 收到拒绝，跳过操作
- 不再出现"发了 prompt 后 Agent 沉默"的情况

### 要改什么

#### 3.1 后端：实现 agent→client 请求的响应机制

当前 `acp.rs` 的 `read_stdout` 中，当收到带有 `id` 和 `method` 的消息时，仅 `emit_event` 但从不回复。需要：

- 当收到 agent→client 请求时，不将它当作 response 处理（当前会误匹配到 `pending` map）
- 将这类请求存入一个 pending_requests 队列，等待前端响应
- 新增 Tauri command：`respond_agent_request(sessionId, requestId, response: Value)`
- 用户选择后，前端调用此 command，后端将 JSON-RPC response 写回 stdin

#### 3.2 前端：权限确认 UI

- 监听 ACP event，识别 `request` 类型的消息
- 判断是否是需要用户确认的类型（通常是 `tools/call` 等）
- 渲染确认卡片：显示请求内容、请求来源、Allow / Deny 按钮
- 用户选择后调用 `respond_agent_request`

### 验收标准

1. OpenCode 请求读取文件时出现确认提示
2. 点击 Allow 后 Agent 继续执行
3. 点击 Deny 后 Agent 跳过该操作
4. 不出现 Agent 静默等待响应的情况

---

## 4. Phase D：不静默失败（错误状态与恢复）

### 目标

任何错误都有清晰的状态显示，用户可以自主诊断。

### 用户可见变化

- ACP 连接失败 → 显示 "OpenCode ACP 无法连接" 和原因
- 认证失败 → 显示 "请在终端中运行 opencode login"
- 进程崩溃 → 显示 "进程已退出，点击重试" 按钮
- stderr 关键信息（认证错误）不淹没在日志里

### 要改什么

#### 4.1 后端：错误分类与提取

- 改进 stderr 读取：区分认证错误、命令不存在、权限拒绝
- session 退出时记录 exit code 和最后几行 stderr
- 提供 `get_session_error(sessionId) -> SessionError | null`

#### 4.2 前端：错误状态 UI

- session 状态栏显示具体错误原因
- 提供 "Retry"、"Open in Terminal" 等恢复动作
- 不可恢复错误（如 command not found）引导用户解决

### 验收标准

1. 删除/重命名 opencode 命令后启动 session，显示 "command not found"
2. 认证过期时显示 "需要重新登录" 提示
3. 进程崩溃后用户能点击重试

---

## 5. 执行顺序

```
Phase A (能力协商)    → 必须先做，Composer 基础
Phase B (真实切换)    → 依赖 A 完成后做
Phase C (权限闭环)    → 可与 A/B 并行（独立模块）
Phase D (错误处理)    → 独立，可穿插在任何阶段

建议顺序：
Phase A → Phase B (串联)
Phase C + Phase D (与上面两个并行捉)
```

## 6. 完成后用户能用 AgentShell 做什么

到 Phase B + C 完成后，用户可以在 AgentShell 中：

1. 创建 OpenCode session
2. 看到 OpenCode 实际支持的 model/mode/effort
3. 自由切换 mode（Build/Plan 等）
4. 切换模型
5. 发送 prompt，Agent 能正常回复
6. Agent 请求权限时确认/拒绝
7. 出错了知道原因和解决方法

这个闭环做完之后，距离 Zed 的 Agent 体验还有差距（Clean View、Handoff、session 恢复），但**核心交互已经可用了**。
