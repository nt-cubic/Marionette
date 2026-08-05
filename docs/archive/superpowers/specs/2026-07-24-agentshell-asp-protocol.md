# ASP — AgentShell Protocol

> 日期：2026-07-24  
> 状态：草案 v0（已命名，可执行）  
> 版本：`asp/0`

---

## 0. 一句话定义

**ASP（AgentShell Protocol）** 是 AgentShell 的**内部会话协议**。  
UI 与 Session Runtime 只讲 ASP；ACP、PTY 等是 Backend 方言，由适配器翻译。

> ASP 不是行业 Agent 标准。  
> 不要求 OpenCode / Claude / Codex / Grok 实现 ASP。  
> 第三方 Agent 继续用它们自己的 CLI/TUI 或 ACP；由 AgentShell Backend 适配进 ASP。

---

## 1. 为什么需要 ASP

### 1.1 问题

- 只绑 ACP：能力参差（无模型列表、无强度、不能中断），`/connect` 等 TUI 运维命令常常不可用。
- 只绑 PTY：能跑任何 CLI，但统一控件与结构化 Clean View 很脆。
- 产品真正需要的是**稳定的壳语义**，而不是把某一种传输方式抬成产品本身。

### 1.2 分层

```text
┌────────────────────────────────────────────┐
│  UI：Composer / Clean / Raw / Shelf        │
│  只认识 ASP 命令、事件、Capabilities        │
└─────────────────────┬──────────────────────┘
                      │  ASP (asp/0)
                      ▼
┌────────────────────────────────────────────┐
│  Session Runtime                           │
│  路由 · 能力合并 · handoff · 日志 · 状态    │
└─────────────────────┬──────────────────────┘
                      │
        ┌─────────────┼─────────────┐
        ▼             ▼             ▼
   AcpBackend    PtyBackend    (future)
   讲 ACP        讲字节/PTY    Marker 等
```

| 层 | 角色 |
|----|------|
| **ASP** | 产品语义：session、发送、能力、handoff、错误 |
| **ACP** | 可选 Backend：结构化 prompt / config / cancel |
| **PTY** | 默认/兜底 Backend：真终端、Raw 权威源 |

---

## 2. 设计原则

1. **Raw 是权威源**  
   每个 session 必须能提供 `raw.chunk`（或等价字节流投影）。Clean View 可以失败，Raw 不能失败。

2. **能力驱动 UI（诚实降级）**  
   只展示 Capabilities 声明为可用的控件。  
   **禁止假控件**（点了没效果的 model/mode/effort/cancel）。

3. **Unsupported 是一等结果**  
   `session.cancel` / `session.set_option` 可以返回 `unsupported`，不得静默吞掉。

4. **跨 Agent 上下文 ≠ 共享内存**  
   切 Agent 不保证同一对话线程。  
   v0 用 **Handoff 文件 + 用户确认发送** 近似传递上下文。

5. **传输可替换**  
   UI 不直接依赖 ACP 类型或 xterm 细节；只依赖 ASP。

6. **Windows 优先**  
   与产品范围一致；协议本身不绑定 OS，实现阶段先 Windows。

---

## 3. 标识与版本

| 字段 | 值 |
|------|-----|
| 名称 | AgentShell Protocol |
| 缩写 | ASP |
| 版本字符串 | `asp/0` |
| 稳定性 | 内部契约；可 breaking，改版本号 |

序列化建议（实现期）：

- 前端 / 后端共享 TypeScript / Rust 类型，名称与下文一致。
- Tauri command 可继续用现有命名，但语义应对齐 ASP（可渐进迁移）。
- 事件 payload 建议带 `protocol: "asp/0"`（可选，便于调试）。

---

## 4. 核心对象

### 4.1 SessionId

AgentShell 侧的 session 主键（UI tab / 持久化用）。  
与 ACP 的 `sessionId`、PTY 进程 id **不是同一个东西**；Backend 内部自己映射。

### 4.2 Transport

```text
acp | pty
```

