# Rust 进程内引擎方言审计（Go magpie 对照）

日期：2026-11-18　范围：只读审计，未改 `src-tauri/`。
Rust 侧：`src-tauri/src/engine_gateway/`（routes / httpd / translate / chat / responses / messages / bodycap / groups / effort / codex_account / grok_account）。
Go 侧（参照）：`third_party/magpie/internal/gateway/`（gateway.go 路由表、request_bounds.go、anthropic.go、chat.go、responses.go、cors.go、codex_catalog.go、codex_socket.go、prev_response.go）。

## 0. 先说结论

- **当前在跑的是 Go 引擎**：`dev.log` 里 `engine state → running ... version=0.1.4232`，横幅是 Go 的 `magpie gateway on …`，日志格式是 Go 的 `2026/11/18 … codex: fetched its list again`。Rust 引擎只在 `MARIONETTE_ENGINE=rust` SKU 下出现。下面的缺口全部是**切到 Rust 引擎前**必须补的清单，目前不影响默认发布版。
- 与你提到的「custom/namespace/tool_search/local_shell 被丢」相邻的问题里，**local_shell 不是 Go 的回归对照项**：Go `responses.go` 的工具 switch 同样只认 function/custom/namespace，本身也丢 local_shell。它是 parity，不是回归。
- 真正会在 Rust 切换后直接打断核心流程的有四类：**zstd 请求体不解码**（Codex）、**Claude Code 直通 DeepSeek 的 thinking 回放缺失**（Claude Code）、**无 Origin 防护**（安全）、**Codex 订阅出站未 zstd、未走 WS**（大会话 503 / 120s 断流）。

标注约定：**[双侧]** = 我读了 Rust 和 Go 两侧代码；**[Rust 实读]** = 只读了 Rust 侧，缺失由 grep 确认；**[推断]** = 客户端行为来自 Go 注释或经验，未抓到真实请求。

## 1. 各 agent 接入方式（用于判断影响面）

| agent | 入站方言 | 指向 | 依据 |
|---|---|---|---|
| Claude Code | anthropic `/v1/messages`（`/v1/messages/count_tokens`） | `ANTHROPIC_BASE_URL=http://127.0.0.1:<port>`（引擎根） | acp.rs:998-1010；model_bridge.rs ClaudeEnv |
| Codex CLI | responses `/v1/responses` | `model_providers.custom-gateway.base_url`=引擎 `/v1`，CODEX_CONFIG 注入 `model_context_window` | model_bridge.rs:2963-3005；acp.rs:911-951 [推断 wire_api=responses，未抓包] |
| opencode | chat `/v1/chat/completions` | `@ai-sdk/openai-compatible`，baseURL=引擎 `/v1` | model_bridge.rs:545-546, 555 |
| pi | chat | `api: openai-completions`，baseUrl=引擎 `/v1` | model_bridge.rs:684-685 |
| zcode | chat | `type: openai-chat-completions`（个人层 provider） | model_bridge.rs:845-847 |
| DSH | chat | `openai-completions`（cordis.patch.yml 路由） | model_bridge.rs / dsh_bridge（AGENTS 跨用桥节） |
| Grok CLI | chat（`api_backend="chat_completions"`） | config 表 base_url=引擎 `/v1` | model_bridge.rs:2150, 2370, 2591-2602 |

## 2. 缺口总表（按优先级排序）

列：缺口 / 方言 / 影响的 agent / 用户可见症状 / Go 证据 / 难度 / 优先级 / 核实方式

