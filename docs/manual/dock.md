# 贴边停靠（edge dock）开发手册

一句话：窗口贴屏幕左右缘收成 6px 淡轨窄条、mouseenter 弹出的整套停靠机制——
几何全物理像素、绝不 hide 窗口、拖放/文字拖入/浮层 z-index 全有专门纪律。

## 文件地图（改 X 找 Y）

| 想改什么 | 去哪 | 备注 |
|---|---|---|
| 状态机 + 窗口编排 + prefs | `src/lib/dockMode.ts` | `initDockMode`/`enterDock`/`undock`/`measureInsets`/`suppressAutoDockUntil` |
| 窄条顶部铬 | `src/components/DockChrome.tsx` | 标题栏/菜单（`DockMenuLayer` portal）/额度卡/进行中列表/`DockResizeGrip` |
| 底栏 portal | `src/lib/dockFooter.ts`（`useDocked`/`dockFooterHost`）+ `#dock-footer` | 两侧栏 footer 子树 portal 到 body |
| 收纳盒页面 | `src/components/MemoBox.tsx` | 见 [memo-box.md](memo-box.md) |
| dock 态样式变体 | `src/styles/dock.css` | `.dock-mode`：单列网格、侧栏转覆盖层、标签行隐藏 |
| 拖放路由（窄条分区） | `src/components/DockDropRouter.tsx` + `src/lib/dockDrop.ts` | 上半条→收纳盒、下半条→composer 附件 |
| 文字/链接拖入 | `src/lib/dragText.ts`（`onDragText`）+ `src-tauri/src/drag_text.rs` | vendor wry 补丁读 CF_UNICODETEXT |
| z-index 总表 | `src/styles/base-chrome.css` 头部 | 对话框 9950–9970 一段 |
| Rust 侧 | `src-tauri/src/dock_support.rs` | 只有 set_background_mode/quit_app/fullscreen_active |
| 能力命名 | capabilities | `core:window:allow-set-skip-taskbar`（不是 allow-skip-taskbar）；`monitors_work_areas` 命令**不存在** |

## 红线（绝不能做）

- **停靠期间绝不 `hide()` / `destroy()` / `setFocus()`**（tao paint assert 红线延伸）。
  窗口只是停在屏外留 6 逻辑 px；聚焦只来自用户点击或单实例交接。
- **几何全物理像素、按客户区定位**：tao 给无边框窗留 ~8px 不可见边框
  （`calculate_insets_for_dpi` 左/右/下），`outerPosition`/`setPosition` 是外框坐标——
  按外框停靠整个客户区落屏外。`dockMode.ts::measureInsets()` 量 inner/outer 差值。
- **但 `setSize` 是 inner size**（tao `set_inner_size`）：给尺寸加 insets 会让停靠窗
  宽出 16px、右缘戳出屏。`clientSize()` 只返回 `width × workArea.height`；浮动恢复用
  `innerSize()` 别用 `outerSize()`。
- **窄条标题栏禁用 `data-tauri-drag-region`**（双击会 toggle_maximize——400px 窄条被
  最大化）。`DockChrome` 自己在 `.dock-bar` 上 `startDragging()`（跳过 button/input）
  并 `stopPropagation()` 挡注入脚本。
- **贴边浮层必须逃出 `.dock-chrome` 的层叠上下文**：`.dock-chrome` 是
  `position:fixed; z-index:40` = 独立层叠上下文，里面写多高都出不了 40。新浮层照
  `DockMenuLayer` 形状 `createPortal` 到 `<body>`、rect 现算、`z-index:10040` 一段；
  **别把 z-index 往 .dock-chrome 里面加**。portal 后外部点击守卫要算上面板 ref。
