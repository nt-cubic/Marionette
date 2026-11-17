# 设计方案：Browser Workspace（浏览器 + Agent 对话）

- 状态：设计稿，2026-11-17
- 目标分支：feat/browser-annotation
- 前置调研：research-browse-ux.md
- 目标：让用户在同一窗口中一边看浏览器，一边和正在处理该页面的 Agent 对话、批注、授权和接管

## 结论

第一版不要改造成通用异类标签分屏。先做专用 Browser Workspace：

- Browser Workspace 不是默认布局；
- 浏览器位于主画布；
- Agent 对话位于可拖宽、可收起的右侧栏；
- 复用现有 renderConversation，不复制 SessionView、Composer 或 ACP 状态；
- 浏览器 child WebView2 继续通过现有 host bounds 同步定位；
- 退出 Browser Workspace 后恢复原来的单会话或会话分屏。

这样能以较小风险先验证最重要的用户闭环：

1. 用户在浏览器中看到 Agent 正在处理什么；
2. 用户能直接指着页面批注；
3. 批注立即进入右侧 Composer；
4. Agent 请求权限或等待用户时，页面和对话同时可见；
5. 用户不需要在浏览器伪标签和会话之间来回切换。

Browser Workspace 只服务“正在看一个 Web 产物”的任务态。普通聊天、代码修改、文档整理和一般网页研究仍保持现有以对话为主的界面。

## 任务入口与自动切换

不是所有用户都是 Web 开发者。让浏览器成为主画面，只有在它确实是被制作的产物时才有意义。

### 默认行为

- 普通 Chat / 代码 / 文档 / 收纳盒：不显示 Browser Workspace；
- Agent 为了查资料、搜索或阅读网页而打开标签：浏览器标签留在后台；
- Agent 新开普通网页：不改变当前会话、文档或 Composer；
- 用户主动点浏览器伪标签：进入现有浏览器页，并可打开 Agent 侧栏；
- 用户主动点“在浏览器预览”：进入 Browser Workspace。

### 自动进入 Browser Workspace 的条件

优先级从高到低：

1. Agent 明确把页面标记为 Web 产物预览；
2. 用户要求创建/修改网页、站点或 Web 原型，Agent 完成启动后在预置浏览器中打开 localhost/dev URL；
3. 用户在批注、截图或预览操作中明确选择“在浏览器中查看”；
4. 后续可选：项目里检测到 dev server 和 Web framework，且 Agent 启动的命令产生本地预览地址。

第一版建议给 browser_open_tab 增加：

    display?: "background" | "workspace"

默认 background。MCP instructions 明确：

- 当用户要求做网页、站点或 Web 原型，需要持续展示预览时，用 display = workspace；
- 单纯查资料、打开链接、登录或读取网页时用 background；
- 不确定时保持 background，由用户主动打开。

不要仅凭 URL 包含 localhost 就强制切界面。localhost 也可能只是 API、文档服务或用户当前不想看的后台服务。

### 不自动进入的情况

- 搜索、研究、读文档、看报错页；
- Agent 自己打开参考站点；
- 登录、2FA、支付等只处理一次的外部页面；
- 用户正在 Composer 输入，且当前任务没有 Web 预览意图；
- 后台长任务只是偶尔访问网站。

### 切换体验

进入 Browser Workspace：

- 当前会话原样成为右侧 Agent 会话；
- 浏览器加载正在制作的页面；
- 右侧保留转写和 Composer，不自动抢 Composer 焦点；
- 顶部出现短暂的“已在浏览器预览”状态和“返回对话”入口；
- 用户从浏览器关闭或点返回时，恢复进入前的单会话、文档或分屏状态。

用户提供三种偏好：

- 自动进入：Web 产物预览默认切到 Browser Workspace；
- 询问：显示一条“打开预览”操作，不自动切；
- 留在对话：浏览器标签在后台，用户手动打开。

默认先选择“Web 产物预览时自动进入”，普通网页保持后台。该偏好只影响明确产物预览，不影响所有 browser_open_tab。

## 当前代码约束

### 已经具备的基础

