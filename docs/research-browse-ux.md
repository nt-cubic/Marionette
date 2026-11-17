# 调研：anomalyco/browse 的用户体验对标（2026-11-17）

- 参考仓库：<https://github.com/anomalyco/browse>
- 分析快照：首轮 dev a7181bf；二轮最新 dev c3ad2ada（2026-11-17）；仓库版本仍是 0.1.0，接口和行为仍会变化
- Marionette 基线：feat/browser-annotation 分支 200e4bf5，包含元素批注及其安全、生命周期修复
- 研究重点：用户实际感受到的浏览器体验，不以“工具数量”排名
- 架构差异：browse 驱动用户现有的 Chrome 系浏览器；Marionette 驱动内置 WebView2 小浏览器

## TL;DR

browse 和 Marionette 最大的体验差异不是工具多少，而是对浏览器的定义不同：

- browse 把浏览器视为**用户自己的领地**，Agent 只是被允许进入的客人。客人有自己的标签组，工作时不抢当前标签，需要人时会明确亮起 WAIT，做完后应自己收拾。
- 调研基线时的 Marionette 更像**在应用里给 Agent 开了一块浏览器面板**。P1–P4 后已经补齐后台标签、标签归属、运行状态、handoff，以及 Browser / Document / Memo 与对话同屏。

调研基线时，Marionette 最明显的六个 UX 缺口（P1–P4 后大部分已解决，正文保留基线判断）：

1. **Agent 打开页面会打断用户当前操作。** 已解决：默认后台打开，只有显式 workspace 才进入 surface。
2. **缺少“Agent 暂停，等用户完成登录/2FA/支付”的正式流程。** 已解决：browser_handoff、WAIT、Continue / Deny 和通知闭环已落地。
3. **多 Agent 的标签归属不清楚。** 已解决：标签记录 owner session、busy、status、lastUsedAt，会话删除会清理其标签。
4. **用户很难一眼判断 Agent 正在跑、等人、已完成还是出错。** 已改善：标签显示简短状态文字，等待和失败有显式状态。
5. **失败后缺少“下一步做什么”。** browse 会识别 challenge、给 warning/nextStep、解释无法分享的页面；Marionette 多数错误只回到 Agent 的工具结果，用户界面不一定接得住。
6. **浏览器和对话还不能同时工作。** 已解决：Browser、Document、Memo 共用 SurfaceWorkspace，右侧复用同一会话和 Composer。

Marionette 已有而 browse 没有同样完整体验的部分：

- 元素拾取、框选、逐条批注、自动裁剪截图、发送前预览；
- 不可信页面数据声明、URL 脱敏、多页面分组；
- 更细的 none/read/control 授权，以及 Agent 自开标签与用户标签的隐私边界；
- 与应用内对话、Composer 附件和会话恢复直接结合。

建议保留这些优势，把 browse 的“用户在场感”补进来，而不是把 browse 的扩展安装、配对和 relay 架构整套搬过来。

## 同类产品：Codeg、Codex、ZCode、Claude Desktop 是不是也一样

结论：**是，Codeg、Codex desktop、ZCode、Claude Desktop 都已经在同一个桌面工作区里让 Agent 对话和网页并存。** 差异主要不是“有没有浏览器”，而是谁是主画面、浏览器是内置还是接入用户 Chrome、以及是否需要侧聊天扩展。

