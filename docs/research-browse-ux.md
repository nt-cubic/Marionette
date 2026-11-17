# 调研：anomalyco/browse 的用户体验对标（2026-11-17）

- 参考仓库：<https://github.com/anomalyco/browse>
- 分析快照：dev 分支 a7181bf，2026-11-17 拉取；仓库版本仍是 0.1.0，接口和行为仍会变化
- Marionette 基线：feat/browser-annotation 分支 200e4bf5，包含元素批注及其安全、生命周期修复
- 研究重点：用户实际感受到的浏览器体验，不以“工具数量”排名
- 架构差异：browse 驱动用户现有的 Chrome 系浏览器；Marionette 驱动内置 WebView2 小浏览器

## TL;DR

browse 和 Marionette 最大的体验差异不是工具多少，而是对浏览器的定义不同：

- browse 把浏览器视为**用户自己的领地**，Agent 只是被允许进入的客人。客人有自己的标签组，工作时不抢当前标签，需要人时会明确亮起 WAIT，做完后应自己收拾。
- Marionette 目前更像**在应用里给 Agent 开了一块浏览器面板**。用户授权粒度更清楚，元素批注也更适合“指着页面告诉 Agent”，但 Agent 的标签、运行状态和“轮到你了”仍不够可见。

Marionette 最明显的五个 UX 缺口：

1. **Agent 打开页面会打断用户当前操作。** browse 默认在后台开标签；Marionette 收到新标签事件会把浏览器页顶到前台。
2. **缺少“Agent 暂停，等用户完成登录/2FA/支付”的正式流程。** browse 用页内卡片、标签 WAIT、通知和 Continue 形成闭环；Marionette 目前只有 Control 权限确认，没有 handoff。
3. **多 Agent 的标签归属不清楚。** browse 记录标签属于哪个会话、组名、最后使用时间和 busy 状态；Marionette 只有全局浏览器标签和 openedByAgent 布尔值。
4. **用户很难一眼判断 Agent 正在跑、等人、已完成还是出错。** browse 的 ON/RUN/WAIT、组展开、页面 pill 都是持续状态；Marionette 主要靠当前页、分享级别和工具结果推断。
5. **失败后缺少“下一步做什么”。** browse 会识别 challenge、给 warning/nextStep、解释无法分享的页面；Marionette 多数错误只回到 Agent 的工具结果，用户界面不一定接得住。
6. **浏览器和对话还不能同时工作。** 当前分屏树只接受会话；浏览器、文档、收纳盒都是互斥的中栏覆盖页。用户批注后想一边看页面一边和 Agent 对话，只能来回切。

Marionette 已有而 browse 没有同样完整体验的部分：

- 元素拾取、框选、逐条批注、自动裁剪截图、发送前预览；
- 不可信页面数据声明、URL 脱敏、多页面分组；
- 更细的 none/read/control 授权，以及 Agent 自开标签与用户标签的隐私边界；
- 与应用内对话、Composer 附件和会话恢复直接结合。

建议保留这些优势，把 browse 的“用户在场感”补进来，而不是把 browse 的扩展安装、配对和 relay 架构整套搬过来。

## 用户心理模型

| 问题 | browse 的回答 | Marionette 当前的回答 | 体验差距 |
|---|---|---|---|
| 这是谁的浏览器？ | 你的浏览器，Agent 是 guest | 应用里的 Agent 浏览器页 | browse 更容易建立“我能随时收回”的安全感 |
| Agent 在哪工作？ | 独立标签组、可选独立窗口 | 同一套应用内浏览器标签 | browse 更能隔离多任务，不打断用户 |
| 它现在在干什么？ | RUN 标签、页面 pill、组状态 | 当前页、工具轨迹、权限弹窗 | browse 的浏览器状态独立于聊天可见 |
| 是否轮到我？ | WAIT + 页内卡片 + 系统通知 | 通常是 Control 权限弹窗 | browse 把“安全确认”和“人要做一步”分成两种明确交互 |
| 我能否立刻接管？ | 工具栏按钮永远可收回标签 | 可在应用内切回、改分享级别 | Marionette 能做，但状态和入口不够显眼 |
| 失败后怎么办？ | status/nextStep、challenge、warning | 工具错误或 Agent 自己转述 | browse 更容易让用户知道该登录、关 DevTools 或换页面 |
| 做完后谁清理？ | Agent 自己关标签；空闲标签自动关 | 用户手动关或 Agent 记得关 | browse 更接近“客人离开会收拾” |

