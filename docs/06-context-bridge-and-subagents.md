# Skill / MCP 打通 · 子代理分屏 · 多 Agent 桥接（调查报告）

> 状态：**调查完成，未实现**（对应需求 5 / 6 / 7）
> 日期：2026-07-25
> 依据：本仓库现有实现（`src-tauri/src/acp.rs`、`src/app/App.tsx` 等）、`agent-client-protocol-schema 1.4.0` 源码、本机各 Agent 的真实配置、以及社区已有做法。
> 底线不变：不做 MCP marketplace、不自研 Agent runtime、不把壳做成平台。

---

## 0. 一句话结论

这三件事**共用同一根管道**：ACP 的 `session/new.mcpServers`。

AgentShell 已经是每个 Agent 进程的父进程和唯一入口，只要在建会话时往里注入 MCP server，就同时解决了：

| 需求 | 靠什么实现 |
|----|----|
| 5 · skill/mcp 兜底 | 注入「项目 MCP 集合」+ 内置 `agentshell-skills` MCP server |
| 6 · 子代理分屏 | 事件加 `laneId`；真正能分屏的是 AgentShell 自己派生的子会话 |
| 7 · 多 Agent 互调 | 内置 `agentshell-bridge` MCP server，把「换个 Agent 干活」变成一个工具 |

建议顺序：**5 → 6(一半) → 7 → 6(另一半)**。

---

## 0.5 优先级 0：tool 卡片显示真实内容 ✅ 已完成（2026-07-25）

调查 6 的时候发现，「只看到一个 tool 块、不知道里面在干什么」**不是协议缺陷，是渲染层丢数据**。

真实报文（dev.log，session-1784860385219）：

```json
{"sessionUpdate":"tool_call_update","toolCallId":"call_00_5dPf…","status":"completed",
 "title":"README.md","locations":[{"path":"D:\\Myself\\AgentsShell\\README.md"}],
 "content":[{"type":"content","content":{"type":"text","text":"# AgentShell\n…整篇文件…"}}]}
```

而 `acpTranscript.ts` 当时只渲染 `title · status` + `rawInput` 前 200 字，`content[]` / `locations` / `rawOutput` 全丢。

已改为：

- `content[]` → 卡片正文（text 块原样、`diff` 折算成 `+N/-M 行`、`terminal` 显示 id、image/resource 占位）
- `locations[0].path` → 标题行下面显示在读/改哪个文件，折叠摘要里也带文件名
- `rawOutput` → 没有 content 时的兜底（对象转 JSON）
- 单卡上限 4000 字符，超出标 `… (+N more characters)`；`ClippedBody` 固定 220px 高度内滚动
- **合并语义**：ACP 的 update 是部分字段，一条只带 `status` 的 ping 不能把已有 title / path / 输出擦掉；`title`/`status` 不再在解析层伪造默认值（之前会把 `README.md` 改回 `tool`）

事件结构新增 `path` / `detail` / `input` 三个字段并落盘（旧 transcript 只有 `text`，向下兼容）。

**这一条同时把需求 6 的"内部 subagent"部分吃掉了大半**：Task 类工具往 `content` / `rawOutput` 里塞的东西现在都能看见，真正缺的只剩「独立的第二条对话流」，而那个必须靠需求 7 的子会话。

---

## 1. 现状事实（先把地基说清楚）

### 1.1 代码里已经有的

| 事实 | 位置 |
|----|----|
| `session/new` 已经在发 `mcpServers`，但恒为 `[]` | `src-tauri/src/acp.rs:360` |
| ACP 进程由 AgentShell 独占 spawn / 持有 stdin | `acp.rs` `AcpService.start/send_prompt/stop` |
| 已有「首条 prompt 注入本地历史」的机制（可复用为 skill 提示注入） | `src/lib/sessionHistory.ts` |
| 已有权限弹窗链路（`session/request_permission`） | `acp.rs` + `src/components/PermissionDialog.tsx` |
| 每个会话独立 transcript JSONL + session 记录 | `src-tauri/src/storage.rs` |
| 多会话并存、按 sessionId 路由事件 | `AcpService.sessions` + `acp-event` |

