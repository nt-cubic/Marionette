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
| `verify-bridge-routing.mts` | `npx tsx scripts/verify-bridge-routing.mts` | 跨用模型路由：绑定裸 id → agent 原生 `marionette…` wire id；OpenCode 落回 OpenRouter/缺 provider 时芯片判 stale，缓存态不误报，Claude/Codex 间接注入不误判 |
| `verify-model-source-engine.mjs` | `node scripts/verify-model-source-engine.mjs [magpie.exe]` | 临时隔离目录与本机假接口，验证已安装引擎的自定义 provider 添加、带 tag 的模型、换钥匙、模型更新、删除；仅用假 key |
| `verify-engine-contract.mts` | `npx tsx scripts/verify-engine-contract.mts <engine.exe>` | 进程内 Rust 引擎与 Magpie 回退共用的唯一合同尺：C1–C6 + D1–D10（三协议流式/工具/thinking/压缩/Origin/错误/断流/组转移/整数工具参数） |
| `verify-rust-sku-e2e.mjs` | `node scripts/verify-rust-sku-e2e.mjs` | 隔离 profile + CDP 9223 驱动真实 Tauri 注册链，验证进程内网关状态、来源写盘、模型目录、Chat/SSE、额度命令；不要混 `--lang` |
| `t13-real-key-reconcile.mjs` | `node scripts/t13-real-key-reconcile.mjs` | 使用隔离目录和用户已配置的真实来源做只读/最小生成对账；不打印 key，不碰真实 usage 台账 |
| `upstream-watch.mts` | `npx tsx scripts/upstream-watch.mts` | 研究克隆的 Magpie 上游 diff → 合同区分级 digest；只提示需要哪些行为移植，不自动合并代码 |
| `caps-smoke.mts` | `node scripts/caps-smoke.mts` | Composer 的能力合并：live `session/new` 数据优先、兜底表只在 live 为空时生效、档位标签不重复轴名、未知 agent 直通 |
| `smoke-stream-merge.mts` | `npx tsx scripts/smoke-stream-merge.mts` | 流式合并：Grok Reply 逐 token、Codex Thought 不丢标点、多会话交错不串台；notice/retry 作为独立通知卡进入对话流并通过 JSONL 往返；OpenCode v2 子代理流进 Task 卡不进 Reply；OpenCode write 从 `rawInput.content` 合成 diff 并在 success note 后保留；逐字数字 `1000` 与重复字不丢；Grok `spawn_subagent` + `_x.ai/session/update` 收进并行子代理卡 |
| `verify-memo-box.mts` | `npx tsx scripts/verify-memo-box.mts` | 收纳盒：origin `memo` 的 Chat 会话拆进侧栏「收纳盒」栏目、10-22 方案的 `~/.marionette/inbox` 幽灵项目在加载口被过滤（正/反斜杠、尾斜杠）、memoPrompt dialog/doc 预填契约、文字拖放通道（页内拖拽源 tracker 让路 + dock 区文字 DOM 事件）、单列帖子流（`groupMemosByDay` / 顶上写一条 / 四种卡，不再是 40px 文件行）、图片卡按横竖比 contain 完整显示；同时钉住单格完整 tab page、真实多格才用每格标签栏、surface 标签不串格、常驻复选框多选、单一更多菜单和窄格底栏不溢出 |
| `verify-agent-reentry.mts` | `npx tsx scripts/verify-agent-reentry.mts` | 再次进入对话（post-turn 续跑）：waiting+优雅期才武装、迟到回声/运行中/无回合边界不武装；安静收尾只封本会话尾卡（Reply→durationMs、Thought→endedAt），已封卡与他窗事件不动；zcode 真实形状回放不被粘进前一张卡 |
| `verify-staff-status.mts` | `npx tsx scripts/verify-staff-status.mts` | 员工模式纯逻辑：waiting_user > failed/interrupted > running > thinking > idle、终态保留与 stale、任务/承诺计数、channel-neutral 通知优先级与勿扰/冷却/小时额度、项目权限 round-trip、员工行 due commitment 徽标 |
| `verify-staff-chat.mts` | `npx tsx scripts/verify-staff-chat.mts` | 员工聊天投影逻辑：chief 会话仅保留用户、员工、决策以及警告/错误事件。 thought、tool call、activity、model 和 token 元数据保持隐藏。 输入状态和身份解析具有回归覆盖率。 |
| `verify-exit-meaning.mts` | `npx tsx scripts/verify-exit-meaning.mts` | 命令退出码人话（移植 codeg exit-meaning）：grep/rg exit 1 无匹配、test 不成立、diff 有差异、which/ls 未找到、git grep/diff/rev-parse 分命令答案、127/126/124 命名失败、128+n 信号名；保守面（管道取尾段、`&& echo` 回退、cd setup、set -e/子壳/替换/后台/heredoc 放弃、shell 抱怨行不冒充答案）；i18n 详情文案 |
| `verify-pasted-files.mts` | `npx tsx scripts/verify-pasted-files.mts` | 粘贴文件按真实路径附加（移植 codeg pasted-file-paths）：webview File（名字+大小）与 OS 剪贴板 CF_HDROP 条目配对、目录按名配对、同名不同文件夹一对一不串、配不上的保持顺序进 rest、中文文件名 |
| `verify-inline-dollar.mts` | `npx tsx scripts/verify-inline-dollar.mts` | 守卫式单美元行内公式（移植 codeg inline-dollar-math）：紧贴同行 TeX 形态（`$x^2$`/`$E=mc^2$`/`$f(x)$`/字母符号）改写为 `$$` 行内；货币对/shell 展开/代码与原始 HTML/display 围栏/裸金额/跨行/未闭流式片段保持字面；幂等；scan 把围栏外候选当数学提示 |
| `inventory-zh.mts` | `npx tsx scripts/inventory-zh.mts [--rust] [--lines] [--check]` | i18n 盘点：列出（注释以外）还有中文的源码文件与行号。`--rust` 扫 `src-tauri/src`，`--lines` 给全量行号，`--check` 前端白名单外非零即退出 1。注意：正则密集文件可能误报（正则里的引号会骗过注释剥离器），Rust 侧 `ts!` 调用点按设计保留中文 |
| `check-locale-fragments.mjs` | `node scripts/check-locale-fragments.mjs src/locales/parts/<batch>.zh.ts …` | i18n 片段对齐快检：每个 en 片段的键必须与对应 zh 片段完全一致（缺键/多键/重复键报错）；权威校验仍是一把梭 `npx tsc --noEmit` |
| `usage-smoke.mts` | `npx tsx scripts/usage-smoke.mts` | usage 面板解析，断言用的是真实 adapter 抓到的报文 |
| `verify-quota-fallback.mts` | `npx tsx scripts/verify-quota-fallback.mts` | 额度面板 own-family 兜底、Antigravity 模型分组与本地重置时间、窗口和余额展示 |
| `verify-claude-usage-meta.mts` | `npx tsx scripts/verify-claude-usage-meta.mts` | Claude `_claude/rateLimit` 三种载荷形状：unifiedWindows-only（claude-agent-acp 0.109.x 常态，顶层 utilization 缺席）、顶层+unified 并存、顶层-only 旧形状；合并序「有百分比的行不被无百分比的行顶掉」；claude-code 会话的右栏 own 订阅主块 |
| `verify-usage-refresh.mts` | `npx tsx scripts/verify-usage-refresh.mts` | 冷会话刷新额度前先启动 ACP；正在运行的回合延后探针 |
| `title-smoke.mts` | `npx tsx scripts/title-smoke.mts` | 会话标题：拒绝 Marionette 自己注入的 prompt 抬头被当成标题，修复时从转录里取用户真正打的第一句 |
| `suspend-smoke.mts` | `npx tsx scripts/suspend-smoke.mts` | 挂起规则两套：手动 ⏸ 按钮对**任何还持有进程**的会话都可按（starting / running / waiting / error，running 时按下去直接中断），只有已挂起的不画按钮；而 2 小时空闲自动挂起仍只碰「热但空闲且没有待办」的会话（子代理/后台命令/定时发送算在运作） |
| `verify-live-subagents.mts` | `npx tsx scripts/verify-live-subagents.mts` | 嵌套子代理仍在跑：OpenCode/Claude `in_progress` Task、Grok「Subagent started in background」完成桩、ZCode `[background]`、Codex collab `spawnAgent` 及活跃 `wait` 的全部 receiverThreadIds、通用 spawn 动词 `delegate_task`；`create_goal` / 已完成 wait 不算 |
| `verify-subagent-adapters.mts` | `npx tsx scripts/verify-subagent-adapters.mts` | 各 agent 走私方式都能认成独立子代理卡：ZCode 标题是任务描述 + `_meta.claudeCode.toolName`、Codex collab `rawInput` / 无 spawn 的 `_meta.codex.subagent`、通用 spawn 动词 `delegate_task`；wait/close/goal 排除 |
| `fork-smoke.mts` | `npx tsx scripts/fork-smoke.mts` | 分叉的切点与拷贝：按 message id（退化为文本）定位那条回复，子任务/交接卡片不带过去，事件原对象不被改 |
| `image-path-smoke.mts` | `npx tsx scripts/image-path-smoke.mts` | 对话里图片的路径：相对路径按会话工作目录拼成绝对路径，`file://` 去协议，绝对路径原样（用的是真实转录里的路径） |
| `verify-image-crop.mts` | `npx tsx scripts/verify-image-crop.mts` | 图片附件默认裁剪框：全图起点、四边/四角拖动、最小尺寸与越界限制、标注随裁剪重映射 |
| `verify-usage-row-layout.mts` | `npx tsx scripts/verify-usage-row-layout.mts` | usage 行布局：不再渲染 Last turn / Session total |
| `verify-text-pipeline.mjs` | `npm run verify:text` | 文本 / 展示管线的冒烟检查（只依赖 `markdownText.ts`，不需要整棵 import 图） |
| `verify-rich-markdown.mts` | `npx tsx scripts/verify-rich-markdown.mts` | 富文本管线：纯字符串扫描决定 katex/shiki/mermaid/spec 是否加载（bash `$$` 不当公式、单 `$` 永远字面、首 token 路由、```` 围栏状态机）、LaTeX `\[..\]`→`$$..$$` 改写不动围栏内、```spec 卡片 schema 三道防护（JSON 容错/原型污染/深度+尺寸上限）与各 type 形状校验 |
| `verify-browser-pick.mts` | `npx tsx scripts/verify-browser-pick.mts` | 浏览器元素批注的数据层：不可信页面声明、URL/query/hash/path 脱敏、多页面分组、元素/框选块、ref 与截图顺序、页面文本单行净化、标签拼装、页面输入屏障/可信事件/24 条上限的静态契约、双语词典对齐 |
| `verify-browser-overlay.mts` | `npx tsx scripts/verify-browser-overlay.mts` | 原生浏览器遮挡判断：模态弹窗、相交菜单、隐藏浮层及多个浮层关闭 |
| `verify-browser-link.mts` | `npx tsx scripts/verify-browser-link.mts` | 浏览器链接路由：主窗口进应用内浏览器、无宿主时回退系统浏览器，file/javascript 不被当作浏览器链接 |
| `verify-workspace-surface.mts` | `npx tsx scripts/verify-workspace-surface.mts` | Surface 标签 lib 层：`document:` tab id 与会话树路径编解码、fresh browser tab 只有 `display:"workspace"` 才抢工作区。旧的并排 SurfaceWorkspace / 宽度换算助手已删除，布局与拖拽语义由 `verify-split-workspace` 钉 |
| `verify-dev-server.mts` | `npx tsx scripts/verify-dev-server.mts` | 开发服务器发现：ANSI 去除、loopback/通配地址归一化、公网与 LAN 拒绝、尾标点清理、同 origin 去重、多服务识别、默认只通知 |
| `verify-browser-prefs.mts` | `npx tsx scripts/verify-browser-prefs.mts` | Agent tab 空闲清理偏好：默认 10 分钟、允许值、非法值回落，以及 10 → 30 → 60 → never 循环 |
| `verify-activity-cluster.mts` | `npx tsx scripts/verify-activity-cluster.mts` | 活动折叠：thought+tool 同一集群、同类连续分组、结束后摘要为 `1 think, 2 read, 3 edit`、live 时跟当前工具 |
| `verify-transcript-export.mts` | `npx tsx scripts/verify-transcript-export.mts` | 对话 Markdown 导出：主干发言、thought/tool 进 `<details>`、连续 `file_change` 合成交付表、Windows 文件名净化、空 thought 不产出空折叠块 |
| `verify-conversation-find.mts` | `npx tsx scripts/verify-conversation-find.mts` | 对话内查找：主干卡可搜（你/回复/计划/提问/交接/子代理/改动，重命名旧路径与选项/选中项参与）、thought/tool 返回 null（折叠簇内 v1 不搜）、大小写不敏感子串、查询 trim、命中下标升序、空查询/无命中 |
| `verify-usage-ledger.mts` | `npx tsx scripts/verify-usage-ledger.mts` | 跨会话 Token 台账 + 牌价折算：按本地日落账（input/output/cached/cacheCreation/reasoning/turns/流式时长）、agent 与 model 拆分、小时桶、全 null turnStats 不记、180 天修剪边界、今天/近 7 天/近 30 天 + 上一周期聚合、按助手/模型排名（>limit 折叠其它）、逐日序列与小时×星期折叠、models.dev 目录解析/厂商优先/命名空间剥离匹配（`marionette\`、`marionette::`、`vendor/`）/按桶计价与未计价覆盖率、en 词典直查（占位符 `{count}`）、**CSV 导出**（long format 表头/日合计行含 ms/模型行按四桶降序且 reasoning·ms 留空/无记录日不出行/逗号引号转义/文件名后缀） |

| `verify-turn-delivery.mts` | `npx tsx scripts/verify-turn-delivery.mts` | 交付卡折叠文案「N 个文件已更改」、git truth 与 tool edit 并集、git numstat 覆盖工具统计、**本轮归属规则**（起点就脏的文件需本轮工具碰过、无 HEAD 不丢文件、路径归一） |
| `verify-transcript-scroll.mts` | `npx tsx scripts/verify-transcript-scroll.mts` | 对话 tab 滚动：贴底/历史位置快照、切回时恢复、滚轮朝上 hold-off 不被 80px 回贴、虚拟列表上方行高补偿、行高缓存跨 tab 还在 |
| `verify-session-order.mts` | `npx tsx scripts/verify-session-order.mts` | 侧栏折叠预览：置顶行和「刚完成、还没点进去」的对话不会掉进 Show more |
| `verify-agent-errors.mts` | `npx tsx scripts/verify-agent-errors.mts` | 错误分类：Grok `auth_transient` 不当成需要登录、OpenCode 按当前模型渠道给提示、426 仍走更新 |
| `verify-background-tasks.mts` | `npx tsx scripts/verify-background-tasks.mts` | 后台任务面板聚合：需要你置顶、跑动中次之、已结束沉底；完成/失败委托不进列表、结束终端保留退出码；key 跨重建稳定；空输入不渲染 |
| `verify-delegate-kit.mts` | `npx tsx scripts/verify-delegate-kit.mts` | AI 自主派活（第一期拆门）：agent-delegate 行状态映射（waiting_user→需要你）/终态 10 分钟退场/点击 worker 优先；fe-delegate 片段对键集与 {agent}{title} 占位符；agent-worker 归后端驱动家族（挂起/压制豁免 + tee 回填）；父对话卡片规则 lib/delegateCards.ts（task.progress 不是卡——额度午休不得被画成失败、运行卡/结果卡各只落一次、failed/cancelled 的 error 留空只显示本地化状态、有存档点才带回滚依据，2026-11-23） |
| `verify-split-workspace.mts` | `npx tsx scripts/verify-split-workspace.mts` | 分屏树：拆/并/关标签、上下拆、1\|1\|1 均分、1\|2 / 2×2、第 5 叶降级、命中测试、撕窗领养进格、屏幕坐标换算；surface 窗格标签（2026-11-18）：tab 编解码、openSessionIds/visible 只数会话、纯 surface 树合法不触发占位、恢复闸门丢 browser 保 memo/document |
| `verify-composer-compact-toolbar.mts` | `npx tsx scripts/verify-composer-compact-toolbar.mts` | 输入框过窄时底栏芯片收成贴边同款 hub：composer 自身 ≤580px（贴边始终）、≥640px 摊开；容器查询第一帧就收、JS 回差拖分屏条不闪 |
| `verify-composer-prefs.mts` | `npx tsx scripts/verify-composer-prefs.mts` | 磁盘偏好回放的作业寿命：caps/sessionPrefs 身份变化不得取消正在跑的恢复链（Grok `/always-approve` 是最后一步）；只有切对话、卸载、agent 进程死掉才作废；always-approve 回放仍在链内 |
| `verify-capability-drift.mts` | `npx tsx scripts/verify-capability-drift.mts` | 偏好漂移检测：account: 前缀的模型偏好（zcode 账号枚举按 spawn 凭据浮动）不因「本 spawn 目录里没有」被清空；普通模型 id 沿用清空行为；`isAccountPrefTransientlyMissing` 是发送链 fail-closed 的唯一判定 |
| `verify-ui-restore.mts` | `npx tsx scripts/verify-ui-restore.mts` | 冷启动恢复：旧扁平快照迁成单格、两格树还原、扣掉仍在撕窗的会话、撕窗 URL 忽略树；Document/Memo surface 恢复，旧 Browser URL/活动项永久忽略 |
| `verify-acp-render-budget.mts` | `npx tsx scripts/verify-acp-render-budget.mts` | 渲染预算：paintSet 含 @-delegate 子会话、隐藏标签 thought 走 shadow、tool 不 defer、activate 时 flush |
| `verify-permission-plan.mts` | `npx tsx scripts/verify-permission-plan.mts` | Claude Code ExitPlanMode：从权限请求的 `{plan}` JSON 里拆出 markdown，普通改文件权限不误判 |
| `verify-interaction-stream.mts` | `npx tsx scripts/verify-interaction-stream.mts` | Plan/Ask 进对话流：独立卡片、transcript 往返、composer 按会话收起 |
| `check-ui.mjs` | `node scripts/check-ui.mjs` | 通过 CDP 检查运行中的界面；需要先有一个开着 `--remote-debugging-port=9222` 的窗口 |
| `patch-zcode-acp-personal-models.mjs` | `node scripts/patch-zcode-acp-personal-models.mjs [--dry-run]` | 给已安装的 `zcode-acp-server` 打补丁：个人层 provider 没有 `providerModelRules` 时回退到 `personalModelIds` / 内置模板 `builtinModelIds`，并给这些模型接上模板的上下文窗口（否则 ACP 菜单里该 provider 一条模型都没有、用量表读 `used / 0`）。**`npm i -g zcode-acp-server` 后要重跑**；分段幂等，旧版补丁会就地升级 |

Rust 侧的自检在 `src-tauri\`：`cargo test --bin marionette`（这个 crate 是 bin-only，`--lib` 不适用）。
