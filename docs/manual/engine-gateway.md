# 引擎网关（engine_gateway）开发手册

一句话：**magpie 子进程的进程内 Rust 替身**——`src-tauri/src/engine_gateway/`（36 模块，
~40k 行）在 `engine-rust` feature 下编译进 marionette.exe，serve = 起一个线程。
rust SKU 5.36MB（magpie SKU 24.95MB）。目标：magpie 子进程彻底退役。

- 账本（任务史+红线）：`docs/plans/plan-engine-rust.md`
- 合同面（唯一验收尺）：`docs/plans/plan-engine-contract.md`
- Go 真相源：`third_party/magpie/internal/gateway/`（只读，逐条对照文件:行号移植）

## 文件地图（改 X 找 Y）

### 端点与路由
| 想改什么 | 去哪 | 备注 |
|---|---|---|
| HTTP 服务器本体（chunked/超时/并发上限） | `httpd.rs` | loopback-only，std 线程，64MB 体上限 |
| 路由分发（C2/C3/C4/C5 面） | `routes.rs` | 对话臂的 bodyprep/intercept 挂点都在这 |
| chat 直通 + 候选循环/休息表 | `chat.rs` | rest 表 provider 级 + `provider::model` 模型级；流 relay 链在这 |
| responses 端点（codex 母语） | `responses.rs` | 直通优先 + 翻译路径复用 chat 全链 |
| anthropic messages 端点 | `messages.rs` | 含 count_tokens；Claude 桥只挂这 |
| usage 台账 + 流捕获 | `usage_ledger.rs` | TapReader 包 relay 链最外层、流尾收口；三方言桶口径 1:1 |

### 协议翻译
| 想改什么 | 去哪 | 备注 |
|---|---|---|
| 三协议互译（chat↔responses↔anthropic） | `translate.rs` | IR 全六向 + SSE 双向映射 + peel 七档 |
| 整数化（工具参数零分数丢弃） | `integral.rs`（翻译路径）+ `chattidy.rs`（直通状态机） | **两层都要过**合同 D9/D10 |
| 文本写调用（text calls） | `textcalls.rs` | responses/messages 共用件在 chat.rs 尾部 |
| previous_response_id 回放 | `prev_response.rs` | 纯内存 LRU；fresh_id 形状 = Go newID |
| 应答修补/规整 | `chattidy.rs` | 直通跨行状态机；finish_reason 规整 |

### 体处理（读体后、路由前）
| 想改什么 | 去哪 | 备注 |
|---|---|---|
| 预处理链（textBody→stdBase64→webp→redact） | `bodyprep.rs` | `try_prepared_request` 是 routes 四臂现行形状（Err→503） |
| 字节上限学 cap/丢图重发 | `bodycap.rs` | 413/ollama 拒答词表；prefit capNear 90% 线 |
| 出站脱敏（默认关） | `redact.rs` | `MARIONETTE_REDACT=1`；Go internal/redact 词表 1:1；HEIC 挂起（无纯 Rust HEVC 解码器+禁新依赖） |

### 组路由与故障换人
| 想改什么 | 去哪 | 备注 |
|---|---|---|
| 组解析（group/裸名/provider/model） | `groups.rs` | peel/嵌套一层/防环 |
| 会话粘性 | `affinity.rs` | stick_key 提取序 = usage session 字段同源 |
| 模型目录（C2 转述） | `catalog.rs` | context_window/modalities 是跨用宣告唯一事实源 |
| models.dev 兜底 | `presets.rs` | 缺窗口兜底，不编数 |
| 内置厂商 preset 端点表 | `vendor_presets.rs` | 端点逐条照抄上游 presets.go |
| effort 档位 | `effort.rs` | 七档词表与 translate 统一 |

### 订阅账号桥（四件套）
| 账号 | 去哪 | 出站形态 |
|---|---|---|
| grok | `grok_account.rs` | chat 直通 + cordis.patch |
| codex | `codex_account.rs` | responses 直通/翻译 + WS（`MARIONETTE_CODEX_WS=1`） |
| claude | `claude_account.rs` | **CLI 子进程桥**，只挂 anthropic 入站；驻留进程池 + 进程内环回 HTTP MCP helper |
| zcode | `zcode_account.rs` | 额度只读 + trial 拒答词表（`trial_refused_for` 四道门） |

配额与余额：`quotas.rs`（DeepSeek 多币种）、`key_balances.rs`（六家来源）、
`plan_quotas.rs`、`quota_extra.rs`（并行 collect）、`quota_kit.rs`（共用件）。

### 连接与出站
| 想改什么 | 去哪 | 备注 |
|---|---|---|
| WebSocket 最小客户端 | `ws.rs` | RFC6455 手写（SHA1/掩码帧）；keep 池语义注释在 codex_account.rs |
| 出站代理 | `mod.rs` 的 `outbound_proxy` | HTTP CONNECT 隧道；SOCKS 两路都没开 |

### 杂项
`cli.rs`（provider/group 动词直调，GUI 零 println）、`cors.rs`、`store.rs`
（providers.json 逐字段互通）、`integral.rs`、`usage_ledger.rs`、`codex_catalog.rs`。

## 红线（绝不能做）

- **不加新依赖**（尤其 tokio/hyper/axum/reqwest）；TLS 保持 ureq+native-tls（ring-less）。
- **engine-rust 与 embedded-engine 互斥**：engine.rs 顶部 compile_error! 断言；
  Cargo features 表达不了互斥，别用 implies 绕。
- **providers.json 与 magpie 逐字段互通**：未知字段 serde(flatten) 原样保留、
  每请求 reload（CLI 写盘即时可见）。别改存储格式。
