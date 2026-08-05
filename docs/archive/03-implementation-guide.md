# Marionette 施工文档

## 0. 文档目标

本文档用于指导弱模型或初级执行者一步一步实现 Marionette。

层级定义：

1. Milestone：最大交付目标。
2. Phase：用户可以检查情况的闭合阶段。
3. Step：执行者必须逐步完成的具体动作。

执行要求：

1. 不自行扩大范围。
2. 每个 Phase 完成后都要能运行或检查。
3. 每个 Step 只做一类事情。
4. 不做 macOS/Linux。
5. 不做 Agent 平台功能。
6. 不做自动发送 handoff。

## 1. 总体里程碑

当前执行口径：M1 已完成，M2“项目和 Agent 配置”已完成，M3“Raw Terminal”核心链路已接通，M4 的多 Session UI 与后端 SessionManager 核心已接通，M5 ACP client 与首批 adapter 已接通。产品策划案与本施工文档使用同一编号。M2 不包含 PTY、ACP 或 provider quota；Usage 面板只展示已知的 provider 窗口，实际百分比在对应 adapter 接入后填充，未连接时显示 `Unavailable`。

当前验收记录（2026-07-12）：

1. 已安装 Rust stable。
2. `cargo test`：2 个后端回归测试通过，覆盖项目添加、重启持久化、项目目录初始化和 PATH 命令探测。
3. `cargo check`：通过。
4. `npm run build`：通过。
5. `npm run tauri -- dev`：已启动，原生窗口 `Marionette` 正常运行。
6. M3 核心已实现：Windows PowerShell PTY、xterm.js 输出、键盘输入、resize、停止和退出事件。
7. M3 尚需在原生窗口手动验收：输入 `dir`、确认输出，再点击停止按钮。
8. M4 UI 基线已实现：多对话 Tab、Tab 新建/关闭、左侧 session 删除、Agent/模式/模型/强度控件和右侧 Information 面板布局。
9. M4 后端核心已实现：Session metadata 持久化、live session manager、独立 raw log、最近 1MB terminal snapshot、list/create/delete session commands。
10. M4 尚需原生窗口手动验收：启动两个 session，切换时不杀进程，切回后能看到此前输出。
11. M5 ACP 核心已实现：stdio JSON-RPC transport、`initialize`、`session/new`、`session/prompt`、`session/cancel` 和 ACP event stream。
12. 本机 OpenCode 1.17.18 的 ACP `initialize` 探测通过；Codex/Claude 使用 npx adapter 配置，实际认证和安装状态需在本机验收。

### Milestone 0：文档和范围冻结

交付：

1. 产品策划案。
2. 程序设计文档。
3. 施工文档。

完成标准：

1. 三份文档存在于 `docs/`。
2. 文档明确 Windows-only。
3. 文档明确首批支持 OpenCode、Codex CLI、Claude Code、Grok Build。
4. 文档明确不做 Agent 平台。

### Milestone 1：可打开的桌面壳

交付：

1. Tauri app skeleton。
2. 主窗口。
3. 基本布局。
4. 假数据项目和 session 列表。

完成标准：

1. `npm run tauri dev` 或等效命令能打开 app。
2. app 首屏就是工作台。
3. 没有 landing page。

### Milestone 2：项目和 Agent 配置

交付：

1. 项目列表。
2. 添加项目。
3. Agent 默认配置。
4. Agent command 测试。

完成标准：

1. 可以添加一个本地目录为项目。
2. 可以看到默认四个 Agent。
3. 可以检查 command 是否存在。

### Milestone 3：Raw Terminal MVP

交付：

1. Rust backend 启动 PTY。
2. xterm.js 显示输出。
3. Raw Terminal 输入。
4. stop session。

完成标准：

1. 能启动 `powershell`。
2. 能输入 `dir` 并看到输出。
3. 能停止 session。

### Milestone 4：多 Agent Session

交付：

1. session manager。
2. 多 session。
3. 切换 session。
4. raw log 保存。
5. 中央顶部对话 tab：新建和关闭窗口不等于删除左侧 session。

边界：M4 只解决 session 生命周期、并行 PTY、输出日志和恢复；不在此阶段接入 ACP。M5 为每个 Agent 建立 adapter/connection，M6 再实现跨 Agent handoff。

完成标准：

1. 可以启动两个 session。
2. 切换 session 不杀进程。
3. `.marionette/sessions/*.raw.log` 有内容。

