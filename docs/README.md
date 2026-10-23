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
| [`research-edge-dock.md`](./research-edge-dock.md) | **调研（2026-10-23）**：QQ 式贴边停靠的技术选型——sliver 触发条 vs 钩子/轮询/AppBar 路线对比、现有代码盘点（余额/会话恢复/升级/拖放皆现成）、memo 收纳盒两案，含外部参考全集 |
| [`plan-edge-dock.md`](./plan-edge-dock.md) | **设计稿（未实现）**：贴边停靠长条——主窗停靠模式三态状态机、4px 触发条唤出、关闭即收起后台常驻、窄条 UI（新建对话/收纳盒/工程列表/余额 chip/footer 升级退出）、memo 收纳盒，五阶段实施计划。**悬浮球计划已废弃并入本计划**（地基/`capture_save_blob`/余额数据层/收集箱方案；原件 [`archive/plan-orb-assistant.md`](./archive/plan-orb-assistant.md)） |
| `archive/` | 历史归档（2026-07 前后的策划、路线图、规格与调研）。**不是**待办清单 |
| `screenshots/` | 根 README 用的界面截图 |

新增文档时的约定：现行行为写进 `CURRENT.md` 或新的专题文件；一旦过时就挪进 `archive/`，
不要让 `docs/` 根目录同时存在两套互相矛盾的说明。