结论：**派生一个「子会话」在后端几乎是免费的**——它就是多一条 session 记录 + 一个 ACP 进程，路由已经按 sessionId 分好了。

### 1.2 ACP 协议给了什么、没给什么

来自 `agent-client-protocol-schema 1.4.0`：

- `NewSessionRequest { cwd, additional_directories, mcp_servers, _meta }`
  - `McpServer` = `Stdio`（**所有 Agent 必须支持**）/ `Http` / `Sse`（需 `mcpCapabilities` 声明）。
  - 本机实测（dev.log）：OpenCode 声明 `mcpCapabilities: { http: true, sse: true }`。
- `SessionUpdate` 只有这些：`UserMessageChunk / AgentMessageChunk / AgentThoughtChunk / ToolCall / ToolCallUpdate / Plan / PlanUpdate / PlanRemoved / AvailableCommandsUpdate / CurrentModeUpdate / ConfigOptionUpdate / SessionInfoUpdate / UsageUpdate`。
- `ToolKind` 只有：`read / edit / delete / move / search / execute / think / fetch / switch_mode / other`。

**关键否定结论**：ACP v1 **没有子代理 / 子会话概念**。Agent 内部起的 subagent（Claude 的 Task、OpenCode 的 task 工具）在协议上只是一条 `ToolCall`，我们能拿到的只有它自己愿意塞进 `content` 的东西。想要「完整的第二条对话流」，只能是 **AgentShell 自己去开第二个会话**。

### 1.3 本机各 Agent 的真实家底（2026-07-25 扫描）

| Agent | MCP 配置位置 | 已装 skills |
|----|----|----|
| OpenCode | `~/.config/opencode/opencode.jsonc` → `"mcp": { blender, notion… }` + `"plugin"` | `~/.config/opencode/skills/`：docx, frontend-design, pdf, pptx, pua, ui-ux-pro-max, workflow, xlsx |
| Codex | `~/.codex/config.toml` → `[mcp_servers.blender] / [mcp_servers.ai-game-developer](url=https) / [mcp_servers.node_repl]`，另有 `[plugins.*]`、`[marketplaces.*]` | 走 plugin/marketplace 体系 |
| Claude Code | `~/.claude.json` / 项目 `.mcp.json`（`~/.claude/settings.json` 里没有） | `~/.claude/skills/`：domain-modeling, grill-with-docs, grilling, officecli |
| Grok | `~/.grok/`（`managed_config` 体系） | `~/.grok/skills/`：check-work, code-review, create-skill, docx, help, imagine, pptx, xlsx |

即：**四家都有 skills 目录、都有 MCP 概念，但内容互不相通**。用户说的「unity skill 装在 opencode/codex，grok build 没有」正是这个格局。

---

## 2. 需求 5：让没装的 Agent 也能用上项目级 skill / MCP ✅ 已实现（Stage 1，2026-07-25）

实现落点：`src-tauri/src/context_inventory.rs`（采集 / 选择 / 注入）、`acp.rs`（session/new 注入）、
`ContextPanel` 的 **Project context** 卡片、`App.handleSend`（skill 提示注入）。

**采集时机**（这条是产品决定，不是技术细节）：

| 时机 | 行为 |
|----|----|
| 添加项目后 | 立刻扫一次，右侧面板直接显示"这台机器能借给它什么" |
| 切换项目 / 右侧面板可见时 | 异步重扫（几个文件读 + 几个目录列举，实测 <10ms） |
| 面板上的 ⟳ | 手动重扫（在别的工具里刚装了 skill 时用） |
| **应用启动** | **不扫**——启动路径刚清理过，不再往里加活 |

缓存故意不做：一份过期的清单比没有清单更糟（用户刚在别处装了 skill，回来看却没有）。

**默认值**：skill 默认借出（纯提示文本，成本近零），MCP server 默认不借（要拉起进程，得用户点头）。

**去重**：目标 agent 自己配置里已有的，永远不注入（`blender` 在 opencode+codex 都有 → 只借给 claude / grok）。

