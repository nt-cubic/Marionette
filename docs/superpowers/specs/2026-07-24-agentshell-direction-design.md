# AgentShell 方向设计文档

> 日期：2026-07-24
> 状态：已收敛 — 内部协议定名为 **ASP（AgentShell Protocol）**
> 正式协议草案：[`2026-07-24-agentshell-asp-protocol.md`](./2026-07-24-agentshell-asp-protocol.md)

---

## 0. 为什么有这个文档

AgentShell 项目已完成基础骨架（Tauri + React + PTY + 持久化），但在架构方向上曾有一个核心分歧：**ACP 协议 vs 纯 PTY**。

讨论结论：**两者都是 Backend，不是产品语义本身。**  
产品层统一讲 **ASP（AgentShell Protocol）**；ACP / PTY 由适配器翻译。详见 ASP 草案。

本文档保留当时的状态盘点与混合架构动机；**命令/事件/Capabilities 以 ASP 文档为准。**

---

## 1. 当前项目状态

### 1.1 已实现的功能

**后端 (Rust/Tauri)：**
- `StorageService` — JSON 文件持久化，项目/session CRUD，有测试
- `PtyService` — `portable-pty` 驱动，powershell 启动/读写/调大小/杀进程，raw log 写入
- `SessionManager` — 追踪 live session
- `acp.rs` (1055 行) — 完整的 ACP JSON-RPC 实现：initialize、session/new、session/prompt、session/cancel、session/set_config_option、能力协商、权限自动 allow
- `commands.rs` — 所有 Tauri 命令已注册

**前端 (React/TypeScript)：**
- `App.tsx` — 主布局 + 状态管理
- `ProjectShelf.tsx` — 左侧项目树 + session 列表 + 深色/浅色切换
- `SessionView.tsx` — 多标签页 + xterm.js Raw Terminal + ACP 终端 + Clean View 占位
- `Composer.tsx` — ACP 能力驱动的 model/mode/effort 选择器 + Agent 切换 + 发送/中断
- `ContextPanel.tsx` — Usage/Todo 面板（部分 mock 数据）
- 完整类型定义 + API 层 + mock 数据

### 1.2 实际缺失的关键模块

| 模块 | 状态 |
|------|------|
| HandoffService（handoff.md 生成） | ❌ |
| GitService（git status/diff） | ❌ |
| FileTreePanel（文件树） | ❌ |
| CleanTranscript（真正的 Clean View 卡片） | ❌ 仅有 placeholder |
| 用户可见的权限确认对话框 | ❌ ACP 权限被自动 allow |
| 错误分类与展示 | ❌ |
| ExternalOpenService | ❌ |
| ActivityTimeline | ❌ |

---

## 2. 核心方向问题：ACP vs PTY

### 2.1 ACP 路线（当前代码实际走的路线）

**已实现的 ACP 能力：**
- 通过 stdio JSON-RPC 与 Agent 通信
- 支持 initialize → session/new → session/prompt 生命周期
- 支持 session/set_config_option 切换 model/mode/effort
- 自动处理 fs/read_text_file、fs/write_text_file 等 agent→client 请求
- Composer 控件全部基于 ACP capabilitySnapshot

**ACP 的优势：**
- 结构化协议 → Clean View 自然产生（message/event/tool_call 有明确语义）
- Composer 统一控件天然工作（session/set_config_option）
- 权限/确认可拦截（session/request_permission）
- 跨 Agent 消息传递在协议层面可行

**ACP 的局限：**
- 只有 OpenCode 原生支持 ACP
- Codex CLI / Claude Code 需第三方 npx 适配器，质量存疑
- Grok Build 完全不支持 ACP
- 新增 Agent 需要适配 ACP 或走 PTY fallback
- 依赖外部 `agent-client-protocol-schema` crate

### 2.2 纯 PTY 路线（原始产品愿景）

**纯 PTY 的理念：**
- 所有 Agent 走真实 PTY，Raw Terminal 是 source of truth
- 前端不解析协议，只显示终端输出
- Composer 仅发送文本到 PTY stdin

**PTY 的优势：**
- 任何 CLI/TUI 都能跑，无协议依赖
- 简单可靠，不依赖 Agent 厂商支持
- Raw Terminal 永远可用

**PTY 的局限：**
- 统一 Composer 控件（model/mode/effort）需 screen scraping 或向 TUI 注入命令，极脆弱
- Clean View 只能做 ANSI 粗解析，无法获得结构化事件
- 无法感知 Agent 内部状态（等待输入/忙碌/权限请求）
- 跨 Agent 通信几乎不可能

### 2.3 真正的难点

项目的核心差异化价值恰好就是 PTY 路线最薄弱的环节：

| 核心价值 | PTY 方案 | ACP 方案 |
|---------|---------|---------|
| 统一控件切换 model/mode | 需向 TUI 注入按键（脆弱） | session/set_config_option（标准） |
| 统一输出排版（Clean View） | ANSI 粗解析（模糊） | 结构化 message/event（清晰） |
| 无缝切换 Agent + 上下文 | 文件级 handoff（勉强） | 协议级上下文传递（自然） |
| 多 Agent 互通 | 无法实现 | 协议桥接（可行） |

---

## 3. 建议方案：ASP + ACP/PTY Backend

### 3.1 核心理念

不需要在 ACP 与 PTY 里二选一当「唯一信仰」。

