# 路线图：从 Codeg 借什么、什么不借（2026-10-25）

依据：[`research-codeg.md`](./research-codeg.md) 的全量调研，对照 Marionette 现状（2026-10-25，dock 阶段 0–4 落地后）逐项核实写成。
工作量标记：S ≈ 半天内，M ≈ 1–2 天，L ≈ 一个迭代以上。条目后括号是 Codeg 实现指针（相对 `D:\Projects\codeg`）。

## 定位原则（取舍的依据，先于一切条目）

1. **轻壳不做 IDE**。Marionette 的差异化是贴边停靠 + 收纳盒 + 模型共享引擎；编辑器/画布/merge editor 这类重工作台件不进。
2. **用户每天碰到的那一层优先**：对话流可读性 > 会话管理 > 能力分发 > 协作 > 外部通道。
3. **壳不实现 runtime**（Codeg 同款哲学，agent 能力借 CLI 之势）；但壳层自有 LLM（共享引擎）是我们比 Codeg 强的点——凡是 Codeg 要"起一个 agent 进程"才能做的小功能（commit message、@assistant），我们用引擎一次 API 调用做。

## 现状对照（已核实的家底）

**已对齐——不用抄，直接划掉**：

- 代理层：`src-tauri/src/proxy.rs` 已注入 `HTTPS_PROXY/HTTP_PROXY` + `NO_PROXY` 回环四拼法 + 大小写双拼写——Codeg 的代理三坑我们已免疫两个（剩 scheme 补全值得顺手复查）。
- 跨 agent 交接（提示层）：`handoff.rs`（切 agent 生成 handoff + composer prefill）+ `delegate.ts` 的 **@ 委托**（行首 `@agent/model` → 子会话 + 父对话委托卡 + 子流展开，`types.ts` 的 delegate child sessions）——Codeg @ 委托的用户形态我们已有（**用户手动路由**）。差的是 **LLM 自主委托**（agent 的 LLM 自己调 MCP 工具并行 fan-out，移 P4 按需）。
- MCP 借出：`context_inventory.rs` 扫各 agent 全局+项目两层聚合成表、按项目 `.marionette/context.json` 决定借出、`mcp_payload_for_agent` spawn 时注入 + zcode 原生配置桥（stdio）——Codeg「Local 扫描+分发」的借出半边已有。差的是**市场安装**与**跨 agent 配置写回管理**（P2.2）。
- Skills 借出：`context_inventory.rs` 的 SkillSpec 扫描 + `skills_prompt_for_agent`（prompt 级借给没有该技能的 agent）。差的是**文件级分发（junction 中央仓）+ 技能包 + 矩阵 UI**（P2.3）。
- Fork from reply（`sessionFork.ts`）。Steer 还没有（P1）。
- 会话导入：12 个 parser（claude/codex/gemini/opencode/zcode/deepseek/grok/hermes/cline/cursor/pi/jsonl_agents）——Codeg 19 个 agent 的原生 store 大部分我们已能读。
- 自定义 agent：`CustomAgentDialog` 已有 registry / manual 双 tab——ACP registry 安装已有入口。差 pin 表 + 未审计升级 + 诊断 verdict（P2）。
- 会话搜索：Ctrl K 面板已有模糊匹配最近对话（`matchScore`）。差跨 agent 索引/标签（P2）。
- 权限/ask/plan 卡、交付卡（TurnDeliveryCard）、实时轨迹图、LiveThoughtPreview、消息大纲（MessageOutline）。
- Telegram 桥（通知/回复/远程批准/配对）、单会话统计（SessionStatsSection）。
- 图片基础：ImageLightbox / ImageAnnotator / ToolImageStrip / imageAttachments。差生图卡片与 markdown 本地图内联（P1）。
- desktopNotify（声音/闪烁/进度 overlay）。差门控语义（P0）。
- panic hook 落 dev.log、data dir gate、单实例交接。

