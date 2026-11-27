# T3 Code ⇄ Marionette 产品审计（cross-project canonical audit）

> **性质**：只读研究 + 单文件产出。本文件是唯一被创建/修改的文件；T3 仓库与 Marionette 源码均未改动，
> 未执行构建、安装或提交。
>
> **审计对象**：`D:\Myself\t3code`（T3 Code，MIT，Node WebSocket server + Electron/Web/React Native 三端）
> **对照对象**：`D:\Myself\Marionette`（Tauri 2 + React，Windows 本地多 Agent 图形壳 + 跨用引擎，v0.9.1）
>
> **日期**：2026-11-27 ｜ **审计口径**：文档优先，文档含糊处读代码；文档与代码冲突时以代码为准并记录。
>
> **版本事实（可验证）**
> - T3：`apps/desktop`、`apps/server`、`apps/web` 均为 **0.1.13**；移动端 `apps/mobile/eas.json` 的
>   testflight 版本变量为 **3.0.0**；根 `package.json` 固定 `node 26.13.1`、`pnpm 12.14.0`。
> - Marionette：`package.json` **0.9.1**。
> - ⚠️ T3 检出是**单个压缩提交**（`git log` 只有 1 条，日期 2026-11-27，PR #44052），**无 tag、无 CHANGELOG**。
>   因此第 3 节的时间线**全部是推断**，已在每行标注 `[推断]`，推断依据是「Labs / beta / alpha / preview 成熟度标签 +
>   各 provider 的版本下限 + 迁移文档」。

---

## 0. 结论速览

**T3 是什么**：一个「控制平面」。执行永远发生在持有工作区的 **environment（server）** 上，Web/桌面/手机只是
认过证的客户端；server 用**事件溯源**（Command → Event → Projection → Outbox → EffectWorker）串行化所有状态变更，
用**隐藏 git ref 检查点**做 diff 与回退，用**一套 scoped session / DPoP / MCP OAuth** 划清信任边界。
它的能力密度集中在：**远程与多端、源码控制全家桶、编排自动化（workflow / 定时 / webhook / 子代理）、权限与企业治理、性能纪律**。

**Marionette 是什么**：一个「本地驻留的壳 + 模型互通层」。它是 Windows 单机、Tauri 桌面应用，
不承载 server、无多端；能力密度集中在：**跨 agent 的模型/key/订阅互通（引擎）**、**贴边常驻形态**、
**内置浏览器 + 电脑控制（agent 可操作原生 GUI）**、**回合轨迹呈现（活动时间图/思考预览）**、
**Telegram 远程控制**、**圆桌多模型审议**、**员工模式/AI 派活**。

**三条最重要的判断**

1. **T3 与 Marionette 不是同类替代品，是互补的两个轴**。T3 是「把 agent 放到远端、多端控制」；
   Marionette 是「把任何模型塞进任何 agent、在桌面上常驻」。因此 T3 的**云端/多端/团队治理**类能力对
   Marionette 基本不可移植（也不该移植），而 T3 的**工程骨架与纪律**恰恰是 Marionette 最缺的。
2. **T3 的价值不在功能条数，在四条红线**：①能力必须是「服务方法」，能被 WS / MCP / 定时任务 / CLI
   四个入口同时触达；②副作用只在事务后以 outbox 执行，失败不重放；③能力声明必须诚实，绝不模拟 provider
   没有的权限档；④线缆有预算，原始工具体不进快照。这四条**直接命中 Marionette 已经踩过的坑**
   （前端 `performSend` 与后端 `chief/runner.rs` 两套发送管线必须手工对齐）。
3. **Marionette 有三个 T3 完全没有的护城河**，审计时不要为了「对齐 T3」把它们稀释掉：
   跨用引擎（任何来源的模型喂给任何 agent）、贴边驻留 + 保持会话进程存活、Computer Use（操作桌面原生应用）。

**先做的一件事**：把「发送 / 回合生命周期」抽成**唯一一份可复用服务**（第 5 节 A1）。
这是 T3 全篇最便宜、回报最高的一条，也是 Marionette 目前技术债最集中的一处。

| 桶 | 条数 | 一句话 |
|---|---|---|
| **立即采纳（adopt now）** | 9 | 骨架级、低风险、收益立刻可见 |
| **改造后吸收（adapt）** | 12 | 方向对，但要按 Windows 单机 + Tauri 重构 |
| **暂缓（defer）** | 8 | 现在做成本 > 收益，条件成熟再评估 |
| **明确拒绝（reject）** | 10 | 与 Marionette 产品边界冲突，做了就是错 |

---

### 0.1 本次并行探测直接暴露的问题：子代理在工作，但会话页看不出来

- **事实**：四个 OpenCode Go / DeepSeek V4.1 Flash 派活 worker 都真的跑了——它们的 session JSONL 里
  各有 **127 / 492 / 84 / 89** 条 `tool_call` 记录，thought / assistant 流有数万到 15 万+ 字符，
  文件也确实被改动了。
- **但在 Marionette 里打开对应的子对话，只看到大约「1 条 thought」**，没有任何可用的工具调用或
  回复进度。
- **判断**：这是**派活 worker 的可观测性缺口**，不是「worker 什么都没干」的证据。
- **因此列为 Wave 0 前置项**：在扩大并行派活之前，worker 对话里必须先能看到实时工具调用、正文进展、
  当前步骤和最终摘要。验证以**会话记录回放（replay）**为准——拿 JSONL 里已有的事件重放一遍，
  确认子对话能把它们全部渲染出来，而不是靠抽查界面。

## 1. T3 功能盘点（按用户工作流，不按包）

> 组织原则：以「用户此刻想干什么」为索引。每条后面跟 `[文档]`/`[代码]` 出处，完整路径见第 7 节。

### 1.1 安装、首启、要不要先配 agent

- 四条安装路径：桌面安装包（brew / winget / scoop / AUR / Nix / .deb / AppImage）、`t3` CLI
  (`curl … install.sh` / `irm … install.ps1` / `npx t3@latest`)、手机 App、源码构建。
  **Intel Mac 没有独立 CLI 二进制**（SEA 不支持），只能桌面版或源码。[文档: `docs/user/install.md`]
- 首启页（Welcome）：先检测本机 agent，**只要一个可用 agent 就能开始**；支持从 Claude Code / Codex / Pi
  的会话目录**导入既有对话**（尽力而为：最多 200 条消息、跳过超大/损坏会话、每轮最多 100 个会话/4 GiB）。
  [文档: `docs/user/welcome-page.md`]
- provider 安装与登录**永远发生在 environment 那台机器**；桌面版可以内置管理 Codex / Antigravity 运行时并
  直接从 UI 完成 ChatGPT / Google 登录。[文档: `docs/user/install.md#providers`]
- 每个 provider 有独立的探活超时环境变量（`T3CODE_<PROVIDER>_PROBE_TIMEOUT_MS`），因为大目录下发现可能 9–14 秒。
  [文档: `docs/user/install.md#provider-check-timeouts`]

### 1.2 日常对话（thread / composer / 上下文）

- **thread 是耐久实体**，provider 进程退出不影响它；可选 worktree、可选 no-project（scratch，落在
  `~/.t3/scratch/<date>-<slug>-<id>`，可「Turn into project」原地升级）。
  [文档: `docs/user/thread-sidebar.md`]
- **三种启动形态**：普通、`Cmd/Ctrl+Enter` 后台启动（提交后立刻开下一个草稿）、**Shift 多选模型** = 同一 prompt
  给多个模型各开一个 thread + worktree。[文档: `docs/user/thread-sidebar.md#start-in-the-background-and-multi-model-runs`]
- composer 能力矩阵：120,000 字符上限；32 KiB 以上粘贴自动转文本附件（`Cmd/Ctrl+Shift+V` 可强制内联）；
  100 个附件；图片 10 MiB/张、80 MiB/条；HEIC 转 JPEG；实时 Markdown 渲染并可关；
  `#` 引 PR、`@` 引 thread/模型、`$` 引 skill、`/` 引命令；
  **queue vs steer 双模**（默认行为可设），`Cmd/Ctrl+Shift+Enter` 把队列首条改成立即 steer，
  `Alt+Up` 编辑最近一条队列消息；离线队列（移动端）。
  [文档: `docs/user/composer.md`]
- **prompt stash**（`Cmd/Ctrl+S` 暂存草稿与附件，最多 100 条，永不自动删除）、
  **recall**（空输入框 `↑` 回溯历史 prompt）、**Cite in composer**（引用助手回复并可加批注，计入长度上限）、
  **Edit from here**（回到某条消息之前，可选「同时回滚文件」，仅 worktree 且无人共用目录时可用）。
  [文档: `docs/user/composer.md`]
- **side chat**：`/side` 或 `/btw` 起一个带父上下文的旁支，父线程继续跑；结果可「Add to chat」或 `@` 提回父线程；
  关闭即删，删/归档父线程连带删/归档。[文档: `docs/user/thread-sidebar.md#side-chats`]
- **`/goal`**（Codex / Claude / OpenCode 2.2.5+）：跨轮保持目标，`pause`/`resume`/`clear` 语义按 provider 不同。
  [文档: `docs/user/composer.md#goals`]
- `/compact`、`/clear`（同一 thread 内开新 provider 会话，历史与 diff 仍可见，插一条 New session 分隔）。
  [文档: `docs/user/composer.md#commands-and-skills`]
- **额度受限恢复**：显示耗尽窗口与重置时间，可「Resume at reset」或「Snooze until reset」，或设为默认。
  [文档: `docs/user/thread-sidebar.md#usage-limits-subagents-and-thread-details`]
- **上下文透明度**：thread details 可显示 context window 用量；Claude 有**空闲压缩三档**
  （回来时提示 / 缓存过期前自动 / 永不）。[文档: `docs/user/providers-claude.md`]
- **建议下一条 prompt**（Labs）：Claude/Codex 回合结束后在空输入框里给一条预测，`Tab` 接受、`Esc` 忽略，
  每次消耗一次缓存请求。[文档: `docs/user/composer.md#suggested-next-prompt`]

### 1.3 权限、审批、以及「把话说清楚」

- 四档运行时模式（代码字面量）：`approval-required`（Supervised）/ `auto-accept-edits` /
  `auto`（provider 自带自动审查）/ `full-access`。默认**Full access**，可按项目覆盖，
  企业策略可用 `maxRuntimeMode` 封顶。[文档: `docs/user/permission-modes.md`；代码: `packages/contracts/src/auth.ts`]
- **provider 差异被如实写出来**（不是一律假装支持）：Grok/fx 无 Auto-accept；Muse/Kiro 只有两档；
  Antigravity 在 Full access 下仍可能弹原生审批；ACP 走「按模式代答」。
  [文档: `docs/user/permission-modes.md#provider-differences`]
- **push hooks 的「人批准」机制**（低于 Full access 时）：内置 PR 工具 push 前会执行本地 git hooks，
  T3 会按**hooks 内容摘要**请求一次性批准（`pre-push`、`reference-transaction`、`hook.<name>.command`），
  拒绝就不 push；无法展示请求的 provider 直接关 hooks 推送并告知 agent。
  **digest 只覆盖被点名的文件**，hook 里再 source 出去的脚本不在内。
  [文档: `docs/internals/04_environment-auth.md`；`docs/user/permission-modes.md#git-push-hooks-below-full-access`]
- **问答/审批落进对话**：审批、问题、secret 请求都有卡片；审批请求可被「打字回复」就地转为带反馈的拒绝（Claude）。
  [文档: `docs/user/activity-log.md`、`docs/user/providers-claude.md`]
- **问答附件**：允许自由文本的问题可同时附文件/图片（100 个，跨一次问答共享配额），agent 拿到的是落盘路径。
  [文档: `docs/user/question-attachments.md`]

### 1.4 代码、差异、交付

- **每个 thread 可独占 git worktree**（隔离分支/目录，多 agent 并行不打架）；共享 checkout 时**禁止切分支**。
  [文档: `docs/user/thread-sidebar.md`]
- **Diff 面板四个来源**：Turn changes（只含本轮编辑/提交/冲突解决的，排除 `pull`/`merge`/`rebase` 带进来的）、
  Uncommitted、Changes（对 base 分支）、Commits（最近 100 个本地提交）；支持 `mod+f` 全量跨文件查找并自动展开。
  [文档: `docs/user/diffs.md`]
- **workspace 文件浏览/编辑**：语法高亮、Markdown/HTML/CSV/TSV 双视图、PDF 预览、ZIP 浏览（≤50 MB）、
  workspace 外只读、远程编辑需 `filesystem:write`。[文档: `docs/user/diffs.md#workspace-files`]
