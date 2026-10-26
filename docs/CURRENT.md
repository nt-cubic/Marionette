# Marionette — 当前状态（2026-10）

> 现行产品说明。历史设计稿与已完成路线图见 [`archive/`](./archive/)。

## 一句话

Windows 本地 **多 Agent CLI 图形壳**（Tauri + React）。不替代 OpenCode / Claude / Codex / Grok 等原厂 agent，只负责启动、展示、切换、记录与 handoff——外加把「任何一个来源的模型 / key / 订阅」喂给「任何一个 agent」的**跨用引擎**。

## 产品底线（仍有效）

- 不做自研 Agent runtime / 模型路由器 / 云同步 / MCP marketplace
- ACP 优先；会话绑定 agent；Clean 为主界面
- 轻、快；不做成 IDE 或 API 管理平台

## 已具备（相对用户可感知）

| 能力 | 说明 |
|------|------|
| 项目 / 会话 | 项目 CRUD、搜索 threads（按标题过滤，不读正文）、项目拖拽排序、会话按发送 recency 排序；空闲 30 分钟的会话自动「挂起」（结束 agent 进程，下次发消息自动接上历史）；行内 ⏸ 只在进程还活着时画（starting / running / waiting / error），按下去就是停——正在跑的直接中断并结束进程，说出口的半截回复留着并标「已挂起」；已挂起的会话不画按钮；冷启动恢复上次焦点和分屏（无存档则不打开对话，点底部输入框会在 Chat 新建）；Ctrl+K 命令面板：跳任意对话/项目、新建对话/聊天、重开刚关的标签、切主题 |
| Chat 对话 | 侧栏项目列表下的独立对话列表；使用进程默认工作目录，不创建项目或工作区 `.marionette`；资源管理器「在此处打开」遇到已有 `.marionette` 才按项目打开 |
| 多 Agent | 内置 + custom ACP agents；懒启动、能力协商、model/mode/effort/文件权限（agent 自己 advertise 的旋钮才出现，按 option id 判定，category 只当提示） |
| Clean 对话 | transcript JSONL、edit&resend（重发会带上原来的 agent/模型标记，后面的回复仍是 Reply 而不是 Notification）、普通权限弹窗、Ask/Plan 进对话流（卡片上回复，输入框收起，写入 handoff.md）、@ 派任务、从任意一条回复分叉出新会话（复制到该条为止，首次发送自动带上这段历史）、回复里的图片按会话工作目录解析（文件名差一个字符时找同目录里唯一近似的那张）；Agent 读到的图片在工具卡里直接出预览；输入框贴图带缩略图；点图片开四周发黑的灯箱（拖动平移、滚轮缩放）；并行子代理（Grok `spawn_subagent` / Claude parentToolUseId / OpenCode Task）收进专用格子而不是刷满主消息流；Markdown 走主流 GFM（react-markdown + remark-gfm，编号列表 10/1000、重复字不再被流合并吃掉）；默认闭眼按「运行段」折叠 tool/thinking：**当前轨迹从第一步就作为一张组卡出现**（标题是 `1 thought, 1 read, 1 search` 这类 tally），live 时上方显示**实时泳道时间图**，下方 tally 详情保持折叠（**不自动展开 thought 或工具正文**）；**整轮结束后自动折回 tally 摘要**（时间图一起收起；点开摘要看到同一套占位条 + 60s 标尺，不是短轴断点），运行时可手动查看详情，完成时统一折叠；开眼状态跨启动记住（细则见下节） |
| Handoff / Git | handoff.md、changed files、只读 diff；**每轮交付卡走私有 index 的内容树快照（本轮窗口口径，见下节）** |
| 多窗口 | Tab 幽灵撕出 → 松手开 detached 窗；关窗 hide 防 tao 闪退；隐藏壳 LRU 回收。主窗对话区最多 4 格分屏（拖标签到中区/格的左、右、上、下约 1/4；可 1\|1\|1、1\|2 或 2×2；同一方向再拆会均分整行/整列）；每格自己的标签、转写、输入框；标签 Columns2 / 右键「在右侧打开」「在下方打开」；重启恢复分屏。撕出去的窗口拖回主窗：边缘拆格、中间并入、坐标失败进焦点格；关撕窗也插回焦点格。侧栏点已撕开的对话只聚焦那个窗口。权限/提问/计划卡片会把焦点切到当事格；权限框一到就画 |
| Usage | 上下文、OpenCode 余额探测、Claude/Codex/Grok 限流类数据、**订阅配额（引擎 `magpie quota`，跨 agent 可见的订阅窗口并入同一面板）** |
| 登录 | 多 agent probe + Sign in 横幅 |
| **Telegram 远程** | 左下角纸飞机图标：@BotFather 建个 bot 粘 token（getMe 校验，DPAPI 落 `~/.marionette/telegram.json`）→ 给 bot 发条消息在浮层确认配对（只认这一个 chat）。会话跑完/等你批准（权限、ask、plan）/出错/进程死亡推送到手机；**跑完推送带全文正文，MarkdownV2 格式（代码块/粗体/链接），长文自动分片**；权限/ask/plan 的通知带内联按钮，远程解答与 UI 共享同一 waiter（先答先得）；在 Telegram 里回复文字 = 送进对应会话（引用回复定向，裸文本去最近活跃会话），走与本地输入完全相同的发送管线（挂起重连/历史注入/排队）；权限等待 120s 到期前 30s 追加提醒。**置顶「总控」面板**：谁在跑/谁在等你实时重画（忙时每 60s 保鲜），按钮点会话名 = 切换裸发文字去向，⏹ = 远程停止（与 UI 停止键同路），`/status` 拉全量清单。通知偏好（跑完/等待/出错/带内容）在浮层开关。token 校验、配对轮询和消息发送统一跟随应用代理，设置变化在下次请求生效，无需 TUN。纯出站 HTTPS，零新依赖；未配对时零行为变化。详见 [`telegram-bridge.md`](./telegram-bridge.md) |
| **贴边停靠（edge dock）** | 主窗可并入屏幕左右边缘（`dockMode.ts` 三态：floating / docked-hidden / docked-expanded）：**把普通窗口拖到屏幕左右边缘会自动吸附**（离边约 20 逻辑 px 停住，边缘亮一条青色提示，松手后贴边并保持展开；**抓住窄条顶部往外拖 >48 逻辑 px 即解除贴边**，窗口留在松手处并恢复普通尺寸，不再弹回贴边前的位置；也可从 Ctrl K 命令面板执行贴边/解除）。停好后整条移出屏外只留 6px 淡轨 + 中间一根青色把手（有对话在跑时把手呼吸），鼠标触碰滑出（~160ms 9 步），离开 300ms 且失焦收回（⋯ 菜单开「**侧边栏常驻**」后不再自动收回，手动收回/解除贴边照旧）；**把文件或文字拖到把手上也会滑出**（OLE 拖拽期间鼠标事件被抑制，展开信号改听 tao 的 onDragDropEvent 与 wry 补丁的 drag-text 通道）；滑出后整条分区接住拖放：**上半条（标题/额度/进行中）= 收入收纳盒，下半条对话区 = 进输入框成附件**（拖的是文字时下半条落进输入框光标处成文本，同一条 `marionette:dock-drop-text` 转交链路），拖动中双层虚线提示标出两个落点、悬停侧点亮（收纳盒整页开着时，App 级路由器整体让路——收纳盒页自己的监听接住**整窗**拖放；其它整页或无对话在开时全部进收纳盒）；弹出只挪窗不抢焦点，全屏视频/演示模式不弹。关闭主窗 = 贴边收起（引擎、全部会话、Telegram 桥继续跑；`autoHideOnClose` 默认开，可在 Ctrl K 切换），真正退出走窄条 ⋯ 菜单或 Ctrl K 的「退出 Marionette」（`quit_app`）；再次启动 Marionette 唤起。窄条（默认 400，**贴边展开时抓内缘可拖宽 320–560**，松手记住宽度）自上而下：标题栏（拖动区 · 搜索=Ctrl K · ⋯ 菜单收纳 侧边栏常驻/用量与项目/升级/解除贴边/退出；**右上角不再单独放用量图标**）→ 操作行（**新建对话点开是目的地菜单**：聊天（默认文件夹）/ 各项目（在所选项目里开新会话）/ 全部对话（进完整列表用每个项目的 ＋），不再固定建进聊天；旁为收纳盒）→ **额度**（每个订阅一张紧凑计量卡：名称独占一行 + 一条 3px 进度条 + 最紧窗口百分比；纯余额来源单独一行做成「余额 + 金额」chip，不再画空心圈；悬停看全部窗口与重置时间，点开用量页）→ 「进行中」列表（仍持有进程的对话 + 当前对话，取代标签行，超过 5 行滚动，悬停可挂起；**完成（waiting）的对话不退场，一直留到真正挂起——30 分钟自动挂起 / 行内 ⏸ 手动挂起 / 进程退出——才从列表消失**；只影响这个列表的显示，全部对话里完成徽标还在）→ 对话与输入框（**贴边时工具栏收成单个入口**：滑杆图标 + 当前模型名，点开「模式/权限/模型/Agent」分组菜单，选一项再进原有完整列表；＋ 附文件与发送钮保留在外，不再是一排会换行的芯片；**composer 在贴边时按内容自动高度**，不再沿用普通窗口记住的 104/112px，也不提供高度拖拽与 42vh 展开按钮——底边直接压在底栏上沿，附图时向上长高，工具按钮不再被挤下去）→ **底栏**（输入框下面单独一栏：模型共享入口 + 主题/通知/代理/Telegram + 右侧检查更新，就是把非贴边时左右侧栏底部的按钮并成一行；由两条侧栏把各自的 footer 子树 portal 进来，见 `src/lib/dockFooter.ts`）；**composer 的所有下拉（模型/agent/mode/权限/effort/发送）在打开后会按视口 8px 边距夹回屏内**（400px 窄条里右对齐的模型菜单原先会从左边溢出被 WebView 裁掉，看起来像整张列表消失）。全部对话 / 用量与项目 / 收纳盒是整页切换，标题栏变成「‹ 返回 + 页名」，Esc 或返回回到主条，从全部对话里选中对话也自动返回；窗口按钮在条内隐藏，分屏停靠时暂停。**收纳盒**（`~/.marionette/memos/`）：收纳盒页开着时**整窗都是落点**——资源管理器拖到窗口任意位置（含侧栏、标签行）即收，不再只认页面本体的矩形（内容哈希去重、复制落盘）、**从其它应用拖入文字/链接也整窗即收**（浏览器选区、链接、图片 URL——裸 URL 自动成链接条目；走 vendor wry 补丁的 `marionette-drag-text` 通道，与文件拖放互不干扰；普通窗口里把文字/链接拖到输入框上则直接插入光标处）、Ctrl+V 收文字/链接、**顶上「写一条」输入卡**（Ctrl+Enter 保存，裸 URL 自动成链接）、搜索与类型筛选、条目按时间倒序铺成**单列帖子卡**（文字卡铺 6 行正文点开展开、图片卡按横竖比完整显示不裁切、链接卡域名+网址、文件卡扁卡；按「今天 / 昨天 / N月N日」分段）、文件条目可直接拖进输入框成附件；「选择」或 Ctrl 点击多选后可「开新对话」（一律在左侧栏「聊天」栏目下方的「收纳盒」分组里建出新对话，材料清单预填进输入框）或「整理成文档」（预填让 agent 按主题整理并存到项目 `docs/notes/<日期>-<主题>.md` 的指令；开着项目建进该项目，没开项目同样落收纳盒分组），都只预填不自动发送。收纳盒分组复用聊天行（重命名/挂起/置顶/删除），有对话才显示。**普通窗口的收纳盒入口位于左侧栏「聊天」栏目正上方**，点开后在标签栏尾部出现一枚「收纳盒」伪标签（带条目数徽标），与其他对话标签并列；中栏只切换对话流那一列（左右侧栏照常），会话 composer 行藏掉，页内顶上是「写一条」卡。选中收纳盒时只有它是当前页（对话标签退成未选中，当前会话仍记着）；选中任一对话标签时收纳盒标签保留、页面切回对话；× 或 Esc 关闭标签（分屏态同样支持）。聊天内容上方不再有「收纳盒 / 能力与上下文」工具栏，分屏不重复入口。实施与红线见 `AGENTS.md`「贴边停靠」节，设计稿 `docs/plan-edge-dock.md` |
| **跨用（模型/key/订阅互通）** | 见下节 |

