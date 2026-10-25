# 调研：Codeg（同类多 Agent 工作台）全量分析（2026-10-25）

- 参考仓库：<https://github.com/spacering-net/codeg>（Apache-2.0，分析时 v0.40.0，3752 个提交）
- 本地克隆：`D:\Projects\codeg`（Marionette 旁边；`git fetch --unshallow` 已补全历史）。更新：`cd D:\Projects\codeg && git pull`
- 方法：8 个只读探索代理分区深挖（agent 接入与生命周期 / 对话流 UX / 多 agent 协作 / 会话聚合 / git 历史坑矿 / 富内容渲染 / 外部连接 / 技能与杂项），主代理核对 per-agent 文档与 ZCode 对接细节
- 定位对比：Codeg 与 Marionette 同赛道（Tauri 2 + ACP 多 agent 壳），但走**全功能工作台**路线（编辑器/画布/merge editor/服务器/手机端/IM 通道）；Marionette 走**轻壳 + 贴边停靠 + 模型共享引擎**路线。本文只挑对 Marionette 有用的部分，不做整体搬运。
- 重要事实：**Codeg 没有任何位置直连 LLM provider**——所有"智能"（commit 生成、委派、@assistant）都是起一个 agent 进程借 CLI 之力。Marionette 的共享引擎（壳层自有 LLM 路由）反而是 Codeg 没有的差异化能力：他们要付"起一个 agent 进程"的代价，我们可以一次 API 调用完成。

## TL;DR

- 最值得抄的用户可感功能：① 回合"活动胶囊"摘要行（干了什么/多久/败没败，失败可点击跳转）；② 向运行中回合插话（steer）+ 消息队列；③ 权限卡"还有 N 个排队"计数；④ 网络代理层（中文用户基本盘，NO_PROXY 三坑现成答案）；⑤ MCP/Skills「装一次分发到 N 个 CLI」。
- 最该提前规避的坑：① 退出时异步杀 agent 不可靠（必须同步 kill_tree 兜底）；② 中文 Windows 子进程 GBK 输出按严格 UTF-8 读会静默截断；③ 虚拟列表 + 吸底锁 + 分数缩放的底部抖动；④ agent 适配器 release notes 不可信，升级必须实测。
- ZCode 情报（同一社区桥 0.76.2）：`ZCODE_ACP_RUNTIME=node` 必须钉（桥会自我迁移到 Bun）；Start Plan 账号过不了桥（captcha）；新会话首条 prompt 前必须显式确认 Build mode。

## 功能速查（"它有什么我们没有的"一览，每条对应下文编号）

| 功能 | 一句话 | 详见 |
|---|---|---|
| 生成式 UI | agent 写 ` ```spec ` 围栏 → 聊天内交互卡片/表格/图表，零协议改动 | B4 |
| 数学/Mermaid/PlantUML | 回复出图，卡片缩放/导出，全本地渲染 | B1、B3 |
| 聊天内图片 | agent 读图/生图原位卡片；markdown 本地图片自动内联 | B2 |
| PDF 旁聊预览 | pdfjs 三处共用；CJK cmaps 缺失会渲染空白 | B5 |
| 活动胶囊 | 回合摘要行：步数/结局/耗时，失败可点击跳转 | A1 |
| 命令组"走过即收" | 回复往前走后整组滑收，live 成员永远露出 | A2 |
| Steer + 消息队列 | 向运行中回合插话；队列可拖拽/编辑，⚡/🕐 如实表达通道 | A3 |
| 权限卡三细节 | 排队计数 / diff 预览 / 事后 approval chip 溯源 | A4 |
| 命令活尾巴 | 折叠行露末 3 行；失败自动展开锚底；<10s 不显示时长 | A5 |
| 通知门控 | FlashWindowEx 覆盖、focus vs hidden、冷却合并、回放抑制 | A6 |
| 网络代理层 | scheme 补全、NO_PROXY 双拼写、回环强制绕过（三坑） | C1 |
| MCP 一键分发 | 扫本机 N 个 CLI 的 MCP 聚合 + 市场装一次分发 N 家 | C2 |
| Skills 中央仓 | 技能×Agent 矩阵，junction 链接零 DB，Superpowers 现成 | C3 |
| 会话落库 | 索引在库正文在原文件；LIKE 搜索 + 标签 + 软删 | C4 |
| 自有 ACP 转录 | 存 wire 格式 JSONL，补原生 store 逆向不了的坑 | C5 |
| Agent 生命周期 | pin + 未审计升级双轨 / 诊断 verdict / registry 目录安装 | C6 |
| dev server 发现 | 扫终端 banner + TCP 探活 + 预览 tab | C7 |
| LLM commit message | ✨ 按钮；我们可用共享引擎一次调用做得更轻 | C8 |
| URL scheme | `codeg://session/<id>` 直达会话，与 TG 桥联动 | C9 |
| ProjectShelf 增强 | 每项目默认 agent / scripts 变命令 / 分组交错排序 | C10 |
| Token 分析页 | 缓存命中环 / 热图 / 牌价折算 / 多维排名 | C11 |
| 主题系统 | token 级改色 / `--radius` 派生 / GIF 壁纸原样存储 | C12 |
| 回合补时钟 | 无钟 agent 壳层自记 JSONL，只存哈希；宁无钟不错钟 | C13 |
| @ 委托 | MCP 伴生进程三工具，进度流回父对话，卡权限原地代批 | D1 |
| @assistant | "agent+模型+常备指令"预设，可 @ 的角色 | D2 |
| 收菜（worktree 任务） | 隔离 worktree 无人值守，review 后一键 merge | D3 |
| awaiting_input 回流 | 无人值守/分屏非活跃侧卡权限，卡片上直接答 | D4 |
| 浏览器共享 | 内嵌 webview；页面元素/报错递给 agent；授权按域名 | D5 |
| Computer Use | cua-driver 操作原生窗口；按键白名单/停止键/指示条 | D6 |
| 微信 iLink 通道 | 官方机器人接口，个人微信扫码即用；TG/Lark 同架构 | D7 |
| Automations/cli/forge | composer 快照+cron / runtime-owned CLI / issue 变任务 | D8 |
| 远程工作区 | 手机/浏览器接桌面；SOCKS-over-WS 隧道（存档备用） | D9 |
| gallery 开发页 | fixture 灌真实渲染管线，消息组件多状态的加速器 | D10 |

