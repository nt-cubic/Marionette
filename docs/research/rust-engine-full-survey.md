# Rust 引擎全量对照调查（magpie Go → engine_gateway Rust）

日期：2026-11-18　性质：只读调查，未改任何 `src-tauri/` 代码。
方法：三路并行深读（Go gateway 345 个非测试文件的功能矩阵 / Rust 侧 24 文件快照 /
合同套件+接入面）+ 针对性 grep 验证。方言翻译层的 27 项缺口另见
`rust-engine-dialect-audit.md`（本文不重复，只标注其修复进度快照）。
Rust 快照时间：11:52–11:58，**工作区正被修复进程修改，读到的态在漂移**。

## 0. 一句话结论

Go 引擎的可感知行为骨架是五段：**请求预处理 → 方言翻译+流修补 →
路由（粘性/回退/退避）→ 四条订阅线 → usage 台账**。Rust 侧现状：
第 1、2 段已有且方言细节正被修复进程补齐；**第 3 段只有「首字节前换成员」
（流开始后失败＝半截流）、第 4 段缺 Claude 订阅桥（其余三条在未提交文件里）、
第 5 段空白**。另有工程风险：**全部未提交成果悬在工作区，不在任何分支**。

## 1. 工程状态（先说风险）

- **未提交**：45 个修改文件 + 10 个 untracked（9 个 .rs 新文件 + 1 个测试向量 txt）。
  其中 9 个新文件：`cors.rs`(319 行) / `effort.rs`(318) / `codex_account.rs`(2770) /
  `grok_account.rs`(1751) / `zcode_account.rs`(1293) / `key_balances.rs`(1007) /
  `plan_quotas.rs`(1231) / `quota_kit.rs`(422) / `quota_extra.rs`(25)。
  **master HEAD 与 origin/engine-rust 都只含 15 个老文件**；engine-rust 顶上两个
  未合提交的内容已以 cherry-pick（同标题不同 hash）进 master。
- `Cargo.toml` 未提交改动 = +`flate2`（入站 gzip 解码，修复进程刚加）。
- **本机当前跑的是 Go magpie 0.1.4232**（dev.log 11:36 起，端口 3425）；
  `rust-0.8.2` 只在 11:29 短暂运行过一次。修复进程的说法（「用户先切 Rust 又切回」）属实。
- engine_gateway 目录内 `#[test]` 共 **239 个**（目录口径；Claude 对话里的
  「867 项」是全 crate cargo test 总数，两者不矛盾）。

## 2. 方言审计 27 项的修复进度快照（11:52–11:58 工作区）

仅记录「代码里已见实现」，**未验证正确性**：

| 审计项 | 快照状态 |
|---|---|
| #1 zstd/gzip 入站解码 | ✅ httpd.rs 已实现（gzip=flate2、zstd=ruzstd 多帧、解码后仍受 512MB 上限、不认的编码 415） |
| #5 Origin 防护 | ✅ cors.rs 新文件（外站 403、null 403、tauri:// 放行）；缺口：Go `settings.CORSOrigins` 白名单无对应物（恒空） |
| #12 体上限 | ✅ 已提 512MB（解码后计） |
| #3 anthropic thinking 回放 | ✅ messages.rs：chat delta.reasoning_content → thinking_delta（Claude Code 可见共享模型思考） |
| #7 usage 双计 | ✅ 改为「取最后见到的那一份」，message_start 恒 0 不编数 |
| #9 Retry-After | ◐ httpd 有「上游头透传侧道」（Retry-After/request-id/ratelimit*） |
| 工具压平（custom/namespace/tool_search） | ✅ translate.rs ToolTable 全链（Claude 对话确认修复，867 项测试过） |
| responses reasoning 应答方向 | ✅ 流式 summary 事件族 + 非流式 reasoning item 均已实现（**responses.rs 头注还写着「未覆盖」，文档滞后于代码**） |
| #8 count_tokens 路由 | ❌ routes.rs 无该路由（httpd 错误整形认得路径只为了出 anthropic 形状 404） |
| #6 /v1/codex/models 入站 | ❌ 无路由。注意：codex_account.rs 里的 `/models?client_version=` 是**出站**拉 ChatGPT 列表，方向相反，勿混淆 |
| text.format / prompt_cache_key | ❌ 全引擎无处理（Claude 对话已声明留白） |
| #4 codex 出站压缩/WS | ❌ codex_account.rs 明文 HTTPS POST，无压缩无 WS |

## 3. 「需要的内容」边界（Go 345 文件 → 引擎要什么）

**必须 1:1 的五段骨架**（任一缺失直接影响成功率/正确性/用量/订阅安全）：

1. **请求预处理**：request_bounds（zstd/gzip/上限）+ body_cap 全语义（含
   marionette 补丁）+ stdBase64Body + HEIC→JPEG + webp + body_text（GBK/坏
   UTF-8 修复，防整会话 400 连坐）+ corsGuard。