## 当前 Marionette 的分屏限制

这里要先纠正一个容易误判的点：**WebView2 不是限制**。

内置浏览器已经是一个 child WebView2，BrowserPage 会实时把宿主 DOM 的矩形同步给原生 surface。因此从窗口和渲染层看，把浏览器、文档或收纳盒放进一个分屏叶并不是天然做不到。

真正的限制在当前交互模型：

- splitWorkspace 的叶节点只有 session id 列表，SplitWorkspace 也只渲染 SessionPane；
- BrowserPage、DocPage、MemoBox 是 App 根上的伪标签状态，激活时覆盖中栏；
- 分屏渲染和 browserActive / activeDocPath / memoActive 是互斥分支；
- 文档与收纳盒的关闭、恢复、拖拽和焦点语义还绑在伪标签上，不在统一工作区标签模型里。

所以当前产品准确描述应是：

> Marionette 已经支持“多对话分屏”，还没有支持“浏览器与对话分屏”，也还没有通用异类标签分屏。

### 推荐形态：Browser Workspace，而不是一步做通用分屏

如果目标是接近 browse 的用户体验，第一版不需要把会话、浏览器、文档、收纳盒一次性统一成任意可拆的工作区。更稳妥的是增加一个专用浏览器工作模式：

1. 用户主动打开浏览器，或 Agent 发起 handoff / 用户发送批注时进入；
2. 浏览器占据主画布，保留自己的标签栏、地址栏和页面；
3. 右侧出现一条 Agent 对话栏，包含当前会话、转写尾部、Composer 和等待状态；
4. 中间可拖宽，浏览器与对话各自保持状态；
5. 关闭 Browser Workspace 后回到原工作区，原会话和文档标签不丢。

它比通用分屏更容易做对三件事：

- child WebView2 只需要跟随一个明确的 browser host 矩形；
- 批注“发给对话”可以直接落到右侧 Composer，不需要切页；
- WAIT、权限和失败 nextStep 可以同时出现在页面边和对话边。

### 四个用户状态

浏览器不应该永远占据主画面，而应按任务阶段切换：

| 用户状态 | 主画面 | Agent 位置 | 触发 |
|---|---|---|---|
| 背景执行 | 对话/文档 | 对话流中显示运行状态 | Agent 自己开后台页 |
| 主动查看 | 浏览器 | 右栏折叠为状态条 | 用户打开浏览器 |
| 协作 | 浏览器 + 对话侧栏 | 当前 Agent 会话 | 用户批注、Agent 请求确认或持续多步任务 |
| Handoff | 浏览器 + 对话侧栏 + 页内卡片 | WAIT 状态 | 登录、2FA、CAPTCHA、支付 |

因此，“主要看到浏览器画面”是对的，但只适用于用户主动查看、协作和 handoff 阶段。Agent 单纯在后台跑时，仍应以对话为主，不能让浏览器持续抢画面。

### 后续再考虑通用分屏

如果 Browser Workspace 验证成立，再把 splitWorkspace 从 session-only 升级为异类标签模型：

- session；
- browser；
- document；
- memo。

这会涉及拖拽、关闭、恢复、快捷键、焦点、原生 WebView bounds、标签去重和 detached 窗口语义，工作量明显大于专用浏览器模式。不要把它设成第一版浏览器 UX 的前置条件。

## 核心体验差距

### 1. Agent 标签不要抢走用户

browse 的 Agent 新标签默认开在后台，并在标签栏开头形成折叠组。用户不主动点进去就看不到页面变化；只有 Agent handoff 等人时，组才会展开。

Marionette 当前收到 fresh browser tab 事件会激活浏览器伪标签，并把浏览器页切到前台。这适合“展示 Agent 刚发现的东西”，但在多 Agent 长时间工作中会频繁打断用户。

建议：

- 默认把 Agent 自开标签放在后台；
- 浏览器标签页内按会话或任务分组，至少显示“Claude / Codex / 会话名”；
- 新标签只增加未读/活动计数，不改变当前会话、文档或设置页；
- 提供“正在看的标签”和“后台 Agent 标签”两段，不必照搬 Chrome 原生 tab group；
- 让用户在设置里选择“后台打开 / 自动切过去 / 独立浏览器窗口”。

