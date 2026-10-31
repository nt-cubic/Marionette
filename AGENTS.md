# AGENTS.md — Marionette

Marionette 是一个 Windows 本地多 Agent CLI 图形壳（Tauri 2 + React + ACP）。
只负责启动/展示/切换/记录 agent，不实现 agent runtime。

## 常用命令

- 启动开发版：`npm run tauri dev`（或 `start-marionette.bat`）
- 构建便携版：`tools\build-portable.bat`（内置引擎，产物 `release\<版本号>\Marionette.exe`，按版本分目录，运行中的旧版不会挡住新构建）。默认会先 `import-magpie.ps1` 拉上游 latest 稳定版进 `third_party/magpie`（`SKIP_MAGPIE_IMPORT=1` 跳过；`MAGPIE_TAG=v…` 钉版本）。用户侧也可在「模型共享」里单独更新 magpie。发布：`tools\publish-release.bat`
- 前端构建：`npm run build`（`tsc && vite build`）
- Rust 检查：`cargo check`（在 `src-tauri/` 下）
- 发布版本号：`tools\bump-version.ps1`（或 `tools\bump-version.bat`）
- 演示实例 / 拍图：`tools\demo-instance.bat`（隔离 profile + 预置对话，窗口在 `%TEMP%\marionette-demo`，抓图用 `PrintWindow`；参数与抓图禁忌见 `tools/README.md`）

## 目录

- 前端：`src/`（React；会话流处理核心在 `src/lib/acpTranscript.ts`）
- 后端：`src-tauri/src/`（ACP 服务 `acp.rs`、终端 `terminal_runtime.rs`）
- 现行产品说明：`docs/CURRENT.md`；索引见 `docs/README.md`，`docs/archive/` 是历史归档，不是现行契约
- 验证脚本：`scripts/`（清单见 `scripts/README.md`）；Windows 启动/构建/发布：`tools/`（清单见 `tools/README.md`）
- 根目录只保留 `start-marionette.bat` 一个入口；改 `tools/` 里的脚本时，仓库根一律用 `%~dp0..`

## 贴边停靠 edge dock（2026-10-24 落地，阶段 0–3）

设计稿 `docs/plan-edge-dock.md`（含与实现的偏差记录），行为描述在 `docs/CURRENT.md`。改这块前先记四条红线：

- **停靠期间绝不 `hide()` / `destroy()` / `setFocus()`**（tao 0.35 paint assert 红线的延伸）。
  窗口只是**停在屏外留 6 逻辑 px**（真实客户区，画成淡轨 + 中间一根把手；DOM mouseenter 即弹出信号）；弹出/收回全是
  `setPosition` 9 步 ~160ms 滑动；聚焦只来自用户点击或单实例交接（交接聚焦 4px sliver 会
  由 dockMode 的 onFocusChanged 转成 expand，不会出现「亮一条 4px」）。全部几何用**物理像素**，且**按客户区定位**：tao 给无边框窗留了 ~8px 不可见边框（`calculate_insets_for_dpi`，左/右/下），
  `outerPosition`/`setPosition` 是外框坐标——按外框停靠时客户区整个落在屏外（看不见、鼠标也进不去，2026-10-24 实锤），
  `dockMode.ts` 的 `measureInsets()` 量出 inner/outer 差值后再算位置与尺寸
  （Tauri `Monitor.workArea` 是物理的；混合 DPI 下 Logical 会错）。**但 `setSize` 是 inner size**
  （tao `set_inner_size`）：给尺寸加 insets 会让停靠窗宽出 16px、右缘戳出屏幕（2026-10-24 实锤，
  表现为额度卡最右一列被屏幕切掉），所以 `clientSize()` 只返回 `width × workArea.height`；
  浮动窗恢复也用 `innerSize()`，别用 `outerSize()`。
- **吸附入口**：floating 时监听 `onMoved`，移动停稳 160ms 后判断窗口外缘离屏幕左右边 ≤20 逻辑 px
  → 亮 `dock-snap-left/right` 提示 → 再停 220ms 调用 `enterDock(edge, { expanded: true })`（保持展开，
  让用户看见吸附结果）；把展开的窄条拖离边缘 >48 逻辑 px、停 160ms 会 `undock({ keepPosition: true })`
  （QQ 式拖出，窗口留在松手处，不弹回贴边前的位置）。
  `setPosition`/滑入滑出自己触发的 onMoved 用 `suppressAutoDockUntil` 屏蔽（floating 吸附路径）；
  窄条拖出**不用时间窗**，而是 `DockChrome` 在 `startDragging()` 前调 `beginDockBarDrag()` 置一个
  positive 标志，只有该标志在时才判 `considerDockOut`（否则 expand/retract 的半程位置会被当成拖出）。
  **窄条标题栏不要用 Tauri 的 `data-tauri-drag-region`**：它的注入脚本只在 mousedown 目标本身带属性时
  才拖，且双击会 `toggle_maximize`——400px 窄条会被最大化。`DockChrome` 自己在 `.dock-bar` 上
  `startDragging()`（跳过 button/input），并 `stopPropagation()` 挡住注入脚本。
  footer 不再放贴边/常驻后台/退出按钮，退出只在窄条 ⋯ 与 Ctrl K 面板；窄条右上角也不放用量图标
  （用量与项目收进 ⋯）。
- **底栏（输入框下面那一栏）**：贴边时左右侧栏都是整页覆盖层，所以两侧栏把各自 footer 子树
  **portal** 到 body 级的 `#dock-footer`（`src/lib/dockFooter.ts` 的 `useDocked` / `dockFooterHost`）：
  ProjectShelf 出模型共享入口 + 主题/通知/代理/Telegram，ContextPanel 出检查更新；它们的 popover
  也必须一起 portal（`DockedLayer`），否则 `translateX(100%)` 的 rail 会连 `position: fixed` 的浮层
  一起藏掉。`.dock-mode .app-shell` 预留 40px，整页 bottom 也收 40px。
- **关闭路径**（2026-10-31 定稿）：× 的行为由用户在 Ctrl K 面板选择（dock prefs 的
  `autoHideOnClose`，Rust AtomicBool 镜像；**默认 false = 直接退出**）。退出 → `shutdown_and_exit`；
  「后台常驻」→ **Rust 在 CloseRequested 里直接 hide 主窗**（`main.as_ref().hide()` webview 级先行，
  配对 TrySuspend 补丁；二次启动交接唤回）——**前端不参与关闭路径**（无 marionette-main-close 交接、
  无 ack/看门狗，那条链已删；前端冻死也拦不住关窗）。贴边与关闭解耦：只走标题栏专属按钮
  （`WindowControls` 最小化左侧的 `window-controls__btn--dock`，调 `enterDock()`）和拖到边缘吸附。
  detached 壳照旧隐藏复用。**prefs 的 `mode:"dock"` 是活窗态，不跨启动**——启动时 normalize 回
  "off"，别把上次会话的停靠当成本次的。
- **能力命名**：capability 是 `core:window:allow-set-skip-taskbar`（不是 `allow-skip-taskbar`，
  ACL 校验在 build script 里直接报错）。`monitors_work_areas` 命令**不存在**——Tauri 2.8
  `Monitor.workArea` 自带工作区，显示器信息全走 JS API（`availableMonitors`/`currentMonitor`），
  Rust 侧只有 `dock_support.rs` 的 `set_background_mode` / `quit_app` / `fullscreen_active`
  （`SHQueryUserNotificationState`，D3D 全屏/演示模式抑制弹出）。
- **收纳盒**（`memos.rs` → `~/.marionette/memos/`）：`index.json`（原子写、读失败 fail-open 成
  空表、500 条上限）+ `files/`（内容哈希命名，重拖同内容 = 同一条目；文件夹整树复制不去重）。
  收纳盒 → Composer 拖拽是**页内 HTML5 DnD**（自定义 type `application/x-marionette-memo` +
  `text/plain`=绝对路径）；Composer 的 HTML5 drop 守卫只为该自定义 type 放行（Tauri 运行时），
  OS 级文件拖放仍走 tao 的 `onDragDropEvent`（物理像素，除以 devicePixelRatio）。