| 产品 | 对话位置 | 浏览器位置 | 是否同屏 | 关键差别 |
|---|---|---|---|---|
| browse | 外部 Agent 客户端；browse 本身没有完整聊天框 | 用户自己的 Chrome；扩展 + relay / MCP | 否，浏览器和 Agent 对话分在两个界面 | 它解决“让外部 Agent 进入我的浏览器”，不是做一体化 Agent 客户端 |
| Codeg | 工作区左栏的 Agent 对话 | 工作区右栏的 file workspace；browser 与文件、diff 是同一种 tab | 是；对话/文件可调分栏，任一栏都能最大化 | 最通用的形态：浏览器不是专用页，而是工作区 surface 的一种 tab；Agent 访问仍要求用户逐 origin 分享 |
| Codex / ChatGPT desktop | ChatGPT desktop 的聊天或任务区 | 内置浏览器（独立 profile），或通过扩展接入用户现有 Chrome 系浏览器 | 是；内置浏览器有 `Enter split view` 和 `Enter full view` | 最接近 Marionette 想做的产品形态，而且已有元素/区域批注、样式调整和 CDP 开发者模式 |
| ZCode | ZCode 自己的 Agent 会话 / 工作区 | Electron 内嵌 BrowserView，位于右侧 side pane，可切完整视图 | 是 | 本机安装包里有 `toggleSidePane`、`toggleSidePaneFullView`、`browser.show/hide`、浏览器标签与全屏预览 |
| Claude Desktop | Chat / Cowork 任务 | Cowork 内置浏览器，开在任务旁的 side panel；也可改用 Claude in Chrome 扩展 | 是 | 目前明显是“任务为主、网页在旁边”；内置浏览器有独立 profile，可选导入站点登录状态 |
| Marionette（P1–P4 后） | 共享 surface 右栏，可切换绑定会话 | 内置 WebView2，与 Document/Memo 共用 surface 标签 | 是 | 已有元素批注、分享级别、多 Agent 会话、handoff 和恢复 |

### Codex

Codex CLI 和 Codex IDE extension 本身没有内置浏览器；这里说的 Codex 使用场景是 **ChatGPT desktop app 里的 Codex**。官方文档明确给出两套方式：

- 内置浏览器：ChatGPT 和用户共享同一页面。页面可以切到 `Enter full view`，也可以切到 `Enter split view` 让 tab 与 chat 并排。内置浏览器使用独立 profile，不自动共享日常 Chrome 的标签和登录状态。
- Chrome 系扩展：接入已有 Chrome、Edge、Brave、Opera、Vivaldi，使用用户当前 profile；前四种浏览器还支持浏览器内的 side chat，Opera 不支持 side chat。

Codex 的批注模型和 Marionette 当前方向高度重合：工具栏进入 Annotation mode，点元素或拖选区域，写评论，再让 Agent 按评论修改。它还提供字体、间距、颜色等 `Adjust` 预览，以及用于 DOM、console、network、性能检查的 CDP 开发者模式。

因此，Codex 已经验证了用户设想的核心闭环，而且做得比“浏览器主画面 + 聊天侧栏”更灵活：聊天与浏览器可以并排，也可以让当前 tab 全屏；浏览器内的页面批注直接服务于 Agent 任务。

### ZCode

ZCode 也是完整桌面 Agent 客户端，不是纯浏览器扩展。本机安装包 `D:\Program Files\ZCode\resources\app.asar` 的 renderer 和 Electron main 代码表明：

- 有完整的 embedded browser / guest webContents、标签恢复、viewport 更新、截图、录制、下载和 CDP 通路；
- 浏览器是右侧 side pane 中的一种内容，另有 review、terminal、文件、Sites 和插件；
- `CmdOrCtrl+Alt+B` 切换 side pane，`CmdOrCtrl+Shift+F` 进入/退出完整视图；
- 文件树里的 HTML 可以直接“用内置浏览器打开”，聊天里的预览卡也能“预览”到 side pane；
- 设置里有内置浏览器控制、Chrome 登录状态导入和 Computer Use。

这说明 ZCode 的交互与 Codex 接近：主工作区承担任务和会话，右侧面板承载浏览器及运行结果，需要时再把右侧面板放大。Marionette 可以借鉴“浏览器只是工作区中的一种 surface”，但仍应保留自己更清楚的分享级别和批注入口。

### Codeg

Codeg 当前 `main`（2026-11-17，`4991406e`）不是给浏览器单独做一种页面，而是先把工作区抽象成 **conversation + files** 两栏：

