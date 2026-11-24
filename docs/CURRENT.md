# Marionette — 当前状态（2026-11）

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
| 项目 / 会话 | 项目 CRUD、搜索 threads（按标题过滤，不读正文）、项目拖拽排序、会话按发送 recency 排序；空闲 2 小时的会话自动「挂起」（结束 agent 进程，下次发消息自动接上历史；子代理/后台命令/定时发送仍算在运作，不挂起）；悬停会话行三个按钮：⏸ 挂起（只在进程还活着时画：starting / running / waiting / error，按下去就是停——正在跑的直接中断并结束进程，说出口的半截回复留着并标「已挂起」；已挂起的会话不画按钮）、删除、⋯（置顶 / 重命名 / 导出为 Markdown）；冷启动恢复上次焦点和分屏（无存档则不打开对话，点底部输入框会在 Chat 新建）；Ctrl+K 命令面板（侧栏底栏齿轮打开同一面板）：跳任意对话/项目、新建对话/聊天、打开收纳盒/圆桌/Telegram 远程/模型共享、导出当前对话为 Markdown、重开刚关的标签、切主题。导出走系统「另存为」，准备中有 toast，失败不写空文件；成功 toast 可复制全文。**Ctrl+F 在当前对话中查找**（面板同入口「在当前对话中查找」）：右上角浮条（Esc 关、Enter/Shift+Enter 上下处、`i/n` 计数），大小写不敏感子串匹配，搜的是**已加载范围**内始终可见的主干卡（你 / 回复 / 计划 / 提问 / 交接 / 子代理 / 改动交付卡；thought/tool 在折叠簇内不参与），命中跳到所在卡并描边高亮（虚拟列表走 `offsetOf` 定位、短对话走 DOM 定位，流式期间不劫持滚动位置）。侧栏原有入口（收纳盒、圆桌、Telegram、模型共享）都还在 |
| Chat 对话 | 侧栏项目列表下的独立对话列表；使用进程默认工作目录，不创建项目或工作区 `.marionette`；资源管理器「在此处打开」遇到已有 `.marionette` 才按项目打开 |
| 多 Agent | 内置 + custom ACP agents；懒启动、能力协商、model/mode/effort/文件权限（agent 自己 advertise 的旋钮才出现，按 option id 判定，category 只当提示） |
| Clean 对话 | transcript JSONL、edit&resend（重发会带上原来的 agent/模型标记，后面的回复仍是 Reply 而不是 Notification）、普通权限弹窗、Ask/Plan 进对话流（卡片上回复，输入框收起，写入 handoff.md）、ACP notice / Codex 重试 / Claude 会话失败 / 发送后 60 秒无输出以独立「通知」卡进入对话流（不在输入框上方弹条）、@ 派任务、从任意一条回复分叉出新会话（复制到该条为止，首次发送自动带上这段历史）、回复里的图片按会话工作目录解析（文件名差一个字符时找同目录里唯一近似的那张）；**Reply/You 里的图片链接（`[说明](D:/x.png)`）会在链接上方直接出图，远程 http(s) 图片同样显示**；Agent 读到的图片在工具卡里直接出预览，**收起的工具行/运行段行也带小缩略图（点击开灯箱）**；输入框贴图带缩略图；点图片开四周发黑的灯箱（拖动平移、滚轮缩放）；并行子代理按厂商走私方式认成独立卡（Claude Agent/Task + parentToolUseId、OpenCode Task、Grok `spawn_subagent`、Codex collab `spawnAgent` / 无 spawn 工具的 `_meta.codex.subagent`、ZCode Agent 标题是任务描述、通用 spawn 动词（`delegate_task` 等）；ACP 没转发内层转录就不编造）收进专用格子而不是刷进 tally / 主消息流；Markdown 走主流 GFM（react-markdown + remark-gfm，编号列表 10/1000、重复字不再被流合并吃掉）；富文本按内容懒加载：公式（KaTeX，单 `$` 恒字面）、代码高亮（Shiki 双主题）、流程图（```mermaid）、生成式卡片（```spec，见下节）；按「运行段」折叠 tool/thinking（**没有全局眼睛开关**，永远折叠；单卡仍可点开）：**当前轨迹从第一步就作为一张组卡出现**（标题是 `1 thought, 1 read, 1 search` 这类 tally），live 时上方显示**实时泳道时间图**，下方 tally 详情保持折叠（**不自动展开 thought 或工具正文**）；**整轮结束后自动折回 tally 摘要**（时间图一起收起；点开摘要看到同一套占位条 + 60s 标尺，不是短轴断点），运行时可手动查看详情，完成时统一折叠（细则见下节） |
| Handoff / Git | handoff.md、changed files、只读 diff；**每轮交付卡走私有 index 的内容树快照（本轮窗口口径，见下节）** |
| 右侧栏 | 固定三标签：**当前**保留 Tasks / 额度 / 改动文件与后台任务行；**共享**收纳 MCP、技能、插件、记忆、交接；**图片**按当前对话列出用户附图和工具图片（总数、来源筛选、日期分组、随栏宽自适应的两列以上网格，长列表分页加载）。标签选择持久化，并支持左右方向键切换；图片不再原地展开挤压其它右栏内容。点图开全屏查看器，点图片外空白 / 遮罩、Esc 或关闭键均可退出；本地图片可点「批注并添加」进入现有打点/框选批注器，确认后作为带批注的 Composer 附件回到当前对话（同路径复用同一附件，再次批注会更新标记而不是重复插图）。后台任务聚合层已为后续独立「任务」标签预留扩展点 |
| 多窗口 | Tab 幽灵撕出 → 松手开 detached 窗；关窗 hide 防 tao 闪退；隐藏壳 LRU 回收。主窗对话区最多 4 格分屏（拖标签到中区/格的左、右、上、下约 1/4；可 1\|1\|1、1\|2 或 2×2；同一方向再拆会均分整行/整列）；每格自己的标签、转写、输入框；标签 Columns2 / 右键「在右侧打开」「在下方打开」；重启恢复分屏。**收纳盒 / 文档 / 浏览器面板也是窗格标签（2026-11-18）**：单格时点击就是完整标签页，主标题栏照常列标签，不出现第二套空标签栏；拖到中栏边缘才进入多格，每格只列自己拥有的标签，可与对话任意混排、拖动条调宽高、跨窗格移动；收纳盒与文档标签可拖出主窗口成独立小窗（无对话栏的纯面板窗，「合并回主窗口」按钮或拖回主窗恢复原位），浏览器面板不可撕（原生控件钉在主窗，拖出=取消）。撕出去的窗口拖回主窗：边缘拆格、中间并入、坐标失败进焦点格；关撕窗也插回焦点格。侧栏点已撕开的对话只聚焦那个窗口。权限/提问/计划卡片会把焦点切到当事格；权限框一到就画。**输入框过窄时底栏芯片收成贴边同款 hub**（量的是这一格 composer 自己的宽度，不是整窗）：≤580px 收成滑杆+当前模型名一个入口（＋ 附文件与发送留在外），≥640px 再摊开芯片，中间一段不来回闪 |
| Usage | 上下文、OpenCode 余额探测、Claude/Codex/Grok 限流类数据、**订阅配额（内置引擎 `/v1/magpie/quotas`，跨 agent 可见的订阅窗口并入同一面板）**、**Token 用量分析伪标签页 v2**（2026-11-08 家里做的版面 + 2026-11-09 修正批）。入口只有 Ctrl K「Token 用量分析」（**右栏不放 Token 总览卡**；主界面不加按钮）：普通窗口开 `is-usage` 伪标签、贴边走 DockPage `usage` 整页（**对话列表行可点开对话**）、detached 弹窗兜底。页面（`src/components/usage/`）：英雄五数（四桶合计/费用/缓存命中/轮数/活跃时长，均带环比与来源标签）→ 每日四桶堆叠柱+折算费用虚线（按天/按周）→ Token 去向树图（缓存命中历史/工具调用/agent 自带+未缓存历史/你的输入/思考/回复，每块标实测/估算/推算）+ 时间去向（等首字/思考/回复/工具/其它，每日丝带 + p50/p95）→ 按 agent·模型表（可展开、走势火花线）→ 工具/命令/技能·MCP 排行 + **文件活动**（被读/被改最多的文件）→ 作息热图 + 上下文占用分布 + **压缩/失败轮/中断轮统计** → 对话列表（**可搜索**、按时间窗取数——短窗口不再显示整段历史累计；行尾「详情」开**单会话按轮面板**：上下文占用曲线、压缩刻度、失败轮、逐轮 token/模型/结局、**对话组成**（Grok：九类真数组成条——系统提示词/工具定义/注入/你的输入/附件/回复/思考/工具调用/结果，按结束时的实测上下文大小校准；其它 agent 如实显示暂无））→ **历史对话（本机日志）**：扫描各 agent CLI 自己的会话库（`usage_backfill.rs`，增量缓存），补上不经过 Marionette 开的对话，与实时台账按 agent 自报会话编号去重（重复的只列不计）→ 口径脚注。**记账口径（`usageLedger.ts` + `usageDigest.ts`）**：四桶 + 按天/按 agent/按会话三层聚合，每轮 turn/complete O(1) 增量、绝不重载转录、保留 180 天；时间去向/工具命令技能计数/文件活动/可见输入估算按轮折叠；**改动行数优先 git 对账**（轮末交付卡同一快照，工具文本口径仅兜底）；**上下文占用只记本轮被实时更新过的读数**（usage_update / Grok per-call；冻结的种子值不算——部分 agent 只报一次，旧版把它们当实测堆进图表）；**agent 自报费用优先**（Grok `costUsdTicks` 记相邻轮差值；其 token 不再叠牌价）+ **DeepSeek 高峰时段按牌价 ×2**（北京时间工作日 9–12/14–18，仅 DeepSeek 自家牌价的模型，不含法定节假日、略偏高——文案注明）；压缩事件吃 ACP `compaction_update`（能力早已声明）；agent 自续跑（无 turn 边界的续段）按「上一轮记账时刻」去重防重复折叠；失败/中断轮按 turn/complete 停止原因分类。**存放位置**：桌面版 `~/.marionette/usage/`（`usage_store.rs`：`ledger.json` 乐观 rev 写 + `sessions/<id>.jsonl` 按轮行；写失败记 dev.log 并在下一轮重试，localStorage 配额问题成为历史，首次启动自动迁移旧 localStorage 账本）；浏览器预览退 localStorage。牌价折算（models.dev 目录 24h 缓存、厂商自家价优先、未计价 token 标覆盖率；CSP 仅放行 `https://models.dev`；文案「API 牌价折算，非订阅实付」）。**工具栏「导出 CSV」**：long format（`scope=total` 日合计 + `scope=model` 模型拆分），原生另存为，成功 toast 可一键打开所在文件夹 |
| 登录 | 多 agent probe + Sign in 横幅 |
| **Telegram 远程** | 左下角纸飞机图标：@BotFather 建个 bot 粘 token（getMe 校验，DPAPI 落 `~/.marionette/telegram.json`）→ 给 bot 发条消息在浮层确认配对（只认这一个 chat）。会话跑完/等你批准（权限、ask、plan）/出错/进程死亡推送到手机；**跑完推送带全文正文，MarkdownV2 格式（代码块/粗体/链接），长文自动分片**；**回复里的图随正文之后发出（≤4 张保最新）**——agent 读到/产生的图（image 块含 base64 截图、工具 locations/rawInput 指向的图片文件、正文 markdown 图片）按「UI 里看得到的图都带得出去」口径收集，本地文件/内联字节手写 multipart 上传（照片被拒退文件发送）、远程 URL 直传，用户自己附的图不推回；受「带内容」偏好门控，回复一张图也能路由到会话；权限/ask/plan 的通知带内联按钮，远程解答与 UI 共享同一 waiter（先答先得）；在 Telegram 里回复文字 = 送进对应会话（引用回复定向，裸文本去最近活跃会话），走与本地输入完全相同的发送管线（挂起重连/历史注入/排队）；权限等待 120s 到期前 30s 追加提醒。**置顶「总控」面板**：谁在跑/谁在等你实时重画（忙时每 60s 保鲜），按钮点会话名 = 切换裸发文字去向，⏹ = 远程停止（与 UI 停止键同路），`/status` 拉全量清单。通知偏好（跑完/等待/出错/带内容）在浮层开关。token 校验、配对轮询和消息发送统一跟随应用代理，设置变化在下次请求生效，无需 TUN。纯出站 HTTPS，零新依赖；未配对时零行为变化。详见 [`telegram-bridge.md`](./plans/telegram-bridge.md) |
| **贴边停靠（edge dock）** | 主窗可并入屏幕左右边缘（`dockMode.ts` 三态：floating / docked-hidden / docked-expanded）：**把普通窗口拖到屏幕左右边缘会自动吸附**（离边约 20 逻辑 px 停住，边缘亮一条青色提示，松手后贴边并保持展开；**抓住窄条顶部往外拖 >48 逻辑 px 即解除贴边**，窗口留在松手处并恢复普通尺寸，不再弹回贴边前的位置；也可从 Ctrl K 命令面板执行贴边/解除）。停好后整条移出屏外只留 6px 淡轨 + 中间一根青色把手（有对话在跑时把手呼吸），鼠标触碰滑出（~160ms 9 步），离开 300ms 且失焦收回（⋯ 菜单开「**侧边栏常驻**」后不再自动收回，手动收回/解除贴边照旧）；**把文件或文字拖到把手上也会滑出**（OLE 拖拽期间鼠标事件被抑制，展开信号改听 tao 的 onDragDropEvent 与 wry 补丁的 drag-text 通道）；滑出后整条分区接住拖放：**上半条（标题/额度/进行中）= 收入收纳盒，下半条对话区 = 进输入框成附件**（拖的是文字时下半条落进输入框光标处成文本，同一条 `marionette:dock-drop-text` 转交链路），拖动中双层虚线提示标出两个落点、悬停侧点亮（收纳盒整页开着时，App 级路由器整体让路——收纳盒页自己的监听接住**整窗**拖放；其它整页或无对话在开时全部进收纳盒）；弹出只挪窗不抢焦点，全屏视频/演示模式不弹。关闭主窗 = 贴边收起（引擎、全部会话、Telegram 桥继续跑；`autoHideOnClose` 默认开，可在 Ctrl K 切换），真正退出走窄条 ⋯ 菜单或 Ctrl K 的「退出 Marionette」（`quit_app`）；再次启动 Marionette 唤起。窄条（默认 400，**贴边展开时抓内缘可拖宽 320–560**，松手记住宽度）自上而下：标题栏（拖动区 · 搜索=Ctrl K · ⋯ 菜单收纳 侧边栏常驻/用量与项目/升级/解除贴边/退出；**右上角不再单独放用量图标**）→ 操作行（**新建对话点开是目的地菜单**：聊天（默认文件夹）/ 各项目（在所选项目里开新会话）/ 全部对话（进完整列表用每个项目的 ＋），不再固定建进聊天；旁为收纳盒）→ **额度**（每个订阅一张紧凑计量卡：名称独占一行 + 一条 3px 进度条 + 最紧窗口百分比；纯余额来源单独一行做成「余额 + 金额」chip，不再画空心圈；悬停看全部窗口与重置时间，点开用量页）→ 「进行中」列表（仍持有进程的对话 + 当前对话，取代标签行，超过 5 行滚动，悬停可挂起；**完成（waiting）的对话不退场，一直留到真正挂起——2 小时自动挂起 / 行内 ⏸ 手动挂起 / 进程退出——才从列表消失**；只影响这个列表的显示，全部对话里完成徽标还在）→ 对话与输入框（**贴边时工具栏收成单个入口**：滑杆图标 + 当前模型名，点开「模式/权限/模型/Agent」分组菜单，选一项再进原有完整列表；＋ 附文件与发送钮保留在外，不再是一排会换行的芯片；**composer 在贴边时默认按内容自动高度**（跟着正文增长、有上限），不再沿用普通窗口记住的 104/112px；**顶边拖拽把手在贴边态保留：上下拖动即钉住高度（84–340px，单独记住，双击把手恢复自动高度）**，普通窗口的高度记忆互不影响，42vh 展开按钮仍只在普通窗口——底边直接压在底栏上沿，附图时向上长高，工具按钮不再被挤下去）→ **底栏**（输入框下面单独一栏：模型共享入口 + 设置齿轮（打开 Ctrl K）+ 主题/通知/Telegram + 右侧检查更新；语言与代理收进 Ctrl K。由两条侧栏把各自的 footer 子树 portal 进来，见 `src/lib/dockFooter.ts`）；**composer 的所有下拉（模型/agent/mode/权限/effort/发送）在打开后会按视口 8px 边距夹回屏内**（400px 窄条里右对齐的模型菜单原先会从左边溢出被 WebView 裁掉，看起来像整张列表消失）；**窄条标题栏的 ⋯ 菜单与「新建对话」目的地菜单 portal 到 body 层**（`.dock-chrome` 自带层叠上下文，菜单挂在里面永远压在整页覆盖层下面，表现为「点了没反应」），按锚点几何现算位置并夹进视口；**贴边态弹出的对话框（Add project / 设置 / 权限卡 / 确认框）同样在这一层之上**，不会藏到侧栏整页后面。全部对话 / 用量与项目 / 收纳盒是整页切换，标题栏变成「‹ 返回 + 页名」，Esc 或返回回到主条，从全部对话里选中对话也自动返回；窗口按钮在条内隐藏，分屏停靠时暂停。**收纳盒**（`~/.marionette/memos/`）：收纳盒页开着时**整窗都是落点**——资源管理器拖到窗口任意位置（含侧栏、标签行）即收，不再只认页面本体的矩形（内容哈希去重、复制落盘）、**从其它应用拖入文字/链接也整窗即收**（浏览器选区、链接、图片 URL——裸 URL 自动成链接条目；走 vendor wry 补丁的 `marionette-drag-text` 通道，与文件拖放互不干扰；普通窗口里把文字/链接拖到输入框上则直接插入光标处）、Ctrl+V 收文字/链接、**顶上「写一条」输入卡**（Ctrl+Enter 保存，裸 URL 自动成链接）、搜索与类型筛选、条目按时间倒序铺成**单列帖子卡**（文字卡铺 6 行正文点开展开、图片卡按横竖比完整显示不裁切、链接卡域名+网址、文件卡扁卡；按「今天 / 昨天 / N月N日」分段）、文件条目可直接拖进输入框成附件；**任何卡片都可拖进输入框（2026-11-18）**：文件/图片成附件，文字/链接条目落光标处成正文；**卡上操作区**：编辑（文字/链接改标题+正文，文件/图片改标题）是卡片右上角的常驻铅笔图标；标签（每条目 ≤8 个标签，筛选行的标签芯片多选=并集，搜索也匹配标签）从卡底标签行末尾的「＋ 标签」新建；「更多」只放置顶（置顶条目浮到顶部独立「置顶」组，不占日分组）与删除；**输入框「存入收纳盒」按钮**（＋ 旁，紧凑 hub 同款入口）把当前草稿与图片附件存副本进盒（不清空输入框），收纳盒开着即时刷新；编辑/置顶/标签走 `memo_update`（原地更新不重排信息流）。点卡片左侧常驻复选框或 Ctrl 点击多选后可「开新对话」（一律在左侧栏「聊天」栏目下方的「收纳盒」分组里建出新对话，材料清单预填进输入框）或「整理成文档」（预填让 agent 按主题整理并存到项目 `docs/notes/<日期>-<主题>.md` 的指令；开着项目建进该项目，没开项目同样落收纳盒分组），都只预填不自动发送。收纳盒分组复用聊天行（挂起/删除/⋯ 里置顶·重命名·导出），有对话才显示。**普通窗口的收纳盒入口位于左侧栏「聊天」栏目正上方**，点开后在标签栏尾部出现一枚「收纳盒」伪标签（带条目数徽标），与其他对话标签并列；中栏只切换对话流那一列（左右侧栏照常），会话 composer 行藏掉，页内顶上是「写一条」卡。选中收纳盒时只有它是当前页（对话标签退成未选中，当前会话仍记着）；选中任一对话标签时收纳盒标签保留、页面切回对话；× 或 Esc 关闭标签（分屏态同样支持）。聊天内容上方不再有「收纳盒 / 能力与上下文」工具栏，分屏不重复入口。实施与红线见 `AGENTS.md`「贴边停靠」节，设计稿 `docs/plans/plan-edge-dock.md` |
| **跨用（模型/key/订阅互通）** | 见下节 |
| **圆桌会议（Council）** | 侧栏「圆桌」/ Ctrl+K「打开圆桌会议」进入（标签行出「圆桌」伪标签，与收纳盒同语义）：多个模型独立提案 → 交叉质询 → 修订 → 主席综合，产出经评审的结构化实施计划（Plan Gate 等待用户裁决）。召集页可套阵容模板（标准圆桌 / 身份局 / 垂帘听政 / 科研研讨）或 AI 组队（画像 → 可选探针 → 审阅后填席位）；人卡分主导/协办，协办产出须主导背书才进综合。模型来自共享引擎目录（同模型可配多个成员，角色独立）；独立提案阶段互相不可见（编排器构造输入时强制隔离，非提示词约定）；分歧不靠投票，裁决不了进「遗留不确定性」并附消解实验。run 可暂停/取消/崩溃恢复（已完成的阶段不重调模型）；请求数/token/墙钟三层预算，触顶即停；全部消息与状态转移 append-only 落盘（`~/.marionette/council/runs/`），可审计。M1 为「仅审议」全流程：计划批准即交付完成，自动执行（agent 落码）在后续版本接入——不伪造 Executing 状态。设计文档见 [`council/`](./council/)，架构定稿 [`council/architecture.md`](./council/architecture.md) |

