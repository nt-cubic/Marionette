# docs — 文档索引

| 位置 | 性质 |
|---|---|
| [`CURRENT.md`](./CURRENT.md) | **现行契约**：当前实现状态、产品底线、工程入口（含「跨用引擎」一节）。与代码冲突时以代码和本文件为准 |
| [`cross-agent-bridge.md`](./cross-agent-bridge.md) | 指针：跨用桥已落地，现行行为在 `CURRENT.md`；完整设计稿在 [`archive/cross-agent-bridge.md`](./archive/cross-agent-bridge.md) |
| [`close-freeze-deadlock.md`](./close-freeze-deadlock.md) | 案例记录：关窗死锁的成因、日志证据链，以及后续 kill 路径必须遵守的规则 |
| [`pitfalls-zcode-acp.md`](./pitfalls-zcode-acp.md) | **踩坑集**：ZCode ACP 接入与跨用桥的九个实测雷区（EFTYPE/stdio MCP 毒化/个人 provider 层 zod 整层拒绝等），含证据链与快速确诊表 |
| [`pitfalls-model-share-quota.md`](./pitfalls-model-share-quota.md) | **踩坑集**：模型共享引擎与额度面板（2026-10-23）——opencode 凭据迁库 auth.json→opencode.db、双钥匙抢槽、Go 额度端点规矩、ZCode Coding Plan 钥匙藏位（provider_config.json 个人层）、pi-ai catalog 限制边界勘误，含快速确诊表 |
| [`telegram-bridge.md`](./telegram-bridge.md) | Telegram 远程桥——MVP 已实现（通知/回复/远程批准/配对）；设计、调研与实现差异记录 |
| [`split-workspace.md`](./split-workspace.md) | **设计稿（未实现）**：对话区内分屏/停靠——拖标签预览、左右（后续四格）、焦点格、与撕窗共存、渲染预算 |
| [`research-content-filter.md`](./research-content-filter.md) | 归因报告：content_filter 不是 marionette 中转引入的（抓包逐字段对照 + 直连反例 + 时间线） |
| [`plan-orb-assistant.md`](./plan-orb-assistant.md) | **设计稿（未实现）**：桌面悬浮球后台常驻助手——关闭即后台、收集箱（速记流+AI 知识库）、场景按钮、余额列表、任务提醒，五档实施计划 |
| `archive/` | 历史归档（2026-07 前后的策划、路线图、规格与调研）。**不是**待办清单 |
| `screenshots/` | 根 README 用的界面截图 |

新增文档时的约定：现行行为写进 `CURRENT.md` 或新的专题文件；一旦过时就挪进 `archive/`，
不要让 `docs/` 根目录同时存在两套互相矛盾的说明。