- **行级评论**：任何 diff/文件里选行留评论 → 变成 composer 的内联上下文芯片，可跨多文件攒成一条消息发出。
  [文档: `docs/user/diffs.md#inline-review-comments`]
- **检查点**：用**隐藏 git ref** 保存工作区快照（不往用户分支加提交）；回退必须先把工作区与 provider 会话对齐，
  provider 不能回滚会话就**先拒绝，绝不动文件**。[文档: `docs/internals/01_architecture.md`；代码: `apps/server/src/checkpointing/`]
- **回合结算与检查点分离**：`run.completed` 在 provider 回合结束时发生，**turn-end 检查点是之后一个 outbox effect**；
  在它提交前，读者把缺失的检查点当 pending，命令决策仍看到 `waiting`，于是 steer / 排队 / 回滚 / 恢复都排在它后面。
  [文档: `docs/internals/01_architecture.md#turn-completion-and-checkpoints`]
- **多 repo 项目**：自动发现（一层 + 非 repo 目录再深一层）、可加外部目录（≤128）、`ignoreRepositories`、
  submodule 自动 checkout、per-repo 的 commit/push/PR、per-repo worktree、`worktreeLayout` 自定义脚本
  （`T3_WORKTREE_*` 环境变量、10 分钟超时、串行化、幂等、失败回滚）。
  [文档: `docs/user/multi-repo-projects.md`]

### 1.5 编排与自动化（T3 最「重」的一段）

- **两类子代理**：provider 原生（Claude/Codex/Cursor/Antigravity/fx/Pi，只读、审批冒泡到父线程）与
  **T3 派发**（`delegate_task`，可选别的 provider/model、独立 worktree、甚至**另一台已链接的机器**）。
  规则：子代理**永不越过父线程权限档**；不能 watch PR；Stop 父线程连带停子代理。
  [文档: `docs/user/subagents.md`]
- **定时任务**：三种目的地（每次新开 thread / 同一 thread 继续 / 同一 thread 但每次新 provider 会话）；
  固定时间用目标环境时区，`Once` 用本地时区；播 webhook 需 T3 Connect。[文档: `docs/user/project-settings.md#scheduled-tasks-and-webhooks`]
- **Webhook 自动化**：`{{body.path}}`/`{{headers.name}}`/`{{query.name}}` 占位符，
  值被包在 `<webhook-data-…>` 标签里并把 `$`/`@`/行首 `/` 中和（防注入成 skill/命令）；
  可选 HMAC 校验、过滤器、去重键（7 天）、合并窗口（10–3600 s，60/min 限流）、
  离线暂存（T3 Connect Durable Object，24 h）、Deliveries 调试面板。
  [文档: `docs/user/project-settings.md#webhook-automations`]
- **Workflows（Labs）**：放在 `~/.t3/workflows/` 的**耐久 TypeScript 脚本**，`T3`/`Effect`/`Schema`/`Option`
  是全局；`workflow()`/`step()`/`ask()`/`stop()`/`title()`/`tags()`/`subject()`/`within()`（并发池）；
  每步 checkpoint，重启续跑；版本化（旧版本带未完 run 时保留）；两种触发器（`every`/`daily`、webhook、
  issue/PR 事件）；失败 code 步指数退避（5/20/80 min）、agent 步暂停等人工 resume；CLI 全套
  （`t3 workflow start|list|show|resume|cancel|questions|signal|withdraw|hooks|store`）。
  [文档: `docs/user/workflows.md`；代码: `apps/server/src/workflows`、`apps/server/src/scheduling`]
- **MCP 里程碑事件**：外部 bot 可轮询/流/订阅（HTTPS webhook，≤1 h 租约，最近 1000 条内存缓冲），
  覆盖 turn / approval / guest / PR watch / workflow 事件。[文档: `docs/user/outside-agents.md`；代码: `apps/server/src/milestones`]

### 1.6 工作区工具（桌面/远程侧的「手」）

- **内置浏览器**：桌面本地 tab 原生跑；Web/移动/远程则从宿主 headless Chromium 推流（首次下载 ~120 MB，与
  HTML 渲染共享）；桌面打开远程环境的 tab 可本地渲染但把 `localhost` 经 **preview tunnel** 转发。
  profiles（可导入 Chrome/Safari/Firefox/Zen/Helium 的 cookie，Windows 上 Chromium 系受 app-bound 加密限制）、
  视口/缩放/外观、**元素拾取 + 区域批注 → composer 芯片**、**录屏**（帧率、按键/鼠标提示）。
  [文档: `docs/user/browser.md`、`docs/user/browser-import.md`]
- **agent 浏览器工具**：`preview_*`（open/act/snapshot/…），可关（按环境或项目覆盖），
  agent 开的 tab 默认**浮在对话上**，人要先「Take control」才能接管。
  [文档: `docs/user/browser.md#agent-browser-access`；代码: `apps/server/src/preview/`、`mcp/toolkits/preview*`]
- **设备（Device）**：iOS 模拟器、Android 模拟器、真机 Android、SSH 设备宿主、Simlock 池（alpha）；
  实时画面 + 点击/拖拽/输入 + 工具抽屉（暗色模式、字号、无障碍覆盖、mock 定位、权限、推送、折叠屏）；
  agent 通过 `device_*` 驱动。[文档: `docs/user/devices.md`；代码: `apps/server/src/device/`]
- **终端**：宿主机器上的集成终端，滚动缓冲 5,000 行 / 8 MiB，`T3CODE_TERMINAL_SHELL` 覆盖 shell。
  [文档: `docs/user/terminal.md`]
- **语音输入**：四类落点（本机 Apple Speech / 连着的那台环境本地跑 / 云 STT / 移动端离线），
  模型下载/复用（Handy、HF 缓存）、语言与翻译、个人与项目词典、静音过滤、口头语清理、
  cleanup 走文本生成模型、麦克风优先级、`mod+shift+d`、最长 5 分钟。
  [文档: `docs/user/voice-input.md`；代码: `apps/server/src/speech`]