- **收纳盒交互梳理（2026-10-26，karakeep 式「收集→整理成对话」）**：
  1. **memo 页激活时整窗接住 OS 拖放**（普通窗口伪标签页 / 贴边整页两个挂载点，`MemoBox` 的
     onDragDropEvent）——**不做 rect 命中测试**：旧实现只认页面矩形，拖到侧栏/标签行 = 无声
     no-op，本机 `~/.marionette/memos/` 从未被创建过（一次都没落过盘）；`enter` 无 paths 不武装
     （页内 memo 行拖拽保持纯 HTML5 链路）。贴边态 `DockDropRouter` 在 `page==="memo"` 时**整体
     让路**（memo 页监听拥有整窗），否则路由器标题带分支会与页面监听**双保存**。
  2. **「开新对话」一律建 `origin:"memo"` 的 Chat 会话**（材料清单照旧预填输入框）：
     `create_chat_session` 命令新增可选 `origin` 参数，**白名单只认 `"memo"`**（其余回落
     `"user"`）；`storage::create_chat_session` 签名同步加 `Option<&str>`，另三个调用点传
     `None`。侧栏在「聊天」下方渲染「收纳盒」分组（`ProjectShelf` 的 `memoSessions` prop +
     两栏共用的 `renderChatRow`，**非空才显示**；状态过滤/搜索/未读计数已并入；分组拆分在
     `lib/sessionGroups.ts::splitShelfChatSessions`，命令面板用 chat+memo 合并列表）。
     「整理成文档」开着项目仍建进项目，没开项目落收纳盒组。memo 会话在运行时**就是普通 Chat
     会话**（spawn cwd/prefs/标签全走 CHAT_PROJECT_ID 原路径）——别给它开新特判分支。
  3. **10-22 悬浮球方案的 inbox 幽灵项目过滤**：rootPath 指向 `~/.marionette/inbox` 的项目
     条目（那代「收集箱会话」的家）用 `lib/sessionGroups.ts::withoutGhostProjects` 在两个
     `listProjects` 加载口过滤；本机数据已清（projects.json 条目 + inbox 整树删除，旧收集箱里
     唯一一条真实材料已迁进 memos/index.json）。
  4. 回归：`npx tsx scripts/verify-memo-box.mts`。
  5. **呈现（2026-10-26）**：单列帖子流，不是 40px 文件行。顶上 `memo-box__compose`（写下 /
     粘贴链接，Ctrl+Enter 保存，裸 URL 走既有 `memo_save_text`）；卡片
     `.memo-box__card--text/image/link/file`，按 `groupMemosByDay` 插「今天 / 昨天 / N月N日」。
     点文字卡原地展开全文（6 行 clamp），点链接/图片/文件走 `openExternal`。
     图片卡按横竖比完整显示（`memoImageOrient`：横图铺满宽、竖图触到更高上限后收窄，
     `object-fit: contain`，禁止定高 `cover` 裁切）。收/找/用通道
     （整窗拖入、页内拖到输入框、多选开新对话/整理成文档）不动。
- **2026-10-25 用户反馈批（六件）**：
  1. **拖文件到收起态把手也展开**：OLE 拖拽期间鼠标事件被抑制，sliver 的 mouseenter 不触发——
     `initDockMode` 另听 `getCurrentWebview().onDragDropEvent`（enter/over → `expand()` 并置
     pointerInside；leave → 复位 + `scheduleRetract`，否则指针标志残留会导致永远不自动收回）。
  2. **侧边栏常驻**：prefs 新增 `pinned`（⋯ 菜单切换，`toggleDockPinned`）。只抑制
     `scheduleRetract` 的**自动**收回；手动路径（close→retract、拖出、解除贴边）不受影响。
  3. **新建对话 = 目的地菜单**（DockChrome；`.dock-menu--left` 左对齐变体，基础 `.dock-menu`
     是 right:0 会溢出左缘）：聊天（默认文件夹，`handleCreateChatSession`）/ 各项目
     （`createSessionForProject`，建好即打开并切项目）/ 全部对话。App 传 `projects={shelfProjects}` +
     `onNewChatIn`。菜单状态与 ⋯ 合并成 `openMenu: "more" | "new" | null`。
  4. **贴边 composer 工具栏收成单入口**：`<Composer docked>` 只渲染 hub（滑杆图标 + 当前模型名；
     menu 状态新增 `"hub"`），hub 行 `setMenu("mode"/"permission"/"model"/"agent")` 复用原有菜单
     与提交链路；芯片本体在 `.composer.is-compact` 下 `display:none`（stream-cards.css），但**锚点必须保留在流里**（菜单挂在
     锚点内，夹边 effect 靠锚点几何定位）。always-approve 无子菜单，hub 行内直切
     （`toggleAlwaysApprove` 抽出与浮窗芯片共用，冷会话只落盘偏好）。＋ 附文件与发送钮不进 hub。
     **普通窗口 / 分屏过窄也走同一套**：composer 自身宽度 ≤580px 加 `is-compact`（贴边始终加），≥640px 才卸；量的是这一格 footer，不是整窗。
  5. **窄条宽度可拖（320–560）**：`DockResizeGrip`（DockChrome.tsx 导出；App 只在 docked-expanded
     且无整页时渲染，`position:fixed` 贴内缘全高）。换算**相对活的客户区**：右贴
     `新宽 = 当前宽 − clientX`、左贴 `新宽 = clientX`——每次 apply 后内缘正好落在指针下，下一个
     事件的 clientX 从 ~0 重新起算，无累计漂移；`applyDockWidth` 先同步改 `dockWidthPhys` 再走
     **promise 链串行** setSize+setPosition（两步不能与下一次交错），松手 `persistDockWidth` 落
     prefs（拖动中不写 localStorage）。WIDTH_MAX 由 480 放宽到 560。停靠窗 `setResizable(false)`
     不挡程序化 setSize；onMoved 在 docked-expanded 且无 `dockBarDrag` 时早退，宽度拖拽不会被
     误判成拖出贴边。
  6. **进行中列表退场时机（2026-10-26 改）**：~~waiting 安静 60s（`DOCK_DONE_GRACE_MS`）退场~~——用户反馈
     「对话一完成就消失」观感不对。现在完成（waiting）的行**一直留着，直到真正挂起**：2 小时自动挂起 /
     行内 ⏸ / 进程退出把状态翻成 `exited`（`canPark` 为 false）才从 `dockActiveSessions` 消失。
     `DOCK_DONE_GRACE_MS` 常量已删；15s tick 只用于保鲜「5m」新鲜度标签。
  7. **窄条分区接住拖放（2026-10-26，`DockDropRouter.tsx` + `lib/dockDrop.ts`）**：dock 态 App 级
     onDragDropEvent 路由接管整条窄条的 OS 拖放——**上半条（chrome 带）→ 收纳盒**、**下半条对话区 →
     composer 附件**（DOM 事件 `marionette:dock-drop-paths` 转交，composer 的原生落点 effect 在
     `docked` 时关闭）；拖动中双层 `dock-drop-veil` 虚线提示、悬停侧点亮（仿 composer 落点提示）。
     收纳盒整页开着时标题栏以下归该页自己的落点 UI，其它整页/无对话在开时全条 → 收纳盒。
     `enter` 无 paths 不武装——tao 对页内 HTML5 memo 拖拽也发 enter/over，那些必须留给纯 HTML5 链路。
  8. **文字/链接拖入（2026-10-26，vendor wry 补丁 + `lib/dragText.ts`）**：浏览器选区/链接这类
     **无 CF_HDROP 的 OLE 拖拽**在 stock wry 里被静默吞掉（DragEnter 读不到文件就 return，一个事件
     都不发），WebView2 的 HTML5 外部拖放又被 wry 关死——两头堵，前端怎么监听都收不到。
     `src-tauri/vendor/wry`（`[patch.crates-io]`，补丁说明 `vendor/wry/MARIONETTE-PATCH.md`）让
     DragEnter 在无文件时改读 `CF_UNICODETEXT`，经进程级钩子（`wry::marionette_drag::set_text_drag_hook`）
     交给 `src-tauri/src/drag_text.rs`，按 HWND 路由成对应窗口的 `marionette-drag-text` 事件
     （payload `{kind: enter|over|drop|leave, text, x, y}`，物理像素，与 onDragDropEvent 同坐标系）。
     **文件拖放的 paths 事件流一字不动**；钩子未注册时行为与 stock 完全一致。
     前端消费（`lib/dragText.ts::onDragText`）：收纳盒页激活 → 整窗收成文字/链接 memo（裸 URL 由
     Rust 端自动转 link 条目）；dock 窄条分区同文件路由（memo 带 → 收纳盒、chat 区 → DOM 事件
     `marionette:dock-drop-text` → composer 插入光标处）；浮动窗口 composer 输入框 rect 命中 → 插入
     文字；收起窄条上方悬停 → 展开（与文件拖放同款）。**页内 HTML5 拖拽（memo 行）也会从 WebView2
     转发到 OLE 目标且带 CF_UNICODETEXT**——App 挂载 `installPageDragSourceTracker()`（window 级
     dragstart/dragend/drop 标记本页是拖拽源，**别用布尔第三参的 capture 形式**，node EventTarget
     的 removeEventListener 不认），所有 drag-text 监听在 `isPageDragSource()` 时让路，保证 memo →
     composer 仍是纯 HTML5 链路。升级 wry 前先看补丁文件。