## 桌面提醒（2026-11-17）

ACP 的回复完成、回合错误、权限、提问和计划审批由 Rust 事件调度器直接播放原生 WAV 并点亮任务栏，**不依赖前端定时器或 WebView2 是否已恢复**；最小化、后台常驻和普通失焦都立即提醒。前端仍负责通知开关与纯前端信号（卡顿、发送失败、浏览器 handoff 等）；探测回合、静默回合、Council 执行和 Chief 探针会显式抑制后端提醒，避免内部回合误响。实现见 `src-tauri/src/desktop_notify.rs`。

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
- **没有全局眼睛开关**：对话流永远按运行段折叠。单张 thought / 工具卡仍可点开；`marionette-details-visible` 旧值忽略。

## ChatGPT 网页桥的原排版回复

ChatGPT 网页版与 DeepSeek 网页版是内置 agent，新建会话可直接选择，无需注册脚本。
桥源码（`tools/web-chat-bridge`）通过 `web_chat_bridge.rs` 编译进 exe，便携版同样携带；启动会话时按内容哈希自动释放到 `~/.marionette/bundled/web-chat-bridge/`，不依赖源码目录。需要 Node.js 22+ 与 Edge / Chrome，首次使用登录网页。旧版同 id 自定义条目不重复展示，恢复会话也使用内置桥。