### Milestone 5：首批 Agent 可启动

交付：

1. OpenCode config。
2. Codex CLI config。
3. Claude Code config。
4. Grok Build config。
5. GenericPtyAdapter。

完成标准：

1. 已安装的 Agent 可以被启动。
2. 未安装的 Agent 显示清楚错误。
3. 发送 prompt 可用。
4. ACP Agent 使用 stdio JSON-RPC；非 ACP Agent 使用 Generic PTY。

### Milestone 6：Handoff v0

交付：

1. `.marionette/handoff.md`。
2. user notes。
3. pinned files。
4. git changed files。
5. composer prefill。

完成标准：

1. 切换 Agent 时可以生成 handoff。
2. Composer 出现预填 prompt。
3. 不自动发送。

### Milestone 7：Clean View v0

交付：

1. Clean/Raw toggle。
2. Clean raw chunk cards。
3. user message cards。
4. handoff event cards。

完成标准：

1. Clean/Raw 切换不重启 session。
2. Clean View 出错时 Raw Terminal 仍可用。

### Milestone 8：OpenCode Clean 增强

交付：

1. OpenCode hybrid adapter。
2. OpenCode event/message 读取。
3. OpenCode Clean View 优先增强。

完成标准：

1. OpenCode Raw Terminal 仍可用。
2. OpenCode Clean View 信息更结构化。

### Milestone 9：体验和性能打磨

交付：

1. UI 主题。
2. 输出批处理优化。
3. 虚拟列表。
4. 错误状态。
5. 基本打包。

完成标准：

1. 大量输出不卡 UI。
2. 视觉接近 Zed/Palot 的克制美观方向。
3. 用户能稳定完成多 Agent 切换。

### Milestone 10：Workspace Awareness v0

交付：

1. 轻量文件树。
2. Changed Files 增强。
3. 点击文件用系统默认 app 打开。
4. Open in Zed / Cursor / VS Code。
5. Copy Path。
6. Pin to Handoff。
7. 简单 git log。
8. Session activity timeline。

完成标准：

1. 文件树懒加载。
2. 默认忽略重目录。
3. 文件可以打开。
4. 文件可以 pin 到 handoff。
5. 不内置代码编辑器。
6. 不做全文索引。

## 2. Milestone 1：可打开的桌面壳

### Phase 1.1：创建 Tauri 项目

目标：

创建最小可运行 Tauri app。

Steps：

1. 检查当前仓库是否为空。
2. 初始化前端项目，推荐 Vite + React + TypeScript。
3. 初始化 Tauri 2。
4. 确认 `src-tauri/` 存在。
5. 确认 `src/` 存在。
6. 添加基础 npm scripts。
7. 启动 dev server。
8. 启动 Tauri dev。

验收：

1. 桌面窗口能打开。
2. 窗口标题显示 `Marionette`。
3. 没有报错弹窗。

禁止：

1. 不添加 Agent 逻辑。
2. 不添加数据库。
3. 不添加 PTY。

### Phase 1.2：实现主布局静态版

目标：

实现四区布局。

Steps：

1. 创建 `TopBar.tsx`。
2. 创建 `ProjectShelf.tsx`。
3. 创建 `SessionList.tsx`。
4. 创建 `SessionView.tsx`。
5. 创建 `ContextPanel.tsx`。
6. 创建 `Composer.tsx`。
7. 在 `App.tsx` 中组合这些组件。
8. 添加 `tokens.css`。
9. 添加 `app.css`。
10. 使用假数据显示项目和 session。

验收：

1. 左侧有项目和 session。
2. 中间有 Clean/Raw 占位。
3. 右侧有 Changed Files/Handoff 占位。
4. 底部有输入框。
5. 界面不是营销页。

设计要求：

1. 深色主题优先。
2. 边框克制。
3. 字号不要过大。
4. 不使用大面积毛玻璃。
5. 不使用装饰性背景动画。

### Phase 1.3：添加基础类型

目标：

前端先定义核心类型。

Steps：

1. 创建 `src/lib/types.ts`。
2. 添加 `Project` 类型。
3. 添加 `AgentConfig` 类型。
4. 添加 `Session` 类型。
5. 添加 `SessionEvent` 类型。
6. 添加 `ChangedFile` 类型。
7. 添加 `HandoffResult` 类型。
8. 替换组件中的临时 any。

验收：