- `WorkspaceMode` 只有 `conversation` 和 `fusion`；只要文件栏有可见 tab，就进入 `fusion`；
- 左栏是 Agent 对话，右栏是文件工作区，默认约 56/44，可拖分隔条；
- 对话栏或文件栏都可以“最大化”覆盖另一栏，另一栏保持挂载和状态，不会因切换而重建；
- `BrowserWorkspaceTab` 是 `FileWorkspaceTab` 的一个 discriminated-union 分支，和 file / diff / rich-diff 共用 tab strip、持久化、切换、预览、suspend 和 surface 生命周期；
- 最右侧的 aux panel 是文件树、会话详情、Git、任务等，不是浏览器；浏览器仍在中间的 file workspace 里。

所以 Codeg 的默认形态更准确地说是 **对话左、浏览器右**；把 file 栏最大化后，就是用户设想的“浏览器主画面，对话在旁边且随时可恢复”。它没有为浏览器复制一套 Composer，也没有把 browser 做成和会话分屏互斥的伪标签。

浏览器 tab 的工具栏把两个方向放在同一个地址栏里：

- 左边是 Agent 分享：用户把当前 origin 以 `read` 或 `control` 分享给 Agent；
- 右边是 Agent 活动记录：读过、点过、输入过、被拒、失败都按动作与结果列出；被拒记录不会因为后续操作成功而消失；
- 中间还有“发给对话”菜单：拾取元素、截取页面、截图框选/箭头批注、递送 console error；结果进入当前活跃对话的 Composer，作为可编辑、可删除的胶囊和附件，不自动发送。

这和 Marionette 的批注抽屉有一个关键差异：Codeg 的元素拾取是单次立即交接，截图画布则提供框和箭头；Marionette 当前允许同一批收集多条元素/区域、逐条写 note，再统一发送。两者可以并存，Marionette 的批量批注仍是更强的设计评审体验。

Codeg 的 Agent→页面授权也更严格：即使 tab 是 Agent 自己打开的，也不会自动获得读取权；用户必须主动分享当前 origin。localhost 的 grant 还会固定监听程序和 working directory，避免端口被另一个项目复用后继续沿旧授权读取。Marionette 当前“Agent 自开 tab 默认可读”的便利性更高，但权限语义比 Codeg 宽松，这是需要在 Browser Workspace 阶段明确的产品选择。

Codeg 的 dev server 发现值得直接借鉴：终端输出出现 loopback URL 后，先做 TCP probe，再按偏好选择“自动打开 / 通知 / 关闭”。自动打开时只选中 browser tab，不把用户从正在读的对话里强行带走；浏览器 tab 也可从 `+` 菜单的“Local services”重新打开。这个机制比仅凭 URL 字符串切界面可靠得多。

对 Marionette 的直接启示：

- 长期最优架构不是永远维护一个特殊的 `BrowserPage`，而是把 session / browser / document / memo 收敛成统一 worksurface；
- 第一阶段仍可用专用 Browser Workspace 控制风险，但组件边界应照着“surface + chat”设计，避免以后拆第二套 Composer；
- 页面→对话继续保留 Marionette 的批量批注；页面工具栏可补单次 pick、整页截图、console error 等快捷入口；
- Agent→页面继续用 `none/read/control`，并考虑让 Agent 自开 tab 也经过一次用户分享；
- dev server 自动预览应采用“显式事件 + probe + 用户偏好”，不要用 localhost 正则抢界面。

### Claude Desktop

Claude Desktop 现在也不是“只有普通聊天”。Claude Cowork 在桌面应用内自带浏览器，官方描述是：当任务涉及网站时，浏览器在任务旁边的 side panel 打开；Claude 打开站点、读取页面、点击、输入和填写表单时，用户可以直接看着。链接也会在同一个面板中打开。

Claude 同时保留两种浏览器路径：