前端结构：`src/lib/dockMode.ts`（状态机 + 窗口编排 + prefs）、`src/components/DockChrome.tsx`
（窄条顶部铬：标题栏/操作/额度计量卡/进行中列表，高度量进 `--dock-chrome-top`；无标签行；全部对话/用量/收纳盒是带「返回」的整页）、`src/lib/dockFooter.ts` + `src/components/MemoBox.tsx`
（底栏 portal / 收纳盒页面）、`src/styles/dock.css`
（`.dock-mode` 变体：单列网格、左右栏转覆盖层 `dock-drawer-open`/`dock-context-open`、
标签行隐藏、body min-width 放开）。分屏在 dock 态禁用（提示解除贴边），整页在收回时自动关。
**普通窗口的收纳盒是标签页不是全屏覆盖**（2026-10-25 改）：入口仍在左侧栏「聊天」栏目正上方
（`ProjectShelf.tsx` 的 `.shelf-memo-entry`），点开后 `SessionTabs` 尾部出现一枚 `is-memo`
伪标签（App 的 `memoOpen`=标签存在 / `memoActive`=选中；`SessionTabs` 的 `memoTab` prop），
中栏只切换对话流那一列（`App.tsx` 的 `.memo-tab-page` 覆盖 `renderConversation`/SplitWorkspace，
无 composer 行；左右侧栏照常可见）。选中收纳盒时对话标签去掉 `is-active`（当前会话 id 仍记着，点回去即切回）；选中任一对话标签只取消收纳盒选中（标签保留），× 或 Esc 才关标签；
分屏态伪标签渲染在 titlebar 的 fill 前，页面同样覆盖整个中栏。`MemoBox` 组件不再有 `showClose`
（标题与关闭都在挂载点的铬里：贴边窄条的返回 / 普通窗口的标签 ×）。
聊天内容上方不放收纳盒或「能力与上下文」工具栏，分屏也不重复添加入口。
`.center-workspace` 与 `.session-pane__body` 都是「对话 1fr | composer auto」两行；
分屏时 `.center-workspace` 只有 SplitWorkspace，行改成 `1fr`。贴边入口仍由 DockChrome 提供。
**贴边 composer（2026-10-26 改）**：默认内容撑高（`.dock-mode .composer` auto + min/max，
textarea `field-sizing: content` 跟着正文长、上限 min(28vh,190px)，底边正好压在 footer 上沿）；
**顶边拖拽把手在贴边态保留**——拖动即钉住高度（`.is-dock-fixed`：field 回到浮窗式
`minmax(36px,1fr)` 填满 + textarea 内部滚动，clamp 84–min(340,52vh)），**双击把手恢复自动高度**。
钉住值存独立 key `marionette-dock-composer-height`（无 key = 自动），浮窗自己的
`marionette-composer-height` 互不干扰；拖拽 effect 里 **没实际移动的 mouseup 不落值**（别把
自动态一碰就冻住）。展开按钮（`composer-expand`）在 dock 态仍隐藏。
**Composer 下拉夹边**：`Composer.tsx` 的 menu 布局 effect 会在打开后测量每个 `.composer-menu`，按视口
8px 边距改写 `left/right`（400px 窄条里右对齐的模型菜单原先向左溢出，被 WebView 裁到只剩一条）。
**窄格 / 窄窗底栏**：分屏或撕窗把某一格 composer 压到 ≤580px 时，走与贴边相同的 hub（`.composer.is-compact`）；
≥640px 摊开芯片。阈值带 60px 回差，拖分屏条不会在边界闪。

**2026-10-25 反馈批二（侧栏状态过滤 + 挂起豁免）**：
1. **状态过滤不再「看起来像数据丢了」**（`ProjectShelf.tsx`）：点「运行中/等你/已完成/出错」chip 后，
   旧实现把无匹配对话的项目整行删掉 + 计数归零只解显示不解状态（`statusFilter` 留在 state 里，下次
   任一对话再进该状态会**静默重挂**过滤——实测「发消息后列表自己回来了」就是自动解除，用户以为见了鬼）。
   现在：纯状态过滤**保留全部项目行**、无匹配的置灰（`.project-group.is-filtered-out`，不列会话、
   不显 "No sessions"）；过滤激活时常驻 `shelf-filter-note` 提示条（标签 + 匹配数 + × 清除）；计数
   归零 `setStatusFilter(null)` 彻底清掉；**点项目/对话/聊天行即清状态过滤**（query 搜索词不动，
   搜索仍只留命中项——那是明确的收窄意图）。
2. **2 小时自动挂起不再无条件豁免当前对话**（`App.tsx` 挂起巡检）：旧规则「当前对话永不挂起」在
   窗口藏后台几小时后 = 对话永远停在「等你」+ 养着一个活 agent 进程（本次反馈的直接来源；另有
   lazy warm 在点开对话时把状态置回 waiting，等你还以为它是几小时前的残留）。现在豁免只在
   `document.visibilityState === "visible"` 时生效——藏后台/最小化到期照挂，下次发消息自动接上；
   贴边窄条窗口没有 hide、仍是 visible，照旧豁免。**子代理仍在跑、ACP 后台命令、定时发送**
   的会话一律不自动挂起（Grok `spawn_subagent` 会立刻把工具卡标成 completed，子进程其实还在
   父进程里，必须按「Subagent started in background」桩识别）。

**对话轨迹（clean view）**：连续 thought/tool 从**第一步**起就是一张组卡（稳定 `act-起点` key，
单步也分组）。运行时**上方实时轨迹面板**（`LiveActivityTimeline` → `ActivityTimeline`，
thought / tool / command / search 等泳道与时间条），**下方默认关闭的 tally 详情折叠**。
**时间图必须在 details 外面**，不能再用 live 步骤列表代替，也不能等 open 才渲染。
500ms 时钟局限在图表子组件，第一条未结束 thought 也随时钟增长；工具间 settle 保留尾段图表。
运行段结束（后续发言 / idle）移除实时图和思考预览，留下折叠 tally；整轮结束清掉详情和步骤开合状态，
保证手动展开过的卡也自动折好。历史卡手动展开仍显示时间图（**同一套占位 + 60s 标尺**，不是短轴断点）
和按类型分组；子步骤用 `renderEvent(..., true)` 禁掉工具/thought 卡片正文自动展开。
按用户后续要求，轨迹图与 tally 之间另有 `LiveThoughtPreview`：固定四行（72px 正文）显示最新
非空 thought；切工具时保留并标「最近思考」，下一段 thought 原地替换，新段空 chunk 不清空旧文字。
预览不驱动 details 开合，不给它按 thought id 加 key，也不随短 think 改高度；长文内部滚动，
尊重用户滚离底部。运行段结束随实时图一起移除。不要恢复旧的自动展开整张 thought 卡或 200px 槽。
`tailActivityFoldKey` 只找列表尾（跳过 delivery），`summarizeActivity` 的 title 永远是 tally。
时间轴有 60s 起步标尺（`turnActivity.ts` 的 `LIVE_AXIS_FLOOR_MS`）：开跑先在
固定标尺上**从左往右长**，长满后才切回「轴=已流逝时间」的相对压缩——否则第一条 think 一睁眼
就是 100% 宽（它是唯一 open 段时 0→now 恒铺满轴）。10s 会在几个工具之后就把当前 think
贴到右缘（看起来像从右边长）。**实时图和点开后的冻结图都走这把标尺**（`ActivityTimeline` 默认
`axisFloorMs=LIVE_AXIS_FLOOR_MS`），14s 的回合点开后轴仍是 0s–1m，不会把 14s 拉满整条轨道；
轴右端标签/aria 跟着 total 走，别再用回 `durationMs`。条用占位（本步开始 → 下一步开始 /
现在或 Reply 的 `untilMs`），不要用工具的 `completedAt` 墙钟——Grok write/search 经常 0–1ms，
诚实时长会让工具泳道全是小点、只有 think 在涨，点开后再退回短轴断点。尾段 thought 经常没有
`endedAt`（turn/complete 只盖最后一张 Reply），要把后面 Reply 的 `createdAt` 当 `untilMs`
收尾，否则组标题会只剩工具那几秒。实时最后一帧与冻结图的 span 起止必须一致。
回归：`npx tsx scripts/verify-activity-cluster.mts`。

## 富文本渲染管线（2026-10-26 接线定稿）

MarkdownBody 之上的懒加载富文本（公式/代码高亮/流程图/生成式卡片）。库件在 `5adf569` 入库、本次接线；改前先过这几条：

- **分流顺序**：`MarkdownBody` 先跑 `richMarkdown.ts::scanRichFeatures`（纯字符串扫描，围栏状态机与 micromark 同口径——info string 按**首 token** 小写比对）→ `mathPack.ts::useMathPack` 懒载 remark-math + rehype-katex（`singleDollarTextMath:false`，单 `$` 恒字面）→ `MdPre` 按 code 子元素 className 的语言路由：`mermaid` → `MdMermaid`、`spec` → `SpecCard`、其余 → `MdCodeBlock`（shiki 200ms 防抖 + 120 块缓存，虚拟列表重挂载不重高亮）。
- **math 归一化别动围栏内**：`normalizeMathDelimiters` 只在围栏外做两件事——`\[..\]`→`$$..$$`，**整行只有一对 `$$..$$`**（body 无 `$`）的转成多行 block 形式。后者是因为 micromark 的 math **flow**（display）要求 `$$` 后接行尾，单行 `$$E=mc^2$$` 原本只解析成段落内 text math（inline）。prose 里嵌着的 `$$..$$` 保持 inline。先跑 `prepareMarkdownForRender` 再归一化（修好围栏才判得准）。
- **hooks 顺序红线**：`MarkdownBody` 的 hooks（scan/useMemo/useMathPack）必须在 `if (!text) return null` **之前**——text 在空/非空间翻转时早退会打断 hook 链。
- **spec 卡片（生成式 UI）**：`SpecCard.tsx` 渲染 `specSchema.ts` 校验过的 card/table/steps/chart；chart 是零依赖手绘（bar=flex 条、line=SVG polyline、pie=stroke-dasharray 圆环），系列色走 `.md-spec__series-N` 自定义属性（主题 token 派生，深浅主题免费）。actions 按钮**只派发** `SPEC_ACTION_EVENT`（`marionette:spec-action`，detail=prompt 字符串），`App.tsx` 监听后 `prefillComposer`（只预填不自动发）。解析三道防护（JSON 容错/原型污染+深度/形状+尺寸上限）+ React 错误边界兜渲染异常；流式未完 JSON 安静显示「等待数据完整」+ 源码。
- **类型坑**：`requireStr` 必须是 `{ok:true|false}` 判别联合——可选属性联合在真值检查下不收窄，`string|undefined` 会漏进严格调用点。CSS 导入类型来自 `src/vite-env.d.ts` 的 `vite/client`（katex css 动态 import 靠它）。`remarkPlugins` 里带配置的插件要显式元组类型，数组推断进不了 `PluggableList`。
- 样式在 `src/styles/rich-markdown.css`（经 app.css @import，位于 transcript.css 之后）；shiki 双主题 span 颜色 = `var(--shiki-dark)`，`[data-theme="white"]` 下切 `--shiki-light`。
- 回归：`npx tsx scripts/verify-rich-markdown.mts`（scan/归一化/spec schema，24 项）。