**密钥**：`.agentshell/context.json` 只存 `{id: true/false}`；env 的**键名**进 UI，**值**只在 `session/new` 那一刻从原配置里读，不落盘、不进日志。

实测本机扫描结果：

```
mcp: ai-game-developer(http, codex) · blender(stdio, opencode+codex) · node_repl(stdio, codex) · notion(stdio, opencode)
skills(17): opencode 8 · grok-build 8 · claude-code 4（docx/pptx/xlsx 在两家都有，已合并）
```

### 2.1 设计（原始方案，已按此实现）

```
┌─ 采集（只读） ─────────────┐   ┌─ 项目清单（SSOT，我们own） ──┐   ┌─ 注入 ────────────┐
│ opencode.jsonc  → mcp[]   │   │ .agentshell/context.json     │   │ session/new       │
│ ~/.codex/config.toml      │──▶│  mcpServers: [...启用项]     │──▶│  mcpServers: [...] │
│ ~/.claude.json/.mcp.json  │   │  skills:     [...启用项]     │   │ +首条prompt前缀   │
│ 各 skills/ 目录 + 项目内   │   │  perAgent:   覆盖/排除       │   │ +skills MCP server│
└───────────────────────────┘   └──────────────────────────────┘   └───────────────────┘
```

1. **采集层（Rust，只读）**：每个 Agent 一个 adapter，把它的配置解析成统一结构
   `McpServerSpec { name, transport: stdio|http|sse, command/args/env | url/headers }`、
   `SkillSpec { id, name, description, dir, source }`。
   项目级也要扫：`.mcp.json`、`.claude/skills/`、`AGENTS.md`、`.agentshell/skills/`。
2. **项目清单**：`.agentshell/context.json` 由 AgentShell 拥有（不写别人的配置文件），用户勾选哪些 MCP / skill 属于本项目。
3. **注入**：`start_acp_session` 时把清单转成 `mcpServers` 数组下发。
   - **去重是硬要求**：目标 Agent 自己配置里已有的（按 name + command 判断）不要再注入，否则 codex 会出现两个 `blender` 工具命名空间。
   - http/sse 只在对方 `mcpCapabilities` 声明时使用，否则跳过并在 UI 说明原因。
4. **skill 兜底两级**：
   - **L1 提示注入**（便宜、100% 兼容）：会话首条 prompt 前面挂一段清单——每个 skill 的名字 / 什么时候用 / `SKILL.md` 绝对路径。有文件读权限的 Agent 会自己去读。复用 `sessionHistory.ts` 那套 `withXxxInjection` 机制，成本几百 token。
   - **L2 内置 MCP server**（正解）：AgentShell 自己起 `agentshell.exe --mcp skills --project <root>`（stdio），暴露 `list_skills()` / `get_skill(id)` / `read_skill_file(id, path)`。这样 grok build 也能像原生一样发现 unity skill。
   - 顺序：**原生已有 → 什么都不做**；否则 L2；L2 不可用（对方连 stdio MCP 都拒）再退 L1。

### 2.2 工作量与风险

| 项 | 估计 |
|----|----|
| 采集 adapter ×4 + 统一模型 | 中（每家格式不同，jsonc 要容注释） |
| `.agentshell/context.json` + 右侧面板勾选 UI | 中 |
| session/new 注入 + 去重 | 小 |
| 内置 skills MCP server（复用同一个 exe，加一个 `--mcp` 子命令） | 中 |

风险：
- **密钥**：别人 MCP 配置里的 `env` 往往含 token — 采集后不得进 dev.log，不得写进 `.agentshell/context.json`（只存引用，运行时再读原配置）。
- **工具名冲突**：同名 server 注入两次会让 Agent 侧行为不可预测 → 去重优先，冲突时以 Agent 自己的为准。
- **MCP 列表是 session 级的**：改了清单必须重连会话才生效（正好符合「连接只是一次刷新」的模型）。

---

## 2.9 子代理权限：项目外路径的预授权 ✅ 已实现（2026-07-25）

**现象**：让 opencode 的子代理读项目外的图片，卡 6 分钟无反应，中断才结束。

