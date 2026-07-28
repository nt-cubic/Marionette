# Marionette 产品策划案

## 0. 一句话定义

Marionette 是一个运行在 Windows 本地的轻量多 Agent CLI/TUI 图形壳。它负责启动、展示、切换、记录和 handoff 多个原厂 Agent 工具，例如 OpenCode、Codex CLI、Claude Code、Grok Build。

Marionette 不替代这些 Agent，也不实现自己的 Agent runtime。它只让这些原厂 CLI/TUI 更容易同时使用。

## 1. 产品目标

### 1.1 核心目标

1. 让用户在一个漂亮、快速的桌面界面里管理多个 Agent session。
2. 每个 Agent session 背后都是真实运行的原厂 CLI/TUI。
3. 用户可以随时在 Clean View 和 Raw Terminal View 之间切换。
4. 用户可以把一个 Agent 的上下文整理成 handoff prompt，交给另一个 Agent 继续工作。
5. 软件要轻、快、稳定，不变成 IDE、平台、模型路由器或云服务。

### 1.2 首批支持目标

首批支持以下 Agent：

1. OpenCode
2. Codex CLI
3. Claude Code
4. Grok Build

第一版支持标准是：

1. 能配置命令。
2. 能启动真实进程。
3. 能显示 Raw Terminal。
4. 能输入。
5. 能保存 raw log。
6. 能切换 session。
7. 能通过 handoff prompt 接收上下文。

第一版不要求每个 Agent 都有完美 Clean View。

## 2. 用户画像

### 2.1 主要用户

主要用户是不想写复杂脚本、不想维护多个终端窗口，但经常同时使用多个 Agent CLI 的本地开发用户。

用户关心：

1. 界面好看。
2. 打开快。
3. 切换快。
4. 输出不卡。
5. 不丢 session。
6. 原厂 CLI/TUI 永远可用。
7. 不被复杂配置和平台功能打扰。

### 2.2 不服务的用户

第一版不服务以下场景：

1. 团队多人协作平台。
2. 云端 Agent 调度平台。
3. API key 和模型供应商管理平台。
4. 复杂 MCP marketplace。
5. 自动化任务平台。
6. Office/PPT/文档生成套件。
7. 完整 IDE。

## 3. 产品底线

### 3.1 不做 Agent 平台

Marionette 绝对不做以下功能：

1. 不管理 API key。
2. 不管理模型供应商。
3. 不做 token/cost 统计。
4. 不做 MCP marketplace。
5. 不做 skills 管理。
6. 不做 prompt marketplace。
7. 不做云同步。
8. 不做多人协作。
9. 不做自动化任务平台。
10. 不做代码库 embedding。
11. 不做 RAG。
12. 不做全仓库索引。
13. 不做自己的 Agent runtime。
14. 不替代原厂 CLI/TUI 的能力。

### 3.2 Raw Terminal 是权威源

每个 session 的 source of truth 是真实 PTY。

Clean View 只是 Raw Terminal 输出的漂亮投影。Clean View 可以失败，但 Raw Terminal 不能失败。

必须满足：

1. 切换 Raw Terminal 不重启进程。
2. 切换 Clean View 不丢上下文。
3. Raw Terminal 输入直通 PTY。
4. GUI composer 发送输入必须明确可见。
5. GUI 不自动干扰 TUI 菜单、登录、权限确认、模型选择。

### 3.3 性能优先于复杂功能

Marionette 必须保持：

1. 启动快。
2. 空闲占用低。
3. 终端输出不卡顿。
4. 大量输出时 UI 不冻结。
5. parser 崩溃不影响 session。
6. raw log 写入不阻塞 UI。
7. Clean View 使用虚拟列表或增量渲染。
8. 后端负责重活，前端负责显示。

### 3.4 Windows 优先

当前阶段只保证 Windows。

暂不处理：

1. macOS 兼容性。
2. Linux 兼容性。
3. 跨平台打包细节。
4. 跨平台 PTY 差异。

后续如果要扩展平台，再单独开兼容性里程碑。

## 4. 产品形态

### 4.1 主界面布局

