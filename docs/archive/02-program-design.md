# Marionette 程序设计文档

## 0. 文档目标

本文档描述 Marionette 的程序架构、模块边界、数据结构、接口、线程模型和关键实现策略。

目标读者是可以执行具体实现任务的模型或开发者。本文档要求实现者尽量按模块、接口和验收标准执行，不自行扩大范围。

## 1. 技术路线

### 1.1 平台范围

第一版只支持 Windows。

暂不处理：

1. macOS。
2. Linux。
3. 跨平台安装包。
4. 跨平台 PTY 差异。

### 1.2 推荐技术栈

桌面壳：

1. Tauri 2。
2. Rust backend。
3. React 或 Svelte frontend。
4. xterm.js raw terminal。
5. SQLite 存 metadata。
6. append-only file 存 raw log。

PTY：

1. 优先使用 Windows 可工作的 PTY 方案。
2. 设计上用 `PtyService` 抽象包住具体 crate 或实现。
3. 不让 frontend 直接管理进程。

说明：

1. 不采用 Electron 作为第一选择。
2. 不 fork Zed 做基座。
3. 不 fork Codeg 做基座。
4. 不把 Marionette 做成 web server 平台。

## 2. 总体架构

### 2.1 高层结构

```text
Frontend UI
  ├── Project/Session Sidebar
  ├── Raw Terminal View
  ├── Clean View
  ├── Context Panel
  └── Composer

Tauri Bridge
  ├── commands
  └── event/channel streams

Rust Backend
  ├── AppState
  ├── SessionManager
  ├── PtyService
  ├── OutputTee
  ├── AdapterRegistry
  ├── HandoffService
  ├── GitService
  ├── StorageService
  └── LogWriter

Local Files
  ├── .marionette/handoff.md
  ├── .marionette/sessions/*.raw.log
  ├── .marionette/transcripts/*.jsonl
  └── ~/.marionette/app.db
```

### 2.2 Source of Truth

真实 PTY 是 session 的 source of truth。

Clean View 从 PTY 输出、结构化协议或日志派生。

禁止：

1. Clean View 修改 PTY 状态。
2. parser 崩溃导致 PTY 进程退出。
3. frontend 直接启动 Agent process。
4. frontend 直接写 raw log。

## 3. 核心数据模型

### 3.1 Project

```ts
type Project = {
  id: string
  name: string
  rootPath: string
  createdAt: string
  lastOpenedAt: string
}
```

字段说明：

1. `id`：UUID。
2. `name`：默认使用目录名。
3. `rootPath`：Windows 绝对路径。
4. `createdAt`：ISO 字符串。
5. `lastOpenedAt`：ISO 字符串。

### 3.2 AgentConfig

```ts
type AgentConfig = {
  id: string
  label: string
  command: string
  args: string[]
  cwdMode: "project-root" | "custom"
  customCwd?: string
  launchMode: "pty" | "server" | "hybrid"
  sendStrategy: "stdin" | "bracketed-paste" | "http"
  parser: "ansi-raw" | "opencode-sse" | "none"
  enabled: boolean
}
```

首批默认配置：

```yaml
agents:
  - id: opencode
    label: OpenCode
    command: opencode
    args: []
    launchMode: pty
    sendStrategy: bracketed-paste
    parser: ansi-raw

  - id: codex
    label: Codex CLI
    command: codex
    args: []
    launchMode: pty
    sendStrategy: bracketed-paste
    parser: ansi-raw

  - id: claude-code
    label: Claude Code
    command: claude
    args: []
    launchMode: pty
    sendStrategy: bracketed-paste
    parser: ansi-raw

  - id: grok-build
    label: Grok Build
    command: grok
    args: ["build"]
    launchMode: pty
    sendStrategy: bracketed-paste
    parser: ansi-raw
```

### 3.3 Session