ChatGPT 网页桥复制本轮回复的 HTML 与原站 CSS，在 Reply 卡片的 Shadow DOM 中显示，保留表格、代码高亮、卡片和网格图；字体与中性色适配 Marionette 主题，整页纵向滚动，宽内容可横向滚动。HTML 与纯文本一起流式更新、保存和恢复；纯文本继续用于复制、搜索和历史上下文。CSS 与可导出的图片按内容哈希缓存在 `BRIDGE_STATE_DIR/rich-replies/`；缓存缺失则回退文字。链接、代码复制和图片放大由壳接管，原站引用浮窗等脚本交互不迁移。旧记录不自动补 HTML。首次升级需重新构建前端与 Rust。

DeepSeek 正文按网页语义转成 Markdown，保留标题、强调、嵌套列表、引用、链接、表格与代码块，沿用 Marionette 的字体和主题；公式暂显示 TeX 文本。流式更新使用 `marionette.textSnapshot` 完整正文快照，明确替换当前未封口回复，支持网站回写格式和删改短文本；历史仍保存普通 Markdown。思考区继续使用独立思考事件。Telegram 转发同步按快照替换，避免重复累加。首次升级需重新构建前端与 Rust 并重启，旧记录不自动补排版。

## 富文本渲染（公式 / 流程图 / 代码高亮 / 生成式卡片，2026-10-26）

对话 Markdown 在主流 GFM 之上按内容懒加载三个重引擎，全部同一条管线（`MarkdownBody.tsx` + `richMarkdown.ts` 的纯字符串扫描决定是否加载）：

- **公式（KaTeX）**：消息里出现围栏外的 `$$` 或 LaTeX `\[ … \]` 才加载 remark-math + rehype-katex。单 `$` 永远是字面文本（`$HOME`、`$9.99` 不误判）；`\[ … \]` 自动改写成 `$$`；整行只有一对 `$$ … $$` 的自动转成块级公式（micromark 的 flow math 要求 `$$` 独占行，单行写法原本只会渲染成行内大小）。围栏内的 `$$`（如 bash 的进程号）原样不动。
- **代码高亮（Shiki）**：带语言标注的围栏代码块在文字安静 200ms 后原地换成高亮（双主题，深/浅主题切换只换 CSS 变量）；流式期间保持纯文本外观，虚拟列表滚动重挂载不重新高亮（缓存 120 块）。原有语言标签、行数、复制、长块折叠全部保留。
- **流程图（Mermaid）**：` ```mermaid ` 围栏出图。流式期间先 `parse` 再渲染——语法不完整时安静地显示源码等它写完，不闪语法错误；改写源码时旧图保留到新图就绪。深/浅主题跟随切换，`securityLevel: strict`。
- **生成式卡片（` ```spec ` 围栏，research-codeg.md B4）**：agent 在回复里写一段 JSON 围栏，聊天里直接出交互卡片——`card`（字段清单）/ `table`（支持单元格着色 ok/warn/err/dim）/ `steps`（done/active/todo/error 步骤）/ `chart`（bar/line/pie，零依赖手绘 SVG/CSS）。`actions` 按钮把预置 prompt 填进输入框（只预填，绝不自动发送）。三道解析防护（JSON 容错、原型污染/深度上限、逐字段形状校验 + 尺寸上限）加 React 错误边界，坏围栏只降级成「等待数据完整」的安静卡片，绝不下翻整个对话流。

全部引擎懒加载（mermaid ~686KB / katex ~262KB / 各语言 grammar 独立 chunk，命中才拉）。任何一步失败都退回纯 Markdown 管线。回归：`npx tsx scripts/verify-rich-markdown.mts`。

## 再次进入对话（agent re-entry，2026-10-25）

Agent 在回合结束后自己续跑（它起的后台指令回来了，触发新模型调用，没有新的用户消息）：对所有 ACP 走同一条通用线（`src/lib/agentReentry.ts`）。会话空闲（`turn/complete` 已到）超过 2.5s 后再收到 assistant/thought 流内容，就视为再次进入对话——会话重新显示为运行中（Working 条、侧栏脉冲、实时轨迹图回来），续跑内容另起一张 Reply 卡。收尾两条路：适配器随后发了 `turn/complete` 就按正常回合收尾；没发的（如 zcode）静默 6 分钟后封口尾卡（补时长）、状态回空闲并像跑完一样通知/记未读；之后再续跑再武装新段。用户发消息随时接管正常回合生命周期。回归：`npx tsx scripts/verify-agent-reentry.mts`。

## 跨用引擎（2026-10 定稿形态）

目标：**用户无脑打开软件就能用**——任何一个来源的模型，任何一个 agent 的会话里直接选。

