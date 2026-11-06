# 浏览器共享 + Computer Use 实施案（设计稿，2026-10-31 定稿；批次 2 已落地，见文末偏差记录）

- 来源调研：`docs/research-codeg.md`（D5/D6 + 2026-10-31 复核节，全部 file:line 已对
  v0.41.3 核实）；参考实现在 `D:\Projects\codeg`（Apache-2.0）。
- 用户定调：**分批交付、每批完整实现**（不是 MVP 裁剪）；体积/内存零默认开销
  （浏览器 tab 才起 WebView2 renderer，CU 首次使用才下载 driver）。
- 批次：**批次 2 = 浏览器共享**（双方向全量）；**批次 3 = Computer Use**（Windows 全量）。
  每批独立可用、独立发版。

## 0. 共同骨架（两批共用，先建）

### 0.1 localhost HTTP MCP 服务（工具的唯一出口）

- **位置**：主进程内嵌，`src-tauri/src/tools_mcp.rs` 新模块。`tiny_http`（Cargo 已有，
  `grok_dialect.rs:177` 的 start_server 形状：固定偏好端口 + 随机回退）。
  路径 `/mcp`，**每 ACP 会话独立 token**：`mcp_payload_for_agent`
  （`context_inventory.rs:1096`）注入合成条目
  `{name:"marionette-tools", url:"http://127.0.0.1:<port>/mcp", headers:{Authorization:
  "Bearer <session-token>"}, type:"http"}`——token 编 sessionId，服务端由此知道调用
  来自哪个会话（授权模式判定、权限卡归属都要用）。
- **为什么 url 型是唯一正解**：zcode 的 session/new 只收 url 型（stdio 直接 -32602，
  `zcode_mcp_bridge.rs:1-19`）；其余 agent 全收 http 型。一条通道全 agent 通吃，
  且进程生命周期在我们手里（Codeg 的 stdio 伴生进程方案在 zcode 上直接不可用）。
- **协议**：手写 JSON-RPC（initialize / tools/list / tools/call 三方法足够；
  streamable HTTP 单 POST 一应答即可，agent 侧 MCP client 都兼容）。token 校验失败
  401；**所有工具的拒绝都是返回值不是协议错误**（Codeg 姿势，`browser_tools.rs:16-19`）。
- 注册时机：`acp.rs:1305` session_new_params 组装处（现有 mcp_servers 向量 append
  合成条目）；总开关落 prefs（默认 on，卸载即从 payload 消失）。

### 0.2 对话内权限卡（授权与对话模式吻合——用户定调）

- Control 级请求（浏览器点击/输入、CU 全部操作）不弹独立对话框，走**对话内权限卡**：
  MCP 工具调用进服务端 → 不满足授权 → `emit` Tauri 事件
  `marionette-tool-permission`（含 sessionId / 请求摘要 / origin 或窗口标题）→ 前端在
  该会话转录里插一张权限卡（复用 PermissionCard 视觉）→ 用户答 → 服务端 resolve
  pending 调用。**120s 无应答默认拒绝**（Codeg eval confirm 语义）。
- **会话模式吻合**：该会话开着 always-approve → 服务端直接放行，不出卡（与
  「always-approve = 别问我」的现有语义一致；它本就自动放行 bash，比点击更危险）。
  会话模式快照由前端 `set_ui_locale` 同款通道推给 Rust（或 Rust 侧查会话状态表）。
- **例外**：用户自开的浏览器标签**永远不适用自动放行**（隐私边界，见 1.3）。

## 1. 批次 2：浏览器共享（完整双方向）

### 1.1 child webview 宿主（`src-tauri/src/browser_surface.rs` 新模块）

- wry 原生 `WebViewBuilder::build_as_child(owner)`（Codeg `surface_child.rs:919-934` 的
  组装：`.with_id(label).with_bounds(rect).with_visible(v).with_focused(false)` +
  自定义 UA）。全部活在主线程 thread_local registry，外部持 `ChildHandle` hop 进去
  （`surface_child.rs:57-77`）；**持 registry mutex 时绝不调 handle**（死锁规则）。
- bounds 用逻辑像素，前端 `NativeSurfaceHost` 算矩形 → 命令 `browser_set_bounds` →
  wry set_bounds（Codeg `commands/browser.rs:957`）。
- **必修坑①（我们的 wry 0.55.1 正中枪）**：wry 在任何以宿主为父的 webview drop 时
  无条件摘窗口 resize subclass（wry `src/webview2/mod.rs:76`）——关一个 tab 会让 app
  自己的 webview 卡在旧尺寸。照搬 `SetWindowSubclass` hook（id `0xC0DE`）：
  `WM_SIZE`→Controller.SetBounds、`WM_MOVE/MOVING`→NotifyParentWindowPositionChanged、
  `WM_SETFOCUS/WM_ENTERSIZEMOVE`→MoveFocus、`WM_NCDESTROY` 摘除
  （Codeg `shim/windows.rs:1714-1815`）。**vendor/wry 若升级，先对
  MARIONETTE-PATCH.md 核这条还成不成立。**