- 内置浏览器：独立于用户自己的浏览器，不碰现有标签和登录；可逐站导入 cookies，适合把网页部分交给 Agent。
- Claude in Chrome：在用户现有 Chrome side panel 里运行，能直接读取当前页面、已登录账号和控制浏览器。

因此 Claude Desktop 的交互更偏“任务为主、浏览器在旁边”，而不是固定的“浏览器主画面、聊天右栏”。但这已经证明页面和对话同处一个桌面工作区是主流方向。

### 对 Marionette 产品的含义

“浏览器主画面 + 右侧 Agent 对话”这个理解是对的，但要加一个限定：它是 **Web 产物任务态**，不应成为所有任务的默认壳。普通聊天、研究、文档整理仍然以对话为主；Agent 在后台查资料时，浏览器也应在后台。

真正值得 Marionette 做的是专用 Browser Workspace：

- 浏览器为主画布，右侧复用当前 Agent 会话和 Composer；
- 只在 Agent 明确预览 Web 原型 / 网站，或用户主动“在浏览器中查看”时进入；
- Agent 普通打开网页默认后台，不改写当前文档、会话和输入草稿；
- 批注、权限卡、WAIT 和后续 handoff 在浏览器与对话两侧同时可见；
- 退出后恢复原来的会话分屏或文档标签。

这里不能仅凭 `localhost` 自动切界面：API、文档服务、后台任务和一次性登录页也会命中 localhost。第一版应由 `browser_open_tab` 的显式 `display: "workspace"` 控制，默认 `background`；以后再加项目类型识别作为提示，而不是强制切换。

Marionette 的相对优势要说得准确：**一体化浏览器 + 对话本身已经不是独家优势。** Codeg、Codex、ZCode、Claude Desktop 都已有同类能力。Marionette 真正有机会领先的是：

- 同一应用里切换多个 Agent 和多个会话；
- 元素/区域批注直接形成带截图的 Composer 草稿；
- 更细的 `none/read/control` 分享语义与 Agent 自开标签边界；
- 后续把浏览器、会话、文档、收纳盒和 handoff 统一到一套本地工作区状态。

如果只做“页面在左、聊天在右”，只能补齐与 Codeg / Codex / Claude 的产品差距；把多 Agent、批注和安全边界做成统一体验，才是 Marionette 的差异化。

## 用户心理模型

| 问题 | browse 的回答 | Marionette 当前的回答 | 体验差距 |
|---|---|---|---|
| 这是谁的浏览器？ | 你的浏览器，Agent 是 guest | 内置独立 profile；用户标签默认不共享 | 两者都明确，Marionette 的授权粒度更细 |
| Agent 在哪工作？ | 独立标签组、可选独立窗口 | 后台 tab 带 owner session；workspace tab 与对话同屏 | 方向已对齐，会话分组 UI 仍可继续细化 |
| 它现在在干什么？ | RUN 标签、页面 pill、组状态 | tab 显示使用中 / 等你 / 失败，对话同时显示轨迹 | 核心状态已经同屏可见 |
| 是否轮到我？ | WAIT + 页内卡片 + 系统通知 | waiting_user + surface 内 handoff + 系统通知 | 两种人工步骤已经与安全授权分开 |
| 我能否立刻接管？ | 工具栏按钮永远可收回标签 | tab、handoff 提示和分享菜单都在应用内 | 入口已显式，仍可补健康面板 |
| 失败后怎么办？ | status/nextStep、challenge、warning | 工具错误或 Agent 自己转述 | browse 更容易让用户知道该登录、关 DevTools 或换页面 |
| 做完后谁清理？ | Agent 自己关标签；空闲标签自动关 | 删除会话/项目会清 owned tabs；空闲自动清理仍待做 | 生命周期已接上，自动回收还需补偏好 |

## 调研基线的分屏限制（2026-11-17 已迁移）

这里要先纠正一个容易误判的点：**WebView2 从来不是限制**。

内置浏览器一直是 child WebView2，BrowserPage 会实时把宿主 DOM 的矩形同步给原生
surface。调研时真正卡住的是旧伪标签状态：