2. **三协议翻译与流修补**：parse/build IR 族（chat/responses/anthropic）+
   chattidy（SSE 错帧修补）+ continuation（断流续答）+ textcalls（DSML
   `｜tool｜` 伪调用行清理）+ 工具 schema 方言修补族。
3. **路由与账号选择**：routing.go（排序/rest/backoff/forbiddenAlike 两账号
   同词不连坐）+ fallback.go（plan 族/roomy/passing/holdWriter）+ affinity.go
   （会话→账号粘性，落盘重启保持）+ patience + weighted/rpm/burst。
4. **订阅线**：codex_backend（4455 行）+ codex_socket（WS 规避 120s 断流）+
   claude_subscription（7972 行，`claude -p` 子进程桥，OAuth 直连会烧
   Extra Usage）+ grok/zcode 线 + grok_integral（**全协议共用**的工具参数
   整数化：Codex serde 拒浮点=整轮失败）+ grok_namespace/schema + zcode_trial
   （trial 过期只 rest 该条目不连坐 GLM 主力）。
5. **usage 台账**：appendUsage → usage.Append（sqlite），M2 对账门槛 3 的数据源。

**可整组裁掉**（判定依据：多租户/GUI/服务端工具，7 家 agent 流量不经过）：
key_slots/key_models/issue/join/oidc（gateway key 多租户）；search_*/draw*/
browser*/memory_mcp/image_mcp/vision（magpie 自托管服务端工具）；
history/trace/route_events/live*/statusline（GUI 回放）；admin/advisor/lan/
mtls/oneport/handover/remote/restart（管理面与多进程）；JS 插件中间件；
gemini **入站**端点（无人说 gemini 协议；gemini **出站**翻译保留——C6 词表
有 `gemini=` 形态，当前无注册者，暂缓）。

## 4. 27 项之外的新缺口（本次调查的主要增量）

grep 实锤：以下在 Rust 侧**零对应物**（仅 groups.rs 一处字符串字面量命中）。

| # | 缺口 | Go 坐标 | 用户可见影响 | 难度 |
|---|---|---|---|---|
| N1 | **失败转移链：holdWriter 流首扣留**——上游失败时扣住未发的流首、静默换下一候选重发；`passing`（Retry-After/patience 退避）、`roomy`（窗口装不下先剔除）、rest/backoff 休息记账 | fallback.go:2786 holdWriter、:2335 passing、:2470 roomy；routing.go:69 served | Go：一次上游故障 = 换人后仍拿到完整回复。Rust：**流开始后失败 = 半截流直接给 agent**（chat.rs 自有测试 `streaming_failover_only_before_bytes_and_stream_is_final` 承认只保首字节前）。组路由下多成员形同虚设 | L |
| N2 | **affinity 路由粘性**——会话→曾答账号排最前（prompt_cache_key/openCodeSeed 种子）、落盘 affinity.json 重启保持、fallback 后 backToMain 回主 | affinity.go:569 affine、:553 stickKey、:260 flushSticksAt | 同会话在组内成员间跳来跳去 = 厂商缓存全失效：**变慢 + 全价新算**；订阅多账号时额度消耗模式与 Go 不同 | M |
| N3 | **continuation 断流续答**——上游 SSE 中断后用 prefill 续写拼回完整回复（unecho 防回声） | continuation.go:364 again、:378 prefillText、:455 streamTranslated | 上游网络抖动/掐流时，Rust 直接把半截回复收口给 agent；Go 会续答 | L |
| N4 | **chattidy 流修补**——chat SSE 错帧修补：usage 帧修正、空名工具清掉、split chat calls 合并、`withReasoningOnToolTurns`（工具回合补思考占位） | chattidy.go:209 openAIUsage、:253 line、:1357 joinSplitChatCalls、:846 | 厂商流不规整时 agent 解析崩/丢 usage；DeepSeek 工具轮行为漂移 | M |
| N5 | **textcalls DSML 清理**——DeepSeek 写的 `｜tool｜` 伪调用文本行会让 Claude Code 空转 | textcalls*.go 三文件 | Claude Code→DeepSeek 共享时偶发空转 | S |
| N6 | **Claude 订阅桥**——订阅账号必须走 `claude -p` 子进程（OAuth 直连被判三方流量烧 Extra Usage）；会话落盘/重放/工作区管理/额度发现约 20 个配套文件 | claude_subscription.go:910 start、:981 startRun；attempt :5777 转入 | **Rust 侧完全没有 claude 订阅路径**——若用户配了 Claude 订阅账号，切 Rust 后该账号不可用（key 型厂商不受影响） | XL |
| N7 | **grok_integral 工具参数整数化**——`2500.0→2500`，**所有协议出站共用**（Codex serde 拒浮点=整轮失败），不限 grok | grok_integral.go:168 intTidy、:123 wholeEncoder | 任意 chat 厂商返回浮点整数参数时，Codex 整轮失败 | S |
| N8 | **grok_namespace / grok_schema**——namespace 工具名密封恢复、ECMA regex class 转写 | grok_namespace.go:85 nsTidy；grok_schema.go:25 | grok_account.rs 头注自认「Codex namespace 回传不补回」；schema 不转写=上游 400 | M |
| N9 | **zcode_trial 连坐防护**——trial 条目过期只 rest 该条目 | zcode_trial.go:30 | GLM 主力账号被 trial 条目连坐进休息 | S |
| N10 | **请求预处理缺件**：stdBase64Body（URL-safe→std base64）、HEIC→JPEG、webp、body_text（GBK/坏 UTF-8） | base64_input.go、heic.go、webp.go、body_text.go（serve :2489 对每路由生效） | 特殊图片格式静默丢失；GBK 内容整单 400 | S-M |
| N11 | **usage 台账**——每轮 appendUsage 落 sqlite（RouteID/Agent/Provider/Model/Millis/Status/Session…），提前拒答也落 Rejected 行 | lan.go:866 appendUsage → usage/usage.go:325 Append | **M2 门槛 3（真机对账 usage 与 magpie 基线一致）没有数据源**；前端额度/统计长期失真 | M |
| N12 | **prev_response 404 语义**——claude/codex 账号收到不持有的 response id 必须显式 404（prevLost） | prev_response.go:305 prevLost、:313 writePrevNotFound | 账号切换后 Codex 带 prev id 重试会得到误导性错误 | S |
| N13 | **脱敏 fails-closed**——出站体脱敏（redact），失败即拒发 | redact.go、serve :2841 | 安全合规行为缺失（默认关时无操作，保开关语义即可） | S |
| N14 | codex_backend 的订阅耗尽回退（codexFallBack）、sealed 读取约束、held/dropped 恢复 | codex_backend.go:1811、:1746、:420 | Codex 订阅限流时不会转 fallback，直接失败 | M |

