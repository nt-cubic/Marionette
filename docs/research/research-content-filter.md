# content_filter 归因报告：中转不是原因（2026-10-22）

> **2026-10-22 16:35 追加：性质是上游误报/缺陷，不是内容阈值。** 决定性实验：把 10-22 14:38 那次
> content_filter 的请求从会话日志原样还原（34 条消息、69061 prompt tokens），字节完全相同地重放 3 次：
> 2 次 `finish=stop`（正常出正文），1 次上游返回**无关的 400**
> `Messages with role 'tool' must be a response to a preceding message with 'tool_calls'`（该 body
> 结构经校验无孤儿 tool 消息），**0 次 content_filter**。同一份字节三种结果 → 上游行为不确定，
> 与内容无关。旁证：① 8 次失败全部 `out == reasoning`、正文 0 字；真正撞输出上限时上游报的是
> `length` 而不是 `content_filter`；② `47dfae7b` 第 8 回合 22:20 失败，22:21 第 9 回合同样上下文就通过；
> ③ 公开记录：opencode issue #35475「False positive content-filter on claude-fable-5 — charged ~$20
> for blocked output」，以及 CNB 的 deepseek-harness PR #7（网关对不支持的 `thinking`/`reasoning_effort`
> 参数直接回 `finish_reason: content_filter`，去掉参数即恢复）。结论：`content_filter` 在
> OpenAI-compatible 网关上是一个**泛化的失败信号**，不等于审核命中。


结论先行：**magpie 转发给上游的请求，除了把模型 id 从 `opencode-go/deepseek-v4.1-flash` 改写成
`deepseek-v4.1-flash`，与 DSH 直连的请求逐字段一致**（实测抓包，messages/tools/采样参数完全相同）。
`PI_AI_ERROR: Provider finish_reason: content_filter` 在升级后的**直连**路由上同样出现；原统计里
"marionette 27% vs 直连 0%" 是**时间选择偏差**——42 个直连会话有 40 个在 10-10~10-14（升级前）。
A–E 五项全部不成立，详见第 2 节。

---

## 1. 同一 prompt 的三份请求体（实测，不是推演）

对照用的对话：system（DSH agent 提示词节选）+ 一条"看下本地 v0.2 战斗策划案，实现 html 版"的用户消息 +
一轮带 thinking 的 assistant 工具调用 + tool 结果 + 用户"继续"。三份 body 的采集方式见 §1.2。

| 字段 | ① 直连上游（DSH → opencode-go） | ② marionette（DSH → 127.0.0.1:3425） | ③ 中转出站（magpie → opencode-go） |
|---|---|---|---|
| `model` | `deepseek-v4.1-flash` | `opencode-go/deepseek-v4.1-flash` | **`deepseek-v4.1-flash`**（去前缀） |
| `thinking` | `{"type":"enabled"}` | **无** | **无** |
| `reasoning_effort` | `"high"` | **无** | **无** |
| `max_tokens` | `384000` | 无 | 无 |
| `max_completion_tokens` | 无 | `32768` | `32768` |
| `store` | 无 | `false` | `false` |
| `tools[0].function.strict` | `false` | 无 | 无 |
| `messages` | 5 条：system / user / assistant / tool / user | **与①逐字节相同** | **与②逐字节相同** |
| assistant 的 `reasoning_content` | 有（thinking 原文） | 有（同一段） | 有（同一段） |
| `tools` | 1 个（read） | 同左 | 同左 |
| `stream` / `stream_options` | `true` / `{"include_usage":true}` | 同左 | 同左 |

逐 JSON 路径 diff（①→②）只有 6 处，外加 tools 里的 `strict`：

```
/model                    "deepseek-v4.1-flash" -> "opencode-go/deepseek-v4.1-flash"
/thinking                 {"type":"enabled"}    -> <absent>
/reasoning_effort         "high"                -> <absent>
/max_tokens               384000                -> <absent>
/max_completion_tokens    <absent>              -> 32768
/store                    <absent>              -> false
/tools/0/function/strict  false                 -> <absent>
messages identical: True
```