- splitWorkspace 的叶节点只有 session id；
- BrowserPage、DocPage、MemoBox 激活时覆盖中栏且彼此互斥；
- 文档与收纳盒的关闭、恢复、拖拽和焦点语义绑在伪标签上。

P1–P4 没有重写 session-only 分屏树，而是新增一个独立的 `SurfaceWorkspace`：

- Browser、Document、Memo 都是 surface 的一种；
- 右侧只保留一套 SessionView + Composer；
- surface 打开时不渲染 session split tree，退出后原树原样恢复；
- 三种 surface 共用标签、会话切换、关闭快捷键和冷启动恢复。

因此当前产品准确描述是：**Marionette 同时支持多会话分屏，以及 Browser / Document /
Memo 与 Agent 对话同屏；两种布局各管自己的状态，互不吞掉。**

### 采用形态：最小通用 SurfaceWorkspace

实现按以下边界落地：

1. `surface + chat` 是独立于 session split tree 的布局边界；
2. Agent 只在 Web 产物预览时用 `display:"workspace"` 进入，普通网页保持后台；
3. 右侧复用当前会话转写、Composer、权限卡和等待状态；
4. 中间可拖宽，窄窗自动上下堆叠；
5. 关闭 surface 后回到原工作区，原会话和 content 标签不丢。

### 四个用户状态

浏览器不应该永远占据主画面，而应按任务阶段切换：

| 用户状态 | 主画面 | Agent 位置 | 触发 |
|---|---|---|---|
| 背景执行 | 对话/文档 | 对话流中显示运行状态 | Agent 自己开后台页 |
| 主动查看 | 浏览器 | 右栏折叠为状态条 | 用户打开浏览器 |
| 协作 | 浏览器 + 对话侧栏 | 当前 Agent 会话 | 用户批注、Agent 请求确认或持续多步任务 |
| Handoff | 浏览器 + 对话侧栏 + 页内卡片 | WAIT 状态 | 登录、2FA、CAPTCHA、支付 |

因此，“主要看到浏览器画面”是对的，但只适用于用户主动查看、协作和 handoff 阶段。Agent 单纯在后台跑时，仍应以对话为主，不能让浏览器持续抢画面。

### 更多的内容页已经迁进 worksurface

Document 与 Memo 已经迁进同一 `SurfaceWorkspace`，并共用 Browser / Document / Memo 标签条。后续只有在拖拽、恢复、关闭、焦点和 detached 语义都稳定后，才考虑把 session 也并入同一棵任意异类标签树：

- session；
- browser；
- document；
- memo。

通用标签树会涉及拖拽、关闭、恢复、快捷键、焦点、原生 WebView bounds、标签去重和 detached 窗口语义，工作量和风险都远大于 Browser surface。不要把它设成第一版浏览器 UX 的前置条件。

## 核心体验差距

### 1. Agent 标签不要抢走用户

browse 的 Agent 新标签默认开在后台，并在标签栏开头形成折叠组。用户不主动点进去就看不到页面变化；只有 Agent handoff 等人时，组才会展开。

调研基线时，Marionette 收到 fresh browser tab 会激活浏览器伪标签，并把浏览器页切到前台。这适合“展示 Agent 刚发现的东西”，但在多 Agent 长时间工作中会频繁打断用户；Phase 1 已改为默认后台，只有 `display:"workspace"` 才进入 surface。

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

Marionette 现在明确区分两件事：BrowserPermissionDialog 解决“是否允许 Agent 执行这个动作”，browser_handoff 解决“请用户本人完成一个步骤”。两者不能混用：

- **安全授权**留在应用级弹窗，防止页面伪造或遮挡确认按钮；
- **人工步骤**显示在 Browser surface 的页面与 host 之间，用户能边看页面边完成登录和验证码。

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

## 推荐优先级与落地状态

### 已完成（2026-11-17，P1–P4）