推荐四区布局：

```text
┌──────────────────────────────────────────────────────────┐
│ Top Bar: Project / Agent / View Toggle / Open in Editor  │
├──────────────┬───────────────────────────┬───────────────┤
│ Project &    │ Clean Chat / Raw Terminal │ Context Panel │
│ Session List │ Diff / Logs               │               │
├──────────────┴───────────────────────────┴───────────────┤
│ Composer: input / attach / prefill handoff / send          │
└──────────────────────────────────────────────────────────┘
```

### 4.2 左侧栏

左侧栏显示：

1. 项目列表。
2. 当前项目 session。
3. Agent 名称。
4. session 状态。
5. 最近活跃时间。

状态包括：

1. starting
2. running
3. waiting
4. exited
5. error

### 4.3 中间区域

中间区域支持：

1. Clean View。
2. Raw Terminal View。
3. Diff View。
4. Log View。

第一版重点是 Raw Terminal View。

Clean View 第一版可以只是：

1. 用户输入卡片。
2. raw chunk 分组卡片。
3. handoff 事件卡片。
4. parser unavailable 状态。

### 4.4 右侧栏

右侧栏显示：

1. Changed Files。
2. Handoff。
3. Pinned Context。
4. Session Metadata。
5. Tool Calls placeholder。

第一版 Tool Calls 可以为空，只保留位置。

### 4.5 底部 Composer

Composer 支持：

1. 输入普通 prompt。
2. 编辑 handoff prefill。
3. 选择发送目标 session。
4. 发送到 PTY 或协议 adapter。
5. 记录本次发送内容。

默认不自动发送 handoff。

## 5. 首批功能范围

### 5.1 MVP 必须有

1. 添加项目。
2. 选择项目。
3. 配置 Agent command。
4. 启动 Agent PTY。
5. Raw Terminal View。
6. Raw Terminal 输入。
7. raw log 保存。
8. session 列表。
9. stop session。
10. restart session。
11. Clean/Raw toggle。
12. Clean View placeholder。
13. `.marionette/handoff.md` 生成。
14. 切换 Agent 时 prefill handoff prompt。
15. Changed Files 面板。
16. Open in Zed。
17. 如果 Zed 不存在，显示 fallback。

### 5.2 MVP 可以没有

1. 完美 tool call parser。
2. 完美 permission prompt parser。
3. 自动发送 handoff。
4. MCP。
5. skills。
6. token/cost。
7. cloud。
8. account system。
9. auto-update。
10. plugin marketplace。
11. voice。
12. mobile。
13. automation。
14. full diff editor。

### 5.3 Workspace Awareness v0

Workspace Awareness 是工作区可见性功能。它只负责显示、打开和传递上下文，不负责编辑代码，不负责索引仓库，不负责替代 IDE。

建议加入第一版后半段或第二版早期：

1. 本次改动清单。
2. Changed Files 面板增强。
3. 点击文件后用系统默认 app 打开。
4. Open in Zed / Cursor / VS Code。
5. Copy Path。
6. Pin to Handoff。
7. 轻量文件树。
8. 简单 git log 列表。
9. 单文件 diff。
10. Session activity timeline。

### 5.4 Workspace Awareness 禁止项

禁止把 Workspace Awareness 做成 IDE。

不做：

1. 内置代码编辑器。
2. LSP。
3. 代码补全。
4. 符号搜索。
5. 全文索引。
6. commit graph。
7. merge/rebase/stash 管理。
8. token/cost dashboard。
9. 自动读取全仓库内容。
10. 自动总结所有修改。

## 6. Agent 支持策略

### 6.1 Generic PTY 优先

Claude Code、Codex CLI、Grok Build 第一版全部走 Generic PTY。

Generic PTY 支持：

1. 启动命令。
2. 设置 cwd。
3. 读取输出。
4. 写入输入。
5. resize terminal。
6. stop process。
7. 写 raw log。

### 6.2 OpenCode 优先增强

OpenCode 第一版也可以用 PTY，但它最适合优先增强 Clean View。

原因：