| # | 缺口 | 方言 | 影响 agent | 用户可见症状 | Go 证据（证明 Go 做了） | 难度 | 优先级 | 核实 |
|---|---|---|---|---|---|---|---|---|
| 1 | **入站 `Content-Encoding: zstd/gzip` 不解码**。Rust 全 crate 无 zstd/gzip 入站分支（ruzstd 只用于 backfill），`asked_model`/`from_slice` 直接解析压缩字节失败。 | responses、chat、anthropic 全部 | Codex（Go #1223 注释称 Codex 对自定义 provider 的 `/v1/responses` 也发 zstd） | responses：`400 invalid request: model`（responses.rs:58-66, 106-109）；chat：`404 no provider for model:`（chat.rs:368-374）；切换后 Codex 整轮不可用 | request_bounds.go:150-175 `decodedBody` 支持 zstd/gzip/identity，其余 400；gateway 入站路径统一经它 | S（ruzstd 已在 Cargo，gzip 需确认依赖） | **P0** | [双侧] Rust 无解码 grep 为空；Go 读了解码函数。Codex 是否对自定义 provider 发 zstd = [推断]，以 Go #1223 为准，需抓一次真实请求确认 |
| 2 | **anthropic 直通路径不做 DeepSeek thinking 修补**。直通（`forward_direct`）原样转发，没有 `withThinkingOnToolTurns`（tool_use 轮补 thinking 占位）、`redacted_thinking` 清洗、`withoutThinkingOff`。DeepSeek 预设有 anthropic_base（model_bridge.rs:92），所以 Claude Code 走 DeepSeek 时必走直通。 | anthropic | Claude Code（路由组 Claude→DeepSeek 回退，或直接选 DeepSeek 模型） | 第二轮工具调用 400：`The content[].thinking in the thinking mode must be passed back`；redacted_thinking 报 422 | anthropic.go:398-432（`withThinkingOnToolTurns`，注释给出 2026-10-11/12 实测日期与 #1611/#1822）；anthropic.go:540-552 `withoutThinkingOff`；anthropicReplaysThinking 排除 api.anthropic.com | M | **P0** | [双侧] messages.rs:135-181 直通无改写 + translate 无 thinking 处理；Go 读了函数体 |
| 3 | **anthropic 入站丢弃 `thinking` 块**（无签名与有签名都丢）。翻译路径（如 DeepSeek 只剩 chat 基址、或 OpenRouter 等纯 chat 厂商）回放不了 `reasoning_content`。 | anthropic（翻译路径） | Claude Code（无 anthropic_base 的厂商）、任何 thinking 模式的 chat 厂商 | 第二轮 400（reasoning_content 必须回传）；或 thinking 上下文断裂 | anthropic.go:251-253（保留 Thinking + Signature）；chat.rs:239-277 的回放只从 `PartKind::Thinking` 取，而 Rust `Thinking` 只有 responses 的 reasoning item 会产生 | M | **P1** | [双侧] translate.rs:506-508 明确注释「本矩阵丢弃」；chat.rs 回放逻辑读了 |
| 4 | **Codex 订阅出站未 zstd、未走 WebSocket**。codex_account.rs 用 ureq 明文发 JSON 到 chatgpt；Go 按 `CodexCompressed` 压缩，并用 WS 规避 ~120s 流断。 | responses（订阅直连） | Codex 订阅账号（大会话） | 大请求 503（Go 注释：1.1MB 明文被 ChatGPT 回 503）；部分网络 120s 处流被切 | gateway.go:6436-6442（codex 出站 `CodexCompressed`）；codex_socket.go:1-30（WS 规避 120s 断流，含实测数字） | M（zstd）/ L（WS） | **P1**（zstd 先做） | [双侧] Rust `zstd` 全 engine_gateway 无命中；Go 读了注释与调用点 |
| 5 | **无 Origin / 外站网页防护**。Rust 路由对 `Origin` 完全不看；任何被用户访问的网页可用 `text/plain` 简单 POST 打 `127.0.0.1:<port>/v1/responses`，**盲打消耗 Codex/Grok 订阅**（Rust 不校验 Content-Type，也不回 CORS 头所以页面读不到回包，但请求照常执行）。 | 全部 | 全部（引擎一个口） | 无感烧配额；安全问题 | cors.go:1-60 注释直述该威胁；cors.go `foreignPage` 对外站 Origin 回 403，`Origin: null` 无 key 回 403；corsGuard 只对列表内 origin 回 CORS 头 | S | **P0（安全，切换前必修）** | [双侧] routes.rs:46-155 无任何 Origin/Content-Type 判断；grep `origin` 在 engine_gateway 无命中 |
| 6 | **Codex 模型目录端点缺失**。Go 提供 `GET /v1/codex/models`（`{"models":[…]}`，Codex 的 model_catalog_url 形态），并写 `model_catalog_json`；注释明确说 **`/v1/models` 对 Codex 不可读**。Rust 两者都没有；Rust `/v1/models` 只返回 OpenAI 形 `{object,data}`。 | responses（Codex） | Codex | 引擎模型不出现在 Codex 选择器；context_window / reasoning 档位回落 Codex 内置表（与 CODEX_CONFIG 窗口注入相互矛盾） | codex_catalog.go:20-33（`CodexCatalogPath`、注释）；codex_catalog.go:49-63 `codexReadsWholeCatalog` / 1MB 上限 | M | **P1** | [双侧] Go 读；Rust 无路由（routes.rs 无 `/v1/codex/models`，全仓无 `model_catalog`）。Codex 是否从 `/models?client_version` 读 = [推断] |
| 7 | **anthropic 输出 usage 双计 cache**。Rust 把 OpenAI `prompt_tokens`（已含 cached）原样填进 `input_tokens`，同时又填 `cache_read_input_tokens`。Anthropic 语义里 `input_tokens` 不含 cache。Claude Code 的上下文占用 = input + cache_read + cache_creation，会**高估**，导致过早自动压缩。反向（anthropic→chat）`chat_usage` 把 input 当 prompt_tokens 却不加 cache，又会**低估** OpenAI 客户端的 prompt。 | anthropic 输出 / chat 输出 | Claude Code（主要）、OpenAI 系客户端 | Claude Code 上下文条偏高、过早 compact；账单统计偏差 | chat.go:30-46（`Usage` 把 CacheRead/CacheWrite 从 PromptTokens 中分出，Input 为净额；`whole` 判定）；anthropic.go:1495-1530 `aUsage` 四桶 | S | **P1** | [双侧] Rust translate.rs:1618-1628、messages.rs:805-815 读了；Go 读的是 Usage 结构与拆分函数，输出端的减法 = [推断，需用 Go 输出一条 anthropic usage 确认] |
| 8 | **`/v1/messages/count_tokens` 缺失**（Rust 无此路由，落 404）。Go 按厂商计数、失败给粗估。 | anthropic | Claude Code（/context、自动压缩估算） | /context 报错或估算缺失；可能触发重试 | gateway.go:1136（路由）；gateway.go:2058-2075 `countTokens`（注释：厂商计数，否则粗估；失败切下一个 key） | S（粗估 chars/4 即可） | **P1** | [双侧] routes.rs 无该路径（grep count_tokens 全 engine_gateway 无命中） |
| 9 | **响应头全部丢弃**：`Reply::Json` 只有 status+body，没有 `retry-after` 等头；错误映射只认 429/529，`503` 落 `api_error`，不是 `overloaded_error`。 | anthropic、responses、chat | Claude Code、Codex（靠 retry-after 退避） | 429/503 时客户端立即重试、无退避，加剧限流；Claude Code 对 503 的 overloaded 分支走不到 | Go 代码库内 `Retry-After` 出现在 24 个文件（grep 计数，未逐个核对 Go 的写入点）；Go 的 `errorTypeFor` 表（messages.rs:827 注释称同表）。Rust 的 `error_type` 只映射 529 → overloaded_error | S-M | **P1** | [双侧] httpd.rs:51-60（Reply 无头）；messages.rs:827-840（Rust 只有 529） |
| 10 | **`response_format` / `text.format`（JSON schema）在 responses 路径被丢**。`text` 字段在 Rust parse_responses 无读取（grep `text.format` 无命中）；`verbosity` 同样丢。chat 方言是 raw 透传，理论上保留（chat.rs:368-380 直接转发 body；未核 fit_for_vendor 是否剥离）。 | responses（→chat 翻译）；anthropic 无此概念 | Codex（output schema / verbosity）、用 Responses 结构化输出的 SDK | 结构化输出变成自由文本，下游解析失败 | Go responses.go 的 text 处理与 response_format 出现在 11 个文件（[双侧] 读了 responses.go 对应 case；Go 侧 json_schema 映射细节未逐行读） | M | **P2** | [Rust 实读] grep 无命中；chat 透传保留 = [推断] |
| 11 | **`previous_response_id` 被静默忽略**（翻译路径，responses.rs:1086 注释明说「安静丢弃」）。Go 为每个应答保存 input/output，按 id 还原历史。 | responses | 只发增量 input 的 Responses 客户端（Codex 的 HTTP 路径发全量，影响小） | 上下文丢失且无报错 | prev_response.go 文件头：客户端只发新 input + previous_response_id，magpie 保存每个 response 的输入输出 | M | **P2** | [双侧] responses.rs:1086；prev_response.go 头注释 |
| 12 | **入站体上限 64MB**（httpd.rs:19）vs Go **512MB**（request_bounds.go:30）。Codex 整轮截图内联 base64，30MB+ 很常见。超限 → `parse` 返回 None → 400 `malformed request`。 | 全部 | Codex（多图长会话） | 大会话直接 400 malformed | request_bounds.go:23-30 注释（截图内联、zstd、512MB） | S | **P1** | [双侧] httpd.rs:19、MAX_BODY 检查在 parse() |
| 13 | **chat 内容块丢 `input_audio` / `file`**（translate.rs:303-323 仅 text / image_url）；responses 的 `input_file` 同样丢（translate.rs:543-566 仅 input_text/output_text/text/input_image）。 | chat、responses | 发 PDF / 音频附件的客户端（opencode、pi、Codex 附件） | 模型「看不到」文件，且无报错 | responses.go:1148 `input_file` case；Go 的 chat 侧 input_audio/input_file 在 3–4 个文件 | M | **P2** | [双侧] translate.rs 两处 match 读了；Go responses.go:1140-1150 读了 |
| 14 | **`cache_control` 在 anthropic 入站被丢**（Part 结构无字段，parse_anthropic 不读）。Anthropic 模型经 chat 厂商（如 OpenRouter）时失去 prompt caching：费用高、cache_read 恒 0。 | anthropic（翻译路径） | Claude Code 经 OpenRouter 等 chat 厂商 | 成本上升；与 #7 叠加，用量面板失真 | anthropic.go:1023-1129（cache marks 进 IR 并输出）；claude_cache.go（clientCache 持有 marks） | M | **P2** | [双侧] translate.rs 无 cache_control 解析（仅注释行 1414）；Go 读了输出点 |
| 15 | **anthropic 直通缺 `anthropic-beta` 转发**（send_messages 只设 version + key，不带客户端的 beta 头）。Claude Code 依赖的 beta（interleaved-thinking、context-management 等）在直连 Anthropic 时失效。 | anthropic 直通 | Claude Code（直连 Anthropic/第三方 anthropic 基址） | 特定功能（交错思考、上下文编辑）静默关闭 | gateway.go:6287-6288（`anthropic-version`/`anthropic-beta` 同时转发）；anthropic.go `fitUserBetas` | S | **P2** | [双侧] messages.rs:183-207 头设置读了；Go gateway.go 读了 |
| 16 | **`thinking` 的 display / adaptive 修补缺失**。Opus 4.7+ 的 adaptive 思考默认不回显，Go 补 `display:"summarized"`；Rust effort.rs 只映射 `output_config.effort`，不设 display。 | anthropic 直通 | Claude Code（Opus 4.7+） | 思考内容不显示（观感问题，非错误） | anthropic.go:606-625 `adaptiveThinking`（#1485） | S | **P2** | [双侧] effort.rs:199-220 读了 |
| 17 | **anthropic `disable_parallel_tool_use` 被丢**（parse_anthropic tool_choice 只取 type/name，translate.rs:529-538）。 | anthropic（翻译路径） | Claude Code（需要串行工具时） | 模型并行调用，违反客户端约束 | Go anthropic.go 唯一命中 `disable_parallel_tool_use`（[双侧] 读了其读取点） | S | **P2** | [双侧] |
| 18 | **stop_reason 映射不全**：`stop_sequence` 恒 null，`pause_turn`/`compaction` 不识别。翻译路径 `chat_stop_to_anthropic` 把 `stop` 全映射成 `end_turn`，`stop_sequence` 恒 null。 | anthropic 输出 | Claude Code（罕见） | 自定义 stop 序列命中时客户端分不清 | Go anthropic.go / chat.go 的 stop_sequence 处理（5 个文件命中） | S | **P3** | [双侧] messages.rs:794-801 |
| 19 | **anthropic 流式帧不完整**：无 `ping`；`message_start` 的 usage 恒 0（真 usage 只在 message_delta）。 | anthropic 输出（翻译） | Claude Code | 基本可用；SDK 通常容忍，上下文条在首帧为 0 | [推断]，Go 发 ping 的位置 52 个文件命中多为无关 | S | **P3** | [Rust 实读] messages.rs:658-667 |
| 20 | **`/v1/models` 缺 `has_more` / `first_id` / `last_id`，`limit` 被忽略**。 | anthropic（模型列表） | Claude Code 的 gateway 模型发现（[推断]） | 模型选择器不完整（若客户端翻页） | gateway.go:1930-1932 | S | **P2** | [双侧] routes.rs:81 只返回 `{object,data}` |
| 21 | **缺 `GET /v1/models/{id}`**、`GET /v1/responses`（Go 返回 405 + `Allow: POST`，WS 升级返回 426）、`OPTIONS` 预检（Go 对 cors 与 omarchy 有处理）。Rust 一律 404。 | 全部 | 个别客户端探测（[推断]） | 探测失败；浏览器预检失败（与 #5 相关） | gateway.go:1042（`GET /v1/models/{id...}`）、1128-1129（GET /v1/responses）、1123（OPTIONS） | S | **P2**（OPTIONS 随 #5 一起做） | [双侧] routes.rs 无这些分支 |
| 22 | **Responses→仅 anthropic 基址的厂商无出路**：responses.rs:121-168 无 chat 基址时直接 404（`has no endpoint for /v1/responses`）；chat 入站同理（chat.rs:302-307 `outbound_for` 失败 → 404）。`chat_to_anthropic`/`responses` 翻译函数存在但只在测试里用。Go 有完整跨方言出站。 | responses、chat → anthropic-only | 用户自建的 anthropic-only 厂商（预设里的均有 chat 基址，见 model_bridge.rs:92-176） | 选择该厂商时直接 404 | [双侧] translate.rs 的 `chat_to_anthropic` 仅测试引用（grep 结果）；Go 的跨方言出站在 messages/responses 的 target 选择处 | M | **P2** | [双侧] |
| 23 | **sibling：local_shell 工具与 local_shell_call 历史被丢**。`ToolSet::add` 只认 function/custom/namespace（translate.rs:752-800）；parse_responses 的 `local_shell_call` 落到「其余 item 丢弃」（translate.rs:1112-1113）。**Go parity**：responses.go 工具 switch 同样只认 function/custom/namespace，本身也不转 local_shell。 | responses（→chat） | Codex（非 GPT-5 或使用 shell 工具的模型） | 模型失去 shell 工具；历史里的 shell 调用消失 | responses.go:558-575（工具 switch）；responses.go:714-722（item switch 无 local_shell） | M（若要支持：映射为 function `local_shell` 并还原） | **P2**（parity，非回归） | [双侧] |
| 24 | **sibling：web_search 工具与 web_search_call 历史被丢**（翻译路径）。Rust codex_account 有 web_search 补声明（codex_account.rs:946-980），但 chat 厂商路径无对应。 | responses（→chat） | Codex | 搜索能力静默缺失；历史 web_search_call 被丢 | codex_account 同类处理为 Go declareSearch（codex_account.rs 注释引 Go） | M | **P3** | [双侧] |
| 25 | **Expect: 100-continue 不应答**；连接 `Connection: close`、无 keep-alive、thread-per-connection（上限 64）。 | 全部 | 个别慢客户端 | 大请求多等 1s（curl 默认）；高并发 503（超 64） | Go 用 net/http 自动应答 | S | **P3** | [Rust 实读] httpd.rs:24-29、107-121 |
| 26 | **错误形状：Responses 翻译路径错误原样透传 chat 形状**（responses.rs:261-270 注释）。Codex 依据 `error.code`（如 `context_length_exceeded`、`rate_limit_exceeded`）决定是否重试/压缩，透传的是厂商原文，码不一定对。 | responses | Codex | 上下文超限不触发自动压缩 | [推断]；Go writeError 按方言写码（gateway.go 多处 writeError(w, provider.Responses, …)） | M | **P2** | [Rust 实读] responses.rs:261-270 |
| 27 | **Codex 订阅 `/v1/responses` 直通与 Go 的 SSE 聚合**：Rust codex_account 对非流式客户端把 SSE 聚合回对象（注释），Go 对应逻辑在 codex_backend.go。未逐行比对聚合正确性。 | responses | Codex 订阅 | 未知 | — | M | **P3（需专项测）** | [推断] 未深读 |