- **跨用模型选择 fail-closed（2026-11-18）**：会话绑定保存引擎裸 id，真正写入 agent 时由 `lib/bridgeRouting.ts` 转成各 agent 原生 `marionette…` id；桥芯片的 stale 判定只看 live ACP caps，不被磁盘偏好覆盖。若 agent 落回原生默认、目录没有注入 provider，或发送前模型同步失败，界面显示实际模型并中止本轮 prompt，避免「芯片写 Ollama DeepSeek、账单走 OpenRouter GPT」的静默串源。回归：`npx tsx scripts/verify-bridge-routing.mts`。
- **key 型直连**（OpenCode Go / DeepSeek / GLM / Kimi / OpenRouter / xAI）：claude-code 走 spawn env、grok 走 owned 配置表，不过网关；来源含凭据库手存的 key 和 opencode `auth.json` 的只读扫描引用（引用只在直连注入瞬间读钥匙，不复制）。
- **订阅型 + codex 全场景**：默认经 `engine-rust` **进程内模型共享引擎**（`src-tauri/src/engine_gateway/`）；它只兼容 Magpie 配置格式，不再启动 Go 子进程。`MARIONETTE_ENGINE=magpie` 只保留一个发布周期的构建回退。引擎仍**懒启动（2026-11-06）**：随应用启动只做目录/清障，不启动网关——**存在引擎模型绑定或跨用入口才自动拉起线程**，纯 GPT/键 key 直连用户引擎零监听。按需唤醒入口：网关按钮、绑定共享模型（最多等 20s）、启动已接桥的 agent 会话（dsh/pi/zcode/opencode）；30 分钟无绑定且无需求自动停线程。key-plan 与扫描引用的 api key 自动登记进引擎；Grok / ChatGPT / Claude 订阅走只读登录桥。codex 只接引擎源（协议约束），经 `providers/set` 改道 + 路由组把内建模型名别名到所选引擎模型；spawn 时再把目录里的真实 `context_window` 合并进 `CODEX_CONFIG.model_context_window`，避免 Codex 继续按内建窗口提前压缩。
- **引擎模型目录与配额的离线快照（2026-11-06）**：引擎不在 Running 时模型菜单的 marionette 组回放 `~/.marionette/engine-catalog.json`（live 非空时自动回写；note 标「引擎未运行 · 离线目录」，离线选中会经绑定链自动拉起引擎）；右栏额度卡的引擎配额行保留上次快照并标注「引擎未运行 · 快照时间」（localStorage `marionette-quota`，7 天过期）——引擎睡眠/唤醒不再让菜单组与配额行整组消失/插回。纯浏览菜单**不再唤醒引擎**（仅存在绑定时才等 5s 唤醒）。引擎目录只列**具体来源**的模型：`group/` 与 `auto/` 路由组一律隐藏（2026-11-06 起 auto 与 group 同罪；存量 auto 绑定启动时清绑，受影响会话回落 agent 原生默认）。
- **DSH（@openma/deepseek-harness-acp）跨用（2026-10-20）**：spawn 时把 llm-pi-ai 托管路由 `marionette`（openai-completions → 引擎 `/v1`）写进 `~/.dsh/cordis.patch.yml`（文本外科手术、幂等、保用户内容），令牌常量进 `~/.dsh/.credentials.yaml` refs + spawn env；模型经 dsh-acp 原生目录广告（id `marionette::provider/model`）。引擎停用即移除路由；home patch 被 dsh Web/desktop 共享，Web 里也能走这条路由。写入器在 `dsh_bridge.rs`，调研定稿见 `research-dsh-cross-use.md` §8。
- **pi（pi-acp）跨用（2026-10-20）**：spawn 时把 provider `marionette` 合并进 `~/.pi/agent/models.json`（pi 官方兼容端点机制，`apiKey` 用 `$MARIONETTE_ENGINE_TOKEN` env 插值、凭据不落盘；语义幂等、保用户 provider）；模型经 pi-acp 原生目录广告（id `marionette/<engineId>`，与 grok/opencode 前缀同形），`set_config_option` 直接切。引擎停用即移除托管段。写入器在 `model_bridge.rs`（`refresh_pi_marionette_route`）。
- **Roster 收束（2026-10-20；2026-11-09 hermes 移除）**：内置 agent 收为「五跨用（claude-code / grok / codex / opencode / deepseek）+ **pi + zcode**」；cline / gemini / kimi / codebuddy / openclaw / cursor / omp 退出内置名单（cursor 封闭无口子、gemini 协议锁死，其余用户面小——可接性矩阵见 `research-acp-cross-use-matrix.md`）。退出者可经「自定义 agent」随时加回；历史会话导入的 parsers 与登录探针保留未动。hermes 则于 2026-11-09 **全量移除**（用户定案：不再支持）——roster 条目、harness_meta、auth 探针/登录、uvx 预检（`DistributionKind::Uvx` 随之删除）、会话导入 parser 一并清掉；原计划的跨用桥（config.yaml providers）不再做。**新 agent 的加入原则（2026-11-09 定案）：等阵营收敛、有人真在用再接**（2026-11-19 例外：antigravity 由用户点名接入——Google 官方 first-party ACP server + Google 账号免费档，见下节）——agent 界正处在标准混战期（ACP 官方目录已 48 家），别为「在目录里」就抢着内置；真出线了从 ACP registry / 自定义 agent 加入都是小改动。
- **Grok × 智谱 Coding Plan（2026-10-27）**：Grok 自建的 `zhipu-*` 表若拿的是 Coding Plan 钥匙，spawn 时把 `base_url` 从按量 `https://open.bigmodel.cn/api/paas/v4` 改到 `https://open.bigmodel.cn/api/coding/paas/v4`（`rewrite_grok_zhipu_coding_plan_bases`）。按量钥匙与 `$ENV` 展开的自建表不动。Grok 只在进程启动读 `config.toml`，改完要新开会话。证据链见 [`pitfalls-model-share-quota.md`](./pitfalls/pitfalls-model-share-quota.md)。
- **一种便携版**（`tools\build-portable.bat` → `release\<版本号>\Marionette.exe`）：默认把 Rust 网关直接编进 exe，不下载、不落位第二个引擎进程；打包也不需要 Go/Magpie 载荷。一个发布周期的回退用 `MARIONETTE_ENGINE=magpie`，此时才走 `import-magpie.ps1` + 精简种子载荷。发布统一用 `tools\publish-release.bat`。
- **引擎形态切换（运行时逃生口，2026-11-19）**：默认内置 Rust 引擎；模型共享面板新增「引擎形态」——可切到 **magpie 子进程形态**（面板的「检查更新/更新」即从 GitHub 下载安装 magpie，与旧版一致），随时切回。偏好存 `~/.marionette/engine-preference.json`（一行 `rust`/`magpie`，读失败按 rust）；切换 = 写偏好 + 停当前网关 + 按新形态拉起。偏好 magpie 但未下载时引擎保持未运行（绝不偷偷用另一个引擎顶包），面板引导下载或切回。serve/唤醒/CLI/检查更新/下载共八处按偏好运行时分流；magpie 回退 SKU 构建行为不变。
- **引擎版本**：Rust 引擎随 `marionette.exe` 发布和更新；默认形态下面板不出现 Magpie 下载/恢复出厂入口（切到 magpie 形态后恢复，恢复出厂仍仅内嵌种子构建可用）。`~/.marionette/engine/data/` 继续保存 Magpie 兼容的 `providers.json`、登录桥缓存和各引擎账本；配置损坏时编辑 fail-closed，`providers.json.bak` 保留最后正确版本。Magpie 回退 SKU 才保留旧的 3425 端口复用、下载和恢复出厂路径。
- **额度行（2026-11-22）**：同一把钥匙被登记成多个来源（预设 + 从各 CLI 配置发现的）时只显示一个额度圈；Claude 订阅额度经本机 `claude -p /usage` 读取，任何对话里都可见（首次读得慢时下一次刷新补上）。
- **中转安全批（2026-11-22）**：四件事一批——① Claude 订阅桥的 CLI 解析修复：对话桥与 /usage 共用主程序注入的可靠查找（.exe 优先、npm 垫片解包成 exe / node+cli.js、空格与中文路径安全），spawn 不再经 `cmd /C` 重拼接（「找不到 Claude，请安装」误报的根因）。② 模型名注入闸：`claude/*` 的模型名只放行字母/数字与 `._:+-`（首字符必须字母数字），空格、引号、`&|<>^%` 等 cmd 元字符一律 400。③ 网关 Bearer 令牌：`~/.marionette/engine/serve-token.txt` 每安装一枚（引擎回收再唤醒不换，活会话注入不失效）；除 CORS preflight 外所有端点要求 `Authorization: Bearer <令牌>`，本机其它进程与浏览器插件既读不到模型列表、也用不了订阅额度；magpie 子进程回退形态维持旧字面量（Go 网关本就任意 token 放行）。④ 订阅共享默认关：**claude（Claude 订阅）/ codex（ChatGPT 订阅）/ grok / gemini 四族默认不共享到其他 agent**——菜单/注入（engine.rs 目录过滤，唯一过滤点）与网关执行闸（关 = 直连请求也与未安装无异）三层一致；模型共享面板左栏新增「订阅共享」区块，逐族开启且开启前有账号/条款风险警告；DeepSeek 等按量来源不受影响、默认可用；额度展示行不受影响；厂商自家 agent 会话（如 Claude Code）即使开了共享也看不到本族桥组（原生模型已覆盖，不再「两个都显示」）。
- **入口**：左下角互联图标 = 模型共享状态/一键启动；凭据管理退居其浮层底部链接。用量面板点刷新会拉网关 `/v1/magpie/quotas`（各家订阅额度）。
- **模型目录与显示管理**：统一按来源分组原生和共享模型，只有「接口主机 + 凭证」确认同一账号的入口才合并（agent 自带 provider、Grok 的 `[model.*]` 表、引擎目录会并成一组，标题用 agent 的厂商名）；订阅登录态（Codex 的 ChatGPT、Grok 的 xAI）同样只算一个账号——拥有登录文件的 agent 整份目录并入引擎分组，另一个 agent 复制的同一 OAuth 登录（OpenCode 的 `openai`）按 `accountId` 判重后不再单列；模型名相同但主机或凭证不同的条目保留。合并前保存的隐藏与排序按旧标识迁移。模型菜单底部的「配置模型与来源…」与模型共享浮层里的「模型与来源 · 显示和排序…」都打开这一页，可分别隐藏 provider / 模型、调整 provider / 组内模型顺序、恢复默认；大来源默认收起、排序箭头悬停才出现；保存到 `~/.marionette/model-library.json`，多个窗口同步，隐藏不影响已有会话。扫描 Grok / Pi / OpenCode / Codex / Claude 的用户自定义接口，保留原配置引用，经引擎供其他已接桥 agent 使用；扫描位置与失败原因可在凭据页查看。「添加密钥」支持自定义 Chat Completions / Responses / Anthropic 接口和显式模型 ID。范围与使用说明见 [模型与来源](./guides/model-library.md)。
- **卫生**：删除凭据条目会从引擎回收对应 provider（注册账本对账）；30 分钟无绑定且无需求的空闲引擎自动停线程，任何跨用入口（菜单/绑定/按钮/启动）自动唤醒。Rust 账本独立写 `engine-usage.jsonl`，首次打开只迁移旧文件里能严格识别的 Rust 行，不触碰 Magpie 的 `usage.jsonl`。
- 安全底线不变：明文 key 只进子进程 env / owned-section / 进程内登记链，绝不进日志与 agent 配置；扫描只读；外站网页 Origin 调本地网关返回 403；网关全部端点要求 Bearer 令牌（2026-11-22 起，令牌按安装持久、随注入下发）。

## Google Antigravity agent（2026-11-19，M0–M3 落地）

Google 官方 `agy_acp_server`（dl.google.com 二进制 zip，非 npm）作为内置 agent（id `antigravity`）。
完整事实与实测：`docs/research/research-antigravity.md`；方案与偏差：`docs/plans/plan-antigravity.md`。