**部分差距（路线图主攻区）**：markdown 只有 react-markdown+gfm（无公式/图表/代码高亮流式管线）；storage.rs 已有每会话 transcript JSONL 但**格式待核对**（Codeg 教训：存 wire 格式防 refactor 腐化，见 P2.1）；MCP/Skills 已有扫描+借出（context_inventory），差市场/写回与文件级分发；通知有通道缺语义。

---

## P0 速赢（本周，全部 S/M 级）

1. **权限卡排队计数 Badge**（S）——"还有 N 个排队"，不显示排队数用户会以为 agent 卡死。（`permission-dialog.tsx:149-153`）
2. **命令活尾巴 + 失败自动展开锚底 + <10s 不显时长**（M）——折叠行露末 3 行输出（去 ANSI、500ms 淡入）；失败展开且锚定底部；`SHOW_DURATION_MS=10000`。（`command-rows.tsx:41-48`）
3. **折叠状态外置 store**（S）——模块级 Map + `useSyncExternalStore`，虚拟化卸载/流式重建后开合不丢。（`disclosure-store.ts`）
4. **通知门控四条语义**（M）——FlashWindowEx 后发覆盖先发（级别互护）；focus vs hidden（第二显示器可见未聚焦该收通知）；每事件 3s 冷却合并；断线重连回放抑制。（`desktop-notification.ts`、`window-attention.ts`）
5. **`ZCODE_ACP_RUNTIME=node` 注入**（S）——桥发现 Bun ≥1.4 会自我迁移；一并补进 `pitfalls-zcode-acp.md`。（`docs/agents/zcode.md:14`）
6. **防御性排雷并行启动**（不产出功能，对照 `research-codeg.md` §2 逐条过）：退出路径同步 kill_tree 兜底；子进程输出 lossy 读 + spawn 入口强制 UTF-8；按字节切片全库扫描；`Path/PATH` 双键注入序；流式 delta 禁止整树广播（现有实时轨迹图重点查）。

## P1 对话流骨架（1–2 个迭代）

1. **回合活动胶囊**（M/L）——与现有 tally + 交付卡**合体成一行**：步数 · 结局（失败红色可点击跳转）· 耗时；运行中 `Working N steps`；无步骤回复保留占位（槽位永不闪烁进出）；分类明细只进 tooltip。（`activity-header.tsx`）
2. **命令组"走过即收"**（M）——`followed/ended/slideShut` 状态机；折叠后 live 成员顶出；未恢复失败强制展开。（`command-rows.tsx:546-638`）
3. **Steer + 消息队列**（L）——向运行中回合插话；队列可拖拽/编辑/删除；⚡（即时通道）与 🕐（下次拉取）图标如实表达；`canSteer` 如实禁掉点了没用的行；能力探测复用 capabilityCache。（`message-queue-display.tsx`）
4. **markdown 富渲染管线**（L）——懒加载三引擎（纯字符串扫描命中才 import：shiki/katex/mermaid）；`singleDollarTextMath:false`（单 `$` 永远字面文本）；流式 remend + 未闭合链接先显示纯文本；完成后切 static 重绘。**这是 B 类一切的地基**。（`streamdown-plugins.ts`）
5. **图片补齐**（M）——agent 生图/读图原位卡片（改写后 prompt + 图 + 骨架占位）；markdown 本地图片相对路径解析到工作区根后内联。（`content-parts-renderer.tsx:248-287`）
6. **PDF 旁聊预览**（S/M）——pdfjs-dist legacy + postinstall 拷 worker/cmaps/standard_fonts（**cmaps 缺失 CJK PDF 渲染空白**）。（`lib/pdfjs.ts`）

## P2 数据与能力分发（地基，2–4 周）