坑清单（进程管理/编码/Windows/Tauri/流式 UI/适配器漂移/MCP 注入/解析容错/并发治理）见第二节；ZCode 同桥情报见第三节；行动优先级见第四节。

---

## 一、值得抄的功能

### A. 对话流体验（第一梯队：直接补强现有体验）

**A1. 回合"活动胶囊"头部** — 每条回复顶部一行胶囊：`14 steps · 1 failed, recovered · 3m 34s`。失败是红色可点击链接直接跳到出错步骤；运行中 `Working 6 steps` 带微光；无步骤回复保留 `Reply 3s` 占位，**槽位永不闪烁进出**；分类明细只进 tooltip 不进正文。Codeg 最近 30 个提交一半在打磨这一行。入口 `src/components/message/activity/activity-header.tsx`（截断优先级 L104-212、失败跳转 L168-175）。Marionette 的 tally 可升级成这个形态并与交付卡合并成一行。

**A2. 命令组"回复走过后自动滑收"** — 流式期间连续命令逐条列出；回复继续往下走且无成员在跑时，整组带动画收拢成 `Ran 2 commands · all passed`；折叠后仍在跑/等确认的成员行被"顶"在组头外（当前步骤永不被藏）；未恢复的失败强制展开。状态机 `command-rows.tsx:546-638`（`followed`/`ended`/`slideShut`）。

**A3. Steer（向运行中回合插话）+ 消息队列** — 排队消息可拖拽排序/编辑/删除，每条两种插话按钮：⚡（立即插入）与 🕐（下次检查才读——图标如实表达通道差异）。入口 `message-queue-display.tsx`；`canSteer` 明确禁掉"点了也没用"的行。Marionette 有 Fork from this reply，steer 与队列完全缺失，是多 agent 工作台高频刚需。

**A4. 权限确认卡三细节** — ①排队计数 Badge（`还有 N 个排队`，FIFO 一次只显示一个——不显示排队数用户会以为 agent 卡死）；②文件改动权限内嵌 diff 预览；③批准结果事后以 `🛡 approved once` 芯片标回工具行，tooltip 记录谁批的/何时。`permission-dialog.tsx:61-160`、`approval-chip.tsx:45-78`。

**A5. 命令"活尾巴"与度量噪声阈值** — 跑着的命令折叠行下实时露最后 3 行输出（去 ANSI、500ms 淡入防闪现）；失败自动展开并把输出锚定到底部；成功且 <10s 不显示时长（`SHOW_DURATION_MS=10000`）；输出 >2000 行只渲染尾部并注明。`command-rows.tsx:41-48`。

**A6. 通知门控矩阵** — 四条语义：Windows `FlashWindowEx` 后发请求覆盖先发（低级别会削掉权限请求的持续闪烁，需按级别互护）；`document.hidden` 门漏掉"第二显示器可见但未聚焦"窗口（应问 focus）；每事件 3s 冷却合并 + 全局 400ms 间隔；断线重连回放绝不触发通知。`desktop-notification.ts:38-106`、`window-attention.ts:113-123`。

**A7. 折叠状态外置存储** — 模块级 Map + `useSyncExternalStore`（LRU 1024 条 + latched 集合），展开状态在虚拟化卸载/流式重建后不丢。`disclosure-store.ts:1-30`。成本极低，虚拟列表必备。

### B. 富内容渲染（对话流的地基，Marionette 当前是纯 markdown）

**B1. 流式 markdown 管线**（streamdown：cjk/code/math/mermaid 四插件）— 三个重引擎懒加载（shiki ~3.8M / katex ~4.3M / mermaid ~70M，纯字符串扫描命中才 import，进程级单例）；`singleDollarTextMath:false`（单 `$` 永远是字面文本，`$HOME`/`$9.99` 不误判）；流式期间 remend 补全未闭合 markdown + 未闭合链接先显示纯文本（不显示 `[blocked]` 占位）；回复完成后切 `mode="static"` 重渲染（防多余 closer）。全部细节在 `src/components/ai-elements/streamdown-plugins.ts` + `message.tsx:389-540`。

**B2. 聊天内图片呈现** — 用户附件气泡下方缩略图排 + 统一预览对话框；agent 生图/读图在时间线原位显示带标签卡片（改写后 prompt + 图，未到图前骨架占位）；markdown 内本地图片相对路径解析到工作区根后 base64 内联。`content-parts-renderer.tsx:248-287`、`remark-local-images.ts`。ACP 多 agent 场景高频（Read 图片、生图），纯 markdown 渲染会丢。

**B3. Mermaid / PlantUML 图卡片** — ` ```mermaid ` / ` ```plantuml ` 围栏直接出图；卡片自带缩放（改宿主宽度而非 CSS transform，8x 仍锐利）/平移/全屏/复制/导出 SVG/PNG；视口外图用 IntersectionObserver 推迟渲染；过期渲染用 AbortController 取消。PlantUML 是 `@plantuml/core` 浏览器版（本地渲染、无需 Java、不连服务器），渲染串行排队（引擎有模块级状态，并发丢回调）；Graphviz 只是 PlantUML 内部布局引擎（坑：包内 viz-global.js 是 UMD 会被 Monaco AMD loader 弄坏，改从 `@viz-js/viz` ES import 挂 `globalThis.Viz`）。`mermaid-block.tsx:937-950`、`plantuml-engine.ts`。