v0 仅这两种。未来可加 `marker` 等，但不进入 asp/0 必选。

### 4.3 SessionStatus

```text
starting | running | waiting | exited | error
```

| 状态 | 含义 |
|------|------|
| `starting` | 进程/协议握手中 |
| `running` | 可交互，或正在流式输出 |
| `waiting` | 空闲等待用户输入 |
| `exited` | 正常或用户停止后结束 |
| `error` | 启动失败或不可恢复错误 |

### 4.4 Capabilities（能力卡片）

Session 就绪后，Runtime 必须给出一份 Capabilities（可后续 `capabilities.updated` 刷新）。

```typescript
type AspCapabilities = {
  protocol: "asp/0"
  transport: "acp" | "pty"

  /** 结构化模型列表；null/空 = UI 不展示模型选择 */
  models: { id: string; label: string }[] | null
  currentModel: string | null

  /** 模式列表；null/空 = 不展示 */
  modes: { id: string; label: string }[] | null
  currentMode: string | null

  /** 思考强度；null = 不展示 */
  effort: { min: number; max: number; default: number } | null
  currentEffort: number | null

  /** 优雅中断当前 turn（≠ 杀进程） */
  cancel: boolean

  /**
   * slash 策略：
   * - advertised: Backend 提供命令列表（如 ACP available_commands）
   * - passthrough: 原样送入 Agent（典型 PTY TUI）
   * - none: 不宣称 slash 能力
   */
  slash: "advertised" | "passthrough" | "none"
  slashCommands: { name: string; description?: string }[] | null
}
```

**UI 规则（规范，必须遵守）：**

| Capabilities | Composer |
|--------------|----------|
| `models` 有效且长度 ≥ 1 | 显示模型选择 |
| 否则 | **隐藏**模型选择 |
| `modes` 有效且长度 ≥ 1 | 显示 mode |
| 否则 | **隐藏** mode |
| `effort` 非 null | 显示强度 |
| 否则 | **隐藏**强度 |
| `cancel === true` | 显示中断按钮（语义：cancel turn） |
| `cancel === false` | 禁用或隐藏；可另提供「停止 session」（杀进程），文案必须区分 |
| `slash === advertised` 且列表非空 | `/` 补全用该列表 |
| `slash === passthrough` | `/` 当普通文本发送；补全可不做或仅本地提示 |
| `slash === none` | 不宣称 slash |

---

## 5. 命令（Client → Runtime）

v0 命令集刻意小。未列出的能力不要假装存在。

### 5.1 `session.start`

```typescript
type SessionStart = {
  method: "session.start"
  params: {
    sessionId: string
    projectId: string
    agentId: string
    cwd: string
    /** 来自 AgentConfig；Runtime 选 Backend */
    transport: "acp" | "pty"
    command: string
    args: string[]
    sendStrategy?: "stdin" | "bracketed-paste"
  }
}
```

**结果：**

- 成功：进入 `starting` → 随后 `capabilities.updated` + `status.changed`
- 失败：`status.changed` → `error`，并带 `error` 事件

### 5.2 `session.send`

```typescript
type SessionSend = {
  method: "session.send"
  params: {
    sessionId: string
    text: string
    /** 默认 true：记为 user_message */
    recordAsUserMessage?: boolean
  }
}
```

**语义：**

- **AcpBackend**：映射为 ACP `session/prompt`（或等价）
- **PtyBackend**：按 `sendStrategy` 写入 PTY（stdin 或 bracketed paste）
- 以 `/` 开头的文本：**不在 ASP 层特殊解析为壳命令**（壳命令见 §8）；默认整段交给 Backend

### 5.3 `session.cancel`

```typescript
type SessionCancel = {
  method: "session.cancel"
  params: { sessionId: string }
}
```

**结果：**

- `ok`：已请求中断当前 turn
- `unsupported`：Capabilities.cancel === false
- `error`：调用失败

**禁止**把 `session.stop`（杀进程）伪装成 `session.cancel` 的成功。

### 5.4 `session.set_option`