1. **转录 wire 化 + 会话索引落库 + 标签**（L）——先核对 storage.rs 的 transcript JSONL 存的是 wire 格式还是内部事件（是内部事件则新会话起存 wire、旧的只读兼容）；会话索引表（agent+id 去重、folder、标题、软删）+ 标签两张小表 + LIKE 搜索接进 Ctrl K/全部对话；`(mtime,size)` 指纹缓存。正文永远留在原文件按需解析。（`import_service.rs`、`db/entities/`）
2. **MCP 市场 + 跨 agent 写回管理**（M/L）——扫描与 spawn 时借出已有（`context_inventory.rs`），补两块：① Market（官方 registry + Smithery 搜索 + 参数化安装表单）；② 把扫到的 spec 编辑后写回目标 agent 自己的配置文件（含传输兼容预检——Codex 不收 SSE），单文件坏只 warning 不炸列表。**多 agent 壳第一痛点**。（`commands/mcp.rs`）
3. **Skills 中央仓 + 文件级分发**（M/L）——扫描与 prompt 级借出已有（`context_inventory.rs`），补文件级：`~/.marionette/skills/` 中央仓 + junction 装进 agent 自己的技能目录 + 矩阵 UI（链接五态：未链接/链接到我们/链接到别处/被真实目录挡住/断链）；vendor superpowers 起步。**也是生成式 UI 的前提（P3.4）**。（`skill-packs-settings.tsx`、`commands/experts.rs`）
4. **Agent 生命周期补齐**（M）——pin 表（每次 bump 注释记 tarball diff）+ "Upgrade to unreviewed latest" 按钮 + 诊断 verdict（"为什么起不来"一句话：GUI PATH 缺口 / 装了官方 CLI 但壳要的是另一个适配器——Codeg 被报最多的 bug）。registry 安装入口已有，加的是版本纪律与诊断。（`latest_release.rs`、`compute_verdict`）
5. **dev server 发现 + 预览 tab**（M）——扫终端输出 banner（仅 loopback、仅整行）+ TCP 四次探活（0/250/750/1500ms）+ 工作区开预览（内嵌 webview 或系统浏览器）。（`browser/service_url.rs`）
6. **URL scheme**（S）——`marionette://session/<id>`；冷启动 pending slot + nudge；TG 桥推送带链接点开直达会话。（`deep_link.rs`）
7. **commit message ✨**（S/M）——共享引擎一次调用（比 Codeg 起一个 agent 进程轻得多）；草稿 hint + 常备指令 + nonce 防缓存 + 可选 auto-commit。（`commit_message/`）
8. **回合补时钟**（M）——对原生 store 无时间戳的 agent 壳层自记 JSONL（只存 prompt SHA 前 16 hex）；六层匹配，宁无钟不错钟。（`turn_timings.rs`）

## P3 协作（月级）

