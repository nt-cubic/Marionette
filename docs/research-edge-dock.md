# 调研：QQ 式贴边停靠形态（edge dock）— 2026-10-23

> 结论先行：**可行，且比预想的顺**。所需的窗口权限大半已在 capabilities 放行，
> 「余额 / 会话恢复 / 新建对话 / 升级 / 文件拖放」五件事的数据层全部现成；
> 真正要新建的只有两块——**贴边停靠引擎**（窗口滑入滑出状态机）和 **memo 收纳盒**（存储 + 拖入交互）。
> 悬浮球计划（`plan-orb-assistant.md`，已废弃并入 `archive/`）的「后台常驻」方向由贴边长条继任，
> 其可复用部分（档①地基、`capture_save_blob`、余额数据层、收集箱方案）已并入实施计划 `plan-edge-dock.md`。

## 1. 需求还原

用户描述的产品形态（对标 QQ 贴边、uTools、Snipaste 贴边）：

- 窗口可**并入屏幕边缘**（上 / 左 / 右），平时收起，**鼠标触碰边缘即拉出**；
- 拉出的是**一整条竖长条**（≈360-440px 宽 × 全屏高），内容自上而下：
  1. 新建对话
  2. **memo 收纳盒**（拖拽文件/文字/图片进来直接收好）
  3. 左侧边栏的工程对话列表
  4. 剩余余额显示
  5. **之前开着的整个对话框和输入框**（会话延续）
  6. 底部：设置选项、升级按钮等

## 2. QQ 式贴边的标准行为拆解（业界通行做法）

| 行为 | 实现要点 | 出处 |
|---|---|---|
| 隐藏态 | 窗口保持完整尺寸，整体移出屏幕，只留 **1-6px** 触发条在边缘内 | cnblogs MFC 经典文、CSDN Electron 版 |
| 触发弹出 | 定时器轮询 `GetCursorPos` 或 `WH_MOUSE_LL` 全局钩子检测鼠标进入触发区 | 同上 + appinn 讨论 |
| 防误触 | 触发条只留 1-2px；可加 TrackMouseEvent / 短延迟 | appinn WinAutoHide 讨论 |
| 收回 | 鼠标移出窗口 + 窗口失焦双条件，带 200-400ms 延迟 | QQ / WinAutoHide |
| 解除停靠 | 把窗口拖离边缘 → 恢复普通浮窗 | QQ |
| 焦点 | 弹出不抢前台焦点，点进去才聚焦（保证不打断用户在别处的输入） | getquicker 讨论（保焦点是痛点） |