- **必修坑②**：popup（window.open / target=_blank）必须复用 opener 的
  `ICoreWebView2Environment` 存为 OpenerConfiguration（`surface_child.rs:682,742-751`）。
- **坑③ profile**：`~/.marionette/browser-profiles/<id>/`（独立 WebView2 user data
  folder，cookie/storage 与系统 Edge 隔离；默认只建 `default`）。新建 webview 自动继承
  vendor/wry 补丁 #2 的 `MemoryUsageLevel=Low`。
- 懒加载：**没有任何浏览器 tab 时零进程零内存**；tab 全关 = renderer 回收。

### 1.2 aria 引擎与注入（vendor Codeg 的 browser-agent）

- 直接拷 `D:\Projects\codeg\browser-agent\`（TS 源码 + vendored Playwright v1.63.0
  ariaSnapshot 九文件 + `scripts/build-browser-agent.mjs` esbuild 构建）进
  `src-tauri/browser-agent/`，**保留 VENDOR.md 与 Apache-2.0 署名**；产物
  `agent.bundle.js` 提交进仓、Rust `include_str!`（Codeg 同款，CI 不用跑 esbuild）。
  bundle 暴露 `__marionetteAgent = { snapshot, elementForRef, act, locate, rectOf }`
  （改名即可，接口照抄）。**自研可达性树是大坑，一个字都别自己写。**
- Windows 注入 = CDP（Codeg `shim/windows.rs:314-385`）：isolated world + 
  `Runtime.addBinding` + `Page.addScriptToEvaluateOnNewDocument`。装载协议
  「probe→install」：world 不在返回哨兵，install 一次求值装 bundle + 拍快照。
  **只在第一次有授权读取时才注入**——未共享页面里不存在读取代码。
- 注入脚本四件：helper.js（page→host 通道，document-start）、console.js、picker.js
  （按需）、agent.bundle.js（授权后）。

### 1.3 授权模型（用户定调的最终形态）

- 三级 `Grant { None, Read, Control }` 挂 tab 状态（无独立 map，origin 写入点同锁
  撤销，Codeg `agent.rs:113-141`）；origin 严格相等、跨域导航即失效；
  `grantable_origin` 只收 http(s)，about:blank/data: 不可授。
- **agent 自开标签（browser_open_tab）→ 自动 Read**（调研零摩擦），**Control 走
  对话内权限卡**（§0.2，含 always-approve 自动放行）。
- **用户自开标签（用户手动输 URL/点链接）→ Read 和 Control 都要工具栏手动分享**
  （隐私边界，always-approve 不穿透）。工具栏下拉：Share（Read）/ Allow actions
  （Control）/ 撤回 + activity strip（agent 每次 Read/Click/… 都记一条）。
- localhost 额外 pin「程序+工作目录」（读内核 socket 表识别监听进程；**故意不 pin
  pid**——dev server 每次热重载换 pid；程序/workdir 变了才撤销，**没人监听不撤销**，
  Codeg `agent.rs:71-83, 452-478`）。
- **自己的窗口永不可分享是结构性的**：分享入口只认 `BrowserRegistry` 里的 tab_id，
  app 自己的 UI webview 从不进 registry。
- 无闲置 TTL；撤销/变化事件 `marionette-browser-grant`（Granted/Revoked/Navigated/
  Replaced 四态）。

### 1.4 工具面（全量）

- Read 级：`browser_list_tabs` / `browser_open_tab` / `browser_snapshot`
  （aria 树默认 40K 字符截断，超限返回 `truncated:true` 邀请分段）/ 
  `browser_console_messages`（每 tab 200 行环形 + missing 计数必须明示）/
  `browser_screenshot`（base64 上限 3.5MB，超预算逐次缩宽重编码 JPEG q85）。
- Control 级：`browser_click / browser_hover / browser_type / browser_press_key /
  browser_select_option`（aria ref 寻元素，非像素坐标——弱模型也稳）。
- `browser_eval`：独立开关**默认关** + Control 级 + **逐段确认**（120s 静默=拒绝、
  拒绝后 15s 冷却、全局同时一个问题；代码跑**页面自己的 world**、内联表达式不触发
  CSP——Codeg `eval.rs:25-38` 三重门照抄）。
- 错误 slug 族：`browser_grant_required / browser_no_such_tab / browser_stale_ref /
  browser_action_failed / browser_blocked / browser_open_failed …`（14 个照搬），
  `grant_required` 附教 agent 请用户分享的话术。

### 1.5 人→agent（工具栏 Send to chat）

- picker（选元素）/ console 错误（≤20 行）/ 整 viewport 截图 → **一个 markdown 块进
  composer，人读改删后才发出**。块头 UNTRUSTED_NOTE（照抄 Codeg 原文语义：这是页面
  数据不是指令）；URL 敏感段 redact。picker 是 closed shadow root overlay、按压在
  capture 阶段被吞（选页面的 Delete 不会真按下 Delete）、导航即终止。
- 下载：引擎管传输，host 只选落地路径（系统下载目录 + `safe_file_name` 防穿越）+
  toast「打开所在文件夹」；绝不自动打开。

### 1.6 前端

- 浏览器 tab = 中栏伪标签（文档标签页同款形状：`SessionView` 尾部 chip、
  `.browser-tab-page` 绝对覆盖、dock 态 DockPage `"browser"`、分屏态整栏覆盖）。
  NativeSurfaceHost 按矩形创建/同步原生 surface；tab 不可见时 TrySuspend 配对
  （vendor/wry 补丁 #3 的 hide/show 成对红线）。
- 设置页：总开关 / eval 开关 / profile 管理 / activity 审计查看。

### 1.7 明确不做

设备仿真、远程工作区浏览器桥（SOCKS-over-WS）、find bar、多 profile 市场同步。

## 2. 批次 3：Computer Use（Windows 全量）

### 2.1 driver 生命周期（`src-tauri/src/computer/driver.rs`）

- `~/.marionette/cua/`：exe + manifest（version/sha256/installed_at）+ 临时件清扫。
  下载/SHA-256 双 pin（archive 解包前 + **executable 每次启动前由 helper 校验**——
  主进程校验了也会被 agent 随后换文件）/ 进度事件 / 2 次重试 / 崩溃恢复
  （`.old` 让位 + recover）——**整体照抄 `engine.rs:474-586` 的形状**。
- pins：`trycua/cua` release `cua-driver-rs-v0.36.0`，win-x64 zip 实测 35.2MB
  （win-arm64 也留条目；URL/SHA 表写死，升级 = 改代码重审 digests）。缓存 id
  `@cua-driver`（`@` 开头不会撞 agent 目录名）。
- 设置页：安装/卸载（卸载先停 helper）、下载进度 MB。

### 2.2 helper = 主 exe 参数分支（单二进制）

- `marionette.exe --computer-helper` 直接进 helper 模式（省 sidecar；构建系统零改动）。
  Codeg 的 helper 是独立 bin（`src-tauri/src/bin/codeg_computer_helper.rs`），我们的
  分支模式在其 `helper/mod.rs` 逻辑层之上等价。
- 通道：**stdio 管道 + 长度前缀 JSON 帧**（16MiB 上限；`PROTOCOL_VERSION` 握手，
  helper 先发 Ready；kill_on_drop + `CREATE_NO_WINDOW`）。主进程超时：写 10s /
  ready 15s / 请求 300s。helper chdir 到自己的数据目录（`~/.marionette/computer-helper/`）
  再启 driver。**Marionette 死 → helper stdin 关 → helper 自退并停 driver**。
- **HelperOp / WindowAction 封闭清单照抄**（`computer/protocol.rs:73-345`：Configure/
  ListWindows/Capture/Snapshot/Verify/Act/CaptureScreen/ActScreen/ClipboardRead/
  ClipboardWrite/Halt + Click/Drag/Scroll/Type/Key/SetValue/Restore/InvokeMenu/
  SetFrame），**没有任何 op 允许传 driver 工具名**——driver 原生广告几十个工具
  （杀进程/录输入回放），helper 只把每个 op 亲手重组成固定 driver 调用。
- driver 通信：stdio MCP（initialize + tools/call，单调用串行）。环境钉死：固定 PATH、
  每次启动新 HOME、telemetry 关、`max_image_dimension:0`（按窗口原尺寸出图，坐标
  换算 1:1——中间夹小图会移动后续所有点击坐标）。

### 2.3 授权与安全（Windows 集）

- `GrantLevel` 复用浏览器模块；`GrantScope { Window, App, Screen }`；target 身份 =
  `(pid, started_at, window_id, content)`（pid 会复用，GetProcessTimes 取启动时间）。
  内存表不落盘；TTL 设置项默认 30 分钟（从最后一次读起算，墙钟；时钟回拨 >60s 判失效）。
- **分享纯人发起**：状态栏 popover → 窗口选择器（缩略图 + 「只读」/「读取并操作」
  两级）+ 整屏 tile。agent 请求未共享目标只拿 `computer_grant_required` 返回值
  （附「请用户在状态栏分享」话术）。**agent 主动发起的分享请求同样走对话内权限卡**
  （§0.2，与浏览器 Control 一致）。
- **自己的窗口永不可分享**：`marionette.exe` 按镜像名兜底（防 dev 构建装在正式版
  旁边）+ 默认黑名单（系统设置/密码提示 `credentialuibroker.exe`/`consent.exe`/
  密码管理器族）。screen 共享时每个窗口重新过 `grantable()`，不可共享的在截图上
  **涂死**（`paint_over`，前后两次列窗口、期间静止，最多重试 3 次）。
- **按键白名单**（`keys.rs` 照抄）：窗口级裸键放行（F 键和 shift+delete 除外）、
  快捷键只放行 a/c/x/f/z/g/y/方向/ctrl+home/end/backspace/delete；**粘贴一律拒绝，
  例外 = agent 自己 `computer_clipboard_write` 写入且 stamp 未变**；App 级挡桌面
  和弦（任何 Win 键、Alt+Tab/Esc、Ctrl+Esc、Ctrl+Alt+任意）；Screen 级挡
  Win+L / Win+Tab / Ctrl+Alt+Del。
- **全局停止**：tauri global-shortcut 注册（≥2 修饰含 Ctrl，面板可改键、占用时显示
  失败态）；按下 = 撤销全部 + helper Halt + stop 计数（helper 拒绝服务任何
  Stop 之前放行的请求）。
- **指示条**：小 always-on-top 无装饰 WebviewWindow（`focusable(false)` +
  `accept_first_mouse`），顶部居中 360×44；**Windows 上 show 必须绕过 tao 直调
  `ShowWindow(SW_SHOWNOACTIVATE)`**（tao 0.37.1 的 show 无视 WS_EX_NOACTIVATE 抢
  键盘焦点——照搬 `overlay.rs:22-62`）。动作落点标记 96px 点击穿透环 1.2s 消失。
- 遮挡：`DwmGetWindowAttribute(DWMWA_CLOAKED)` + `IVirtualDesktopManager` +
  `IsIconic`；被盖住返回 `computer_occluded`（不抢前台恢复；恢复走
  ShowWindowAsync）。整屏只取主显示器。
- 键盘态：foreground 动作前 `GetAsyncKeyState` 查用户正握着的修饰键，握着就不发。

### 2.4 工具面（Windows 可用集，18 选 N）

`computer_list_apps / list_windows / screenshot / snapshot / verify / launch_app /
clipboard_read / clipboard_write / click / drag / scroll / type / press_key /
hold_key / set_value / restore / invoke_menu / set_window_frame`——snapshot 默认
40K 截断、截图长边 1568（helper 侧缩，driver 永不缩）；错误 slug 族照搬
（`computer_grant_required / computer_stale_ref / computer_occluded /
computer_stopped / computer_paused …`）。走 §0.1 同一个 localhost MCP。

### 2.5 不移植（macOS-only）

`tcc.rs / codesign.rs / launch_req.rs / spawn.rs / helper_app.rs` 五个模块整体跳过
（TCC、代码签名双向验、launch requirement、POSIX_SPAWN、bundle 外副本）。Windows
保留：每次启动前 SHA-256 + 固定环境 + 封闭 op 面。

## 3. 风险与对策

| 风险 | 对策 |
|---|---|
| wry 升级把 resize-hook 前提改掉 | 升级 vendor/wry 前对照 MARIONETTE-PATCH.md + 重测「开关浏览器 tab 后主窗 resize」 |
| prompt injection（页面诱导 agent 点击） | Read/Control 分离；Control 必经权限卡；用户自开标签不自动放行；UNTRUSTED_NOTE；activity 审计 |
| 有 shell 的 agent 绕过工具面 | 文档明说 gate 不是安全边界（Codeg 原文 `computer/mod.rs:36-40`）；边界 = 工具面允许模型做什么 |
| MCP token 泄漏给其它进程 | token 仅随 session/new 给对应 agent 进程；服务端绑定 sessionId；回环-only |
| 权限卡无人应答卡死调用 | 120s 默认拒绝；前端卡可手动 dismiss |
| cua-driver 上游 breaking | pin 死版本 + SHA；升级走「改代码重审」流程（Codeg 同款纪律） |
| 浏览器 tab 内存 | MemoryUsageLevel=Low 继承；不可见 tab TrySuspend 配对；全关即回收 |

## 4. 验证

- 批次 2：手动 E2E——开 tab 走正常网页；agent `browser_open_tab` + snapshot 出 aria 树；
  未授权 tab 调用返回 `browser_grant_required`（不是报错）；权限卡出现/应答/超时拒绝
  三态；always-approve 会话自动放行 Control；picker 选元素进 composer 带 UNTRUSTED_NOTE；
  关闭所有浏览器 tab 后任务管理器确认 renderer 进程回收、主窗 resize 正常。
- 批次 3：driver 下载 pin 校验（改一字节 → 拒装）；分享→snapshot→点击闭环；
  `computer_occluded`（把目标窗盖住重试）；停止快捷键全撤销；粘贴拒绝 +
  自写剪贴板例外；卸载清目录。
- 回归：`cargo test`（tools_mcp/browser/computer 各加单测：token 校验、帧协议
  round-trip、按键白名单表驱动）；`npm run build`；zcode 会话真实拉起 http MCP
  （E2E：工具出现在 configOptions 后的模型调用里）。
- dev.log：新增 `[tools-mcp]` / `[browser]` / `[computer]` 三类 source，事故可溯源。

## 5. 参考

- Codeg 本地克隆 `D:\Projects\codeg`（Apache-2.0；browser/ `surface_child.rs`、
  `shim/windows.rs`、`agent.rs`、`handoff.rs`；computer/ `protocol.rs`、`keys.rs`、
  `driver.rs`、`overlay.rs`、`hwnd.rs`）。
- 行号基准：v0.41.3（origin/main = `ddd1f09f`，2026-10-30 复核）。

## 6. 落地记录（2026-10-31，批次 2 浏览器共享已实现）

实现文件：Rust `src-tauri/src/browser/{mod,surface,agent}.rs` + `tools_mcp.rs`（注入点
`acp.rs` session/new 前）；前端 `src/components/BrowserPage.tsx` +
`src/lib/browserTabs.ts` + App/SessionView/DockChrome 接线 + `browser-page.css`；
vendored `src-tauri/src/browser/js/`（agent.bundle.js + helper.js，Apache-2.0，
VENDOR.md 记来源与「为什么保留 `__codegAgent` 名字」）。

与 §1 设计稿的偏差（都是有意取舍，后续批次可补）：

1. **权限卡 v1 是模态弹窗不是转录内卡片**（`BrowserPermissionDialog`，允许一次 /
   允许并记住此站点 / 拒绝；Rust 侧 120s 超时自动拒绝）。always-approve 集成照设计
   生效（`set_session_approval` 镜像，Composer 挂载时推送；**只覆盖挂载过 Composer
   的会话**——后台会话的 Control 请求会出卡，这是安全方向的漏，不是反方向）。
2. **localhost listener pin（程序+工作目录）未做**：grant 只按 origin 绑定。
   agent 自开标签跨 origin 导航时**自动重授 Read**（调研要跟链接，否则死在第一个
   外链）；用户自开标签跨 origin 照抄 codeg 撤销。
3. **popup 降级**：wry `NewWindowResponse::Deny` + 下一拍开新标签——`window.opener`
   丢失（codeg 用 Create 保 opener）。弹出的标签继承来源标签的 opened_by_agent。
4. **人→agent 手递（picker / console 递话 / Send to chat）未做**——批次 2 的下一小批。
5. **browser_eval 未实现**（设计稿本来就是独立开关默认关）。
6. **下载未接管**：WebView2 自带下载条在页面内弹出，落系统默认下载目录。
7. **前进/后退按钮未做**（reload 有；历史导航留待 toolbar 完善）。
8. 工具面实现：list/open/close/snapshot/console/screenshot + click/hover/type/
   press_key/select_option 共 11 个，schema 与 slug 族照设计稿。

红线（改代码前重读）：

- **grant 检查只有一处**：`browser::tool_read` / dispatch 的 Control 分支。MCP 与
  工具栏共用 `set_grant`，**别在别处再写第二套检查**。
- **resize hook 是生存条件**：`surface::ensure_resize_hook` 必须在第一个 child
  webview 之前装上（`create_surface` 自己保证）。升级 vendor/wry 前先对照
  MARIONETTE-PATCH.md 重测「开关浏览器标签后主窗口 resize」。
- **world/binding 名字保留 codeg**：`WORLD_NAME="codeg"`、`__codegSend`、
  `__codegAgent`——bundle 是压缩产物，改名 = 重放上游构建（js/VENDOR.md）。
- **拒绝是返回值**：所有 `browser_*` slug 走 `{"error": …, "note": …}` JSON，
  isError 只给宿主级意外。
- **持 registry borrow 时绝不 CDP 等待**：`call_and_wait` 泵消息循环，泵期间任何
  surface 生命周期事件都要能拿到 registry（`with_surface` 的 clone-out 模式）。

## 7. 落地记录（2026-10-31，批次 3 Computer Use 已实现）

实现文件：`src-tauri/src/computer/{mod,driver,helper,keys,protocol,win}.rs`（helper =
主 exe `--marionette-computer-helper` 参数分支）；工具面进 `tools_mcp.rs`（12 个
`computer_*`，与浏览器同一 localhost MCP）；前端
`src/components/ComputerShareDialog.tsx` + `src/lib/computer.ts`（Ctrl K 入口）。
cua-driver 0.36.0 win-x64 双 SHA pin（archive `f8c470…353ad` / exe `99291d…52f4f`，
**zip 是 6 文件整包**——UIA provider/SDK dll/node runtime 是必需兄弟文件，单拷 exe
装出的是握手成功但看不见窗口的残废 driver；解压用系统 tar.exe）。

与 §2 设计稿的偏差（有意取舍）：

1. **工具面 12 个不是 18**：list_windows / screenshot / snapshot / click / drag /
   scroll / type / press_key / set_value / restore / clipboard_read /
   clipboard_write。**未实现**：verify（snapshot 覆盖读需求）、launch_app/list_apps
   （Windows 上 driver 的 app 清单价值低）、hold_key、invoke_menu、set_window_frame
   ——enum 形状留好，接到 driver 对应调用即可。
2. **App/Screen 范围在协议里、不在 UI/检查里**：共享对话框只列窗口；Screen 捕获的
   paint_over（涂死不可共享窗口）与 ActScreen 未实现。这是**明确的范围裁剪**：
   整屏共享含密码管理器泄露面，做就要带 paint_over，别只做一半。
3. **权限卡的「记住」对 CU 等于允许一次**：卡上「允许并记住此站点」只翻浏览器
   tab 的 grant；CU 窗口的长期共享从共享面板给。always-approve 语义照常生效。
4. **指示条（indicator overlay）未做**：停止 = 全局快捷键 Ctrl+Alt+Shift+S
   （tauri-plugin-global-shortcut，占用只告警）+ 面板「全部停止」。always-on-top
   悬浮条需要独立 HTML surface，价值/成本不划算，搁置。
5. **foreground 交付未做**：全部动作走 driver 的 background 模式（不抢焦点），
   `held_modifiers` 检查（用户正握修饰键时不发）随之未接线——background 模式下
   不与用户输入组合，风险面本来就小。
6. **粘贴例外已实现**：helper 记 `GetClipboardSequenceNumber` 戳，agent 自己
   `computer_clipboard_write` 写入且序列号未变时 `Control+v` 放行。
7. occlusion：截图失败/verify 无图返回 `occluded` slug；`computer_occluded` 精确
   判定（DWMWA_CLOAKED 等）未接线（native 枚举段删除时一并裁掉）。
8. **驱动 MCP 客户端是同步 NDJSON**：单请求在途（driver 串行），无并发管道；
   超时读依赖 driver 必答（挂死由外层 request timeout 兜底杀 helper）。

红线（在 AGENTS.md 同步）：

- **封闭 Op 集**：主进程→helper 只有 protocol.rs 里那 8 个 op；helper→driver 的
  工具名硬编码在 `driver_call`/各 op 里，**任何地方不得透传 agent 给的工具名**。
- **exe SHA 每次启动前校验**（helper 里做，不是装完拉倒——装完校验挡不住之后被
  换文件）。
- **grant 检查在 mod.rs 的 ensure_read/ensure_control**，MCP dispatch 只调它们。
- **自己的窗口/黑名单**：is_own_image（镜像路径 + marionette.exe 名兜底）+
  DEFAULT_BLOCKLIST（密码管理器族）在 list 与 share 两处过滤。
- 按键白名单在 keys.rs（表驱动测试），粘贴例外 = 剪贴板序列号未变。

## 8. 2026-10-31 夜间修复批（用户实测「浏览器标签开后全死」）

**症状**：新建浏览器标签后页面钉在左上角盖住标题栏/侧栏/工具栏，点不到 ×、关不掉标签。

**根因（对照 codeg 逐行核实）**：
1. **焦点抢夺**：wry 原生 `set_visible` 用 `ShowWindow(SW_SHOW)` = 激活容器窗口 →
   键盘焦点进页面，而本 app 全部快捷键（Esc/Ctrl+W/Ctrl K）是 app webview 的 DOM
   监听 → 全部失灵。codeg 同样用 SW_SHOW（他们接受），我们叠加根因 2 后就是全死。
2. **初始 bounds 800×600@0,0**：创建时给了一个「占位」矩形，前端测量同步一旦
   失手/迟到，容器就以 800×600 盖住窗口左上角（自定义无边框标题栏、侧栏、浏览器
   工具栏全在内）——鼠标点击全部落进页面区域。
3. **自制显示路径绕开补丁配对**（同夜修）：曾用 ShowWindowAsync+SetIsVisible 直控，
   绕开了 vendor/wry 补丁 #3 织在 `set_visible` 里的 TrySuspend/Resume 配对——
   挂起无人解除（沙箱内表现为页面永不合成）。**结论：回到 codeg 的原样调用
   `wv.set_visible(v)`，永远别绕开。**
4. **CU 两处致命偏差**（对照 codeg driver_proc/mcp.rs）：cua-driver 必须带
   **`mcp --direct --no-overlay`** 三参数启动才会讲 stdio MCP（否则永不应答
   initialize）；工具结果的结构化数据在 **`structuredContent`** 键（MCP 2025-06-18），
   refusal/error 码在 `/refusal/code`、`/error/code` 指针下。

**修复**：创建 bounds 改 1×1（失手=隐形点，不再是盖屏板）；前端测量循环重试
（<40px 不显示，最多 2s rAF），bounds 成功才 set_visible；set_visible 回到 wry 原生
（补丁配对完整）+ 显形时 MemoryUsageLevel=Normal；CU 补 driver 参数与结果键。

**沙箱验证到的**：容器几何正确（中栏内、不盖标题栏）、主线程健康（WM_NULL 秒回）、
焦点不再被抢（留在 app webview）、无 panic。**沙箱验证不到的**：子 webview 页面
渲染（本沙箱虚拟 GPU 对后创建子窗口的 D3D 内容 PrintWindow 拍不出来；用户机器上
页面本来就渲染正常）。**待用户实测**：开标签→页面显示→工具栏可点→Esc/Ctrl+W 生效
（点进页面内容后键盘归页面，点一下工具栏即回来——与任何内嵌浏览器一致）。

## 9. 2026-11-01 根因定案：嵌套消息泵冻死 app 渲染器（二分实锤）

**症状**：开浏览器标签 → app 自己的 JS 永冻（ui-heartbeat 停、点啥都没反应、零 CPU、
无崩溃、无 panic）。用户两次实测复现；沙箱用 `MARIONETTE_AUTO_BROWSER_TAB=1` +
「90 秒后数 ui-heartbeat」稳定复现。

**二分矩阵**（skip 旗标逐一排除）：

| 组 | 配置 | 结果 |
|---|---|---|
| 对照 | 不开标签 | 活（1 心跳） |
| C | 完整创建但前端不激活 | **死** → 凶手在 Rust 侧 |
| B | 只 wry build（跳过 world/load） | **死** |
| D | 默认环境（不共享 WebContext）+ 跳过 world/load | 活 |
| D2 | 默认环境 + world + load | **死** |

**根因**：wry 的 `build_as_child` 与 CDP 安装内部用
`webview2_com::wait_with_pump` = **嵌套的阻塞 GetMessage 泵**。在 tauri
`run_on_main_thread` 回调（= tao 0.35 事件循环回调内部）里跑它，泵会重入
tao/tauri 的消息分发——IME 补丁容忍了断言不崩，但重入本身把 app webview 的
渲染器 IPC 永久卡死。codeg 无恙是因为他们跑在**未打补丁的新版 tao** 上
（subclass/paint-flush 机制已移除）；我们 tao 0.35 的红线（AGENTS.md 记过
paint assert）就是同一颗雷的另一种踩法。

**修复（两层）**：
1. **预热**：tauri `setup`（事件循环启动前——泵是外层循环，唯一安全窗口）
   内联建好一个隐藏 1×1 surface（about:blank），首个标签**零创建**直接复用
   （`surface::prewarm` / `take_prewarmed`）。
2. **world 懒装**：CDP 订阅 + 注入从创建路径挪到**首次读取**（snapshot/act/
   console/screenshot 的 `ensure_world`，幂等）——创建突发期不再连打十几个泵。

**验证**（完整配置、无 skip 旗标、自动化）：JS 心跳恢复 ✓、页面加载
（frameTree = example.com）✓、**渲染器真渲染**（CDP screenshot 95KB）✓。

**遗留**：第 2+ 个标签仍在事件循环内创建（短泵；D 组证明裸创建可存活，
world 已不在该路径）——如再出冻死，把 prewarm 池扩到 N 个即可。

> **2026-11-06 偏差：预热已整体移除（懒建）。** 启动预热让第二套 WebView2
> 进程组（browser-profiles 的 browser/GPU/utility）**常驻整个进程生命周期**，
> 哪怕用户从没开过浏览器标签（内存压制批 X1）。首标签现在走 `create_surface`
> 直落 `build_surface`——即上文「第 2+ 个标签」的既有路径（`ensure_resize_hook`
> 在其中自保证）。风险按 §9 矩阵看是「首次建 WebContext + 事件循环内 build」，
> 与 D 组（复用 WebContext）不完全同形；若首标签再出冻死，回退 = 恢复
> main.rs setup 里的 `surface::prewarm` 调用（prewarm/`PREWARM_SURFACE`/
> `take_prewarmed` 已随本批删除）。

## 10. 2026-11-01 CU 面板修复批（状态机自锁 + 全流程演练台）

用户实测三个症状：装完回到未安装态 / 二次点击卡「下载中…」/「helper did not answer
configure in time」。根因与修复：

1. **helper 引导自锁**（致命）：读线程经全局 STATE 锁路由应答，而首次 configure 的
   请求方**抱着同一把锁等应答** → 15s 超时。修：HelperProc 收发管道拆独立
   `Arc<Mutex<HelperIo>>`（writer+pending），读线程只碰 IO 锁、请求方等应答时不持
   STATE 锁；helper 先入表再 configure。
2. **已安装早退不发事件**：二次点安装卡进度。修：早退也 emit "installed" + 前端
   invoke resolve 即无条件刷新。
3. **列表拉取失败误报未安装**：helper 抖动 → 整个命令 Err → 前端回到安装按钮。
   修：`computer_list_shareable` 降级返回 `helperError`（驱动状态独立报告），
   面板显示错误行 + 刷新按钮。
4. **面板按钮样式**：「只读/可操作/刷新」塞进 icon-only 的 pill 样式会竖排/挤爆
   → 换文字按钮样式；列表限高 42vh 保底栏常驻。

**全流程演练台**（`MARIONETTE_AUTO_CU_CHECK=1` + `lib/cuWalkthrough.ts`）：真实
app 内按面板同款调用链走完 装引擎(35MB) → 列表(12 窗) → 共享(read) → 验证 →
全撤回 → 开面板量 DOM 几何。**最终全绿**：installed/12windows/share=ok/shared=read/
revoke 干净/dialog 居中(x=360,w=560)/底栏可见。沙箱 PrintWindow 对该帧的抓图
与 DOM 几何矛盾（GPU 合成抓取在此沙箱早已证实不可靠），以 DOM 实测与用户真机
截图为准。

## 11. 2026-11-01 补遗：幽灵标签 + 坑清单汇总 + 演练台工具

**幽灵标签**（用户实测：启动即出现关不掉的「浏览器 1」）：预热 surface 曾被直接放进
正式注册表——启动时前端一拉列表就看见它。修法：`build_surface`（纯构建）与
`register_surface`（入表+广播）分离，预热面存注册表**之外**（`PREWARM_SURFACE`
持有 `Rc<Surface>`），首个真实标签 `take_prewarmed` 认领时才转正。芯片 × 语义同步
修：无激活标签时关第一个（否则 × 变成"只藏不删"）。

**本轮全部坑的速查表**（详情见 §6–§10 与 AGENTS.md）：

| 坑 | 一句话教训 | 记录处 |
|---|---|---|
| wry 嵌套泵冻死 app 渲染器 | tao 回调内绝不 `wait_with_pump`；创建挪到 setup 预热、CDP 懒装 | §9 + AGENTS |
| 预热面泄进注册表 | 基础设施（预热/缓存）永远不进用户可见的状态表，认领才转正 | §11 |
| 自创 CSS 类名（dialog-scrim/btn） | 写弹窗前先看 DialogShell/ConfirmDialog 用什么类；`pill-action--sm` 是 icon-only 专用，装文字会竖排/挤爆 | §10/§11 |
| helper 引导自锁 | 等应答绝不持有路由要用的锁；收发管道独立 `Arc<Mutex<IO>>` | §10 |
| 已安装早退静默 | 前端靠事件驱动的流程，早退路径也要发"完成"事件 | §10 |
| 列表失败误报未安装 | 驱动状态与列表状态必须解耦，降级不误报 | §10 |
| cua-driver 启动参数/结果键 | `mcp --direct --no-overlay`；结果在 `structuredContent`，refusal 在 `/refusal/code` | §7 |
| 自制 set_visible 绕开补丁 | 永远走 wry 原生调用（补丁把 TrySuspend/Resume 织在里面） | §8 |
| 沙箱 PrintWindow 抓不出 GPU 内容 | 本沙箱验证子 webview 画面不可信，用 CDP screenshot / DOM 几何代替 | §10 |

**演练台工具**（以后改这两块先跑这些，别再拿用户当测试）：

- `tools/probe-computer-helper.py <exe>` — helper 模式协议四连（ready/未知op拒绝/假configure拒绝/净退）
- `tools/probe-cu-e2e.py` — helper→driver→list-windows 端到端（真机真驱动）
- `tools/capture-window.py <title> <png>` — PrintWindow 拍窗（GPU 内容在本沙箱不可信，见上）
- `MARIONETTE_AUTO_BROWSER_TAB=1` — 开机 3s 自动开浏览器标签（冻死复现钩子）
- `MARIONETTE_AUTO_CU_CHECK=1` — 开机跑 CU 面板全流程演练（`lib/cuWalkthrough.ts`，日志 `[cu-check]`，含 DOM 几何）
- 冻死检测口径：**数 ui-heartbeat**（60s 一条；开标签后停跳 = JS 冻死），比看窗口/进程可靠得多
- 二分旗标：`MARIONETTE_BROWSER_SKIP_WORLD / SKIP_LOAD / SKIP_HOOK / NO_CONTEXT / NO_ACTIVATE`（surface.rs / App.tsx）
