# 引擎合同（engine contract）——Marionette ↔ magpie 种子

状态：**M1 落地（2026-11-17）**。验证套件：`scripts/verify-engine-contract.mts`。
上游：https://github.com/yetone/magpie（MIT，本地研究克隆约定见 `tools/README.md`）。

## 为什么需要这份文档

Marionette 的单 exe 内嵌 magpie 引擎种子（`third_party/magpie`，MIT）。上游以
~140 tags/天 的速度演进，而 Marionette 与引擎之间**只有一张窄而稳定的合同面**。
这张合同过去全靠真机事故教训堆出来（每条都实锤过静默降级），散落在 AGENTS.md；
本文档把它集中成：

1. **导入门禁**——每次 `import-magpie.ps1` 拉新上游后跑合同套件，绿灯才准编译发布；
2. **自动移植管线的分级依据**——`scripts/upstream-watch.mts` 按「路径 → 合同区」
   映射给上游 diff 分级，产出移植简报；
3. **M2 Rust 重写的验收合同**——同一套测试将来直接打在 Rust 引擎二进制上。

## 合同面枚举

### C1 进程与生命周期

| 项 | 内容 | 消费点 |
|---|---|---|
| 拉起 | `magpie serve`，长跑单进程 | engine.rs spawn |
| 地址 | env `MAGPIE_ADDR=127.0.0.1:<port>`（端口每次启动漂移，壳永远现查不缓存） | engine.rs:857 |
| 统计 | env `MAGPIE_NO_STATS=1`（上游遥测禁用；**种子已整体裁掉 `stats.Run`**，2026-11-17，连同唯一被它拖进来的 `internal/agent` 树） | engine.rs:858/2242/2300 |
| 账号 | 刻意**不设** `MAGPIE_NO_ACCOUNTS`（订阅额度 discovery 要开） | engine.rs:859 |
| 内存 | `GOMEMLIMIT=128MiB`（壳注入，用户自设不覆盖） | engine.rs:868 |
| CLI 超时 | 10s（`output_with_kill_timeout`，超时杀子进程） | engine.rs:2215 |
| 崩溃安全 | magpie 全部 tmp+rename 原子写 → `TerminateProcess` 直杀安全 | engine.rs kill_engine 注释 |
| 落盘 | `appdir`：`~/.config/magpie`（或 exe 旁有 `.portable` 文件 = 便携模式，状态 beside-exe）；`XDG_CONFIG_HOME`/`XDG_CACHE_HOME` 等可重定向（测试沙箱依赖） | internal/appdir |

### C2 `GET /v1/models`（模型目录——跨用宣告的唯一事实源）

响应 `{"data":[…]}`（兼容裸 `{"models":[…]}`）。Marionette 逐行读（`model_bridge.rs:1525`）：

| 字段 | 语义 | 下游 |
|---|---|---|
| `id` | 非空、`provider/model` 形态、不重复 | 全部 |
| `context_window`（同义 `context_length` / `max_input_tokens`，取第一个正值） | 上下文窗口 | opencode `limit.context`（**必须与 output 成对**，缺 output 补 32000）/ pi `contextWindow` / dsh `cordis.patch.yml contextWindow` / zcode `providerModelRules.contextWindow`（schema 要正数）/ codex `CODEX_CONFIG.model_context_window`。**漏写 = 各 agent 按自己的默认窗口办事（静默降级实锤：1M 被显示成 256K、提前压缩）** |
| `max_output_tokens`（同义 `max_tokens`） | 回复上限 | 同上（opencode `limit.output`） |
| `modalities.input` 含 `"image"` | 图片输入能力 | dsh `input: [text, image]`（漏写 = 附图被换成占位文本，2026-10-21 实锤） |

红线：**窗口/上限为 0 = 不知道**（引擎不写、壳不编数）；`catalog_direct_entries()`
是唯一过滤点（`auto/` 与 `group/` 行不进任何菜单）；目录缓存
`~/.marionette/engine-catalog.json`（camelCase `contextWindow`/`maxOutput`/`images`）。

### C3 `GET /v1/magpie/quotas[?kept=1]`（订阅额度）

- 形状：`{object:"list",data:[…]}` 或裸数组（CLI 兜底是裸数组）；坏形状 → 空表不 panic。
- 行：`provider`、`name`、`user?`、`windows[]`（空/缺失的条目跳过）、`balance?`（key 余额）。
- 参数：无参 = force（现场问供应商），`kept=1` = 读网关缓存。
- HTTP 失败才退 `magpie quota --json`（种子**没有**这个 CLI——种子态 HTTP-only 是既定语义）。
- stale 语义（2026-11-13 批）：两侧都失败时返回旧快照并如实标注，不冒充实时。

### C4 `GET /v1/models/group/<id>`（组宣告探针）

200 = 该用户自建组被网关 serve；404 = 不认。**红线（2026-11-12 实锤）**：
`plugins/groups` 不在种子插件集 → 用户组全部 404 → `magpie group add` 照样写盘报成功，
codex 的裸内建名落回内建 codex 源头**烧订阅**。种子插件集 = `groups`/`gateway`/`routing`/`usage` 四个，一个都不能少。

