# docs — 文档索引

| 位置 | 性质 |
|---|---|
| [`CURRENT.md`](./CURRENT.md) | **现行契约**：当前实现状态、产品底线、工程入口。与代码冲突时以代码和本文件为准 |
| [`close-freeze-deadlock.md`](./close-freeze-deadlock.md) | 案例记录：关窗死锁的成因、日志证据链，以及后续 kill 路径必须遵守的规则 |
| `archive/` | 历史归档（2026-07 前后的策划、路线图、规格与调研）。**不是**待办清单 |
| `screenshots/` | 根 README 用的界面截图 |

新增文档时的约定：现行行为写进 `CURRENT.md` 或新的专题文件；一旦过时就挪进 `archive/`，
不要让 `docs/` 根目录同时存在两套互相矛盾的说明。