## 再次进入对话（agent re-entry，2026-10-25）

Agent 在 `turn/complete` 之后自己续跑（后台指令回来触发新模型调用，无 client prompt）：
zcode-acp-server 0.76.2 实测（session-1792813095629）prompt 应答返回后 54s 出现 `bg_*`
tool 完成 + 新的 agent message，**没有 turnState、没有 turn/complete**——旧版这张 Reply 卡永不
封口、会话不再显示为运行中。支持对所有 ACP 走**同一条线**（不按 agent 特化）：

- 判定在 `src/lib/agentReentry.ts` + `App.tsx` 转录事件分支：会话 `waiting` 且距离上次回合边界
  ≥ `REENTRY_GRACE_MS`（2.5s，滤掉与 prompt 应答赛跑的迟到回声）时，assistant/thought 分块
  武装 re-entry（状态置回 `running`，Working 条/侧栏脉冲/实时轨迹图全回来）；武装后任何转录分块
  （含 tool）刷新安静计时。
- 收尾两条路：适配器真发了 `turn/complete`（有些会发）→ 正常回合收尾路径接管（边界块先解除武装）；
  没发的（zcode 这类）→ 静默 `REENTRY_IDLE_MS`（6 分钟，高于 activityHealth 的 5 分钟「quiet」线）
  后 `sealAgentSegmentForIdle` 封尾卡（Reply 补 durationMs、Thought 补 endedAt，与 turn/complete
  同口径：中段卡保持 open）+ 状态回 `waiting` + 桌面通知/未读。之后再来分块就再武装新段。
- **用户 prompt 一律接管生命周期**：`performSend` / edit-resend 开头就解除武装（否则安静计时可能
  在活回合中途放掉 composer）。冷启动恢复的会话（`turnEndedAtRef` 无记录）永不武装。
- 回归：`npx tsx scripts/verify-agent-reentry.mts`。

## Council 多模型协作(2026-10-31,M1–M3)

UI 名「圆桌 / 圆桌会议」(en Roundtable);代码模块、路径、事件仍叫 council。
前端版式跟收纳盒同一套表面:整栏 `--panel` 灰底、卡片
`--panel-raised` + 8px 圆角;居中栏 + 容器查询按中栏宽度收窄。
多模型审议 → 计划 → agent 执行 → 验证回路的编排器。改前先记四条:

- **模块**:`src-tauri/src/council/`(types/state/store/worker/prompts/
  orchestrator/tasks/executor/verify);数据在 `~/.marionette/council/
  runs/{runId}/`(spec.json 不可变、state.json 原子写、messages 与
  transitions 是 append-only JSONL、plan.json);恢复语义 = 以消息账本
  为进度,已完成阶段(含执行任务)绝不重放。
- **状态机红线**:`plan_ready → executing` 只能经 `council_approve_plan`
  (执行门);终态无出边;转移全走 `state.rs::apply_transition`(硬编码
  表 + transitions.jsonl 审计)。并行执行任务各持 `git worktree`
  (`council/<runId>/<stepId>` 分支),按拓扑序顺序合并,冲突显式
  `Failed(文件清单)`——**绝不静默覆盖**(真实 git 仓库的回归测试
  `executor::parallel_same_file_conflicts_are_explicit` 守着)。
- **测试**:`cargo test --bin marionette council`(43 项,沙箱在
  temp,不碰真实用户目录)+ `npx tsx scripts/verify-council.mts`;
  模型调用全部经 `ModelClient` / `TaskHarness` seam(EngineClient /
  AcpHarness 为生产实现,MockClient / ScriptedHarness 为测试),不要
  在测试里直连真引擎。验证命令的破坏性拦截清单在
  `council/verify.rs::DANGEROUS`,在 Plan Gate 冻结——运行期提示注入
  无法扩它。
- **坑**:引擎端口每次重启漂移(永远 `engine::running_base()` 现查,
  禁缓存 URL);`cmd /C` 传含引号命令会被 PATH 里的 Unix 工具劫持
  (测试里验证命令用 `cmd /C exit N` 语义,别用 `find`);测试的
  store 沙箱走 `set_root_override_for_tests` + 全局 `SANDBOX_LOCK`
  串行(store 与 orchestrator 的测试共用一把锁,别各建各的)。

## 排查「卡退」（崩溃/闪退）—— 第一件事

**用户报「卡退」时，先看 `%USERPROFILE%\.marionette\logs\dev.log`（开发者日志）。**

- 搜 `[panic]` 行，能直接给出 panic 位置和消息，例如：
  `[error] [panic] session=- panicked at src\terminal_runtime.rs:79:25: assertion failed: ...`
- release 配置是 `panic = "abort"`（`src-tauri/Cargo.toml`），**任何线程 panic 都会杀死整个进程**，所以 panic 行 = 崩溃根因，不用再猜。
- 同一时间附近有 `[watchdog] MAIN THREAD STALLED` 表示主线程冻结（假死），一般与 panic 无关，是主线程上的耗时操作。
- Windows 事件日志（Application / Application Error）只提供退出码：`0xc0000409` ≈ Rust panic-abort（fail-fast），`0xc0000005` 是访问违规。
- 崩溃转储：`C:\ProgramData\Microsoft\Windows\WER\ReportArchive\AppCrash_Marionette.exe_*`（有时能拿到 Report.wer 的堆栈模块偏移）。

### 已知崩溃历史

- **PTY 输出截断 mid-codepoint**（`terminal_runtime.rs` 的 `snap.output.drain(..overflow)`）：
  从 PTY 读到的字节截断点可能落在 UTF-8 多字节字符中间（中文/emoji），`drain` 的 `is_char_boundary` 断言 panic → 整个 app abort。
  已修复为回退到字符边界再 drain。**修这类问题不要再写裸 `truncate`/`drain(..n)`**，先保证落在 char boundary。
- **tao 0.35 paint assert**（`flush_paint_messages` 重入）：
  关窗时 WebView2 双 pump paint **以及** CJK IME 在窗口获得焦点/被点击时嵌套泵消息（日语系统 + 中文输入法最容易中）都会踩
  `assert!(flush_paint_messages(..))`。关窗路径：`main.rs` 的 `is_tao_paint_assert` + `shutdown_and_exit`（不要再改那条）。
  点击/IME 路径：`src-tauri/vendor/tao` 把 assert 改成尊重 re-entrant `false`（见 `vendor/tao/MARIONETTE-PATCH.md`）。
  Tauri 升到 tao ≥ 0.36 后删掉 `[patch.crates-io]` 的 `tao` 行。
  另一条 `wry` patch 行是文字/链接拖放（见上文「贴边停靠」第 8 条 + `vendor/wry/MARIONETTE-PATCH.md`），
  升级 wry/tauri 前两条都要对着补丁文件核对能否撤掉。

## 排查「exe 双击没反应 / 构建报 esbuild Access is denied」—— 先查完整性标签

**症状 1：** 双击 `release\<版本号>\Marionette.exe`（或 `target\release\marionette.exe`）完全没反应：没有窗口、没有日志、
退出码 0，`dev.log` 一行都不加。
**症状 2：** `npm run build` / `build-portable.bat` 反复失败：
`[vite:esbuild-transpile] remove C:\Users\...\AppData\Local\Temp\esbuild-<hex>: Access is denied.`，
且在 `%LOCALAPPDATA%\Temp` 里能看到那个删不掉的 1.7 MB `esbuild-*` 残留文件。

**同一个根因：程序以 Low 完整性（Low integrity）运行。** Windows 取「令牌完整性」和「镜像文件完整性标签」中较低者作为
进程完整性；低完整性进程对 `%USERPROFILE%\.marionette`（Medium）和真实 `%TEMP%`（Medium）只能读不能写：

