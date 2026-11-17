# 设计方案：Workspace Surface（浏览器 + Agent 对话）

- 状态：Phase 1–4 已实现，2026-11-17
- 目标分支：`feat/browser-annotation`
- 前置调研：[`research-browse-ux.md`](./research-browse-ux.md)
- 目标：Web 产物预览时，让浏览器和当前 Agent 对话同屏；普通聊天、研究和后台网页不抢工作区

## 结论

采用 Codeg 的核心产品模型，而不是照搬它的完整 IDE：

- 中间工作区由 **Agent 对话 + content surface** 两部分组成；
- Browser 是第一种 content surface；
- 现有 SessionView、Composer、ACP 事件和会话状态只保留一套；
- 现有 `SplitWorkspace` 继续负责纯对话分屏；
- Browser surface 关闭后，原来的单会话或对话分屏原样恢复；
- Document、Memo 已迁入同一 surface；Browser / Document / Memo 共用标签、对话选择器和恢复快照。

第一版实现的是通用边界，不是把所有异类标签一次改完：

    SurfaceWorkspace
      kind: "browser" | "session" | "document" | "memo"
      surface: ReactNode
      chat: ReactNode

当前注册 `browser`、`document`、`memo`。三者共享同一布局和对话栏，不再各写一套双栏。

## 用户界面

普通主窗口进入 Browser surface 后：

    ┌──────────────────────────────────────────────────────────────────┐
    │ 左侧栏 │ 浏览器 62%                         │ Agent 对话 38%       │
    │        │ ┌───────────────────────────────┐ │ ┌──────────────────┐ │
    │        │ │ 浏览器标签 / 地址栏 / 页面     │ │ │ 会话名 / 返回对话 │ │
    │        │ │                               │ │ ├──────────────────┤ │
    │        │ │                               │ │ │ 对话转写          │ │
    │        │ │                               │ │ │                  │ │
    │        │ │                               │ │ │ Composer         │ │
    │        │ └───────────────────────────────┘ │ └──────────────────┘ │
    └──────────────────────────────────────────────────────────────────┘

- 浏览器默认占内容区约 62%，对话占其余宽度；
- 中间分隔条可拖动，浏览器最小 360px、对话最小 320px；
- 拖动结束才写 `marionette-workspace-surface-width`；没移动的点击不改宽度；
- 内容区小于 760px 时改为上下堆叠，浏览器上、对话下，避免两个窄柱；
- 进入时最右侧 ContextPanel 自动收起，退出后恢复进入前状态；
- 顶部是 Browser / Document / Memo 共用标签条；对话栏顶部可切换绑定会话；
- Dock 模式继续使用现有整页浏览器，不做窄条双栏；
- Detached window 不进入该 surface。

## 状态模型

普通窗口共用一组 surface 状态：

    activeSurfaceTabId
      = "browser"
      | "document:<absolute-path>"
      | "memo"
      | null

`browserActive`、`activeDocPath`、`memoActive` 只决定当前选中的 surface 类型；打开过的 Document/Memo/Browser 标签继续保留。surface 可见时统一渲染 `SurfaceWorkspace(surface, renderConversation(displaySession))`，关闭后回到原来的单会话或 session-only 分屏。

`workspaceSplit`、`splitState`、`openSessionIds` 保持原样。surface 内可切换绑定会话，切换只改右半边对话，不销毁 surface；回到会话标签才关闭 surface。

## 打开策略

`browser_open_tab` 增加：

    display?: "background" | "workspace"

默认 `background`。这个字段是一次打开动作的意图，不是全局布局开关。

### background

- Agent 查资料、搜索、读文档、登录、看参考站点；
- 新 tab 只进入后台标签列表；
- 不切当前会话、文档、收纳盒或 Composer；
- 用户自己的新建 tab 仍由前端在命令返回后主动选中。

### workspace

- 只在用户明确要求制作、修改或预览 Web 页面/站点时使用；
- 新 tab 被选中，并进入 Browser surface；
- 当前 Agent 会话成为右侧对话；
- 页面与对话同时可见，批注直接进入右侧 Composer。

MCP `initialize.instructions` 和工具 description 都明确：

- 制作 Web 原型/网站、需要持续看渲染结果时用 `workspace`；
- 研究、登录和一次性网页读取用 `background`；
- 不允许仅凭 URL 包含 `localhost` 推断 workspace。

### dev server 自动预览

已经落地，按 Codeg 的安全顺序：

1. 只扫描终端/Agent 命令输出中的 loopback URL；
2. 先做 TCP probe，确认端口真的在监听；
3. 按用户偏好选择“自动打开 / 发通知 / 忽略”；
4. 自动打开仍创建正常 Browser tab，不伪造 Agent 的 workspace 意图。

不做“看到 localhost 字符串就切界面”。API、文档服务、后台任务和一次性登录页都可能使用 loopback。

## 页面与对话交接

### 页面 → 对话

现有批注链路保留：