**B4. json-render 生成式 UI（差异化最大的"哇"功能）** — agent 在回复里写 ` ```spec ` 围栏（json-render 的 patch 流格式，纯数据无 script），聊天内直接渲染交互卡片/表格/Tabs/表单/7 种图表（recharts）。零协议改动，纯前端。安全三道防护：原型污染 patch 过滤 + children 环检测（防无限渲染挂死）+ 错误边界只降级该围栏。技能分发：生成 SKILL.md（含"不许编造数据/聊天列宽有限禁 h-screen/图表放 Card 里"等规则）装进中央技能库按 agent 链接。`json-render-pre.tsx:50-68`、`lib/json-render/{catalog,spec,skill}.ts`、`commands/generative_ui.rs`。

**B5. PDF 旁聊预览** — pdfjs-dist legacy 构建（兼容系统 WebView），postinstall 拷 worker/cmaps/standard_fonts/wasm 到 public（**cmaps 缺失会让 CJK 编码 PDF 渲染空白**）；`PdfPreview` dynamic import 三处共用。`lib/pdfjs.ts`、`pdf-viewer.tsx`。Windows 本地壳看 agent 产出的报告很自然，一天工作量。

### C. 壳的基础设施（第二梯队）

**C1. 网络代理层（中文用户基本盘，不做好代理其他功能都不可用）** — `src-tauri/src/network/proxy.rs`（495 行可作蓝本）。三个实测坑：① 裸 `127.0.0.1:7890` 会被 reqwest 静默修复但 npm 的 `new URL()` 直接 `ERR_INVALID_URL`——边界统一补 `http://`；② `NO_PROXY` 大小写双拼写都要写（reqwest 读大写、curl/Bun/Python/Node 读小写）；③ **回环地址强制加入 NO_PROXY 四种拼法**——否则 agent 子进程的 `ws://127.0.0.1:<port>` 会被发给远端代理，agent 直接死在 session/new（真实踩坑记录）。

**C2. MCP「装一次分发到 N 个 CLI」** — `Settings → MCP` 两 tab：Local 自动扫本机所有 agent CLI 已配置的 MCP 聚合成表（可编辑/重分发/删除，单文件损坏只 warning 不炸列表）；Market 从官方 MCP Registry + Smithery 搜索安装（参数化安装表单来自 registry schema），安装前按 agent 能否承载该 transport 预检（如 Codex 不支持 SSE）。`commands/mcp.rs`（10312 行，`ALL_MCP_APPS` 19 目标 + 编译期穷尽测试）。多 agent 壳第一痛点。

**C3. Skills 中央仓 + 技能×Agent 矩阵** — 中央库 `~/.codeg/skills/<id>/`（experts=Superpowers vendored / science / office / ui-render / custom），启用机制**无 DB 状态纯文件系统**：从 agent 自己的 skills 目录 symlink/junction 到中央副本；矩阵 UI 每行技能×每列 agent，单元格即开关（链接五态：未链接/链接到 codeg/链接到别处/被真实目录挡住/断链）。多个 agent 共享 `~/.agents/skills`，链进共享目录一次点亮多个。`skill-packs-settings.tsx`、`commands/{experts,custom_skills}.rs`。与 Marionette 共享引擎同一心智模型（中央能力 + per-agent 分发），Windows junction 代码可参考。

**C4. 会话落库 + 搜索/标签** — 一张 `conversation` 表（agent_type+external_id 去重、folder、标题、软删），**正文永远留在 agent 自己文件里按需现解析**——证明"只存索引不存正文"就够撑起搜索/标签/排序；搜索只做 title+folder+branch 的 LIKE contains 不做 FTS 也够用；标签两张小表（folder 作用域 + 复合主键链接表）。导入收敛三件套：`(agent,id)` 去重、resilient 批量（单行失败计数不中断）、已有行只做"标题补齐+外部活动时间回填"且 0 语句快路径。`db/entities/`、`import_service.rs:62-310`。列表性能要害：`(mtime,size)` 指纹 memoize 每文件摘要 + **解析后复查指纹**（读期间被写入不入缓存）。

**C5. 自有 ACP 转录** — 把 `session/prompt` + `session/update` 原样 JSONL 落盘（**存外部 wire 格式而非内部事件类型**，防 refactor 腐化历史）。免费获得：无法逆向原生 store 的 agent 的完整历史（Codeg 对 ZCode 就这么做的，"不扫原生 SQLite 会话"）、跨 agent 统一语料、usage/timing。`acp_transcript.rs`。直接补 Marionette「zcode 导入只覆盖 db.sqlite」的缺口。

**C6. Agent 生命周期管理** — ① **pin + 未审计升级双轨**：每适配器钉审计过的精确版本，另给 "Upgrade to unreviewed latest" 按钮（npm 查 dist-tags latest，binary 从 archive URL 反推，**模板不匹配就不提供升级而不是给 404**）。② **诊断 verdict**："为什么起不来"浓缩成一句话（GUI PATH 缺口 / 装了官方 CLI 但壳要的是另一个 ACP 适配器——后者是 Codeg **被报最多的 bug**）。③ **ACP registry 目录**：agentclientprotocol.com 官方 registry 搜索一键添加（平台无 binary 自动落 npx；添加时查 npm 元数据解析真实 bin 名；`kimi/qoder/kiro` 上游不同名要防重复）。`acp/{registry,latest_release,preflight,remote_registry}.rs`、`commands/acp.rs:1294-1451`（compute_verdict）。npm 安装健壮化：固定官方 registry + `--include=optional`（CodeBuddy esbuild 坑）+ EACCES 重试用户 prefix + EEXIST `--force`，全程流式输出。

**C7. dev server 发现 + 预览 tab** — 从**终端输出**扫 dev server banner（`Local: http://localhost:5173/`、`Uvicorn running on…`；仅 loopback、仅整行、保留 query、0.0.0.0 改写 localhost）→ TCP connect 四次重试探测（0/250/750/1500ms，覆盖"打印早于就绪"）→ 活的记入并 emit 事件 → 工作区自动/手动开预览 tab（内嵌 webview）。Windows 友好（不依赖 lsof//proc），可几乎原样搬。`browser/{service_url,services}.rs`。