**日志证据**（`session-1784955715691`）：两次子代理尝试期间，ACP 事件分别只有 4 条和 2 条，
`session/request_permission` **一次都没有**；而同一个会话里让主模型直接读，1 毫秒弹窗、
2.7 秒答完、19 秒读完，30 秒窗口内 214 条事件。

**原因**：子代理跑在 agent 进程内部，它的审批走 agent 自己的通道，**不转发成 ACP 请求**。
客户端根本不知道有东西在等确认 → 永久静默等待。

**实测三种解法**（同一个项目外文件、同一句 prompt，真实 opencode 会话）：

| 方案 | 结果 |
|----|----|
| 什么都不做 | ❌ 弹权限请求 |
| ACP `session/new.additionalDirectories` | ❌ **字段被接受但不影响权限判定**，照样弹 |
| `OPENCODE_PERMISSION` 环境变量 | ✅ **不再询问，直接读完** |

opencode 的真实闸门在 `Tool.assertExternalDirectory`：路径不在 cwd 内就以
`permission: "external_directory"`、`patterns: ["<dir>/*"]` 发起询问；而 `config.permission`
会被 `OPENCODE_PERMISSION` 这个环境变量 merge 覆盖。**AgentShell 本来就是 spawn agent 进程的人**，
所以可以按会话注入，不碰用户任何配置文件：

```
OPENCODE_PERMISSION={"external_directory":{"C:/Users/.../Screenshots/*":"allow"}}
```

**实现**：发送前扫描草稿里的路径 → 项目外且未授权的弹一次确认 → 授权写进
`.agentshell/context.json` 的 `workspaceRoots` → 重连 agent（scope 只能在 `session/new` 时定）→
opencode 会话带着 `OPENCODE_PERMISSION` 启动。用户已有的 `OPENCODE_PERMISSION` 会被合并而不是覆盖。

`additionalDirectories` 仍然照发——它是协议里正确的表达方式，别的 agent 可能认；只是对 opencode
不能指望它。

---

## 3. 需求 6：子代理分屏

### 3.1 必须先接受的事实

- **AgentShell 自己派生的子会话**：有完整事件流 → 能做真正的分屏（左主对话、右子对话，各自 You/Thinking/Tool/Reply）。
- **Agent 内部的 subagent（Claude Task / OpenCode task）**：协议上只是一条 `ToolCall`，只能拿到它塞进 `content` 的摘要 → 只能做「一条泳道 + 折叠详情」，做不出真正的第二条对话流。UI 上必须诚实标注这是工具视图，不能假装是一个独立 Agent 在说话。

### 3.2 设计

1. **数据层**：`SessionEvent` 增加 `laneId?: string`（缺省 = `"root"`）。
   - 子会话：`laneId = 子 sessionId`。
   - 内部 subagent：`laneId = toolCallId`（Phase 2）。
2. **UI 层**：`.session-stage` 从单列改成 `grid-template-columns: repeat(var(--lane-count), minmax(0,1fr))`（现在已经是 `position:relative; overflow:hidden` 的 flex 容器，改造成本低）。
   - 开：出现新 lane 时自动分屏（最多 2~3 栏，超出走 Tab）。
   - 关：lane 结束后 **延迟折叠**（比如 3s）并留一个「已完成 · 展开」的小卡，而不是当场消失——当场消失等于把证据吞了。
   - 用户可 pin 住某条 lane 不自动关。
3. **顺序**：Phase 1 只做「AgentShell 派生的子会话」（等需求 7 落地）；Phase 2 再加内部 subagent 的启发式泳道（按 `ToolCall.title` / tool name 匹配 `task`、`Task(...)`、`subagent`）。

---

## 4. 需求 7：多 Agent 自动桥接

### 4.1 社区已经怎么做（参考）

主流做法高度一致：**把一个 CLI Agent 包成 MCP server，另一个 Agent 当工具调用它**，任务异步（返回 `task_id` 而不是阻塞）。

- `opencode-mcp` — 把 OpenCode 暴露成 subagent 给任意 MCP 客户端。
- `cursor-delegate-mcp` — Claude Code / Codex / Copilot 把实现交给 Cursor CLI，自己负责计划与验收（正是用户说的「GPT 派活、别人干、GPT 验收」）。
- Codex CLI 自带 MCP server 模式，直接被别的 Agent 调。

