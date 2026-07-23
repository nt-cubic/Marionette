# AgentShell 参考项目审计

## 0. 审计目标

本审计用于判断 AgentShell 应该从 Palot、Agent Deck、Codeg、Zed 借鉴哪些产品和工程思路。

结论原则：

1. 只参考架构和交互，不直接复制实现。
2. 不把 AgentShell 做成 Agent 平台。
3. 不把 AgentShell 做成 IDE。
4. Windows-first。
5. Raw Terminal 仍然是权威源。

## 1. 已拉取参考仓库

本地参考目录：

```text
_research/repos/
  palot/
  agent-deck/
  codeg/
  zed/
```

当前快照：

```text
palot      main fd63a75
agent-deck main f70f19e
codeg      main 0f1ec2d
```

来源：

1. Palot: `https://github.com/ItsWendell/palot`
2. Agent Deck: `https://github.com/asheshgoplani/agent-deck`
3. Codeg: `https://github.com/xintaofei/codeg`
4. Zed: `https://github.com/zed-industries/zed`

Zed 已克隆到 `_research/repos/zed/`，作为只读产品与工程参考。AgentShell 不复用 Zed 源码，也不把 Zed 的 GPUI/编辑器体系带入产品。

## 2. 许可证观察

1. Palot：MIT。
2. Agent Deck：MIT。
3. Codeg：Apache-2.0。

即使许可证允许，也不建议直接复制代码。AgentShell 的目标和边界更窄，直接搬代码会引入不必要的平台功能。

## 3. Palot 观察

### 3.1 定位

Palot 是 OpenCode 的桌面 GUI 层。它使用 Electron + React，自动管理 OpenCode server，并通过 OpenCode SDK、HTTP、SSE 做结构化 UI。

### 3.2 值得借鉴

1. OpenCode server lifecycle。
2. OpenCode SDK 接入。
3. SSE 保留在 renderer，不把 streaming response 通过普通 IPC 强行序列化。
4. localhost 请求做 retry。
5. 非 SSE 请求可通过主进程代理，避免浏览器连接数限制。
6. chat message、tool call、permission、question、diff 的视觉组织。
7. review panel 对大量 diff 使用虚拟化和折叠策略。
8. Open in external editor。
9. command palette。

### 3.3 不适合第一版照搬

1. Electron 基座。
2. model/provider selector。
3. permission allow always。
4. automation。
5. migration wizard。
6. secure credential storage。
7. mDNS remote server discovery。
8. auto-update。
9. commit/push/PR 工作流。

### 3.4 对 AgentShell 的结论

OpenCode 应该成为第一批结构化 Clean View 的优先对象。

建议：

1. MVP 仍先用 Generic PTY 启动 OpenCode。
2. 后续做 OpenCode hybrid adapter。
3. OpenCode hybrid adapter 可以参考 Palot 的 server 管理、SDK 连接、SSE 处理。
4. 不引入 Palot 的 provider/config/migration/automation 平台功能。

## 4. Agent Deck 观察

### 4.1 定位

Agent Deck 是 Go 写的 AI agent command center，主要面向 TUI、tmux、session fleet、worktree、cost、MCP、skills、conductor。

### 4.2 值得借鉴

1. session dashboard。
2. session 状态：running、waiting、done、errored。
3. 多 session 搜索、分组、归档。
4. terminal bridge 的清晰边界。
5. terminal resize 的 race guard。
6. writer 加锁。
7. session lifecycle 测试意识。
8. ring buffer/logging 思路。

### 4.3 不适合第一版照搬

1. tmux-first 架构。
2. WSL/macOS/Linux 假设。
3. conductor。
4. phone/Telegram/Slack 控制。
5. MCP manager。
6. skills manager。
7. cost dashboard。
8. worktree fleet。
9. remote watcher。

### 4.4 对 AgentShell 的结论

Agent Deck 适合参考 session 管理和状态语义，不适合作为 Windows 原生 GUI 的技术基座。

建议：

1. 借鉴 session 状态模型。
2. 借鉴 session list 的信息密度。
3. 不使用 tmux 作为核心依赖。
4. 不引入 cost/MCP/skills/conductor。

## 5. Codeg 观察

### 5.1 定位

Codeg 是 Tauri 2 + Next.js + Rust 的多 Agent coding workspace。它覆盖 Agent 聚合、Tauri desktop、server/Docker、MCP、skills、office、automation、chat channel、git、file tree、diff、terminal。

### 5.2 值得借鉴

1. Tauri 2 + Rust + Web frontend 的总体方向。
2. `portable-pty` 用于 terminal。
3. Windows shell UTF-8 设置。
4. PTY reader/writer 分线程。
5. writer 使用 channel 串行写入。
6. child process exit reconciliation。
7. frontend 使用 xterm.js。
8. 文件树的信息结构。
9. git changed files 的树形聚合。
10. file open target 的抽象。
11. Claude/Codex/OpenCode 历史路径和 parser 方向。

### 5.3 不适合第一版照搬

1. server/Docker mode。
2. MCP 管理。
3. skills 管理。
4. Office workflow。
5. chat channels。
6. automation。
7. git credential/account 管理。
8. multi-agent collaboration runtime。
9. 内置 preview/editor tabs。
10. 完整 parser 体系。

### 5.4 对 AgentShell 的结论