1. TypeScript 无类型错误。
2. 组件使用统一类型。

## 3. Milestone 2：项目和 Agent 配置

### Phase 2.1：后端路径服务

目标：

实现全局和项目内路径生成。

Steps：

1. 在 `src-tauri/src/storage/paths.rs` 创建路径模块。
2. 实现获取 `%USERPROFILE%\.marionette`。
3. 实现创建全局目录。
4. 实现项目 `.marionette` 路径。
5. 实现项目 `sessions` 路径。
6. 实现项目 `transcripts` 路径。
7. 添加路径创建函数。
8. 添加错误信息。

验收：

1. 启动 app 后能创建 `%USERPROFILE%\.marionette`。
2. 添加项目后能创建项目 `.marionette`。

### Phase 2.2：项目 metadata 存储

目标：

实现项目添加和列表。

Steps：

1. 决定第一版存储方式。
2. 如果 SQLite 尚未准备好，可以先用 `%USERPROFILE%\.marionette\projects.json`。
3. 定义 Rust `Project` struct。
4. 实现 `list_projects`。
5. 实现 `add_project`。
6. `add_project` 检查路径是否存在。
7. `add_project` 检查路径是否目录。
8. `add_project` 创建项目 `.marionette`。
9. 前端调用 `list_projects`。
10. 前端添加项目按钮可以先用输入框。

验收：

1. 用户输入路径后能添加项目。
2. 重启 app 后项目仍存在。
3. 无效路径显示错误。

### Phase 2.3：默认 Agent 配置

目标：

显示首批四个 Agent。

Steps：

1. 创建后端 `adapters` 模块。
2. 定义 `AgentConfig` struct。
3. 添加默认配置：OpenCode。
4. 添加默认配置：Codex CLI。
5. 添加默认配置：Claude Code。
6. 添加默认配置：Grok Build。
7. 实现 `list_agents` command。
8. 前端调用 `list_agents`。
9. 左侧或顶部显示 Agent switcher。

验收：

1. UI 能看到四个 Agent。
2. 每个 Agent 有 label 和 command。

### Phase 2.4：Agent command 测试

目标：

用户能知道某个 Agent 是否安装。

Steps：

1. 实现 `test_agent_command(agentId)`。
2. 后端查找 command 是否可执行。
3. Windows 下可以尝试运行 `{command} --version`，设置短超时。
4. 如果失败，返回 `not_found` 或 `failed`。
5. 前端显示 installed/missing/unknown 状态。

验收：

1. 未安装命令不会导致 app 崩溃。
2. 用户看到清楚错误。

## 4. Milestone 3：Raw Terminal MVP

### Phase 3.1：PTY 抽象

目标：

后端建立 PTY 服务接口。

Steps：

1. 创建 `src-tauri/src/pty/mod.rs`。
2. 创建 `pty_service.rs`。
3. 定义 `PtyCommand`。
4. 定义 `PtyHandle`。
5. 定义 `PtyService` 接口或等效 struct 方法。
6. 添加 `spawn` 方法占位。
7. 添加 `write` 方法占位。
8. 添加 `resize` 方法占位。
9. 添加 `kill` 方法占位。
10. 编译通过。

验收：

1. Rust 编译通过。
2. 暂时不要求真的启动 PTY。

### Phase 3.2：实现 Windows PTY spawn

目标：

启动真实进程。

Steps：

1. 选择一个 Windows 可用 PTY 实现。
2. 添加依赖。
3. 实现 `spawn(command)`。
4. 支持 cwd。
5. 支持 args。
6. 支持 cols/rows。
7. 返回 pty id。
8. 保存 writer。
9. 保存 child/process handle。
10. 启动 reader 后台任务。

验收：

1. 能启动 `powershell`。
2. 后端能收到输出 bytes。

### Phase 3.3：Backend streaming 到前端

目标：

PTY output 能发送到前端。

Steps：

1. 定义 `pty_output` event 或 channel。
2. reader 读取 bytes 后进入 channel。
3. 实现 16ms 或 16KB batch。
4. 每批发送 `{ sessionId, data }`。
5. 前端订阅 output。
6. 前端暂时打印到 console。

验收：

1. 启动 powershell 后前端 console 能看到输出。
2. 大输出不会产生每字符一个事件。

### Phase 3.4：RawTerminal 组件

目标：

xterm.js 显示 PTY 输出。

Steps：