②→③（中转实际发出的 body）：**只有 `/model` 一处不同**。中转没有注入 system prompt、没有合并/重排
messages、没有补 thinking 参数、没有改 `max_completion_tokens`/`store`/`strict`。

### 1.1 上游接受度

把 ①、② 两份 body 的 `model` 都改成 `deepseek-v4.1-flash`，带 `x-opencode-session` 直接 POST
`https://opencode.ai/inference/go/v1/chat/completions`：

| body | HTTP | finish_reason | reasoning chunks | completion tokens |
|---|---|---|---|---|
| ① 直连形态 | 200 | `tool_calls` | 746 | 967（reasoning 747） |
| ② 中转形态 | 200 | `tool_calls` | 300 | 378（reasoning 301） |

即 `max_completion_tokens`、`store:false`、缺 `thinking` 这些差异都被上游接受，且**两种形态都会思考**。

### 1.2 采集方法（可复现）

- ① / ②：用 pi-ai 自己的 `streamSimple`（`@earendil-works/pi-ai/dist/compat.js`）在 `fetch` 前截获 body，
  不联网。模型对象按两条路由的真实物化规则构造：
  - 直连：`providers/data/opencode-go.json` 的 `chat:deepseek-v4.1-flash`，`provider=opencode-go`；
  - marionette：`dsh-llm-pi-ai` 对"目录里没有的 route"的物化结果——`reasoning:false`、无 `compat`、
    `contextWindow:262144`、`maxTokens:32768`、`baseURL:http://127.0.0.1:3425/v1`。
  脚本：`D:\Temp\capture.mjs`；输出：`D:\Temp\body_direct.json`、`D:\Temp\body_marionette.json`。
- ③：把 `~/.marionette/engine` 复制成 portable 副本（`D:\Temp\mgtest`），只把
  `data/providers.json` 里 `opencode-go.chat` 指向本地 catcher，`magpie serve` 起在 3598，把 ② 原样 POST
  进去；catcher 记录到的 body 即 ③（`D:\Temp\mg_capture.jsonl`）。真实上游 URL 下 magpie 还会加
  `x-opencode-session`/`x-opencode-client` 等头（`provider/opencode_client.go:85-90`），body 不变。

---

## 2. A–E 逐项判定

### A. 凭证：不是账号差异

- 引擎用自己 `data/providers.json` 里的 key（`oc_sk_38b0…`，sha256 前缀 `72000db2b4`）；
  DSH 直连用 `.credentials.yaml` 的 `OPENCODE_GO_API_KEY`（`oc_sk_154e…`，sha256 前缀 `77bfd207f040`）。
- 两把 key 打 `https://opencode.ai/zen/go/v1/usage` 返回**完全相同**的窗口与重置时刻：
  `rolling 18% @ 2026-10-22T11:23:20Z`、`weekly 17% @ 2026-10-26T00:00:00Z`、
  `monthly 88% @ 2026-11-09T15:23:05Z` —— 同一工作区/套餐，只是两把独立 key。
- 更直接：用 DSH 自己 key 的直连路由也出现了 content_filter（见 §3）。**A 排除。**

### B. 模型 id 映射：没有换变体

- 中转出站 `model` = `deepseek-v4.1-flash`（抓包），上游响应回显 `model` 也是 `deepseek-v4.1-flash`；
  没有 `:free`、没有 revision 后缀、没有换 provider。
- 引擎 usage 里 `served` 字段也一直是 `deepseek-v4.1-flash`（`~/.marionette/engine/data/usage.jsonl`）。
  改写发生在 `gateway.go:3963-3964` 的 `UpstreamNameIn` + `rewriteModel`。**B 排除。**