Codeg 对 AgentShell 最有工程参考价值，尤其是 terminal 和 Workspace Awareness。

建议：

1. PTY 方案优先采用 `portable-pty`。
2. Windows shell 需处理 UTF-8。
3. 文件树和 git changed files 可以做轻量版。
4. 不做上传、删除、commit、office preview、远程 server。

## 6. Zed 观察

### 6.1 定位

Zed 是高性能代码编辑器，不是 AgentShell 同类产品。它适合参考速度感、布局密度、文件树/git/terminal 的工作区体验。

### 6.2 值得借鉴

1. 克制 UI。
2. 高密度但清晰的 panel。
3. 快捷键和 command palette。
4. 文件树体验。
5. terminal 与编辑区并存的布局感觉。
6. 低噪音状态展示。

### 6.3 不适合第一版照搬

1. GPUI。
2. 编辑器核心。
3. 语言服务。
4. tree-sitter/symbol 搜索。
5. 多人协作。
6. 内置 AI agent runtime。

### 6.4 对 AgentShell 的结论

Zed 只作为体验标尺，不作为代码基座。

### 6.5 ACP 与外部 Agent 的边界复核

Zed 当前把三种路径明确分开：

1. Zed Agent：Zed 负责模型、工具、权限、Skills 和上下文。
2. External Agent：通过 ACP 接入，Agent 自己负责认证、模型、订阅和原生配置。
3. Terminal Thread：直接运行 CLI/TUI，Zed 只负责线程容器、分组和恢复。

这直接解释了 AgentShell 需要避免的假设：ACP 不保证每个 Agent 都暴露模型切换、思考强度、订阅剩余额度、历史恢复和 token usage。界面应按 adapter 能力显示状态，而不是为所有 Agent 强行绘制相同的按钮。

Zed 的线程 token usage 主要表示当前上下文窗口的已用/上限；它不等价于 Claude、Codex、Grok 或 OpenCode 的账户余额。账户额度属于 Agent/provider 原生能力，AgentShell 只能通过明确的 adapter 查询并标注更新时间，查询不到时显示 `Unavailable`。

### 6.6 轻量版 Zed 的产品边界

AgentShell 只保留 Zed 最有价值的工作流骨架：

1. 项目分组下的 session/thread 列表。
2. 中央单对话窗口，可打开多个 conversation tabs，默认不分屏。
3. 左右侧栏可快速收缩。
4. Clean/Raw 双视图，Raw Terminal 是权威源。
5. Clean View 统一折叠工具调用和思考内容。
6. 右侧 Todo 与可手动刷新的 usage 状态。
7. 会话持久化、退出恢复、进程重连和明确的错误状态。

暂不做 Zed 的编辑器、LSP、内置 Git review、Parallel Agents、MCP 管理、Skills 管理和完整 ACP Registry。它们都可以在后续通过独立页面加入，但不应继续挤进主对话界面。

### 6.7 针对已知痛点的实现策略

| 痛点 | AgentShell 策略 |
| --- | --- |
| Agent 无法切换模型/思考强度 | 读取 ACP session config/mode；无能力则隐藏选择器并显示原生配置入口 |
| Claude 身份验证失败 | 认证状态、启动错误和 stderr 单独记录；提供“在原生 Agent 中登录”的明确动作，不共享假想凭据 |
| 无法看到剩余用量 | 区分 thread context usage 与 provider quota；右侧手动刷新，显示来源和更新时间 |
| 对话窗口闪退/关闭 | 持久化 session manifest、原始日志和最后状态；启动时恢复，进程退出时保留可重连/重试入口 |
| 工具调用和思考内容太吵 | Clean View 默认折叠细节，提供单一全局显示切换；Raw View 永远保留完整输出 |

## 7. 新增产品方向：Workspace Awareness

用户提出的文件清单、点击打开、文件树、git 修改历史，属于 Workspace Awareness。

它应该是显示层和操作层，不是 IDE。

### 7.1 应该做

1. 本次改动清单。
2. Changed Files。
3. 点击文件用系统默认 app 打开。
4. Open in Zed/Cursor/VS Code。
5. Copy Path。
6. Pin to Handoff。
7. 轻量文件树。
8. 简单 git log。
9. 单文件 diff。
10. session activity timeline。

### 7.2 不应该做

1. 内置代码编辑器。
2. LSP。
3. 全文索引。
4. 符号搜索。
5. commit graph。
6. rebase/merge/stash 管理。
7. token/cost dashboard。
8. 自动总结所有修改。

### 7.3 实现原则

1. 后端按需读取。
2. 文件树懒加载。
3. 默认忽略 `.git`、`node_modules`、`dist`、`build`、`target`。
4. git 数据手动刷新或低频刷新。
5. 打开文件交给系统或外部编辑器。
6. 不把文件内容长期存入 SQLite。

## 8. 对现有文档的修订建议

建议补充：

1. 产品策划案：新增 Workspace Awareness 范围。
2. 程序设计文档：新增 WorkspaceService、FileService、Workspace commands。
3. 施工文档：新增 Milestone 10：Workspace Awareness v0。

建议保持不变：

1. Raw Terminal 是权威源。
2. GenericPtyAdapter 优先。
3. Handoff 默认 prefill。
4. 不做 Agent 平台。
5. 不做内置 IDE。