- **SnapShots**：截任意窗口 + 应用名/标题/**无障碍文本** → 直接进草稿；双击两个 Shift 触发；
  macOS 需屏幕录制权限，Linux Wayland 有 GNOME/KDE/Hyprland/Niri 各自的 helper。
  [文档: `docs/user/snap-shot.md`]
- **可视化回复与 MCP Apps**：agent 写自包含 HTML（图表/示意图/表格/collage）→ 沙箱渲染在回复上方，
  发布前用内置 Chromium 自检；MCP Apps（ext-apps 标准）在工具调用下方渲染交互 UI，
  非只读操作会先问。[文档: `docs/user/html-renders.md`；代码: `apps/server/src/htmlRender`、`mcpApps`]
- **prompt links**：`/new?prompt=&repo=&provider=&model=&pr=`，**永不自动发送、不改权限档/分支/worktree**；
  剥掉零宽与隐藏 tag 字符并告警。[文档: `docs/user/prompt-links.md`]

### 1.7 远程、多端、多机

- **T3 Connect**（推荐）：Clerk 身份；relay 只协调链接、发 DPoP 绑定的 bootstrap 凭证，
  **HTTP/WS 数据不经 relay**（唯一例外是 webhook 转发的 per-link Durable Object）。
  支持「新设备需批准」（10 分钟码）、撤销设备、免费 3 台/Pro 10 台/Premier 100 台。
  [文档: `docs/user/remote-access.md`、`docs/internals/06_t3-connect.md`]
- **LAN / Tailscale / SSH**：`t3 up --lan`、`t3 pair`（QR/`t3code://pair`）、Tailscale HTTPS
  （`--tailscale-serve`）、桌面版托管 SSH（自动下发运行时到 `~/.t3/runtime`、端口转发）。
  一个 host 可有多条路由，自动学 LAN/Tailscale、择优连接、回优先级。
  [文档: `docs/user/remote-access.md`]
- **多机负载均衡**（客户端设置）：新 thread 在共享项目里自动选机器，权重 Prefer/Normal/Less often/Manual only。
- **Linked environments（beta）**：A 机 agent 可以 launch/message/delegate 到 B 机；单向、可双向、不链式、
  30 天不活跃过期；**低于 Full access 时被 `linkOrigin` 围栏**（只能改自己起的 thread，不能碰项目/设置/定时任务/再链接）。
  [文档: `docs/user/remote-access.md#link-another-environment`；代码: `apps/server/src/peer/`、`mcp/linkOrigin.ts`]
- **Guest links**：把**单条 thread** 只读分享给一个人（无账号，24 h，绑一个浏览器）；
  guest 发的消息以 **Request** 形式排队，由你批准才跑；可见范围=消息/文件/回复/推理/计划/问题/命令输出，
  不可见 diff/文件浏览/切模型/审批。token 是 **HMAC + 绑 projection 的 share**，不是 SessionStore 行，
  所以任何 RPC/WS/MCP 路由都不收它。[文档: `docs/user/guest-links.md`；`docs/internals/04_environment-auth.md`]
- **移动端**：iOS/Android 原生 App；推送（T3 Connect + 「Publish agent activity」）、
  iOS Live Activity / 小组件、Android Live Updates；前台时普通提醒静音。
  [文档: `docs/user/mobile-notifications.md`]

### 1.8 治理、运营、诊断

- **设置作用域四级继承**：项目覆盖 → 环境设置 → 仓库里的 `t3.json` → 内置默认；
  「All environments」是**对当前已连接 server 的批量编辑**，不是持久全局默认，也不是同步承诺。
  [文档: `docs/user/project-settings.md`、`docs/internals/01_architecture.md#settings-ownership`]
- **`t3.json`（checked-in 项目配置）**：`iconPath`、`defaultThreadEnvMode`、`worktreeSubmodules`、
  `worktreeSparsePatternsPath`、`ignoreRepositories`、`worktreeLayout`、`scripts`（≤50 个动作，
  `runOnWorktreeCreate` / `async` / `stopOnSettle` / `runOnSettle` / `previewUrl` / `autoOpenPreview`）。
  [文档: `docs/user/t3-json.md`]
- **Worktree 清理**：不活跃天数、结算后天数、PR 合并、已结算且无超出 base 的提交；
  「保留有本地改动的」三档（未提交 / 任意本地文件 / 仅已跟踪编辑）；手动 Delete now 并列出「保留/移除/孤儿」。
  [文档: `docs/user/project-settings.md#storage-cleanup`]
- **用量与额度**：跨环境聚合 8 家 provider 的 token/cost（API 等价，非订阅实付）；
  默认保留 92 天；自定义价格与模型映射（`Map to`）；**订阅额度池**按账号去重（多机同账号只算一次）；
  `Use reset` 兑换 Codex/Claude 重置额度；CLIProxyAPI hub；**自定义 HTTP 配额源**（JSON schema v1）。
  [文档: `docs/user/usage.md`]
- **托管策略（managed policy）**：JSON schema v1，macOS 配置描述文件 / Windows GPO+ADMX 或注册表 /
  Linux `/etc/t3code/policy.json`；`settings[].locked`、`limits.maxRuntimeMode`、`clients`、
  `connect`/`webhookTasks`/`browserCookieImport`/`previewTunnel`/`worktreeLayoutCommands`/`telemetry`、
  `providers.allowedDrivers`/`instances`、`updates.feedUrl`；**策略读不懂就拒绝启动**。
  [文档: `docs/user/managed-policy.md`；代码: `apps/server/src/policy`、`packages/contracts/src/managedPolicy.ts`]
- **机密**：1Password（CLI / service account / Connect）引用 `op://vault/item/field`；
  agent 可 `request_secret` 出**私有卡片**（值不进聊天记录；shell 场景写私有临时文件 ~24 h）。
  [文档: `docs/user/secrets*.md`；代码: `packages/secret-provider-*`]
- **诊断**：Maintenance 里看实时日志、trace span、CPU/内存历史、子进程并可发信号；
  **OTLP HTTP 导出**；`t3 trace summary`；`t3 doctor`（回收 SQLite 空间）；移动端最近 7 天启动崩溃。
  [文档: `docs/user/diagnostics.md`、`docs/user/telemetry.md`]
- **遥测边界**：server 独占投递与身份，客户端不加载浏览器 SDK；`distinct_id`=install id（随机 id 的哈希）；
  **身份永不自 provider 登录文件推导**；模型只报 catalog slug，其余 `custom:<driver>`；
  唯一例外是 opt-in 的 `agent.feedback`（先预览再发、逐条独立 id、不含 installId）。
  [文档: `docs/internals/08_product-analytics.md`]

### 1.9 扩展与集成

- **11 个内置 driver**：`acpRegistry` / `antigravity` / `claude` / `codex` / `cursor` / `fx` / `grok` /
  `kiro` / `opencode` / `pi` / `muse`（`apps/server/src/provider/builtInDrivers.ts`）。
- **通用 ACP 的纪律**（写进文档并被 review 强制）：非 first-party agent **严格照 ACP 规范**，
  不加 agent-id 判断、不加 vendor 扩展、不做 driver 特判；上游跑偏就让编排按 capability 降级，
  或让作者去上游提 issue（附最小协议 transcript + 规范章节）。[文档: `docs/internals/03_providers.md`]
- **model manifest**：`apps/server/src/provider/model-manifest.json` 支持离线启动，
  远程拉取可覆盖；**内置 manifest 按 `updatedAt` 压过缓存，所以每次编辑必须 bump**；
  破版下限写在 manifest policy 里，不写死在 driver 常量。
- **外部 agent 接入（反向 MCP）**：`https://<env>/mcp`，PKCE OAuth，无状态客户端注册；
  权限不是裸 scope，而是「只读 / 某个 runtime mode 上限」；`t3_thread_launch`/`wait`/`read` 等工具
  让外部 agent 也能驱动 thread；`t3 thread` 命令行在本机免 OAuth 直接驱动。
  **只有跑在 T3 内部的 thread 才能用 `delegate_task`/`preview_*`/`device_*`/`request_secret`。**
  [文档: `docs/user/outside-agents.md`；代码: `apps/server/src/mcp/`]
- **文本生成通道**：OpenAI / Anthropic / OpenRouter / 自定义 OpenAI 兼容端点，用于标题、分支名、
  提交信息、PR 文案、语音清理；`Test`/`Show models` 自检；受限策略下会被一并禁掉。
  [文档: `docs/user/text-generation.md`；代码: `apps/server/src/textGeneration`]
- **技能**：skill 来自 agent 自身（Claude `<CLAUDE_CONFIG_DIR>/skills`、项目 `.claude/skills`；
  Antigravity `.gemini/skills`→`.agents/skills`→`.agent/skills`；Grok/Pi/fx/Muse 各自的），
  T3 侧只提供 `skills/t3-workflows`（写 workflow 的 skill）。**T3 没有插件市场**。

### 1.10 T3 明确没有的（避免误判）

- **没有**内置终端之外的「IDE」：有文件编辑与 diff，但没有 LSP/多文件工程编辑。
- **没有**跨 agent 的模型共享/转发引擎；它明确「不转卖 token」，只驱动你已登录的 agent
  （文本生成 provider 是唯一例外，且只是 API key 直连）。
- **没有**多模型圆桌/审议（有「同题多开」和 `delegate_task`，但没有交叉质询与主席综合）。
- **没有**桌面原生应用操作（Computer Use）——浏览器是唯一的「手」。
- **没有**贴边驻留/悬浮球形态；关掉最后一个桌面窗口，本地 server 就停了（后台要用 `t3 service`）。
- **没有**插件注册表/一键装 MCP；外部 MCP 由 agent 自己配，T3 只注入自己的 MCP server。

---

## 2. 架构与主要子系统

### 2.1 所有权边界（谁拥有什么）

| 层 | 拥有 | 绝不拥有 |
|---|---|---|
| `apps/server` | provider 进程、终端、git、检查点、耐久状态、项目文件 | UI 状态、设备偏好 |
| `packages/client-runtime` | 连接管理 + 领域状态（web/desktop/mobile 共用） | 平台能力、渲染 |
| `packages/contracts` | 带类型的线缆边界（Effect/Schema） | 重运行时逻辑 |
| `packages/provider-*` | 各家协议与怪癖，藏在 adapter 后面 | 被编排层按名字特判 |
| `infra/relay` | 链接/凭证/隧道分配、webhook 转发 | 会话 token、业务数据 |
| `apps/desktop` | Electron 壳 + 打包 server runner | 绕过边界的本地文件系统假设 |

关键红线：**远程客户端永远不能拿自己的文件系统/provider 凭证/机器状态去替代 environment 的**；
桌面版内置 server，但 renderer 同样受这条约束。[`docs/internals/01_architecture.md`]

### 2.2 事件溯源编排（T3 的心脏）

- 名词：Command（改状态的请求）/ Event（持久事实）/ Projection（同事务提交的读模型）/
  Command receipt（幂等凭据）/ Outbox effect（与事件一起提交的副作用意图）/ Effect worker（提交后执行并把结果
  再喂回成 command）。[`docs/internals/00_glossary.md`]
- **命令决策不做 I/O**；外部 I/O 一律在 outbox 之后；**丢失进程的 effect 在恢复时退役而不是重放**。
  [`Orchestrator.ts` / `EventSink.ts` / `EffectWorker.ts`，见 `docs/internals/01_architecture.md`]
- **事件日志只保留有界重放窗口**，所以投影**不能**从日志全量重建；保留的事件必须跨版本可解码
  （schema 变更影响启动时的存量历史，不只是活的 RPC 流量）。
- 回合结算与检查点分离（见 1.4）。**客户端只渲染 server 的结算状态**，不自己按本地时钟或 PR 缓存推断。

### 2.3 Provider 适配层

- 身份：**App ID 为主，provider ID 只是引用**；按 **provider instance** 路由，不按 driver kind
  （同 driver 的两个账号不能共享可变状态；没配的 ambient 凭证要剥掉）。
- 「诚实能力」四条：只声明真支持的（fork/rollback/steering/subagent 缺就关，让编排干净降级）；
  绝不模拟权限档（不靠自动代答伪造一个 provider 没有的模式，未提供的存量模式回落 Supervised）；
  没有 proposed-plan 卡片就别露出 plan 模式；启动阶段（`startupPhase`）只能来自协议真信号，不能来自定时器。
- 健康检查必须**被动**：后台状态/模型探测绝不打开会跑 MCP/hook/登录流程的会话。
- 已知跨 adapter 陷阱清单：惰性配置广告、`"Default"` 是哨兵不是模型 ID、过早 Stop、
  工作活过回合（子代理/后台任务/续跑）、终端结算要收尾所有未闭合项、额度错误要分类成 `usage_limit`、
  符号链接 workspace。[`docs/internals/03_providers.md`]

### 2.4 授权与信任边界

- 会话是**scope 化的**，配对只能收窄不能放宽；`access:write` 才允许把凭证交回调用方，
  且要带上该凭证的每个 scope；撤销不受调用方 scope 限制。
- **DPoP** 绑证明密钥；无效证明必须失败而不是回落 bearer；T3 Connect/CLI/desktop 会话**不自续期**。
- **WebSocket ticket**：长寿命 token 绝不进 socket URL；每次 RPC 在 `RpcAuthorization.ts` 声明并校验 scope
  ——**握手成功不授予任何额外权限**。
- **Guest** 不是 session；**MCP client** 是另一个 audience（`/mcp` 之外全部拒绝），
  每个 MCP 工具必须在 `McpToolAccess.ts` 静态声明访问级别，且线程级工具在处理函数里仍要拒绝外部调用
  （从列表里藏起来只是「不被发现」，不是权限）。
- **实例是文件系统边界，项目不是**：`filesystem:read` 允许读服务器 OS 用户能读的任何文件；
  相对路径与写入仍受 `WorkspaceFileSystem.ts` 约束；签名资源 URL 是**单文件**的 bearer 凭证。
  [文档: `docs/internals/04_environment-auth.md`]

### 2.5 远程与中继

- 身份与路由分离：environment ID 跨重启/换端点不变；**路由是不可信提示**，只有连接的客户端能证明可达性；
  可分享端点不可用时**绝不回落 loopback**（loopback 指的是客户端自己的机器）。
- **peer link** = 一个 environment 的 agent 以 OAuth 客户端身份登录另一个 environment 的 `/mcp`；
  转发的调用带 `T3-Mode-Limit`，只能向下取交；非 Full access 被 `linkOrigin` 围栏；
  幂等键要按调用方命名空间哈希；客户端通过 `PeerThreadRelay` 读同一套 stream 格式（不维护第二套转录 schema）。
- **桌面预览隧道**：固定 SOCKS5（Chromium 对 loopback 忽略 PAC），只把 loopback 走隧道，失败就报错、
  绝不回落直连；Linux/macOS 上转发监听器只服务本 OS 用户拥有的连接（Windows 不检查）。
  [文档: `docs/internals/05_remote.md`]

### 2.6 T3 Connect relay

- Clerk 身份 + 一次性的、DPoP 绑定的 bootstrap 凭证（relay 从不见 session token）；
  双向认证（relay 证明 + 环境签名响应绑 nonce 与客户端证明密钥）。
- 明确写出**信任假设**：DPoP 保护诚实交换，不保护被攻破的 relay 签名密钥；
  `requireConnectDeviceApproval` 把信任从 relay 移到「已批准的 DPoP 指纹」。
- 隧道生命周期：allocation 每次变更 bump `generation`；unlink 先撤销授权再做外部拆解；
  回收按声称的 generation 加锁，避免延迟清理删掉并发重启复用的隧道。
  [文档: `docs/internals/06_t3-connect.md`]

### 2.7 客户端共享运行时

- **唯一重试所有者**是 supervisor：端点解析与开 session 都是单次尝试；瞬态失败用抖动退避，
  **只有连接稳定后才重置**；离线/鉴权失败等显式唤醒；前台化/网络变化只**探测**已建立的会话。
- 连接按 environment 分 scope：非自愿断线**保留注册与缓存投影**；显式移除才清凭证/缓存/草稿。
- **传输健康 ≠ 数据新鲜**：socket 开着不代表可用；缓存投影离线可读，但绝不能暗示实时，也不能覆盖更新的活数据。
- **shell 快照故意是部分的**（省略空闲子代理与较老的已结算 thread）——所以「shell 里没有」**不等于已删除**，
  要删数据/工作树必须先向 family 或 server 核实。
  [文档: `docs/internals/09_connection-runtime.md`]

### 2.8 性能与线缆预算

- CI 有 **thread payload 预算**（`transferBudgetV2.integration.test.ts`，opt-in 对象按 contracts 类型 `Required`，
  新增客户端 opt-in 必须被测到）。
- 四条线缆不变量：**有界窗口**（冷开/历史页/续连追赶都先按字节与行数封顶再解码）；
  **thread 快照不带原始工具体**（命令输出/动态工具结果/内联 file-change 体在边界就省掉，按需取）；
  **实时命令输出旁路**（adapter 把原始输出送到 `CommandOutputHub`，客户端只在展开命令行时订阅，
  绝不把 delta 灌进 `turn_item.updated`）；**shell 增量同步**。
  [文档: `docs/internals/16_performance-regressions.md`]

### 2.9 质量体系（这部分最值得抄）

- 测试规则：只测会坏的（编排/事件溯源、协议翻译、权限守卫、持久化与迁移、外部格式解析、竞态修复）；
  **要么扩展现有用例，要么不写**；**一层测一次**（最低层）；
  **禁止 sleep/轮询/墙钟断言**（`TestClock`、drain effect worker、等 `Deferred`）；
  browser 测试只在真需要浏览器时写；bug 修复的证明分两类（逻辑/数据→先写失败测试，纯视觉→前后截图）；
  `it.skip` 必须带 issue。[`docs/internals/15_testing.md`]
- **Scenarios**：`scripts/scenarios/*.e2e.ts` 驱动真 app（server + HTTP/WS + 浏览器/iOS 模拟器），
  用免费的 `Replay (dev)` agent；`scenarios:prove` 的退出码 0 只在「base 失败、head 通过」时给出。
- **Evals**：`evals/` 检的是「模型面对 T3 的文本时表现如何」——agent 指令、MCP 工具描述与 schema、生成的标题；
  改这些文本前先读 `evals/README.md` 并跑它点名的套件；修模型行为 bug 要加一条「修之前会失败」的用例。
- **Lint 即规则**：`t3code/no-manual-memo`、`no-unreasoned-state-sync`、`no-test-timeout-or-unreasoned-skip`、
  `no-frame-waits-in-browser-tests`、`no-wall-clock-in-tests`、`no-polling-loop-in-tests`、
  `require-disposable-render`、`no-untranslated-copy`、`no-low-value-comments`、`shadcn/no-restyle`、
  `no-layout-command-approval-writes`；React Compiler 覆盖率测试**编译每个文件，跳过即失败**。
- **i18n**：`gt-react` / `gt-react-native`，无 GT 云；`gt generate` 扫描 `<T>`/`gt()`/`msg()` 写出以
  **英文字符串哈希为 key** 的 catalog，翻译靠人手写进 JSON；同一个英文在 `gt()` 与 `<T>` 里是两个 key。
  [文档: `docs/internals/14_i18n.md`]

### 2.10 发布与更新

- 渠道 `stable` / `nightly` / `preview`（preview 只出 draft，**绝不发布**）；nightly 每 30 分钟一次且至少间隔 6 h。
- **launcher 拥有运行时**：systemd/launchd/Task Scheduler 起的稳定 launcher 是唯一的耐久服务状态写入者；
  server 子进程只通过继承的 IPC 请求更新。远程更新只替换 server 子进程 → 老 launcher 会落后，
  所以新 server 必须兼容上一版 launcher 协议（preflight 校验）。
- **更新事务**：先原子记录 pending（rename+fsync）→ 磁盘不够就拒绝 → 停旧进程 → 快照 SQLite（main/WAL/SHM，
  只一次，失败可回滚迁移）→ 新运行时 trial（迁移、绑定端口、持有长任务根）→ launcher 耐久提交后才放行。
- 客户端区分「提交」与「回滚」靠 update ID + 重连后的 ready 状态；桌面是两阶段交接。
- 崩溃恢复：退役丢失进程的 effect → 续跑意图进 outbox 等激活 → **每回合连续崩溃恢复有上限** →
  归档/已结算/已 snooze/用户停止/等待用户动作的回合**绝不自动续跑**。
  [文档: `docs/internals/07_server-updates.md`、`docs/operations/03_release.md`]

### 2.11 存储与检查点

- 生产库 SQLite（`statev2.sqlite`；V1 是 `state.sqlite`，只做一次拷贝迁移，之后互不同步）。
- 检查点用隐藏 git ref（代码里前缀 `refs/t3/…`），按 scope + 序号 + tool 组织；
  所以工作区快照不给用户分支加提交。[`apps/server/src/checkpointing/`]
- 启动恢复：事件序列游标与恢复候选必须**seek 索引**，不能扫全部保留历史；
  trace/日志必须异步串行写 + 轮转（同步 append 会在慢盘上卡住事件循环，Windows 尤其）。
  [`docs/internals/16_performance-regressions.md#persistence-and-background-work-invariants`]

---

## 3. 产品里程碑与演进（chronology，全部标注推断）

### 3.1 证据与推断方法

| 证据 | 可读出什么 | 置信度 |
|---|---|---|
| `docs/user/thread-migration.md`（V1 `state.sqlite` → V2 `statev2.sqlite`） | 存在一次大版本重写（编排 v2），且**不做**检查点/工具活动的迁移 | 高（文档明示） |
| `apps/server/src/orchestration-v2/` 与 `legacy/` 子目录并存 | 新编排层落地后旧层保留兜底 | 高（代码结构） |
| FAQ 里「Since v0.1.7，Claude 5 小时额度口径修正」 | 至少到 0.1.7 时产品已在大量用户手里跑 | 高（文档明示） |
| `apps/{desktop,server,web}/package.json` = 0.1.13 | 当前稳定线在 0.1.x 的第 13 个补丁附近 | 高 |
| `apps/mobile/eas.json` → `T3CODE_MOBILE_VERSION: "3.0.0"`、App Store id `6787819824` | 移动端已上架并有独立版本线（3.x），且**版本号与桌面/server 不同步** | 高 |
| 成熟度标签：Labs（workflows / guest links / linked environments / suggested prompt / agent feedback / fast worktree checkout / Simlock）、beta（Working section / group-by-project / Review guides / linked environments / fast worktree checkout）、alpha（Simlock）、preview（Secrets）、private beta（TanStack Source） | 功能是按「Labs → beta → 默认」推进的；这些就是「最近一年新增」的候选名单 | 中高（标签是文档事实，先后顺序是推断） |
| provider 版本下限：Antigravity **1.5.1+**（1.5.12 推荐）、Pi **0.84.3+** + Node **22.19+**、OpenCode **1.14.19+/2.0.18+**、fx **0.0.13+/0.0.14+**、Cursor Node **22.13+**、Codex 建议功能需 **0.165+**、Kiro **V3** | provider 集成持续跟进上游，且**新 agent 是逐家接入**（11 家） | 高（下限是文档事实） |
| `apps/server/src/provider/Drivers/AntigravityDriver.ts` + `AntigravityInstallation.ts` + `antigravityCliRelease.ts` | Antigravity 是「托管运行时 + 专属 driver」，不是通用 ACP 一个分支 | 高（代码） |
| `OpenCode2OrchestratorV2.integration.test.ts`、`providers-opencode.md`「1.x 有限支持 / 2.x 转换共享库」 | OpenCode 2 是一次外部协议迁移，T3 跟随 | 高 |
| `docs/user/providers-muse.md`（默认关闭） | 新 provider 默认关闭，先做可用再默认开 | 中高 |
| `AGENTS.md`「over 500,000 users」 | 规模声明 | **低（无法验证，见第 8 节）** |

### 3.2 推断时间线（粗粒度，只标「先后关系」，不标具体日期）

| # | 阶段 | 支撑证据 | 标注 |
|---|---|---|---|
| 1 | 单机 CLI + 桌面壳 + 一个/少数 provider，`state.sqlite` 编排 | V1 迁移文档描述「旧库只有 thread/消息」 | [推断] |
| 2 | **编排 v2 重写**：Command/Event/Projection/Outbox、隐藏 git ref 检查点、V1→V2 一次性迁移 | `thread-migration.md`、`orchestration-v2/` + `legacy/` | [推断：里程碑级] |
| 3 | **远程化**：WebSocket 层 + 配对 + LAN/Tailscale，随后 T3 Connect（Clerk、DPoP、托管隧道） | `05_remote.md`、`06_t3-connect.md`、`infra/relay` | [推断] |
| 4 | **多端**：Web（`app.t3.codes` 托管 + 本地 `t3`）、Electron 桌面（内置 server）、React Native 移动端（→ 3.0.0 上架） | `AGENTS.md`、`eas.json`、`apps/mobile` | [推断] |
| 5 | **源码控制全家桶**：worktree、检查点回退、commit/PR、issuance 多托管商（GitHub/GitLab/Forgejo/Gitea/Bitbucket/Azure/GitCafe/Cursor Origin/TanStack）、Linear、PR watching、Review guides | `docs/user/source-control.md`、`packages/source-control-*`（9 个）、`issue-tracker-linear` | [推断：按托管商逐个加] |
| 6 | **编排自动化**：子代理/`delegate_task` → 定时任务 → webhook → Workflows（Labs） | `subagents.md`、`project-settings.md`、`workflows.md`、`workflowsEnabled` 默认 false | [推断：Workflows 最新] |
| 7 | **工作区工具**：内置浏览器（→ 流媒体/元素拾取/录屏/cookie 导入）→ 设备 → 语音 → SnapShots → HTML 渲染/MCP Apps | `browser.md`、`devices.md`、`voice-input.md`、`snap-shot.md`、`html-renders.md` | [推断] |
| 8 | **治理**：托管策略（MDM/GPO）、机密（1Password）、用量/额度池、诊断/OTLP、产品分析 | `managed-policy.md`、`usage.md`、`diagnostics.md`、`08_product-analytics.md` | [推断：企业向，较晚] |
| 9 | **对外接口**：外部 agent 经 MCP 接入、`t3` CLI 全量命令、prompt links、guest links、peer links | `outside-agents.md`、`cli.md`、`guest-links.md`、`peer/` | [推断] |
| 10 | **性能与质量制度化**：payload 预算 CI、`CommandOutputHub`、perf 基准 A/B + A/A 噪声底、React Compiler 覆盖、evals、scenarios | `16_performance-regressions.md`、`evals/`、`scripts/metrics/*` | [推断：越靠后越像「补制度」] |

### 3.3 从演进里能提取的产品判断

1. **T3 把「执行」和「控制」彻底分开**，因此远程/多端/多机是自然结果而不是附加功能。Marionette 的结构里没有这条缝
   （Tauri 前后端一体、单机），所以 T3 的远程能力**无法**照搬——只能换形态（Telegram 已经是对的形态）。
2. **T3 每加一个 provider 都要写「差异表」**（权限档、压缩、fork、图片、额度）。它把「不支持的」也写进文档，
   这是它敢开 11 家的原因。Marionette 已经吃过反面的亏（桥接模型档位假成功、账号模型漂移静默降级），
   第 5 节 D5 就是把这条变成通则。
3. **T3 的重功能都是「先 Labs 再默认」**。Marionette 没有这个闸，新能力往往直接进主路径——这既是它的速度优势，
   也是它红线越来越多的原因（`AGENTS.md` 已近千行）。

---

## 4. 能力矩阵（T3 现状 vs Marionette 现状）

> 读法：「判断」列只表示**对 Marionette 的行动建议**，不代表 T3 做错了。
> 缩写：T3=`[T3 文档/代码]`，M=`[Marionette 文档/代码]`。

| # | 能力 | T3 现状 | Marionette 现状 | 判断 |
|---|---|---|---|---|
| 1 | 运行形态 | Node WS server + Electron/Web/RN 三端，执行全在 environment | Tauri 2 单机桌面（Windows），Rust 后端，无 server 概念 | **架构差异，不追** |
| 2 | 多端 | 桌面/浏览器/手机同一 server，同一 threads | 仅 Windows 桌面；远程=Telegram | reject（形态不同） |
| 3 | provider 数 | 11 个内置 driver + ACP registry 任意 | 内置 5 跨用 + pi + zcode + antigravity + 自定义 ACP | 已足够；**不追数量** |
| 4 | 会话/线程 | Thread=耐久对话；Session=provider 运行时，可停可续 | 会话绑定 agent；进程可挂起，历史续接 | 等价 |
| 5 | 并行工作区 | **每 thread 独立 git worktree**，共享 checkout 时禁切分支 | **无**（`worktree` 只用于快照机制） | **adapt（高价值）** |
| 6 | 同题多模型 | Shift 多选 → 每模型一个 thread + worktree | 无（圆桌是多模型审议，不是多开） | adapt（可选） |
| 7 | 队列/引导 | queue 与 steer 双模、队列可编辑/立即发 | 只有排队（`pendingSendsRef` → 回合结束 drain），无 steer | **adopt** |
| 8 | 编辑重发/回溯 | Edit from here：可只回对话或连文件；需 provider 回滚支持 | edit&resend 只截断本地转录 + 重发；**无 provider rewind 通道** | adopt+adapt |
| 9 | 检查点/文件回退 | 隐藏 ref、任意检查点、任意消息回溯 | 私有 index 快照 + **仅最后一轮**文件恢复（`restoreTurnFiles`） | **adapt（扩展，不是新建）** |
| 10 | 每轮交付 diff | Turn changes 只含本轮自己的编辑/提交/冲突解决 | 交付卡＝本轮内容树差异（含「起点脏只算被工具碰过的」规则） | **已对齐，甚至更严** |
| 11 | PR/Issue | 9 家托管商 + Linear + PR 审阅/合并/栈/自动合并/revert | 无 | reject（无 PR 路径） |
| 12 | PR watching / Review guide | 2 分钟轮询、多类唤醒、自动停止条件；Review guide（beta） | 无 | defer |
| 13 | 子代理 | provider 原生（只读）+ `delegate_task`（可跨机） | provider 原生 + `marionette_delegate_task`（同项目、并发 2、禁递归） | **已对齐** |
| 14 | 派活的可靠性 | 子代理按 subagent 路由、终端路径必须结算、后台工作报告 | 二至六期已做：回合结局分类、卡死解药、额度午休、回执跨重启、可回滚 | **Marionette 更深** |
| 15 | 多模型审议 | 无 | 圆桌（独立提案/交叉质询/主席综合/Plan Gate/预算三层） | **Marionette 独有** |
| 16 | 定时任务 | 三种目的地 + 时区 + Once + webhook | 无（只有 Telegram 与派活） | adapt（窄切片） |
| 17 | 工作流引擎 | 耐久 TS + 并发池 + 版本化 + CLI 全套（Labs） | 无 | defer |
| 18 | Webhook | 占位符净化 + HMAC + 过滤 + 去重 + 合并 + 离线暂存 | 无 | reject（需公网） |
| 19 | 权限档 | 4 档，provider 差异如实标注，可被策略封顶 | 6 档（只读/审批写/预授权写/…）+ per-project + 全局裁剪 | **Marionette 更细** |
| 20 | 审批卡/问答卡 | 审批+问题+secret 三类卡；问答附件；push-hook 批准 | 权限/Ask/Plan 卡 + 问答卡通道矩阵（Grok/elicitation，按 schema 推导） | **Marionette 更深** |
| 21 | 内置浏览器 | 本地/流媒体双模、profiles、视口、拾取、批注、录屏、cookie 导入 | WebView2 单机、profiles、拾取/框选批注、handoff、20 个 `browser_*` 工具、ref lease | **各有胜负**（T3 多「录屏/导入」；M 多 handoff/ref lease） |
| 22 | 浏览器远程 | 推流 + preview tunnel（固定 SOCKS5）+ 端口转发 | 无（本地 WebView2） | reject |
| 23 | 元素批注 | 有（→ composer 芯片） | 有（→ 预填 composer + 截图附件 + 不可信数据标注） | **已对齐** |
| 24 | 电脑控制 | 无 | Computer Use（cua-driver，12 个 `computer_*`，黑名单/按键白名单） | **Marionette 独有** |
| 25 | 设备仿真器 | iOS/Android/SSH/Simlock | 无 | defer |
| 26 | 终端 | 集成终端（5000 行/8 MiB，可换 shell） | 无用户终端（只有 ACP terminal 输出扫描） | **defer/adopt 待定** |
| 27 | 语音 | 本机/环境/云/移动离线 + 词典 + 清理 | 无 | defer |
| 28 | 截屏进输入框 | SnapShots（含无障碍文本） | 无（但 Computer Use 的截图/元素树已在手边） | **adopt（复用现成通道）** |
| 29 | 可视化回复 | 沙箱 HTML（含本地图内嵌）+ MCP Apps | ` ```spec ` 卡（card/table/steps/chart，零依赖，只预填不自动发） | adapt（可选，别丢安全边界） |
| 30 | 用量/额度 | 8 家聚合、92 天、价格映射、额度池去重、hub、HTTP 配额源 | v2 用量页（四桶/去向/热图/组成/回填）+ 引擎额度 | **各有胜负**；M 的「去向/组成」更细 |
| 31 | 跨 agent 模型共享 | **无**（明确不转卖 token） | **引擎**：任何来源的模型/key/订阅喂给任何 agent，含 fail-closed | **Marionette 独有** |
| 32 | 远程控制 | T3 Connect / LAN / Tailscale / SSH / peer link | Telegram（推送+内联按钮+总控面板） | 形态不同，**已覆盖** |
| 33 | 通知 | 桌面 + 移动推送 + Live Activity；按注意力 | 桌面（Rust 事件调度 + WAV + 任务栏）+ Telegram | **已对齐**（M 的后端提醒更抗冻） |
| 34 | 搜索 | 命令面板搜 title/branch/PR/project/**消息正文** + thread 内 find | 项目/会话按标题；对话内 Ctrl+F（已加载范围） | **adopt（全文搜索）** |
| 35 | 键盘/无障碍 | 可自定义 keybindings.json + `when` 条件 + F6 区域跳转 + 屏幕阅读器播报 + 窗口标题状态 | Ctrl+K/Ctrl+F/少量快捷键；无自定义、无播报 | **adopt（播报+标题最便宜）** |
| 36 | i18n | gt-react 哈希 catalog + lint 禁未翻译 | 自建 zh/en 词典 + `inventory-zh` 检查 | 已对齐 |
| 37 | 诊断 | Maintenance 面板 + OTLP + `t3 trace` + `t3 doctor` | Ctrl K 导出诊断报告（脱敏）+ `--doctor` + dev.log | **已对齐**（M 的导出更完整） |
| 38 | 自动更新 | 渠道 + launcher + trial 迁移 + 回滚快照 | app_update + 便携版按版本目录 + doctor | 已对齐（M 无 trial 迁移） |
| 39 | 主题 | 内置 + 自定义调色板/CSS/背景图 + 服务端发布主题 + CLI | 内置主题 + 跟随系统 | defer（自定义 CSS 与 ui-language 冲突） |
| 40 | 插件/技能 | 无插件市场；skill 来自 agent 自身 + `skills/t3-workflows` | **共通插件层**：装一次全 agent 生效（url 型注入 + stdio 子进程中继）+ 全局技能目录 | **Marionette 独有/更强** |
| 41 | 性能纪律 | payload 预算 CI、A/B 基准 + A/A 噪声底、渲染计数、启动剖析 | 有大量实测修复与红线，但**没有自动化预算** | **adopt（脚本化）** |
| 42 | 测试/评测 | 规则+lint+scenarios(e2e)+evals+React Compiler 覆盖 | `scripts/verify-*.mts` 纯函数回归（很强）+ 手工真机清单 | **adopt（evals/scenario/replay）** |
| 43 | 文档纪律 | internals 只写「代码读不出来的」，改动**重写不追加** | `AGENTS.md` 千行红线 + 手册/踩坑/计划齐备，但**只增不减** | **adapt（治理）** |
| 44 | 安全边界 | 4 层授权 + guest/peer/policy + 文件系统边界表述 | 引擎 Bearer 令牌、注入闸、授权=知情同意、fail-closed 发送 | 已对齐（不同威胁模型） |
| 45 | 企业/团队 | managed policy（MDM/GPO）、clients 白名单、审计面 | 无 | defer |
| 46 | 分发 | 多包管理器 + npm + 3 端 + 自更新 | 单 exe 便携 + install.ps1 + 内置 Rust 引擎 | 已对齐（M 更简单） |

**矩阵读数**：46 行里 **已对齐/更强约 15 行**，**Marionette 独有 6 行**，**值得吸收 12 行**，
**因形态冲突应拒绝 8 行**，**暂缓 5 行**。真正需要紧张的不是功能缺口，而是第 41–43 行的「制度缺口」。

---

## 5. 四桶决策

### 5.1 立即采纳（adopt now，9 条）

> 判据：不依赖新架构、不改产品形态、能在 1–2 周内落地并有可执行验收。

#### A1. 把「发送 / 回合生命周期」抽成唯一服务
- **为什么**：T3 的红线是「能力必须是服务方法，能被 WS/MCP/定时/CLI 同时触达；写在 handler 里的逻辑，
  其它入口就是缺的」。Marionette 现在有两条实现：前端 `src/app/App.tsx` 的 `performSend`（UI、Telegram 普通回复）
  与 `src-tauri/src/chief/runner.rs` 直接调 `AcpService`（员工/AI 派活 worker）。`AGENTS.md` 里那一串
  「权限档继承、模型重放、只读天花板、复活后重放设置」的镜像红线，全是这两条线手工对齐的代价。
- **切片**：新建 `src-tauri/src/turn_service.rs`（会话回合的启动/队列/中止/结算），先做成 `performSend` 的**薄封装**，
  再让 `delegation.rs`、`chief/runner.rs`、`telegram_remote.rs` 逐步改走它；`App.tsx` 保留 UI 状态但不再自己拼 ACP 调用。
- **验收**：`npx tsx scripts/verify-delegate-kit.mts`、`verify-chief-tee.mts`、`verify-agent-reentry.mts` 全绿；
  新增 `scripts/verify-turn-service.mts` 断言「同一 goal 经 UI / 派活 / Telegram 三条入口产生的回合事件序列一致」。
- **风险**：大重构。安全做法是**先只加转发层**，任何行为差异都先在转发层对齐，再删旧路径。

#### A2. 文本生成通道（标题 / 分支名 / 提交信息）
- **为什么**：T3 把所有「轻文本」任务从编码 agent 里拿出来，交给便宜的 text generator（OpenAI/Anthropic/
  OpenRouter/自定义 OpenAI 兼容）。Marionette 现在**没有自动标题、没有提交信息生成**（只有手改重命名），
  每开一个会话都少一条「一眼能认出来」的线索。
- **切片**：新增 `src-tauri/src/textgen.rs` 与 `~/.marionette/textgen.json`（baseURL+key+model，复用 `credentials.rs`
  的加密存放）；先落两个用途：会话标题、分支名。UI 在会话行菜单加「重新生成标题」；失败静默保留原标题。
  成本记账进现有 `usage_store`（新 source=`textgen`，别混进 agent 台账）。
- **验收**：断网/无 key 时不影响发消息；生成不阻塞首条消息；`verify-usage-ledger` 不回归。

#### A3. 转录全文搜索（跨会话）
- **为什么**：T3 的命令面板能按**消息正文**搜索（`messageSearchIndex.ts`）。Marionette 只按标题搜
  （`docs/CURRENT.md` 明写「不读正文」），而它本地就有全部 JSONL——这是纯粹的信息可得性损失。
- **切片**：`src-tauri/src/search_index.rs` + SQLite FTS5（或 Rust 侧 trigram 倒排）增量索引
  `~/.marionette/transcripts/*.jsonl`；写入侧挂在现有 `append_transcript_events` 之后（异步、可丢弃）；
  查询接进 Ctrl+K 面板（结果带会话名 + 命中片段 + 跳转定位，复用 `findSeenEventIndex` 那一套定位）。
- **验收**：索引可随时删档重建；单条 24 MB 转录不阻塞 UI；新增 `scripts/verify-search.mts`（分词/大小写/
  中文子串/删除会话后清理）。

#### A4. 破坏性操作 5 秒撤销
- **为什么**：T3 的 settle/snooze/archive/discard 都有 5 秒 Undo toast（`thread.undo`，`mod+z`）。
  Marionette 的「删除会话」「挂起会话」是**不可逆**的，且都在悬停才出现的三个按钮里——误点概率不低。
- **切片**：`storage.rs` 增加「延迟删除队列」（先移到 `~/.marionette/trash/<id>/`，5 秒后真删，
  窗口关闭/崩溃后下次启动清扫超期项）；`toast.ts` 加 undo 动作；`mod+z` 绑到最近一次可撤销动作。
- **验收**：删除后 5 秒内 `mod+z` 完整恢复（含转录、`.marionette` 分片、会话元数据）；超时后目录被回收；
  崩溃重启不会「复活」已超时的删除。

#### A5. 队列 → steer 双模
- **为什么**：T3 把「agent 正在跑时我又有话说」拆成 queue（排队）与 steer（插进当前回合），
  并允许编辑/立即发送队列首条。Marionette 只有排队（`pendingSendsRef` + 回合结束 drain），
  用户想纠偏只能先 Stop 再发——丢掉了已经跑出来的半截工作。
- **切片**：`Composer` 加「立即插入（steer）」按钮与 `mod+enter` 反动作；后端按 agent 能力
  （ACP `session/steer` 或 provider 特定的中断后同会话续问）决定真 steer 还是降级为队列并**如实提示**；
  队列条支持编辑/删除（现有 `queuedStrip` 扩展）。
- **验收**：能力不支持的 agent 上按钮变灰并写清原因（不假装成功）；steer 后回合不重启、历史不重复
  （`verify-turn-delivery`/re-entry 断言）。

#### A6. 抓窗进输入框（复用 Computer Use）
- **为什么**：T3 的 SnapShots 是「双击两个 Shift → 窗口截图 + 应用名 + 无障碍文本 → 进草稿」。
  Marionette 已经有 `computer_*` 的窗口列表、截图、元素树——这几乎是零成本的增量，
  但补上的是「我看到这个 bug，想立刻指给 agent 看」这条最高频的路径。
- **切片**：`src-tauri/src/computer/` 暴露 `capture_window_into_draft`；前端注册全局热键（默认双击两 Shift，
  可在 Ctrl K 改），拿到 PNG + `WindowInfo`（标题/进程）+ 可选元素树摘要 → `composerImageInjection` 预填。
  自己的窗口与黑名单照旧排除。
- **验收**：不共享窗口也能抓（这是用户自己的动作，不是 agent 的动作）；截图落 `~/.marionette/captures/` 并被
  现有图片附件管线接管；`computer` 的授权语义不变（agent 侧仍要卡）。

#### A7. 性能预算脚本（A/A 噪声底 + 渲染计数 + 启动剖析）
- **为什么**：T3 用「同一台机器交替 A/B + A/A 噪声底 + React 提交计数 + 启动剖析」把性能变成可判定的事实。
  Marionette 的性能史是一串「实锤事故」（隐藏面板 4fps、24 MB 转录全量重写、贴边 composer 高度、
  活动时间图重渲染），修得好但**没有防回归**。
- **切片**：`scripts/perf/` 三个脚本：①`perf-startup.mts`（冷启动到首次可输入）；②`perf-transcript.mts`
  （打开 1 MB/8 MB/24 MB 转录、滚动、流式追加的提交次数与耗时）；③`perf-dock.mts`（贴边展开/收回序列）。
  每个都输出机器可读 JSON 并支持 `--compare <a> <b>`；先只做「同一份构建跑两遍取噪声底」，不设阈值。
- **验收**：能在本机稳定复现「同一构建 A/A 差异 < X%」并记录；CI 或发布前手工跑一次；结果落
  `docs/manual/perf-baseline.md`（新增文件由本轮决定，不在本次审计范围内动手）。

#### A8. 模型面向文本的评测（rulebook / 工具描述 / 标题）
- **为什么**：T3 的 `evals/` 专门检「agent 指令、MCP 工具描述与 schema、生成的标题」在真模型下的表现，
  并要求「修模型行为 bug 时加一条修之前会失败的用例」。Marionette 的 `delegation::rulebook()`
  是**文字即代码**（AGENTS 明写「改一个字必须过测试」），但那个测试只断言「核心纪律还在」，
  不测「模型看完会不会照着做」。
- **切片**：`evals/` 目录 + 三个固定场景（用最便宜的可用模型）：①派活时是否把 goal 写成自包含；
  ②失败后是否重派而不是读摘要；③是否在派完就停而不是轮询。断言用结构化输出而不是自然语言匹配。
  失败不阻断合并，先只做「可观察」。
- **验收**：能给出「改动 rulebook 前后，三个场景的成功率变化」；跑一次的成本可预估（记录 token）。

#### A9. 工具按需检索（tool_search）
- **为什么**：T3 有 `toolSearch` toolkit，把「工具有很多」变成「先搜再调」。Marionette 每个会话都注入
  20 个 `browser_*` + 12 个 `computer_*` + 插件工具的完整 schema，这是持续的上下文税
  （`browser_help` 只解决了「怎么用」，没解决「列出来就占地方」）。
- **切片**：`tools_mcp.rs` 的 `tools/list` 改为**分层**：核心工具常驻，`browser_*`/`computer_*` 折叠进
  `marionette_tool_search(query)`，命中后再通过既有 `/mcp` 路由暴露完整 schema；`browser_help` 保持为唯一权威说明。
- **验收**：会话启动注入的 schema 字节数下降（记录前后）；`verify-browser-refs`/`cargo browser` 不退；
  模型在需要浏览器时能在 ≤2 次调用内找到工具（用一个固定场景手测）。

### 5.2 改造后吸收（adapt，12 条）

| # | 能力 | 改造要点（相对 T3） | 切片 | 验收 |
|---|---|---|---|---|
| D1 | **thread 级 worktree** | T3 是「每 thread 一个 worktree + 多 repo 布局 + 自定义脚本」；Marionette 只需**单仓、单 worktree、可选**。不做多 repo、不做 `worktreeLayout` 脚本（安全面太大） | `src-tauri/src/worktree.rs`（`git worktree add` 到 `~/.marionette/worktrees/<project>/<session>`）；会话创建时选「本目录 / 新 worktree」；删除会话时按「有无本地改动」提示；`doctor` 加孤儿 worktree 检查 | 两个会话并行改同一项目不互相污染；删会话不静默丢改动；`cargo test worktree` |
| D2 | **任意消息回溯 + 文件恢复** | T3 支持任意检查点序号且「provider 不能回滚就先拒绝」；Marionette 已有 `restoreTurnFiles`（仅最后一轮）。改造成按轮次记快照 + 允许选任意一条历史消息回溯 | `git_service` 增加 `snapshot_turn(ordinal)` 保留最近 N 轮树；`TurnDeliveryCard` 的 undo 扩成「回到这条消息之前」；若 agent 不支持会话回滚，只回文件并**明确写清「对话没回滚」** | 回退第 3 轮后第 4 轮文件消失、1–2 轮不动；提示文案与真实行为一致 |
| D3 | **有界窗口 / 快照不带原始工具体** | T3 是 wire 级；Marionette 是本地 JSONL，但要抄的是**形状**：列表/搜索/预览只取窗口，原始工具输出按需读 | 现有 tail 800 行机制保持；`ContextPanel` 图片与改动文件列表改为按需分页；`toolBody.ts` 的大输出默认只留摘要 + 路径 | 打开 24 MB 会话的内存占用与首次可交互时间有记录；`verify-turn-delivery` 不退 |
| D4 | **实时输出旁路（CommandOutputHub）** | T3 让原始命令输出不进事件流，只在展开时订阅。Marionette 现在把流式工具输出写进转录事件 | 增加会话级 `live_output` 通道（内存 + 环形缓冲），转录只落「开始/结束/摘要」；展开工具卡时前端订阅该通道 | 长命令不再每次增量都触碰 JSONL；崩溃后转录仍可读（摘要 + 末尾 N KB） |
| D5 | **能力诚实 / 不模拟权限档** | 把 T3 的四条「honest capabilities」写成 Marionette 的通则 + 测试：声称支持的档位必须真生效；档位不生效就写清并降级 | 在 `capabilityDrift.ts` 与 `delegation.rs` 各加一层「声明 vs 实测」断言；把 AGENTS 里散落的 fail-closed 条款收成一篇 `docs/guides/honest-capabilities.md`（新文件，另行排期） | 新增 `verify-honest-capabilities.mts`：桥接模型档位、账号模型漂移、派活档位继承三类场景都被钉住 |
| D6 | **会话三态与自动收尾** | T3：settle/snooze/archive + 7 天或 PR 合并自动结算 + 每条可豁免。Marionette 现在只有「挂起（进程级）」与「删除」 | 加「已收尾」这一**展示层**状态（不动进程语义）：7 天无活动、无未读、无后台任务 → 收进「已收尾」组；可手动豁免；不影响挂起 | 收尾只是折叠，不杀进程、不丢未读；筛选器与计数一致 |
| D7 | **项目动作（`t3.json` 类比）** | T3 允许仓库里 checked-in 的脚本与 worktree 生命周期挂钩。Marionette 已有项目概念与 `context_inventory`，但无「项目动作」 | `~/.marionette/projects/<id>/project.json`（**不进仓库**，避免 checked-in 脚本的执行风险）声明 `setup` / `dev` / `cleanup`；入口在项目行菜单；setup 只在新建 worktree（D1）后跑 | 动作输出进 ACP 终端或独立日志；失败不阻塞会话；可一键停 |
| D8 | **单线程定时发送** | T3 是三种目的地 + webhook + 时区。Marionette 只做「同一会话、按时间、重复」 | `src-tauri/src/scheduler.rs`（应用运行或贴边常驻时生效，关了就不跑并如实显示「下次需应用在运行」）；UI 在会话行菜单加「定时发送」 | 应用不在时不假装已发；错过的运行在启动后按策略补一次或跳过（明确文案）；`verify-scheduler.mts` |
| D9 | **无障碍（播报 + 窗口标题）** | T3 有三档播报 + 窗口标题前缀状态（Working / Needs approval / Needs your input） | 前端用 `aria-live` 播报「回合开始 / 需要你 / 完成」；`shell_integration.rs` 把状态写进窗口标题（现在只写产品名） | 播报不念推理文本；标题在贴边态不乱（窄条标题有自己的规则）；开关持久化 |
| D10 | **迁移前快照 + doctor 扩展** | T3：更新前快照 SQLite，trial 迁移失败可回滚。Marionette 是 JSON/JSONL，但 schema 迁移同样不可逆 | `storage.rs` 在 `schema_version` 变化时先把 `~/.marionette` 元数据（index/ledger/bindings/settings）打成一个 zip 到 `backups/`；`doctor` 报「最近备份时间 + 可回滚性」 | 迁移失败自动还原并留日志；备份不复制转录正文（体积不可控） |
| D11 | **文档治理（rewrite-not-append）** | T3 的 internals 规则：代码能读出来的不写；决定变了就**重写**那一段，不追加第二份说明 | 把 `AGENTS.md` 里「已失效的批次记录」压成「现行红线 + 指向代码/手册」，历史进 `docs/archive/` | 红线条目不随时间增长；每条红线都能指到代码或测试 |
| D12 | **沙箱化 HTML 视觉回复**（可选） | T3 用沙箱页 + 发布前自检 + 阻断私网；Marionette 的 `spec` 卡更安全但表达力窄 | 若做：只允许**纯静态 HTML + 内联资源**，渲染在无 `allow-same-origin` 的 iframe/独立 child webview，禁网络 | 恶意 HTML 不能读 localStorage/不能发请求；不改现有 spec 卡行为 |

### 5.3 暂缓（defer，8 条）

| # | 能力 | 为什么现在不做 | 重新评估的触发条件 |
|---|---|---|---|
| F1 | 工作流引擎 | 需要耐久执行器 + TS 沙箱 + 版本化 + 并发池 + 恢复语义；Marionette 是单进程桌面应用，进程一关就没了，与「耐久」冲突 | 出现「多阶段、跨天、需人工审批」的真实用户诉求，且接受「必须应用常驻」 |
| F2 | 设备仿真器 | Windows 上只能接 Android 模拟器/真机；iOS 需要 macOS。Marionette 用户画像里没有「手机开发」这一层 | 有真实用户在 Marionette 里做移动端开发并明确要求 |
| F3 | 语音输入 | 需要 STT 模型（体积）、麦克风权限、隐私口径；与「轻」的产品底线冲突 | 用户明确要求，且能走系统级 STT（不打包模型） |
| F4 | MCP Apps | 需要 WebView 内跑第三方 UI + 权限中介，安全面大；Marionette 的 spec 卡已覆盖「看图表」 | 出现「必须交互」的 MCP 工具（表单/画布）且用户愿意接受沙箱限制 |
| F5 | Prompt stash / Side chat | 都需要额外的「临时会话/草稿」生命周期与清理语义；Marionette 有 fork 与圆桌，够用 | 用户反馈「想先存着这句话」/「想问一句但不想脏历史」成为高频 |
| F6 | 多 repo 项目 | 需要仓库发现、per-repo diff/Git 动作、worktree 布局；先把 D1 单仓做扎实 | D1 上线并被日常使用后 |
| F7 | 企业策略（managed policy） | Marionette 是单用户桌面应用，没有「管理员 vs 用户」这条线；做早了只是自缚 | 出现团队部署/受管设备诉求 |
| F8 | 自定义主题 CSS / 背景图 | 与 `docs/guides/ui-language.md` 的设计系统冲突（自定义 CSS 会破坏 3px/1px/无阴影的语言） | 设计系统本身允许「皮肤层」时 |

### 5.4 明确拒绝（reject，10 条）

| # | 能力 | 拒绝理由（不是「做不了」，是「做了就错」） |
|---|---|---|
| R1 | T3 Connect / 自建 relay | 需要云端身份（Clerk）、托管隧道、DPoP 签发、Durable Object 暂存。Marionette 的产品底线是「不做云同步」；远程已经有 Telegram（纯出站、零新依赖） |
| R2 | Guest links | 依赖 relay 提供 hostname 与 HMAC 分享；且「把单条 thread 暴露给一个匿名浏览器」与本地桌面应用的安全叙事冲突 |
| R3 | Peer links / linked environments | 需要多 environment 身份、mode 天花板、`linkOrigin` 围栏、幂等键命名空间——一整套多机协议。Marionette 没有 environment 概念 |
| R4 | 移动端 App / 托管 web app | 需要三端共享 runtime + server 化。Telegram 已经是「手机控制」的正确形态（通知 + 按钮 + 总控面板 + 裸文本路由） |
| R5 | 产品遥测（云端 ingest）/ relay tracing | 与「本地、离线、可审计」的立场冲突；Marionette 的诊断报告是**用户主动导出**的，这个方向更好 |
| R6 | 多托管商 source control + PR/Issue 全家桶 | Marionette 不做 PR。它的交付语义是「本轮改了什么 + 可撤销」，不是「把改动发到远端评审」 |
| R7 | 远程浏览器流媒体 + preview tunnel | 需要服务端 Chromium 池、SOCKS5 隧道、媒体 ticket、本用户归属检查。Marionette 用本机 WebView2，天然没有这个问题 |
| R8 | 反向 MCP（外部 agent 控制 Marionette） | 等于给外部进程开一个「启动会话/发消息/读转录」的授权面。Marionette 的模型是「我把工具借给你（agent）」，不是「我把壳借给别人」 |
| R9 | 移植 Effect/TS 架构与三端共享 runtime | 技术栈不同（Rust + Tauri），照搬只会得到一层无用的抽象。抄**原则**（服务边界/outbox/预算），不抄**代码** |
| R10 | 会话跨机同步 / 负载均衡 | 没有第二台机器可均衡。Marionette 的多窗口是同一进程内的窗格，不是多 environment |

---

## 6. 推荐三波路线图与验收

> 排序原则：**先修骨架（A1）→ 再补高频体验（A5/A6/A3）→ 最后做结构（D1/D6）**。
> 每一波都可单独发布，且都能用现有 `verify-*.mts` / `cargo test` 卡住回归。

### Wave 1（2–3 周）「骨架与可见收益」

| 项 | 交付物 | 验收（可执行） |
|---|---|---|
| A1 回合服务化 | `src-tauri/src/turn_service.rs` + 三个调用点改道（先转发层） | `npx tsc --noEmit`；`node scripts/verify-all.mjs`；`cargo test --bin marionette`；新增 `verify-turn-service.mts` |
| A2 文本生成 | `textgen.rs` + 设置项 + 会话行「重新生成标题」 | 无 key/断网静默；标题生成不阻塞发送；usage 台账新增 source 不污染 agent 桶 |
| A4 撤销 | 延迟删除队列 + toast + `mod+z` | 5 秒内完整恢复；超时回收；崩溃不回魂 |
| A7 性能脚本 | `scripts/perf/*.mts` + 一份基线记录 | 同构建 A/A 差异可复现并被记录 |
| A8 评测 | `evals/` 三场景 | 能给出 rulebook 改动前后的成功率对比 |

**Wave 1 出口条件**：`npm run build` 通过；三条入口（UI / 派活 / Telegram）的回合事件序列一致；
删除可撤销；性能基线有数。

### Wave 2（3–5 周）「高频体验与上下文成本」

| 项 | 交付物 | 验收 |
|---|---|---|
| A5 steer | Composer 反动作 + 队列条编辑 | 不支持的 agent 按钮变灰并写原因；steer 不重启回合 |
| A6 抓窗 | 全局热键 + 截图进草稿 | 不需要共享窗口；走现有图片附件管线 |
| A3 搜索 | FTS 索引 + Ctrl K 结果 | 可重建、不阻塞、中文子串可命中 |
| A9 tool_search | `tools/list` 分层 | 注入字节数下降有数；≤2 次调用找到工具 |
| D4 输出旁路 | `live_output` 通道 | 长命令不触碰 JSONL；崩溃后摘要可读 |
| D2 回溯 | 按轮快照 + 回到某条消息 | 行为与文案一致；不支持回滚时明确说明 |
| D5 诚实能力 | 通则 + `verify-honest-capabilities.mts` | 三类场景钉死 |

**Wave 2 出口条件**：会话内「想纠偏」「想指给我看」「想找那句话」三条路径都 ≤2 次操作；
上下文注入成本有前后对比。

### Wave 3（5–8 周）「结构与治理」

| 项 | 交付物 | 验收 |
|---|---|---|
| D1 worktree | 单仓 worktree + 删除保护 + doctor 孤儿检查 | 两会话并行不互相污染；`cargo test worktree` |
| D6 三态 | 「已收尾」展示层 + 自动收尾规则 | 只折叠不杀进程；可豁免 |
| D7 项目动作 | `project.json` + setup/dev/cleanup | 失败不阻塞；可停 |
| D8 定时发送 | `scheduler.rs` + 会话菜单 | 应用不在时不假装已发 |
| D9 无障碍 | 播报 + 窗口标题状态 | 不念推理；贴边标题不乱 |
| D10 迁移快照 | `backups/` + doctor 展示 | 迁移失败自动还原 |
| D11 文档治理 | AGENTS 收敛 + 归档 | 红线条目不再增长 |
| D3 有界窗口 | 列表/图片/工具输出按需 | 24 MB 会话指标不退 |

**Wave 3 出口条件**：`node scripts/verify-all.mjs`、`cargo test --bin marionette`、
真机手册（贴边/分屏/派活/浏览器/Computer Use）全过；`docs/CURRENT.md` 只改行为段落，不追加批次史。

### 全局验收口径（每一波都要过）

1. `npx tsc --noEmit` 与 `npm run build` 干净。
2. `node scripts/verify-all.mjs` 全绿（含本轮新增脚本）。
3. `cargo test --bin marionette`（按现有 475+ 项口径）全绿；新增模块各自有测试。
4. 真机走一遍：普通窗口 / 贴边 / 分屏 / detached 四形态下，新功能都能用且不闪不卡。
5. 每个新能力都写「失败时用户看到什么」——**不允许静默成功**（这是 Marionette 已有 fail-closed 传统的延伸）。

---

## 7. 源码地图（T3 侧逐条，便于复核）

> 规则：每条断言至少一个路径。文档路径相对 `D:\Myself\t3code`。

### 7.1 产品与文档

| 主题 | 路径 |
|---|---|
| 用户指南索引 | `docs/user/README.md` |
| 文档总索引 / internals 阅读顺序 | `docs/README.md` |
| 安装与 provider 表 | `docs/user/install.md` |
| 首启与导入 | `docs/user/welcome-page.md` |
| thread 侧栏 / worktree / settle / snooze / side chat | `docs/user/thread-sidebar.md` |
| composer（queue/steer、stash、cite、goal、edit from here、附件上限） | `docs/user/composer.md` |
| 权限档与 provider 差异、push hooks | `docs/user/permission-modes.md` |
| diff / workspace 文件 / 行级评论 | `docs/user/diffs.md` |
| 托管商全家桶 / PR watching / stacks / issues | `docs/user/source-control.md` |
| 子代理两类 | `docs/user/subagents.md` |
| 定时任务 / webhook / 清理 / 继承 | `docs/user/project-settings.md` |
| `t3.json` schema | `docs/user/t3-json.md` |
| 多 repo 与 `worktreeLayout` | `docs/user/multi-repo-projects.md` |
| Workflows | `docs/user/workflows.md` |
| 浏览器 / 导入 / 默认浏览器 | `docs/user/browser.md`、`browser-import.md`、`default-browser.md` |
| 设备 | `docs/user/devices.md` |
| 终端 | `docs/user/terminal.md` |
| 语音 | `docs/user/voice-input.md` |
| SnapShots | `docs/user/snap-shot.md` |
| HTML 渲染 / MCP Apps | `docs/user/html-renders.md` |
| prompt links | `docs/user/prompt-links.md` |
| 远程接入 / 负载均衡 / 链接环境 | `docs/user/remote-access.md` |
| Connect 排障 | `docs/user/t3-connect-troubleshooting.md` |
| 移动通知 | `docs/user/mobile-notifications.md` |
| guest links | `docs/user/guest-links.md` |
| 外部 agent（MCP）/ `t3 thread` | `docs/user/outside-agents.md` |
| 机密（1P） | `docs/user/secrets.md`、`secrets-1password-service-account.md`、`secrets-1password-connect.md` |
| 后台服务 / `t3 doctor` | `docs/user/background-service.md` |
| 更新 | `docs/user/updating.md` |
| 用量与额度 | `docs/user/usage.md` |
| 诊断 / `t3 trace` | `docs/user/diagnostics.md` |
| CLI 全量命令 | `docs/user/cli.md` |
| 遥测 / 反馈 / OTLP | `docs/user/telemetry.md` |
| 托管策略 | `docs/user/managed-policy.md` |
| 迁移（V1→V2） | `docs/user/thread-migration.md` |
| 便携 handoff 预算 | `docs/user/portable-handoffs.md` |
| 问答附件 | `docs/user/question-attachments.md` |
| triage | `docs/user/triage.md` |
| provider 分册 | `docs/user/providers-{codex,claude,grok,antigravity,opencode,pi,muse,fx,kiro,acp}.md`、`docs/user/cursor.md` |
| FAQ（含 v0.1.7 口径） | `docs/user/faq.md` |

### 7.2 internals（架构断言）

| 主题 | 路径 |
|---|---|
| 术语表 | `docs/internals/00_glossary.md` |
| 所有权边界 / outbox / 检查点语义 | `docs/internals/01_architecture.md` |
| Effect 服务规则（能力=服务方法、handler 只做三件事） | `docs/internals/02_effect-services.md` |
| provider 纪律（诚实能力、被动健康检查、跨 adapter 陷阱） | `docs/internals/03_providers.md` |
| 授权（scope/DPoP/ticket/MCP/guest/push hooks digest/文件系统边界） | `docs/internals/04_environment-auth.md` |
| 远程（路由不可信、peer link、hosted web、preview tunnel） | `docs/internals/05_remote.md` |
| T3 Connect（bootstrap、双向认证、generation、webhook DO） | `docs/internals/06_t3-connect.md` |
| 服务更新（launcher、trial 迁移、崩溃恢复上限） | `docs/internals/07_server-updates.md` |
| 产品分析（install id、边界、投递） | `docs/internals/08_product-analytics.md` |
| 客户端运行时（supervisor、registry、部分 shell） | `docs/internals/09_connection-runtime.md` |
| Web UI（primitive、Tailwind、React Compiler、handler 闭包陷阱） | `docs/internals/10_web-ui.md` |
| 移动布局五层 | `docs/internals/11_mobile-layout.md` |
| 移动导航 / UIKit fork 约束 | `docs/internals/12_mobile-navigation.md` |
| 移动开发生命周期 | `docs/internals/13_mobile-development.md` |
| i18n（gt-react、哈希 key） | `docs/internals/14_i18n.md` |
| 测试规则（禁 sleep、scenarios） | `docs/internals/15_testing.md` |
| 性能（预算、线缆不变量、基准、移动动画） | `docs/internals/16_performance-regressions.md` |
| 脚本规则（effect/cli） | `docs/internals/17_scripts.md` |

### 7.3 代码锚点

| 断言 | 路径 |
|---|---|
| monorepo 结构 / 脚本 | `package.json`、`pnpm-workspace.yaml` |
| 11 个 driver | `apps/server/src/provider/builtInDrivers.ts` |
| Antigravity 专属 driver 与托管运行时 | `apps/server/src/provider/Drivers/AntigravityDriver.ts`、`.../AntigravityProvider.ts`、`.../AntigravityInstallation.ts`、`.../antigravityCliRelease.ts` |
| 编排（命令/事件/投影/outbox/worker） | `apps/server/src/orchestration-v2/Orchestrator.ts`、`EventSink.ts`、`EffectWorker.ts`、`EventStore.ts`、`ProjectionStore.ts` |
| 回合结算/收尾 | `apps/server/src/orchestration-v2/RunFinalizationService.ts`、`RunCompletionReads.test.ts` |
| 检查点 | `apps/server/src/checkpointing/`（`CheckpointStore.ts`、`CheckpointDiffQuery.ts`、`refs/t3/...`） |
| 回滚与还原安全 | `apps/server/src/orchestration-v2/CheckpointRollbackService.ts`、`CheckpointRestoreSafety.ts` |
| 上下文 handoff 预算 | `apps/server/src/orchestration-v2/ContextHandoffService.ts`、`ContextHandoffBudget.test.ts` |
| 权限模式常量 | `packages/contracts/src/auth.ts`、`providerPolicy.ts`、`managedPolicy.ts` |
| 授权中间件 | `apps/server/src/auth/RpcAuthorization.ts` |
| MCP 工具面 | `apps/server/src/mcp/toolkits/`（attachment/device/environment/feedback/html/issues/message/orchestrator/preview/previewControls/project/pullRequests/theme/thread/toolSearch/workflow/worktree） |
| MCP 访问级别声明 | `apps/server/src/mcp/McpToolAccess.ts` |
| 连接围栏 | `apps/server/src/mcp/linkOrigin.ts` |
| peer link | `apps/server/src/peer/PeerLinks.ts`、`PeerThreadRelay.ts` |
| guest share | `apps/server/src/guest/GuestShareService.ts`、`orchestration-v2/GuestShareDecider.ts` |
| Connect / relay 客户端 | `apps/server/src/cloud/CloudLink.ts`、`infra/relay/src/environments/EnvironmentConnector.ts`、`ManagedEndpointProvider.ts` |
| webhook 转发与暂存 | `infra/relay/src/hooks/HookForwarder.ts`、`HookInboxObject.ts` |
| 预览浏览器 / 隧道 | `apps/server/src/preview/`（`ServerBrowser.ts`、`PreviewTunnel.ts`、`PreviewBrowserHost.ts`、`PortScanner.ts`） |
| 设备 | `apps/server/src/device/` |
| 终端 | `apps/server/src/terminal/`（`NodePtyAdapter.ts`） |
| 语音 | `apps/server/src/speech/` |
| 用量 | `apps/server/src/usage/`；契约 `packages/contracts/src/usage.ts`、`providerUsageLimits.ts` |
| 定时任务 | `apps/server/src/scheduledTasks/`、`scheduling/` |
| workflows | `apps/server/src/workflows/`、契约 `packages/contracts/src/workflow.ts`、`skills/t3-workflows/` |
| 里程碑事件 | `apps/server/src/milestones/` |
| 文本生成 | `apps/server/src/textGeneration/` |
| 策略 | `apps/server/src/policy/`、`packages/contracts/src/managedPolicy.ts` |
| 机密 | `packages/secret-provider-core/`、`-definitions/`、`-onepassword/` |
| source control（多托管商） | `packages/source-control-*/`：core, definitions, github, gitlab, forgejo, bitbucket, azure-devops, gitcafe, cursor-origin, tanstack-source, testing |
| issue tracker | `packages/issue-tracker-linear/`、`apps/server/src/issue/` |
| PR 领域 | `apps/server/src/pullRequest/`、契约 `packages/contracts/src/pullRequest.ts`、`pullRequestReviewGuide.ts` |
| 线缆预算 | `apps/server/integration/transferBudgetV2.integration.test.ts`、`TransferBudgetReport.integration.ts` |
| 实时输出旁路 | `apps/server/src/orchestration-v2/CommandOutputHub.ts` |
| shell 增量同步 | `apps/server/src/orchestration-v2/ShellStream.ts`、`boundedSnapshotTransport.test.ts` |
| 性能基准 | `scripts/metrics/{core,server,web,mobile}`、`scripts/profile-startup.mjs` |
| 场景测试 | `scripts/scenarios/`、`scripts/prove-scenario.ts` |
| 评测 | `evals/{mcp-tool-use,pr-content,test-signal,thread-titles,voice-cleanup,workflow-authoring}` |
| lint 规则 | `oxlint-plugin-t3code/` |
| i18n catalog | `apps/web/src/i18n/translations/`、`apps/mobile/src/i18n/translations/`、`apps/desktop/src/i18n/translations/` |
| 发布流水线 | `.github/workflows/release.yml`、`docs/operations/03_release.md` |
| 桌面打包 | `scripts/build-desktop-artifact.ts` |
| 原生资源监控 | `native/resource-monitor/`（Cargo） |
| 契约总表 | `packages/contracts/src/`（`rpc.ts` 98 KB、`orchestrationV2.ts` 190 KB、`settings.ts` 99 KB 是三个最大的面） |

### 7.4 Marionette 侧对照锚点（便于实施时定位）

| 断言 | 路径 |
|---|---|
| 发送管线（现状双实现之一） | `src/app/App.tsx`（`performSend`）、`src-tauri/src/chief/runner.rs`（直调 `AcpService`） |
| 派活 | `src-tauri/src/delegation.rs`、`src/lib/delegateCards.ts`、`docs/plans/plan-agent-delegation.md` |
| 交付卡与快照 | `src-tauri/src/git_service.rs`（`begin/end_workspace_snapshot`）、`src/lib/turnDelivery.ts`、`src/components/TurnDeliveryCard.tsx`（`restoreTurnFiles`） |
| 转录存储 | `src-tauri/src/storage.rs`、`src/app/useSessionRails.ts` |
| 用量 | `src-tauri/src/usage_store.rs`、`src-tauri/src/usage_backfill/`、`src/lib/usageLedger.ts`、`src/lib/usageInsights.ts` |
| 引擎（跨用） | `src-tauri/src/engine_gateway/`、`src-tauri/src/engine.rs`、`src/lib/bridgeRouting.ts` |
| 浏览器 / 电脑控制 | `src-tauri/src/browser/`、`src-tauri/src/computer/`、`src-tauri/src/tools_mcp.rs` |
| 圆桌 | `src-tauri/src/council/`、`src/components/CouncilPage.tsx` |
| 贴边 | `src/lib/dockMode.ts`、`src/components/DockChrome.tsx` |
| Telegram | `src-tauri/src/telegram_remote.rs` |
| 插件层 | `src-tauri/src/plugins.rs`、`src-tauri/src/plugin_host.rs` |
| 诊断 | `src-tauri/src/diagnostics.rs`、`src-tauri/src/doctor.rs`、`docs/guides/diagnostics.md` |
| 回归脚本 | `scripts/verify-*.mts`（清单见 `scripts/README.md`） |

---

## 8. 未能验证的断言 / 文档缺口（诚实清单）

### 8.1 结构性限制（导致时间线只能是推断）

1. **T3 仓库是单提交快照**（`git log` 只有一条 2026-11-27，PR #44052），**无 tag、无 CHANGELOG、无发布说明归档**。
   第 3 节所有先后关系都来自「成熟度标签 + 版本下限 + 迁移文档」，不是提交历史。
2. 因此**无法**回答「某功能是哪天上线的」「两个功能谁先谁后」这类问题。若需要真实时间线，
   应改为读 GitHub Releases / npm dist-tags（本次任务禁止联网安装与建库，故未做）。
3. `AGENTS.md` 称「over 500,000 users」——**无法验证**，本报告不据此做任何推断。

### 8.2 只到文档层、未做行为验证的断言

| 断言 | 状态 |
|---|---|
| 浏览器首次下载「~120 MB headless Chromium」 | 文档数字，未测 |
| 「paint < 16 ms 记为 16 ms」「LLM 预算 16k/64k」等阈值 | 文档/常量引用，未运行 |
| guest link 的 HMAC 绑定与「任何路由都不收」 | 读了 `GuestShareService.ts`/`GuestShareDecider.ts` 的存在与文档描述，**未逐行审计密码学实现** |
| push hooks digest「只覆盖被点名文件」 | 文档明示，**未跑真实 push 验证** |
| T3 Connect relay 的生产部署（Cloudflare/Alchemy stage prod） | `infra/relay` 代码在，**部署状态未验证** |
| 移动端真机行为（Live Activity、widget、离线队列） | 读了 `apps/mobile` 与文档，未运行 |
| `refs/t3/...` 之外检查点的实际命名规则 | 只在测试里看到形状，未追到生产构造函数 |
| 各 provider「真实能力差异表」的准确性 | 文档声明 + driver 代码存在，**未逐家真机对照**（Marionette 自己在这方面踩过坑，这正是 D5 的动因） |
| T3 的 perf/eval 套件是否真在 CI 跑 | workflow 文件存在（`release.yml` 等），**未逐一核对 CI 触发条件**；`docs/internals/16` 说预算在 CI，未验证运行记录 |

### 8.3 Marionette 侧未验证的推断

| 推断 | 依据 | 风险 |
|---|---|---|
| 「edit&resend 只截断本地转录，没有 provider 级 rewind」 | `acp.rs` 与全仓 grep **无** `rewind`/`rollback` 会话通道；`storage.rs` 里的 truncate 都是文件写安全 | 可能通过「新会话 + 历史注入」实现，效果类似但不等于协议回滚；D2 实施前需确认 |
| 「Marionette 无用户终端」 | `package.json` 无 xterm 类依赖；`components/` 无 Terminal 组件（`ContextPanel` 里的 Terminal 是 dev server 文案） | 低 |
| 「Marionette 无 PR 集成」 | 全仓 grep `pull_request`/`gh pr` 零命中 | 低 |
| 「Marionette 无定时任务」 | 无 scheduler/cron 模块（只有 tick 保鲜） | 低 |
| 「Marionette 无跨会话全文搜索」 | `docs/CURRENT.md` 明写「按标题过滤，不读正文」；`conversationFind.ts` 是当前对话内查找 | 低 |

### 8.4 文档本身的缺口（T3 文档没写清的地方）

1. **没有产品路线图文档**：`AGENTS.md` 说「我们分享 roadmap」，但仓库里没有 roadmap 文件，roadmap 在 GitHub（未访问）。
2. **没有迁移矩阵**：V1→V2 之外没有「schema 版本 → 兼容窗口」的表格；`01_architecture.md` 只说「保留事件必须跨版本可解码」。
3. **`orchestration-v2/legacy/` 的存在没有文档解释**（是兜底？是迁移期残留？）——只能从目录名推断。
4. **`docs/internals/adding-a-provider.md` 只剩一个指针**（233 字节），真正的策略在 `03_providers.md`；
   对「如何在 11 家里加第 12 家」没有分步清单。
5. **Workflows 的沙箱边界没写**：脚本能 `Effect`/`Schema`，但「能不能读任意文件/起进程/发网络」文档未明确（只在 managed policy 提到 webhook 可禁）。
6. **`infra/task-cache` 没有任何文档**（只在 `package.json` 脚本里出现），无法判断它是产品功能还是构建缓存服务。

### 8.5 本报告的边界

- 不做代码质量评价、不做安全性审计结论（只复述文档宣称的边界）。
- 不建议 Marionette 引入 T3 的技术栈（Electron/Effect/React Native）；所有建议都以「Rust + Tauri + React」可实现为前提。
- 第 6 节的工期是**相对估算**（按单人熟悉代码库计），不是承诺。

---

## 附录 A · 术语对照

| T3 | Marionette | 备注 |
|---|---|---|
| Environment / Server | 本机 | M 没有 server 概念，一切在同一个进程树里 |
| Client（web/desktop/mobile） | 主窗 + detached 窗 + 贴边窄条 | 同进程多窗口，不是多端 |
| Project | 项目 | 近似；M 的项目不做多仓发现 |
| Thread | 会话 | 近似；M 的会话绑定 agent |
| Turn / Run | 回合 | 近似 |
| Session（provider runtime） | agent 进程 | M 的「挂起」= 结束进程，下次续接 |
| Checkpoint | 交付卡快照 / `restoreTurnFiles` | T3 用隐藏 git ref，M 用私有 index 内容树 |
| Runtime mode | 权限档（只读/审批写/预授权写…） | T3 四档，M 六档 + per-project |
| Interaction mode（plan） | Plan 卡 | M 的 Plan 进对话流 |
| Subagent | 子代理（厂商原生/派活） | 两边都有 |
| Settlement | 「已收尾」（本报告建议新增的展示层） | M 现在只有挂起/删除 |
| Outbox effect | — | M 没有；这是 T3 最值得抄的机制思想 |
| Effect worker | — | 同上 |
| Manifest | `engine-catalog.json` / agent registry | 形状不同，目的相近 |
| Guest link | — | M 用 Telegram 做「外部人看/说」 |

## 附录 B · 关键默认值与常量（T3，速查）

| 项 | 值 | 出处 |
|---|---|---|
| 消息长度上限 | 120,000 字符 | `docs/user/composer.md` |
| 附件 | 100 个；图片 10 MiB/张、80 MiB/条；其他 50 MiB | 同上 |
| 大粘贴转附件阈值 | 32 KiB | 同上 |
| prompt stash 上限 | 100 条 | 同上 |
| 终端滚动 | 5,000 行 / 8 MiB | `docs/user/terminal.md` |
| 语音录音上限 | 5 分钟 | `docs/user/voice-input.md` |
| 默认权限档 | `full-access` | `docs/user/permission-modes.md` |
| 默认用量保留 | 92 天 | `docs/user/usage.md` |
| 自动结算 | 7 天无活动 或 PR 合并/关闭 | `docs/user/thread-sidebar.md` |
| PR watch 轮询 | 2 分钟；连续 10 次纯评论唤醒 / 8 次读失败即停 | `docs/user/source-control.md` |
| handoff 预算 | 默认 16,000（1024–64000）；导入历史 64,000 字节 | `docs/user/portable-handoffs.md` |
| 模型窗口兜底 | 128,000；预留 ≥16,000 或 25% | 同上 |
| 多机链接过期 | 30 天不活跃 | `docs/user/remote-access.md` |
| guest link 有效期 | 24 小时、一个浏览器 | `docs/user/guest-links.md` |
| webhook 去重/合并 | 7 天 / 10–3600 s（默认 120 s）/ 60 次每分 | `docs/user/project-settings.md` |
| workflow 跳过保留 | 500 条（可配） | `docs/user/workflows.md` |
| 稳定渠道检查 | nightly 每 30 分钟、间隔 ≥6 h | `docs/operations/03_release.md` |
| Node / pnpm | 26.13.1 / 12.14.0 | 根 `package.json` |

---

*本文件由只读审计生成；除本文件外未修改任何仓库内容。*