- Marionette 打不开 `instance.lock` → 走单实例交接分支 → **静默 `exit(0)`**（`shell_integration.rs`）；
- esbuild 的 Go 进程删不掉自己写在 `%TEMP%` 的临时文件 → `remove ...: Access is denied.`。

**来源：** Windows 沙箱（如 DSH 的 workspace-write 限制）会给工作区目录加一条**可继承的 Low 完整性标签**，
于是 `D:\Apps\Marionette` 下**所有 .exe**（包括 `Marionette.exe`、`node_modules\@esbuild\win32-x64\esbuild.exe`，
甚至拷进来的 `cmd.exe`）都以 Low 跑起来——沙箱早已结束，标签却留在文件上，双击也一样中招。

**确认（看到 Low 即确诊）：**

```powershell
icacls D:\Apps\Marionette | Select-String 'Mandatory Label'
icacls D:\Apps\Marionette\Marionette.exe | Select-String 'Mandatory Label'
```

**修复：** 跑 `scripts\fix-workspace-integrity.ps1`（等价于下面这条；只改完整性标签，不动文件内容、所有者或访问权限）：

```powershell
icacls D:\Apps\Marionette /setintegritylevel (OI)(CI)Medium
```

改完 `npm run build` 与 `release\<版本号>\Marionette.exe` 立即恢复正常。**只要本工作区再被沙箱以 workspace-write
限制跑过一次，标签就会再来一遍**，所以再次出现上述症状时先跑这个脚本。

**产品侧兜底（已加）：** 启动时 `app_paths::ensure_global_dir_writable()` 会写探测文件，失败则走
`data_dir_gate::prompt_and_exit()` 弹原生对话框并 `exit(1)`，不再出现「静默 exit(0)」；
单实例锁只有真被占用（Windows 32/33 共享冲突）才交接，权限类错误会记一条 `[shell] warn` 后照常启动。

## 交付卡 / 每轮改动追踪（2026-10-23 修）

**别再用 `git status` 或逐文件 `git diff` 指纹当「本轮改了什么」——口径不一致会造假。**

- 本仓库实测：`git status -uall` 报 224~227 条，`git diff HEAD` 真实内容差异只有 52 条
  （`third_party/magpie/**` 这些 `text=auto`/CRLF 文件 stat-dirty 但 diff 0 行）。旧实现「status 列文件 +
  diff 算指纹」于是给 186 个文件留空指纹；再加上基线 `catch { return [] }` 把失败当成「基线是空的」，
  交付卡就成了「整库改动」展示器。
- 现行实现：`git_service::begin_workspace_snapshot` / `end_workspace_snapshot` / `discard_workspace_snapshot`
  —— turn 起止各拍一棵 worktree 内容树（私有 `GIT_INDEX_FILE` + 私有 object dir + alternates），
  `diff-tree -r -M --name-status/--numstat` 出真 `+/-`。**性能要害：私有 index 必须先复制用户
  `.git/index` 当 stat 缓存（0.6–1.2s）；裸 `read-tree`+`add --all` 要 8.6s。** 别写回用户 index。
- 归属规则在 `src/lib/turnDelivery.ts::selectTurnWorkspaceChanges`：起点干净→结束脏无条件算；
  **起点就脏的必须被本轮 edit/write 工具碰过才算**（否则同工作区的另一场对话会串进本卡）。
- **fail closed**：基线失败 / 不是 git 仓库 → 不发布任何 git 真相，卡片只留工具账本。
- 口径与实测数据另见 `docs/CURRENT.md`「交付卡（每轮改动的口径）」；回归用例
  `npx tsx scripts/verify-turn-delivery.mts` + `cargo test --bin marionette git_service`。

## 资源优化批（2026-10-30：引擎内存 / WebView2 挂起 / dist 瘦身）

体积与内存的**实测账**（完整推导见当日会话，数字按 magpie v0.1.1877）：发布 exe = 引擎
UPX 载荷 15.4MB（UPX/xz/xz+BCJ 三个压缩器实测打平，**这是熵极限，别再试压缩器**）+
壳与 dist 的 UPX 压缩。空闲进程树 ≈ 主进程 55.8MB private + WebView2 六进程 ~243MB +
magpie 120–160MB——前两个是「运行时入场费」，调不掉，能动的杠杆全在本批：

1. **引擎内存**（`engine.rs::spawn_serve`）：注入 `GOMEMLIMIT=128MiB`（Go 软上限，
   用户自设不覆盖），空闲 private 从 ~160 往 ~100 压；同点 `assign_child_to_kill_job`
   把 magpie 挂进 **Job Object（kill-on-close）**——崩溃/强杀后不再留孤儿 magpie
   （实测孤儿各 ~100-160MB 且要几分钟才自退）。沙箱把本进程收编进别的 job 时
   Assign 会失败，只记 warn 不致命。
2. **WebView2 挂起**（vendor/wry 补丁 #3/#4，细节 `vendor/wry/MARIONETTE-PATCH.md`）：
   `set_visible(false)` 追加 `TrySuspend`（隐藏时释放 ~150-250MB renderer 内存）、
   `set_visible(true)` 与 `focus()` 解除。**红线：tauri 的 `window.hide()/show()` 只走
   tao 层、不碰 wry 的 set_visible**——所以每条隐藏/唤醒路径都必须**成对**补 webview 级
   调用：前端 `getCurrentWebview().hide()/show()`（dockMode 兜底 hide、detachedWindow
   的 `withDetachedWebview`、App 的 merge-back show），Rust `win.as_ref().show()`
   （shell_integration 二次启动交接）。**漏掉 show 半边 = 唤醒后白屏窗口**，新增加
   show 路径时先想到这条。capabilities 已加 `core:webview:allow-webview-show/hide`。
   贴边窄条**永不触发挂起**（窗口停在屏外是 visible，DOM mouseenter 靠它活着）。
   创建时统一 `MemoryUsageLevel=Low`（逃生门 env `MARIONETTE_WEBVIEW_MEM=NORMAL`）。
3. **dist 瘦身**（19MB → ~11MB）：`MdShikiCode.tsx` 从全量 `import("shiki")`（317 门
   语言 12.2MB）换成**精选 36 门语法表 + `createJavaScriptRegexEngine`**（oniguruma
   wasm 622KB 一并省掉；JS 引擎略慢，但 200ms 防抖 + 120 块缓存兜住）。**加语言 =
   改 `LANG_LOADERS` 表**（grammar 自带别名 sh/py 免费注册；额外别名进 `LANG_ALIASES`），
   表外语言回落纯文本，行为与旧版一致。**mermaid 不裁**：v12 的 core 在 parse/render
   里无条件注册全部图表懒加载器，无官方按需入口，深路径 DIY 对升级太脆——4.6MB
   mermaid 全家桶保留（UPX 后 ~1MB）。katex/音效不动。
4. **引擎种子精简版**（2026-10-30，payload 19.6 → **12.0MB**）：stock magpie 是完整
   多 agent 管理器，`internal/cli.Main` 让十几个 agent 安装器/TUI/导入器/OAuth 全部
   从 main 可达（链接器一字节不砍）。`tools/magpie-seed/main_seed.go`（build tag
   `marionette_seed`）绕过 internal/cli，只挂 **gateway/routing/usage 三个插件**只跑
   `serve`；`import-magpie.ps1` 每次导入后自动重放补丁（拷 main_seed.go + 给上游
   main.go 加 `!marionette_seed` guard），`build-portable.bat` 用
   `-tags "nogui marionette_seed"` 编。**上游重构 serve 路径 = 编译报错（响亮）**。
   种子刻意不做：agent 管理、`magpie quota --json` CLI（配额走 HTTP
   `/v1/magpie/quotas` 主路径）、codex 目录保鲜 goroutine（会拖回整个 internal/agent；
   `catalog.KeepFresh`+`provider.FetchNew` 仍在）。用户「更新引擎」装完整版后种子让位。
   **0.5.3 起 release exe ≈ 15.9MB**（壳+dist UPX ~4.7 + 种子 12.0）。

## 2026-10-30 修复批（面板可关 / effort 档位 / 来源注册三连）

1. **「模型与来源」面板刷新期间可关闭**（`ModelLibraryDialog.tsx`）：打开即自动刷新
   （来源扫描+注册可能好几十秒），旧实现 busy 期间禁掉 backdrop/×/取消——像卡死。
   现在关闭路径永不因 busy 禁用（fetch 继续跑，React 18 卸载后 setState 是无害 no-op）。