**C8. LLM commit message 生成** — 版本面板 commit 输入框旁 ✨ 按钮：读本次变更 diff → 组 prompt（含用户草稿 hint、常备指令如"用中文写"、nonce 防缓存）→ 一次性隐藏会话回答 → 清洗回填，可选 auto_commit。三级 fallback：显式设置 → 当前会话 agent → 文件夹默认 agent。`commit_message/`。Marionette 可用共享引擎一次 API 调用做得更轻。

**C9. URL scheme** — `codeg://session/<id>` 外部唤起直达会话；冷启动竞态处理：后端单槽 pending slot + 无载荷 nudge 事件，前端 mount 后原子 take。`deep_link.rs`。与 Telegram 桥联动（推送带链接点开直达会话）体验闭环，一天工作量。

**C10. ProjectShelf 增强点** — 每项目**默认 agent**（`default_agent_type`，新会话/commit 生成 fallback）；分组与散项目**共用一个排序空间**（交错而非两段）；package.json scripts 自动成为项目命令（按 lock 文件识别包管理器）；worktree 扁平父子挂在根下。`db/entities/folder*.rs`、`commands/folder_commands.rs:13-50`。

**C11. Token 用量分析页** — 趋势堆叠柱（缓存读 vs 新鲜算两色）、缓存命中率环（`cacheRead/(input+cacheCreation+cacheRead)`，故意排除输出 token）、活动热图（小时×星期，**在 Rust 按查看者本地时区折叠而非 SQL 字符串格式化**）、按文件夹/agent/模型排名（>7 折叠进 Other）、API 牌价折算（未定价 token 标明覆盖率）。物化表 `token_usage_turn`（整会话事务性整组替换）+ `token_usage_sync` 游标（`source_updated_at` 与会话 updated_at **精确不等**即 stale，防同毫秒/时钟回拨）。`token-usage-page.tsx`、`commands/token_usage.rs`。

**C12. 主题系统** — 12 主题 CSS 变量预设 + token 级行内覆盖（写 `<html>` 行内 style，优先级天然高于 data-theme 规则）+ `--radius` 单变量乘法派生 7 档 + 聊天宽度滑杆 + 自定义 CSS 注入；壁纸原图字节**原样存储不重编码**（GIF/APNG 动图能播）+ wallhaven 市场（SFW 硬编码 + host allowlist 每跳重定向重校验）。文件夹颜色只染标题文字（对比度预算 ≥5.4:1 AA）。`theme-presets.ts`、`backgrounds/`。

**C13. turn timings 补时钟** — 为原生 store 无时间戳的 agent（动因 Cursor）在连接层自记 JSONL（`prompt_sha` 只存 SHA 前 16 hex **不存明文**）；读回六层防御匹配（版本/单调/尾段连续/计数守卫/尾锚哈希回走/同哈希游长相等），原则"宁可无钟，不给错钟"。`turn_timings.rs:35-110`。

### D. 多 agent 协作与外部通道（中期/按需）

**D1. `@` 委托（MCP 伴生进程）** — 每会话注入一个 stdio MCP 伴生二进制（Windows named pipe + token 鉴权，父连接 teardown 时 revoke），暴露 `delegate_to_agent`（异步，立即返回 task_id）/`get_delegation_status`（`wait_ms` 省略=快照、>0 阻塞上限 60s、任一 terminal/blocked_on 提前唤醒）/`cancel_delegation` /`resume_delegation`。broker 在主进程 spawn 子 ACP 会话（可以是不同 agent 类型），进度流回父对话：事件 + meta patch 到工具卡（`codeg.delegation`，崩溃后有兜底注入）+ 子 agent 卡权限时卡片自动展开**在主对话里直接替子 agent 批准**。深度限制默认 1 级；正常 end_turn 不取消子任务（后台继续跑）；结果缓存 512MB FIFO（挤掉后回落 DB 只查状态）。**工具 schema 文案是调教过的**（"等待中不要说话、blocked_on 是唯一该开口的情形"——没这段教学 LLM 会把轮询打成碎碎念），照抄。`acp/delegation/`（broker.rs 头注释即完整生命周期）、`bin/codeg_mcp.rs`。

**D2. @assistant（委派预设）** — 不是直连 LLM，是"名字/头像/描述 + 底座 agent + 该 agent 全部选项的显式选择（如 model）+ 常备指令"的预设；composer `@` 即用；子会话首 prompt 与每个 resume prompt 尾部附 assistant brief 帧（≤256KB）。`models/assistant.rs`、`delegation/assistants.rs`。与 Marionette 共享引擎天然契合：@assistant = agent CLI + 共享引擎模型路由 + 常备指令三合一。

**D3. worktree 无人值守任务（"收菜"）** — 任务四列看板（To do / In progress / **Needs you** / Done），每任务在自己的 `git worktree add -b task/<id>` 里跑，互不踩；跑完进 review（diff 预览 + verdict + preflight 灯）→ 一键 merge（**merge 本身让 agent 在任务会话里执行一个 turn 解冲突**；A 段 base merge INTO worktree——冲突留在 agent 能看到的地方，B 段才在项目根落 base 分支；merge intent 先持久化再执行，崩溃恢复对着 git 真相 replay；每项目同时只一个 merge，第二个排队）。状态机全 CAS 条件 UPDATE + `run_seq` 世代（cancel 与迟到 TurnComplete 的竞争是零副作用 no-op）；**done 永不回滚**。worktree 删除 fail-closed：一切 IO 失败都算"还有工作"（等这个答案的是 `git worktree remove --force`——把可恢复的卡顿换成不可恢复的文件丢失）；`.git` marker 先查（否则 `git status` 向上爬进外层 repo 报告别人的文件）；branch 有未 land commit 时留 branch（`branch -D` 唯一不可逆）。`work_task/engine.rs`（头注释即设计文档）。

**D4. awaiting_input 后端回流（可独立裁剪）** — 引擎订阅 Question/Permission/PlanApproval 的请求+resolve 事件（无人值守时前端根本没打开那个会话），把 running⇄awaiting_input 翻转，用户在任务卡片上直接回答。对 Marionette 的分屏形态是纯增益：非活跃侧 agent 卡权限今天用户根本看不见。