参考实现：[cnblogs 窗口边缘自动隐藏（MFC）](https://www.cnblogs.com/happykoukou/p/7346710.html)、
[CSDN Electron 仿 QQ 贴边（多显示器）](https://blog.csdn.net/qq_30392343/article/details/95800127)、
[WinAutoHide 源码](https://github.com/scavin/winautohide)、
[appinn 讨论：1px 边框 vs 系统钩子](https://meta.appinn.net/t/topic/53261)、
[Quicker：保住输入焦点的讨论](https://getquicker.net/QA/Question/2811)、
[PowerToys 同款诉求 issue](https://github.com/microsoft/PowerToys/issues/50549)。

## 3. 技术路线对比（Windows + Tauri 2.11）

### 路线 A（推荐 v1）：残留触发条（sliver）+ DOM 事件

隐藏态 = **完整尺寸窗口移出屏外只留 2-6px**；那几像素是真实可见的 WebView 客户区，
DOM `mouseenter` 即弹出信号，`mouseleave`+失焦即收回。

- **零全局钩子、零轮询**：鼠标事件由窗口系统天然送达；
- **不抢焦点**：弹出靠 `set_position` 滑入（SetWindowPos 默认不激活），点击才聚焦——正是 QQ 行为；
- 现有代码直接支撑：capabilities 已放行 `set-position` / `set-always-on-top` / `show` / `hide` /
  `set-focus` / `cursor-position`（`src-tauri/capabilities/default.json`）；
  `detachedWindow.ts:178-203` 已有 `cursorPosition()` + `scaleFactor()` 物理像素换算先例；
- 隐藏态窗口仍"可见"，不受 WebView2 隐藏节流影响（悬浮球计划风险表那条仅适用于纯 `hide()` 态）。

### 路线 B（备选增强）：WH_MOUSE_LL 全局鼠标钩子

适用于"窗口完全隐藏（`hide()`）也要能唤出"或 sliver 被其他置顶窗盖住的场景。
`windows-sys 0.59` 已启用的 `Win32_UI_WindowsAndMessaging` feature 就含 `SetWindowsHookExW`——零新依赖。
代价：需专用线程 + 消息泵；**回调必须极简且绝不 panic**（本仓库 `panic = "abort"`，任何线程 panic 杀全进程；
低级钩子超时还会被系统摘除）。

### 路线 C（最简兜底）：轮询 `cursor_position()`

Tauri 2.0 起自带 `app.cursor_position()`（[2.0 发布说明](https://v2.tauri.app/blog/tauri-20)），
前端 `cursorPosition()` 本仓库已在用。100-150ms 间隔轮询开销极小，人手感知不到延迟。
可与 A 混用：sliver 被挡住时降级轮询。

### 路线 D（不推荐）：Windows AppBar API（`SHAppBarMessage` / `ABM_SETAUTOHIDEBAR`）

官方"正规"停靠方案（会保留屏幕边缘空间），但[多显示器只作用于任务栏所在屏](https://learn.microsoft.com/en-us/windows/win32/shell/abm-setautohidebar)、
每边只允许一个 autohide appbar、还有[把自己挤出保留区的怪癖](https://stackoverflow.com/questions/80002486/shappbarmessage-my-own-window-gets-pushed-out-of-the-appbar-space-it-just-regis)
（官方样例：[AppBar.cpp](https://github.com/microsoft/Windows-classic-samples/blob/main/Samples/Win7Samples/winui/shell/legacysamples/appbar/AppBar.cpp)）。
QQ / uTools / 各类 dock 均不走此路；Marionette 长条是覆盖式面板（不挤占别的窗口），更不需要。

**结论：v1 走 A；B/C 留作增强；D 放弃。**

## 4. 现有架构盘点（file:line 基于 1b56561）

### 4.1 直接可复用的

| 需求 | 现状 | 位置 |
|---|---|---|
| 新建对话 | 按钮已有 | `ProjectShelf.tsx:1304-1310`，`handleCreateChatSession` |
| 会话恢复 | 冷启动恢复上次焦点会话 + 标签 + 分屏 | `src/lib/uiRestore.ts`（`marionette-ui-restore`） |
| 余额数据 | 引擎配额 30s 缓存 + 逐 provider 探测（zhipu coding-plan 等） | `commands.rs` `engine_quota`/`probe_provider_usage`、`provider_usage.rs:965`、前端 `usage.ts`、样式 `ContextPanel.tsx:525-580` |
| 升级 | 自研 GitHub 更新器 + 底部横幅 | `app_update.rs`（check/download/relaunch）、横幅 `App.tsx:7876-7935` |
| 文件拖放 | 原生 `onDragDropEvent` 收文件路径 + 浏览器 mock 双轨 | `Composer.tsx:1834-1926` |
| 窗口先例 | 前端创建/定位/聚焦/鼠标命中判断全套 | `src/lib/detachedWindow.ts:178-203, :380-401, :419-515` |
| 底部功能按钮 | 主题/通知/代理/引擎/TG 五按钮 footer | `ProjectShelf.tsx:1457-1469` |

### 4.2 必须动 / 必须新建的

1. **主窗配置**：`tauri.conf.json:13-25` 静态单窗 1280×820、`minWidth: 960`、无 `alwaysOnTop/skipTaskbar`。
   停靠需运行时 `setAlwaysOnTop/setSkipTaskbar/setMinSize/setSize/setPosition`（capabilities 缺
   `set-size`、`current-monitor`、`available-monitors`、`scale-factor` 四条——悬浮球计划档①已列同款增补）。
2. **关闭路径**：`main.rs:323-382` CloseRequested 永远 `prevent_close` → `shutdown_and_exit`
   （绕 tao paint assert 的既定路径）。贴边 = 主窗 **hide 化后台常驻**，绝不 destroy；
   真退出只走 footer 菜单 `quit_app` → `shutdown_and_exit`。与悬浮球计划档①是同一批改造（已并入贴边计划阶段 0）。
3. **停靠引擎**：全新（状态机 + 动画 + 边界处理，见 §6）。
4. **memo 收纳盒**：全新（存储 + 拖入交互，见 §7）。
5. **窄条布局**：`App.tsx`（8054 行）布局分支；`tokens.css:83` `min-width: 960px`；
   仓库 CSS 零 `@media`——建议用 `.dock-mode` class 变体而非媒体查询，合乎现状。
6. **全局钩子/裸 Win32**：目前零基础（`windows-sys` 仅注册表/FindWindowW/DPAPI 等有限用途）。
   work area 需加 `Win32_Graphics_Gdi`（`MonitorFromPoint`）feature；全屏抑制的
   `SHQueryUserNotificationState` 在已有的 `Win32_UI_Shell` feature 里。

## 5. 形态决策：改主窗 vs 独立停靠窗

- **方案一（推荐）：主窗"停靠模式"**。同一个 SPA 变窄变高。优点：全部状态（会话/标签/分屏/输入框
  草稿/进行中的 turn）原地保留——"之前开着的对话和输入框"**天然满足**，零跨窗同步。
  代价：App.tsx 增加一条 dock-mode 布局分支（建议抽独立容器组件，别再往 8000 行里堆）。
- **方案二：独立 strip 窗**（已废弃的悬浮球计划即属此路）。隔离好但要双份 UI + 事件桥（detached 先例证明可行但重），
  且"之前开着的会话"要靠桥接还原。

**结论：方案一。** 悬浮球计划（方案二路数）已废弃，其可复用部分由本方案吸收（见 `plan-edge-dock.md` §0）。

## 6. 停靠引擎设计（阶段 1 的核心交付）

### 状态机

```
floating ──(停靠指令)──► docked-hidden ──(mouseenter on sliver)──► docked-expanded
   ▲                        ▲                                   │
   └──(拖离边缘/菜单解除)────┤◄──(mouseleave 300ms + Focused(false))─┘
```

- **docked-hidden**：窗口完整尺寸，`x = 工作区右边 - 4px`（右停靠为例），
  `always_on_top + skip_taskbar`，不抢任务栏；页内检测到窗口处于隐藏坐标时渲染一条
  高亮指示边（那 4px 的内容）。
- **docked-expanded**：8-10 步 `set_position` 滑入（~160ms，等步长即可；窗口级动画每次整窗重绘，
  不要更长）。展开**不 set_focus**。
- **收回**：`mouseleave` 300ms 延迟 + `Focused(false)` 双条件（延迟兼容"拖出东西到别的窗口"的路径）。
- **停靠入口（v1 手动）**：footer「贴边隐藏」按钮 + **关闭按钮改为贴边**（QQ 惯例，引擎/TG 桥继续跑，
  正好兑现 CURRENT.md 里"要改成后台常驻"的方向）；v2 再做拖到边缘吸附（复用 `handleOpenBeside`
  的 edge 判定思路 + Moved 事件）。

### 边界与坑（Windows 特有）

| 问题 | 对策 |
|---|---|
| 不能压任务栏 | 用显示器**工作区**而非全屏：`MonitorFromPoint` + `GetMonitorInfo`（加 `Win32_Graphics_Gdi` feature）；v1 可先用 tauri monitor 尺寸 + 已知任务栏高度余量近似 |
| 多显示器 / 混合 DPI | 停靠绑定到「停靠时鼠标所在显示器」的边缘；全部 `LogicalPosition` + `scaleFactor` 换算（`detachedWindow.ts:178-203` 照抄） |
| 全屏游戏/演示模式误弹 | `SHQueryUserNotificationState`：`QUNS_RUNNING_D3D_FULL_SCREEN` / `QUNS_PRESENTATION_MODE` 时抑制自动弹出（feature 已有） |
| 任务栏同边贴边冲突 | 两个 topmost 抢同一条边；v1 检测任务栏边（`ABM_GETSTATE` 或注册表），默认建议对边，文档说明 |
| 焦点礼仪 | 弹出不激活、点击才聚焦打字；CJK IME 的嵌套泵已有 vendor/tao 补丁兜底，无新增风险 |
| panic 红线 | 新 Rust 代码零 panic、零 unwrap；全程 hide/show/set_position，**绝不 destroy webview**；唯一退出路径 `shutdown_and_exit` |
| 单实例交接 | `shell_integration.rs` 的 FindWindowW + SW_RESTORE 对隐藏主窗依然有效（悬浮球计划档①已论证），不动 |

## 7. memo 收纳盒设计（阶段 3 的核心交付）

### 拖入（三条通道，覆盖全部来源）

1. **文件/文件夹**（资源管理器拖入）：原生 `onDragDropEvent` 给绝对路径（`Composer.tsx:1874-1926`
   同款），复制进 `~/.marionette/memos/files/`（去重按 hash），index 记元数据——**不复制则只存引用，
   文件被移走就断链，默认复制**。
2. **文字/链接**（浏览器选区拖入）：Tauri 原生拖放只给文件路径，收文字必须
   `dragDropEnabled: false` + HTML5 DnD（[官方说明](https://v2.tauri.app/reference/javascript/api/namespacewebview)、
   [issue #14373](https://github.com/tauri-apps/tauri/issues/14373)、
   [react-dropzone 指南](https://react-dropzone.js.org/guide/tauri)）。
   **这是 per-window 开关，与主窗原生文件拖放互斥**。两条出路：
   - a. 整窗切 `dragDropEnabled:false`，文件也走 HTML5（blob → `capture_save_blob` 落盘拿回路径）——
     `capture_save_blob`（悬浮球计划档③的设计，随其并入贴边计划）即为此准备，Composer 已有浏览器 mock 的 HTML5 双轨代码可启用；
   - b. v1 先不切：收纳盒收文件（原生）+ 收文字（Ctrl+V 粘贴 + 页内「存入收纳盒」按钮），拖文字进 v2 再上。
3. **页内拖出**（收纳盒 → Composer 附件）：页内 HTML5 DnD 不经过 tao 的跨进程 OLE 拦截，天然可用。

### 存储（两案并列，实施前二选一）

- **案甲（独立存储，直觉简单）**：`~/.marionette/memos/index.json` + `memos/files/`，
  仿 `chats/sessions/index.json`（`storage.rs:1039-1081`）模式；字段
  `{id, kind: file|text|image|link, title, path/content, size, createdAt, source}`。
  Rust 侧 `memo_save_paths` / `memo_save_text` / `memo_list` / `memo_delete` 四个薄命令。
- **案乙（收集箱会话方案，零新存储，源自悬浮球计划 §2）**：收纳即"逐条发进置顶的收集箱会话"，
  transcript 即时间线，`search_sessions` 天然可检索，还能让 AI 自动整理（kb/todo 归档）。
  代价：每条捕获过一遍 agent 提示词路径。

倾向：**先案甲做纯收纳（确定性、离线可用），把案乙当它的"AI 整理"增值层**——两者不冲突，甲的条目可后续批量喂给乙。

## 8. 长条布局草图（宽 ≈ 400px，左右停靠）

```
┌──────────────────────────┐
│ ＋ 新建对话   [收纳盒 n]  │  ← 收纳盒点开为覆盖层列表（搜索/预览/拖出）
│ 💰 余额: glm ▓▓▓░ 62% ⟳ │  ← engineQuotaRows 单行 chip，点击刷新
├──────────────────────────┤
│ ▤ 工程对话列表（抽屉）     │  ← ProjectShelf 紧凑模式，按钮唤出覆盖层
├──────────────────────────┤
│ [tab1][tab2][tab3] →     │  ← SessionTabs 横向滚动；分屏 dock 态禁用
│                          │
│   transcript（对话流）    │
│                          │
│ ┌──────────────────────┐ │
│ │ 输入框 Composer       │ │
│ └──────────────────────┘ │
├──────────────────────────┤
│ ⚙ 设置 ⬆ 升级 ⏏ 退出     │  ← 现有五按钮 + 新增；升级接 app_update 横幅逻辑
│   (主题·通知·代理·引擎·TG) │
└──────────────────────────┘
```

- 右栏 ContextPanel（Usage/changed files/todos）在 dock 态收进「抽屉/覆盖层」，从顶部图标唤出。
- **上边缘停靠**：横向长条变体（宽=屏宽 × 高≈2/3），布局要整套镜像——建议 v1 只做左右边缘，上边缘放 v2/阶段 4。

## 9. 分期落地建议（天数 = 串行粗估，dev 验证口径：`npm run build` + `cargo check` + 手测）

| 阶段 | 内容 | 粗估 | 来源（悬浮球计划吸收） |
|---|---|---|---|
| 0 地基 | CloseRequested→hide、`quit_app`、`set_background_mode`、capabilities 增补 4 条 | 1-2 天 | = 悬浮球档①（已吸收） |
| 1 停靠引擎 | sliver/弹出/收回状态机、动画、边缘选择设置、多显示器 + 工作区、全屏抑制 | 3-5 天 | 新增 |
| 2 窄条 UI | `.dock-mode` 布局（ProjectShelf 抽屉/tabs 滚动/右栏收抽屉）、余额 chip、footer 三新按钮 | 4-6 天 | 余额复用悬浮球档⑤的数据层 |
| 3 收纳盒 | 存储 + 拖入三通道 + 列表 UI + 拖出到 Composer | 3-5 天 | `capture_save_blob` 源自悬浮球档③ |
| 4 打磨 | 上边缘变体、拖拽吸附停靠、收回曲线、任务栏同边提示、轮询兜底 | 2-3 天 | — |

## 10. 主要参考

- QQ 贴边实现：[cnblogs（MFC）](https://www.cnblogs.com/happykoukou/p/7346710.html) ·
  [CSDN（Electron 多屏）](https://blog.csdn.net/qq_30392343/article/details/95800127) ·
  [WinAutoHide](https://github.com/scavin/winautohide) ·
  [appinn 讨论](https://meta.appinn.net/t/topic/53261) ·
  [Quicker 保焦点讨论](https://getquicker.net/QA/Question/2811)
- Tauri 2：[窗口定制](https://v2.tauri.app/learn/window-customization) ·
  [Window JS API](https://v2.tauri.app/reference/javascript/api/namespacewindow) ·
  [2.0 `cursor_position()`](https://v2.tauri.app/blog/tauri-20) ·
  [dragDropEnabled 说明](https://v2.tauri.app/reference/javascript/api/namespacewebview) ·
  [issue #14373](https://github.com/tauri-apps/tauri/issues/14373) ·
  [#8532 RegisterDragDrop 冲突](https://github.com/tauri-apps/tauri/issues/8532) ·
  [react-dropzone Tauri 指南](https://react-dropzone.js.org/guide/tauri) ·
  [Ellie's Notes](https://ellie.wtf/notes/drag-event-issues-in-tauri)
- Win32 AppBar（弃用依据）：[SHAppBarMessage](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shappbarmessage) ·
  [ABM_SETAUTOHIDEBAR 多屏限制](https://learn.microsoft.com/en-us/windows/win32/shell/abm-setautohidebar) ·
  [SO 80002486](https://stackoverflow.com/questions/80002486/shappbarmessage-my-own-window-gets-pushed-out-of-the-appbar-space-it-just-regis) ·
  [官方 AppBar 样例](https://github.com/microsoft/Windows-classic-samples/blob/main/Samples/Win7Samples/winui/shell/legacysamples/appbar/AppBar.cpp)
- 需求同类：[PowerToys issue #50549](https://github.com/microsoft/PowerToys/issues/50549)