## 3. 已核对、Rust 覆盖良好（无需改）

- **tool_choice 各形态**：chat（string / `{function:{name}}`）、anthropic（auto/none/any→required/tool）、responses（string、allowed_tools、命名）均在 IR 中统一（translate.rs:428-436、529-538、1136-1174）。
- **parallel_tool_calls（responses）**：读入 `Ir.parallel`，build_chat 带工具时转发（translate.rs:899）。
- **stop 序列**：chat（string/array）与 anthropic（stop_sequences）都读（translate.rs:345-351、455-457）。
- **reasoning effort**：三方言统一经 `effort::requested`（effort.rs:137-170），含 anthropic `output_config.effort` 与 `thinking.budget_tokens` 映射。
- **chunked 请求体**：解码正确，含扩展与尾随头（httpd.rs:221-260）。
- **工具压平与还原**：custom / namespace / tool_search 的表驱动还原（translate.rs:577-830、1916-2183）。这是你在提的回归修复区，本次未重审。
- **chat→chat 透传**：`chat::forward` 原样转发，只改 model 与 token 字段（chat.rs:78-88, 186-203）。
- **流式 include_usage 注入**：responses→chat 与 anthropic→chat 翻译路径都补（responses.rs:270-283；messages.rs:236-247）。
- **stream 结束原因**：responses `incomplete`（length/content_filter）双向映射（responses.rs:453-455；translate.rs:1892-1905）。
- **anthropic 直通的 429/529 错误**：vendor 的 anthropic 形状错误原样透传（messages.rs:843-858）。
- **代理与认证头**：anthropic 出站同时带 `x-api-key` 与 `Bearer`（messages.rs:183-207）——第三方中转两种都认。
- **`[1m]` 与 `[effort]` 标记**：出站前剥离（messages.rs:51-62 注释）。