- **安装**：agent 菜单一键下载（registry 版本优先 + sha256 校验，离线回退 pin 1.5.0），
  落 `~/.marionette/agents/antigravity-acp/<version>/<platform>/`；`agy_acp_server.exe` 与
  `localharness_external.exe` 必须**同目录成对**（缺后者=握手正常但 agent 死，安装与读侧
  双重校验）。spawn 优先安装目录，PATH 手装其次。解压走系统 `tar.exe`（回退 PowerShell），
  **不加 zip crate**（Cargo.toml 红线）。
- **认证**：spawn 前把 `auth.type:"oauth-personal"`（免费档默认）同步进
  `<GEMINI_HOME>/antigravity-acp/settings.json`（单 key RMW、strict JSON fail-closed）；
  `GEMINI_HOME` 指 `.gemini` 目录**本身**（与 Gemini CLI 的 `GEMINI_CLI_HOME` 相反），未设时
  默认 `~/.gemini`。settings 写好后 `session/new` 直接成功，OAuth 由 server 自己触发（弹浏览器 +
  stderr 打授权 URL）；浏览器没弹时对话里出现「等待 Google 授权」通知卡兜底（URL 可选中复制）。
  登录探测 = `acp_token.json` 存在性。
- **scratch 红线**：Windows 构建是 PyInstaller onefile，**每次启动解压 ~100MB 进 %TEMP% 且杀树
  不清**（1.5.0 实测 ~90-150MB/次）——spawn 时 `TMP/TEMP/TMPDIR` 重定向进
  `~/.marionette/scratch/antigravity/<sessionId>`，stop/stdout-EOF 两处树死回收 + 启动清扫孤儿。
  **只对此 agent 开**（借走 TMP 会污染用户 Git Bash 的 /tmp）。
- **探测防执行**：`installed_version`/`--version` 类探测对 `agy_acp_server` **永不执行**
  （stem 精确匹配，大小写/路径/可选 .exe 归一）——连 `--version` 都先整包解压。版本显示来自
  安装目录名；升级链查 ACP registry（`cdn.agentclientprotocol.com`），URL 模板不符（Google 改名）
  = 不提供升级。
- **usage 回填 + 历史导入**：`parsers/antigravity.rs`（prost 手写 protobuf 子集，字段号对
  1.5.0 实测钉死：type 14=user/15=planner、metadata 1=created/9=usage/24=model）+
  `usage_backfill/antigravity.rs`（db 文件名即 ACP sessionId，去重锚天然对上；usage 四桶
  input/output/cache_write/cache_read/thought 与 ACP 侧实测吻合）。0 字节库=正常空会话。
- **turn 边界**：不发 `turn/complete` 通知，但 `session/prompt` 响应自带 stopReason+usage——
  走 acp.rs 既有「prompt 响应即回合完成」路径，回合/记账正常，无需 re-entry 兜底。
- **问答卡未接**：agy 的 `ask_question` 走私有 `session/request_permission` + `interaction_<hex>`
  （codeg 用 `try_bridge_antigravity_ask` 桥成问答卡）；Marionette 没有这条桥，提问目前会落到
  权限卡上，选项被当成批准按钮。记录在案（2026-11-22），未排期——接的时候照 codeg 的形状做。
  各家 agent 的问答通道对照见 `docs/research/research-ask-card-channels.md`。
- **存量回收**：`antigravity_scan_leaked_temp` / `antigravity_reclaim_leaked_temp` 命令
  （15 分钟年龄 + CreateFileW share-none 非变异存活探测 + 重校验），诊断清单报数量/体积。
- 回归：`cargo test --bin marionette antigravity`（15 项 + 2 个 #[ignore] 真库/真网 oracle）。

## 问答卡（Agent 提问，2026-11-22 通道矩阵落地）

问答卡只有两个来源：Grok 的 `x.ai/ask_user_question`，以及 ACP `elicitation/create`（form）。
卡片形态**按 agent 自己声明的能力**渲染，不按 agent 名字写死：

- **其他（替代答案）**：有自由文本答案通道才显示。Grok（任意字符串）、Codex
  （`_meta.codex.isOther`）、任何 `{id}_custom` / `_other` / `__other` 伴生字段。
- **补充（附注）**：只有独立附注通道才显示。Codex 的 `{id}_note`（title
  "Additional answer or note"）、Claude 单选的 `_custom`（有 pick 时进
  `annotations[q].notes`）。OpenCode / ZCode / DSH 只有一个自由文本槽，不显示补充。
- **纯文本题**只有一个「答案」输入框；**严格 enum**（Codex `isOther:false`）只有选项。
- 单选再点一次取消；「其他」与选项互斥（多选也一样），「补充」纯附加不参与互斥。
- 协议哨兵不渲染：Codex 的 `None of the above`、ZCode 的 `__skip__`。
- Grok 多选发**数组**（不是 `", "` 拼接）；补充走 `annotations[q].notes`（不带空 preview）。
- 没有问答卡的 agent：Pi（只实现 `session/request_permission`）。Antigravity 的
  `ask_question` 仍落在权限卡（桥未接，见其专节）。

完整通道矩阵与真机探针证据：`docs/research/research-ask-card-channels.md`。
回归：`cargo test --bin marionette elicitation`（含 Codex `_note` 不是第二题、
Claude 的 pick+notes 分流、ZCode 哨兵过滤）+ `npx tsc --noEmit`。

## 交付卡（每轮改动的口径）

卡上的文件列表 = **本轮窗口内的真实内容差异**，不是工作区状态。

- **基线**：`send` 时用私有 git index（复制用户 `.git/index` 当 stat 缓存 + 私有 object dir/alternates）给整个 worktree 拍一棵内容树（`git_service::begin_workspace_snapshot`，实测本仓库 0.6–1.2s；裸 `read-tree`+`add --all` 要 8.6s，所以必须 seed）。turn 末再拍一次，`diff-tree -r -M --name-status/--numstat` 出每文件增删——`+/-` 是真数字，不再靠工具 card 里的 diff 行数猜。
- **为什么不用 `git status` / `git diff HEAD`**：二者口径在本仓库实测分叉——`status -uall` 报 224~227 条，真实内容差异只有 52 条（`third_party/magpie/**` 这类 `text=auto`/CRLF 文件 stat-dirty 但 diff 0 行），于是「按 status 列文件、按 diff 算指纹」会给 186 个文件留空指纹，任何比对都可能把它们当成新改动。
- **谁算本轮的**：起点干净、结束脏 → 无条件算（工具、子代理、`npm run build` 都认）；**起点就脏**的文件只有被本轮自己的 edit/write 工具碰过才算，否则同一个工作区里另一场对话（或用户的编辑器）的改动会串进这张卡。没有 HEAD 时无法拆分，宁可全留。
- **fail closed**：基线拍失败/不是 git 仓库 → 不发布任何 git 真相，卡片只留本轮工具账本。旧实现 `catch { return [] }` 把失败当「基线是空的」，于是整库脏文件全进了卡——这就是「交付卡显示全仓库改动」的直接成因（2026-10-23 修）。
- 子目录项目（项目根 ≠ 仓库根）的路径按**项目根**相对返回（`rev-parse --show-prefix` 剥掉），与 `getFileDiff` 的解析口径一致。
- **一行一个文件**：卡片合并的键是规范化路径（正斜杠、去掉 cwd 前缀、小写），不是原始字符串——git 报的是工作区相对路径（`src/lib/api.ts`），工具调用里带的是 agent 原样给的绝对路径（`D:\Projects\Marionette\src\lib\api.ts`），按原串合并会把每个被工具改过的文件列两遍（2026-10-23 修，回归用例在 `scripts/verify-turn-delivery.mts`）。git 报出的相对路径优先用于行标题、diff 请求与撤销。

## 内存压力韧性（2026-10-31）

外部基准测试打满内存的缺页风暴里，浏览器类应用存活而 Marionette 前端失去响应——
这一批把关键路径从 O(文件)/O(n) 改成有界：

- **大会话秒开**：打开对话只装载转录末尾 800 行，更早的历史在顶部「加载更早的消息」
  按需翻页（此前 24MB 会话 = 一次性全量加载 + 全量解析）。
- **流式落盘不再全量重写**：纯新增的事件直接追加到转录文件尾（此前流式期间每 450ms
  把整个文件重写一遍）；写盘并发冲突（临时文件互吞）有进程级写锁 + 重试 + 兜底。
- **窗口永远关得上**：后台模式「关窗 = 隐藏」的交接带 2 秒兜底——前端 2 秒内没应答
  就由后端直接隐藏窗口，Alt+F4 / 任务栏关闭重新成为可靠逃生门。
- **启动不卡 2 分钟**：模型引擎的 provider 注册/来源刷新移出会话启动关键路径并全部
  带超时；种子引擎短路 TUI/quota 等完整 CLI，自定义来源走种子自带的 `provider` 命令；菜单打开不再触发网络刷新排队。
- **孤儿引擎自动收割**：上一代实例异常死亡留下的引擎进程（父进程已死、镜像在我们
  目录下）在下次启动时回收，不再占着默认端口漂几个小时。
- **可诊断性**：前端每 60 秒记一条 ui-heartbeat，window error / unhandledrejection / React 根边界写入 ui-* 日志，并把最近一条暂存在 sessionStorage，重载后仍可进报告；测试运行不再污染真实 dev.log。Ctrl K 的「导出诊断报告」会把最近四代日志（总量上限 8 MB）、App/系统/WebView/当前会话与 agent 快照、最近错误索引和机器可读 manifest 写成一个 Markdown 附件；常见密钥、Authorization、token 和用户目录在保存前脱敏，不复制完整对话、项目文件、凭据库或进程环境块。字段与提交流程见 guides/diagnostics.md。

## 文档标签页（2026-11-18 起为分屏树窗格标签）

`.md` / `.markdown` / `.mdx` / `.txt` 点击后在 app 内打开：文档是分屏树里的一枚窗格标签
（`document:<绝对路径>`），与会话标签同条同拖拽。单格时它就是完整标签页，主标题栏继续
显示会话 / 文档 / 收纳盒 / 浏览器标签；把它拖到中栏边缘才拆成多格（左右/上下、拖动条
调宽高），文档还能拖出主窗口成独立小窗（拖回合并）。

