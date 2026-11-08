# 调研：上下文透明化——把 dsh-context 级别的洞察搬进 Marionette（2026-11-06）

**问题起源**：[bowenliang123/dsh-context](https://github.com/bowenliang123/dsh-context)（DSH 插件，~4.3k stars）展示了"上下文怎么组成、怎么演化"的完整透视：组成环形图（系统提示词/工具定义/用户消息/注入/技能/工具调用/推理/回答）、逐请求趋势、压缩事件流、文件活动、命令执行、会话卡片。用户问：**能不能整套弄过来，让所有 agent 的数据都这样透明**。

**结论先行**：整套搬不动（它的核心数据长在 DSH harness 进程内部），但"对每个 agent 干同样的事"在架构上成立——dsh-context 本质是「**读 agent 本机会话日志 + 密度启发式估算 + 好看的 UI**」，这三件事 Marionette 都能做，而且我们支持的 agent 里 **grok-build（xAI 官方 Grok Build CLI）的本机会话库比 DSH 的还富**（系统提示词原文、工具定义、发给模型的原始消息、官方口径成本全都落盘）。缺的不是数据，是采集与呈现管线。

---

## 1. dsh-context 是怎么拿到数据的（源码级）

调研了它的仓库（v0.81.0），关键事实：

- **它自己几乎不碰磁盘路径**。会话日志经 DSH harness 服务抽象读取（`sessionQuery.observeSession` / `sessionPersistence.open(id,'read')`）；唯一有明确路径的产物是它自己写的投影缓存 `~/.dsh/storages/session_projcache/sessions/<id>.json`。
- **组成拆分是两层的**：官方 token 投影（`contextPressure`/`contextBreakdown`）只给**三分**（system/tools/messages）；插件把 `messageTokens` 按自己折叠出的面（user/inject/skill/assistant/tool）**比例分摊**。没有官方 breakdown 时整体回落插件估算。
- **"固定密度启发式"的确切公式**（`src/shared/estimate.ts`）：`CHARS_PER_TOKEN=4`、`ROLE_OVERHEAD=4`、`BLOCK_OVERHEAD=4`——文本 = `ceil(len/4)+4`；工具 schema = `ceil(JSON.stringify(tool).length/4)+4`；图片有专门的 `estimateImageTokens(w,h)`。**所有"组成"数字都是估算，官方值只用于并排校准**。
- **服务商真实值**：V4 日志里每个 `assistant/message` 事件带 `data.usage {inputTokens, cacheReadTokens, cacheWriteTokens, outputTokens, reasoningTokens}`——**每个 API 请求一条**。成本按每条 usage 的时间戳逐条入账（models.dev 牌价；DeepSeek 工作日 9–12/14–18 北京时间之外按谷价、之内整单 ×2）。
- **上下文事件**：`compaction/summary`（带 `shadowedTokenCount`）、`compaction/prune`、user/developer 消息里的注入（`source.kind` 判定）、`request/header` 的模型切换、`plan/mode`。
- **只支持 DSH**，零其它 harness 适配。

**对我们的启示**：它没有魔法——"透明化" = 本机日志逐事件重放 + 官方 usage 锚定 + chars/4 估算补维。这三件套可以泛化到任何把会话落盘的 agent。

## 2. 四层采集架构（建议）

| 层 | 机制 | 覆盖 | 时效 |
|---|---|---|---|
| **L0 ACP 在线** | 协议内的 `usage_update {used,size,cost}`（**已是 stable v1**）、`compaction_update`/`compaction_summary_chunk`（v1.25+，需 client 能力 `session.compaction`）、turn/complete 的 turnStats；Grok 无 usage_update 但已有 `applyGrokLiveContext` 推导 | 按适配器实现程度 | 实时 |
| **L1 ingest 盖戳** | 转录写入时给每个事件盖**估算 token 数**（user/thought/tool 输入输出，dsh-context 同款 chars/4+4 公式）、每轮盖 `contextUsed` 快照、压缩事件盖时间戳 | **全部 agent** | 只有今天起的数据（有损维度今天不记以后补不回来） |
| **L2 原生日志提取器** | `parsers/` 从"会话导入"扩出"统计提取"：每个 agent 读自己的本机会话库（§3 矩阵），重建历史 per-request 用量、上下文演化、组成、压缩事件 | 落盘丰富的 agent | 可回溯全部历史 |
| **L3 呈现** | 用量分析页（跨会话）+ 会话视图（单会话趋势/组成），ui-language 视觉 | — | — |

L1 与 L2 互补：L1 保"从今以后全 agent 一致口径"，L2 保"历史回填 + 真组成（系统提示词/工具定义原文）"。

### ACP 规范动向（值得盯）

- `usage_update` 已进 stable v1（PR #1371，2026-06）：`used`=上下文内 token、`size`=窗口、可选 `cost`。claude-agent-acp 重度实现，gemini-cli 实现；grok 不发（我们推导兜底）。
- `compaction_update`/`compaction_summary_chunk` 随 v1.25 稳定——**agent 的上下文压缩已经能通过 ACP 显式上报**，我们目前没消费。
- unstable 正在加 `PromptResponse.usage` 六桶（`unstable_end_turn_token_usage` 能力位）；**Issue #1860/RFD #2485 正在激烈争论 per-turn vs 会话累计语义**——各家用 `_meta` 空块、私有通知 workaround 是现状。我们的 `recordTurnUsage` 单窗口不变量恰好绕开了多头记账，方向正确。

## 3. per-agent 数据源矩阵（本机实证，2026-11-06）

机器：`C:\Users\user`，全部路径实测存在、字段抽样验证过。

### grok-build（`~/.grok/sessions/<url编码cwd>/<uuid>/`）——**最富，能做真组成拆分**

每个会话目录（本机实测）：`system_prompt.txt`（**渲染后的系统提示词原文**，实测 5.4KB）、`tool_definitions.json`、`chat_history.jsonl`（**发给模型的原始消息**，首行就是 `{"type":"system","content":"You are Grok 4.6…"}`）、`updates.jsonl`（ACP 事件流原文）、`signals.json`、`usage.json`、`summary.json`、`rewind_points.jsonl`、`compaction_checkpoints/`、`subagents/`。

- **`updates.jsonl` 的 `turn_completed` 携带 per-turn 用量**（本机最新会话实测）：`inputTokens:1143074`、`cachedReadTokens:1065984`、`costUsdTicks:2428450000`（1e-10 USD/tick = **$0.24 官方发票口径成本**，比我们 models.dev 折算更真）。注意 OpenAI 风格 `inputTokens` 含缓存，拆桶时与 Claude 语义对齐（同 usage-ledger 的 Grok 路径）。
- **`signals.json` 是现成的会话级聚合**（实测）：`contextTokensUsed:319674 / contextWindowTokens:500000 / contextWindowUsage:63`、`toolCallCount`、`toolsUsed[]`、`modelsUsed[]`、`compactionCount`、`totalTokensBeforeCompaction`、`turnCount`、`errorCount`、`toolFailureCount`、`gitCommitCount`、`prCreatedCount`…几乎就是 dsh-context 会话卡片的字段表。
- **组成拆分不需要启发式**：`system_prompt.txt` + `tool_definitions.json` + `chat_history.jsonl` 三件套就是发给模型的全部内容，按类目数 token 即可得到**真实组成**（不是估算）。
- 官方文档：[docs.x.ai sessions](https://docs.x.ai/build/features/sessions)、[仓库 17-sessions.md](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/17-sessions.md)；CLI 自带 `grok usage <session-id>`。
- ⚠️ 隐私注脚：0.2.93 被社区线迹分析证实上传 git 历史/会话 trace 到 GCS（可 opt-out）——与读取无关，但值得知道。

### claude-code（`~/.claude/projects/<encoded>/*.jsonl`）

- 每个 assistant 行 `message.usage` 四桶（本机实测）：`input_tokens` / `cache_creation_input_tokens` / `cache_read_input_tokens` / `output_tokens`（+ `server_tool_use` 明细）。**per API request**，`input+cache_read+cache_creation` = 当次 prompt 总量 → **上下文趋势可重建**。
- 两个必防的坑（ccusage 生产代码结论）：① 一次 API 轮写多行 assistant、usage 逐行重复——去重键 `(message.id, requestId, sessionId, timestamp)`；② `isSidechain:true` 的子代理日志会**重放父会话消息但换新 requestId**，须单独归账。
- **Compact 事件完整可读**：`type:"system", subtype:"compact_boundary"` 带 `compactMetadata {trigger:"auto"|"manual", preTokens:167219}`（自动压缩 ~167K 触发）；紧跟 `isCompactSummary:true` 的合成 user 行；跨文件续接靠结构判断（无 parentSessionId 字段）。
- 系统提示词/工具定义：官方文档说 transcript "can hold" 但常规 message 行里不可靠——组成只能估算。
- 无官方 schema 文档；最严谨的参考实现是 [ccusage](https://github.com/ccusage/ccusage)（Rust 解析器+去重有测试）。

### codex（`~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl`）

- `token_count` 事件（本机实测）：`info.last_token_usage`（当轮）+ `info.total_token_usage`（累计），**六桶**：`input_tokens` / `cached_input_tokens` / `cache_write_input_tokens` / `output_tokens` / `reasoning_output_tokens` / `total_tokens`。
- `session_meta`/`turn_context` 事件含 cwd、模型、instructions（系统提示词）——组成有源头可数。

### dsh（`~/.dsh/sessions/<encoded-cwd>/<uuid>/session.v4.jsonl.zstd`）

- **zstd 压缩的 typed JSONL**（本机实测）：每行 `{type, seq, time, data}`，如 `{"type":"session/end-seed","seq":3,"time":1792478421012,"data":{}}`。dsh-context 消费的事件类型：`assistant/message`（带 `data.usage` 五桶）、`compaction/summary`、`compaction/prune`、`user/message`·`developer/message`（注入判定）、`request/header`、`plan/mode`、`subagent/catalog`。
- Rust 侧需加 zstd 解压（`zstd` crate，流式）。格式无官方文档，但**事件形状在本机可直接验证**，且 DSH 自己的原则是"一条形状驱动的代码路径，绝不按版本分支"——照抄这个原则。
- 备选：`~/.dsh/storages/session_projcache/sessions/<id>.json` 是 harness 原生投影缓存（rows `{ver,seq,val}` 结构，本机实测存在），dsh-context 装上后也会往里写 `contextTimeline/contextHeaders/contextActivity` 三行——**如果用户装了 dsh-context，我们可以直接读它折叠好的结果**（耦合其内部 stateVersion，作为增强而非依赖）。
- `DSH_SESSION_ROOT` 已由我们 spawn 时钉在 `~/.dsh/sessions`（acp.rs），Marionette 会话、dsh web、导入器同库——**不存在找不到文件的问题**。

### opencode（`~/.local/share/opencode/opencode.db`，1.14+ SQLite）

- 本机 `storage/` 只剩 `migration`/`session_diff`（legacy JSON 已迁移），真相在 `opencode.db`：`session(id,parent_id,title,directory,time_created,time_updated,model,cost)`、`message(id,session_id,data)`、`part(id,message_id,session_id,data)`，data 列存完整 JSON；message JSON 带 `tokens {input,output,reasoning,total,cache:{read,write}}` + `modelID`/`providerID`——**消息级四桶**。
- 坑：schema 跨版本漂移（读前 `PRAGMA table_info`）；WAL 未 checkpoint 时最新会话可能读不到（`opencode.db-wal`）；`cost` 列常为 0 不可信。系统提示词不落盘。
- 我们的 `credentials.rs` 已经在读这个库的 credential 表，SQLite 通路现成。

### zcode（`~/.zcode/cli/db/db.sqlite`）

- 库在本机存在（含 -shm/-wal）。`parsers/zcode.rs` 已掌握 session/message/part 三表结构（导入用）；**message/part 行里有没有 usage 字段尚未探测**——parsers 升级时顺手查（SQLite 直查，勿信文档）。系统提示词大概率不落盘。

### 其它

- gemini：`~/.gemini/tmp/<hash>/chats/*.jsonl`，gemini 消息带 `tokens {input,output,cached,thoughts}` 四桶（已裁出内置名单，parser 保留，低优先）。
- pi（`~/.pi/agent/sessions`）、hermes（`~/.hermes/state.db`）：未探，低优先。
- web-chatgpt / web-deepseek 桥：我们自己就是数据源，想要什么记什么。

## 4. Marionette 现状缺口（对照）

1. **台账无 session 维度**：`usageLedger.ts` 纯按天聚合，会话卡片做不了（且红线禁止为聚合重载转录——必须 turn/complete 增量记）。
2. **`contextUsed` 不落盘**：只活在 React state 最新值；转录 `turnStats` 盖戳不含它——"上下文演化"这条有损维度**每分钟都在丢**。
3. **`compaction_update` 没消费**：ACP 已稳定上报压缩事件，我们的转录里没有。
4. **转录 `tool_call.detail` 是 clipped**：工具结果全文只在 agent 原生日志里有；我们自己的组成估算若要准，要么 ingest 时数完再裁，要么靠 L2。
5. **`parsers/` 只做导入**（list/load 两个方法），没有统计提取通道；也没有任何"跨会话读原生日志"的设施（除了导入与调试 raw log）。

## 5. 建议落地路径（分阶段，每阶段独立可验收）

- **P0（纯前端，立刻止血有损维度）**：转录 `turnStats` 盖 `contextUsed`；消费 `compaction_update`（+Grok 已有推导）；台账 day 加 `sessions` rollup（O(1) 增量）。→ 用量页出会话卡片，会话视图出上下文趋势。
- **P1（Rust 提取器，grok-build 先行）**：`parsers/` 加 `stats()` 提取通道，grok 第一个吃：`signals.json` + `updates.jsonl` 的 turn_completed（含 costUsdTicks 官方成本）+ `chat_history.jsonl`/`system_prompt.txt`/`tool_definitions.json` 做真组成。→ **grok 会话的组成环形图 + 官方成本对账**，dsh-context 的招牌功能在 grok 上先复刻。
- **P2（提取器铺开）**：claude（四桶趋势 + compact_boundary，防双计）、codex（六桶）、dsh（zstd V4）、opencode（sqlite）。统一成 `UsageFacts` 形状喂分析页，与台账并存（台账=自家会话实时账，提取器=全历史含非 Marionette 会话）。
- **P3（组成估算通用化）**：chars/4+4 密度启发式组件（dsh-context 同款公式），给没有原生日志的 agent（zcode 等）从转录估组成；有官方三分/原文的（dsh/grok）用真值锚定。

## 6. 诚实的边界

- **ACP 永远不会给出组成拆分**（协议层面只有 used/size 两个数）；组成只能靠 L2 原文或 L3 估算。
- **opencode/zcode/claude 的系统提示词不落盘**——它们的组成图里"系统提示词"块永远是估算或缺失；grok/codex 有原文。
- ACP per-turn usage 语义（RFD #2485）未定稿，跨 agent 对账要容"当轮 vs 累计"两种口径。
- dsh 的 V4 格式是 harness 内部格式，无稳定性承诺——形状驱动 + 本机回归测试兜底（DSH 自己的插件也这么过活）。
- grok 隐私争议（GCS 上传）不影响本地读取，但产品文案里别把"本地会话库"说成"数据不出机器"。

## 附：来源

- dsh-context 源码调研（仓库树 + 8 源码文件 + 2 文档，2026-11-06）：投影缓存路径/估算公式/usage 字段名/事件映射/高峰计费实现均在代码中证实。
- Claude Code 格式：[官方 claude-directory 文档](https://code.claude.com/docs/en/claude-directory)、[ccusage 源码](https://github.com/ccusage/ccusage)（去重键/sidechain 重放）、[fsck.com compact 逆向](https://blog.fsck.com/agent-blog/2026/02/22/claude-code-session-continuation)、[adityabawankule 格式笔记](https://www.adityabawankule.io/blog/claude-code-session-jsonl-format)。
- ACP 规范：[agentclientprotocol.com schema](https://agentclientprotocol.com/protocol/v1/schema)、[prompt-turn 文档](https://agentclientprotocol.com/protocol/v1/prompt-turn)、[Issue #1860](https://github.com/agentclientprotocol/agent-client-protocol/issues/1860)、[RFD #2485](https://github.com/agentclientprotocol/agent-client-protocol/issues/2485)、[PR #1371](https://github.com/agentclientprotocol/agent-client-protocol/pull/1371)。
- Grok Build sessions：[docs.x.ai](https://docs.x.ai/build/features/sessions)、[17-sessions.md](https://github.com/xai-org/grok-build/blob/main/crates/codegen/xai-grok-pager/docs/user-guide/17-sessions.md)、[ccusage grok 指南](https://ccusage.com/guide/grok)。
- OpenCode：[官方 troubleshooting](https://opencode.ai/docs/troubleshooting)、[ccusage opencode 适配器](https://github.com/ccusage/ccusage/tree/main/rust/adapters/opencode)。
- 本机实证：`~/.grok`、`~/.dsh`、`~/.claude`、`~/.codex`、`~/.zcode`、`~/.local/share/opencode` 六处目录结构与字段抽样（2026-11-06，本文 §3 各"实测"标注）。