### C5 对话代理（`/v1/chat/completions`、`/v1/responses`、anthropic `/v1/messages`）

- 按 `provider/model`（及组别名）路由到来源协议。
- **字节上限自适配（body_cap）**：来源对超限体的拒答词要能认出来——上游认「too large」类，
  Marionette 补丁 `marionette_body_cap.go` 额外认 ollama.com 的
  `400 + "failed to read request body"`（body ≥ 8 MiB 才算，防小请求连接误判）。
  认出后走自适配：cap = 本次体量的 3/4 → 丢**最老**的图重发 → 无图可丢才 413（带尺寸语义，
  让 codex 能读懂方向而不是当传输错误原样重发）。
- 已知例外：连续同角色消息被合并后图全在最新一条用户消息 → 无更老图可丢 → 仍 413（诚实失败）。

#### C5.1 方言细则（2026-11-18 批 1 增补——Go 网关真实行为，套件 D1–D8 锁定）

下表每条都以 Go 种子实测为准（`verify-engine-contract.mts` 的 D 系列 case）；
Rust 引擎 2026-11-18 批 1–3 后 D1–D10 零挂起全绿（`CONTRACT_PENDING` 机制保留给后续批次使用）。

| # | 细则 | Go 实测行为 |
|---|---|---|
| D1 | **三协议流式收尾**：chat SSE 以 `data: [DONE]` 收尾（尾部空 delta 帧可被折叠，finish_reason 不保证独立成帧，usage 帧与 [DONE] 是硬合同）；responses SSE **没有 [DONE]**，`response.completed` 必须是最后一个事件；anthropic SSE 帧族 `message_start → content_block_* → message_delta → message_stop` 齐全，**finish 帧缺失的怪流也要合成 message_delta**（end_turn + 已见 usage），只发裸 message_stop 是违约 |
| D2 | **工具调用往返**：chat 直通下 `tool_calls` 原样保留（id/name/arguments 字符串不重编）；后续 `role:"tool"` 消息与工具定义必须完整到达出站体 |
| D3 | **thinking 回放**：chat 厂商的 `reasoning_content`（整块或 delta）必须翻成 anthropic `thinking` 块 / `thinking_delta` 帧（Claude Code 可见共享模型思考） |
| D4 | **入站压缩解码**：`Content-Encoding: gzip` 与 `zstd` 解码后路由；identity/gzip/zstd 之外的编码拒绝（Go 是 400） |
| D5 | **Origin 防护**：外站 `Origin: https://…` 与 `Origin: null` 一律 403；agent CLI 无 Origin 放行 |
| D6 | **错误保真**：厂商 429 的状态码与 `Retry-After` 头透传到 agent；`/v1/messages` 的厂商错误体必须是 anthropic 形状（`{"type":"error",…}`），不透传 chat 原文 |
| D7 | **断流底线**（2026-11-18 批 3 起双引擎绿）：上游 SSE 中途断开，网关欠 agent 一个「可判定的收尾」——续答到 [DONE]、显式 error 事件、或非 200 的可判定失败（ureq chunked 断连不吐已收 chunk，与 Go http 客户端的真实行为差异——502 同样满足底线）；200 裸 EOF 违约 |
| D8 | **组内失败转移**：组 pick 的成员 5xx/不可达时，同轮内换下一个成员重发，agent 拿到 200 而不是失败 |
| D9 | **工具参数零分数（翻译路径）**：厂商在工具调用里写 `"yield_ms":2500.0`，翻译到 responses/messages 方言时必须是 `2500`（Codex serde 拒浮点整单，Go grok_integral.go #1431/#3152） |
| D10 | **工具参数零分数（chat 直通）**（2026-11-18 批 3 起双引擎绿）：同上，作用于 chat→chat 直通流的 tool_calls 参数改写（chattidy.rs 的跨行状态机） |

### C6 CLI 动词（`run_magpie_cli`，10s 超时，stderr 参与判错）

| 动词 | 语义要点 |
|---|---|
| `provider add <preset\|name> [key] k=v…` | k = id/name/url(chat/openai)/responses/anthropic/gemini/key/models/catalog/region|plan/header.*；provider id 必须过 slug（小写字母数字横线，`provider_id_slug`）；带 preset 时 region= 在其 Regions 里挑 |
| `provider set <id> k=v…` / `provider key <id> <key>` | 改写既有来源 |
| `provider rm <id>` | 不存在时 stderr 含 `no provider` → Marionette 视为「已回收」成功 |
| `provider refresh <id>` 然后 `provider models <id> all` | **顺序不可反**（`all` 暴露的是最近一次抓取的清单）；`SetModels(id,nil)` = 解钉 → live 语义 |
| `provider models <id> <ids…>` | 钉死清单 |
| `models` | 列出 `provider/model`（含 tag） |
| `group add <name> models=<m>[,m…] [id=] [pick=]` | 种子语义 = `SaveGroup` 按 id **upsert**（stock 是已存在报错，Marionette 留有 `group_already_exists` 兼容路径）；pick 必须在 members；Routing=Manual |
| `group rm <id>` | |
| `quota --json` | 仅 CLI 兜底路径；种子无此动词（C3 的 HTTP-only 依赖这一点） |