- **入口**：转录链接、交付卡、ContextPanel 改动文件行、收纳盒文件卡——四处都经
  `src/lib/openFile.ts::openInDocTabIfEligible` 分流（先照旧 resolve，命中 doc 扩展名
  且非 risky 才进标签页）；右键菜单与页内「在系统中打开」保留 OS 路线。detached 窗
  不装 opener，照旧走系统应用。
- **页面**（`src/components/DocPage.tsx`）：预览态复用转录 markdown 管线（math/
  mermaid/shiki/spec 卡全有，图片相对路径按文档所在目录解析）；编辑态等宽源码
  textarea；Ctrl+S 保存。保存带 **mtime 乐观锁**（`save_doc_file` 的 stale 拒绝是
  返回值不是错误 → 确认后 force 覆盖）。文档内相对 `.md` 链接页内跳转。
- **引用给 agent**：选中文字（预览或编辑态）→ 引用块预填输入框（`prefillComposer`
  通道，只预填不自动发）。锚点式批注评论明确不做。
- **未保存草稿**跟随文档路径：切换标签/暂时回对话后仍保留；从标签关闭有未保存内容的
  文档会先弹确认（含撕出的独立文档窗）。旧的 DocCompareLayout 不再是正常入口。
- **贴边态**：开文档 = dock 整页（`DockPage "doc"`，返回/Esc 由 dock chrome 管）。
- Ctrl+W / Ctrl+Tab / Ctrl+1..9 统一按聚焦窗格的标签序列工作（会话与 Browser、
  Document、Memo 混排）。

## 内置浏览器 Workspace Surface + agent 工具通道（2026-11-17 P1–P4；2026-11-18 起为分屏树窗格标签）

app 内置浏览器，agent 通过工具通道读写它。普通窗口里 Browser、Document、Memo 都是
分屏树里的窗格标签（点开落在聚焦窗格，可拖拽分屏/跨窗格移动）。单格时页面占满该标签页，
标题栏仍是正常标签栏；多格时每个窗格只列出自己拥有的标签。浏览器面板不可撕出主窗
——child WebView 钉在主窗；Dock 仍用整页浏览器。

- **入口**：标签条「浏览器」chip、Ctrl K「新建浏览器标签」、贴边态整页。agent 的
  `browser_open_tab(display:"background"|"workspace")` 默认把 tab 留在后台；只有
  `workspace` 才进入工作区，且只用于用户明确要求查看/操作页面的情况。开发服务器发现默认**只通知**；
  自动打开必须在 Ctrl K 显式 opt in。Agent 自己启动 localhost 不会因此获得 workspace 权限。
  地址栏接受无 scheme 的 `localhost:5173` / `127.0.0.1:3000`（走 http），`http://localhost`
  导航改写为 `127.0.0.1` 避开 WebView2 先连 `::1`；`file:///D:/…/index.html` 和绝对
  Windows 路径直接作为本地文件打开，不会再被补成 `https://file…`。
- **窗格标签（2026-11-18）**：Browser / Document / Memo 标签在窗格标签条里（child
  WebView2 host 之外），与会话标签同一套拖拽/分屏/键盘管线。单格继续用顶部正常标签栏，
  不额外画一条空的首格标签栏；真实多格才切成每格标签栏。surface 标签位置随
  `splitLayout` 快照恢复，但 **Browser 标签冷启动不恢复**（绝不自动创建 child
  WebView2——否则一个卡死的 demo 页会形成「启动自动恢复 → UI 卡住 → 强杀 → 再恢复」循环）；
  旧快照里的浏览器字段一律忽略。拖任何标签期间 BrowserPage 暂时隐藏 child WebView，
  让 DOM 的拖拽幽灵、分屏高亮和落点命中不被原生 surface 抢走；放下或取消后立即恢复。
- **运行状态（P2）**：Agent 打开的 tab 记录 `ownerSessionId`、`busy`、
  `status: idle|running|waiting_user|failed` 与最近使用时间；并发调用用计数，避免一个
  调用结束时把仍在工作的 tab 误报为空闲。删除会话/项目会关闭该会话拥有的 tabs，
  用户自开的 tabs 不受影响。
- **dev server 发现（P2）**：4 秒轮询运行中 ACP terminal 的 64KB 输出尾，去除 ANSI，
  只接受 loopback/通配地址，并先做 TCP probe。通过后默认只发通知；只有用户显式选择
  「自动打开」才创建 workspace tab，同一 origin 十分钟内不重复打扰。实现见 `src/lib/devServer.ts` 与
  `src-tauri/src/browser/service.rs`。禁止仅按 URL 里的 localhost 字符串切界面。
- **handoff（P4）**：`browser_handoff` 让 Agent 暂停在登录、2FA、CAPTCHA、支付确认等
  必须由人完成的步骤。tab 进入 WAIT，应用切到来源会话并打开 Browser surface，页面与
  对话之间显示标题、指令、origin、Continue / Deny；后台窗口同时发通知。Continue 放行，
  Deny 作为可转述的拒绝返回工具，超时不会默认继续。
- **布局**：单格标签页占满中栏；需要并排时拖到中栏边缘进入统一 `SplitWorkspace`，
  每格自带标签栏和会话 composer，分隔条可拖、宽高随树快照恢复。浏览器 child WebView 只
  跟随自己窗格里的 host 矩形；离开该标签或整页接管时隐藏。ContextPanel 不再因 surface
  自动收起。
- **标签生命周期（2026-11-17）**：Agent 打开 tab 可传稳定 `id`（如 `work-mail`），同名调用
  返回已有标签而不重复导航。标签菜单可「保留 / 取消保留」或「转为我的标签」；claim 后清空
  owner 与 grant，会话删除和空闲 cleanup 都不能关它。用户切换 tab、手动导航和分享动作都会
  刷新 lastUsed；10 / 30 / 60 / never 空闲清理只关普通 Agent tab，跳过 pinned、named、busy、
  waiting_user、handoff 和当前可见 tab。
- **Browser 产品边界**：Marionette 只把内置 WebView2 开发工作区作为一等 Browser。产品不安装或
  配置 Chrome/Edge 扩展、relay 和配对，也不提供个人浏览器助手。用户需要日常登录态浏览器能力时
  可自行安装 browse；它本质上是外部 MCP server，底层 Agent 可以使用，Marionette 会把调用展示成
  普通 MCP 工具。项目上下文也可按通用规则扫描并借出该 MCP（默认关闭）。browse 的页面、tab group、
  WAIT 和配对状态留在用户浏览器与 browse 中，不导入 surface。边界见
  `docs/plans/plan-browser-scope.md`。
- **授权**：agent 自开标签自动**可读**（快照/console/截图）；**点击/输入（Control）
  逐次弹权限卡**（允许一次 / 允许并记住此站点 / 拒绝，2 分钟超时自动拒绝），该会话
  开着 always-approve 则自动放行；用户自开标签读写都要手动分享。grant 绑 origin，
  跨域导航用户标签撤销、agent 标签重授读。
- **工具通道**：主进程内嵌 localhost HTTP MCP（`tools_mcp.rs`，每会话独立 Bearer
  token），session/new 注入 url 型条目——全 agent 通吃含 zcode。20 个 `browser_*`
  工具（help / waits / load_state / dialog / batch 是 2026-11-18 ego-lite 批）；
  拒绝是返回值不是报错（`browser_grant_required` 附转述话术）。
- **等待与弹窗（2026-11-18，ego-lite M3）**：`browser_wait_for`（ref ×
  attached/visible/hidden/detached）与 `browser_wait_for_load_state`
  （domcontentloaded/load）替代固定 sleep 与盲重拍——轮询在 MCP worker 线程睡
  （**绝不睡主线程**），每拍一次短 eval，硬 deadline（默认 3000/5000ms，上限
  15000/30000ms）；超时是观察不是瞎重试信号（not_found = ref 不再匹配，重拍）。
  页面 script dialog（alert/confirm/prompt）由 `Page.javascriptDialogOpening`
  事件镜像进 `Surface.dialog`（**零 CDP**），此时所有 renderer-bound eval 在
  `evaluate_in_world_raw` 入口**快速失败 `dialog_blocked`**（被 dialog 阻塞的
  renderer 会让 eval 挂死在进程级 CDP lease 里）；`browser_dialog`（Control 级）
  经 `Page.handleJavaScriptDialog` accept/dismiss（prompt 带 promptText），
  **任何 dialog 都不自动应答**，用户始终能看到 WebView2 默认弹窗并自行作答，
  agent 先答则用户无感；用户先答的话 browser_dialog 诚实回报 `present:false`
  并自愈清除镜像。navigation 清镜像。
- **typed batch（2026-11-18，ego-lite M2）**：`browser_batch` 一次调用串行执行
  最多 32 步封闭 op（click/hover/type/setValue/pressKey/selectOption/
  waitForSelector/waitForLoadState/pageState/pageReady/snapshot），无脚本、无
  evaluate、无导航。每个 Control 步仍走自己的权限卡（**拒绝即停批，不受
  stopOnError 影响**）；权限等待不烧批预算（冻结剩余时间）。首错即停（可关）；
  失败记录带 index/op/error；**snapshot 只能做最后一步**（它换掉整张 ref 表）；
  中途导航 → 后续 ref 步全 stale。结果含逐步审计（`browser-batch` 落 debug.log）
  + 末态 pageState + 仍有效的 generation。
- **aria 引擎** vendor 自 Codeg（Apache-2.0，`src-tauri/src/browser/js/`）：
  快照按元素 ref 操作（非像素坐标），只在第一次有授权读取时注入 isolated world。
  **快照只覆盖主 frame**（engine 不跨 iframe，`contentDocument` 零命中实锤）；
  picker 的同源 iframe 递归命中不受此影响。