```ts
type Session = {
  id: string
  projectId: string
  agentId: string
  label: string
  cwd: string
  status: "starting" | "running" | "waiting" | "exited" | "error"
  processId: number | null
  ptyId: string | null
  startedAt: string
  lastActiveAt: string
  exitedAt?: string
  exitCode?: number
  rawLogPath: string
  transcriptPath: string
  handoffPath: string
  viewMode: "clean" | "raw-terminal" | "diff" | "logs"
}
```

### 3.4 SessionEvent

```ts
type SessionEvent =
  | {
      type: "user_message"
      sessionId: string
      text: string
      createdAt: string
    }
  | {
      type: "assistant_message"
      sessionId: string
      text: string
      createdAt: string
    }
  | {
      type: "raw_chunk"
      sessionId: string
      text: string
      createdAt: string
    }
  | {
      type: "handoff_prepared"
      sessionId: string
      targetAgentId: string
      handoffPath: string
      prompt: string
      createdAt: string
    }
  | {
      type: "file_change"
      sessionId: string
      path: string
      changeType: "added" | "modified" | "deleted"
      createdAt: string
    }
```

第一版可以只实现：

1. `user_message`
2. `raw_chunk`
3. `handoff_prepared`

## 4. 文件和存储设计

### 4.1 项目内目录

每个项目根目录下创建：

```text
.marionette/
  handoff.md
  state.json
  sessions/
    {agentId}-{timestamp}.raw.log
  transcripts/
    {agentId}-{timestamp}.jsonl
```

要求：

1. 如果 `.marionette/` 不存在，创建。
2. 不覆盖已有 raw log。
3. raw log 文件只 append。
4. transcript jsonl 每行一个 `SessionEvent`。

### 4.2 全局目录

全局目录：

```text
%USERPROFILE%\.marionette\
  config.json
  agents.yaml
  app.db
  cache\
```

### 4.3 SQLite 存什么

SQLite 只存轻量 metadata：

1. projects。
2. sessions metadata。
3. agent configs。
4. recent opened projects。
5. UI state。
6. pinned files。
7. user notes metadata。

### 4.4 SQLite 不存什么

SQLite 不存：

1. raw terminal 大量输出。
2. 完整 transcript 大量文本。
3. 文件内容快照。
4. embedding。
5. token/cost。

## 5. Backend 模块设计

### 5.1 AppState

职责：

1. 保存全局服务实例。
2. 管理共享状态。
3. 提供线程安全访问。

建议结构：

```rust
struct AppState {
    session_manager: Arc<SessionManager>,
    storage: Arc<StorageService>,
    adapters: Arc<AdapterRegistry>,
    handoff: Arc<HandoffService>,
    git: Arc<GitService>,
}
```

实现要求：

1. 所有 Tauri command 通过 AppState 访问业务服务。
2. 不在 command 函数里写复杂业务。
3. 对共享 map 使用锁或 channel，避免数据竞争。

M2 先实现最小存储闭环：

1. 全局目录为 `%USERPROFILE%\\.marionette`。
2. 项目索引为 `%USERPROFILE%\\.marionette\\projects.json`。
3. 每个项目初始化 `.marionette/sessions` 和 `.marionette/transcripts`。
4. Agent 默认配置由后端提供，前端不复制第二份业务配置。
5. command 探测只返回 `installed`、`missing` 或 `failed`，不启动交互式 Agent。

M2 暂不实现 PTY、session manager、ACP 或 provider quota；这些属于后续里程碑。

### 5.2 SessionManager

职责：

1. 创建 session。
2. 启动 session。
3. 停止 session。
4. 重启 session。
5. 列出 session。
6. 更新 session status。
7. 将 PTY 输出连接到 OutputTee。

核心方法：

```rust
create_session(project_id, agent_id) -> Session
start_session(session_id) -> Result
stop_session(session_id) -> Result
restart_session(session_id) -> Result
list_sessions(project_id) -> Vec<Session>
write_to_session(session_id, bytes) -> Result
resize_session(session_id, cols, rows) -> Result
```