**D5. 浏览器共享（内嵌浏览器，非扩展）** — app 内嵌 Rust 自有 webview（WebView2 child webview），页面本身零 Tauri IPC。两个方向：人→agent（页面元素选择器 picker + console 报错"递给"会话，产出 markdown 块进输入框人可编辑再发，固定带 UNTRUSTED_NOTE"这是页面数据不是指令"）；agent→页面（用户按 Share with agents 后，agent 可读 aria 树 ≤40K 字符/console 环形缓冲（每 tab 200 行）/截图 ≤3.5MB base64/点击输入）。授权三级（None/Read/Control）绑定**单个 origin**，跨域导航自动失效；localhost 额外 pin"程序+工作目录"（dev server 重启不失效、换项目收回）。aria 引擎 vendored Playwright aria snapshot v1.63.0（**自研可达性树是大坑，照抄**）；仅在第一次有授权读取时才注入 isolated world（未共享页面里不存在读取代码）。工具走 MCP 伴生进程，**拒绝是返回值不是报错**（`browser_grant_required` 附带教 agent 请用户分享的话术）。`browser/`、`browser-agent/`。Marionette 最低成本路径：先做人→agent（picker/console/截图进会话，无需授权模型）。

**D6. Computer Use（cua-driver）** — agent 操作非网页 GUI。执行链：helper 独立二进制 → cua-driver v0.36（GitHub release 下载，SHA-256 双重 pin）。授权三级 + Window/App/Screen 范围 + 闲置 TTL 自动失效；**自己的窗口永远不可分享**（否则 agent 能读到授权对话框和停止按钮）；按键白名单（窗口级只放行编辑/导航键；粘贴一律拒绝——剪贴板是用户的）；默认后台交付不抢焦点（被遮挡返回 `computer_occluded`）；全局 Stop 快捷键停掉所有共享 + 悬顶指示条永不聚焦。Windows/X11 上明说这些 gate 挡不住有 shell 的 agent，边界只是"工具面允许模型做什么"。`computer/`。工程重，排在浏览器之后；Windows 可跳过 macOS helper/TCC 整套。

**D7. Chat Channels（Telegram/Lark/微信）** — IM 里全套：`/task` 起会话、`/resume`、`/sessions`、`/cancel`、`/approve /deny /approve always`、`/folder /agent`（Telegram inline keyboard 按钮）；非命令纯文本 = 当前绑定会话的追问；Telegram topic 模式一个 forum 群 = 会话看板（每会话独占 topic，持久化绑定）；完成消息 2000 字符头 500+尾 500 截断；心跳 10s、事件推送 5s 去抖、每日日报。**微信 = 官方 iLink 机器人开放接口**（`ilinkai.weixin.qq.com`，bot_type=3 扫码，个人微信、无需企业认证）：get_bot_qrcode 拿码 → 个人微信扫 → get_qrcode_status 得 bot_token+base_url → getupdates 游标长轮询收 + sendmessage 发；`context_token` 过期（ret=-2）缓冲最多 50 条等下一条入站刷新再补发；被动模式（用户先说话才能回）。`chat_channel/`。**微信通道对中文用户价值极大**（TG 要梯子，iLink 扫码即用，API 细节可直接照抄）；Marionette 已有 Telegram 桥的 MarkdownV2 分片比 Codeg 的截断体验好，保留；差多会话模型 + `/approve always` + `ChatChannelBackend` trait 抽象。

**D8. Automations / codeg-cli / forge（长期）** — Automations：composer 配置快照 JSON **整体重放从不解析查询** + cron（IANA 时区）+ `worktree_per_run`/`shared_in_root` 隔离；进程唯一靠 data 目录独占文件锁（fails-closed 不启动）；reconcile backstop 30s + 未跟踪 run 超 180 分钟 force-fail。codeg-cli：**runtime-owned**（CLI 挂了任务照跑、结果 bounded 队列可再取；`--permissions` policy 即时答权限、`--output-schema` 结构化结果提交、fanout 多目标）。forge（GitHub/GitLab/Gitea/Forgejo issues/PR 变任务）：provider 服务端从 remote 推导；issue 正文/评论当不可信数据单独 envelope（prompt 注入面）；手动触发和自动认领**共用同一条 create 路径**；PR head OID 触发时刻 pin 死；PR adopt 四元组全匹配否则弹回 review。

**D9. 远程工作区/手机接力（存档备用）** — 桌面内嵌 web 服务（3080+token，WS 子协议带 token——浏览器 WS 加不了 header 的标准解法）；远程工作区多窗口 LRU 重放恢复；**浏览器隧道**：为远程连接起只绑 127.0.0.1 的 SOCKS5 listener，连接打包成流过一条 WebSocket 到远端代连（字节级透传，TLS/WS/HTTP2 不动）——若 Marionette 将来做"远程预览 agent 起的 dev server"，SOCKS-over-WebSocket 是正确形态。

**D10. gallery 开发页（工程实践）** — 文件名故意叫 `page.gallery.tsx`，仅 `CODEG_GALLERY=1` 构建时通过 `pageExtensions` 成为路由（release 永不包含）；把 fixture 假回复灌进**真实渲染管线**并排展示每种消息步骤类型与状态（流式逐 step/失败/委派/ranked plan/…），可切 i18n 语言。比手工造会话快一个量级，可当视觉回归基线。`src/app/activity-gallery/`。

### 已排除 / 暂不考虑

- **桌面宠物**（pets/，兼容 Codex /pet 格式 + 市场 + 会话状态映射）：用户明确不需要。
- **Project Boot**（可视化脚手架，live preview 是官方 ui.shadcn.com 站点 + URL 参数编码）：与 Marionette「壳不是脚手架产品」定位不符。
- **科研技能包内容**（13 个 MIT 技能）：机制（中央库+链接）值得抄（见 C3），内容本身对目标用户价值一般。
- **Office 实时预览**（office_watch：每文件一个 `officecli watch` 长驻进程 + ref-count + cap token，替代每次重跑 `officecli view`——旧路径 Windows 上和 agent 编辑**抢同一把文件锁**报 "file is in use"）：依赖外部 OfficeCLI，起步可先 `officecli view html` 一次性渲染；**「预览器与编辑者共用一个 watch 进程」的 Windows 文件锁解法**值得记住。
- **无限画布/三栏 merge editor/Monaco 编辑器**：离轻壳定位远，最后考虑。