- **吸附/拖出的时间窗与标志**：`setPosition`/滑入滑出自己触发的 onMoved 用
  `suppressAutoDockUntil` 屏蔽；窄条拖出**不用时间窗**，`DockChrome` 在
  `startDragging()` 前调 `beginDockBarDrag()` 置 positive 标志，只有该标志在才判
  `considerDockDockOut`（否则 expand/retract 半程被当成拖出）。宽度拖拽
  `applyDockWidth` 先同步改 `dockWidthPhys` 再 promise 链串行 setSize+setPosition；
  换算相对活客户区（右贴 `新宽=当前宽−clientX`、左贴 `新宽=clientX`）。
- **TrySuspend 成对**：贴边窄条**永不触发挂起**（窗口停在屏外是 visible，DOM
  mouseenter 靠它活着）。所有 hide/show 路径纪律见 [memory-process.md](memory-process.md)。
- **prefs 的 `mode:"dock"` 是活窗态，不跨启动**——启动时 normalize 回 "off"。
- **能力**：`monitors_work_areas` 命令不存在——显示器信息全走 JS API
  （`availableMonitors`/`currentMonitor`，Tauri 2.8 `Monitor.workArea` 是物理的；
  混合 DPI 下 Logical 会错）。
- **OLE 拖拽期间鼠标事件被抑制**：sliver 的 mouseenter 不触发——另听
  `onDragDropEvent`（enter/over → expand；leave → 复位 + scheduleRetract，否则指针
  标志残留永不收回）。
- **`enter` 无 paths 不武装**：tao 对页内 HTML5 拖拽也发 enter/over，那些必须留给
  纯 HTML5 链路（memo→composer、页内拖拽源标记 `installPageDragSourceTracker`）。

## 坑

- 弹出/收回全是 `setPosition` 9 步 ~160ms 滑动；dock 弹出信号是 DOM mouseenter。
- 退出路径与贴边解耦：× 行为由 Ctrl K 的 `autoHideOnClose` 定（Rust AtomicBool 镜像，
  默认 false=直接退出）；「后台常驻」= Rust 在 CloseRequested 里直接 hide 主窗，
  **前端不参与关闭路径**。关闭看门狗 2s；前端 `handleCloseFromRust` 一进来就
  `ack_main_close`（别等 retract 动画）。
- 底栏 portal 的 popover 必须一起 portal（`DockedLayer`），否则 `translateX(100%)`
  的 rail 会连 `position:fixed` 浮层一起藏掉。
- composer：贴边默认内容撑高（textarea `field-sizing:content`）；顶边把手拖拽钉住
  高度（`.is-dock-fixed`，独立 key `marionette-dock-composer-height`）；**没实际移动的
  mouseup 不落值**（别把自动态一碰就冻住）；双击把手恢复自动。
- `DockChrome` 的 `openMenu: "more"|"new"|null`；menu 状态新增 `"hub"`（贴边 composer
  单入口）；`.composer.is-compact` 下芯片 `display:none` 但**锚点必须留在流里**
  （菜单挂锚点内，夹边 effect 靠锚点几何）。
- 普通窗口 / 分屏过窄也走同一套 hub：composer 宽度 ≤580px 加 `is-compact`、≥640 卸，
  阈值带 60px 回差。
- 状态过滤（`ProjectShelf`）在 dock 态同样适用；进行中列表的完成行**一直留到真正
  挂起**（exited 才消失）。

## 验证

```bash
npm run build && npx tsc --noEmit
npx tsx scripts/verify-memo-box.mts   # 拖放路由相关
```
真机（docs/CURRENT.md）：吸附/拖出/宽度拖拽/浮层 z-index/拖文件到把手展开/
文字拖入/关闭路径三态/最小化白屏。

## 已知差距与源流

- 设计稿与偏差记录：`docs/plans/plan-edge-dock.md`；行为描述 `docs/CURRENT.md`。
- wry/tao 补丁（文字拖放、paint assert、TrySuspend）：`vendor/*/MARIONETTE-PATCH.md`，
  升级 wry/tauri 前逐条核对能否撤掉。
- 蒸馏自：AGENTS.md「贴边停靠 edge dock」全段（2026-10-24 落地 + 10-25/10-26 反馈批）。