实现约束：

1. session 切换不影响进程。
2. stop 只停止目标 session。
3. 输出读取必须在后台任务里运行。
4. 进程退出要更新 status。

### 5.3 PtyService

职责：

1. 启动 PTY。
2. 获取 reader。
3. 获取 writer。
4. resize。
5. kill。

抽象接口：

```rust
trait PtyService {
    fn spawn(&self, command: PtyCommand) -> Result<PtyHandle>;
    fn write(&self, pty_id: &str, bytes: &[u8]) -> Result<()>;
    fn resize(&self, pty_id: &str, cols: u16, rows: u16) -> Result<()>;
    fn kill(&self, pty_id: &str) -> Result<()>;
}
```

`PtyCommand`：

```rust
struct PtyCommand {
    command: String,
    args: Vec<String>,
    cwd: PathBuf,
    env: HashMap<String, String>,
    cols: u16,
    rows: u16,
}
```

Windows 注意：

1. cwd 必须是存在的目录。
2. command 不存在时返回清晰错误。
3. args 不要拼成一个字符串传入 shell，除非用户明确要求 shell 模式。
4. 第一版可以支持 `shellMode`，但默认直接启动 command。

### 5.4 OutputTee

职责：

将同一份 PTY output 分发给：

1. frontend raw terminal。
2. raw log writer。
3. transcript parser。
4. session event bus。

流程：

```text
PTY reader
  -> bounded channel
  -> OutputTee
      -> LogWriter append
      -> FrontendStream batch
      -> ParserWorker enqueue
      -> Activity update
```

实现要求：

1. PTY reader 不直接调用 frontend。
2. PTY reader 不直接做 parser。
3. LogWriter 慢时不能永久阻塞 UI。
4. FrontendStream 要 batch。
5. ParserWorker 可以丢弃过旧任务，但 raw log 不能丢。

建议批处理规则：

1. 每 16ms flush 一次 frontend output。
2. 或累计到 16KB flush。
3. 单次 event/channel payload 不超过合理大小，例如 64KB。
4. 大输出时合并 chunk。

### 5.5 LogWriter

职责：

1. append raw bytes 到 `.raw.log`。
2. append event JSON 到 `.jsonl`。
3. flush 策略可控。

实现要求：

1. raw log 保留原始 bytes 或 utf8-lossy 文本。
2. 文件打开失败要报告到 session error。
3. 写入失败要显示 warning，但不一定杀进程。
4. 不要把 raw log 写入 SQLite。

### 5.6 AdapterRegistry

职责：

1. 加载 agent configs。
2. 根据 agentId 找 adapter。
3. 提供 prepare input。
4. 提供 launch command。

接口：

```rust
trait AgentAdapter {
    fn id(&self) -> &str;
    fn label(&self) -> &str;
    fn build_command(&self, project: &Project) -> PtyCommand;
    fn prepare_input(&self, text: &str, mode: SendMode) -> Vec<u8>;
    fn parser_kind(&self) -> ParserKind;
}
```

`SendMode`：

```rust
enum SendMode {
    Stdin,
    BracketedPaste,
}
```

### 5.7 GenericPtyAdapter

职责：

1. 支持任意命令。
2. 默认 bracketed paste 发送多行 prompt。
3. 支持 fallback 到 stdin。

Bracketed paste 格式：

```text
\x1b[200~
{text}
\x1b[201~
\n
```

实现要求：

1. 单行和多行都可以走 bracketed paste。
2. 用户可以在设置中切换 send strategy。
3. 发送内容要记录为 `user_message`。
4. 自动发送 handoff 默认关闭。

### 5.8 HandoffService

职责：

1. 创建或更新 `.marionette/handoff.md`。
2. 生成 prefill prompt。
3. 管理 pinned files。
4. 管理 user notes。

输入来源：

1. Project。
2. Last active session。
3. Selected target agent。
4. Git status。
5. Pinned files。
6. User notes。
7. Recent raw log tail。

