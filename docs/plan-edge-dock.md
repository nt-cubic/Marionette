# 贴边停靠长条（edge dock）— 设计稿（未实现）

> 2026-10-23 定稿，依据 [`research-edge-dock.md`](./research-edge-dock.md) 的调研结论。
> 五阶段实施，每阶段做完可验收再做下一阶段。
> 状态：**阶段 0–3 已落地**（2026-10-24，commits cef3afd / 5b9978e / 3212135 / 0d16a5b + 打磨批）；
> 阶段 4 剩：上边缘横向变体、拖标题吸附、收回曲线微调、sliver 轮询兜底（本节内原样保留）。
> 2026-10-25 增补六件（用户反馈批次）：① 收起态拖文件到把手也展开（OLE 拖拽抑制鼠标事件，
> 改听 tao onDragDropEvent，enter/over=expand、leave=重排收回）；② ⋯ 菜单「侧边栏常驻」
> （`prefs.pinned`，抑制自动收回，手动路径不变）；③ 新建对话改为目的地菜单（聊天/各项目/全部对话）；
> ④ 贴边 composer 工具栏收成单入口（模式/权限/模型/Agent 分组菜单再进原列表，＋与发送保留）；
> ⑤ 窄条内缘可拖宽 320–560（`applyDockWidth` 串行 setSize+setPosition，clientX 相对活客户区换算，
> 松手 `persistDockWidth`）；⑥ 进行中列表：~~waiting 且安静 60s（`DOCK_DONE_GRACE_MS`）自动退场~~
> （2026-10-26 用户反馈改为：完成（waiting）的对话不退场，一直留到真正挂起——2 小时自动挂起 /
> 行内 ⏸ / 进程退出把状态翻成 `exited` 才消失；`DOCK_DONE_GRACE_MS` 已删）。2026-10-26 又增：
> 拖文件进窄条分区接住（`DockDropRouter`：上半条 → 收纳盒、下半条对话区 → composer 附件，
> 拖动中双层虚线提示；dock 态 composer 的原生落点处理关闭，路径经 `lib/dockDrop.ts` 事件转交）。
> 2026-10-26 收纳盒梳理：memo 页开着时页面监听**整窗**接住拖放（不做 rect 命中——旧实现拖到
> 页面外无声失败，memos 目录从未落过盘），`DockDropRouter` 对 memo 页整体让路防双保存；§4.1 的
> 「开新对话」改为在侧栏「收纳盒」分组（`origin:"memo"` 的 Chat 会话）里建出，10-22 方案的
> inbox 幽灵项目在加载口过滤（`lib/sessionGroups.ts`）。
> 2026-10-26 呈现：收纳盒从 40px 文件行改成 Karakeep 手机 Home 式单列帖子流（文字铺正文、
> 图片按横竖比完整显示、链接域名+网址、文件扁卡；「今天 / 昨天 / N月N日」分段），顶上加「写一条」
> 输入卡。设计稿 ③ 的「一行一条」作废。
> 实施偏差两处：`monitors_work_areas` 未做——Tauri 2.8 `Monitor.workArea` 已带工作区，全走 JS API；
> capability 实名是 `core:window:allow-set-skip-taskbar`（计划里的 `allow-skip-taskbar` 不存在）。
> 行为描述已并入 `CURRENT.md`「贴边停靠」行；全部阶段完成后本文件挪入 `archive/`。
>
> **与悬浮球计划的关系**：悬浮球计划（[`archive/plan-orb-assistant.md`](./archive/plan-orb-assistant.md)）
> 已于 2026-10-23 废弃，本计划是其「后台常驻」方向的继任者，可复用部分已全部并入（见 §0 末清单）；
> 球体窗、跨窗事件桥、场景按钮（原档④）、球体角标（原档②）随球废弃。

## 0. 背景与目标

Marionette 目前是「打开窗口才存在」的 1280×820 大窗。目标形态对标 QQ 贴边：

1. 窗口可**并入屏幕左右边缘**（上边缘为阶段 4 变体），平时收起，**鼠标触碰边缘即拉出**；
2. 拉出的是一条**竖长条**（默认宽 400，可调 320-480），内容自上而下：
   新建对话 → memo 收纳盒 → 工程对话列表 → 余额 → 之前开着的对话与输入框 → 底部设置/升级/退出；