## 对话流轨迹（clean view 折叠，2026-10-24）

- **折叠粒度 = 运行段**：连续 thought / tool_call 合成一段（`turnActivity.ts::clusterActivityRuns`），**第一步就是一组**，后续流入沿用起点 key，单步历史也保留统一分组。
- **一组卡有三部分**：上方实时轨迹面板（`LiveActivityTimeline` → `ActivityTimeline`）按 thought / tool / command / search 等类别画时间泳道；中间为当前思考预览（仅运行中）；下方是**默认完全折叠**的计数详情。实时图与预览均在 details 外面，不驱动详情开合。
- **思考预览防闪烁**：首次有非空 thought 时出现固定四行（正文 72px）的 `LiveThoughtPreview`，流式显示最新 thought。切到工具时保留文字、标为「最近思考」；下一段 thought 在同一个区域原地替换，新段还没有文字时不清空旧内容。短 think 不改变高度，长文内部滚动并在用户未上滚时跟随底部；不会自动展开整组或任何步骤卡。运行段结束随实时图和思考预览一起移除。
- **实时性**：从第一步起显示，500ms 子组件时钟推进当前 thought/tool 时间条，不要求下一条事件到达；两步之间短暂 settle 不消失。时钟不带整个聊天重渲染。
- **时间轴先长后缩**：开跑先落在一把固定 60s 的标尺上（`LIVE_AXIS_FLOOR_MS`），时间条从左往右长，长满标尺后轴才改跟真实流逝时间（进入相对压缩）——否则第一条 think 一睁眼就是 100% 宽（0→now 恰好铺满轴）。10s 的标尺太短：跑过几个工具后当前 think 已经贴在右缘，看起来像从右边往左长。**实时图和点开后的冻结图都用这把标尺**，14s 的回合点开后轴仍是 0s–1m，不会把 14s 拉满整条轨道。
- **条按占位画，不只按工具墙钟**：Grok 的 write/search 经常 0–1ms 就完成，若用 `completedAt` 诚实时长，工具泳道全是小点、只有 think 会涨。每一步从它开始画到下一步开始；最后一步运行中跟时钟涨到现在，点开历史图时用后面那条 Reply 的开始时刻收尾（尾段 thought 常常没有 `endedAt`，回合结束只给最后一张 Reply 盖戳）。实时图和冻结图同一公式，点开后不会退回短轴上的断点。组标题上的时长和卡片上的 `Thought · Ns` 同一口径。
- **组卡标题永远是 tally**：`1 thought, 2 reads, 3 edits`；live 工具可带一行当前工具 gist。折叠组件忽略 React 自己写 open 属性引发的 toggle，防止误记成用户开合。
- **完成后自动折叠**：运行段结束（后续发言或 idle）一起移除实时图与思考预览，只留下 tally；点摘要才能再看见时间图（同一套占位条 + 60s 标尺）和步骤详情。整轮完成清掉详情/步骤开合状态，手动打开过的卡也折回。历史步骤正文只在用户点击后显示。
- **选择保护**：拖选文本期间 `viewRunning` 冻结在冻结时刻的值（事件本身也已冻结），尾段不会在复制途中自己折起来。
- 眼睛（开/关）只决定「每张卡各自成行」还是「按运行段折叠」；默认闭眼，开眼状态跨启动记住。