输出：

1. handoff.md 文件。
2. prefill prompt 字符串。
3. `handoff_prepared` event。

要求：

1. 不自动发送。
2. 不读取敏感文件内容。
3. 不扫描全仓库。
4. 文件不存在时正常生成。

### 5.9 GitService

职责：

1. 获取 changed files。
2. 获取 diff summary。
3. 获取单文件 diff。

实现方式：

第一版可以调用系统 `git` 命令。

命令：

```text
git status --porcelain
git diff --stat
git diff -- path
```

要求：

1. cwd 是 project root。
2. 如果不是 git repo，返回空状态和提示。
3. 超时要可控，例如 5 秒。
4. 不在 UI thread 执行。

### 5.10 WorkspaceService

职责：

1. 读取项目文件树。
2. 懒加载目录子项。
3. 忽略重目录。
4. 打开文件。
5. Reveal in Explorer。
6. 为 Copy Path 返回绝对路径。
7. 为 Handoff 提供 pinned files。

第一版只做显示和打开，不做编辑。

默认忽略：

```text
.git
node_modules
dist
build
target
.next
.turbo
.cache
coverage
```

数据结构：

```ts
type FileTreeNode =
  | {
      kind: "file"
      name: string
      path: string
      absolutePath: string
      size?: number
      gitStatus?: string
    }
  | {
      kind: "dir"
      name: string
      path: string
      absolutePath: string
      children?: FileTreeNode[]
      hasMore?: boolean
      gitStatus?: string
    }
```

实现要求：

1. `path` 使用项目相对路径。
2. `absolutePath` 只用于打开文件和显示 tooltip。
3. 默认只加载根目录和展开目录。
4. 单目录最多返回合理数量，例如 500 项。
5. 目录过大时返回 `hasMore` 或提示。
6. 不读取文件内容。
7. 不把文件树长期写入 SQLite。

### 5.11 ExternalOpenService

职责：

1. 用系统默认 app 打开文件。
2. 在 Explorer 中 reveal 文件。
3. 打开项目到 Zed。
4. 打开项目到 Cursor。
5. 打开项目到 VS Code。

命令策略：

1. 默认 app：Windows shell open。
2. Explorer reveal：`explorer.exe /select,{path}`。
3. Zed：`zed {path}`。
4. Cursor：`cursor {path}`。
5. VS Code：`code {path}`。

要求：

1. 外部命令不存在时返回 `not_found`。
2. 不阻塞 UI。
3. 不把失败当作 session error。
4. 前端显示 fallback copy command。

## 6. Frontend 模块设计

### 6.1 页面结构

```text
src/
  app/
    App.tsx
  components/
    TopBar.tsx
    ProjectShelf.tsx
    SessionList.tsx
    SessionView.tsx
    RawTerminal.tsx
    CleanTranscript.tsx
    Composer.tsx
    ContextPanel.tsx
    ChangedFiles.tsx
    HandoffPanel.tsx
    FileTreePanel.tsx
    ActivityTimeline.tsx
  lib/
    api.ts
    types.ts
    terminal.ts
    sessionStore.ts
  styles/
    tokens.css
    app.css
```

### 6.2 App.tsx

职责：

1. 加载项目列表。
2. 加载 agent config。
3. 管理当前 project。
4. 管理当前 session。
5. 渲染主布局。

要求：

1. 不直接处理 PTY output。
2. 不直接写文件。
3. 状态流向清楚。

### 6.3 RawTerminal.tsx

职责：

1. 创建 xterm.js 实例。
2. 监听 backend output stream。
3. 写入 terminal。
4. 捕获用户键盘输入。
5. 发送 terminal input 到 backend。
6. 处理 resize。

关键规则：

1. 同一个 live session 的 terminal 实例尽量保持。
2. 切 Clean View 时不要销毁 PTY。
3. 如果必须卸载 xterm，要保存 buffer snapshot 作为显示 fallback。
4. Raw Terminal 聚焦时不让全局快捷键吞掉普通输入。

