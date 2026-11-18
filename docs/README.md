# docs — 文档索引

目录分区：**规范**进 `guides/`、**踩坑·案例·审计**进 `pitfalls/`、**调研**进 `research/`、
**设计·实施方案**进 `plans/`；过时的挪 `archive/`。根目录只留入口（本索引 + `CURRENT.md`
+ 指针 stub）。

## 入口（根目录）

| 位置 | 性质 |
|---|---|
| [`CURRENT.md`](./CURRENT.md) | **现行契约**：当前实现状态、产品底线、工程入口（含「跨用引擎」一节）。与代码冲突时以代码和本文件为准 |
| [`cross-agent-bridge.md`](./cross-agent-bridge.md) | 指针 stub：跨用桥已落地，现行行为在 `CURRENT.md`；完整设计稿在 [`archive/cross-agent-bridge.md`](./archive/cross-agent-bridge.md)。**代码注释里的章节锚点（§4.x）引用本文件，别挪** |
| `logo.png` / `screenshots/` | 根 README 用的 logo 与界面截图 |

## guides/ — 规范（改界面前先读）

| 位置 | 性质 |
|---|---|
| [guides/diagnostics.md](./guides/diagnostics.md) | **诊断报告**：用户如何在 Ctrl K 导出脱敏日志、报告字段与隐私边界、dev.log 轮转，以及自动化可读取的 manifest 契约 |
| [`guides/ui-language.md`](./guides/ui-language.md) | **视觉语言**：对话流就是设计系统（3px 卡、You 卡左边条、composer 芯片）。改任何用户可见界面前先读。技能 `.grok/skills/marionette-ui` |
| [`guides/i18n-guide.md`](./guides/i18n-guide.md) | **i18n 规范**：中英双语的词典/`ts!` 用法、术语表、探测串双语并集红线与验收命令（2026-10-30 落地） |
| [`guides/model-library.md`](./guides/model-library.md) | **模型与来源（现行行为）**：模型共享浮层「模型与来源 · 显示和排序」——按来源/账号区分模型、直连/原生/引擎入口的合并规则（同名不作去重依据） |

## pitfalls/ — 踩坑·案例·审计

| 位置 | 性质 |
|---|---|
| [`pitfalls/pitfalls-zcode-acp.md`](./pitfalls/pitfalls-zcode-acp.md) | **踩坑集**：ZCode ACP 接入与跨用桥的九个实测雷区（EFTYPE/stdio MCP 毒化/个人 provider 层 zod 整层拒绝等），含证据链与快速确诊表 |
| [`pitfalls/pitfalls-model-share-quota.md`](./pitfalls/pitfalls-model-share-quota.md) | **踩坑集**：模型共享引擎与额度面板（2026-10-23）——opencode 凭据迁库 auth.json→opencode.db、双钥匙抢槽、Go 额度端点规矩、ZCode Coding Plan 钥匙藏位（provider_config.json 个人层）、pi-ai catalog 限制边界勘误，含快速确诊表 |
| [`pitfalls/close-freeze-deadlock.md`](./pitfalls/close-freeze-deadlock.md) | 案例记录：关窗死锁的成因、日志证据链，以及后续 kill 路径必须遵守的规则 |
| [`pitfalls/code-audit-2026-10-24.md`](./pitfalls/code-audit-2026-10-24.md) | **全库代码摸底（2026-10-24）**：10 个只读子代理 + 主代理逐条抽查的全量体检——1 个 P0（Telegram token 进 dev.log）、45 个 P1 按主题分组（前端热路径 / Rust 假死面 / 安全边界 / 存储耐久 / 工程卫生）、P2 摘要、巨型文件拆分方案与处理顺序；条目修复后就地打 ✅ |

## research/ — 调研