```typescript
type SessionSetOption = {
  method: "session.set_option"
  params: {
    sessionId: string
    key: "model" | "mode" | "effort" | string
    value: string | number | boolean
  }
}
```

**结果：**

- `ok` + 可选 `capabilities.updated`
- `unsupported`：当前 Backend/Agent 无此选项
- `error`

UI 只有在 Capabilities 显示该选项存在时才应发起此命令。

### 5.5 `session.resize`

```typescript
type SessionResize = {
  method: "session.resize"
  params: { sessionId: string; cols: number; rows: number }
}
```

PTY 必须支持；ACP 可 no-op 或忽略。

### 5.6 `session.stop`

```typescript
type SessionStop = {
  method: "session.stop"
  params: { sessionId: string }
}
```

结束 session：停进程、释资源、状态 → `exited`。

### 5.7 `handoff.prepare`

```typescript
type HandoffPrepare = {
  method: "handoff.prepare"
  params: {
    projectId: string
    sourceSessionId?: string
    targetAgentId: string
    userNotes?: string
    pinnedPaths?: string[]
  }
}
```

**结果：**

```typescript
type HandoffResult = {
  projectId: string
  targetAgentId: string
  handoffPath: string   // 通常 {project}/.agentshell/handoff.md
  prompt: string        // 供 Composer prefill
  createdAt: string
}
```

**规范：**

- 只生成文件 + 返回 prompt  
- **默认不自动 `session.send`**  
- 用户编辑并确认后，再 `session.send` 到目标 session

---

## 6. 事件（Runtime → Client）

所有事件带 `sessionId`（handoff 类可带 projectId）与 `createdAt`（ISO-8601）。

### 6.1 `status.changed`

```typescript
{ type: "status.changed"; sessionId: string; status: SessionStatus; message?: string }
```

### 6.2 `capabilities.updated`

```typescript
{ type: "capabilities.updated"; sessionId: string; capabilities: AspCapabilities }
```

### 6.3 `raw.chunk`

```typescript
{ type: "raw.chunk"; sessionId: string; text: string; exited?: boolean }
```

- PTY：终端输出  
- ACP：可将 stderr / 调试流 / 可选 transcript 投影到 raw（实现自定，但 Raw 视图要有东西可看或明确「本 transport 无 TUI raw」）

### 6.4 `message.user` / `message.assistant`

```typescript
{ type: "message.user"; sessionId: string; text: string }
{ type: "message.assistant"; sessionId: string; text: string }
```

有结构化消息时发；PTY 可仅有 raw，没有这两类。

### 6.5 `tool.*`（v0 可选）

```typescript
{ type: "tool.started"; sessionId: string; toolId: string; name: string; input?: unknown }
{ type: "tool.completed"; sessionId: string; toolId: string; ok: boolean; output?: unknown }
```

无则不做。禁止用假 tool 卡片填充。

### 6.6 `permission.request`（v0 可选）

```typescript
{
  type: "permission.request"
  sessionId: string
  requestId: string
  title: string
  detail?: string
  options: { id: string; label: string }[]
}
```

Client 以 UI 对话框响应（另定 `permission.respond` 命令，v0.1 可补）。  
v0 若 Backend 自动 allow，必须在文档/UI 标明，并尽快改为显式确认。

### 6.7 `handoff.prepared`

```typescript
{
  type: "handoff.prepared"
  sessionId?: string
  projectId: string
  targetAgentId: string
  handoffPath: string
  prompt: string
}
```

### 6.8 `error`

```typescript
{
  type: "error"
  sessionId?: string
  code: string
  message: string
  retriable?: boolean
}
```

建议 `code` 稳定可分支，例如：`spawn_failed` | `command_missing` | `acp_handshake_failed` | `unsupported` | `permission_timeout`。

---

## 7. Backend 契约

### 7.1 共同接口（逻辑）

每个 Backend 必须实现（或明确 unsupported）：