---

## 二、坑清单（提前规避；全部有 commit hash 或文件证据）

### 1. 进程管理（Marionette 一定会撞）

| 坑 | Codeg 教训 | 对策 |
|---|---|---|
| 退出杀不死 agent | 依赖 async Drop 杀进程，Tauri 退出后线程没机会跑，agent CLI 及 MCP 子进程活到 stdin EOF（~30s）还锁二进制挡升级（`5fe6e7a9`） | 退出路径**同步 kill_tree 兜底**：spawn 记 pid，500ms 宽限后强杀 |
| 裸 Child 被 drop | tokio 对被 drop 的 Child 只悄悄 reap 从不 kill；没被 poll 的 future 留活孤儿（`9005ded2`） | 审视每个 Child：drop 时谁负责杀 |
| 孙进程永生 | MCP 伴生进程是孙进程，Windows 不传播父死亡，挂死的中间层吞 stdin EOF（`95f0501a`） | 若做 @委托抄 `--parent-pid` watchdog：父死硬退（先 drain 再 exit） |
| spawn→注册间有 await | 中途 shutdown 留无主幽灵（`8214e7a6`） | 注册路径不可 yield |
| PTY EOF ≠ 进程退出 | 终端缓存驱逐无脑 kill+wait 会**卡住关窗路径**（`c8547c36`） | 区分"终端关了"和"进程死了"，只在 child 已退出才 reap |
| Stop 先关 PTY 输入 | close 输入会发 newline+EOF——活程序死前收到 Enter，echo 的 `^D` 混进保留输出（`8c053003`） | kill+wait 之后才关输入 |
| Tauri setup 里 tokio::spawn | setup 回调在 runtime 外，直接 panic（`781759d5`） | 一律走 `tauri::async_runtime` |
| `cmd /C` 拼接注入 | 文件名 `a&calc` 真的会启动 calc（`4eaa4502`） | `.cmd` shim 作为 program 交给 std 构造 batch 专用引用 |
| ETXTBSY | Linux fork 持有刚写脚本的写描述符（`3eecf597`） | spawn 带 1s 预算退避重试 |

### 2. 编码（中文 Windows 用户必踩）

- **所有子进程输出 lossy 读**：npm/安装流按严格 UTF-8 读，遇中文 Windows GBK 本地化错误文本静默截断、stderr 空、失败无细节（`650f205f`）。
- **统一 spawn 入口强制 UTF-8**：注入 `PYTHONUTF8/PYTHONIOENCODING/LC_ALL=C.UTF-8`，PTY 按 shell 配 `chcp 65001`（cmd）/`[Console]::OutputEncoding`（PowerShell）（`25def31a`）——与我们 PTY UTF-8 截断 panic 同源，他们在入口统一解决。
- **按字节切片字符串全库扫**：标题截断、API key 脱敏都 panic 过多字节边界（`ce951482`/`71e218b7`）——我们已有同款教训，值得复查清干净。
- API key 掩码按 **Unicode 字符**掩码防 CJK 输入 panic（`models/model_provider.rs`）。

### 3. Windows 特有

- **`Path`/`PATH` 双键**：Windows env 大小写不敏感但键可并存，逐条 apply 时旧值覆盖注入值——注入前全删再插一个（`connection.rs:1057-1105`，含回归测试）。
- **清缓存/自更新遇 Defender 锁**：`ERROR_ACCESS_DENIED`（运行中 exe 或 Defender 扫描持锁）时用 **NTFS rename 进 `.trash/`**（子项被锁的目录改名仍成功），后台慢慢清（`f264f560`）。
- **PowerShell + WMI 查进程会无限阻塞安装器**——改 Windows API 直接结束进程；WMI 查询加 15s 上限回退 Get-Process（`7756455e`）。
- **多组件共存"按名字杀进程"是事故面**：server 和桌面 app 装同一文件夹互相覆盖、installer 按名字杀对方 helper；32 位 PowerShell 看不见 64 位进程（`d6719a55`）。
- **`COM¹`/`LPT¹`（上标数字）是保留设备名**——路径校验黑名单要含 Unicode 形变（`e6a37660`）。
- **裸命令名按 `.exe→.cmd→.bat` 解析**：`CreateProcessW` 找不到 npm shim 会让 agent 挂死或 ENOENT；`cmd` 撞系统 shell、`agent` symlink 互相覆盖（Grok/Cursor）。
- **npm shim 要解析到真实 JS entry 再跑**（`%dp0%` 正则），绕过 shim 里的旧 Node；Windows `C:xxx` drive-relative 会被 `Path::is_absolute` 漏过，override 一律拒绝相对路径。

### 4. Tauri / tao / wry（我们已付过学费的同款）

- Tauri JS `listen()/unlisten()` **注册/注销竞态**：依赖 registration eval 填充内部表，注销早于注册落表 → 间歇 TypeError + 监听器泄漏。修法：直调 `plugin:event|listen/unlisten` + 客户端注册表守卫（`5dd1deb9`）。
- `WebviewWindow::url()` 在首导航失败的 surface 上 **unwrap panic 带走整个 app**；wry 无导航失败回调、主机不响应 tab 永远转圈（`56612bdc`）。
- **tao/wry 行为承诺必须实测不能靠文档**：他们建立在一个 tao 根本不成立的前提上（`941c1a9c`）；升 Tauri 要逐条对 tao/wry changelog 做行为回归（tao 0.37 Windows 不经激活显示 action mark 等）。
- 升级更新（Windows）：下载只下载+验证，"Restart to update" 才跑 installer；验签/杀软持锁时把 exe **move aside**、没写成新 exe 就放回（`43a6612f`）。
- panic hook 全入口装（desktop/server/mcp/supervisor/credential helper）：Windows abort 是 `0xc0000409`，没有 hook 用户只有 WER 桶和戛然而止的日志（`eefd53ac`）——我们已有 dev.log `[panic]` 通道，方向确认。
- supervisor 自更新：新二进制试用期崩溃自动回滚 `.bak`，**不依赖 HTTP rollback 端点**（新版本挂了恰恰不可达）；试用起点时间戳必须先于 spawn（`de5cf7c3`）。

