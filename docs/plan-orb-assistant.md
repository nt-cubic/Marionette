# 悬浮球后台常驻助手 — 设计稿（未实现）

> 2026-10-22 定稿。五档实施计划，每档做完可验收再做下一档。
> 状态：**设计已认可，代码未开工**。落地后本文件的行为描述并入 `CURRENT.md`，本文件挪入 `archive/`。

## 0. 背景与目标

Marionette 目前是「打开窗口才存在」的图形壳：关主窗 = 杀引擎、杀全部 agent 会话、
Telegram 桥随之死亡。目标是把 Marionette 变成**后台常驻的桌面管家**：

1. 随手开新对话；
2. 记录想法 / 收藏网上看到的图片、链接、文字 → AI 自动整理；
3. 自定义按钮（如 Notion 按钮：把东西丢进去，AI 归档到对应位置）；
4. 余额列表（QuotaBar 式，全 provider 一屏可见）；
5. 并行任务总览；
6. 任务完成的表现（球体感叹号角标 + 提示音）。

概念模型：**不同按钮 = 预设好的情景**。例如「记录」按钮一定是记录行为而非执行任务，
内容自动作为对话发进 Marionette 既有的会话体系——不另起炉灶建运行时。

远期设想（不在本计划内）：后台常驻后进一步看屏幕、控鼠标，配合 Telegram 远程查询
（「我之前那个文档去哪了」）、与 NAS 联动。都建立在「会话 + 文件」这两样既有原语上。

## 1. 总体概念：输入法式悬浮球

新增一个**桌面右下角常驻小圆球**：独立 always-on-top 透明 WebView 窗口
（label=`orb`，约 72×72），承载全部功能。点开是功能面板（约 380×540）。

- **主窗口关闭 = 隐藏到后台**（默认开，可在设置关掉）：引擎、全部会话、Telegram 桥
  继续运行，点球随时唤回主窗。**真正退出只从球菜单进**（`quit_app` → 现有
  `shutdown_and_exit`，绕 tao paint assert 的既定路径）。
- **球是遥控器，主窗是执行器**：主窗隐藏但 WebView 存活。程序化建会话/发消息沿用
  Telegram `telegram-remote-command` 的「事件 → 主窗 App.tsx 执行 → 回执」模式
  （参照 `src/app/App.tsx` 中 `/new` 向导的命令处理分支）。
- 球窗口 `dragDropEnabled: false` 走 **HTML5 DnD**：这是从浏览器拖**文字/链接/图片**
  进来的唯一途径（Tauri 原生拖放只给文件路径，见 `Composer.tsx` 的双轨注释）；
  文件走 blob 读取后由 Rust 命令落盘。
- **零新依赖**：不加 tray、不自启、不加全局热键；不抄重件
  （Meilisearch / Puppeteer 截图 / OCR / 向量库全部不要）。存储只有「会话 + markdown」。

## 2. 收集箱 = 速记流 + AI 知识库（两层，零新存储）

