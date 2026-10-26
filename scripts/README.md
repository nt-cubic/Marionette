# scripts — 代码验证脚本

改完代码后的自检脚本，都在仓库根目录下运行。Node 24 自带 type stripping，
`.mts` 可以直接 `node` 跑；用 `npx tsx` 也可以。

**聚合入口：`npm run verify`**（`scripts/verify-all.mjs`）跑全部离线脚本
（`.mts` 自动经 `npx tsx` 起动，`.mjs` 直接 node）；需要活体环境的
`check-ui.mjs`（CDP 窗口）、`verify-model-source-engine.mjs`（引擎二进制）、
`patch-zcode-acp-personal-models.mjs`（改全局安装）不在此列。

| 脚本 | 跑法 | 检查什么 |
|---|---|---|
| `verify-model-library.mjs` | `node scripts/verify-model-library.mjs` | 原生/共享模型按真实来源合并、不同账号同名模型保留、provider 与模型隐藏/排序/搜索、凭据扫描返回格式兼容 |
| `verify-model-source-engine.mjs` | `node scripts/verify-model-source-engine.mjs [magpie.exe]` | 临时隔离目录与本机假接口，验证已安装引擎的自定义 provider 添加、带 tag 的模型、换钥匙、模型更新、删除；仅用假 key |
| `caps-smoke.mts` | `node scripts/caps-smoke.mts` | Composer 的能力合并：live `session/new` 数据优先、兜底表只在 live 为空时生效、档位标签不重复轴名、未知 agent 直通 |
| `smoke-stream-merge.mts` | `npx tsx scripts/smoke-stream-merge.mts` | 流式合并：Grok Reply 逐 token、Codex Thought 不丢标点、多会话交错不串台；notice/retry 不进聊天；OpenCode v2 子代理流进 Task 卡不进 Reply；OpenCode write 从 `rawInput.content` 合成 diff 并在 success note 后保留；逐字数字 `1000` 与重复字不丢；Grok `spawn_subagent` + `_x.ai/session/update` 收进并行子代理卡 |
| `verify-memo-box.mts` | `npx tsx scripts/verify-memo-box.mts` | 收纳盒：origin `memo` 的 Chat 会话拆进侧栏「收纳盒」栏目、10-22 方案的 `~/.marionette/inbox` 幽灵项目在加载口被过滤（正/反斜杠、尾斜杠）、memoPrompt dialog/doc 预填契约、文字拖放通道（页内拖拽源 tracker 让路 + dock 区文字 DOM 事件）、单列帖子流（`groupMemosByDay` / 顶上写一条 / 四种卡，不再是 40px 文件行）、图片卡按横竖比 contain 完整显示 |
| `verify-agent-reentry.mts` | `npx tsx scripts/verify-agent-reentry.mts` | 再次进入对话（post-turn 续跑）：waiting+优雅期才武装、迟到回声/运行中/无回合边界不武装；安静收尾只封本会话尾卡（Reply→durationMs、Thought→endedAt），已封卡与他窗事件不动；zcode 真实形状回放不被粘进前一张卡 |
| `usage-smoke.mts` | `npx tsx scripts/usage-smoke.mts` | usage 面板解析，断言用的是真实 adapter 抓到的报文 |
| `title-smoke.mts` | `npx tsx scripts/title-smoke.mts` | 会话标题：拒绝 Marionette 自己注入的 prompt 抬头被当成标题，修复时从转录里取用户真正打的第一句 |
| `suspend-smoke.mts` | `npx tsx scripts/suspend-smoke.mts` | 挂起规则两套：手动 ⏸ 按钮对**任何还持有进程**的会话都可按（starting / running / waiting / error，running 时按下去直接中断），只有已挂起的不画按钮；而 30 分钟空闲自动挂起仍只碰「热但空闲且没有待办」的会话 |
| `fork-smoke.mts` | `npx tsx scripts/fork-smoke.mts` | 分叉的切点与拷贝：按 message id（退化为文本）定位那条回复，子任务/交接卡片不带过去，事件原对象不被改 |
| `image-path-smoke.mts` | `npx tsx scripts/image-path-smoke.mts` | 对话里图片的路径：相对路径按会话工作目录拼成绝对路径，`file://` 去协议，绝对路径原样（用的是真实转录里的路径） |
| `verify-usage-row-layout.mts` | `npx tsx scripts/verify-usage-row-layout.mts` | usage 行布局：不再渲染 Last turn / Session total |
| `verify-text-pipeline.mjs` | `npm run verify:text` | 文本 / 展示管线的冒烟检查（只依赖 `markdownText.ts`，不需要整棵 import 图） |
| `verify-activity-cluster.mts` | `npx tsx scripts/verify-activity-cluster.mts` | 活动折叠：thought+tool 同一集群、同类连续分组、结束后摘要为 `1 think, 2 read, 3 edit`、live 时跟当前工具 |
| `verify-turn-delivery.mts` | `npx tsx scripts/verify-turn-delivery.mts` | 交付卡折叠文案「N 个文件已更改」、git truth 与 tool edit 并集、git numstat 覆盖工具统计、**本轮归属规则**（起点就脏的文件需本轮工具碰过、无 HEAD 不丢文件、路径归一） |
| `verify-transcript-scroll.mts` | `npx tsx scripts/verify-transcript-scroll.mts` | 对话 tab 滚动：贴底/历史位置快照、切回时恢复、虚拟列表行高缓存跨 tab 还在 |
| `verify-session-order.mts` | `npx tsx scripts/verify-session-order.mts` | 侧栏折叠预览：置顶行和「刚完成、还没点进去」的对话不会掉进 Show more |
| `verify-agent-errors.mts` | `npx tsx scripts/verify-agent-errors.mts` | 错误分类：Grok `auth_transient` 不当成需要登录、OpenCode 按当前模型渠道给提示、426 仍走更新 |
| `verify-background-tasks.mts` | `npx tsx scripts/verify-background-tasks.mts` | 后台任务面板聚合：需要你置顶、跑动中次之、已结束沉底；完成/失败委托不进列表、结束终端保留退出码；key 跨重建稳定；空输入不渲染 |
| `verify-split-workspace.mts` | `npx tsx scripts/verify-split-workspace.mts` | 分屏树：拆/并/关标签、上下拆、1\|1\|1 均分、1\|2 / 2×2、第 5 叶降级、命中测试、撕窗领养进格、屏幕坐标换算 |
| `verify-ui-restore.mts` | `npx tsx scripts/verify-ui-restore.mts` | 冷启动恢复：旧扁平快照迁成单格、两格树还原、扣掉仍在撕窗的会话、撕窗 URL 忽略树 |
| `verify-acp-render-budget.mts` | `npx tsx scripts/verify-acp-render-budget.mts` | 渲染预算：paintSet 含 @-delegate 子会话、隐藏标签 thought 走 shadow、tool 不 defer、activate 时 flush |
| `verify-permission-plan.mts` | `npx tsx scripts/verify-permission-plan.mts` | Claude Code ExitPlanMode：从权限请求的 `{plan}` JSON 里拆出 markdown，普通改文件权限不误判 |
| `verify-interaction-stream.mts` | `npx tsx scripts/verify-interaction-stream.mts` | Plan/Ask 进对话流：独立卡片、transcript 往返、composer 按会话收起 |
| `check-ui.mjs` | `node scripts/check-ui.mjs` | 通过 CDP 检查运行中的界面；需要先有一个开着 `--remote-debugging-port=9222` 的窗口 |
| `patch-zcode-acp-personal-models.mjs` | `node scripts/patch-zcode-acp-personal-models.mjs [--dry-run]` | 给已安装的 `zcode-acp-server` 打补丁：个人层 provider 没有 `providerModelRules` 时回退到 `personalModelIds` / 内置模板 `builtinModelIds`，并给这些模型接上模板的上下文窗口（否则 ACP 菜单里该 provider 一条模型都没有、用量表读 `used / 0`）。**`npm i -g zcode-acp-server` 后要重跑**；分段幂等，旧版补丁会就地升级 |

Rust 侧的自检在 `src-tauri\`：`cargo test --bin marionette`（这个 crate 是 bin-only，`--lib` 不适用）。