1. **awaiting_input 回流**（M，可提前到 P2 做）——订阅 permission/question/plan 请求事件，分屏非活跃侧/后台会话卡权限时 toast + 角标 + 原地回答。今天分屏用户就碰得到这个盲区。（`work_task` 引擎的事件订阅）
2. **@assistant 委托预设**（S/M，建在现有 @ 委托上，不依赖 LLM 自主委托）——存一条预设：名字/头像/描述 + 底座 agent + 该 agent 的选项（模型/档位）+ 常备指令；@ 菜单里直接选，发委托时子会话首 prompt 尾部附指令帧（Codeg 的 brief 帧 ≤256KB）。我们的变体：预设可指向**共享引擎模型**——"不 spawn agent CLI 的纯 LLM 角色"，Codeg 做不到这点。（`models/assistant.rs`、`delegation/assistants.rs`）
3. **Token 分析页**（L）——跨会话 usage 物化（token_usage_turn 整组替换 + 游标精确不等）+ 缓存命中环 + 热图（Rust 本地时区折叠）+ 牌价折算；额度数据源（共享引擎）已有。（`token-usage-page.tsx`）
4. **生成式 UI（json-render）**（M/L）——` ```spec ` 围栏 → 聊天内交互卡片/表格/图表；三道防护（原型污染/环检测/错误边界）+ 懒加载；SKILL.md 教 agent 写 spec（依赖 P2.3 文件级分发）。（`json-render-pre.tsx`）

## P4 远期/按需（有明确触发条件再做）

| 功能 | 触发条件 | 备注 |
|---|---|---|
| LLM 自主委托（codeg-mcp 式） | 现有**手动** @ 委托不够用：要 agent 的 LLM 自己并行 fan-out 时 | MCP 伴生进程三工具（schema 文案照抄：等待中不说话、blocked_on 才开口）+ broker + 进度流回；**前置逐 agent 验证 MCP 通道**（双注册/收下不启动/整表拒绝——§2.7 清单）；全路线图风险最高项，手动委托能覆盖前不动 |
| 收菜（worktree 无人值守任务） | awaiting_input 落地 + 用户要无人值守 | CAS + run_seq + fail-closed 删除照抄；merge 让 agent 解冲突 |
| Automations | 收菜落地后 | EnqueueTask 型几乎免费；composer 快照整体重放 |
| 浏览器共享 | 用户高频前端调试 | 先人→agent（picker/console/截图进会话，无需授权模型）；aria 引擎 vendored Playwright |
| Computer Use | 明确需求 | Windows 可砍 macOS helper 整套；按键白名单/全局停止键/悬顶指示条照抄 |
| codeg-cli 形态 | 脚本/CI 用户出现 | runtime-owned：CLI 挂了任务照跑 |
| forge（issue/PR 变任务） | 团队协作需求 | 只做 GitHub 也要：untrusted envelope + source_key 去重 + 手动/自动共用 create 路径 |
| Office 实时预览 | 办公文档用户 | 记住"预览器与编辑者共用一个 watch 进程"的 Windows 文件锁解法 |

## 明确不做（及理由）

- **桌面宠物**——用户拍板不要。
- **微信 iLink 通道**——用户拍板不做（2026-10-25）；远程轻交互由 Telegram 桥承担。Codeg 的 `ChatChannelBackend` 抽象与 TG topic 模式记录在 `research-codeg.md` D7 备查，将来要加 IM 通道时再翻。
- **Project Boot 可视化脚手架**——壳不是脚手架产品。
- **无限画布 / Monaco 编辑器 / 三栏 merge editor**——违背轻壳定位；我们分屏都刻意只做两格。
- **server / Docker / 手机端部署形态**——Marionette 是 Windows 本地壳；远程轻交互 TG 桥（已落）+ iLink（P3）已覆盖，完整 web 工作区不碰。SOCKS-over-WebSocket 隧道设计存档在 `research-codeg.md` D9 备查。
- **科研技能包内容**——机制（中央仓+矩阵，P2.3）要，内容不要。
- **主题 token 级改色 / 自定义 CSS / 壁纸市场**——无用户诉求，轮到再说；「壁纸原样字节存储保 GIF 动画」的做法记档即可。

## 依赖关系（先后的硬约束）

```
markdown 富渲染管线 (P1.4) ──→ 生成式 UI (P3.4)
技能文件级分发 (P2.3) ────────→ 生成式 UI (P3.4)
会话索引 (P2.1) ──────────────→ 标签/搜索增强
现有 @ 委托（已落） ──────────→ @assistant 预设 (P3.2)
awaiting_input (P3.1) ────────→ 收菜 (P4)
usage 落库 (P2 起步) ─────────→ Token 分析页 (P3.3)
逐 agent MCP 通道验证 ────────→ LLM 自主委托 (P4)【风险最高前置】
capabilityCache 能力探测 ────→ Steer (P1.3)
```

## 工程纪律（做任何一项之前重读）

- 新虚拟列表/滚动/流式组件：先过 `research-codeg.md` §2.5（底部抖动、跳转 vs 吸底锁、重渲染风暴、状态外置、转动元素无子元素）。
- 任何子进程输出读：lossy；任何字节切片：char boundary（中文 Windows + CJK 内容是我们的基本盘）。
- 适配器 pin bump：tarball diff + 真机 replay，注释记 wire 变化——release notes 不可信。
- 每接一个新 agent：过 §2.7 MCP 注入清单 + §2.6 适配器漂移清单 + §2.3 Windows 路径清单。