对标 [Memos](https://github.com/usememos/memos)（时间线速记）与
[Karakeep/Hoarder](https://github.com/karakeep-app/karakeep)（捕获 → 富化 →
LLM 自动打标签归档）的轻量合成：

**第 1 层 · 原始速记流（抄 Memos 时间线）**

一切捕获——想法 / 备忘 / 待办 / 链接 / 图片 / 文件——逐条发进置顶的「收集箱」会话，
一条 = 一则，时间戳由 transcript 天然携带。会话记录本身就是时间线：主窗可翻阅、
现有 `search_sessions` 可检索、未来 TG 远程可查。**没有独立数据库**。

**第 2 层 · AI 知识库（抄 Karakeep 的轻半边）**

收集箱 agent（真实会话、有文件工具、cwd=`~/.marionette/inbox/`）在整理时维护：

- `kb/<主题>.md`：按主题归档，每条带 `#标签`、日期、出处 URL；
- `todo.md`：AI 判断待办性质的捕获（「记得…」「下周…」）自动路由——
  **备忘 / 待办就此并入收集箱**，不再是独立功能；
- `README.md`：agent 自维护的主题 / 标签索引页。

每条捕获的整理提示词：富化后的内容 → 归并进对应文件（去重、合并、补标签）。
会话忙时靠现有 per-session 队列天然串行 / 攒批。

**富化（只做一件小事）**：链接发送前 `capture_fetch_url` 抓页面 `<title>`；图片 URL
顺手下载（content-type 校验 + 20MB 上限）落 `inbox/files/`。不截图、不 OCR、不存档整页。

## 3. 事件与命令契约

**前端事件**（Tauri `emit` 广播，按 window label 过滤，仿 `detachedWindow.ts` 的
origin 过滤模式）：

- `marionette-orb-state`（主窗 → 球）
  `{mainVisible, running:[{sessionId,label,agentLabel,startedAt}],
   notices:[{kind:"reply"|"permission"|"error",sessionId,label,at}]}`
  —— turn 起/止、主窗显隐、标签变更时推送。
- `marionette-orb-command`（球 → 主窗）
  `{kind:"newChat"|"openSession"|"showMain"|"scenario"|"capture", scenarioId?, payload?}`，
  主窗执行后经 `marionette-orb-state` / ack 事件回报。

**新增 Rust 命令**（薄封装，不动现有）：

| 命令 | 作用 |
|---|---|
| `quit_app` | 调 main.rs 现有 `shutdown_and_exit`（唯一合法退出路径） |
| `set_background_mode(bool)` | Rust 侧 AtomicBool，决定 CloseRequested 走 hide 还是退出 |
| `scenario_list` / `scenario_save` | 场景配置 CRUD，`~/.marionette/scenarios.json`（新 `scenarios.rs`，仿 `custom_agents.rs`） |
| `capture_save_blob` | 球窗拖入/粘贴的文件 blob 落 `inbox/files/`，返回绝对路径（供 ACP 图片附件） |
| `capture_fetch_url` | ureq：图片下载 + `<title>` 抓取（类型/尺寸限制，失败降级为只记 URL） |

**capabilities 增补**：`core:window:allow-set-size`、`allow-current-monitor`、
`allow-scale-factor`（现有文件已含 set-position / set-always-on-top / show / hide /
create-webview-window）。

## 4. 分档实施

### 档① 悬浮球基座 + 后台常驻

- `src/main.tsx` 入口分支：`?orb=1` 渲染轻量 `src/orb/OrbApp.tsx`（不挂 8000 行 App）。
- `src/lib/orbWindow.ts`：`ensureOrbWindow()`——主窗 App 挂载时创建/唤起球窗
  `{width:72, height:72, transparent:true, decorations:false, alwaysOnTop:true,
  skipTaskbar:true, shadow:false, focus:false, resizable:false, dragDropEnabled:false}`；
  默认主屏右下角，位置 localStorage 持久化，圆球带 `data-tauri-drag-region` 可拖动。
- 主窗关闭=隐藏：main.rs CloseRequested 分支改写（后台模式开 → hide；detached 关闭
  逻辑同步调整，主窗隐藏时不再因「无可见窗口」触发 shutdown；`Destroyed` 兜底保留）。
- ProjectShelf 设置行加「关闭时隐藏到后台」开关（默认开），改动即同步
  `set_background_mode`。
- 单实例交接（FindWindowW + SW_RESTORE）对隐藏主窗依然有效，不动。
- 球菜单：打开主窗口 / 退出 Marionette。

### 档② 快捷新对话 + 并行任务 + 完成提醒

- 点球展开面板（`setSize` 至约 380×540，向左上展开并 clamp 到显示器内；窗口 blur
  自动收起）。面板单列滚动：快捷输入区、场景按钮网格（档④填充）、运行中任务、
  余额（档⑤填充）、底部菜单行。
- App.tsx 加 orb 桥：推送 `marionette-orb-state`（挂点：`sendPromptWithTurnWait`
  起/止、主窗显隐、标签变更）；监听 `marionette-orb-command`：
  `newChat` → `handleCreateChatSession` + showMain；`openSession` → 打开该会话标签；
  `showMain` → show + focus。
- **完成提醒**：球体角标——任务完成 `!`（橙）、待授权 `🔑`（蓝）、出错 `⚠`（红），
  点球清除；运行中任务行 spinner，完成转绿 ✓ 短暂停留。声音：`mainVisible=false`
  时由球窗播现有 WAV（复用 `src/assets/notifications`），主窗可见不播避免双响；
  角标为主信道，声音尽力而为（无手势自动播放可能被 WebView2 拦）。

### 档③ 收集箱（§2 设计落地）

- 球窗 HTML5 DnD（`text/uri-list` / `text/html` / `text/plain` / `Files`→blob）
  + 面板快捷输入 + 球聚焦时 Ctrl+V 粘贴；拖悬停球体放大高亮。
- 归一化 capture item `{text, urls[], html?, filePaths[]}` → blob/URL 先富化落盘 →
  `capture` 命令 → 主窗确保「收集箱」项目（cwd=`~/.marionette/inbox/`）+ 置顶会话
  存在（现有 `add_project` / `createSessionForProject`）→ 模板化提示词 + 图片附件经
  `performSend` 发入。
- 整理完成走档② `!` 提醒；整理进行中球体显示计数。

### 档④ 自定义场景按钮

- 场景模型：`{id, name, emoji, kind:"capture"|"prompt", agentId?, modelId?, modeId?,
  promptTemplate(含 {content} 占位), target}`；出厂内置「记录收藏」「新对话」。
  用户可建任意按钮——例如 Notion 按钮 = prompt 场景，模板指示 agent 归档到指定目录。
  **真实 Notion API / MCP 不在本期**：v1 场景 = 本地目录 / 文件操作，云端目标后续接。
- UI：面板顶部场景网格（emoji+名）；主窗 ProjectShelf「快捷动作」入口 + 管理小对话框
  （仿 `CustomAgentDialog`）；快捷输入 Enter = 跑默认 capture 场景。
- 执行：`marionette-orb-command {kind:"scenario"}` → 主窗按场景 prefs 走
  `createSessionForProject` / `handleCreateChatSession`（prefs 参数即为此预留）→
  模板渲染 → `performSend`。

### 档⑤ 余额列表（QuotaBar 式）

- 面板「余额」区：球窗直接 invoke 现有 `engine_quota`（Rust 侧已有 30s 缓存），
  列全部 provider：订阅配额窗口（剩 X% + 进度条 + 重置时间）与 key 余额（¥/$ 字符串），
  样式抽取复用 ContextPanel `.usage-row` / `.usage-meter` 与 `usage.ts` 的
  `engineQuotaRows` 格式化。
- 引擎运行时 5 分钟自动刷新 + 手动刷新；引擎停止显示引导文案。
- 本期不接 magpie `/v1/magpie/usage` 账本与 `provider_usage.rs` 逐家探测（后续增强）。

## 5. 涉及文件

**新增**：`src/orb/OrbApp.tsx`（+ 面板/角标子组件）、`src/lib/orbWindow.ts`、
`src/lib/scenarios.ts`、`src-tauri/src/scenarios.rs`、`src-tauri/src/capture.rs`、
orb 样式段（app.css 或独立 orb.css）。

**修改**：`src/main.tsx`（`?orb=1` 分支）、`src/app/App.tsx`（orb 桥 / 收集箱
ensure+send / 后台模式同步）、`src/components/ProjectShelf.tsx`（设置开关 + 快捷动作
入口）、`src-tauri/src/main.rs`（CloseRequested / 后台模式 / quit_app）、
`src-tauri/src/commands.rs`（新命令注册）、`src-tauri/capabilities/default.json`
（3 条权限）、`AGENTS.md`、`docs/CURRENT.md`。

## 6. 风险与对策

| 风险 | 对策 |
|---|---|
| 隐藏主窗的 WebView2 定时器节流（~1s 粒度） | 事件照常到达，批处理略慢可接受；真出问题再把关键路径下沉 Rust |
| tao 0.35 paint assert | 全程只 hide / resize / 创建窗口，**绝不 destroy**；退出唯一路径 `shutdown_and_exit` |
| 透明 + 置顶小窗的 DPI / 多屏 | 统一 LogicalPosition，移动后持久化并 clamp 到显示器内 |
| 无手势自动播放声音被拦 | 角标为主信道，声音尽力而为 |
| 收集箱会话被用户改名/删除 | 每次 capture 前按 cwd 匹配项目 ensure，缺则重建 |
| 双窗口声音双响 | `mainVisible=false` 时才由球窗播声 |

## 7. 验证

每档完成：`npm run build` + `cargo check` + `npm run tauri dev` 手测该档验收点
（拖拽收图收链接、隐藏主窗后台跑任务、球体提醒、todo 路由、kb 归档、余额刷新、
场景按钮端到端）。

## 参考

- [Memos](https://github.com/usememos/memos) — 速记时间线模式（MIT）
- [Karakeep / Hoarder](https://github.com/karakeep-app/karakeep) — 捕获→富化→AI
  自动打标签模式（AGPL，只抄交互模式不抄代码）
- [QuotaBar](https://github.com/QuotaBar/QuotaBar) — 余额列表 UI 参考（macOS 菜单栏）