1. 安装 xterm.js。
2. 创建 `RawTerminal.tsx`。
3. 在组件 mount 时创建 Terminal。
4. 绑定到 DOM。
5. 收到 backend output 时写入 xterm。
6. xterm `onData` 时调用后端写入。
7. 支持 fit addon。
8. resize 时调用后端 `resize_session`。

验收：

1. 界面能显示 powershell prompt。
2. 输入 `dir` 有输出。
3. 调整窗口后 terminal 不严重错位。

### Phase 3.5：Session start/stop commands

目标：

用户可以启动和停止 session。

Steps：

1. 定义 `create_session`。
2. 定义 `start_session`。
3. 定义 `stop_session`。
4. 前端添加 Start 按钮。
5. 前端添加 Stop 按钮。
6. 启动时状态变为 `starting`。
7. 成功后状态变为 `running`。
8. 停止后状态变为 `exited`。

验收：

1. 点击 Start 启动 powershell。
2. 点击 Stop 停止进程。
3. UI 状态同步变化。

## 5. Milestone 4：多 Agent Session

### Phase 4.1：SessionManager

目标：

集中管理多个 session。

Steps：

1. 创建 `sessions/session_manager.rs`。
2. 创建 `sessions/session_model.rs`。
3. 定义 Rust `Session` struct。
4. 使用 map 保存 live sessions。
5. `create_session` 只创建 metadata。
6. `start_session` 启动 PTY。
7. `list_sessions(project_id)` 返回 metadata。
8. `stop_session` 只停止目标 session。
9. 进程退出时更新 status。

验收：

1. 同一项目可以有多个 session。
2. 停止一个 session 不影响另一个。

### Phase 4.2：Session 切换

目标：

切换 session 不杀进程。

Steps：

1. 前端维护 `currentSessionId`。
2. SessionList 点击 session 时只切换当前 id。
3. RawTerminal 根据 sessionId 订阅输出。
4. 不调用 stop。
5. 切换后显示该 session 最近 ring buffer。

验收：

1. session A 执行长命令。
2. 切到 session B。
3. 切回 session A。
4. session A 没有被杀。

### Phase 4.3：Raw log 保存

目标：

保存完整原始输出。

Steps：

1. 创建 `sessions/log_writer.rs`。
2. session 启动时创建 `.marionette/sessions`。
3. 生成 raw log 文件名。
4. OutputTee 每批输出写入 raw log。
5. 写入失败产生 warning。
6. session metadata 保存 rawLogPath。

验收：

1. 启动 session 后生成 raw log。
2. 输入命令后 raw log 文件变大。
3. 重启 app 后 log 文件仍存在。

### Phase 4.4：Ring buffer

目标：

保存最近输出，方便切换显示。

Steps：

1. 创建 `pty/ring_buffer.rs`。
2. 每个 session 保存最近 1MB 输出。
3. 新 RawTerminal mount 时请求最近输出。
4. 前端先写入 snapshot。
5. 再接收 live output。

验收：

1. 切换回 session 后能看到最近输出。
2. 不需要读取完整 raw log。

## 6. Milestone 5：首批 Agent 可启动

### Phase 5.1：GenericPtyAdapter

目标：

统一启动任意 Agent command。

Steps：

1. 创建 `adapters/agent_adapter.rs`。
2. 定义 `AgentAdapter` trait 或 struct。
3. 创建 `adapters/generic_pty.rs`。
4. 实现 `build_command`。
5. 实现 `prepare_input`。
6. 默认使用 bracketed paste。
7. fallback 支持 stdin。
8. 注册到 AdapterRegistry。

验收：

1. 用 GenericPtyAdapter 启动 powershell。
2. 用 GenericPtyAdapter 启动任意配置 command。

### Phase 5.2：OpenCode 配置

目标：

OpenCode 能通过 Generic PTY 启动。

Steps：

1. AgentConfig 添加 `opencode`。
2. command 为 `opencode`。
3. args 默认为空。
4. launchMode 为 `pty`。
5. sendStrategy 为 `bracketed-paste`。
6. UI 可选择 OpenCode。
7. 启动失败时显示 command not found。

验收：

1. 已安装 OpenCode 时能启动。
2. 未安装时错误清楚。

### Phase 5.3：Codex CLI 配置

目标：

Codex CLI 能通过 Generic PTY 启动。

Steps：