### 5. 流式 UI（虚拟列表 + 实时轨迹图最先撞）

- **底部抖动**：虚拟列表停在离底一点点 + 行挂载高度≠稳定高度（截断块未测完/图片未载完）+ 分数缩放 1px 修正差 + 吸底锁滑回 = 每 ~430ms 跳一次。修法：接近末尾时把缓冲区到末尾的行全 keepMounted 按 key 持有（`0148598`）。
- **跳转与吸底锁打架**：点导航跳上去被吸底锁拽回——跳转要主动释放锁且等 resize 稳定（`e5a91ed6`）。
- **流式重渲染风暴**：每个 delta 整树重建是性能杀手——行级 memo 以 step key+内容为界、解析结果按内容缓存、测量只在布局后一次（`0ad1eaa0`/`d968412b`/`d5a9dd0f`）。
- **滚动期间全局 restyle**：虚拟化容器 `pointer-events:none` 可继承，开关各触发一次全树 restyle——几百行时间线一帧几百 ms（`c83aab6c`）。
- **组件内状态活不过一次滚动**：展开/折叠状态必须外置（见 A7）。
- **转动的动画元素必须是无子元素 span**：转 SVG 每帧重排整页（`rail-icon.tsx:113-119` 注释）。
- **affordance 消失引起布局跳动**：fork 按钮忙时变灰而不是消失（整行图标位移闪烁他们修过好几轮）。
- **流式工具输出不要全量快照反复广播**：cumulative 快照 O(N²)，多 GB 瞬时分配崩 webview——bounded 增量 delta + 尾指纹 + 64KB 单 chunk 上限按 char 边界截断（`bcfaba10`）。
- RTL 是系统性坑（4 个独立 fix）：diff 代码强制 `dir="ltr"`、`+/-` 统计整体 ltr 包裹、chevron 方向、`dir="auto"` 按内容方向。
- 运行中的卡不能被折叠/虚拟化吞掉：运行中压缩卡保持可见（`5034776f`）；不能折叠的回复也有"跳到失败"路径（`e1907019`）。

### 6. agent 适配器版本漂移（acpSupplements 体系会持续撞）

- **release notes 不可信**：claude/codex 多个关键 wire 变化不在 notes 里（codex 1.2.0 把 goal 通道 `_meta.codex.goal`→`_meta.goal` UNANNOUNCED，靠 initialize 锁定通道选择才没炸）——**pin 表每次 bump 做 tarball diff + 真机 replay，注释记录 wire 变化**（registry.rs 的注释纪律本身就是工程资产）。
- **"能开 opt-in" ≠ "opt-in 是对的"**：claude 0.64.0 steering 有 bug（被 steer 的 turn 结果把 prompt settle 成干净 end_turn），0.65.0 才修——版本门槛设在修好的版本。
- **CLI 自更新漂移 pin**：Grok `--no-auto-update`、Cline `CLINE_NO_AUTO_UPDATE=1`；Cursor 同日双 build 版本号相同要比 build hash。
- **Node 门槛是运行时守卫**：OpenClaw 把 engines.node 22→24 且带运行时硬守卫——"Node 22 用户过了 preflight、launch 硬崩"；`node_required` 跟最低支持版本。
- **optional 依赖被跳过 = 启动崩**：CodeBuddy esbuild optional 却无条件 require——npm 装 `--include=optional`。
- **"假成功"安装最糟**：Antigravity archive 有两个可执行文件，单文件 copy-out 装完能握手但 agent 废——`required_siblings` 机制。
- **权限层中途重写**：claude 0.70-0.73 删了 option 级 `_meta.permission.changes[]`——渲染代码新旧双轨。
- **外部可写配置静默失效**：deepseek-acp 0.9.0 imageDetail 第一行 throw——一条模型配置写错 = 整份 section 静默回退，用户无感知。回读/预检必须有。
- **authRequired 从 end_turn 改成 prompt 拒绝**（claude 0.74.0）：不把 `ErrorCode::AuthRequired` 保持在 turn 范围内处理，会把整条连接拆掉、会话翻成 Cancelled。

### 7. MCP 注入（逐 agent 验证，没有通用规律）

- OpenClaw 拒绝 `mcpServers` 任何条目（空 `[]` 反而能过）；Cline 收下不启动（改写原生 settings 文件）；pi-acp 收了就丢（wire 短路）；codex-acp 过滤项目 `.codex` 层（强制 `DISABLE_MCP_CONFIG_FILTERING`）；十来个 agent 原生配置+wire 转发会**双注册**，而 DeepSeek/Codex 只认 wire（`connection.rs:6926-6980` 长注释）。
- MCP 工具名识别要在 `__`/`/`/`:`/`.` 四种边界切，近亲不算——它决定 auto-approve，认错一个就少弹一张批准卡。
- Cursor 把 MCP 工具 announce 成匿名的（"MCP: tool" 无参数）——需要 round-trip 后重写 title 才有卡片绑定。

### 8. 会话解析容错（对 Marionette parsers 直接适用)

