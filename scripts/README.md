# scripts — 代码验证脚本

改完代码后的自检脚本，都在仓库根目录下运行。Node 24 自带 type stripping，
`.mts` 可以直接 `node` 跑；用 `npx tsx` 也可以。

| 脚本 | 跑法 | 检查什么 |
|---|---|---|
| `caps-smoke.mts` | `node scripts/caps-smoke.mts` | Composer 的能力合并：live `session/new` 数据优先、兜底表只在 live 为空时生效、档位标签不重复轴名、未知 agent 直通 |
| `smoke-stream-merge.mts` | `npx tsx scripts/smoke-stream-merge.mts` | 流式合并：Grok Reply 逐 token、Codex Thought 不丢标点、多会话交错不串台 |
| `usage-smoke.mts` | `npx tsx scripts/usage-smoke.mts` | usage 面板解析，断言用的是真实 adapter 抓到的报文 |
| `verify-usage-row-layout.mts` | `npx tsx scripts/verify-usage-row-layout.mts` | usage 行布局：不再渲染 Last turn / Session total |
| `verify-text-pipeline.mjs` | `npm run verify:text` | 文本 / 展示管线的冒烟检查（只依赖 `markdownText.ts`，不需要整棵 import 图） |
| `check-ui.mjs` | `node scripts/check-ui.mjs` | 通过 CDP 检查运行中的界面；需要先有一个开着 `--remote-debugging-port=9222` 的窗口 |

Rust 侧的自检在 `src-tauri\`：`cargo test --bin marionette`（这个 crate 是 bin-only，`--lib` 不适用）。