### C7 Marionette 种子补丁（import-magpie.ps1 重放，锚点丢失 = 响亮 throw）

| 补丁 | 锚点 | 丢失后果 |
|---|---|---|
| `main_seed.go` + `main.go` 加 `!marionette_seed` guard | main.go 无 guard 则补 | 种子失效（编译出全量 magpie，载荷暴涨） |
| `marionette_peel.go`（effort peel） | `strings.TrimSuffix(strings.TrimSpace(id), "[1m]")` 调用点 | Codex `model[xhigh]` 别名 404 |
| `marionette_body_cap.go` | `page := !byteCapRefusal(res.StatusCode, b)`（body_cap.go，两处决策点同形替换） | ollama 超限 → 会话死锁（2026-11-12 实锤） |

种子构建：`go build -tags "nogui marionette_seed" -trimpath -ldflags "-s -w"`；
插件集四个（C4）；`stats.Run` 已裁（2026-11-17，-5.5MB raw / -1.45MB packed）。

## 验证矩阵（scripts/verify-engine-contract.mts）

| 测试 | 覆盖 |
|---|---|
| `provider add → models` | C6 登记 + C2 id 形态 |
| `/v1/models` 行字段 | C2 全字段（context_window / max_output_tokens / modalities.input） |
| `group add → /v1/models/group/<id> 200 → group rm → 404` | C4 组宣告生死 |
| `/v1/chat/completions` 基础路由 | C5 转发 |
| 超限体 → 丢最老图重发 200 | C5 + C7 body-cap 补丁（会话死锁回归） |
| `/v1/magpie/quotas` 形状 | C3 |
| `provider rm` 不存在 → `no provider` | C6 回收语义 |
| `provider refresh → models all` | C6 顺序 + live 语义 |
| `/v1/responses` 翻译（responses 形 → 200 + output 文本，T4） | C5 |
| `/v1/messages` 翻译（anthropic 形 → 200 + content 文本，T3） | C5 |
| D1 三协议流式（帧族 + 终止符断言） | C5.1 |
| D2 工具调用往返（tool_calls 保留 + tool 消息出站） | C5.1 |
| D3 thinking 回放（thinking 块 / thinking_delta 帧） | C5.1 |
| D4 入站 gzip+zstd 解码（手工 zstd raw frame）+ 未知编码拒绝 | C5.1 |
| D5 Origin 防护（外站/null 403、CLI 放行） | C5.1 |
| D6 错误保真（429 + Retry-After + anthropic 形状错误体） | C5.1 |
| D7 断流底线（cut 流必须可判定收尾） | C5.1 |
| D8 组内失败转移（坏成员同轮换好成员） | C5.1 |
| D9 工具参数零分数——翻译路径（2500.0 → 2500） | C5.1 |
| D10 工具参数零分数——chat 直通 | C5.1 |

沙箱：exe 拷进临时目录 + `.portable` 文件（beside-exe 落盘，同
`verify-model-source-engine.mjs` 模式），env `MAGPIE_NO_ACCOUNTS=1 MAGPIE_NO_STATS=1
DO_NOT_TRACK=1 NO_COLOR=1`；绝不碰真实 `~/.config/magpie` 或 `~/.marionette`。

## 路径 → 合同区映射（upstream-watch digest 分级用）

| 上游路径 | 合同区 | 分级 |
|---|---|---|
| `internal/gateway/**` | C2/C3/C5 | 高（含 body_cap.go：补丁锚点风险） |
| `internal/provider/**` | C6/C4（GroupFor peel） | 高（peel 调用点锚点风险） |
| `plugins/groups|gateway|routing|usage/**` | C4/C3/C2 | 高 |
| `internal/catalog/**` `internal/codexcat/**` | C2（预设/目录数据） | 中 |
| `internal/sqlitedb/**` `internal/sessions/**` | 存储（喂 C2/C3） | 中 |
| `internal/appdir/**` `internal/settings/**` `internal/pluginrt/**` | C1 | 中 |
| `main.go` `internal/cli/**` | C7（guard/启动序列） | 中（编译错会响亮） |
| `go.mod` `go.sum` `third_party/` | 构建依赖（本地 replace 清单） | 中 |
| 其余（`internal/agent`、site、tools…） | 种子不链接 | 低 |

## M2（Rust 引擎重写）验收门槛

1. 本合同全部测试对 Rust 二进制绿灯（同一套 `verify-engine-contract.mts`，换 exe 参数）；
2. C7 的三个补丁语义在 Rust 侧有对应实现并有测试；
3. 真机对账：usage 台账 / 额度卡 / 跨用 6 家 agent 宣告与 magpie 基线一致；
4. `engine-rust` 已于 2026-11-19 转正为默认；`MARIONETTE_ENGINE=magpie` 保留一个发布周期的回退，期间继续跑双路径合同。