### 6.4 CleanTranscript.tsx

职责：

1. 显示 SessionEvent。
2. 显示 raw chunk grouped cards。
3. 显示 user_message。
4. 显示 handoff_prepared。
5. 显示 parser unavailable 状态。

要求：

1. 使用虚拟列表或增量渲染。
2. 不渲染无限历史。
3. 对大文本做折叠。
4. Clean View 出错不影响 RawTerminal。

### 6.5 Composer.tsx

职责：

1. 输入 prompt。
2. 显示 handoff prefill。
3. 发送到当前 session。
4. 清空或保留草稿。

规则：

1. 默认用户手动点击 Send。
2. Enter 行为要明确。
3. 多行输入支持 Ctrl+Enter 发送或按钮发送。
4. 发送前显示目标 Agent/session。
5. 发送后记录 user_message。

### 6.6 ContextPanel.tsx

职责：

1. 显示 ChangedFiles。
2. 显示 HandoffPanel。
3. 显示 PinnedContext。
4. 显示 SessionMetadata。

要求：

1. 不自动读取大文件。
2. 不自动读取敏感文件。
3. 操作清晰，例如 Pin、Copy Path、Open in Zed。

### 6.7 FileTreePanel.tsx

职责：

1. 显示项目文件树。
2. 展开目录时懒加载。
3. 显示 git status 标记。
4. 文件点击打开。
5. 右键菜单提供操作。

右键菜单第一版包含：

1. Open with Default App。
2. Open in Zed。
3. Open in Cursor。
4. Open in VS Code。
5. Reveal in Explorer。
6. Copy Path。
7. Pin to Handoff。

要求：

1. 不内置编辑器。
2. 不预览大文件。
3. 不做全文搜索。

### 6.8 ActivityTimeline.tsx

职责：

显示本次 session 关键事件：

1. session started。
2. user message sent。
3. handoff prepared。
4. changed files refreshed。
5. file pinned。
6. session exited。

要求：

1. 事件数量过多时折叠。
2. 不显示完整 raw output。
3. 点击事件可以定位相关文件或 session。

## 7. Tauri Commands 设计

### 7.1 Project commands

```ts
list_projects(): Project[]
add_project(path: string): Project
open_project(projectId: string): Project
remove_project(projectId: string): void
```

### 7.2 Agent commands

```ts
list_agents(): AgentConfig[]
update_agent(config: AgentConfig): AgentConfig
test_agent_command(agentId: string): CommandTestResult
```

### 7.3 Session commands

```ts
create_session(projectId: string, agentId: string): Session
start_session(sessionId: string): void
stop_session(sessionId: string): void
restart_session(sessionId: string): Session
list_sessions(projectId: string): Session[]
write_session_input(sessionId: string, text: string, mode?: SendMode): void
write_session_raw(sessionId: string, bytes: number[]): void
resize_session(sessionId: string, cols: number, rows: number): void
set_session_view_mode(sessionId: string, mode: SessionViewMode): void
```

### 7.4 Handoff commands

```ts
generate_handoff(projectId: string, targetAgentId: string): HandoffResult
get_handoff(projectId: string): string
update_user_notes(projectId: string, text: string): void
pin_file(projectId: string, path: string): void
unpin_file(projectId: string, path: string): void
```

### 7.5 Git commands

```ts
get_changed_files(projectId: string): ChangedFile[]
get_diff(projectId: string, path: string): string
get_diff_stat(projectId: string): string
```

### 7.6 External editor commands

```ts
open_project_in_zed(projectId: string): OpenEditorResult
open_file_in_zed(projectId: string, path: string): OpenEditorResult
```

Fallback：

1. 如果 `zed` 命令不存在，返回 `not_found`。
2. 前端显示可复制命令。

### 7.7 Workspace commands