## 4. 建议优先级（切换 Rust 引擎前的清单）

1. **P0 #5 Origin/外站防护**（安全；S；routes.rs 入口加 Origin 判定，外站 403、`null` 无 key 403，并补 OPTIONS）。
2. **P0 #1 zstd/gzip 入站解码**（S；ruzstd 已在依赖内，复用 request_bounds 的口径，identity/gzip/zstd 之外 400 并写明）。
3. **P0 #2 anthropic 直通的 DeepSeek 修补**（M；移植 `withThinkingOnToolTurns` / redacted_thinking 清洗 / `withoutThinkingOff`，并保留 Claude 自家模型的豁免）。
4. **P1 #3 anthropic thinking 入站保留**（M；把 thinking+signature 变成 `PartKind::Thinking` 带签名，build_chat 回放 reasoning_content）。
5. **P1 #4 Codex 订阅出站 zstd**（M；WS 另议，L）。
6. **P1 #6 Codex 模型目录 `/v1/codex/models`**（M）。
7. **P1 #7 usage 去双计 + #14 cache_control 透传**（S + M，一起做，否则用量面板仍失真）。
8. **P1 #8 count_tokens 粗估**（S）、**#9 Retry-After 与 503→overloaded**（S-M）、**#12 体上限提到 512MB**（S）。
9. P2 其余按表。

## 5. 需要真实抓包才能定论的项（未在本次做，未发任何网络请求）

- Codex 对自定义 provider 的 `/v1/responses` 是否发 zstd（#1）。
- Codex 是否读 `{base}/models?client_version=`（#6）。
- Claude Code 是否在 `ANTHROPIC_BASE_URL` 下调用 `/v1/models` 翻页参数与 `count_tokens`（#8、#20）。
- Go 对 anthropic 输出端 `input_tokens` 的减法（#7，目前只读了 Usage 结构）。

可用的线索：`dev.log` 里的 Go 日志（注意它是 Go 引擎的日志，不是 Rust 的）；`.marionette/transcripts/` 下有 Codex/zcode 会话转录，但不含原始请求体，不能替代抓包。