### 4.2 AgentShell 应该怎么做（比上面更顺）

上面那些方案要每个 Agent 各自配一遍、各自 spawn 一遍子进程。**AgentShell 本来就是所有 Agent 的父进程**，所以应该当 broker：

内置 `agentshell-bridge`（stdio MCP，按项目/会话开关注入）：

| 工具 | 说明 |
|----|----|
| `list_agents()` | 返回已安装的 Agent + 可用模型（复用刚做的 `list_agent_commands`） |
| `delegate(agent, prompt, model?, mode?, files?)` | 起一个**子会话**，立刻返回 `taskId`（异步） |
| `poll(taskId)` / `result(taskId)` | 取进度 / 最终回复 |
| `cancel(taskId)` | 复用现有 `cancel_acp_session` |

执行侧：`delegate` → 在同项目建一条 `parentSessionId = 当前会话` 的 session（不进左侧列表，只作为 lane 显示）→ 走现有 `start_acp_session` / `send_prompt` → 事件带 `laneId` 推给 UI → 结束后把最终 assistant 文本作为工具结果回给发起方。

于是「GPT 调 DeepSeek」变成：Codex 会话里模型调用 `delegate("opencode", prompt, model="deepseek-…")` → 右边分屏出现 DeepSeek 在干活 → 干完结果回到 Codex，Codex 验收。

### 4.3 交互与闸门（这块比技术更重要）

- **默认关闭**，按项目开启；开启后仍然每次 `delegate` 走一次现有权限弹窗（可勾「本会话内自动批准」）。
- **硬上限**：`depth = 1`（子会话不能再 delegate）、并发子会话 ≤ 2、单任务超时、单任务 token 上限；任一超限直接拒绝并把原因回给调用方。
- **两个入口**：
  1. Agent 主动调（上面这条）。
  2. 用户主动编排：Composer 里一个「Delegate…」动作，选 Agent + 模型，生成一句结构化 prompt；再配一个「让 X 验收」的小 recipe。第 2 条应该**先做**——它不需要模型配合，能马上验证分屏与子会话链路。
- **成本可见**：子会话的 usage 要并进右侧面板，否则用户完全不知道花了多少。

### 4.4 工作量

| 项 | 估计 |
|----|----|
| 子会话模型（`parentSessionId` + 不进 shelf + lane 路由） | 中 |
| bridge MCP server（同一 exe 的 `--mcp bridge` 子命令） | 中 |
| 权限 / 上限 / 取消 | 中 |
| 用户主动编排入口（先做） | 小 |

---

## 5. 建议的落地顺序

0. **Stage 0** ✅：tool 卡片显示 `content` / `locations` / `rawOutput`（见 0.5）。
1. **Stage 1** ✅：MCP 采集 + 项目清单 + `session/new` 注入 + skills L1 提示注入。
   收益最大、风险最低，`grok build` 当天就能「知道」unity skill 的存在。
2. **Stage 2**：内置 `agentshell-skills` MCP server（L2），让不支持 skill 的 Agent 有原生级体验。
3. **Stage 3**：子会话 + lane 数据模型 + 分屏 UI + 用户主动 delegate。
4. **Stage 4**：`agentshell-bridge` MCP，让 Agent 自己发起 delegate（带权限与上限）。
5. **Stage 5**：内部 subagent 的启发式泳道。

---

## 6. 待定问题

- `.agentshell/context.json` 要不要进 git？（建议：进，但密钥只存引用）
- 注入的 MCP 出错时（对方连不上 blender-mcp），是静默降级还是在 Clean 里报一条？（建议：Clean 里一条可折叠的系统卡）
- skill 命中率：L1 提示注入到底有多大概率让 Agent 真的去读 `SKILL.md`？需要在 grok build 上做一次真实测量再决定要不要直接上 L2。
- 子会话的 transcript 要不要单独存盘？（建议：存，路径 `.agentshell/transcripts/<childId>.jsonl`，父会话记录一条 `delegate` 事件指过去）