1. OpenCode 有 server/API/SSE 方向。
2. 更容易拿到结构化 session/message/event。
3. 更适合做漂亮的 chat、tool card、diff card。

### 6.3 Adapter 优先级

第一版优先级：

1. GenericPtyAdapter
2. OpenCodeAdapter
3. CodexAdapter
4. ClaudeCodeAdapter
5. GrokBuildAdapter

Codex、Claude、Grok 的专用 adapter 第一版可以只是 GenericPtyAdapter 的配置封装。

## 7. Handoff 策略

### 7.1 Handoff 目标

Handoff 只解决“把当前项目状态交给另一个 Agent 继续做”的问题。

它不是 memory 系统。

### 7.2 Handoff 来源

只允许使用轻量来源：

1. 用户手写 notes。
2. pinned files。
3. 最近 session 摘要。
4. raw log tail。
5. git status。
6. git diff summary。
7. 最近修改文件列表。
8. 当前 project description。

### 7.3 Handoff 禁止项

禁止：

1. 自动长期记忆。
2. embedding。
3. 全仓库索引。
4. 云端同步。
5. 自动读取敏感文件。
6. 自动发送给 Agent。

### 7.4 默认模式

默认模式是 prefill。

流程：

1. 用户从 Agent A 切换到 Agent B。
2. Marionette 更新 `.marionette/handoff.md`。
3. Marionette 在 composer 预填 handoff prompt。
4. 用户检查。
5. 用户手动发送。

## 8. 视觉和交互目标

### 8.1 视觉参考

参考方向：

1. Zed 的速度感、密度、克制。
2. Palot 的漂亮对话流、tool card、diff card。
3. AgentDeck 的 session 管理直觉。
4. Codeg 的 adapter/session 思路。

不要完整复制任何一个项目。

### 8.2 设计原则

1. 不做花哨营销页。
2. 打开就是工作台。
3. 状态要清晰。
4. 面板密度适中。
5. 不使用过重动画。
6. 不使用大面积毛玻璃。
7. 不使用高消耗背景特效。
8. 深色主题优先。
9. 字体清楚。
10. 输出区域要耐看。

### 8.3 用户必须始终知道

1. 当前项目是什么。
2. 当前 Agent 是什么。
3. 当前 session 状态是什么。
4. 当前看的是 Clean View 还是 Raw Terminal。
5. 哪些上下文会被 handoff。
6. 哪些输入已经发送。
7. 哪些文件发生了变化。
8. 如何回到原厂 Raw Terminal。

## 9. 里程碑

### Milestone 0：产品规格冻结

目标：

1. 明确产品边界。
2. 明确首批 Agent。
3. 明确 Windows-only。
4. 明确 MVP 不做事项。

验收：

1. 策划案完成。
2. 程序设计文档完成。
3. 施工文档完成。

### Milestone 1：可启动壳

目标：

1. 桌面 app 能启动。
2. 有基本布局。
3. 有项目列表和 session 列表占位。

验收：

1. 用户能打开 app。
2. 用户能看到主工作台。
3. UI 不像 demo landing page。

### Milestone 2：项目和 Agent 配置

目标：

1. 能登记本地项目目录。
2. 能持久化项目 metadata。
3. 能读取四个默认 Agent 配置。
4. 能检查 Agent command 是否存在。

验收：

1. 用户可以添加一个本地目录为项目。
2. 重启后项目仍然存在。
3. 可以看到 OpenCode、Codex CLI、Claude Code、Grok Build。
4. 未安装的 command 显示清楚状态，不导致 app 崩溃。

### Milestone 3：Raw Terminal 可用

目标：

1. 能启动任意命令。
2. 能显示终端。
3. 能输入。
4. 能停止进程。

验收：

1. 能启动 `powershell`。
2. 能启动 `cmd`。
3. 能输入 `dir` 并看到输出。
4. 能停止 session。

### Milestone 4：多 Session 管理

目标：

1. 多个 session 可以同时存在。
2. 切换 session 不杀进程。
3. 每个 session 有独立 raw log。

验收：

1. 启动 Claude 和 Codex 两个 session。
2. 在两者之间切换。
3. Raw Terminal 状态保持。