### C. thinking / reasoning 参数：真实差异，但不是原因

差异确实存在，来源不是中转，而是 DSH 侧模型元数据缺失：

- `dsh_bridge.rs:189-219`（`marionette_lines`）只写 `id`/`input`，**没有 `reasoningEfforts`**；
- `dsh-llm-pi-ai/lib/index.js:585-607`：`reasoningEfforts` 缺失 → `reasoning: base?.reasoning ?? false`，
  而 marionette 路由不在 pi-ai 目录里 → `base` 为空 → `reasoning:false`；
- `pi-ai/dist/api/openai-completions.js:670-681`：只有 `model.reasoning && thinkingFormat==="deepseek"`
  才发 `thinking:{type:"enabled"}` + `reasoning_effort` → marionette 路由都不发。

但这不是过滤的原因：

1. 上游缺省就思考：不带 `thinking` 的请求实测仍返回 reasoning（探针 49 tokens；§1.1 的②也 301 tokens）。
2. 带 `thinking` 的**直连**在 10-22 15:19 的会话里同样被 content_filter（§3）。

**C 排除为因果项**（它是"路由不等价"的独立缺陷）。

### D. system prompt / messages 重排：没有发生

- 抓包 messages 与直连**完全相同**（system、user、assistant 含 reasoning_content、tool、user）。
- 中转在 Chat→Chat 时走 `passthrough`（`gateway.go:3497,3547-3553`），不经过 `buildChat`
  （`chat.go:267`；`chat.go:540-541` 的注释也写明 Chat→Chat 原样转发）。
- `passthrough` 的 Chat 分支只有这些改写（`gateway.go:4031-4044`）：`developerAsSystem`、
  去空 assistant、`withReasoningOnToolTurns`、`chatCallSignatures`。其中唯一可能在 messages 上留下痕迹的是
  `withReasoningOnToolTurns`（`chattidy.go:396-425`）：**只**给"有 tool_calls / 紧跟 tool 结果、且没有
  reasoning_content"的 assistant 补一个 `" "`。本例 assistant 自带 reasoning_content，未触发。
- 中转不注入任何自己的 system prompt。**D 排除。**

### E. 请求头 / 字段差异：有，但均被上游接受

- `max_tokens:384000`（直连）vs `max_completion_tokens:32768`（marionette）：因为
  `model.maxTokens` 缺失时 dsh-llm-pi-ai 落 `DEFAULT_MAX_TOKENS=32768`
  （`dsh-llm-pi-ai/lib/index.js:695,959-961`），而 `compat.maxTokensField` 又因 route 不在目录而
  默认为 `max_completion_tokens`（`openai-completions.js:1253-1270`）。
- `store:false`：marionette 的 baseUrl 不是 opencode.ai，pi-ai 的 `supportsStore` 探测为 true，
  于是发 `store:false`（`openai-completions.js:1264-1265,587-589`）。
- `tools[].strict:false`：直连 compat 的 `supportsStrictMode:true`，marionette 无 compat 不发。
- 头：直连 `x-opencode-session`=会话 id；中转每请求新生成一个（`opencode_client.go:76-90`），
  只影响亲和性。两把 key 同工作区（A）。
- 两份 body 直接打上游都 200、都 `tool_calls`（§1.1）。**E 排除为因果项**。

---

## 3. 统计复核：原对比被时间因素混淆

按会话文件（`~/.dsh/sessions/**/session.v4.jsonl.zstd`，逐帧解压）重扫，按 `request/header.config.provider` 分组：

| 路由 | 时间 | 会话数 | 有 content_filter 的会话 | 失败调用数 |
|---|---|---|---|---|
| 直连 `opencode-go` | 10-10 ~ 10-14 | 40 | 0 | 0 |
| 直连 `opencode-go` | 10-22 15:16 / 15:19 | 2 | 1 | 1 |
| `marionette` | 10-21 ~ 10-22 | 22 | 5 | 8 |