## ChatGPT 网页桥的原排版回复

ChatGPT 网页桥（`tools/web-chat-bridge`）复制本轮回复的 HTML 与原站 CSS，在 Reply 卡片的 Shadow DOM 中显示，保留表格、代码高亮、卡片和网格图；字体与中性色适配 Marionette 主题，整页纵向滚动，宽内容可横向滚动。HTML 与纯文本一起流式更新、保存和恢复；纯文本继续用于复制、搜索和历史上下文。CSS 与可导出的图片按内容哈希缓存在 `BRIDGE_STATE_DIR/rich-replies/`；缓存缺失则回退文字。链接、代码复制和图片放大由壳接管，原站引用浮窗等脚本交互不迁移。旧记录不自动补 HTML。首次升级需重新构建前端与 Rust。

DeepSeek 正文按网页语义转成 Markdown，保留标题、强调、嵌套列表、引用、链接、表格与代码块，沿用 Marionette 的字体和主题；公式暂显示 TeX 文本。流式更新使用 `marionette.textSnapshot` 完整正文快照，明确替换当前未封口回复，支持网站回写格式和删改短文本；历史仍保存普通 Markdown。思考区继续使用独立思考事件。Telegram 转发同步按快照替换，避免重复累加。首次升级需重新构建前端与 Rust 并重启，旧记录不自动补排版。