- 半写最后一行逐行跳过即可；**时间戳探测用字节窗 + from_utf8_lossy** 别 read_to_string。
- **Claude resume 重放历史**按 uuid 去重，否则消息数翻倍、时间戳倒流。
- 别人的 SQLite：只读打开 + busy_timeout；崩溃后 wal-index 需恢复时报 `SQLITE_READONLY_RECOVERY`，降级读写打开让 SQLite 自愈；**绝不动别人的活库**（不 VACUUM；备份只读 page-copy；恢复前删 live -wal/-shm）。
- 格式漂移实测集：Grok `tool_call_update.content` 是**累计**的要 100KB cap；Qoder 失败回合写成真 assistant 记录 `model:"<synthetic>"`（直接渲染会把供应商错误 JSON 放进模型嘴里）；Hermes 时间戳是 epoch 秒 REAL 浮点（别家毫秒 int）；Kimi 工作目录只在 index 文件；Codex 线程名只在 session_index.jsonl。
- 时长推断：每个子回合都从 prompt 起算会把 9 分钟报成 49 分钟——"铺砖"而非重复计时。
- usage 缺失保持 `None`，写 0 会显示成"这次回复免费"。
- 坏时间戳别 `unwrap_or_default()`（静默变 epoch 0）；日/周桶在 Rust 按查看者本地时区折叠。
- transcript JSON 里的 id 直接拼路径可逃逸目录——拒绝 `/ \ .. : \0`（Unix 也拒 `:` 防 Windows `C:x` 替换整盘）。
- agent 设置里改 `CLAUDE_CONFIG_DIR`/`CODEX_HOME` 不会传到宿主进程，parser 拿 inherited env 读错目录——**per-agent env**（issue #948）。

### 9. 并发/存储/治理

- SQLite deferred BEGIN 读快照升级写锁失败 `SQLITE_BUSY_SNAPSHOT(517)`，**busy_timeout 不覆盖**——写优先事务（`d921d42b`）。
- 多面板共享一条 ACP 连接：路由表必须一対多且 detach 引用计数，否则后 attach 的偷走路由、没有任何机制能把状态拉回来（卡 "responding" 永远，`1e3e5a10`）。
- prompt 发送加**每连接互斥**串行化 link-check+DB 写+发送（`d47288be`）。
- TMPDIR 坑：从 agent 会话里启动的 codeg 继承会话 scratch TMPDIR，启动清扫把它当孤儿删了自己正在用的 temp；tmpdir 嵌套多 ~25 字节让 macOS sun_path 104 字节爆掉，症状是离奇的"关掉 MCP 支持"。
- stderr 是泄密面：agent 错误回显 `api_key=sk-…`——环形缓冲写时脱敏 + 模板重建；debug 日志风暴曾写 217GB。
- 空闲连接回收：3 分钟无活动断开（浏览器关 tab 无 hook、桌面崩溃），活跃连接永不清。
- 事件时区/游标：token 同步游标用精确不等而非 `>`（防同毫秒/时钟回拨）。

---

## 三、ZCode 专项（同一社区桥 zcode-acp-server 0.76.2）

Codeg 的 `docs/agents/zcode.md`（他们唯一的 per-agent 文档）确认了几条我们文档没记的：

1. **`ZCODE_ACP_RUNTIME=node`**：桥发现 Bun ≥1.4 会把自己迁移到 Bun 跑。我们钉了 `ZCODE_NODE`（给 .cjs 后端找 node），但没记这条——用户装了 Bun 时桥的运行时会不受控。**建议 spawn 时一并注入，补进 `pitfalls-zcode-acp.md`**。
2. **Start Plan 账号过不了桥**：模型请求的 captcha 只有桌面 app 能解，每条 prompt 都失败——不可修的上游限制，UI 层值得给 Start Plan 用户提前说明。
3. **新会话首条 prompt 前必须显式确认 Build mode**（已保存 mode 含 Yolo 优先）；确认后桥**在连接时就创建后端会话**而不是等首条 prompt。
4. **附件限额**：每 prompt ≤100 内容块 / 50 MiB 解码总量 / 单图 10 MiB（PNG/JPEG/WebP/GIF/BMP/SVG）；发二进制附件前要暂停原生 goal loop（排队 goal prompt 只收文本）。
5. **多轮内部循环（/auto 等）**：usage 每轮加总、时长以宿主测量为准；**取消的回复记录不到 usage**（桥只在回合结束后上报）——解释 zcode 上可能的 usage 缺失。
6. **消息级 fork 被整个禁用**（桥的 fork/checkpoint 行为与类型化流程不兼容）——不留半残功能的取舍值得参考。
7. 相对路径 override 一律拒绝（连 Windows `C:xxx` drive-relative 都拦）；无效显式 override 直接失败而不是换一个安装——与我们做法一致，方向确认。
8. `ZCODE_ACP_SANDBOX=1` / bridge 配置 `sandbox.enabled` 才会写 `.zcode/acp/sandbox.json`（默认不写）。

---

## 四、行动优先级建议

1. **马上做**（低成本高感知）：权限卡排队计数；命令活尾巴 + 失败自动展开锚底 + 10s 时长阈值；折叠状态外置；通知四条门控语义；`ZCODE_ACP_RUNTIME=node` 注入 + 文档补课；NO_PROXY 代理规范（若尚无）。
2. **短期**（结构级）：回合活动胶囊（与 tally/交付卡合体）；命令组"走过即收"；steer + 消息队列；诊断 verdict；agent pin + 未审计升级双轨；流式 markdown 管线升级（懒加载 + `singleDollarTextMath:false` + static 重绘）；聊天内图片呈现。
3. **中期**（能力面）：自有 ACP 转录落盘 → 会话落库 + 搜索/标签；MCP 本地扫描 + registry 分发；Skills 中央仓 + 矩阵；dev server 发现 + 预览；`@` 委托（MCP 伴生进程三工具起步）+ @assistant 预设；awaiting_input 回流；token 用量分析页；URL scheme + Telegram 桥联动；微信 iLink 通道。
4. **长期/按需**：worktree 无人值守任务（"收菜"）；automations；codeg-cli；浏览器共享（先人→agent）；Computer Use；forge；Computer Use 之后的远程工作区（SOCKS 隧道设计已存档）。
5. **防御性复查**（拿坑清单对我们的库过一遍）：退出路径同步 kill_tree；子进程输出 lossy 读 + spawn 入口强制 UTF-8；按字节切片复查；虚拟列表底部抖动/跳转与吸底锁打架预防；`Path/PATH` 双键注入；panic hook 覆盖面。

> 分报告（8 份，含更细的文件行号证据）在本文件派生过程的探索代理输出中；深挖任何一块时直接到 `D:\Projects\codeg` 对应文件。
