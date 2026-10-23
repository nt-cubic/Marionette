# scripts — 代码验证脚本

改完代码后的自检脚本，都在仓库根目录下运行。Node 24 自带 type stripping，
`.mts` 可以直接 `node` 跑；用 `npx tsx` 也可以。

| 脚本 | 跑法 | 检查什么 |
|---|---|---|
| `caps-smoke.mts` | `node scripts/caps-smoke.mts` | Composer 的能力合并：live `session/new` 数据优先、兜底表只在 live 为空时生效、档位标签不重复轴名、未知 agent 直通 |
| `smoke-stream-merge.mts` | `npx tsx scripts/smoke-stream-merge.mts` | 流式合并：Grok Reply 逐 token、Codex Thought 不丢标点、多会话交错不串台；notice/retry 不进聊天；OpenCode v2 子代理流进 Task 卡不进 Reply；OpenCode write 从 `rawInput.content` 合成 diff 并在 success note 后保留；逐字数字 `1000` 与重复字不丢；Grok `spawn_subagent` + `_x.ai/session/update` 收进并行子代理卡 |
| `usage-smoke.mts` | `npx tsx scripts/usage-smoke.mts` | usage 面板解析，断言用的是真实 adapter 抓到的报文 |
| `title-smoke.mts` | `npx tsx scripts/title-smoke.mts` | 会话标题：拒绝 Marionette 自己注入的 prompt 抬头被当成标题，修复时从转录里取用户真正打的第一句 |
| `suspend-smoke.mts` | `npx tsx scripts/suspend-smoke.mts` | 挂起规则两套：手动 ⏸ 按钮对**任何还持有进程**的会话都可按（starting / running / waiting / error，running 时按下去直接中断），只有已挂起的不画按钮；而 30 分钟空闲自动挂起仍只碰「热但空闲且没有待办」的会话 |
| `fork-smoke.mts` | `npx tsx scripts/fork-smoke.mts` | 分叉的切点与拷贝：按 message id（退化为文本）定位那条回复，子任务/交接卡片不带过去，事件原对象不被改 |
| `image-path-smoke.mts` | `npx tsx scripts/image-path-smoke.mts` | 对话里图片的路径：相对路径按会话工作目录拼成绝对路径，`file://` 去协议，绝对路径原样（用的是真实转录里的路径） |
| `verify-usage-row-layout.mts` | `npx tsx scripts/verify-usage-row-layout.mts` | usage 行布局：不再渲染 Last turn / Session total |
| `verify-text-pipeline.mjs` | `npm run verify:text` | 文本 / 展示管线的冒烟检查（只依赖 `markdownText.ts`，不需要整棵 import 图） |
| `verify-activity-cluster.mts` | `npx tsx scripts/verify-activity-cluster.mts` | 活动折叠：thought+tool 同一集群、同类连续分组、结束后摘要为 `1 think, 2 read, 3 edit`、live 时跟当前工具 |
| `verify-transcript-scroll.mts` | `npx tsx scripts/verify-transcript-scroll.mts` | 对话 tab 滚动：贴底/历史位置快照、切回时恢复、虚拟列表行高缓存跨 tab 还在 |
| `verify-session-order.mts` | `npx tsx scripts/verify-session-order.mts` | 侧栏折叠预览：置顶行和「刚完成、还没点进去」的对话不会掉进 Show more |
| `verify-agent-errors.mts` | `npx tsx scripts/verify-agent-errors.mts` | 错误分类：Grok `auth_transient` 不当成需要登录、OpenCode 按当前模型渠道给提示、426 仍走更新 |
| `verify-split-workspace.mts` | `npx tsx scripts/verify-split-workspace.mts` | 分屏树：拆/并/关标签、上下拆、1\|1\|1 均分、1\|2 / 2×2、第 5 叶降级、命中测试、撕窗领养进格、屏幕坐标换算 |
| `verify-ui-restore.mts` | `npx tsx scripts/verify-ui-restore.mts` | 冷启动恢复：旧扁平快照迁成单格、两格树还原、扣掉仍在撕窗的会话、撕窗 URL 忽略树 |
| `verify-acp-render-budget.mts` | `npx tsx scripts/verify-acp-render-budget.mts` | 渲染预算：paintSet 含 @-delegate 子会话、隐藏标签 thought 走 shadow、tool 不 defer、activate 时 flush |
| `verify-permission-plan.mts` | `npx tsx scripts/verify-permission-plan.mts` | Claude Code ExitPlanMode：从权限请求的 `{plan}` JSON 里拆出 markdown，普通改文件权限不误判 |
| `verify-memos.mts` | `npx tsx scripts/verify-memos.mts` | Memo 时间线：拆掉收集箱整理提示词、#标签、按日分组 |
| `verify-interaction-stream.mts` | `npx tsx scripts/verify-interaction-stream.mts` | Plan/Ask 进对话流：独立卡片、transcript 往返、composer 按会话收起 |
| `check-ui.mjs` | `node scripts/check-ui.mjs` | 通过 CDP 检查运行中的界面；需要先有一个开着 `--remote-debugging-port=9222` 的窗口 |

Rust 侧的自检在 `src-tauri\`：`cargo test --bin marionette`（这个 crate 是 bin-only，`--lib` 不适用）。