2. **grok-build 档位补 max**（`acpSupplements.ts`）：桥接模型（marionette/*）和 grok
   config.toml [model.*] 条目没有 `_meta.reasoningEfforts` live 菜单，全走 fallback——
   旧 fallback 只有 3 档。Rust 侧 `clamp_effort_id` 对无菜单模型**透传**（acp.rs），
   所以加 max 只需扩 fallback 列表；live 菜单仍然优先，不影响 grok 自家模型。
3. **Grok `api_backend="messages"` 接入共享**（`model_sources.rs`）：grok 把 Anthropic
   线协议叫 "messages"，旧映射只认 "anthropic" → 整族模型被跳过。`"anthropic" | "messages"`
   都归 anthropic 协议，`anthropic=<url>` 的注册形态已实测可用。
4. **来源注册失败根治**（`engine.rs`）：发现来源带着主机名 id（`deepseek.com`/
   `opencode.ai`/`open.bigmodel.cn`），magpie CLI 的 provider id 只收小写字母/数字/
   横线——`provider set`（不存在）与 `provider add`（id 非法）两头失败，面板永远
   "来源注册失败，可刷新重试"。现在 CLI 前过 `provider_id_slug`（合法 id 恒等，
   账本无缝）。顺手：`run_magpie_cli` 错误带 stderr；`provider rm` 报 "no provider"
   视为已回收（曾每秒刷屏 dev.log）；种子引擎拒绝 provider 子命令时给"更新引擎"
   指引而不是含混重试提示。
5. **来源模型清单 live 化**（`engine.rs`，同日）：旧注册把发现时的模型清单**钉死**
   （`models=`），中央中转（opencode.ai 免费池、openrouter `:free`）新模型永远
   进不来——grok 里只剩老快照。现在注册不带 `models=`（add 自动抓厂商 live
   列表），已钉住的来源经 `provider refresh` → `provider models <id> all` 迁移到
   全量 live 语义（**顺序不能反**：`all` 暴露的是最近一次抓取的清单；refresh
   失败保持旧列表）。手动面板刷新全量 force；引擎拉起等自动链路每来源 6h 一
   次（`maybe_refresh_source_live`，进程重启首轮必刷；2026-10-31 起失败退避 10min、
   注册链后台化——见「内存压力韧性批」）。语义均经本地假厂商服务
   实测（sandbox：钉死 add→refresh 不露新、no-picks add→live 全露、all 解钉）。

## 内存压力韧性批（2026-10-31：转录分页 / 落盘减压 / 启动解卡 / 关窗兜底）

**事故定调**：外部 llama 基准吃掉 ~110GB 内存 → 全机缺页风暴，ZCode/浏览器存活、
Marionette 前端失去响应（点新建对话、点关闭都没反应），Rust watchdog 全程 12s 心跳
健康、零 panic。**当晚实锤的第二个真凶见第 9 条：storage 锁被持锁跑 git 霸占**——
「点什么都没反应」是它，缺页风暴只是放大器。

**0.「没法新建对话」的真凶（当晚 CDP 实锤的根因链）**：`get_changed_files` 等命令
**持全局 storage 锁跑 git 子进程**，而 `output_with_kill_timeout` 是「先 try_wait 等
退出、退出后才读管道」——`git status --porcelain -uall` 在本仓输出 **69.7KB，刚好
超过 64KB OS 管道缓冲**，子进程写管道阻塞永不退出，父进程干等到 60s 超时杀进程，
**storage 锁全程被占**；前端 12s 一次的 git 轮询让它几乎永久饿死所有 storage 命令
（新建对话/会话列表/转录加载）。修法两层：① `output_with_kill_timeout` 双管道并发
排水（超时路径不 join 读线程——kill 只杀直接子进程，孙进程攥管道句柄，同步等会把
超时拖成等整棵进程树）；② git/交接类命令锁内只解析路径（复用 `project_root_of`），
子进程一律锁外跑。**新增加 storage 锁的命令时照这个形状写，别在锁内等任何子进程。**
连锁修复：`persistableEventsForSession` 的 map 每次重建 `{...event}` → append 快路径
的「前缀引用全等」判定永不成立 → 截断会话每次 flush 误触发 ensureFullTranscript
静默补头（`publish:false`）再清 olderHeads——哨兵消失、界面卡尾部。修法：WeakMap
引用缓存 + 补头 `publish:true` + olderHeads 走 ref+tick（异步派生展示态免疫并发
更新丢失）。

1. **转录尾部优先加载 + 向上翻页**：`load_transcript` 命令带可选 `tail`（App 用
   `TRANSCRIPT_TAIL_EVENTS=800`）；Rust `load_transcript_window`（storage.rs）tail/range
   两模式，只 parse 窗口内行、空行不计数；前端 `olderHeads`（App.tsx）记每会话截断
   起点，SessionView 顶部「加载更早」哨兵（`event-list__older`），前插后按
   scrollHeight delta 平移 scrollTop 保锚。**全量消费者红线**：编辑重发 / agent 交接 /
   fork / 撕窗合并 / 冷热淘汰一律走 `persistRailsNow`——它先 `ensureFullTranscript`
   补头再全量写；**rail 只装了尾部，直接 `writeTranscript` 会把磁盘上的历史头吃掉**。
   `repairInjectedLabels` 只处理打开的标签且 idle 推迟（曾经启动期串行全量加载）。
2. **流式落盘减压**：rails（useSessionRails.ts）`lastPersistedRef` 前缀**引用**比对 →
   纯追加走 `append_transcript_events`（JSONL 追加，不再每 450ms 全量序列化+整文件
   重写——24MB 转录在缺页风暴下每次重写都是秒级）；非纯追加全量写；事件 >3000 时
   debounce 升到 2s。append 失败自动退回全量。**新增直接写盘点必须过 persistRailsNow
   或 rails 的 armTranscriptTimer**，别绕过截断守卫。
3. **write_atomic 加固**（storage.rs）：进程级写锁——两个并发 write_atomic 共用同一
   个 `.json.tmp` 名字会互吞 rename（"os error 2" 实锤，且最终 rename 失败时原文件已
   被挪去 .bak、主路径缺失）+ 3 轮重试 + 兜底直写（弃原子性保数据）+ 失败还原 .bak。
   `append_transcript_file` 共用同一把锁。
4. **关窗兜底**（main.rs）：后台模式 CloseRequested 交给前端后启动 2s 看门狗
   （`arm_main_close_handoff_watchdog`）；前端 `handleCloseFromRust` **一进来就**
   `ack_main_close`（别等 retract 动画收完，否则慢一步 Rust 就把贴边窗整个 hide）；
   超时 Rust 兜底 hide——webview 级 `as_ref().hide()` 先行，与 shell_integration 交接
   的 `win.as_ref().show()` 成对（TrySuspend 白屏红线）。此前前端冻死时连 Alt+F4 都
   没人应答，窗口悬死。
5. **孤儿 magpie 收割**（engine.rs `reap_orphan_engines`，enable_pipeline 第 1 步）：
   toolhelp 快照 + 镜像路径在我们 `engine_dir` 下（含 `.old` 改名镜像与 bun 缓存）+
   父进程已死 → Terminate。**fail-open**：PID 复用只会「误判为活」从而跳过，绝不
   误杀。Job Object 创建失败补了 warn（原来静默）。
6. **cargo test 日志窜流已修**：debug_log 的 `log_path()` 在 `cfg!(test)` 下拒绝真实
   `~/.marionette/logs/dev.log`（格式化照跑、只断文件 sink）。**回归口径**：跑完
   cargo test，`grep -c "marionette_test\|9\.9\.9" dev.log` 零增长。
7. **启动链解卡**（0.5.4 用户机实测 start_acp_session→ready 58–60s，0.5.3 <3s）：
   `run_magpie_cli` 全线 10s 超时（`process_util::output_with_kill_timeout`，超时杀
   子进程）；注册/来源同步移出 enable 关键路径（`spawn_provider_registration` 后台
   线程，Running 状态先行、目录靠 drift 事件跟上）；**种子引擎短路**——
   `engine/cli-absent.marker`：装内嵌种子落标记、GitHub 完整版摘标记、真实 CLI 被
   拒（stderr 含 "engine seed"）自学习补写；注册账本 `save_registered_ledger` 落盘
   前去重排序（`opencode-go` 曾累积 41 份，存量脏账本下次保存自愈）；
   `maybe_refresh_source_live` 失败退避 10min（成功 6h）；菜单路径
   `sync_discovered_sources(false)` 不排队不强制（try-lock 让位），手动面板刷新才
   force。
8. **UI 心跳与周期 tick**：前端每 60s `pushDebug({source:"ui-heartbeat", …})`——下次
   事故 dev.log 直接可判「JS 活没活」（本轮事故只能靠推断，context scan 是事件驱动
   不是定时器）。`document.hidden` 时暂停 3s 终端轮询 / 1s live tick / 1s stats /
   12s git 扫描，可见即续（定时发送、挂起巡检等安全类 tick 不受影响）。

## DeepSeek Harness 接入（@openma/deepseek-harness-acp，2026-10-19 起）

Marionette 的 DSH agent 走 `@openma/deepseek-harness-acp`（bin `dsh-acp`），不再用停更的社区适配器
`deepseek-acp`（xintaofei，0.9.0，2026-09-10 停更）。要点：

- **安装**：`install.package = @openma/deepseek-harness-acp` + 依赖 `@deepseek-ai/dsh`（引擎，
  与 codex-acp/@openai/codex 同款「适配器 + 引擎」模式）。Node ≥ 22.15。
- **spawn 环境两件事**（`acp.rs` 的 DSH 分支）：
  1. `DSH_ACP_HOST` = 全局 `@deepseek-ai/dsh` 包目录（`dsh_runtime_dir()`：从 PATH 上的 `dsh`
     启动器旁推 `<prefix>/node_modules/@deepseek-ai/dsh`，再兜底 `%APPDATA%\npm` 布局）。
     **不要让适配器落回自带 runtime**：首启会把带符号链接的 tar 解压到 `%LOCALAPPDATA%\dsh-acp`，
     普通 Windows（无开发者模式）建符号链接直接 EPERM 崩（实测 2026-10-19，dsh-acp 0.4.39；
     openma CI 的 windows-latest 开着开发者模式，所以上游测不出来）。用户自设
     `DSH_ACP_HOST`/`DSH_PATH` 时尊重不覆盖。
  2. `DSH_SESSION_ROOT` = `$DSH_HOME/sessions`：standalone 默认是 `~/.dsh-acp/sessions`，
     不指过去会和 dsh web、Marionette 的会话导入（`parsers/deepseek.rs`）分家。
- **登录**：`dsh-acp login`（独立控制台窗口跑交互输入）；凭据仍落 `~/.dsh/.credentials.yaml`，
  或 `DEEPSEEK_API_KEY` env（`acp.rs` 会从 OpenCode auth.json 兜底注入）。
- **模型目录 / 推理档位**：适配器从**运行中的组合**出 live catalog（Web UI 里加的第三方 provider
  立即出现，id 稳定为 `provider::model`），effort（off/low/high/max）是独立 configOption。
  旧的「settings.yaml 桥接」hack（`dsh_settings.rs`，724 行）已整体删除；「pi-ai 路由模型丢失」
  「无推理档位」两个老问题随换适配器消失——DSH 0.2.x 把设置搬进 profile patch 的历史背景
  见 git 历史（本次提交前的 AGENTS.md）。
- **跨用桥（2026-10-20，`dsh_bridge.rs`）**：spawn 时把 llm-pi-ai 托管路由 `marionette`
  （openai-completions → 引擎 `/v1`）写进 **`~/.dsh/cordis.patch.yml`**（standalone dsh-acp 组装的
  home 层；settings.yaml / `profiles/acp/` 它**都不加载**——`settings` 行在无 profileContext 时
  disabled），令牌常量 `magpie` 同步进 `.credentials.yaml` refs + spawn env（解析顺序 env → 文件）。
  文本外科手术合并（home patch 允许 `!!js`，禁 serde_yaml 整档重写）、幂等、引擎停用即移除。
  每个模型带引擎目录的 `contextWindow`/`maxTokens`（`engine::catalog_direct_entries()`，引擎
  `/v1/models` 的 `context_window`/`max_output_tokens`）——**不写的话 dsh-llm-pi-ai 对所有
  marionette 路由模型落 `DEFAULT_CONTEXT_WINDOW = 262144`（256K）兜底**，1M 的 DeepSeek 会
  被显示成 256K、自动压缩阈值跟着错（2026-10-21 实测修复）。
  能看图的模型再补 `input: [text, image]`（`CatalogModelRow.images`，引擎 `modalities.input`）：
  **不写就是 pi-ai 默认 `[text]`，DSH 会把附图换成「image omitted because this model accepts
  text only」占位文本**，图根本不出适配器（2026-10-21 实弹复验，见 §8.6）。
  模型 id 广告为 `marionette::provider/model`（首个 `::` 切分，斜杠安全）。定稿与探针记录：
  `docs/research-dsh-cross-use.md` §8。

**NDJSON 探针**（不起 GUI 直接问适配器）：`spawn dsh-acp`（记得设 `DSH_ACP_HOST` 或会踩上面的
符号链接坑），stdin 依次发 `initialize`（`protocolVersion: 1`）和 `session/new`（`cwd` 必须是
绝对路径、`mcpServers: []`），读 `id=2` 应答里的 `configOptions` → `id: "model"` 的 `options`。

**`cordis.patch.yml` 的自锁坑（2026-10-23 修）**：移除托管路由时我们落的是
「说明注释 + `[]`」，而合并入口以前只认字面 `[]` —— 引擎闲置回收/停过一次之后，这个文件
被永久判成「不是顶层数组」拒写（dev.log 只有一条 `dsh marionette route refresh failed;
bridge skipped` warn，界面表现是「DSH 没有共享模型」）。现在空 patch 判定按语义来
（只有注释/空行/`[]` 即当空文件重写）、移除时连带收走说明行，回归测试
`rewrites_comment_only_empty_patch` / `remove_then_readd_round_trip`。
前端同轮补课：跨用目录缓存现在会随 `invalidateBridgeModels()` 通知订阅者（Composer 当场重拉），
引擎在菜单打开期间醒来不再需要切会话才刷新。踩坑定稿：`docs/research-dsh-cross-use.md` §8.7。

**仍成立的上游限制：pi-ai catalog 没有的 model id 会被 DSH 丢掉。** `dsh-llm-pi-ai` 是 DSH 官方唯一的多
provider 层，只服务 catalog 描述得出的模型（本机 catalog 在
`@earendil-works/pi-ai/dist/providers/data/opencode*.json`）。**不要给既有路由补 `api:` 来救** —— 路由级
`api` 优先级高于每个模型自己的协议（`api = request.api ?? base?.api ?? routeApi`），而 `opencode` 这类
catalog 路由横跨 `openai-completions` / `openai-responses` / `anthropic-messages`：补一个 `api` 会把同路由
里别的模型的协议一起改掉。**上游处方是拆路由**（README「Known Limitations」）：新开一个 catalog 不收录的
route key，自己声明 `api` + `baseURL` + `apiKeyEnv` + 只列这一个模型——实测有效，且不动原路由上其它模型的协议。
## 模型共享引擎的凭据源（2026-10-23）

引擎注册（`engine.rs` 的 `register_key_providers`）钥匙来源按优先级：**Marionette 凭据库
key-plan → ZCode 个人 provider 层（`~/.zcode/v2/provider_config.json` 的
`access.type=zhipu-coding-plan-api-key` → glm preset）→ opencode 扫描**，同 preset 先到
先得。两个必记事实：**opencode 凭据已迁 `opencode.db` 的 credential 表**（auth.json 迁移
后不再更新，只读它会拿到死钥匙；合并逻辑在 `credentials.rs` 的
`peek_opencode_db_keys_at` / `merge_opencode_key_sources`，active=1 优先）；**ZCode 设置页
配的 Coding Plan key 不落 v2/config.json**（那里 `options.apiKey` 恒空，本体在个人层）。
完整踩坑链、额度端点规矩与排查速查：`docs/pitfalls-model-share-quota.md`。

## ZCode 接入（社区适配器 `zcode-acp-server`，2026-10-23 起）

> 完整踩坑集（十一雷 + 快速确诊表，含日志证据链）：`docs/pitfalls-zcode-acp.md`。改 zcode 相关代码前先过一遍。

ZCode 官方**没有** ACP（CLI 0.16.x 无 `--acp`，ACP registry 未收录）；走的是社区桥
`zcode-acp-server`（npm，Apache-2.0，bin `zcode-acp-server`）：ACP ↔ `zcode app-server`
（ZCode 私有 stdio 协议 "ZCode Protocol v1"，NDJSON、**无 jsonrpc 信封**，`{id, method, params}`）。
底层协议已实测：`session/create` 需 `workspace{workspacePath,workspaceKey}` 且要应答服务端反向请求
`session/requestRuntimePreferences`；事件流 `state.updated`（revision 补丁）/`v4/telemetry/event`；
转录走 `session/messages`。**懒启动**：适配器首个 `session/prompt` 才 spawn 后端。

- **spawn 注入三件事**（`acp.rs` zcode 分支，用户自设对应 env 时不覆盖）：
  1. `ZCODE_BIN` = 桌面版内置 `resources\glm\zcode.cjs`。桌面版可装任意目录、**注册表卸载键不登记**
     （本机 D:\Program Files\ZCode），探测链：`%LOCALAPPDATA%\Programs\ZCode` → `%ProgramFiles%\ZCode`
     → 开始菜单/桌面 `ZCode.lnk` 里抽 ANSI LocalBasePath 与 UTF-16 路径（`zcode_bin_path()`；只扫 UTF-16 会命中假路径，见 `docs/pitfalls-zcode-acp.md` 雷 10）。
  2. `ZCODE_NODE` = 起适配器的那台 node。适配器对 .cjs 型 ZCODE_BIN 自己找 sqlite-capable node，
     候选链（ZCODE_NODE → unix 路径 → which node → Zed bundle）在 Windows GUI PATH 下全空，
     最后裸 exec .cjs → **`spawn EFTYPE`**（实测 2026-10-23，0.76.2）。
  3. `ZCODE_ACP_RUNTIME=node`：桥发现机器装了 Bun ≥1.4 会把自己迁移到 Bun 跑（从未实测的
     runtime），无条件钉 node（2026-10-26 落，详见 `docs/pitfalls-zcode-acp.md` 雷 15）。
- **MCP 借贷只借远程**：适配器 `session/new` 的 zod schema 只收 url 型（http/sse/acp）条目，
  **stdio 条目让整个 session/new -32602**——所以 `agent_registry::accepts_stdio_mcp("zcode") = false`，
  stdio 在 `mcp_payload_for_agent` 里跳过并记 note。远程条目 per-load 合并，ZCode 自己 `~/.zcode`
  的 MCP/插件（node_repl 等）照常加载；实测 http MCP 借出后模型能真实调用工具（E2E
  PONG:marionette-http-mcp-ok）。反向共通：`context_inventory` 扫 `~/.zcode/cli/config.json`
  的 `mcp.servers` 与 `~/.zcode/cli/skills`（用户层；插件技能不入清单，与其它 agent 的插件口径一致）。
- **stdio MCP 走 zcode 本地配置桥（2026-10-25，`zcode_mcp_bridge.rs`）**：上面的 schema 限制绕不开，
  换路线——ZCode **后端**会原生加载 `~/.zcode/cli/config.json` 的 `mcp.servers`（stdio：
  `type/command/args/cwd/env`，zod strict，见 zcode.cjs 的 `Nso/pJs/Bpt`；适配器 `/mcp` 读的也是
  这个文件）。「能力与上下文」勾选借贷的**全局** stdio MCP（所有项目勾选的并集；project 层
  server 不进机器级文件）幂等合并进去，桌面版与 Marionette 里的 zcode 会话都原生加载。归属记账在
  `~/.marionette/zcode-mcp-bridge.json`（条目 schema 塞不进标记键）：只动自己名下的条目，用户
  在 zcode 里自己配的 server 永不触碰；与 zcode 插件 MCP 同名（`cli/plugins/cache/**/.mcp.json`
  不分启用全量扫）整个跳过防双开。config.json 原文存在但不是合法 JSON → 报错不同步（宁可不修
  也不覆盖可能救得回的配置）。同步时机：`set_project_context_enabled`（mcp kind）+ 每次 zcode
  spawn（acp.rs zcode 分支，紧跟 provider 桥，幂等自愈）。回归：`cargo test zcode_mcp_bridge`。
- **登录/凭据**：与桌面版共享 `~/.zcode`——provider 密钥在 `v2/config.json`（`provider.*`），
  OAuth 令牌在加密的 `v2/credentials.json`（`probe_zcode_auth` 探这两个）。登录 = `zcode login`
  （PATH 没有 zcode 时 `commands.rs` 自动改跑 `node <zcode.cjs> login`，独立控制台）。
- **configOptions 全广告**（model/mode/thought → 模型/模式/推理档位芯片直出，无需 acpSupplements）；
  Node ≥ 22（node:sqlite）；ZCode CLI ≥ 0.14.8；权限经 `session/request_permission` 桥接。
- **跨用桥（2026-10-23，`model_bridge.rs`）**：spawn 时把托管 provider「marionette」
  （openai-chat-completions → 引擎 `/v1`，apiKey 用引擎常量令牌）合并进 **`~/.zcode/v2/provider_config.json`**
  ——zcode 的**个人** provider 层（桌面版 3.12+ 新增 provider 的正位；账号层 `v2/config.json`
  桌面版自己重写不能托管，且适配器的 env 注入只扫账号层、不会被劫持）。三处托管位：
  `providerOrder`（**追加**尾部，不动默认模型）、`providerConfigRules.providerRules`、
  `modelConfigRules.providerModelRules`（每模型 contextWindow/maxOutput/图片输入——不写会落
  内置 `.*` 兜底 200K + 不收图，同 DSH 的 256K 教训）。**个人层 schema 不收内置表那种
  regex `modelRules`**——一个未知键整层被 zod 拒掉、内存降级空配置（实测，zcode 日志
  `Personal Provider Config 加载失败`）；按 `providerId` 识别托管条目，幂等、引擎停用即移除。
  模型 id 广告为 `marionette\<engineId>`（适配器 configOptions 的 `provider\model` 反斜杠拼法，
  前端钉原生偏好用此前缀，`isBridgeNativeModel`/`modelLabel` 已收 `\` 形态）。E2E 实测：
  写入后 `model.available` 出现 `marionette\glm-5.3-e2e | ctx:1000000 | out:65536 | img:true`。
  **`manualProviderModelRules` 是必填数组**（2026-10-24，雷 14）：桥从头建文件（重装后
  `~/.zcode` 全新）缺这个键，整层个人配置 zod 拒载、内存降级空配置——每条 prompt 都死在
  model_creation 的「Select a model before continuing」。合并体现在对缺失/非数组补 `[]`，
  已有值原样保留；回归 `zcode_merge_backfills_manual_provider_model_rules`。
- **会话导入（2026-10-23，`parsers/zcode.rs`）**：ZCode 的对话主库是
  `~/.zcode/cli/db/db.sqlite`（`ZCODE_HOME` 可挪根）——`session.directory` 做项目
  过滤；转录在 `message`（时间线事件，role + semantics.kind）+ `part`（内容分片，
  按 message_id 关联）两表。只导出真实对话轮（user/user_prompt、
  assistant/assistant_response），system_reminder/todo_reminder/timeline_event 是合成
  事件跳过；`sess_subagent_*` / 带 parent_id / 已归档的行不进列表。实机冒烟：
  `cargo test zcode -- --ignored`。
- **NDJSON 探针**：`npx -y zcode-acp-server`（配 `ZCODE_BIN`/`ZCODE_NODE` env），stdin 发
  `initialize` → `session/new`（`cwd` 绝对路径），读 `id=2` 的 `configOptions`（id: model/mode/thought）。
- **个人层 provider 的模型要 `providerModelRules` 才进 ACP 菜单**（2026-10-23）：适配器
  `loadAllModels()` 对只在 `provider_config.json` 里的 provider 只认
  `modelConfigRules.providerModelRules`，从内置模板建的 provider（如桌面版「BigModel
  Coding Plan」）一条都没有 → 桌面能选、ACP 菜单里整家消失。已用
  `scripts/patch-zcode-acp-personal-models.mjs` 给适配器补回退链（→ `personalModelIds`
  → 模板 `builtinModelIds`，上下文窗口同源兜底）。**`npm i -g zcode-acp-server`
  会覆盖 `dist/`，升级后重跑该脚本**；细节见 `docs/pitfalls-zcode-acp.md` 雷 12。
  **且 Windows 上必须带 `--ignore-scripts`**：包的 postinstall 是 Unix-only
  （`2>/dev/null || true`），裸装恒败并被 npm 回滚成 0 文件空壳（`npm ls -g` 版本号
  空、agent 起不来；2026-10-24 实锤）。安装/升级通道已按包名自动加该参数
  （`commands.rs::npm_install_args`，按钮与 auto-update 同通道），手工装也要带。
- **桌面版「在位」的判定 = `acp::zcode_bin_path()` 探测链**（固定根 + 开始菜单/桌面
  `.lnk`）。`~/.zcode/v2` 目录**不是**安装痕迹——跨用桥自己就写 provider_config.json
  进去，没装桌面版也会「已就绪」（`commands.rs::zcode_desktop_present` 2026-10-24 改）。
  桥没有本体跑不了第一条 prompt：ACP 只是翻译层，真正的 `zcode app-server` 来自
  桌面版安装里的 `resources\glm\zcode.cjs`。

## i18n（中英双语，2026-10-30 落地）

规范与术语表：`docs/i18n-guide.md` —— **改任何用户可见文案前先读它**。要点与红线：

- **前端**：`src/lib/i18n.ts`（模块级 locale store + `t(key, params)` + `useT()`/
  `useLocale()`，`useSyncExternalStore` 订阅）。词典在 `src/locales/`：`zh-CN.ts`/`en.ts`
  合并 `parts/*.{zh,en}.ts` 片段（en `satisfies Record<keyof typeof zh, string>` ——
  **en 缺键 tsc 直接红**）。偏好键 `marionette-language`（首启按 `navigator.language`
  自动检测），入口：Ctrl K「切换语言 / Switch language」+ 侧栏底栏 Languages 按钮。
  **useMemo/useCallback 里算字符串必须把 `locale` 加进依赖数组**，否则切语言不重算
  （参照 `CommandPalette.tsx` 的 groups memo）。
- **Rust**：`src-tauri/src/loc.rs` 的 `ts!` 宏就地双语（zh 字面量即 key，两个字面量
  占位符必须一致；const/static 上下文不能调 `ts!`，要么挪到使用点、要么 fn 化——见
  `shell_integration.rs::menu_label`、`memory.rs::import_title`）。语言真相在前端，
  启动时 `loc::init_from_disk()` 读 `~/.marionette/language` 镜像，之后由
  `set_ui_locale` 命令推送。**模型向文本（memory prompt、transcript_digest、
  grok_dialect、`<marionette-memory>` 抬头）保持中文，不翻**。
- **探测串一律双语并集**：`startsWith("正在重试") || startsWith("Retrying")`（App.tsx）、
  `/^(已挂起|Parked\.|…)/`（sessionHistory.ts）、`includes("注册失败") ||
  includes("registration failed")`（ModelLibraryDialog）——生成侧的语言取决于生成时刻
  的 locale，可能与当前 UI 不一致，新探测点照此办理。
- 回归：`npx tsx scripts/inventory-zh.mts --check`（前端白名单外中文清零；正则密集
  文件有已知误报）、`npx tsc --noEmit`、`cargo test`（默认 zh，旧断言原样过）。
