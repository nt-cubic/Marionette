# scripts — 代码验证脚本

改完代码后的自检脚本，都在仓库根目录下运行。Node 24 自带 type stripping，
`.mts` 可以直接 `node` 跑；用 `npx tsx` 也可以。

| 脚本 | 跑法 | 检查什么 |
|---|---|---|
| `caps-smoke.mts` | `node scripts/caps-smoke.mts` | Composer 的能力合并：live `session/new` 数据优先、兜底表只在 live 为空时生效、档位标签不重复轴名、未知 agent 直通 |
| `smoke-stream-merge.mts` | `npx tsx scripts/smoke-stream-merge.mts` | 流式合并：Grok Reply 逐 token、Codex Thought 不丢标点、多会话交错不串台；notice/retry 不进聊天 |
| `usage-smoke.mts` | `npx tsx scripts/usage-smoke.mts` | usage 面板解析，断言用的是真实 adapter 抓到的报文 |
| `title-smoke.mts` | `npx tsx scripts/title-smoke.mts` | 会话标题：拒绝 Marionette 自己注入的 prompt 抬头被当成标题，修复时从转录里取用户真正打的第一句 |
| `suspend-smoke.mts` | `npx tsx scripts/suspend-smoke.mts` | 挂起规则两套：手动 ⏸ 按钮对**任何还持有进程**的会话都可按（starting / running / waiting / error，running 时按下去直接中断），只有已挂起的不画按钮；而 30 分钟空闲自动挂起仍只碰「热但空闲且没有待办」的会话 |
| `fork-smoke.mts` | `npx tsx scripts/fork-smoke.mts` | 分叉的切点与拷贝：按 message id（退化为文本）定位那条回复，子任务/交接卡片不带过去，事件原对象不被改 |
| `image-path-smoke.mts` | `npx tsx scripts/image-path-smoke.mts` | 对话里图片的路径：相对路径按会话工作目录拼成绝对路径，`file://` 去协议，绝对路径原样（用的是真实转录里的路径） |
| `verify-usage-row-layout.mts` | `npx tsx scripts/verify-usage-row-layout.mts` | usage 行布局：不再渲染 Last turn / Session total |
| `verify-text-pipeline.mjs` | `npm run verify:text` | 文本 / 展示管线的冒烟检查（只依赖 `markdownText.ts`，不需要整棵 import 图） |
| `check-ui.mjs` | `node scripts/check-ui.mjs` | 通过 CDP 检查运行中的界面；需要先有一个开着 `--remote-debugging-port=9222` 的窗口 |

Rust 侧的自检在 `src-tauri\`：`cargo test --bin marionette`（这个 crate 是 bin-only，`--lib` 不适用）。