界面约定：左侧列表是全部项目和历史 session；中央顶部 tab 只表示当前打开的对话窗口。新建 tab 会创建一个新的 session，关闭 tab 不删除 session，删除只能从左侧 session 行的悬浮操作触发。

边界：M4 负责 session manager、并行 PTY 生命周期和输出恢复，不负责 ACP 或 provider-specific model/usage 能力。ACP 从 M5 的 Agent adapter 进入，跨 Agent 继续工作由 M6 handoff 负责。

### Milestone 5：首批 Agent 可启动

目标：

1. ACP-capable Agent 使用统一 ACP client 启动、创建 session、发送 prompt 和接收事件。
2. OpenCode 使用原生 ACP；Codex CLI 和 Claude Code 使用可配置的 ACP adapter command。
3. 没有 ACP 入口的 Agent 保留 Generic PTY fallback，并显示真实 transport 状态。

验收：

1. OpenCode ACP `initialize` 和 `session/new` 可完成。
2. ACP prompt、cancel 和 `session/update` 事件能进入当前 Session。
3. adapter command 或认证不可用时显示清楚错误，不导致 app 崩溃。
4. Generic PTY 仍可作为非 ACP Agent 的 fallback。

边界：M5 完成 ACP client 和首批 adapter 接入，不保证每个 Agent 都原生支持 ACP，也不把 provider 的模型、认证、用量和权限能力伪装成统一能力。Claude Code、Grok Build 等 Agent 的实际 ACP 状态由对应 adapter 或原生命令决定。

### Milestone 6：Handoff 可用

目标：

1. 生成 `.marionette/handoff.md`。
2. 切换 Agent 时预填 prompt。
3. 用户可以编辑后手动发送。

验收：

1. Handoff 文件内容可读。
2. Composer 显示预填内容。
3. 不会自动发送。

### Milestone 7：Clean View v0

目标：

1. Clean View 能显示 raw chunk 分组。
2. 能显示用户输入记录。
3. 能显示 handoff 事件。

验收：

1. Clean/Raw 切换不重启进程。
2. Clean View 失败时 Raw Terminal 仍可用。

### Milestone 8：OpenCode 增强

目标：

1. OpenCode 使用更结构化的 server/API/SSE。
2. Clean View 优先支持 OpenCode message/event。

验收：

1. OpenCode session 在 Raw Terminal 可用。
2. OpenCode Clean View 比 Generic PTY 更清楚。

### Milestone 9：体验和性能打磨

目标：

1. UI 主题和密度打磨。
2. 输出批处理优化。
3. 虚拟列表。
4. 错误状态。
5. 基本打包。

验收：

1. 大量输出不卡 UI。
2. 视觉接近 Zed 的克制方向。
3. 用户能稳定完成多 Agent 切换。

### Milestone 10：Workspace Awareness v0

目标：

1. 显示项目文件树。
2. 显示本次改动清单。
3. 显示 git changed files。
4. 支持打开文件和 Copy Path。
5. 支持 Pin to Handoff。

验收：

1. 文件树不扫描 `node_modules`、`.git`、`dist`、`build`、`target`。
2. Changed Files 可手动刷新。
3. 不内置代码编辑器。
4. 不引入全文索引。

## 10. 成功标准

第一版成功标准：

1. 用户愿意把多个终端窗口换成 Marionette。
2. 用户觉得界面足够漂亮。
3. 用户觉得切换 Agent 比原来快。
4. 用户觉得 Raw Terminal 可靠。
5. 大量输出时 app 不冻结。
6. 用户能清楚知道 handoff 发送了什么。

## 11. 失败信号

出现以下情况说明方向偏了：

1. 设置页越来越复杂。
2. 开始管理 API key 和模型。
3. 开始做 marketplace。
4. 开始做自己的 Agent runtime。
5. Clean View 坏了导致 session 不能用。
6. 输出一多 UI 卡死。
7. 用户找不到 Raw Terminal。
8. 用户不知道输入到底发给了谁。
9. 用户觉得它像一个臃肿 IDE。