- **ref lease 与连续动作（2026-11-18，ego-lite M0）**：一张 snapshot 的
  `generation` 覆盖**任意多个连续动作**——click/type/select/press/set_value 反复
  复用同一个 generation，页内 engine 每次全量校验（token/URL/history/popstate·hash
  计数/navigation entry/元素 connected），动了才回 `stale`。host 不再在每次 action
  后推进 generation（旧 bump 从不产生真实失效）；`browser/ref_lease.rs` 只记
  「host 亲眼所见的死亡」（document_navigation / route_navigation /
  user_annotation——用户批注流的重拍会顶掉 agent 的 refs、stale_answer、
  superseded_by_snapshot），已知死因的动作**不打 CDP 直接回 stale + reason**；
  lease 永不判「有效」，页内 engine 是唯一裁判。成功响应回显
  `generation` + `generationStillValid: true`。回退开关：
  `MARIONETTE_BROWSER_REF_LEASE=0` 精确恢复旧行为（逐 action bump、无短路、
  无注记）。回归：`npx tsx scripts/verify-browser-refs.mts` + cargo browser 套件。
- **browser_help（2026-11-18，ego-lite M1）**：只读 meta 工具（无授权检查），
  可选 `topic`（overview/refs/snapshot/actions/tabs/handoff，默认 overview）。
  工具清单**从 `tool_schemas()` 生成**（单一真源，不可能漂移）；overview 载入
  观察经济（先 `browser_page_state` 的 `seq` 判断变没变 → 需要找元素才 snapshot →
  视觉问题才 screenshot；达成即停；不开新 tab 逃跑）。未启用的能力（waits/batch）
  不出现在 topic 里，测试双向钉死。
- **ref lease 真机手工回归**（fixture server：`node tools/serve-browser-fixtures.mjs`，
  10 页在 `tools/browser-fixtures/`，loopback 8899）：
  ①basic-form——同一 token 连点两个按钮/连续 type+select（不重拍）；
  ②basic-form 局部更新后继续 type 目标输入框（仍 connected → 成功）；
  ③dom-replace——目标节点被替换 → `stale`；
  ④spa-routes——pushState / hash / history.back → `stale`
  （reason=route_navigation）；
  ⑤任意真导航 → `stale`（reason=document_navigation）；
  ⑥用户批注发送后旧 token → `stale`（reason=user_annotation）；
  ⑦late-render——`browser_wait_for` 等 500ms 出现的按钮在 deadline 内成功；
  永不出现的元素超时返回 `not_found`/`timeout` + lastState；
  ⑧dialogs——alert/confirm/prompt 打开时其它调用快速失败 `dialog_blocked`；
  `browser_dialog` accept/dismiss 如实回报；confirm 不被自动接受；用户先点默认
  弹窗后 browser_dialog 报 `present:false`；
  ⑨overlay——被覆盖按钮的点击走 bundle 的 hit-target 检查；
  ⑩iframe-same/cross/shadow-dom——快照覆盖范围与 picker 递归边界按「aria 引擎」
  条目所述；双 tab 同时动作不冻 UI（CdpLease 仍串行）；
  ⑪batch——6 步表单链一次调用完成；第二步 stale 时第三步不执行（`stoppedAt`）。
- **元素批注（2026-11-17）**：地址栏旁的十字准星开启 DevTools 式拾取——悬停高亮 +
  角标（tag/id/尺寸），点击拾取元素；切「框选区域」拖拽圈选（列内含元素）。拾取期间
  全屏命中层接住真实指针，底层控件不收到 activation，`isTrusted=false` 的合成事件
  直接拒绝，Esc 退出。批注抽屉在工具栏与页面之间（页面向上让位），每条一行：文件
  落盘后的截图小缩略图（失败可重试）+ 元素摘要 + 批注输入框。状态由 App 持有，切换
  页面不会丢草稿；组件卸载会先停止页内 picker。单批上限 24 条。
  同源 iframe 与 open Shadow DOM 可继续向下命中；跨域 frame 只能选中它的顶层
  iframe，区域扫描会把内容不完整标出来。
  「发给对话」把批注清单（选择器、aria ref——发送时重新快照解析、失效则回落选择器、
  框选内含元素）与截图路径**预填进当前对话输入框**；普通窗口直接在同屏右栏检查，
  Dock 则回到窄条让 Composer 可见。成功后才清草稿；没有真实会话先建一个。页面字段
  明确标为不可信数据，URL 会脱敏；导航后混有多个页面时按页面分组。
  拾取是用户自己的 UI 操作，不走授权（授权只挡 agent）；agent 拿到批注后用现有
  browser_snapshot / browser_click / browser_set_value 操作。
- **体积/内存**：不捆绑浏览器引擎（复用 WebView2 运行时，独立 profile
  `~/.marionette/browser-profiles/`）；**标签懒建**（2026-11-06：启动不再预热
  about:blank 子 webview——那会常驻一整套 WebView2 进程组；首标签走
  `create_surface` 直建，关掉标签 renderer 回收）。
- 设计定稿见 `docs/plans/plan-browser-workspace.md`；用户体验对标见
  `docs/research/research-browse-ux.md`。

## Computer Use（2026-10-31，批次 3）

agent 可以看/操作**其它 Windows 应用**的窗口（浏览器之外的原生 GUI）：

- **引擎**：cua-driver 0.36.0（trycua/cua，开源）首次使用才下载（~35MB，双 SHA-256
  pin，装 `~/.marionette/cua/`）；helper = 主 exe 参数分支
  （`--marionette-computer-helper`，stdio 帧协议），Marionette 退出 → helper →
  driver 全链下线。env 硬化（固定 PATH/独立 HOME/遥测关）+ **exe 哈希每次启动前
  复验**。
- **入口**：Ctrl K →「共享窗口给 agent」面板（引擎安装、窗口列表、每窗 只读/可操作、
  全部停止）；**全局停止键 Ctrl+Alt+Shift+S** 一键撤销全部共享。
- **授权**：agent 读未共享窗口 → 对话内权限卡（同浏览器 Control）；操作（点击/
  输入）同样走卡；会话 always-approve 自动放行；面板手动给的是长期授权（30 分钟
  不用自动失效）。自己的窗口与密码管理器黑名单**永不共享**。
- **工具**（与浏览器同一 localhost MCP）：12 个 `computer_*`——list_windows /
  screenshot / snapshot（元素树 + element_token）/ click / drag / scroll / type /
  press_key（**按键白名单**：桌面和弦不放、粘贴只在剪贴板仍是 agent 自己写的内容
  时放行）/ set_value / restore / clipboard_read/write。
- **边界**（明说）：这些 gate 挡不住有 shell 的 agent——授权语义是「用户知情同意」，
  不是沙箱（同 Codeg 立场）。整屏共享/App 范围在协议里但未开 UI（整屏要连
  paint_over 一起做才安全）。偏差清单见 plan §7。

## 共通插件层（2026-11-05；2026-11-06 收口）

**安装一个插件，所有 agent 都生效**（设计稿 `docs/plans/plan-plugin-layer.md`）：

- **登记处**：`~/.marionette/plugins/index.json`（Marionette 自有，不写任何 agent
  的配置）；安装即全局默认启用。入口：右栏「共享」标签里的「插件」卡片（「所有
  agent」MCP 块 +「技能」块；2026-11-06 起 **DSH 专属插件块已删**，统一入口只此
  一处）+ Ctrl K「插件」（自动切到「共享」标签并滚到位）。
  stdio 型填 command/args/env，http 型填 url/headers；安装时自动**探活**
  （initialize + tools/list，探活失败不拦安装，结果记在 `lastProbe`）。
- **生效方式**：每个启用插件在 `session/new` 时以 **url 型**条目注入（与
  marionette-tools 同形，挂在同一个 localhost server 的 `/mcp/p/<id>` 路由）——
  这是唯一被全部 adapter 接受的形状，**zcode 也吃**；不支持 http 但收 stdio 的
  agent 回退注入原始 stdio spec。新会话自动生效，进行中的会话重连后生效；与借贷
  条目同名时插件赢（防双注册）。
- **stdio 插件**：Marionette 自己起子进程（按会话懒起、`resolve_spawn_command`
  解析 npx/.cmd、Job Object 兜底），把 MCP stdio 翻译成 HTTP；**会话停止/agent
  崩溃即杀**；禁用/卸载跨会话杀。
- **http 插件**：纯中继——**真实凭据只留在 Marionette**，agent 只拿 loopback URL
  + 会话 token；远端 `Mcp-Session-Id` 由宿主代持；只收 JSON 应答（流式 only 的
  服务会明确失败）。
- **skills（2026-11-06 起可从界面装）**：`~/.marionette/plugins/skills/` 是全局
  技能目录（default-on prompt 借贷），装一次全 agent 可见。插件卡「技能」块支持
  **从文件夹安装 / 停用 / 删除**：登记处就是目录本身（每个含 SKILL.md 的子目录
  即一个技能，不进 index.json）；全局停用 = 目录里放 `.disabled` 标记文件，
  `context_inventory` 扫描跳过带标记的目录——「项目上下文→技能」清单与注入
  同步消失，两个入口看同一份真相。安装 = 复制整棵源文件夹（源里带着的
  `.disabled` 不随拷贝，装上即启用）。
- 边界：child→agent 的 server 主动请求（sampling 等）不支持（-32601）；pi/web 壳
  照旧不注入；不做 marketplace（registry/Smithery 安装与 codeg 式 junction skills
  矩阵记为后续）。插件自己的 JSON-RPC error 原样透传给 agent。DSH 自家的
  `dsh plugin` 面板已随统一入口移除（已装进 profile 的 DSH 插件对 dsh 自家
  照常生效，跨用桥与 spawn 链不动）。

## 员工模式 / Chief（2026-11-17，E1 主体 + E2/E3 部分）

Chief 已从「Telegram 远程助理配置」升级为 people-first 员工模式：侧栏顶部是员工，右栏是
员工档案/任务/记忆/承诺；worker 仍是普通项目会话。设计、实际偏差和待办见
[`plan-employee-mode.md`](./plans/plan-employee-mode.md)，前置 Telegram chief 研究见
[`research-telegram-chief/`](./research/telegram-chief/README.md)。