关键反例（直连也中）：

- **`session-6c19d79f-ce24-4cbf-b905-b6d1f5a59913`**，`provider=opencode-go`，`model=deepseek-v4.1-flash`，
  cwd `D:\Projects\惊蛰\战斗`，10-22 15:19 开始，第 45 步 15:42:28 以
  `Provider finish_reason: content_filter` 结束。该时段 `~/.marionette/engine/data/usage.jsonl` 里
  **没有任何 dsh 调用**（15:30–15:49 为空），排除"实际走了中转"。
- 失败发生在纯 reasoning 流中途，最后一段 reasoning 是战斗数值推演，例如直连这次：
  `"pierce hit enemy 01 for 16 (front? 34…18) instead of backmost enemy 03 …"`。
- 升级前同项目直连 `session-f90d63fd…`（10-14 18:31，`D:\Projects\惊蛰\战斗测试`）：
  **1000 步、658 段 reasoning、约 200 万字符**，其中 `kill` 2732 次、`enemy` 886 次、`damage` 256 次，
  **0 次 content_filter**。

每条调用的发生率（可比口径）：

- 中转：8 次失败 / 1712 次 dsh 调用 = **0.47%**（引擎 usage 全量；10-21 为 1/1022，10-22 为 7/696）。
- 直连升级后：1 次 / 约 108 次调用（2 个会话共 63+45 步）≈ **0.9%**。
- 两者同量级，样本都很小；直连不是 0。

时间线也支持"内容/上游变化"而不是"路由"：

- 用户 10-21 18:20 才开始 `D:\Projects\惊蛰\战斗`（该会话 cwd 即此）；
- 引擎侧第一次 content_filter 是 **10-21 22:20**，此前 1022 次调用里只有 1 次；
- 10-22 的 7 次全部集中在 14:04–15:17 的 `战斗`/`战斗测试` 会话。

---

## 4. 修复建议

### 4.1 中转：没有可以"改一行"的修复

抓包证明 body 只差 `model` 前缀改写，且改写是必须的（上游只认裸模型名）。任何在 magpie 里删字段、
加字段、改 messages 的"修复"都缺乏证据，且会破坏 Chat→Chat 的透传语义（`gateway.go:3497,3547-3553`）。

### 4.2 若要做"路由等价性"（独立缺陷，不解决 content_filter）

`dsh_bridge.rs:189-219` 的 `marionette_lines` 可以在每个模型上补：

- `reasoningEfforts`：引擎 `/v1/models` 已经给出 `supported_reasoning_levels: low/high/max`，
  写成 `low: low`、`high: high`、`max: max` 即可让 DSH 认为 `reasoning:true`；
- 路由级 `compat`：`supportsDeveloperRole: false`、`maxTokensField: max_tokens`、
  `thinkingFormat: deepseek`、`requiresReasoningContentOnAssistantMessages: true`、
  `supportsStore: false`；
  **`supportsDeveloperRole: false` 在这一步是必须的**：单独加它现在没有作用（`reasoning:false`，
  pi-ai 本来就走 `system`，用户已验证），但一旦补了 `reasoningEfforts`，`model.reasoning` 变 true，
  而 `127.0.0.1` 的 baseUrl 会被 pi-ai 探测成 `supportsDeveloperRole:true`
  （`openai-completions.js:1264-1266`）→ 立刻开始发 `developer`。两处必须一起改。
- 确认 `contextWindow`/`maxTokens` 真的写进去：当前托管块没有（会话 `request/context` 实测
  `contextWindow:262144`，而引擎目录是 1000000/384000）——这是另一个已知兜底问题。

**已验证**：按上述字段构造 marionette 模型后，pi-ai 生成的 body 与直连 body **逐字节相同**
（除 `model` 前缀；脚本 `D:\Temp\parity.mjs`，输出 `D:\Temp\body_parity.json`）。也就是说这套改动能
把 C/E 的线上差异全部抹平——但仍然不会消除 content_filter（直连也会中）。