1. 通用 `SurfaceWorkspace`：Browser / Document / Memo 与 Agent 对话同屏；
2. Agent 新标签默认后台打开，只有显式 workspace 才进入 surface；
3. 标签记录 ownerSessionId、busy、lastUsedAt 与运行状态，删除会话时清理归属标签；
4. `browser_handoff` 的 WAIT、Continue / Deny、通知和来源会话切换；
5. dev server 的 loopback 扫描、TCP probe、通知 / 自动打开 / 忽略偏好；
6. 共同标签、绑定会话切换、关闭快捷键与冷启动恢复。

### 仍需做

1. 失败统一给出可执行 nextStep，并区分 challenge、login、permission 与页面脚本错误；
2. 浏览器健康面板：MCP/profile/最近错误/一键清理；
3. Agent 标签按会话分组；空闲 Agent 标签可按偏好自动清理，用户标签永不自动关；
4. challenge/blocked 页面停止重试并转 handoff，而不是让 Agent 反复点同一元素。

### 按需探索

1. demonstrate：记录用户示范的点击、输入和导航；
2. 稳定命名标签，例如 work-mail；
3. 将 session 与 content surface 进一步统一成任意异类标签树；
4. 标签独立窗口；
5. 可选连接真实 Chrome，但仅用于 staging、内部后台和登录态开发联调；嵌入式 WebView2
   继续作为默认安全环境，不做个人生活助手或社交、邮箱、银行批量管理。

## 二轮结论：还值得借什么（2026-11-17，最新 dev c3ad2ada）

最新 dev 相比首轮快照没有重做交互模型，主要新增是 read-only tab 的输入防绕过、重连后仍保持
read-only，以及 GET/body 处理。因此上一轮的 P1–P4 判断仍然成立；下面只列还没借、但对真实使用有
明显收益的部分。

### P0：先借“用户随时能接管，Agent 也不会留下垃圾”

2026-11-17 已落地第 2–4 项的基础版：stable tab id、pin/claim、用户 touch、空闲清理和
Agent-close 边界。页面内 WAIT 卡仍未实现，留给下一批。

1. **页面内 WAIT 卡，而不是只在 Browser 顶部横条提示。** browse 把卡片注入页面的 closed
   Shadow DOM 和 top layer，只有真实点击可 Continue；页面不能伪造，密码管理器弹层仍可盖在
   卡片上使用。Marionette 已经有 picker 的 isolated world / trusted-event 机制，可以复用这套
   安全边界。视觉上只做 WAIT 时右下角一张小卡：title、origin、一句指令、Continue；RUN 仍由
   右侧对话和标签状态表达，不额外盖常驻浮层。
2. **Agent 可命名 tab，并允许用户 pin / claim。** browse 用 work-mail 这类 stable id 解决
   “同一页面被 Agent 反复重复打开”和“它到底在操作哪个标签”。Marionette 可给
   browser_open_tab 增加可选 slug；同名调用返回现有 tab。标签菜单增加「保留」和「转为我的
   标签」：pin 不自动清理，claim 后 owner 清空，删除会话也不能关它。
3. **空闲 Agent tab 自动清理，但要有一串硬豁免。** 默认 10 分钟，设置 10 / 30 / 60 /
   never。只清 Agent-owned；busy、waiting_user、handoff、当前可见、用户刚看过的 tab、
   pinned / named 永远跳过。用户切换 tab 和手动导航也要 touch lastUsedAt，不能只算工具调用。
4. **每个标签都有“现在由谁控制，以及怎么收回”。** Agent 工作时提供「收回本页」：立即撤销
   分享、取消未决 permission / handoff、阻止后续动作，并保留页面给用户继续用。browse 的
   toolbar 始终是 kill switch；Marionette 现在只有修改分享级别，用户不够有安全感。

### P1：把失败变成可理解、可行动的状态