- **ASP**：UI / Session Runtime 的统一语言（能力、发送、handoff、错误）
- **Backend**：按 Agent 选择 `acp` 或 `pty`（及未来扩展）
- **诚实降级**：无 model/effort/cancel 就不画假控件；跨 Agent 上下文靠 handoff，不靠协议魔法

```
                    ┌──────────────────────────────────┐
                    │     UI 只讲 ASP（asp/0）            │
                    │  Agent · 输入 · Capabilities · 交接 │
                    ├──────────────────────────────────┤
                    │     OpenCode → AcpBackend          │
                    │     Codex    → PtyBackend          │
                    │     Claude   → PtyBackend          │
                    │     Grok     → PtyBackend          │
                    │     Custom   → PtyBackend          │
                    └──────────────────────────────────┘
```

### 3.2 AgentConfig 设计

```typescript
type AgentConfig = {
  id: string
  label: string
  command: string
  args: string[]
  cwdMode: "project-root" | "custom"
  customCwd?: string

  // 通信方式：acp | pty
  transport: "acp" | "pty"

  // ACP 专用：当 transport = "acp" 时生效
  acpCapabilities?: {
    supportsModes: boolean        // 是否支持 mode 切换
    supportsModels: boolean       // 是否支持模型选择
    supportsEffort: boolean       // 是否支持思考强度
    supportsCancel: boolean       // 是否支持中断
  }

  // PTY 专用：当 transport = "pty" 时生效
  ptyConfig?: {
    shellMode: boolean            // 是否通过 shell 启动
    sendStrategy: "stdin" | "bracketed-paste"
    initialCommands?: string[]    // 启动后自动发送的初始化命令
  }

  enabled: boolean
}
```

### 3.3 Composer 自适应

| 场景 | Composer 展示 |
|------|-------------|
| ACP Agent（如 OpenCode） | 完整控件：Agent 切换 + 模型选择 + Mode 选择 + 强度 + 输入框 + 发送 |
| PTY Agent（如 Claude/Grok） | 简化控件：Agent 切换 + 输入框 + 发送 |
| 无 session | 只有 Agent 切换 + 输入框（自动创建） |

### 3.4 Clean View 自适应

| 场景 | Clean View |
|------|-----------|
| ACP Agent | 结构化卡片：user_message / assistant_message / tool_call / file_change |
| PTY Agent | ansi-raw 粗解析：按空白/时间切块，显示为 raw_chunk 卡片 |

### 3.5 上下文切换（Handoff）

所有 Agent 统一走文件级 handoff（不分 ACP/PTY）：
- `.agentshell/handoff.md` 是标准格式
- 切换 Agent 时自动生成，prefill 到 Composer
- 用户手动检查和发送

### 3.6 多 Agent 互通（远期）

- 第一版：通过 handoff.md 文件传递上下文
- 第二版：ACP Agent 间可通过协议桥接（同为 ACP 时）
- 第三版：PTY Agent 可通过 "转录 + 重放" 实现有限互通

---

## 4. 中期执行计划

### Phase 1：整理代码结构
- 保留 `acp.rs` 但不作为核心依赖
- 重构 PTY 启动：不再硬编码 powershell，改为接收 command + args
- AgentConfig 新增 `transport` 字段，默认 pty
- Composer 根据 transport 自适应控件

### Phase 2：PTY 通用化
- 所有 Agent 配置默认 transport = "pty"
- Composer PTY 模式：简化控件，只保留 Agent 切换 + 输入框
- `start_terminal` 命令改为接收 command + args

### Phase 3：OpenCode ACP 增强
- OpenCode 配置 transport = "acp"
- ACP Composer 控件激活（model/mode/effort）
- ACP Clean View 结构化显示

### Phase 4：Handoff + 文件树 + Git
- HandoffService（handoff.md 生成）
- FileTreePanel（项目文件树）
- GitService（changed files / diff）

### Phase 5：Clean View + 体验打磨
- ansi-raw parser 用于 PTY Agent
- 虚拟列表 + 大文本折叠
- 错误分类展示

---

## 5. 决策记录（相对原稿）

| 项 | 结论 |
|----|------|
| 内部协议名称 | **ASP（AgentShell Protocol）**，版本 `asp/0` |
| ACP vs PTY | 都是 Backend；默认矩阵见 ASP §7.2 |
| PTY 统一 model/mode 控件 | **v0 不做**；无 Capabilities 则隐藏 |
| 跨 Agent 上下文 | **handoff.md + 用户确认**；不承诺无缝共享 session |
| ACP 依赖库 | **先保留**现有实现，不阻塞 ASP 语义落地 |
| OpenCode | ACP 增强继续做，但不阻塞 PTY 真启动校正 |
| `/` 命令 | PTY 透传；ACP 仅 advertised；`/connect` 类指向 Raw/原厂 CLI |

仍可后续讨论（不阻塞 asp/0）：见 ASP 文档 §16。

---

## 附录 A：当前代码 vs 文档对照表

| 文档要求 | 实现状态 |
|---------|---------|
| M1: 可打开的桌面壳 | ✅ |
| M2: 项目/Agent 配置 | ✅ |
| M3: Raw Terminal MVP | ✅（但硬编码 powershell） |
| M4: 多 Session 管理 | ✅ |
| M5: 首批 Agent 可启动 | ⚠️ ACP 方式已实现，但未真正测试过启动各 Agent |
| M6: Handoff | ❌ |
| M7: Clean View v0 | ❌（仅占位） |
| M8: OpenCode 增强 | ⚠️ ACP 协议层通，但 Clean View 未做 |
| M9: 体验打磨 | ❌ |
| M10: Workspace Awareness | ❌ |