- BrowserPage 已经管理地址栏、内置标签、分享、批注抽屉和 child WebView2 bounds；
- renderConversation 已能按任意 session 渲染完整 SessionView + Composer；
- SplitWorkspace 已经证明多个对话视图能共享 App 根状态；
- DocCompareLayout 已经实现“一栏对话 + 一栏内容 + 拖动分隔条”的专用双栏布局；
- 批注发送已经能创建会话、落盘截图、预填 prompt 和附件；
- 浏览器授权、Control 权限卡和用户标签边界已存在。

### 不能直接复用的部分

- SplitWorkspace 的 PaneLeaf 只有 session id；
- BrowserPage、DocPage、MemoBox 是 App 根上的互斥伪标签；
- renderConversation 依赖不少“当前会话”状态，直接放第二个 session 时可能串焦点；
- BrowserPage 的 Esc listener 是 window 级，右侧 Composer 聚焦时可能误退出；
- dock 模式的浏览器整页宽度不足以承载双栏。

因此第一版是 BrowserPage 专用组合层，不修改 SplitWorkspace 数据模型。

## 用户界面

普通主窗口：

    ┌──────────────────────────────────────────────────────────────┐
    │ Marionette 标题栏 / 浏览器伪标签                              │
    ├──────────────────────────────────────────────────────────────┤
    │ 左侧栏 │ Browser Workspace                                    │
    │        │ ┌─────────────────────────────┬────────────────────┐ │
    │        │ │ 浏览器标签 / 地址栏          │ Claude / 会话名      │ │
    │        │ │                             │                    │ │
    │        │ │ 页面                         │ 对话转写            │ │
    │        │ │                             │                    │ │
    │        │ │                             │ 权限 / WAIT 提示    │ │
    │        │ │                             │ Composer           │ │
    │        │ └─────────────────────────────┴────────────────────┘ │
    └──────────────────────────────────────────────────────────────┘

交互规则：

- 浏览器为主画布，右侧对话默认 380px，可在 320–520px 拖动；
- 只有 Web 产物预览进入该布局，普通聊天不出现浏览器主画布；
- 右侧栏可通过浏览器工具栏按钮收起；
- 用户拖动分隔条时只改布局宽度，松手后才持久化；
- 右侧栏聚焦 Composer 时，浏览器 child WebView2 保持显示但失去焦点；
- 浏览器 host 的 ResizeObserver 触发后，WebView2 bounds 跟着新的浏览器矩形同步；
- Browser Workspace 打开时默认折叠最右侧 ContextPanel，避免主画布被三列挤死；
- 退出 Browser Workspace 后恢复进入前的 ContextPanel 状态和分屏树。

### 宽度降级

- 内容区宽度 ≥ 900px：浏览器 + 对话；
- 760–899px：右侧对话自动缩到 340px；
- < 760px：只显示主视图，对话按钮保留；点击后在浏览器上覆盖 360px 侧栏；
- Dock 模式第一版继续使用现有浏览器整页，不强行塞双栏；
- detached window 不提供 Browser Workspace。

## 状态模型

现有 browserActive 继续表示 Browser Workspace 是否打开。App 根再新增两个持久状态：

    browserAgentPanelOpen: boolean
    browserAgentPanelWidth: number

另外保留一个非持久运行态：

    browserChatSessionId: string | null

语义：

- 不新增 browserWorkspaceOpen，避免和现有浏览器伪标签出现两套状态；
- browserAgentPanelOpen 控制右侧对话是否显示；
- browserChatSessionId 指定 Browser Workspace 当前绑定的会话；
- 进入浏览器时优先使用当前会话；
- 由 Agent 打开浏览器、发起 handoff 或用户发送批注时，可把 browserChatSessionId 指向来源会话；
- 没有真实会话时，用户第一次发消息或发批注时走现有创建 Chat 路径。

持久化 key：

    marionette-browser-agent-panel-open
    marionette-browser-agent-panel-width

宽度只在真正拖动后写入，沿用 DocCompareLayout 的“没移动的 mouseup 不落值”规则。

## 组件拆分

### 1. 抽出通用 SidecarSplit

从 DocCompareLayout 抽出通用组件，不把文档语义塞进浏览器：

    AgentSidecarSplit
      chat: ReactNode
      surface: ReactNode
      surfaceSide: "left" | "right"
      storageKey: string
      defaultSurfaceWidth?: number
      minSurface: number
      minChat: number
      onWidthChange?: (width: number) => void