- **入口与雇佣**：首次使用自动创建一个固定示例助手 `Mari`，绑定第一个已启用 Agent 和
  Agent 默认模型，不需要先做配置；点击前不发送 bootstrap、不启动模型回合。seed 只执行一次，
  用户主动删光员工后不会被重启加回。右栏
  不再渲染旧的「远程助理」配置卡；旧 `config` 只保留后端迁移与 Telegram 兼容。额外添加员工
  时首屏只要求名字，模型、persona、项目细权、Telegram 与预算收进「更多设置」；员工档案里的
  技术资料默认折叠。
- **注册表与身份**：`~/.marionette/chief/staff.json` 保存员工、逐项目权限、预算、通知策略和
  注册表级持久暂停；旧 `config.chiefSessionId` 只做一次迁移。迁移屏障完成后删掉最后一名员工，
  旧 config session 不再获得控制工具。坏档保留原文件并 fail-closed，不会被 legacy 配置覆盖。
- **会话关系**：员工是普通 Chat 会话（`origin="chief"`），worker 是普通项目会话
  （`origin="chief-worker"`，不写 parent）；关系只进 task/event 账本和 tee。员工会话强制使用
  Agent 自报的只读/plan 权限；需要改项目时只能派 worker，不支持只读档的 Agent 拒绝启动员工会话。
- **员工聊天**：`origin="chief"`的会话仅显示用户消息、员工消息、决策卡以及警告/错误系统通知。 thought、tool call、活动时间线、model、token和持续时间元数据保留在原始transcript和worker会话中。 员工Composer保留附件、文本和发送。技术控制保留在员工档案或右栏中。
- **控制面**：16 个 `marionette_*` 工具只在员工会话的 `tools/list` 出现，dispatch 双保险；没有
  hire / approve 工具。`task_start` 的 requestId、并发预留和 Proposed 插入在同一任务账本锁内；
  Proposed 也计入并发和急停/辞退。`task_send` 成功后才提交 commandId，resume 只接受
  `Interrupted → Starting` 的精确 CAS。
- **任务与权限**：逐项目可配置只读 / 审批写 / 预授权写、Computer Use、自我修改，并被全局策略
  二次裁剪。read_only 通过 permission/mode 硬落地；worker session id 在 ACP 启动前写进 task，
  策略映射失败就不启动；Computer Use schema 与 dispatch 都按项目 `allowComputerUse` 拒绝。
- **急停**：`/stop`、Telegram「急停」、右栏暂停全部走注册表级 `pausedUntilUserResumes`；新员工
  自动继承。急停取消 Proposed/Starting/Running/WaitingUser、关闭 worker 浏览器 tabs、撤销
  Computer Use 共享窗口；恢复必须用户显式 `/resume` 或 UI 操作。
- **记忆与承诺**：每位员工有 pinned/state/history/journal/index/proposals，tombstone 防复活；
  task 终态自动追加真实 journal，proposal 可在右栏 accept 到 pinned/state 或 reject。
  commitments 与 task 分离，到期/`task:<id>:completed|failed` 条件可唤醒；暂停、manual、禁后台
  时不消费 due，恢复后补唤醒。员工行显示 due 数量但不改 running/waiting 状态。
- **通知**：普通通知的冷却和每小时额度持久写员工档案，写盘失败不发；waiting_user、失败、中断、
  权限/提问/plan、承诺到期可绕过勿扰和普通频率预算，但永久暂停永远优先。`allowTelegram` 只控制
  Telegram 渠道，不误伤桌面通知；task.started 不占完成汇报额度。
- **Telegram**：显式绑定员工私聊即可路由，不再要求全局 `mode=chief`；旧单员工 chief 回落保留。
  任务卡和回复按稳定 `staffId` 找当前会话，员工 session 重建后不丢唤醒/引用。底层 transport 仍是
  单已配对 chat，多员工同时绑定多个私聊是待办。
- **唤醒**：completed / failed / interrupted / waiting 进员工收件箱，2s 心跳在空闲时合并 flush；
  worker 的普通流式 chunk 以 5s 节流刷新 task lastEventAt，避免长时间思考被误标 stale；重启后
  非终态 task 标 `interrupted`，可 `task_resume`。
- **红线**：能力来自 Host 注册表和现有权限卡，模型不能靠参数扩权；发布/push/凭据/系统设置远程
  不可达；自我修改仍要求 worktree/diff/tests，不做自动发布。精确 provider 调用计数、成本请求前
  预占、多 chat transport、健康审计页和 E4 长期 soak 尚未完成。

## AI 自主派活（2026-11-22，第一期「拆门」）

任何一个普通对话里的 agent 都能把活派给**另一个 agent**——你说「把 X 调研派给 Codex 的
某个模型」，她自己调 `marionette_delegate_task`；六期路线图（可信→过夜→可验收→可反悔→
安静）与设计红线见 [`plan-agent-delegation.md`](./plans/plan-agent-delegation.md)。

- **体验形态**：父对话里亮「AI 派活」卡片（复用 @ 委托卡渲染，带芯片标识，内嵌子会话
  实时流）；子任务是顶层普通会话（`origin="agent-worker"`，侧栏带「AI 派活」角标，
  可点开/接管/追问）；父 AI 派完即睡（守则明写禁轮询），子任务完成时系统以
  `[HOST EVENT]` 消息唤醒父会话收结果（忙时排队、空闲即投；父进程不在就留着，
  重新打开的下一拍补投——不自动花钱唤起没打开的会话）。
- **天花板**：任务永远落在父会话自己的项目里（不开放跨项目）；每会话并发 2；
  子员工拿不到派活工具（禁递归）。
- **权限跟着主对话走（2026-11-23 修）**：派出去的 worker 用**和主对话同一档**的
  权限——主对话开了免审批（Grok 的 Always approve / Codex 的完全访问 / Claude 的
  Bypass），worker 也免审批，不再每个操作弹一张卡；主对话是「自动接受编辑」就
  同样只自动接受编辑；主对话是只读，worker 强制只读。worker 永远不会比主对话
  更放权。目标 agent 没有对应档位时如实写明「会逐个询问」（任务行里看得到）。
  Marionette 自己的浏览器控制 / 电脑控制不跟着放行，照常出卡。
- **指定模型不糊弄（2026-11-23 修）**：派活的 AI 选模型前能查到每个 agent 自己的
  真实清单（它有对话在跑时；OpenCode 的免费模型、OpenRouter 的免费模型、zcode 的
  账号模型都在里面）。agent 不认指定的模型名时任务**立刻失败并带回它真实的清单**，
  绝不偷偷换成默认（最贵的）模型；要求的档位没生效时任务行里写明。
- **随时能停（2026-11-23 修）**：父对话卡片的「停止」、右栏后台任务面板的 ×，
  一键停掉——包括额度午休中、到点会自己复活的任务。停止后卡片显示「已取消」，
  父 AI 收到一条已取消的回执（带 worker 做到哪了的最后输出）。任务一结束就收掉
  worker 进程。不是当前对话在要权限时，权限弹窗标明「来自对话：…」。
- **可靠性**：worker 进程消失记 `Failed(worker_gone)`，绝不猜「完成」；requestId 幂等；
  急停纳入这些任务；任务账本启动时自动修剪（终态 30 天 / 400 条硬顶）。
- **面板与开关**：右栏后台任务面板新增 AI 派活行（「在等你确认」置顶）；Ctrl K 可整体
  关闭/开启派活工具注入（默认开，重启回默认）。
- **二至六期（2026-11-22 同日落地）**：
  - **可信**：结果回执跨重启不丢（启动重建唤醒收件箱）；查过状态就算送达（不重复
    唤醒）；回合结束先分类再定论——拒答/报错/零产出都如实记失败，不猜「完成」；
    状态查询把等待原因说成人话（额度第几次重试、还差几分钟、在等你什么）。
  - **过夜**：worker 连续 20 分钟无动静即中止并记「卡死」（是我们掐的，不记成
    完成）；额度/限流受限自动排队（5/15/30 分钟退避，3 次封顶），到点自动复活
    续跑——复活后模型/档位/权限档全部重放——面板显示「额度受限，N 分钟后自动
    继续」。这两样只对 AI 派活生效；被中止的回合一律记「已取消」。
  - **可验收**：派活可附交付模板（JSON Schema），worker 多一件 submit 工具按模板交
    结果、宿主校验格式（缺键/类型不符逐条打回），结果随回执送达父会话。
  - **可反悔**：git 项目的任务开工前自动记存档点；结果卡带「回滚此任务」（确认后把
    任务期间改过的文件恢复到开工时；新文件不动、数量如实报告）。
  - **安静**：统一注意力投影（approval/input/failure/interruption/stalled 需要你；
    额度午休/在跑/已结束安静），后台面板的「需要你」置顶全走它。
- 尚未做（后续批次）：额度管家总览面板、事件触发值班、桌面通知/Telegram 接
  attention 投影、per-轮存档、Telegram 任务卡推送。

## 明确不做 / 暂缓

- 内嵌 llama.cpp / 本地推理 runtime
- 完整文件树 / IDE 级编辑器
- 全 Rust UI 重写
- 把历史 ASP 文档当现行协议实现
- 裸 CLI 全局配置接管（终端里裸跑 claude/codex 不经 Marionette 也能用引擎——已明确不做，见归档设计稿）

## 工程入口

- 前端：`src/`
- 后端：`src-tauri/src/`
- 启动：`start-marionette.bat`（根目录唯一的 GUI 入口；`install.ps1` 是发布资产，不算入口）/ `npm run tauri dev`
- 便携构建：`tools/build-portable.bat` → `release/<版本号>/Marionette.exe`（默认 Rust 进程内引擎）
- Magpie 回退源码：`MARIONETTE_ENGINE=magpie` + `tools\import-magpie.ps1`（仅一个发布周期）
- 验证脚本：`scripts/`；Windows 脚本：`tools/`（各目录索引见其中的 `README.md`）

## 归档说明

`docs/archive/` 保留早期策划、路线图与 superpowers 规格，便于考古，**不以之为当前实现契约**。有冲突时以代码与本文件为准。跨用引擎的完整设计稿（M0-M3 过程记录）已归档至 [`archive/cross-agent-bridge.md`](./archive/cross-agent-bridge.md)。