```ts
get_file_tree(projectId: string, directory?: string): FileTreeNode[]
open_path_default(projectId: string, path: string): OpenPathResult
reveal_path(projectId: string, path: string): OpenPathResult
copy_path(projectId: string, path: string): string
open_path_in_editor(projectId: string, path: string, editor: "zed" | "cursor" | "vscode"): OpenEditorResult
get_session_activity(sessionId: string): SessionActivityEvent[]
```

`get_file_tree` 要求：

1. 默认懒加载。
2. 忽略重目录。
3. 返回项目相对路径。
4. 不读取文件内容。

`open_path_default` 要求：

1. 使用 Windows 默认 app。
2. 支持文件和目录。
3. 失败时返回可读错误。

`open_path_in_editor` 要求：

1. 不要求编辑器必须安装。
2. 未安装时返回 fallback command。
3. 不阻塞 UI。

## 8. Streaming 设计

### 8.1 Backend 到 Frontend 的事件

建议事件：

```ts
type BackendEvent =
  | { type: "pty_output"; sessionId: string; data: number[] }
  | { type: "session_status"; sessionId: string; status: SessionStatus }
  | { type: "session_event"; sessionId: string; event: SessionEvent }
  | { type: "git_changed"; projectId: string; files: ChangedFile[] }
  | { type: "error"; scope: string; message: string }
```

### 8.2 PTY output 批处理

要求：

1. PTY output 不逐字符发送。
2. 合并 chunk。
3. 前端收到后直接写入对应 xterm。
4. 如果当前 session 不可见，也要保留 ring buffer。

### 8.3 Ring Buffer

每个 session backend 维护最近输出 ring buffer。

用途：

1. 新打开 RawTerminal 时补一段最近输出。
2. Clean parser 使用 tail。
3. Handoff 使用 tail。

建议大小：

1. MVP：每 session 最近 1MB 文本。
2. raw log 仍然保存完整历史。

## 9. Terminal 状态保持

### 9.1 推荐方案

每个 live session 有一个 `TerminalModel`。

前端策略：

1. 当前 session 的 xterm mounted。
2. 最近使用的几个 session 可以保留 hidden mounted。
3. 其它 session 依赖 backend ring buffer 恢复最近显示。

### 9.2 不依赖完整序列化

不要把 terminal serialize 当成唯一恢复机制。

原因：

1. 终端状态复杂。
2. TUI alternate screen 不一定完美恢复。
3. 真实 PTY 仍在运行。

第一版目标是：

1. 切换常用 session 时状态尽量保持。
2. 如果无法完整保持，也能显示最近输出。
3. 不杀进程。

## 10. 输入安全设计

### 10.1 Raw Terminal 输入

Raw Terminal 输入直接写 PTY。

流程：

```text
xterm onData
  -> write_session_raw(sessionId, bytes)
  -> PtyService.write
```

### 10.2 Composer 输入

Composer 输入走 adapter。

流程：

```text
text
  -> write_session_input
  -> Adapter.prepare_input
  -> PtyService.write or protocol send
  -> record user_message
```

### 10.3 自动发送限制

默认禁止自动发送。

理由：

1. 无法可靠知道 TUI 当前是否在普通输入框。
2. 可能误触 permission prompt。
3. 可能误触 login prompt。
4. 可能误触 model picker。

第一版只做 prefill。

## 11. Parser 设计

### 11.1 第一版 parser

第一版 parser 是 `ansi-raw`。

职责：

1. 接收 raw text chunk。
2. 去掉部分 ANSI 控制码。
3. 合并为 clean-ish raw cards。
4. 生成 `raw_chunk` event。

要求：

1. parser 失败不影响 session。
2. parser 运行在后台任务。
3. parser 输出可以丢弃，但 raw log 不能丢。

### 11.2 OpenCode parser

第二阶段实现 `opencode-sse`。

职责：

1. 连接 OpenCode server 或 SDK。
2. 读取 session/message/event。
3. 转换成 SessionEvent。
4. 显示 assistant message、tool call、file change。