## 再次进入对话（agent re-entry，2026-10-25）

Agent 在回合结束后自己续跑（它起的后台指令回来了，触发新模型调用，没有新的用户消息）：对所有 ACP 走同一条通用线（`src/lib/agentReentry.ts`）。会话空闲（`turn/complete` 已到）超过 2.5s 后再收到 assistant/thought 流内容，就视为再次进入对话——会话重新显示为运行中（Working 条、侧栏脉冲、实时轨迹图回来），续跑内容另起一张 Reply 卡。收尾两条路：适配器随后发了 `turn/complete` 就按正常回合收尾；没发的（如 zcode）静默 6 分钟后封口尾卡（补时长）、状态回空闲并像跑完一样通知/记未读；之后再续跑再武装新段。用户发消息随时接管正常回合生命周期。回归：`npx tsx scripts/verify-agent-reentry.mts`。

## 跨用引擎（2026-10 定稿形态）

目标：**用户无脑打开软件就能用**——任何一个来源的模型，任何一个 agent 的会话里直接选。

- **key 型直连**（OpenCode Go / DeepSeek / GLM / Kimi / OpenRouter / xAI）：claude-code 走 spawn env、grok 走 owned 配置表，不过网关；来源含凭据库手存的 key 和 opencode `auth.json` 的只读扫描引用（引用只在直连注入瞬间读钥匙，不复制）。
- **订阅型 + codex 全场景**：经内嵌翻译引擎（[yetone/magpie](https://github.com/yetone/magpie)，MIT，Go）。引擎**按需唤醒**：打开跨用菜单（最多等 5s）、绑定共享模型（最多等 20s）、启动已接桥的 agent 会话都会自动拉起；30 分钟无绑定且无需求自动停 serve。key-plan 与扫描引用的 api key 自动注册进引擎；Grok / ChatGPT 订阅由引擎 discovery 自己发现。codex 只接引擎源（协议约束），经 `providers/set` 改道 + 路由组把内建模型名别名到所选引擎模型。
- **DSH（@openma/deepseek-harness-acp）跨用（2026-10-20）**：spawn 时把 llm-pi-ai 托管路由 `marionette`（openai-completions → 引擎 `/v1`）写进 `~/.dsh/cordis.patch.yml`（文本外科手术、幂等、保用户内容），令牌常量进 `~/.dsh/.credentials.yaml` refs + spawn env；模型经 dsh-acp 原生目录广告（id `marionette::provider/model`）。引擎停用即移除路由；home patch 被 dsh Web/desktop 共享，Web 里也能走这条路由。写入器在 `dsh_bridge.rs`，调研定稿见 `research-dsh-cross-use.md` §8。
- **pi（pi-acp）跨用（2026-10-20）**：spawn 时把 provider `marionette` 合并进 `~/.pi/agent/models.json`（pi 官方兼容端点机制，`apiKey` 用 `$MARIONETTE_ENGINE_TOKEN` env 插值、凭据不落盘；语义幂等、保用户 provider）；模型经 pi-acp 原生目录广告（id `marionette/<engineId>`，与 grok/opencode 前缀同形），`set_config_option` 直接切。引擎停用即移除托管段。写入器在 `model_bridge.rs`（`refresh_pi_marionette_route`）。
- **Roster 收束（2026-10-20）**：内置 agent 收为「五跨用（claude-code / grok / codex / opencode / deepseek）+ **pi + hermes**」；cline / gemini / kimi / codebuddy / openclaw / cursor / omp 退出内置名单（cursor 封闭无口子、gemini 协议锁死，其余用户面小——可接性矩阵见 `research-acp-cross-use-matrix.md`）。退出者可经「自定义 agent」随时加回；历史会话导入的 parsers 与登录探针保留未动。hermes 的跨用桥待装好 hermes 后按矩阵处方接（config.yaml providers，双协议面）。
- **一种便携版**（`tools\build-portable.bat` → `release\<版本号>\Marionette.exe`，~16MB）：magpie 打进 exe 作出厂种子，打开即离线落位。打包默认先拉上游 **latest 稳定版**（`import-magpie.ps1`；`SKIP_MAGPIE_IMPORT=1` / `MAGPIE_TAG=` 可跳过或钉版本）。`third_party/magpie` 是提交进仓库的源码，**打包日志会提示「与 HEAD 不一致」——发布前把 `third_party/magpie` 一起提交**，release 提交与发出去的种子才对得上。发布用 `tools\publish-release.bat`。
- **引擎可更新**：本机权威在 `~/.marionette/engine/`。左下角「模型共享」可检查/下载 yetone/magpie 稳定版（约 48MB CLI）；下载的版本**比内嵌新就粘住**，反之**本机装的比出厂种子旧时启动会自动前移**（否则重编新版本也一直跑旧引擎）。默认端口 3425 上若已有别的 magpie：版本 ≥ 内嵌就复用，**更旧则忽略它，自己在下一个空闲端口起最新引擎**。「恢复出厂引擎」仍可随时再解 exe 内嵌。与 Harness 等 agent 自更新同级，不必等 Marionette 发版。
- **入口**：左下角互联图标 = 模型共享状态/一键启动；凭据管理退居其浮层底部链接。用量面板点刷新会拉网关 `/v1/magpie/quotas`（各家订阅额度）。
- **模型目录与显示管理**：统一按来源分组原生和共享模型，只有「接口主机 + 凭证」确认同一账号的入口才合并（agent 自带 provider、Grok 的 `[model.*]` 表、引擎目录会并成一组，标题用 agent 的厂商名）；订阅登录态（Codex 的 ChatGPT、Grok 的 xAI）同样只算一个账号——拥有登录文件的 agent 整份目录并入引擎分组，另一个 agent 复制的同一 OAuth 登录（OpenCode 的 `openai`）按 `accountId` 判重后不再单列；模型名相同但主机或凭证不同的条目保留。合并前保存的隐藏与排序按旧标识迁移。模型菜单底部的「配置模型与来源…」与模型共享浮层里的「模型与来源 · 显示和排序…」都打开这一页，可分别隐藏 provider / 模型、调整 provider / 组内模型顺序、恢复默认；大来源默认收起、排序箭头悬停才出现；保存到 `~/.marionette/model-library.json`，多个窗口同步，隐藏不影响已有会话。扫描 Grok / Pi / OpenCode / Codex / Claude 的用户自定义接口，保留原配置引用，经引擎供其他已接桥 agent 使用；扫描位置与失败原因可在凭据页查看。「添加密钥」支持自定义 Chat Completions / Responses / Anthropic 接口和显式模型 ID。范围与使用说明见 [模型与来源](model-library.md)。
- **卫生**：删除凭据条目会从引擎回收对应 provider（注册账本对账）；30 分钟无绑定且无需求的空闲引擎自动停 serve，任何跨用入口（菜单/绑定/按钮/启动）自动唤醒。
- 安全底线不变：明文 key 只进 env / owned-section / magpie CLI 命令行，绝不进日志与 agent 配置；扫描只读。

## 交付卡（每轮改动的口径）

卡上的文件列表 = **本轮窗口内的真实内容差异**，不是工作区状态。

- **基线**：`send` 时用私有 git index（复制用户 `.git/index` 当 stat 缓存 + 私有 object dir/alternates）给整个 worktree 拍一棵内容树（`git_service::begin_workspace_snapshot`，实测本仓库 0.6–1.2s；裸 `read-tree`+`add --all` 要 8.6s，所以必须 seed）。turn 末再拍一次，`diff-tree -r -M --name-status/--numstat` 出每文件增删——`+/-` 是真数字，不再靠工具 card 里的 diff 行数猜。
- **为什么不用 `git status` / `git diff HEAD`**：二者口径在本仓库实测分叉——`status -uall` 报 224~227 条，真实内容差异只有 52 条（`third_party/magpie/**` 这类 `text=auto`/CRLF 文件 stat-dirty 但 diff 0 行），于是「按 status 列文件、按 diff 算指纹」会给 186 个文件留空指纹，任何比对都可能把它们当成新改动。
- **谁算本轮的**：起点干净、结束脏 → 无条件算（工具、子代理、`npm run build` 都认）；**起点就脏**的文件只有被本轮自己的 edit/write 工具碰过才算，否则同一个工作区里另一场对话（或用户的编辑器）的改动会串进这张卡。没有 HEAD 时无法拆分，宁可全留。
- **fail closed**：基线拍失败/不是 git 仓库 → 不发布任何 git 真相，卡片只留本轮工具账本。旧实现 `catch { return [] }` 把失败当「基线是空的」，于是整库脏文件全进了卡——这就是「交付卡显示全仓库改动」的直接成因（2026-10-23 修）。
- 子目录项目（项目根 ≠ 仓库根）的路径按**项目根**相对返回（`rev-parse --show-prefix` 剥掉），与 `getFileDiff` 的解析口径一致。
- **一行一个文件**：卡片合并的键是规范化路径（正斜杠、去掉 cwd 前缀、小写），不是原始字符串——git 报的是工作区相对路径（`src/lib/api.ts`），工具调用里带的是 agent 原样给的绝对路径（`D:\Myself\Marionette\src\lib\api.ts`），按原串合并会把每个被工具改过的文件列两遍（2026-10-23 修，回归用例在 `scripts/verify-turn-delivery.mts`）。git 报出的相对路径优先用于行标题、diff 请求与撤销。

## 明确不做 / 暂缓

- 内嵌 llama.cpp / 本地推理 runtime
- 完整文件树 / IDE 级编辑器
- 全 Rust UI 重写
- 把历史 ASP 文档当现行协议实现
- 裸 CLI 全局配置接管（终端里裸跑 claude/codex 不经 Marionette 也能用引擎——已明确不做，见归档设计稿）

## 工程入口

- 前端：`src/`
- 后端：`src-tauri/src/`
- 启动：`start-marionette.bat`（根目录唯一入口）/ `npm run tauri dev`
- 便携构建：`tools/build-portable.bat` → `release/<版本号>/Marionette.exe`
- 引擎源码吸纳/升级：`tools\import-magpie.ps1`（默认 latest 稳定；或 `-Tag <上游tag>`）
- 验证脚本：`scripts/`；Windows 脚本：`tools/`（各目录索引见其中的 `README.md`）

## 归档说明

`docs/archive/` 保留早期策划、路线图与 superpowers 规格，便于考古，**不以之为当前实现契约**。有冲突时以代码与本文件为准。跨用引擎的完整设计稿（M0-M3 过程记录）已归档至 [`archive/cross-agent-bridge.md`](./archive/cross-agent-bridge.md)。