**为什么这不会破坏"让所有 agent 读取各自配置的 provider"**：`merge_home_patch` 只在
`llm-pi-ai.config.providers` 下新增/替换 `marionette` 这一个子块（`dsh_bridge.rs:361-386`），
其它 provider（`opencode-go` 直连、`deepseek`、`openrouter`…）原样保留；`marionette` 的模型 id 仍是
`provider/model` 形式，别的 agent 不读这个块，也不受影响。

**必须说明：这不是 content_filter 的修复。** 10-22 15:19 的直连会话带着完整
`thinking`/`reasoning_effort`/`max_tokens` 仍然被过滤。补元数据只会让两条路由的线上请求同构，
并顺带修 256K 上下文兜底。

### 4.3 content_filter 本身怎么处置

上游的 `content_filter` 是**泛化失败信号**（见文首追加），不是请求形状问题，也不是稳定的内容阈值：
同一份字节重放会得到不同结果。可选方向，按证据强度排序：

1. **上报 opencode-go**：附 `response_id`、时间、模型、`finish_reason=content_filter`、失败发生在
   reasoning 流中、以及"同一请求重放 3 次 0 次复现"的对照。这是唯一能真正消除误报的路径。
2. **把 content_filter 当可重试的 finish_reason**：DSH 适配器对"还没有任何正文、只出了 reasoning"的
   content_filter 重发一次（换 session id）。magpie 已有 refusal 语义（`fallback.go:1498-1506`），
   但对单 provider 不重试；扩展它属于行为变更，需要产品决定和次数上限，**本次没有实现**。
3. 不要用"关掉 thinking"或"改 system prompt"来赌：直连带 thinking 也会中，且没有证据表明改动能降低命中率。

---

## 5. 仍未确定 / 缺什么

- 上游是"10-21 起启用/收紧了输出审核"还是"`deepseek-v4.1-flash` 换了 revision"：本机数据无法区分。
  `/v1/models` 的 `created` 是缓存时间（1792655553 ≈ 抓取时刻），不能当模型发布时间。需要 opencode-go
  侧确认。
- 没有拿到审核命中的具体 token 片段；只能确认失败总在 reasoning 流中、且文本是战斗数值/击杀描述。
- 直连升级后的样本只有 2 个会话（1 次失败），发生率置信区间很宽；但"直连 0 次"已被证伪。

---

## 附：证据文件

| 内容 | 路径 |
|---|---|
| 直连形态请求体 | `D:\Temp\body_direct.json` |
| marionette 形态请求体（DSH→中转） | `D:\Temp\body_marionette.json` |
| 中转实际出站请求体 | `D:\Temp\mg_capture.jsonl` |
| 抓包脚本 | `D:\Temp\capture.mjs` |
| 元数据等价性验证（补全后 body 与直连一致） | `D:\Temp\parity.mjs` / `D:\Temp\body_parity.json` |
| 直连失败的会话 | `C:\Users\Administrator\.dsh\sessions\--D-Projects-~60CA~86F0-~6218~6597--\session-6c19d79f-ce24-4cbf-b905-b6d1f5a59913\session.v4.jsonl.zstd` |
| 中转失败的会话（示例） | 同目录 `session-30b8e428-…`、`session-6f787bda-…`、`session-330d1330-…`、`47dfae7b-…`、`80cca61c-…` |
| 引擎逐调用日志 | `C:\Users\Administrator\.marionette\engine\data\usage.jsonl` |
| 升级前直连 1000 步对照 | `C:\Users\Administrator\.dsh\sessions\--D-Projects-~60CA~86F0-~6218~6597~6D4B~8BD5--\session-f90d63fd-e859-4dc1-b82f-adfcb01d3201\session.v4.jsonl.zstd` |