1. AgentConfig 添加 `codex`。
2. command 为 `codex`。
3. args 默认为空。
4. launchMode 为 `pty`。
5. sendStrategy 为 `bracketed-paste`。
6. UI 可选择 Codex CLI。

验收：

1. 已安装 Codex CLI 时能启动。
2. 能输入 prompt。

### Phase 5.4：Claude Code 配置

目标：

Claude Code 能通过 Generic PTY 启动。

Steps：

1. AgentConfig 添加 `claude-code`。
2. command 为 `claude`。
3. args 默认为空。
4. launchMode 为 `pty`。
5. sendStrategy 为 `bracketed-paste`。
6. UI 可选择 Claude Code。

验收：

1. 已安装 Claude Code 时能启动。
2. 能输入 prompt。

### Phase 5.5：Grok Build 配置

目标：

Grok Build 能通过 Generic PTY 启动。

Steps：

1. AgentConfig 添加 `grok-build`。
2. command 为 `grok`。
3. args 为 `["build"]`。
4. launchMode 为 `pty`。
5. sendStrategy 为 `bracketed-paste`。
6. UI 可选择 Grok Build。

验收：

1. 已安装 Grok CLI 且支持 build 时能启动。
2. 不支持时显示原始错误。

### Phase 5.6：Composer 发送 prompt

目标：

从 GUI composer 发送输入到当前 Agent。

Steps：

1. Composer 获取当前 session。
2. 用户输入文本。
3. 点击 Send。
4. 调用 `write_session_input`。
5. 后端调用 adapter `prepare_input`。
6. 写入 PTY。
7. 记录 `user_message` event。
8. 清空 composer。

验收：

1. 多行 prompt 能发送。
2. 发送内容在 Clean View 事件里可见。
3. Raw Terminal 能看到 Agent 响应。

禁止：

1. 不自动发送。
2. 不在 Raw Terminal 聚焦时截获普通键盘输入。

## 7. Milestone 6：Handoff v0

### Phase 6.1：项目 handoff 文件

目标：

生成 `.marionette/handoff.md`。

Steps：

1. 创建 `handoff/handoff_model.rs`。
2. 创建 `handoff/handoff_builder.rs`。
3. 定义 handoff sections。
4. 包含 Current Task。
5. 包含 Last Active Agent。
6. 包含 Important Files。
7. 包含 Recent Changes。
8. 包含 User Notes。
9. 包含 Suggested Next Prompt。
10. 写入项目 `.marionette/handoff.md`。

验收：

1. 点击 Generate Handoff 后生成文件。
2. 文件内容是 Markdown。
3. 文件可读。

### Phase 6.2：User notes

目标：

用户可以写 handoff notes。

Steps：

1. HandoffPanel 添加 notes textarea。
2. 保存 notes 到项目 metadata 或 `.marionette/state.json`。
3. 生成 handoff 时包含 notes。
4. 重启 app 后 notes 仍在。

验收：

1. 用户输入 notes。
2. 生成 handoff 后能在文件中看到 notes。

### Phase 6.3：Pinned files

目标：

用户可以 pin 文件到 handoff。

Steps：

1. ChangedFiles 每个文件添加 Pin 按钮。
2. Pin 后保存路径。
3. ContextPanel 显示 Pinned Files。
4. 可以 unpin。
5. 生成 handoff 时列出 pinned files。

验收：

1. Pin 文件后 handoff 包含该路径。
2. Unpin 后 handoff 不再包含。

### Phase 6.4：Git changed files

目标：

右侧显示 changed files。

Steps：

1. 实现 `get_changed_files(projectId)`。
2. 后端运行 `git status --porcelain`。
3. 解析 added/modified/deleted。
4. 前端显示列表。
5. 非 git repo 显示 empty state。
6. 添加 Refresh 按钮。

验收：

1. 修改一个文件后 Refresh 能看到。
2. 非 git repo 不报崩溃。

### Phase 6.5：切换 Agent 时 prefill

目标：

切换 Agent 后预填 handoff prompt。

Steps：

1. 用户选择目标 Agent。
2. 前端调用 `generate_handoff(projectId, targetAgentId)`。
3. 后端更新 handoff.md。
4. 后端返回 suggested prompt。
5. Composer 显示 suggested prompt。
6. Composer 标记为 handoff prefill。
7. 用户可以编辑。
8. 用户手动点击 Send。

验收：

1. 切换 Agent 后 composer 出现内容。
2. 内容未自动发送。
3. 用户可以修改后发送。