| ASP | AcpBackend | PtyBackend |
|-----|------------|------------|
| start | 启进程 + initialize + session/new | 启 PTY + command/args |
| send | session/prompt | write stdin / bracketed paste |
| cancel | session/cancel 或 unsupported | 通常 unsupported（或启发式 Ctrl+C，须标明实验性） |
| set_option | session/set_config_option | 通常 unsupported |
| resize | 可选 | 必须 |
| stop | 杀进程 | 杀进程 |
| capabilities | 从 initialize/session 解析 | 保守默认：无 models/modes/effort，cancel=false，slash=passthrough |
| raw | 尽力投影 | 主路径 |

### 7.2 默认 Agent 矩阵（asp/0 产品约定）

| Agent | 默认 transport | 说明 |
|-------|----------------|------|
| OpenCode | `acp` | 原生 ACP；控件跟 Capabilities |
| Codex CLI | `pty` | 先真 TUI；ACP adapter 不作为默认承诺 |
| Claude Code | `pty` | 同上 |
| Grok Build | `pty` | 无 ACP |
| Custom | `pty` | 用户配置 command/args |

配置里可强制覆盖 `transport`，但 UI 仍只信运行时 Capabilities。

### 7.3 PTY 启动（对实现的硬要求）

- **禁止**写死仅启动 `powershell.exe` 当作 Agent session。  
- `session.start` 必须使用 AgentConfig 的 `command` + `args` + `cwd`。  
- PowerShell 仅可作为「调试用空终端」或显式配置的 shell Agent，不能冒充 Grok/Claude。

---

## 8. Slash 策略（`/`）

### 8.1 事实

- 各 CLI/TUI 的 `/` 是**私货**，不是统一协议。  
- ACP 可有 `available_commands_update`，但不保证等于完整 TUI（例如 OpenCode `/connect` 类交互向导）。  
- 在 ACP chat 里发 `/connect` **不等于** 原生 TUI 的 `/connect`。

### 8.2 ASP 规则

| transport / slash | 行为 |
|-------------------|------|
| PTY + `passthrough` | Composer 文本（含 `/xxx`）原样进 stdin；完整交互以 **Raw Terminal** 为准 |
| ACP + `advertised` | 用 Agent 广告的列表做补全；发送走 `session.send`；交互式运维命令须降级提示 |
| 任意 + 已知运维命令 | UI 可提示：「请在 Raw Terminal 完成，或使用原厂 CLI（如 `opencode auth login`）」 |

### 8.3 壳自有命令（可选，避免与原厂冲突）

若需要壳级命令，使用 **双斜杠** 或其它前缀，例如：

- `//handoff` — 触发 `handoff.prepare`
- `//raw` — 切换 Raw 视图  
- `//stop` — `session.stop`

单斜杠 `/` 默认留给当前 Agent，ASP 不抢。

---

## 9. Handoff 语义（跨 Agent 上下文）

### 9.1 承诺（对用户可说）

> 切换 Agent 时，AgentShell 会生成一份可审的交接说明（handoff），预填到输入框；你确认后发送。  
> **不会**自动把完整对话线程无缝迁移到另一个 Agent。

### 9.2 不承诺

- 协议级共享 session memory  
- 跨 Agent 自动续聊  
- 统一 model/effort 状态迁移  

### 9.3 文件

- 路径：`{projectRoot}/.agentshell/handoff.md`
- 内容建议段落：目标、来源 Agent/session、用户 notes、pinned files、git/changed files 摘要、recent raw/transcript tail  

---

## 10. 视图与 ASP

| 视图 | 数据来源 |
|------|----------|
| Raw Terminal | `raw.chunk` + 本地 xterm；输入可直通 Backend write（PTY） |
| Clean View | `message.*` / `tool.*` / 粗解析的 raw 块；无数据时 placeholder，不编造 |
| Composer | Capabilities + `session.send` / `set_option` / `cancel` |
| Diff / Logs | 后续；不进 asp/0 必选 |

**Clean ↔ Raw 切换不得重启 session。**

---

## 11. 错误与降级