DocCompareLayout 继续保留为薄包装，避免大范围改文档页。

Browser Workspace 使用：

    AgentSidecarSplit(
      chat = renderConversation(browserChat),
      surface = <BrowserPage ... />,
      surfaceSide = "left",
    )

### 2. BrowserWorkspace

新增 BrowserWorkspace 组件，只负责组合和布局，不持有业务状态：

    BrowserWorkspace
      browser: ReactNode
      conversation: ReactNode
      panelOpen: boolean
      onTogglePanel: () => void
      narrow: boolean

它不直接调用 Tauri、不管理 tab、不管理 ACP，也不持有会话事件。

### 3. BrowserPage

BrowserPage 只增加一组可选展示 props：

    agentPanelOpen?: boolean
    onToggleAgentPanel?: () => void
    agentPanelStatus?: "idle" | "running" | "waiting_user" | "error"
    agentPanelUnread?: number

工具栏增加一个对话侧栏按钮：

- 有未读 Agent 回复时显示计数；
- Agent 运行中显示中性运行提示；
- waiting_user 时显示黄色“等你”；
- 只负责显示和触发回调，不读取全局会话业务。

### 4. App

App 增加一个分支：

    browserActive
      -> BrowserWorkspace
           surface: BrowserPage
           chat: renderConversation(browserChatSession)

不把 Browser Workspace 塞进 SplitWorkspace 的 renderLeaf。

现有 workspaceSplit、openSessionIds 和 splitState 全部保留。浏览器激活时暂不渲染分屏树，退出后原样恢复。

## 会话绑定

第一版只允许右侧栏绑定一个会话，避免重做通用多会话面板。

选择顺序：

1. Browser Workspace 已有 browserChatSessionId，且会话仍存在；
2. 当前 currentSession；
3. 当前项目可用的第一个 Chat；
4. 用户发第一条消息或批注时创建新 Chat。

以下行为更新 browserChatSessionId：

- 用户从左侧栏选择会话；
- Agent handoff 指明来源 session；
- 用户从浏览器打开时当前会话已变化；
- 右侧会话被删除时回退到下一条可用 Chat。

注意：renderConversation 中有部分 autoFocus 和 activeModelId 判断使用 currentSessionId。进入 Browser Workspace 时必须同步 applyFocusedSession(browserChatSession)，不能让右侧 session 与全局 currentSessionId 分裂。

## 批注发送

现有 sendBrowserPicksToChat 已经支持：

- 创建会话；
- 附件路径；
- Composer 预填；
- 失败保留草稿。

Browser Workspace 中只改目标绑定：

- 优先发给 browserChatSessionId；
- 该会话不存在时用当前 session；
- 都没有时创建 Chat，并把 browserChatSessionId 更新为新会话；
- 发送成功后右侧 Composer 自动聚焦，浏览器不切走。

不需要新增批注后端命令。

## Handoff 接线点

虽然 handoff 是下一阶段，Browser Workspace 需要预留事件：

- browser status = waiting_user；
- 自动打开 browserAgentPanel；
- 当前 handoff 的来源 session 成为 browserChatSessionId；
- 页内卡片和右侧对话同时显示“等你”；
- 用户点击 Continue 后，状态回 running，侧栏保留不突然关闭。

## Esc 与焦点

BrowserPage 当前 window Esc 会退出浏览器伪标签。Browser Workspace 下要避免与 Composer 快捷键冲突：

- 焦点在右侧 Composer 或会话内容时，Esc 由现有会话/弹窗逻辑处理；
- 焦点在浏览器地址栏或浏览器工具栏时，Esc 收起侧栏，第二次才退出 Browser Workspace；
- 焦点在 child WebView2 页面时，Esc 只处理页面 picker/插件覆盖层，不关闭整个工作区；
- 权限卡、handoff 卡、确认框拥有最高优先级。

实现时优先在 BrowserPage 检查 event.target 是否位于右侧 Composer，再决定是否调用 onExit；不要给整个窗口增加新的全局 Esc 状态机。

## WebView2 bounds 与焦点风险

必须重点验证：