验收：Agent 连续打开 5 个页面时，用户正在写的 Composer 和当前会话不发生变化；用户能一次看到 5 个后台页属于哪个 Agent。

### 2. 需要用户时，用 handoff，不要靠 Agent 猜

browse 的 handoff 流程完整覆盖登录、2FA、passkey、CAPTCHA 和支付确认：

1. Agent 明确说“这一步需要你”；
2. Agent 暂停，标签显示 WAIT；
3. 标签组展开，页面右下出现卡片，写明站点、Agent 和要让用户做的事；
4. 如果窗口在后台，发系统通知；通知点击后打开对应标签；
5. 用户完成后点 Continue，Agent 才继续；
6. 结果明确区分 continued、timeout、closed、cancelled。

Marionette 当前的 BrowserPermissionDialog 解决的是“是否允许 Agent 执行这个动作”，不是“请用户本人完成一个步骤”。两者不能混用：

- **安全授权**应该继续留在应用级弹窗，防止页面伪造或遮挡确认按钮；
- **人工步骤**更适合页内卡片，因为用户要边看页面边完成登录和验证码。

建议新增 browser_handoff：

- 参数只有一句用户能执行的话和超时；
- 状态进入 waiting_user，侧栏、浏览器标签和 Dock 同时可见；
- 页内 closed Shadow DOM 卡片显示 origin、Agent 名和说明；
- 窗口不可见时走现有桌面通知；
- 只有真实用户点击 Continue 才算完成；
- 页面关闭、用户停止 Agent、超时都要有可理解的结局。

验收：用户离开浏览器页后，Agent 请求 handoff 仍能在 3 秒内让用户看到“需要你”；完成后原 Agent 自动继续，不需要用户回聊天里再解释。

### 3. 标签需要“归属感”

browse 的标签不是一串匿名 b1、b2，而是带身份的工作对象：

- 标签可属于一个 Agent 会话或用户；
- 可由 Agent 指定稳定名字，例如 work-mail；
- 有 group、lastUsed、busy、readOnly 等状态；
- 新 window.open 弹窗自动归到同一任务；
- 用户借出的标签只能归还，Agent 不能把它关掉。

Marionette 当前 TabInfo 只有 id、url、title、origin、grant、openedByAgent。用户看得到“是不是 Agent 开的”，但看不出“哪个 Agent、哪个任务、是否正在调用、多久没用”。

建议至少补四个字段：

- ownerSessionId；
- groupId 或 taskLabel；
- busy；
- lastUsedAt。

UI 不必一次做完整标签管理，但应先把这些信息显示在浏览器标签、列表工具和关闭确认里。后续再加入 chosenId，让 Agent 能重新接回“工作邮箱”这类固定标签。

### 4. 浏览器状态要独立于聊天状态

browse 用三种极短状态建立持续可见性：

- ON：用户把标签借给 Agent，当前没有调用；
- RUN：Agent 正在使用；
- WAIT：Agent 等用户完成一步。

同时有标签组、页面右下 pill、工具栏 badge 和通知互相兜底。用户不用读聊天记录，也能知道浏览器发生了什么。

Marionette 目前主要是：

- share 标签显示 none/read/control；
- 运行中的 Agent 状态在会话侧；
- Control 请求才出现模态权限框；
- 浏览器页本身没有“Agent 正在跑/轮到你”的持续状态。

建议定义一套浏览器用户状态，并让它由事件驱动，而不是从 share 级别猜测：

| 状态 | 用户看到什么 |
|---|---|
| idle | 普通标签，无 Agent 调用 |
| agent_connected | “Claude 已接入”，但未运行 |
| running | 标签/ Dock 行显示运行中与 Agent 名 |
| waiting_user | 黄色“等你”，同时打开当前需要用户处理的页 |
| failed | 错误只在能转述下一步时显示，不铺长堆栈 |
| finished | 短暂显示“完成”，随后回到 idle 或清理 |

安全授权仍走现有应用级权限卡；浏览器状态只负责让用户知道发生了什么。

### 5. 失败和阻塞要给出下一步

browse 在用户可见性上做了三类额外工作：

- 导航后检测 Cloudflare、CAPTCHA、access denied 等 challenge，并提示 handoff；
- status 工具除了状态，还返回 nextStep，明确告诉 Agent 该让用户装扩展、配对、更新或等待；
- 工具栏分享失败会直接解释“这是 Chrome 内部页”“另一个 debugger 占用了它”“需要开启文件 URL 权限”。