### 11.3 Claude/Codex/Grok parser

第一版不做深度 parser。

后续策略：

1. 如果原厂 CLI 有稳定 JSONL/headless 输出，再接。
2. 如果只有 TUI，保持 Generic PTY。
3. 不写脆弱的屏幕 scraping 作为核心依赖。

## 12. UI 性能设计

### 12.1 基本原则

1. xterm 直接写 bytes/string。
2. Clean View 虚拟列表。
3. 大文本折叠。
4. 右侧面板按需刷新。
5. git status 不高频轮询。
6. parser off main thread。
7. CSS 动画克制。

### 12.2 刷新频率

建议：

1. PTY output 前端写入最多每 16ms 一批。
2. changed files 手动刷新或低频刷新。
3. session status 事件驱动。
4. log tail 手动加载。

### 12.3 UI 视觉性能限制

避免：

1. 大面积 blur。
2. 大量 box-shadow。
3. 高频 layout animation。
4. 渐变背景动画。
5. 每个输出 chunk 都触发 React state 更新。

## 13. 错误处理

### 13.1 Command 不存在

显示：

```text
Command not found: {command}
请检查 Agent 是否已安装，或在设置中修改命令路径。
```

### 13.2 PTY 启动失败

显示：

```text
Failed to start session.
Agent: {agent}
CWD: {cwd}
Error: {message}
```

### 13.3 Log 写入失败

显示 warning：

```text
Session is running, but raw log cannot be written.
```

不中断 session，除非错误影响 PTY。

### 13.4 Parser 失败

Clean View 显示：

```text
Clean View unavailable. Raw Terminal is still running.
```

## 14. 安全和隐私

### 14.1 本地优先

所有数据默认保存在本地。

### 14.2 不管理密钥

Marionette 不收集、不保存、不展示 API key。

### 14.3 不自动读取敏感文件

Handoff 不自动读取：

1. `.env`
2. `.env.*`
3. `*.pem`
4. `*.key`
5. `id_rsa`
6. `id_ed25519`
7. credential/config secrets

### 14.4 用户可见性

任何 handoff prompt 发送前必须可见。

## 15. 推荐目录结构

```text
marionette/
  src-tauri/
    src/
      main.rs
      app_state.rs
      commands/
        mod.rs
        projects.rs
        agents.rs
        sessions.rs
        handoff.rs
        git.rs
        editor.rs
      pty/
        mod.rs
        pty_service.rs
        windows_pty.rs
        output_tee.rs
        ring_buffer.rs
      sessions/
        mod.rs
        session_manager.rs
        session_model.rs
        log_writer.rs
      adapters/
        mod.rs
        agent_adapter.rs
        generic_pty.rs
        opencode.rs
        codex.rs
        claude_code.rs
        grok_build.rs
      handoff/
        mod.rs
        handoff_builder.rs
        handoff_model.rs
      git/
        mod.rs
        status.rs
        diff.rs
      storage/
        mod.rs
        sqlite.rs
        paths.rs

  src/
    app/
      App.tsx
    components/
      TopBar.tsx
      ProjectShelf.tsx
      SessionList.tsx
      SessionView.tsx
      RawTerminal.tsx
      CleanTranscript.tsx
      Composer.tsx
      ContextPanel.tsx
      ChangedFiles.tsx
      HandoffPanel.tsx
    lib/
      api.ts
      types.ts
      terminal.ts
      sessionStore.ts
    styles/
      tokens.css
      app.css
```

## 16. 设计验收清单

实现任何功能前，检查：

1. 是否保持 Raw Terminal 权威。
2. 是否没有引入平台化功能。
3. 是否没有管理 API key。
4. 是否不会高频刷新 UI。
5. 是否不会把 raw log 塞入 SQLite。
6. 是否不会自动发送 handoff。
7. 是否能在 parser 失败时继续使用 Raw Terminal。
8. 是否保持 Windows-first。