5. **challenge detector + nextStep。** 复用 browse 的保守分类：Cloudflare、DataDome、
   PerimeterX、Akamai、reCAPTCHA/hCaptcha、generic block。非交互 challenge 先等 15 秒；
   交互 challenge 自动提示 WAIT/handoff；block 明确让 Agent 停止重试。工具返回值同时带
   machine-readable kind/vendor/signal/recommendation，用户界面只展示一句人话和唯一动作。
6. **紧凑 Browser 健康条，而不是完整诊断台。** 有异常时才出现：WebView crash、子页面无响应、
   profile 写入失败、最近工具错误、等待人数、Agent tab 数。动作只有「重新载入」「关掉空闲
   Agent tabs」「复制诊断」。正常时完全不占高度。
7. **handoff 结果细分，并把等待时间放宽。** browse 默认 10 分钟且区分 continued / timeout /
   closed / cancelled；Marionette 当前 2 分钟且关闭标签也归为 denied。建议默认 10 分钟、
   最大 30 分钟，并把 closed、cancelled 如实返回 Agent，避免它误判用户拒绝。
8. **Agent tabs 按会话 / 任务折叠。** 用户 tab 永远直接可见；同一会话的 Agent tabs 收成
   Codex · 4，当前 tab 和 WAIT 组自动展开。browse 的 Chrome tab-group 细节不用照抄，借用
   “隐藏后台、需要时展开、整组关闭”的信息层级即可。

### P1.5：补 Agent 稳定性，间接改善用户看到的体验

9. **browser_wait_for。** 等待 text/textGone、URL、元素可见/隐藏、页面 load，而不是让 Agent
   自己反复 snapshot + sleep。右侧状态可显示「等待页面稳定」，减少假卡死和无效点击。
10. **JS dialog 处理。** 网页 alert/confirm/prompt 或 beforeunload 会把页面暂停；提供
    status/accept/dismiss 和每 tab 的 auto policy。先弹出应用内小提示，比页面冻结后靠 Agent 猜
    更可靠。
11. **Web 原型的 screenshot diff。** 修改前存 baseline，agent 每轮操作后返回是否有像素变化、
    变化区域和截图。它不是永久 UI 控件，只在 Web 原型任务的交付卡里显示「页面已改变 / 未改变」。

### P2：能力更强，但只适合按需加

12. **demonstrate。** 用户在页面里示范点击、输入、选择、导航，Agent 得到结构化 steps；密码值
    永不记录。这很适合非开发者，也最符合 Marionette 让普通用户“做给我看”的方向，但需要
    isolated-world 多 frame 录制、导航续装和敏感值脱敏，工作量明显高于 idle cleanup。
13. **upload / drag / scroll / emulate。** 对上传统计、拖拽排序、响应式预览有用，属于 Agent
    操作能力，不需要新增主界面。
14. **network / recording。** 仅作为 Web 开发排障工具按需开放。它们会显著增加存储、脱敏和导出
    界面，不适合进入默认 Browser HUD。

### 建议的第一刀

先做一个“tab 生命周期包”，因为它同时解决用户最常遇到的三件事：标签越开越多、同名页面重复
打开、删会话时误关用户正在看的页。顺序是：

1. chosenId + pinned + claim + touch lastUsed；
2. idle cleanup 与 Ctrl K 的 10 / 30 / 60 / never；
3. Browser tab 按 ownerSession 折叠、WAIT 自动展开；
4. 再做页面内 WAIT 卡和 challenge 检测。

这样先稳住标签生命周期，再改页面内的接管体验；否则 WAIT/自动清理同时上线时，用户很难判断是
Agent、页面 challenge 还是 cleanup 把标签状态改了。

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