## 8. Milestone 7：Clean View v0

### Phase 7.1：Clean/Raw toggle

目标：

同一 session 可切 Clean/Raw。

Steps：

1. TopBar 添加 View Toggle。
2. 状态值为 `clean` 或 `raw-terminal`。
3. 切换只改变 UI。
4. 不调用 start。
5. 不调用 stop。
6. 不清空 xterm。

验收：

1. 切换视图时进程仍运行。
2. 切回 Raw Terminal 后还能输入。

### Phase 7.2：SessionEvent 存储

目标：

记录 user_message 和 raw_chunk。

Steps：

1. 创建 transcript jsonl 文件。
2. 发送 prompt 时写 user_message。
3. parser 产生 raw_chunk 时写 raw_chunk。
4. 前端维护 event list。
5. CleanTranscript 读取 event list。

验收：

1. Clean View 能看到用户发送过的内容。
2. 能看到 raw output cards。

### Phase 7.3：ansi-raw parser

目标：

将 raw output 粗略显示为 Clean cards。

Steps：

1. 创建 parser 模块。
2. 输入 raw chunk。
3. 去掉常见 ANSI escape。
4. 合并过短 chunk。
5. 按时间或空行切卡。
6. 输出 raw_chunk event。
7. parser 错误只记录 warning。

验收：

1. Clean View 有可读输出。
2. parser 报错时 Raw Terminal 不受影响。

### Phase 7.4：Clean View UI

目标：

Clean View 看起来像漂亮的消息流。

Steps：

1. user_message 右侧或用户样式。
2. raw_chunk 使用 assistant 样式。
3. handoff_prepared 使用系统卡片。
4. 长文本折叠。
5. 大量事件使用虚拟列表。
6. 空状态显示当前 session 信息。

验收：

1. Clean View 不丑。
2. 大量消息不卡。
3. 用户能区分输入和输出。

## 9. Milestone 8：OpenCode Clean 增强

### Phase 8.1：研究 OpenCode 本地接口

目标：

确认当前安装版本支持什么接口。

Steps：

1. 运行 `opencode --help`。
2. 查看是否有 server 模式。
3. 查看 server 端口配置。
4. 查看是否有 session API。
5. 查看是否有 event stream。
6. 记录结果到 `docs/research/opencode.md`。

验收：

1. 文档记录可用命令。
2. 文档记录 API 或不可用原因。

### Phase 8.2：OpenCodeAdapter skeleton

目标：

创建专用 adapter，但不破坏 PTY。

Steps：

1. 创建 `adapters/opencode.rs`。
2. 继承或组合 GenericPtyAdapter。
3. 保持 PTY 启动可用。
4. 添加 optional server connection 字段。
5. 如果 server 不可用，fallback 到 PTY。

验收：

1. OpenCode 仍能 Raw Terminal 启动。
2. server 失败不影响 Raw Terminal。

### Phase 8.3：连接 OpenCode events

目标：

读取结构化事件。

Steps：

1. 实现 server URL 配置。
2. 建立 HTTP/SSE 客户端。
3. 读取 events。
4. 转换为 SessionEvent。
5. 写 transcript jsonl。
6. 前端显示更清楚的消息。

验收：

1. OpenCode Clean View 不只显示 raw_chunk。
2. Tool/file/message 至少一种结构化事件显示成功。

### Phase 8.4：OpenCode composer 增强

目标：

如果 OpenCode API 支持 prompt append/submit，则使用协议发送。

Steps：

1. 判断当前 OpenCode session 是否可用 API。
2. 如果可用，Composer 发送走 HTTP。
3. 如果不可用，fallback bracketed paste。
4. UI 显示当前 send strategy。

验收：

1. API 发送成功时 Raw Terminal 同步。
2. API 失败时可以 fallback。

## 10. Milestone 9：体验和性能打磨

### Phase 9.1：输出性能测试

目标：

验证大量输出不卡。

Steps：

1. 启动 powershell session。
2. 执行会产生大量输出的命令。
3. 观察 UI 是否冻结。
4. 检查 frontend output batch。
5. 检查 raw log 写入。
6. 如果卡顿，减少 React state 更新。
7. 如果事件太多，增大 batch。

验收：

1. 大量输出时窗口仍可操作。
2. Raw Terminal 持续刷新。
3. raw log 完整写入。

### Phase 9.2：视觉打磨