- **合同套件是唯一验收尺**：改任何端点行为先过
  `node scripts/verify-engine-contract.mts <marionette-engine.exe>`。
- **GUI 进程零 println**：windows 子系统写 stdout 会 panic；cli 动词走 `run()` 返 Err。
- **沙箱测试锁只有一把**：`mod.rs::TEST_SANDBOX_LOCK`——动 `XDG_CONFIG_HOME` 的
  测试必须拿它（私锁互踩实锤过）。
- **Claude 桥**：绝不写/刷新凭据、绝不直连 api.anthropic.com（桥的全部意义）、
  只挂 anthropic 入站。
- **401 永不换账号**（各家自己续登录——codex 实锤语义，别当通用重试）。
- **锁内禁网络 IO / 子进程等待**（chat.rs rests()、keep 池、HelperIo 全是此纪律）。
- **serde_json preserve_order 坑**：依赖图传递点亮时 Map=IndexMap 且
  `remove`=swap_remove——剥键后键序漂移、摘要不稳定。做内容摘要用
  codex_account.rs 的 `sorted_value` 递归键排序。
- **记账不编数**：agent/厂商不报的桶就是 None/0；估算按实测封顶。
- **上游跟进 = 研究克隆 + digest**（`../magpie-upstream` + `scripts/upstream-watch.mts`），
  不是合并代码。

## 坑（踩过的，别再踩）

- **council `same_model_two_members_distinct_calls` 是高失败率 flake**（HEAD 对照
  实验 4/5 也败，30s 等待超时）——与引擎改动无关，独跑不绿属正常，别为它回滚引擎批。
- **wry/tao 补丁**：`vendor/wry`（文字拖放 + TrySuspend + WebView2 参数）与
  `vendor/tao`（paint assert）两条 patch 行，升级前对着 MARIONETTE-PATCH.md 核对。
- **MSVC 环境要用 `cmd /c check.bat …`**——裸 cargo check 撞 `cl.exe not found`。
- **zcode trial 的词表门**（2026-11-18）：`zcodeTrialBlocked`（405 拦截页）/
  `zcodeTrialRestricted`（风控 3012）不在 `trial_refused_for` 词表里，别混。
- **mock 假厂商 fixture 必须显式设 statusCode**（Node 默认 200 会把 400 变 200）。
- **`_meta` 里的 `claudecode/toolUseId` 整键带斜杠**——JSON pointer 会拆错路径，
  必须 `get("_meta").get("claudecode/toolUseId")`（claude 桥回填实锤）。

## 常见改动检查单

### 加一个订阅账号桥
见 [howto/add-new-subscription-account.md](howto/add-new-subscription-account.md)。

### 加一个拒答词表（rest 分级）
1. `<vendor>_account.rs` 写纯词表函数（1:1 搬 Go 词表 + 测试向量）；
2. 四道门包装（来源/候选模型形态/status/点名核对 `eq_ignore_ascii_case`）；
3. `chat.rs` 候选循环 `Attempt::Retry` 分支挂 `rest_entry(provider, model)`——
   判不中**回落原行为**（429/5xx 整 provider、400 不 rest），别动别家语义。

### 加一个 bodyprep 处理步骤
1. 按 Go 对应文件写纯函数（Cow 零拷贝风格，字节级预检闸门学 `may_hold_webp`）；
2. 接进 `try_prepare_with` 的链（顺序见模块头——textBody→stdBase64→webp→redact）；
3. 关态必须零行为变化（字节相等 + ptr 相等测试，学 `redact_off_state_is_byte_identical_zero_copy`）。

### 改 tool_calls 应答形状
`integral.rs`（翻译路径 whole_args）与 `chattidy.rs`（直通状态机）**两层都改**，
合同 D9/D10 断言都要绿。

### 加一个 CLI 动词
`cli.rs` 加函数（`run()` 返 Err，不打印）→ `mod.rs` 若需要 → 测试进 `cli.rs` 尾部。

## 验证（改完必跑）

```bash
cd /d/Project/marionette-rust
cmd /c "check.bat check --features engine-rust"        # 编译绿（MSVC 环境）
cmd /c "check.bat check"                               # 默认构建零变化
cmd /c "check.bat test --features engine-rust --bin marionette"   # 1139 项基线（council 1 flake 除外）
cmd /c "check.bat build --bin marionette-engine"
node scripts/verify-engine-contract.mts src-tauri/target/debug/marionette-engine.exe  # C1–C6 全绿
```

rust SKU 发布：`set MARIONETTE_ENGINE=rust && tools\build-portable.bat`
（产物 `release\<版本>\Marionette.exe`，体积对照 5.36MB 量级）。
真机 e2e：`scripts/verify-rust-sku-e2e.mjs`（隔离 profile + CDP 9223，
**别混 --lang**——组合参数下调试口不监听）。

## 已知差距与源流

- **剩余队列**：continuation 断流续答（chat.rs 尾清单）、Claude 桥 gap 1
  （savedSession/replay/branch 家族）与 4–14 细项、WS 的 permessage-deflate/
  分片延续帧/SOCKS、redact 的 crypto.go/more.go 附加规则与用户词表、HEIC
  （红线卡住，见 bodyprep 头注）。
- **待用户**：T13 真 key 对账（真实来源各一轮，比 usage 四桶与额度卡）、
  T14 退役决策（默认 SKU 翻转）。
- 各模块差距清单在**各自头注**（最重的是 claude_account.rs 14 条）。
- 本手册蒸馏自：AGENTS.md「上游移植管线/引擎方言与子系统批次/引擎 Rust 化」段 +
  `docs/plans/plan-engine-rust.md` 全账本 + 收尾/收尾2 批（2026-11-18）实作。