3. **关闭主窗 = 贴边收起**（引擎、全部会话、Telegram 桥继续运行），真正退出只从 footer 菜单进；
4. 解除停靠（拖离边缘 / 菜单开关）即恢复普通浮窗，窗口尺寸与停靠前一致。

**从悬浮球计划（已废弃）吸收的部分**：

- 阶段 0 全部：关闭=后台常驻、`quit_app`、`set_background_mode`、capabilities 增补（原档①）；
- 收纳盒 v2 的 `capture_save_blob`（blob 落盘拿回绝对路径，原档③）；
- 余额列表数据层 `engine_quota` 直连（原档⑤，本计划做成窄条余额 chip）；
- 「收集箱会话」AI 整理方案（原 §2 案乙）作为收纳盒增值层；
- 隐藏态 WebView2 定时器节流风险（~1s 粒度，事件仍达）——阶段 0 的纯 `hide()` 态适用，
  阶段 1 起贴边态窗口常显不受影响；
- 收起态任务完成通知：直接用现有桌面通知（`marionette-desktop-notify` + 通知音），替代原球体角标。

**随球废弃**：球窗（OrbApp/orbWindow）、跨窗事件桥（orb-state/orb-command）、场景按钮（原档④）。

## 1. 总体概念：主窗的「停靠模式」，不是第二个窗口

停靠 = **主窗 SPA 变窄变高的模式切换**。理由（详见调研 §5）：全部状态（会话/标签/分屏/输入框
草稿/进行中的 turn）原地保留，「之前开着的对话和输入框」天然满足，零跨窗同步；
独立 strip 窗方案要双份 UI + 事件桥（已废弃的悬浮球计划即属此路）。

**三态状态机**（编排代码集中在新的 `src/lib/dockMode.ts`，仿 `detachedWindow.ts` 模式）：

```
floating ──(停靠指令/关闭按钮)──► docked-hidden ──(mouseenter on sliver)──► docked-expanded
   ▲                               ▲                                        │
   └──(拖离边缘/菜单解除)───────────┘◄──(mouseleave 300ms + Focused(false))────┘
```

| 状态 | 窗口参数 |
|---|---|
| `floating` | 现状默认：`always_on_top=false`、`skip_taskbar=false`、min 960×640 |
| `docked-hidden` | 条尺寸（宽 W × 工作区高），整体移出屏外**只留 4px** 触发条；`always_on_top=true`、`skip_taskbar=true`、`resizable=false`；不聚焦 |
| `docked-expanded` | 滑入工作区内贴边；其余同上；**不 `set_focus`**（点进去才聚焦，QQ 同款） |

关键机制（调研 §3 路线 A，**零全局钩子、零轮询**）：

- 那 4px 是真实可见的 WebView 客户区，DOM `mouseenter` 即弹出信号，`mouseleave`+失焦即收回；
- 弹出动画：8-10 步 `set_position` 等步长滑入，~160ms（整窗重绘，不做更长）；
- 弹出前查一次全屏抑制（见 §2 `fullscreen_active`），D3D 全屏/演示模式不弹；
- 隐藏态窗口仍 visible，**不受** WebView2 隐藏节流（悬浮球计划风险表那条仅适用于纯 `hide()` 态，见 §7）；
- 全程只 hide/show/set_position/set_size，**绝不 destroy webview**（tao paint assert 红线）。

## 2. 命令与权限契约

**新增 Rust 命令**（`src-tauri/src/dock_support.rs`，薄封装）：

| 命令 | 作用 |
|---|---|
| `quit_app` | 调 main.rs 现有 `shutdown_and_exit`（唯一合法退出路径；源自悬浮球计划档①） |
| `set_background_mode(bool)` | Rust 侧 AtomicBool，决定 CloseRequested 走「贴边收起」还是退出（源自悬浮球计划档①） |
| `monitors_work_areas()` | 全显示器 `{id, 位置, 尺寸, 工作区, scale}`：`EnumDisplayMonitors` + `GetMonitorInfoW`（加 `windows-sys` feature `Win32_Graphics_Gdi`）；停靠定位/弹出动画只用工作区，不压任务栏 |
| `fullscreen_active()` | `SHQueryUserNotificationState`（feature `Win32_UI_Shell` 已有）→ D3D 全屏/演示模式返回 true |
| `memo_list` / `memo_save_paths` / `memo_save_text` / `memo_delete` | 收纳盒存储（§4，阶段 3，`src-tauri/src/memos.rs`） |