目标：

达到克制、漂亮、快速的视觉体验。

Steps：

1. 统一 spacing token。
2. 统一 font size。
3. 统一 border color。
4. 统一 panel background。
5. 优化 active session 状态。
6. 优化 Agent 状态 badge。
7. 优化 Composer。
8. 优化 Clean cards。
9. 优化 Changed Files。

验收：

1. 首屏像工作台。
2. 信息密度合理。
3. 没有大面积装饰背景。
4. 没有文字溢出。

### Phase 9.3：错误状态

目标：

常见错误都有清晰显示。

Steps：

1. command not found 状态。
2. cwd not found 状态。
3. PTY spawn failed 状态。
4. process exited 状态。
5. log write warning。
6. parser unavailable。
7. git not repo。
8. Zed not found。

验收：

1. 错误不会白屏。
2. 用户知道下一步该检查什么。

### Phase 9.4：Open in Zed

目标：

从 Marionette 打开项目或文件到 Zed。

Steps：

1. 实现 `open_project_in_zed`。
2. 后端运行 `zed {projectRoot}`。
3. 实现 `open_file_in_zed`。
4. 后端运行 `zed {filePath}`。
5. 如果 zed 不存在，返回 fallback。
6. 前端显示复制命令按钮。

验收：

1. 安装 Zed 时能打开项目。
2. 未安装 Zed 时不崩溃。

## 11. Milestone 10：Workspace Awareness v0

### Phase 10.1：WorkspaceService skeleton

目标：

后端建立工作区文件和打开操作的服务边界。

Steps：

1. 创建 `workspace` 或 `files` 后端模块。
2. 定义 `FileTreeNode`。
3. 定义 `OpenPathResult`。
4. 定义默认 ignore 列表。
5. 实现路径安全检查。
6. 确保传入路径必须位于 project root 内。
7. 添加 `get_file_tree` command 占位。
8. 添加 `open_path_default` command 占位。
9. 添加 `reveal_path` command 占位。
10. 编译通过。

验收：

1. Rust 编译通过。
2. command 存在但可以先返回空数据。
3. 路径越界会返回错误。

### Phase 10.2：轻量文件树

目标：

显示项目文件树，不扫描重目录。

Steps：

1. 实现 `get_file_tree(projectId, directory?)`。
2. 默认读取项目根目录。
3. 展开目录时读取该目录。
4. 忽略 `.git`。
5. 忽略 `node_modules`。
6. 忽略 `dist`。
7. 忽略 `build`。
8. 忽略 `target`。
9. 单目录最多返回 500 项。
10. 返回 file/dir/name/path/absolutePath。
11. 前端创建 `FileTreePanel.tsx`。
12. ContextPanel 增加 File Tree tab。
13. 展开目录时调用 backend。

验收：

1. 用户能看到项目根目录文件。
2. 展开目录能加载子项。
3. `node_modules` 不显示或显示为 ignored。
4. 大目录不会卡死 UI。

禁止：

1. 不读取文件内容。
2. 不做全文搜索。
3. 不做代码预览。

### Phase 10.3：文件打开和路径操作

目标：

文件树和 changed files 支持基础操作。

Steps：

1. 实现 `open_path_default`。
2. Windows 下使用系统默认 app 打开文件或目录。
3. 实现 `reveal_path`。
4. Windows 下使用 Explorer reveal。
5. 实现 `open_path_in_editor`。
6. 支持 `zed`。
7. 支持 `cursor`。
8. 支持 `code`。
9. 命令不存在时返回 fallback。
10. 前端文件右键菜单添加 Open with Default App。
11. 前端文件右键菜单添加 Reveal in Explorer。
12. 前端文件右键菜单添加 Open in Zed/Cursor/VS Code。
13. 前端文件右键菜单添加 Copy Path。

验收：

1. 点击文件能用系统默认 app 打开。
2. Reveal 能打开 Explorer。
3. 未安装编辑器时显示 fallback，不崩溃。
4. Copy Path 能复制绝对路径。

### Phase 10.4：Changed Files 增强

目标：

Changed Files 面板更接近可用工作区侧栏。

Steps：

1. 复用 `get_changed_files(projectId)`。
2. 显示 modified/added/deleted/untracked 状态。
3. 对路径按目录分组。
4. 每个文件显示状态 badge。
5. 每个文件提供 Open。
6. 每个文件提供 Diff。
7. 每个文件提供 Copy Path。
8. 每个文件提供 Pin to Handoff。
9. 添加 Refresh 按钮。
10. 非 git repo 显示 empty state。