| 场景 | 行为 |
|------|------|
| command 不在 PATH | `error` + status=error，提示安装/配置 |
| ACP 握手失败 | error；可提示「改用 PTY」若配置允许 |
| set_option unsupported | 返回 unsupported；UI 本不应显示该控件 |
| cancel unsupported | 禁用中断；可提供 stop session |
| parser/Clean 崩溃 | 不影响 Raw 与进程 |
| 权限（未来） | 显式 UI；避免长期静默 auto-allow |

---

## 12. 与现有代码的映射（迁移指引）

| 现有 | ASP |
|------|-----|
| `start_terminal` / `start_acp_session` | `session.start` + transport |
| `write_terminal` / ACP prompt | `session.send` |
| `stop_terminal` / `stop_acp_session` | `session.stop` |
| ACP `session/cancel` | `session.cancel` |
| `update_acp_session` / set_config_option | `session.set_option` |
| `CapabilitySnapshot` | `AspCapabilities`（扩展 transport/slash） |
| `session-output` 事件 | `raw.chunk` |
| `acp-event` | Runtime 内部；UI 逐步只订 ASP 事件 |
| 未来 `generate_handoff` | `handoff.prepare` |

迁移策略：**语义先对齐，API 名可渐进**。不必一天改完所有 Tauri command 名。

---

## 13. asp/0 非目标

1. 要求第三方实现 ASP  
2. 统一所有 Agent 的 model/mode/effort UI 承诺  
3. 协议级跨 Agent 无缝上下文  
4. 自研 Agent runtime / 模型路由 / API key 平台  
5. 完美 TUI screen scraping  
6. 自动发送 handoff  

---

## 14. 实现顺序（建议）

1. **文档与类型**：`AspCapabilities`、事件枚举进 `types`（可先并行旧类型）  
2. **PtyBackend 校正**：`command + args` 真启动 Agent  
3. **Composer 诚实降级**：严格按 Capabilities 显隐  
4. **Runtime 门面**：UI 只调一小组 session.* API（内部再分 acp/pty）  
5. **handoff.prepare**  
6. **OpenCode ACP 增强**：权限 UI、Clean 结构化  
7. slash 补全（advertised）与运维降级文案  

---

## 15. 验收标准（asp/0）

1. UI 代码路径不直接分支「如果是 OpenCode 就显示假模型列表」。  
2. Grok/Claude 等 PTY Agent：能启动**真实 CLI**，Composer 无假 model/effort。  
3. OpenCode ACP：Capabilities 来自协商；无 cancel 则中断不可点。  
4. Raw 与 Clean 切换不杀进程。  
5. handoff 生成后需用户确认才发送。  
6. `/connect` 类问题：产品说明指向 Raw/原厂 CLI，不假装 ACP Composer 内可完成。  

---

## 16. 未决（不阻塞 asp/0）

1. Tauri command 是否一次性重命名为 `asp_*`  
2. PTY 是否实验性支持 Ctrl+C 作为 cancel  
3. ACP session 的 Raw 视图具体投影策略  
4. `permission.respond` 细节与默认策略  
5. 是否引入 MarkerBackend  

---

## 附录 A：命名对照

| 缩写 | 全称 | 关系 |
|------|------|------|
| **ASP** | AgentShell Protocol | 本文件；内部会话协议 |
| **ACP** | Agent Client Protocol | 行业/编辑器侧协议；一种 Backend |
| **PTY** | Pseudo Terminal | 传输/进程层；一种 Backend |

口头约定：

- 说「走 ASP」= 走壳的统一语义  
- 说「走 ACP / 走 PTY」= 当前 session 的 Backend transport  

---

## 附录 B：相关文档

- `docs/superpowers/specs/2026-07-24-agentshell-direction-design.md` — 方向讨论（混合架构）  
- `docs/01-product-plan.md` — 产品边界与 handoff  
- `docs/02-program-design.md` — Adapter / PTY 设计  
- `docs/04-opencode-ready-plan.md` — OpenCode 能力协商与真控件  