窗口操作（setPosition/setSize/setAlwaysOnTop/setSkipTaskbar/setMinSize/show/hide/currentMonitor/
scaleFactor）全部走前端 JS API——`detachedWindow.ts` 先例，Rust 不代理。

**capabilities 增补**（`src-tauri/capabilities/default.json`，实施时以实际缺失核对）：
`core:window:allow-set-size`、`allow-set-min-size`、`allow-skip-taskbar`、
`allow-current-monitor`、`allow-available-monitors`、`allow-scale-factor`。
（现文件已含 set-position / set-always-on-top / show / hide / set-focus / cursor-position /
create-webview-window / outer-position / outer-size / start-dragging。）

**localStorage**：`marionette-dock-prefs`
`{mode:"off"|"dock", edge:"left"|"right", monitorId?, width:400, autoHideOnClose:true}`。

**v1 边界（明确不做）**：全局鼠标钩子、Windows AppBar API、上边缘、拖到边缘自动吸附（阶段 4 再议）、
独立停靠窗。依据见调研 §3。

## 3. 长条 UI 契约（阶段 2）

- 根节点挂 `.dock-mode` class（**CSS 变体，不用 `@media`**——仓库现状零媒体查询）；
  `tokens.css` 的 `min-width: 960` 在 `.dock-mode` 下放开；停靠时运行时 `setMinSize` 调小，
  解除停靠恢复。
- 布局自上而下（窄条约 400px）：
  1. **操作行**：新建对话（复用 `handleCreateChatSession`）＋ 收纳盒按钮（角标计数，阶段 3 填充）；
  2. **余额 chip**：单行 `engineQuotaRows` 摘要（引擎配额 + zhipu coding-plan 等逐 provider），
     复用 `usage.ts` 格式化与 `.usage-row/.usage-meter` 样式，点击手动刷新；
  3. **工程对话列表**：ProjectShelf 紧凑模式 → 抽屉/覆盖层（已有折叠态 `marionette-project-shelf-collapsed` 可借力）；
  4. **SessionTabs 横向滚动** + SessionView/Composer 原样进窄列；**分屏在 dock 态禁用**（提示先解除停靠）；
  5. **右栏 ContextPanel**（Usage/changed files/todos）收进覆盖层，从操作行图标唤出；
  6. **footer**：现有五按钮（主题/通知/代理/引擎/TG，`ProjectShelf.tsx:1457-1469`）＋
     **升级**（接 `app_update.rs` 检查/下载/重启，复用现有更新横幅逻辑 `App.tsx:7876-7935`）＋
     **解除贴边** ＋ **退出**（`quit_app`）。
- docked-hidden 时那 4px 渲染一条高亮指示边（`.dock-mode.edge-hidden` 变体）。

## 4. memo 收纳盒契约（阶段 3）

**存储（调研 §7 案甲）**：`~/.marionette/memos/`——

- `index.json`：`[{id, kind:"file"|"text"|"image"|"link", title, path?, content?, size?, createdAt, source?}]`
  （仿 `chats/sessions/index.json` 模式，`storage.rs:1039-1081`）；
- `files/`：拖入文件**复制**进来（内容 hash 去重；只存引用会断链，默认复制）。

**拖入三通道**：

1. **文件/文件夹**（资源管理器拖入）：原生 `onDragDropEvent` 给绝对路径（`Composer.tsx:1874-1926`
   同款，drop 区域判定落在收纳盒热区）→ `memo_save_paths` 复制落盘；
2. **文字/链接**：v1 走 Ctrl+V 粘贴 + 会话内「存入收纳盒」按钮（原生拖放只给文件路径，
   收文字需整窗 `dragDropEnabled:false`，与 Composer 文件拖放**互斥**，v2 再统一切 HTML5 DnD——
   届时启用 `capture_save_blob`（源自悬浮球计划档③）统一 blob 落盘）；
3. **页内拖出**（收纳盒 → Composer 附件）：页内 HTML5 DnD 不经 tao 的跨进程 OLE 拦截，天然可用。

**UI**：操作行收纳盒按钮 → 覆盖层列表（搜索 / 预览 / 拖出 / 删除），拖入悬停高亮。
**与「收集箱会话」的关系**（源自悬浮球计划 §2，保留为增值层）：本收纳盒为确定性纯收纳（离线可用）；
「逐条发进置顶收集箱会话 + AI 自动整理」作为其增值层，后续把本盒条目批量喂入，互不替代。