Marionette 的错误更偏技术实现，例如 stale ref、CdpLease busy、页面未共享、截图失败。Agent 能读，但用户通常只看到“操作失败”或完全看不到。

建议把所有浏览器失败归成四类：

| 类型 | 例子 | 用户动作 |
|---|---|---|
| needs_user | 未分享、需要登录、CAPTCHA | 打开对应页并显示 handoff/权限卡 |
| transient | CDP 忙、页面仍在加载 | 自动重试并显示“正在等页面” |
| recoverable | ref 过期、页面已导航 | 自动或提示重新快照，不显示红错 |
| broken | WebView 崩溃、profile 损坏、工具关闭 | 打开浏览器健康面板，给修复/重置入口 |

工具结果也应稳定带 machine-readable 的 error code 和面向用户的 nextStep，避免每个 Agent 自己编解释。

### 6. 首次使用和健康状态

browse 因为依赖扩展和 relay，必须做较重的 onboarding。它的优点不是“步骤多”，而是每一步有真实状态：

- 当前还缺安装、配对、扩展还是浏览器连接；
- 能自动检查是否已成功；
- 能复制诊断；
- 能一键关闭所有 Agent 标签；
- 设置页直接说明 Agent 标签开在哪里、如何分组、何时清理。

Marionette 内嵌浏览器不需要让用户安装扩展，这是明显优势，不应该照搬 browse 的完整 setup wizard。但还需要一个轻量的“浏览器健康”面：

- Agent 浏览器工具是否开启；
- MCP 是否在运行；
- 当前 profile 与登录状态；
- 活动标签、所属 Agent、busy/waiting；
- 最近一次失败和下一步；
- 清理空闲 Agent 标签；
- 重置浏览器 profile；
- 复制诊断信息。

入口可以放在浏览器页的 ⋯、Ctrl K 或设置页，不必新做一级产品。

### 7. Agent 工作纪律会直接变成用户体验

browse 在 MCP instructions 里固定了工作习惯：

- 先 snapshot，再选择动作；
- 点击/输入后重新验证，不把“调用成功”当“任务成功”；
- 不碰没有授权或没有交给它的标签；
- 能等待就用 wait_for，不靠反复 snapshot 轮询；
- 只有人能做的步骤要 handoff，不尝试过 CAPTCHA；
- 完成后关闭自己开的标签，或用 group 关闭整组。

Marionette 的 MCP 目前以 schema 为主，没有同等强度的全局操作说明。Agent 的纪律会随模型漂移，最终表现成：

- 多余快照和重复点击；
- 标签越开越多；
- 遇到登录页继续重试；
- 忘记关闭自己开的页面；
- 任务完成后不验证。

建议在 MCP initialize 返回的 instructions 中加入一版简短规则，不增加工具数量，但能立刻改善用户看到的行为。

## Marionette 已经领先的地方

### 1. 人告诉 Agent“就是这里”

元素批注是 Marionette 目前最强的差异化体验：

- 十字准星和框选比写 selector 自然；
- 元素摘要、区域元素和截图同时给出；
- 用户先写“把这里改成什么”，再发送；
- prompt 会重新解析 aria ref，失败时保留 selector；
- 页面数据有不可信声明，URL 也被脱敏；
- 用户发送前在 Composer 检查，不会直接执行。

这套体验比 browse 更接近“设计反馈”，应该继续作为主打能力。

### 2. 权限语义更细

browse 的用户体验更像“这个标签借给 Agent”。Marionette 明确区分：

- 用户自开标签；
- Agent 自开标签；
- none/read/control；
- always-approve 不穿透用户标签；
- Control 和应用级权限卡。

这套安全模型不要为了 badge 的简洁而删掉。正确方向是保留 grant 语义，把状态说得更清楚。

### 3. 与聊天和附件管线一体

Marionette 的批注能直接进 Composer、形成附件药丸、随会话保存。browse 更像独立浏览器代理，最终仍要把结果转述回 Agent。这个一体化优势应保留。

## browse 的“demonstrate”值得作为下一阶段

browse 不只有 handoff，还能让用户示范一次流程：

1. Agent 发起 demonstrate；
2. 用户正常点击、输入、选择、导航；
3. 系统记录 selector、role/name、导航，密码值永远不记录；
4. 用户点 Continue；
5. Agent 得到结构化 steps 和一行一步的 summary，之后自己重放。

这和元素批注正好互补：