| 位置 | 性质 |
|---|---|
| [`research/research-dsh-cross-use.md`](./research/research-dsh-cross-use.md) | **调研（2026-10-20）**：DSH 跨用模型怎么接（已落地 `dsh_bridge.rs`，现行行为见 `CURRENT.md`） |
| [`research/research-acp-cross-use-matrix.md`](./research/research-acp-cross-use-matrix.md) | **调研（2026-10-20）**：其余 ACP agent 的跨用桥可接性矩阵（只调研不改代码） |
| [`research/research-agent-memory-locations.md`](./research/research-agent-memory-locations.md) | **调研（2026-10-23）**：各家 coding agent 的记忆存储位置全网调查——`memory.rs` 跨 agent 记忆导入的依据 |
| [`research/research-content-filter.md`](./research/research-content-filter.md) | 归因报告：content_filter 不是 marionette 中转引入的（抓包逐字段对照 + 直连反例 + 时间线） |
| [`research/research-edge-dock.md`](./research/research-edge-dock.md) | **调研（2026-10-23）**：QQ 式贴边停靠的技术选型——sliver 触发条 vs 钩子/轮询/AppBar 路线对比、现有代码盘点（余额/会话恢复/升级/拖放皆现成）、memo 收纳盒两案，含外部参考全集 |
| [`research/research-codeg.md`](./research/research-codeg.md) | **调研（2026-10-25）**：同类产品 Codeg 全量分析（克隆在 `D:\Projects\codeg`）——可抄功能（活动胶囊/steer 队列/富内容渲染/MCP·Skills 分发/收菜任务/@委托/微信 iLink 通道…）、坑清单（进程管理/编码/Windows/流式 UI/适配器漂移…带 commit 证据）、ZCode 同桥情报（`ZCODE_ACP_RUNTIME` 等）、行动优先级 |
| [`research/research-browse-ux.md`](./research/research-browse-ux.md) | **调研（2026-11-17）**：anomalyco/browse 用户体验对标——后台 Agent 标签、ON/RUN/WAIT、页内 handoff、标签归属/清理、失败 nextStep 与 Marionette 元素批注/细粒度授权/Composer 一体化的差异；含 P0–P2 体验路线和不照搬清单 |
| [`research/research-council-convergence-hallucination.md`](./research/research-council-convergence-hallucination.md) | **调研（2026-11-02）**：圆桌的收敛与幻觉治理——现状审计（辩论确实已实现、star 拓扑、已有防护清单）+ 两个断点（`Rebuttal` 死通道、`PlanStep` 无来源字段）+ 文献结论（轮次收益止于 2 轮、收敛到一致≠正确、共识放大幻觉、LLM 引用捏造率 78–90%） |
| [`research/research-context-transparency.md`](./research/research-context-transparency.md) | **调研（2026-11-06）**：上下文透明化——dsh-context 级洞察的通用化路径。四层采集架构（ACP 在线/ingest 盖戳/原生日志提取器/密度估算）、per-agent 数据源矩阵（grok 本机库最富：系统提示词+工具定义+官方成本 ticks 全落盘；claude/codex/dsh/opencode 各有四桶用量；本机实证）、ACP usage_update 已 stable、现状缺口与 P0–P3 分阶段落地 |
| [`research/research-bigfatfishwife.md`](./research/research-bigfatfishwife.md) | **调研（2026-11-17 二轮全量）**：BigFatFishWife / dsh-room 的 README/产品结构、数据模型、任务执行、审批边界、通知、分层记忆、UI 与运维机制拆解——含精确参数、源码证据、适配映射、当前员工实现对账摘要、验证结果和明确不借清单 |
| [`research/telegram-chief/README.md`](./research/telegram-chief/README.md) | **调研 / 方案（2026-11-15）**：Grok Bot 式 Telegram 远程首席助理——固定 Chief 会话、自然语言调度项目 / Agent / Provider、Host 控制 MCP、worker 任务账本、审批与恢复；含竞品对比、安全边界与 M0–M3 施工计划 |
| [`research/ego-lite/README.md`](./research/ego-lite/README.md) | **调研 + 移植方案（2026-11-17）**：ego-lite v2.2.1 的源码/许可证边界、TaskSpace/Page/ref/wait/error 机制、Marionette master 差距、文件级复用地图与 M0–M5 路线；扩展目录还覆盖动作可见化、hard stop、Page 生命周期、popup/download/file、运行时卫生、站点经验与 benchmark。结论：闭源内核不可搬；优先借稳定 ref、typed batch、wait/dialog、错误语义和测试资产 |

## plans/ — 设计·实施方案