- 元素拾取、区域框选、逐条 note、截图缩略图；
- 发送时重新快照解析 aria ref；失败回落 selector；
- 页面内容带不可信声明，URL 脱敏；
- 截图作为附件药丸进入 Composer；
- 只预填，不自动发送。

普通窗口发送成功后 Browser surface 保持打开，用户可直接在右侧检查并发送。Dock 没有双栏，发送成功后仍回到窄条，让 Composer 可见。

### Agent → 页面

继续使用现有 `none/read/control` 分享语义与 Control 权限卡。第一版不改变 Agent 自开 tab 默认可读的现状；是否收紧为 Codeg 式“所有 origin 都必须用户分享”另做产品决策。

### 人接管页面（handoff）

Agent 可调用 `browser_handoff` 暂停在登录、2FA、CAPTCHA、支付确认等必须由人完成的步骤。tab 进入 `waiting_user`；Browser surface 自动打开并切到来源会话，页面与对话之间出现一条极简提示和一个 Continue / Deny 选择，后台窗口同时发系统通知。Continue 放行 Agent 继续，Deny 作为可转述的拒绝返回工具；超时按未完成结束，不默认继续。

## WebView2 与焦点

- BrowserPage 的 host 只在 surface 自己的矩形内；
- 拖动分隔条改变 DOM 宽度，现有 ResizeObserver 随后同步 child WebView2 bounds；
- 对话侧获得焦点时 Browser surface 保持可见，但不抢 Composer 焦点；
- BrowserPage 在 embedded 模式只在 Esc 目标位于浏览器 DOM 时处理退出；
- 右侧 Composer、权限卡、确认框里的 Esc 不被 BrowserPage 截走；
- 关闭 surface 后 child WebView2 隐藏，tab 本身继续保留。

## 组件与文件

- `src/lib/workspaceSurface.ts`：surface 类型、宽度上下限、指针换算、窄栏判定；
- `src/components/SurfaceWorkspace.tsx`：通用 `surface + chat` 布局、共同标签和会话切换；
- `src/styles/workspace-surface.css`：宽栏左右布局、窄栏上下布局、标签条和分隔条；
- `src/app/App.tsx`：Browser/Document/Memo 接线、ContextPanel 让位、后台 tab、快捷键和恢复；
- `src/components/BrowserPage.tsx`：embedded Esc 边界、状态文字和 handoff 提示；
- `src/lib/devServer.ts` + `src-tauri/src/browser/service.rs`：终端 URL 扫描、地址归一化和 probe；
- `src-tauri/src/browser/`：tab 归属、并发 busy 计数、状态、handoff 与会话标签清理；
- `src-tauri/src/tools_mcp.rs`：15 个 browser 工具 schema、initialize instructions 和 workspace 结果说明。

## 分阶段

### Phase 1：Browser surface（已实现）

- 最小通用 `SurfaceWorkspace`；
- Browser 左、Agent 对话右；
- 可拖动宽度、持久化、窄窗上下堆叠；
- ContextPanel 自动让位与恢复；
- `browser_open_tab(display)`；
- 后台 tab 不抢界面，只有 workspace tab 自动进入；
- 批注直接进入同屏 Composer；
- 现有 session 分屏树保持不动并可恢复。

交付标准：Agent 打开 Web 产物预览时，用户无需切换标签就能看页面并在旁边继续对话和批注。

### Phase 2：dev server 与运行状态（已实现）

- loopback URL 扫描 + TCP probe + 用户偏好；
- Browser tab 显示 running / waiting_user / failed；
- Agent 自开标签的 ownerSessionId、busy、lastUsedAt；
- 关闭会话时按策略清理标签。

### Phase 3：更多 worksurface（已实现）

- Document 进入同一 `SurfaceWorkspace`；
- Memo 进入同一模型；
- 允许 surface 内切换绑定会话；
- 原 Document 对照布局由通用 surface 取代。

### Phase 4：Handoff 与通用标签树（已实现）

- `browser_handoff`、页内 WAIT 卡片与通知；
- Browser / Document / Memo 共用标签、恢复、关闭与快捷键；
- 只有当通用 surface 模型稳定后，才考虑把 `SplitWorkspace` 升级成任意异类 surface 树。

## 明确不做

- 不复制第二套 SessionView 或 Composer；
- 不在第一版改 session-only `SplitWorkspace` 的 reducer；
- 不因 `localhost` 字符串自动抢界面；
- 不把 Codeg 的编辑器、文件树、Git、终端一起搬进来；
- 不在 Dock 强制塞双栏；
- 不把 Control 安全授权做成页面内可伪造的卡片；
- 不把一次性网页研究默认变成 workspace。

## 回归

- `npx tsx scripts/verify-workspace-surface.mts`
- `npx tsx scripts/verify-browser-pick.mts`
- `npm run build`
- `cargo check`
- `cargo test --bin marionette browser`
- 真机检查：拖动不覆盖 Composer、child WebView2 bounds 跟随、Esc 不误关、ContextPanel 恢复、后台 tab 不抢界面、workspace tab 自动进入。