验收：

1. 修改文件后 Refresh 能看到。
2. 点击文件能打开。
3. 点击 Diff 能看到单文件 diff。
4. Pin 后 handoff 包含该文件。

禁止：

1. 不做 commit。
2. 不做 discard。
3. 不做 stage/unstage。

### Phase 10.5：简单 git log

目标：

显示轻量 git 修改历史。

Steps：

1. 实现 `get_git_log(projectId, limit)`。
2. 后端运行 `git log --oneline --decorate -n {limit}`。
3. 默认 limit 为 30。
4. 解析 hash 和 subject。
5. 前端创建 Git Log tab。
6. 非 git repo 显示 empty state。
7. 点击 commit 第一版只复制 hash 或显示详情占位。

验收：

1. git repo 中能看到最近 commit。
2. 非 git repo 不报错。
3. 不实现 commit graph。

### Phase 10.6：Session activity timeline

目标：

展示本次对话或 session 发生了什么。

Steps：

1. 定义 `SessionActivityEvent`。
2. session start 时记录 event。
3. user message sent 时记录 event。
4. handoff prepared 时记录 event。
5. changed files refreshed 时记录 event。
6. file pinned 时记录 event。
7. session exited 时记录 event。
8. 创建 `ActivityTimeline.tsx`。
9. ContextPanel 增加 Activity tab。
10. 事件过多时折叠。

验收：

1. 用户能看到本 session 的关键操作。
2. 不显示完整 raw output。
3. 点击文件相关 event 能打开或定位文件。

### Phase 10.7：本次改动清单

目标：

让 app 能像 Codex 回答一样告诉用户“本次改了哪些文档/文件”。

Steps：

1. 基于 git changed files 生成当前改动清单。
2. 基于 session activity 标注文件来源。
3. 在 ContextPanel 显示 `Changed in this session`。
4. 每个文件显示 path/status。
5. 每个文件支持 Open。
6. 每个文件支持 Diff。
7. 每个文件支持 Pin to Handoff。
8. 如果无法判断 session 来源，只显示 project changed files。

验收：

1. 当前项目改动能显示出来。
2. 用户能点击文件打开。
3. 用户能把文件加入 handoff。

## 12. 弱模型执行规则

### 11.1 每次只做一个 Phase

执行者一次最多实现一个 Phase。

完成后必须报告：

1. 改了哪些文件。
2. 实现了什么。
3. 如何运行。
4. 如何验收。
5. 哪些未完成。

### 11.2 不允许跳过验收

每个 Phase 完成后必须至少运行：

1. TypeScript 检查。
2. Rust 编译检查。
3. app 启动检查。

如果当前 Phase 不涉及某项，可以说明原因。

### 11.3 不允许范围蔓延

执行者不得在未要求时添加：

1. API key 管理。
2. 模型选择器。
3. 云同步。
4. 登录系统。
5. marketplace。
6. skills。
7. token/cost。
8. 自动化任务。
9. embedding。
10. RAG。

### 11.4 失败时的处理

如果一个 Step 卡住：

1. 保持已有可运行状态。
2. 写明失败命令。
3. 写明错误输出。
4. 写明已尝试的方法。
5. 不要删除无关文件。
6. 不要重构无关模块。

## 13. 每个 Phase 的通用完成报告模板

执行者完成一个 Phase 后，按以下格式报告：

```text
完成 Phase X.Y：{名称}

已实现：
- ...

修改文件：
- ...

运行/检查：
- ...

验收结果：
- ...

未完成/风险：
- ...

下一步建议：
- Phase X.Y+1：...
```

## 14. 第一轮建议执行顺序

第一轮不要碰复杂 Agent。

建议顺序：

1. Milestone 1 / Phase 1.1
2. Milestone 1 / Phase 1.2
3. Milestone 1 / Phase 1.3
4. Milestone 2 / Phase 2.1
5. Milestone 2 / Phase 2.2
6. Milestone 2 / Phase 2.3
7. Milestone 3 / Phase 3.1
8. Milestone 3 / Phase 3.2
9. Milestone 3 / Phase 3.3
10. Milestone 3 / Phase 3.4

只有 Raw Terminal 跑通后，才开始做 handoff 和 Clean View。