| 位置 | 性质 |
|---|---|
| [plans/plan-github-maintenance.md](./plans/plan-github-maintenance.md) | **路线图（草案，2026-11-18）**：GitHub Issue / PR 自动 triage、隔离复现、draft PR 修复和 PR 维护循环；明确只读起步、外部文本数据化、不自动 merge/release。 |
| [`plans/plan-edge-dock.md`](./plans/plan-edge-dock.md) | 贴边停靠设计稿（阶段 0–3 已落地，现行行为以 `CURRENT.md`「贴边停靠」与 `AGENTS.md` 红线为准；文内记了与实现的偏差）。**悬浮球计划已废弃并入本计划**（原件 [`archive/plan-orb-assistant.md`](./archive/plan-orb-assistant.md)） |
| [`plans/plan-codeg-borrow.md`](./plans/plan-codeg-borrow.md) | **路线图（2026-10-25）**：从 Codeg 借什么、什么不借——对照现状（代理层/跨 agent handoff/12 parser/registry 安装/Ctrl K 搜索已对齐）分 P0 速赢→P1 对话流骨架→P2 数据与能力分发→P3 协作与远程→P4 按需，含明确不做清单、依赖关系与工程纪律 |
| [`plans/plan-browser-computer-use.md`](./plans/plan-browser-computer-use.md) | **实施案（2026-10-31 定稿）**：浏览器共享 + Computer Use 两批全量方案——child webview（wry resize-subclass 坑）、vendor aria 引擎、localhost HTTP MCP（每会话 token，zcode 通吃）、授权与对话权限模式吻合（agent 自开页自动读/Control 走权限卡）、CU 的 cua-driver 按需下载 + helper 单二进制 + 按键白名单。批次 1（文档标签页）已落地，行为见 `CURRENT.md` |
| [`plans/plan-browser-workspace.md`](./plans/plan-browser-workspace.md) | **实施方案（2026-11-17，Phase 1–4 已落地）**：Codeg 对齐的 `SurfaceWorkspace`——Agent 对话 + Browser/Document/Memo content surface。复用 renderConversation/Composer 与 child WebView2 bounds，不改 session-only SplitWorkspace；含后台 tab、dev server probe、handoff、共同标签、会话切绑和冷启动恢复 |
| [`plans/plan-browser-scope.md`](./plans/plan-browser-scope.md) | **产品边界（2026-11-17）**：Marionette 只做内置 WebView2 开发工作区；不做 Chrome/Edge 扩展、relay、外部 MCP 或个人浏览器连接器。用户需要这类能力时自行使用 browse |
| [`plans/plan-council-convergence-guardrails.md`](./plans/plan-council-convergence-guardrails.md) | **实施方案（草案）**：P0 接线 Rebuttal / 综合三段式+强制溯源 / 收益衰减提前终止（Jaccard 纯字符串）→ P1 确定性符号核验 / isolation guard / 置信度加权 → P2 构造反命题 / ABSTAIN；含明确不做与分批验收口径 |
| [`plans/plan-plugin-layer.md`](./plans/plan-plugin-layer.md) | **设计稿（2026-11-05 落地）**：共通插件层——安装一次全 agent 生效。业界对标（codeg 扇出写回 1 万行教训/Zed 直传缺口/mcpd·MCPHub 同型验证/ACP v2 条款）、url 型代理架构（stdio 子进程托管 + http 中继凭据不出主进程）、红线与落地偏差记录 |
| [`plans/plan-employee-mode.md`](./plans/plan-employee-mode.md) | **实现对账 + 设计稿（2026-11-17 工作树）**：员工模式已施工 E1 主体，E2/E3 部分、E4 待办；逐项记录员工注册表、侧栏 roster、状态投影、任务/worker、记忆/承诺/通知/Telegram 的真实行为，含注册表级全局暂停、Proposed 任务预留竞态修复、只读硬门禁、Computer Use 项目策略、持久通知预算、自动 task journal/proposal accept-reject、稳定 staffId 路由和单 chat Telegram 限制 |
| [`plans/plan-engine-contract.md`](./plans/plan-engine-contract.md) | **引擎合同（2026-11-17 M1 落地）**：Marionette ↔ magpie 种子的合同清单——合同面、红线与验证套件 `scripts/verify-engine-contract.mts`；`scripts/upstream-watch.mts` 按它盯上游 magpie |
| [`plans/split-workspace.md`](./plans/split-workspace.md) | 早期分屏设计稿（已落地，现行行为以 `CURRENT.md`「多窗口」为准） |
| [`plans/telegram-bridge.md`](./plans/telegram-bridge.md) | Telegram 远程桥——MVP 已实现（通知/回复/远程批准/配对）；设计、调研与实现差异记录 |

## 专题目录

| 位置 | 性质 |
|---|---|
| [`council/`](./council/architecture.md) | 圆桌（Council）专题：architecture（现行架构）、architecture-audit、formation、implementation-plan、testing、user-guide |
| [`design/`](./design/ui-v2) | 设计画布：edge-dock.html + ui-v2 各页 HTML 画布（画布引用见 `guides/ui-language.md`） |
| [`releases/`](./releases/v0.8.0.md) | 发布说明（v0.7.0 起） |
| `archive/` | 历史归档（2026-07 前后的策划、路线图、规格与调研）。**不是**待办清单 |

新增文档时的约定：现行行为写进 `CURRENT.md` 或新的专题文件；规范进 `guides/`、
踩坑进 `pitfalls/`、调研进 `research/`、设计·实施方案进 `plans/`；一旦过时就挪进
`archive/`，不要让同一主题同时存在两套互相矛盾的说明。