- browse 使用说明与工具表：<https://github.com/anomalyco/browse/blob/c3ad2ada/README.md>
- 标签组交互：<https://github.com/anomalyco/browse/blob/c3ad2ada/src/background/agent-relay/groups.ts>
- 空闲标签清理：<https://github.com/anomalyco/browse/blob/c3ad2ada/src/background/tab-cleanup.ts>
- 页内 handoff 卡片：<https://github.com/anomalyco/browse/blob/c3ad2ada/src/content/page-status.ts>
- 通知兜底：<https://github.com/anomalyco/browse/blob/c3ad2ada/src/background/handoff-notice.ts>
- 设置页：<https://github.com/anomalyco/browse/blob/c3ad2ada/src/status/settings.tsx>
- Agent instructions：<https://github.com/anomalyco/browse/blob/c3ad2ada/rust/crates/browse-relay/src/instructions.md>
- browse 状态与 nextStep：<https://github.com/anomalyco/browse/blob/c3ad2ada/rust/crates/browse-relay/src/tools.rs>
- read-only 输入防绕过：<https://github.com/anomalyco/browse/commit/1ef9276>
- read-only 跨重连保持：<https://github.com/anomalyco/browse/commit/45850a0>
- Codeg 工作区双栏与最大化：<https://github.com/spacering-net/codeg/blob/4991406e6045b7903ae2c5bc78db0d3b7dfd1f77/src/app/workspace/layout.tsx>
- Codeg Browser 作为 file workspace tab：<https://github.com/spacering-net/codeg/blob/4991406e6045b7903ae2c5bc78db0d3b7dfd1f77/src/contexts/workspace-context.tsx>
- Codeg 文件栏渲染 browser tab：<https://github.com/spacering-net/codeg/blob/4991406e6045b7903ae2c5bc78db0d3b7dfd1f77/src/components/files/file-workspace-panel.tsx>
- Codeg 页面→对话：<https://github.com/spacering-net/codeg/blob/4991406e6045b7903ae2c5bc78db0d3b7dfd1f77/src/components/browser/browser-page-handoff.tsx>
- Codeg Agent 分享与活动记录：<https://github.com/spacering-net/codeg/blob/4991406e6045b7903ae2c5bc78db0d3b7dfd1f77/src/components/browser/browser-agent-access.tsx>
- Codeg 浏览器授权与 loopback 身份：<https://github.com/spacering-net/codeg/blob/4991406e6045b7903ae2c5bc78db0d3b7dfd1f77/src-tauri/src/browser/agent.rs>
- Codeg dev server 发现后打开/通知：<https://github.com/spacering-net/codeg/blob/4991406e6045b7903ae2c5bc78db0d3b7dfd1f77/src/components/browser/browser-service-bridge.tsx>
- Codex / ChatGPT desktop 内置浏览器、split view 与批注：<https://learn.chatgpt.com/docs/browser>
- Codex / ChatGPT desktop 浏览器扩展与 side chat：<https://learn.chatgpt.com/docs/chrome-extension>
- Claude Cowork 内置浏览器：<https://support.claude.com/en/articles/16607400-use-the-built-in-browser-in-claude-cowork>
- Claude in Chrome 与桌面端集成：<https://support.claude.com/en/articles/12012173-get-started-with-claude-in-chrome>
- ZCode：本机安装包 `D:\Program Files\ZCode\resources\app.asar`，对照 renderer 文案 `toggleSidePane`、`toggleSidePaneFullView`、`browser.show`、`browser.hide` 与 Electron embedded-browser 实现（2026-11-17 检查）。

Marionette 对照代码：

- src/app/App.tsx：background/workspace fresh-tab 分流、Browser surface 接线；
- src/components/SurfaceWorkspace.tsx + src/lib/workspaceSurface.ts：通用 surface + chat 双栏与宽度规则；
- src/components/BrowserPage.tsx：地址栏、标签、分享菜单、批注抽屉；
- src/lib/browserPick.ts：不可信声明、URL 脱敏、多页面 prompt；
- src-tauri/src/browser/mod.rs：grant、权限请求、标签命令；
- src-tauri/src/tools_mcp.rs：浏览器工具 schema 与 Control 授权；
- src-tauri/src/browser/surface.rs：标签 surface、快照、截图、导航与 picker 生命周期。