- 拖动侧栏宽度时，child WebView2 的 left/width 连续同步，不盖住右侧 Composer；
- 收起/展开侧栏时不会触发主窗口 resize subclass 失配；
- 右侧 Composer 输入中文 IME 时，焦点不会被 child WebView2 抢走；
- 点击浏览器页面后，右侧 Composer 再点击仍能输入；
- 批注抽屉出现或消失时只改变浏览器栏内部高度，不改变侧栏宽度；
- Browser Workspace 切回普通会话时，child WebView2 隐藏且不遗留透明命中层。

这里沿用现有 wry set_visible 配对和 BrowserPage ResizeObserver，不要另写 ShowWindow 路径。

## 分阶段落地

### Phase 1：Browser Workspace 骨架

- 抽出 AgentSidecarSplit；
- BrowserWorkspace 专用布局；
- browser_open_tab 增加 background/workspace 显示模式，默认 background；
- 普通 Agent 标签不再自动抢当前界面；
- MCP instructions 加入“Web 产物预览才使用 workspace”；
- 右侧显示当前会话的 renderConversation；
- 侧栏开关、宽度拖动、持久化；
- 浏览器 host bounds 验证；
- ContextPanel 自动折叠和恢复；
- 批注发送目标改为右侧会话。

交付标准：用户能在同一窗口一边浏览页面一边发消息，批注直接进侧栏 Composer。

### Phase 2：状态与 handoff

- browser status：running / waiting_user / failed；
- BrowserPage 对话按钮显示运行/等你；
- WAIT 自动打开侧栏；
- browser_handoff 工具与页内卡片；
- 桌面通知点击回到来源 tab + session；
- challenge/blocked 转 handoff。

交付标准：Agent 需要用户时，不需要用户自己从聊天里找是哪个标签。

### Phase 3：标签归属和后台行为

- TabInfo 增加 ownerSessionId、busy、lastUsedAt；
- 按 session/task 分组；
- Agent 自开标签自动清理；
- Browser Workspace 侧栏可在同 Agent 的多个会话间切换。

### Phase 4：通用异类分屏

只有前三个阶段成立后才考虑：

- session/browser/document/memo 统一标签；
- 任意两格并排；
- 拖拽、关闭、恢复和 detached 语义统一；
- doc compare 和 Browser Workspace 可迁移到通用模型。

## 明确不做

- 不在 Phase 1 修改 SplitWorkspace 的 PaneLeaf 类型；
- 不为 Browser Workspace 写第二套 Composer；
- 不在右侧栏重新订阅 ACP 事件；
- 不复制第二份 SessionView 状态；
- 不用新 OS 窗口模拟分屏；
- 不在 Dock 第一版强塞双栏；
- 不让浏览器始终抢主画面；
- 不把安全权限确认搬进页面卡片。

## 回归与验收

### 自动回归

- 新增 verify-browser-workspace.mts：
  - 宽度上下限；
  - 拖动未移动不落值；
  - 窄窗折叠判定；
  - 会话回退顺序；
  - annotation 目标会话选择。
- npm run build；
- cargo test --bin marionette browser；
- npx tsx scripts/verify-browser-pick.mts；
- 现有 split-workspace 回归；
- 现有 doc-compare 宽度持久化回归。

### 真机 E2E

1. 打开 Browser Workspace，确认浏览器 child WebView2 只在左侧矩形内；
2. 右侧 Composer 输入中英文和 IME，确认不被浏览器抢焦点；
3. 拖动分隔条和窗口宽度，确认页面与 Composer 不重叠；
4. 拾取元素并发给对话，确认附件和 prompt 落右侧 Composer；
5. 收起侧栏、再打开，确认草稿和会话状态不丢；
6. 从会话分屏进入浏览器，再退出，确认原分屏树恢复；
7. 右侧会话运行中、出错、等待权限时，状态显示清楚；
8. 关闭全部浏览器标签或退出 Browser Workspace，确认主窗口输入和快捷键恢复。

## 最小可交付版本

如果只做一版验证价值，必须包含：

- browser_open_tab 的 background/workspace 两种显示模式，普通网页默认后台；
- 浏览器主画布；
- 右侧当前 Agent 对话；
- 可拖动宽度和收起；
- 批注直接进右侧 Composer；
- ContextPanel 自动让位；
- 退出后恢复原工作区；
- 中文输入、bounds 和焦点真机通过。

handoff、后台标签组和通用文档分屏都可以在该版本验证后再做。