- 元素批注适合“我要你改这个地方”；
- handoff 适合“这一步只能你做”；
- demonstrate 适合“我演示一次，你以后照着做”。

它比增加 eval 或任意脚本执行更贴近普通用户，也更符合 Marionette 的“人可读、可接管”方向。

## 推荐优先级

### P0：先补“用户在场感”

1. 增加专用 Browser Workspace：浏览器主画布 + Agent 对话侧栏；
2. Agent 新标签默认后台打开，不改变当前会话或文档；
3. 标签增加 ownerSessionId、busy、lastUsedAt；
4. 每个标签显示 running/waiting_user/error 三类浏览器状态；
5. 增加 browser_handoff、WAIT 状态、页内说明卡和通知；
6. MCP instructions 加入 inspect-act-verify、handoff 和标签清理规则。

### P1：让用户能管理和恢复

7. 浏览器健康面板：工具、MCP、profile、最近错误、清理标签；
8. Agent 标签按会话分组，关闭会话时可选择关闭其标签；
9. Agent 自开且空闲的标签自动关闭，用户标签永不自动关；
10. 失败结果统一提供 nextStep；
11. challenge/blocked 页面停止重试并转 handoff。

### P2：补齐高阶人机协作

12. demonstrate：记录用户示范的点击、输入和导航；
13. 稳定命名标签，例如 work-mail；
14. session/browser/document/memo 通用异类分屏；
15. 标签独立窗口选项；
16. 可选连接真实 Chrome；嵌入式浏览器继续作为默认安全环境。

## 不建议照搬

- 不照搬 browse 的扩展安装、配对码和 relay 设置流程。Marionette 内嵌浏览器没有这层用户成本。
- 不照搬完整 Chrome tab group UI。Marionette 的浏览器页空间更小，应该用会话分组、状态行和未读计数表达同样含义。
- 不把 Control 安全确认改成页内卡片。页内卡片适合用户任务，安全授权应继续留在应用级可信托管层。
- 首轮不做多浏览器连接、远程 relay、network/recording、QuickJS execute。它们提升能力强，但不是当前最主要的体验断点。

## 体验验收清单

- Agent 打开新页面时，用户当前正在写的消息不丢、不切页；
- 用户能在 2 秒内回答“哪个 Agent 在用哪个标签”；
- Agent 需要人时，当前窗口和后台窗口都能明确收到信号；
- 用户能从需要的页面、浏览器标签或会话侧栏任一入口完成 handoff；
- 只有真实用户点击能完成 handoff；
- 失败结果至少给出一个可执行下一步；
- Agent 完成任务后，自开标签可自动或一键清理；
- 用户自己开的标签、固定标签和长期任务标签不会被自动关闭；
- 元素批注、截图和发送前预览保持现有体验，不因新增状态条增加操作步数。

## 证据索引

- browse 使用说明与工具表：<https://github.com/anomalyco/browse/blob/a7181bf/README.md>
- 标签组交互：<https://github.com/anomalyco/browse/blob/a7181bf/src/background/agent-relay/groups.ts>
- 空闲标签清理：<https://github.com/anomalyco/browse/blob/a7181bf/src/background/tab-cleanup.ts>
- 页内 handoff 卡片：<https://github.com/anomalyco/browse/blob/a7181bf/src/content/page-status.ts>
- 通知兜底：<https://github.com/anomalyco/browse/blob/a7181bf/src/background/handoff-notice.ts>
- 设置页：<https://github.com/anomalyco/browse/blob/a7181bf/src/status/settings.tsx>
- Agent instructions：<https://github.com/anomalyco/browse/blob/a7181bf/rust/crates/browse-relay/src/instructions.md>
- browse 状态与 nextStep：<https://github.com/anomalyco/browse/blob/a7181bf/rust/crates/browse-relay/src/tools.rs>

Marionette 对照代码：

- src/app/App.tsx：新浏览器标签自动激活、浏览器伪标签接线；
- src/components/BrowserPage.tsx：地址栏、标签、分享菜单、批注抽屉；
- src/lib/browserPick.ts：不可信声明、URL 脱敏、多页面 prompt；
- src-tauri/src/browser/mod.rs：grant、权限请求、标签命令；
- src-tauri/src/tools_mcp.rs：浏览器工具 schema 与 Control 授权；
- src-tauri/src/browser/surface.rs：标签 surface、快照、截图、导航与 picker 生命周期。
