# docs — 文档索引

| 位置 | 性质 |
|---|---|
| [`CURRENT.md`](./CURRENT.md) | **现行契约**：当前实现状态、产品底线、工程入口（含「跨用引擎」一节）。与代码冲突时以代码和本文件为准 |
| [`i18n-guide.md`](./i18n-guide.md) | **i18n 规范**：中英双语的词典/`ts!` 用法、术语表、探测串双语并集红线与验收命令（2026-10-30 落地） |
| [`cross-agent-bridge.md`](./cross-agent-bridge.md) | 指针：跨用桥已落地，现行行为在 `CURRENT.md`；完整设计稿在 [`archive/cross-agent-bridge.md`](./archive/cross-agent-bridge.md) |
| [`close-freeze-deadlock.md`](./close-freeze-deadlock.md) | 案例记录：关窗死锁的成因、日志证据链，以及后续 kill 路径必须遵守的规则 |
| [`pitfalls-zcode-acp.md`](./pitfalls-zcode-acp.md) | **踩坑集**：ZCode ACP 接入与跨用桥的九个实测雷区（EFTYPE/stdio MCP 毒化/个人 provider 层 zod 整层拒绝等），含证据链与快速确诊表 |
| [`pitfalls-model-share-quota.md`](./pitfalls-model-share-quota.md) | **踩坑集**：模型共享引擎与额度面板（2026-10-23）——opencode 凭据迁库 auth.json→opencode.db、双钥匙抢槽、Go 额度端点规矩、ZCode Coding Plan 钥匙藏位（provider_config.json 个人层）、pi-ai catalog 限制边界勘误，含快速确诊表 |
| [`code-audit-2026-10-24.md`](./code-audit-2026-10-24.md) | **全库代码摸底（2026-10-24）**：10 个只读子代理 + 主代理逐条抽查的全量体检——1 个 P0（Telegram token 进 dev.log）、45 个 P1 按主题分组（前端热路径 / Rust 假死面 / 安全边界 / 存储耐久 / 工程卫生）、P2 摘要、巨型文件拆分方案与处理顺序；条目修复后就地打 ✅ |
| [`telegram-bridge.md`](./telegram-bridge.md) | Telegram 远程桥——MVP 已实现（通知/回复/远程批准/配对）；设计、调研与实现差异记录 |
| [`split-workspace.md`](./split-workspace.md) | 早期分屏设计稿（已落地，现行行为以 `CURRENT.md`「多窗口」为准） |
| [`research-content-filter.md`](./research-content-filter.md) | 归因报告：content_filter 不是 marionette 中转引入的（抓包逐字段对照 + 直连反例 + 时间线） |
| [`research-edge-dock.md`](./research-edge-dock.md) | **调研（2026-10-23）**：QQ 式贴边停靠的技术选型——sliver 触发条 vs 钩子/轮询/AppBar 路线对比、现有代码盘点（余额/会话恢复/升级/拖放皆现成）、memo 收纳盒两案，含外部参考全集 |
| [`plan-edge-dock.md`](./plan-edge-dock.md) | 贴边停靠设计稿（阶段 0–3 已落地，现行行为以 `CURRENT.md`「贴边停靠」与 `AGENTS.md` 红线为准；文内记了与实现的偏差）。**悬浮球计划已废弃并入本计划**（原件 [`archive/plan-orb-assistant.md`](./archive/plan-orb-assistant.md)） |
| [`research-codeg.md`](./research-codeg.md) | **调研（2026-10-25）**：同类产品 Codeg 全量分析（克隆在 `D:\Projects\codeg`）——可抄功能（活动胶囊/steer 队列/富内容渲染/MCP·Skills 分发/收菜任务/@委托/微信 iLink 通道…）、坑清单（进程管理/编码/Windows/流式 UI/适配器漂移…带 commit 证据）、ZCode 同桥情报（`ZCODE_ACP_RUNTIME` 等）、行动优先级 |
| [`plan-codeg-borrow.md`](./plan-codeg-borrow.md) | **路线图（2026-10-25）**：从 Codeg 借什么、什么不借——对照现状（代理层/跨 agent handoff/12 parser/registry 安装/Ctrl K 搜索已对齐）分 P0 速赢→P1 对话流骨架→P2 数据与能力分发→P3 协作与远程→P4 按需，含明确不做清单、依赖关系与工程纪律 |
| `archive/` | 历史归档（2026-07 前后的策划、路线图、规格与调研）。**不是**待办清单 |
| `screenshots/` | 根 README 用的界面截图 |

新增文档时的约定：现行行为写进 `CURRENT.md` 或新的专题文件；一旦过时就挪进 `archive/`，
不要让 `docs/` 根目录同时存在两套互相矛盾的说明。