### 4.1 收纳盒的长期方向：轻量版 Karakeep（2026-10-24 补，设计稿 `docs/design/edge-dock.html` ③–⑥）

参照 [karakeep](https://github.com/karakeep-app/karakeep)（自托管「收藏一切」：链接 / 笔记 / 图片 / PDF，
AI 打标签与摘要，全文搜索，列表，批量操作），但只取「收、找、用」三件事，贴合本软件：

- **收**：文件 / 文字 / 图片 / 链接四类；链接自动抓标题与描述（本地抓取，失败就留网址）。
- **找**：按类型筛选（全部 / 链接 / 文字 / 图片 / 文件）＋ 标题、内容、标签搜索；标签由「模型共享」里
  最便宜的模型自动打，可改可删，引擎没开时就不打，不阻塞收纳。
- **用**（本软件独有的两条出口）：
  1. **一键整理成文档**：选中条目（或整盒）交给模型按主题归并、去重、列待办，先出可改的草稿，
     确认后存进当前项目 `docs/notes/<标题>.md`；原条目保留，可选「整理后归档」。
  2. **用选中的开新对话**：新开对话标签，条目作为输入框附件（文件给路径、图片给图、链接给标题与网址），
     用户补一句话再发——复用 Composer 现有附件通道，不另造发送路径。
- **多选**：点「选择」或直接点行进入；底部浮出选择栏（已选数 · 整理成文档 · 开新对话 · 退出），Esc 退出。
- **明确不做**：网页全文存档、OCR、浏览器插件、RSS、多人协作、服务端——这些是 Karakeep 的重部分。

## 5. 分阶段实施

### 阶段 0 · 地基（后台常驻；源自悬浮球计划档①）

- `main.rs` CloseRequested 分支改写：`background_mode` 开 → 主窗贴边收起（阶段 1 前先 `hide()`），
  引擎/会话/TG 桥继续；关 → 现行 `shutdown_and_exit`。detached 窗关闭逻辑同步（主窗收起时不再因
  「无可见窗口」触发退出）；`Destroyed` 兜底保留。
- `quit_app` / `set_background_mode` 命令；capabilities 增补。
- ProjectShelf 设置行加「关闭时收起到边缘」开关（默认开）。
- **验收**：关主窗进程存活（引擎/TG 正常）；footer 退出能真退；单实例交接（第二实例启动）对
  收起的主窗仍有效；`npm run build` + `cargo check` 过。

### 阶段 1 · 停靠引擎

- `src/lib/dockMode.ts`：状态机 + 窗口编排（enter/exit/expand/retract/undock），LogicalPosition +
  `scaleFactor` 换算照抄 `detachedWindow.ts:178-203`；动画 8-10 步 ~160ms；收回双条件
  （`mouseleave` 300ms 延迟 + `Focused(false)`，延迟兼容从长条拖东西出去的路径）。
- `dock_support.rs`：`monitors_work_areas` + `fullscreen_active`（`Cargo.toml` 加
  `Win32_Graphics_Gdi` feature）。
- 停靠绑定「停靠指令发起时鼠标所在显示器」的边缘；任务栏同边不拦，默认建议对边（设置项文案说明）。
- 入口：footer「贴边收起」按钮；关闭按钮 = 贴边（阶段 0 的 background_mode 落成真形态）。
- **验收**：左右停靠、触碰 4px 滑出、离开+失焦收回、弹出时不抢前台焦点（在别处打字不被打断）、
  多显示器 + 150% DPI 正确、全屏视频/游戏时不弹、解除停靠恢复原尺寸与默认窗口参数。

### 阶段 2 · 窄条 UI

- `.dock-mode` 布局全套（§3）：ProjectShelf 抽屉、tabs 横向滚动、右栏覆盖层、余额 chip、
  footer 三新按钮（升级/解除贴边/退出）、分屏禁用提示、4px 指示边。
- **验收**：窄条内六项内容全部可用；新建对话/切会话/发消息/收附件正常；余额 chip 显示引擎配额与
  zhipu coding-plan；升级按钮走通检查→下载→重启；CJK 输入法在长条内输入正常（vendor/tao 补丁兜底，
  实测确认）。

### 阶段 3 · memo 收纳盒

- `memos.rs` 四命令 + `src/lib/memos.ts` + 收纳盒 UI（§4）。
- **验收**：资源管理器拖文件/多选/文件夹进收纳盒即收（复制+去重）；Ctrl+V 收文字；
  列表搜索/删除；从收纳盒拖文件进 Composer 成为附件；重启后条目仍在。

### 阶段 4 · 打磨

- 上边缘停靠（横向长条变体，宽=屏宽 × 高≈2/3，布局镜像）；
  拖标题到边缘吸附停靠（复用 `handleOpenBeside` 的 edge 判定思路 + Moved 事件）；
  任务栏同边检测与建议；收回动画曲线微调；（可选）sliver 被置顶窗盖住时的轮询兜底
  （`cursorPosition()` 100-150ms，调研 §3 路线 C）。
- **验收**：对应各项手测。

## 6. 涉及文件

**新增**：`src/lib/dockMode.ts`、`src/lib/memos.ts`、`src/components/MemoBox.tsx`（+ 收纳盒覆盖层）、
`src-tauri/src/dock_support.rs`、`src-tauri/src/memos.rs`、`.dock-mode` 样式段（app.css 或独立 dock.css）。

**修改**：`src/app/App.tsx`（dock-mode 布局分支、余额 chip、footer 挂点、关闭路径配合）、
`src/components/ProjectShelf.tsx`（紧凑/抽屉模式、设置开关、footer 按钮）、
`src/components/SessionView.tsx`（SessionTabs 横向滚动）、`src/components/Composer.tsx`
（收纳盒拖出目标热区）、`src-tauri/src/main.rs`（CloseRequested/后台模式/quit_app）、
`src-tauri/src/commands.rs`（命令注册）、`src-tauri/src/lib.rs`（模块注册，如有）、
`src-tauri/capabilities/default.json`、`src-tauri/Cargo.toml`（windows-sys feature）、
`src/styles/tokens.css` / `app.css`、`AGENTS.md`、`docs/CURRENT.md`。

## 7. 风险与对策

| 风险 | 对策 |
|---|---|
| tao 0.35 paint assert（`panic=abort` 红线） | 全程只 hide/show/set_position/set_size，**绝不 destroy**；退出唯一路径 `shutdown_and_exit`；新 Rust 代码零 panic/unwrap |
| App.tsx 体量（8054 行） | dock-mode 布局抽独立容器组件，不往现有 JSX 里堆分支 |
| min-width 双处限制（`tokens.css:83` + tauri.conf `minWidth:960`） | `.dock-mode` class 放开 CSS 侧；停靠时 `setMinSize`、解除时恢复，静态配置不动 |
| 弹出抢焦点打断用户输入 | 弹出只动位置不 `set_focus`；点击才聚焦（验收项） |
| CJK IME 在长条内输入 | vendor/tao 补丁已兜嵌套泵；阶段 2 验收实测 |
| 压住任务栏 / 与任务栏抢同边 | 一律用工作区定位；默认建议对边，设置项文案说明 |
| 原生拖放与文字 DnD 互斥 | v1 文字走粘贴；v2 统一 HTML5 时用 `capture_save_blob`（源自悬浮球计划档③）落盘 |
| 多显示器 / 混合 DPI 位置错 | 停靠绑定发起时鼠标所在显示器；全 LogicalPosition + scaleFactor 换算（detached 先例） |
| 全屏场景误弹 | 弹出前查 `fullscreen_active`，D3D/演示模式抑制 |
| 阶段 0 纯 `hide()` 态 WebView2 定时器节流（~1s 粒度） | 事件仍到达、批处理略慢可接受；阶段 1 起贴边态窗口常显不受影响（源自悬浮球计划风险表） |
| 单实例 / 交接路径回归 | 阶段 0 验收明确覆盖（第二实例对收起主窗的唤起） |

## 8. 验证

每阶段完成：`npm run build` + `cargo check` + `npm run tauri dev` 手测该阶段验收点
（§5 各阶段清单）。阶段 1 起额外过一遍：100% / 150% DPI、双显示器、任务栏在左/右/隐藏态、
全屏视频播放中触碰边缘（应不弹）。

## 参考

- [`research-edge-dock.md`](./research-edge-dock.md) — 本计划的技术选型依据（路线对比、代码盘点、外部链接全集）
- [`archive/plan-orb-assistant.md`](./archive/plan-orb-assistant.md) — 已废弃的悬浮球计划（本计划吸收其档①地基、`capture_save_blob`、余额数据层与收集箱方案）
- QQ 贴边参考实现见调研 §2/§10