> N6（claude 桥）是单体最大件；N1–N4 是「Go 好用、Rust 半残」的体验差主因；
> N7 是小改大害的典型（一行整数化省一整轮失败）。

## 5. 合同套件盲区（为什么「全绿但残废」会重演）

`verify-engine-contract.mts` 12 个 case **全部是非流式、无工具、单轮 happy
path**：流式零断言、工具调用零覆盖、thinking 零覆盖、zstd/Origin/错误形状/
多轮/usage 字段/stop_reason 全零。M2 门槛 1 只要求「套件全绿」——所以方言
审计里 P0×3、P1×7 大多不会被它拦截，N1–N14 更在盲区外（它们连合同面条款
都还没有）。

**修门槛优先于修代码**：C5 需要扩充方言细则条款并配 case——
①流式三协议（SSE 事件序列断言）②工具调用往返（含 custom/namespace）③thinking
回放 ④zstd/gzip 入站 ⑤Origin 防护 ⑥错误形状按方言 ⑦上游中途断流的收口
行为（对 N1/N3 定型）⑧组内成员失败转移（对 N1 定型）。否则每批移植都
「自测全绿」，真机才炸。

## 6. 施工图（等修复进程落地并提交后开工，别并发改同文件）

| 批 | 内容 | 对应缺口 | 备注 |
|---|---|---|---|
| 0 | **提交现状**：修复进程完成后立即 commit 全部工作区（含 9 个悬空 .rs） | — | 防丢，最优先 |
| 1 | 合同面 C5 扩条款 + 套件加 8 类 case | §5 | 先立验收尺，之后每批对着量 |
| 2 | 止血小件：grok_integral 整数化、textcalls 清理、body_text/GBK、stdBase64、zcode_trial | N5/N7/N9/N10 部分 | 无架构改动，纯函数移植 |
| 3 | 流可靠性：chattidy → holdWriter 流首扣留 + rest/backoff → continuation 断流续答 | N1/N3/N4 | 依赖：1 的断流 case 先定型 |
| 4 | 粘性与成本：affinity（含 prompt_cache_key 接线） | N2 | 依赖 store 落盘面 |
| 5 | 订阅线收口：codex 出站压缩（zstd）→ WS → codexFallBack → grok ns/schema | #4/N8/N14 | WS 单独评估 |
| 6 | usage 台账 + 对账 | N11 | M2 门槛 3 |
| 7 | Claude 订阅桥（XL，先做产品决策） | N6 | 见 §7 |
| 8 | HEIC/webp、脱敏、prevLost 404 | N10/N12/N13 | 收尾 |

## 7. 待用户决策

1. **Claude 订阅桥（N6）做不做**：7972 行 + 20 个配套文件，是最大单体。
   若这台机器从不用 Claude 订阅账号（只用 key 型厂商共享模型），可以不做、
   在目录/额度面诚实标注「Rust 引擎不支持 Claude 订阅」。
2. **Codex WS（#4 后半）**：Go 注释记了 1.1MB 明文 503 + 120s 断流两个实锤；
   zstd 压缩先做，WS 是否值得 L 难度单独定。
3. **提交节奏**：建议修复进程每完成一个可编译的可验证块就 commit，
   别再攒 45 个文件。
4. **T13 真实 key 对账**（plan-engine-rust.md 未勾选项）排期——它是
   「Rust 能不能转正」的最终裁判，且依赖 N11（usage 台账）先行。
