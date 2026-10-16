# 跨 agent 模型桥（Cross-Agent Model Bridge）— 程序设计文档

> 状态：**设计稿**（2026-10-16，未开工）。实现落地后把现行行为并入 `CURRENT.md`，本稿挪入 `archive/`。
> 背景：对 [yetone/magpie](https://github.com/yetone/magpie)（MIT，Go，本地网关 + 订阅聚合器）的调研结论 + 产品讨论。
> 调研事实（端点、凭证格式、上游行为）集中在附录 A，动手前不用重新调研。

## 0. 一句话

让「任何一个来源的模型 / key / 订阅」能被「任何一个 agent」使用：**key 型直连（零开销），订阅型过内嵌翻译引擎（按需 ~48MB）**。用户唯一的操作是在模型面板里选一个模型。

## 1. 目标与非目标

### 目标

1. 自动发现本机 opencode / Claude Code / Codex / Grok Build 的 key 与订阅（**只读扫描**，可关）。
2. key 型 plan（OpenCode Go、GLM、DeepSeek、Kimi、OpenRouter、xAI…）跨 agent **直连**，不过网关。
3. 订阅型（Grok / ChatGPT / Claude / Copilot 登录态）经**内嵌 magpie 引擎**跨用。
4. 分发形态不变：仍是一个便携 exe；所有新文件只落在 `~/.marionette` 下。
5. per-spawn 注入 / owned-section，**不改写用户全局配置的既有内容**，终端裸用各 CLI 不受影响。

### 非目标（明确不做）

- 自研协议翻译网关或订阅桥（等于重写 magpie，且违反 CURRENT.md「不做模型路由器」底线——引擎是外挂实现细节，不是自研）。
- stash / drift / 全局配置还原机制（因为我们不抢别人的键，天然不需要）。
- OAuth 订阅 token 的原生跨用：Claude 订阅只能走引擎的真二进制桥，Grok 订阅只能走引擎的私有端点适配。
- 云端聚合（OpenRouter 式）、多机同步、MCP marketplace。

## 2. 术语与来源分类

| 来源类型（`kind`） | 定义 | 例子 | 跨用方式 |
|---|---|---|---|
| `key-plan` | API key 型，厂商同时提供 OpenAI / Anthropic 兼容端点 | OpenCode Go、GLM、DeepSeek、Kimi、OpenRouter、xAI | **L1 直连**（per-spawn 注入） |
| `subscription` | OAuth / 登录态，上游是私有端点 | Grok 订阅、ChatGPT、Claude、Copilot | **L2 引擎**（本地网关翻译） |
| `reference` | 住在别的 agent 自己文件里的凭证，不复制第二份 | opencode auth.json 里的条目 | 用时现场读原文件 |

## 3. 总体架构

```
composer 模型面板（统一入口）
│  本 agent 原生模型（ACP advertise，现状不动）
│  「跨用」分组（Marionette 侧目录）
│
├─ L0 凭据扫描层        装完即有，磁盘/内存增量 = 0
│    只读扫描四家 CLI 的落盘凭证 → 分类 → 掩码呈现
│
├─ L1 key 型直连层      磁盘/内存增量 = 0，延迟最优
│    presets(key→双端点) × per-spawn 注入：
│      claude → env；codex → owned TOML 段；grok → owned TOML 表；opencode → auth.json（原生）
│
└─ L2 订阅型引擎层      ~48MB 懒下载，进程懒启动、随 app 退出
     ~/.marionette/engine/magpie.exe（无头子进程，127.0.0.1:3425）
     grok/chatgpt/claude/copilot 订阅 → 标准协议端点 → 同一套 per-spawn 注入
```

设计原则（讨论定稿，后续不要反复）：

- **key 型不放网关**：端点本身兼容协议，中间加层纯属浪费（延迟 + 常驻进程）。
- **订阅型不自研**：Grok 要伪装 CLI 头 + token 轮换，Claude 要真二进制桥，自研 = 永远追不上的猫鼠游戏。
- **不碰别人的键**：注入要么进进程 env，要么写带所有权标记的 owned-section；所以不需要 magpie 那套 stash/applied/drift。
- **nothing is stored twice**：reference 型凭证不复制，用时现场读。

## 4. 模块设计

### 4.1 凭据扫描 `src-tauri/src/credentials.rs`（新增）

只读扫描，输出统一条目列表。扫描清单（Windows 路径，`~` = `%USERPROFILE%`）：

| 来源 | 文件 | 提取 | 分类 |
|---|---|---|---|
| OpenCode | `~/.local/share/opencode/auth.json`（fallback `.config/opencode/`、`%APPDATA%\opencode\`，复用 `provider_usage::opencode_auth_path`） | 各 provider 的 `type:"api"` 条目 key | `reference`（现场读）；`type:"oauth"` 条目 → 仅展示 |
| OpenCode | `~/.local/share/opencode/providers.json` 或用户 catalog | 自定义 provider 的 baseURL | 元数据补充 |
| Claude Code | `~/.claude/.credentials.json`、`~/.claude.json` | OAuth token 存在性 | `subscription`（仅交给引擎，Marionette 不解读） |
| Codex | `~/.codex/auth.json` | OAuth 条目存在性 | `subscription`；文件不存在（凭证在系统 secret storage）→ 显示「检测到登录、存储方式受限」提示，不深究 |
| Grok | `~/.grok/auth.json` | `key/email/expires_at` 存在性 | `subscription`（引擎用） |
| Grok | `~/.grok/config.toml` 自定义 `[model."…"]` channel | 内联 `api_key` | `reference`，只在面板展示来源 |

- 扫描时机：首启一次 + 打开凭据页/模型面板时惰性刷新（文件 mtime 缓存）。不做常驻 watcher。
- 隐私开关：`~/.marionette/settings.json` 加 `credentialScan: false` 可整体关闭（对齐 magpie 的 discovery off）。

### 4.2 凭据存储 `~/.marionette/credentials.json`（DPAPI）

```jsonc
{
  "version": 1,
  "entries": [
    {
      "id": "opencode-go",             // 稳定 id，preset 或自动生成
      "kind": "key-plan",              // key-plan | subscription | reference
      "label": "OpenCode Go",
      "preset": "opencode-go",         // 见 4.3 presets；subscription/reference 无此字段
      "secret": "dpapi:<base64>",      // CryptProtectData(user scope)；reference 型省略
      "ref": "~/.local/share/opencode/auth.json#opencode-go",  // reference 型
      "source": "scan:opencode" ,      // scan:<origin> | manual
      "createdAt": "...", "lastUsedAt": "..."
    }
  ]
}
```

- **DPAPI**：`CryptProtectData` / `CryptUnprotectData`（user 作用域）。免主密码，同机同用户自动解密，文件单独被拷走是密文。写盘沿用 `provider_usage::write_auth_atomic` 的「临时兄弟文件 + rename」模式。
- 扫描到的、属于别人文件的 key **不落进本文件**（reference 型只存路径引用）。
- 损坏兜底：解析失败不覆盖不清空，改名留档 + 重新初始化（学 `corrupt_auth_json_aborts_instead_of_wiping` 测试的立场）。

### 4.3 Provider presets（L1 的「钥匙→门牌」表）

Rust 常量表，每条：`id / label / openaiBase / wire(chat|responses) / anthropicBase / 探测策略(复用 ProbeStrategy)`。

| preset | OpenAI 兼容端点 | Anthropic 兼容端点 | 备注 |
|---|---|---|---|
| `opencode-go` | opencode 平台 `/v1`（chat） | 有（附录 A） | key 同时兼容双协议，**核心场景** |
| `deepseek` | `api.deepseek.com` | 有 | 已有探测 |
| `glm` / `kimi` / `openrouter` / `xai` | 各自 `/v1` | openrouter 有 `/anthropic` | 其余逐个核实后入表 |

> 端点 URL 以实现时的官方文档为准逐条核实填全，此表是占位结构。

### 4.4 注入策略（per-spawn / per-owned-section）

| agent | 通道 | 写什么 | 还原 |
|---|---|---|---|
| claude-code | **spawn env** | `ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN`(+`ANTHROPIC_MODEL` 钉模型) | 进程退出即消失，无需还原 |
| codex | **ACP `providers/set`**（spawn 后 RPC，不写任何文件） | `providers/set {providerId:"openai", apiType:"openai", baseUrl, headers.Authorization}`，把 openai 槽整体路由到目标端点；key 经 headers 进程内传递 | 不写盘，天然无还原问题 |
| grok-build | **owned TOML 表**（`~/.grok/config.toml`；或 per-spawn `GROK_HOME` 隔离） | `[model."marionette/<model>"]`：`base_url` 指**方言规整器**（`grok_dialect.rs`，见下）/`api_backend="chat_completions"`/`api_key="$MARIONETTE_PROVIDER_KEY"`（**env 展开实测有效**）；**不设** `[models] default`（会让 Grok 退出自身登录） | 写真实 config 时整表删除；隔离 HOME 时随目录清理 |
| opencode | **原生**（`auth.json` + provider catalog） | 复用现有 `write_provider_key`（已带 `type:"api"` 修复） | 已有 delete 路径 |
| deepseek(dsh) | 已有（`DEEPSEEK_API_KEY` 注入） | 现状不动 | — |

**S2 实测约束（2026-10，codex 0.137.0 / codex-acp 2.2.2）——codex 行的硬边界：**

- `wire_api = "chat"` 已被 codex **硬拒**（官方指路改 `responses`），`[model_providers.*]` 自定义 provider **不会**被 codex-acp advertise，「配置钉模型」也被 codex-acp 的内建 catalog 逐 turn 覆盖——owned TOML 段路线整体作废。
- `providers/set` 只支持 `apiType:"openai"`（Responses 协议）。因此：**chat-completions-only 的端点（DeepSeek、OpenCode Go、GLM…）→ codex 必须经 L2 引擎**（magpie `/v1/responses` 做翻译）；Responses 兼容端点才可 L1 直连。模型 id 保持 codex 内建名（gpt-6.1-sol 等），picker 呈现需说明「codex 会话模型名不变、流量已改道」。
- `CODEX_CONFIG` env 在 codex-acp 里是 inline JSON（与 codex core 的文件路径语义撞名），**禁用**。
- 空 CODEX_HOME 下 codex 要 `auth.json`（legacy `{"OPENAI_API_KEY":…}` 可过）；真实用户已有登录，不受影响。

- 模型 id 约定：grok 用 `marionette/<model>`（实测会被 advertise 并可 `set_model` 选中）；codex 无自定义 id（见上）。
- TOML 编辑沿用 `dsh_settings.rs` 的原则：**原文搬运/最小 diff、带所有权头注释、幂等、解析失败不写半截**。另：grok 会自己改写 GROK_HOME 的 config.toml（实测追加了 `[marketplace]` 段），owned-section 写入必须容忍并发编辑。
- grok 注入二选一：写真实 `~/.grok/config.toml`（用户自己配的自定义模型共存）或 per-spawn 隔离 `GROK_HOME`（实测无登录态可起、grok 自写文件全圈在里面，但看不到用户在真实 home 里的其它自定义模型）。默认走真实 config + owned 表，隔离 HOME 作为后续可选。
- L2 订阅型复用同一张注入表，只是 `base_url` 指向 `127.0.0.1:3425`、`api_key` 用网关令牌（**真实订阅凭证永不落 agent 配置**）。

**方言规整器（`src-tauri/src/grok_dialect.rs`，2026-10-16 实机验收发现后新增）**：
grok 重放含 assistant 消息的历史时会在每条消息对象上标注 `model_id`（多模型会话追踪），严格的 OpenAI 兼容端点（实测 api.deepseek.com）以 400 `unsupported_parameter: messages[n].model_id` 拒收——**首条可用、后续全 400** 的根因。修复：grok 跨用 channel 的 `base_url` 指向本地环回微代理（首选端口 17832，惰性启动），它只做「直通转发 + 剥离已知方言字段（当前仅 `messages[*].model_id`）」，不做协议翻译；凭证（grok 展开的 Authorization 头）原样透传、不落地。这是「key 型不放网关」原则**记录在案的最小例外**：不翻译、不改写语义。规整器不可用时回退直连端点（单轮可用）并记 warn。协议级端到端已验证：严格端点 + 直连四轮三挂，经剥离代理四轮全过（复现脚本 `%TEMP%\marionette-repro\repro8.mjs`）。**注意边界：规整器只挂在我们管理的 `marionette/` 表上——用户自己手配的原生 grok channel 不过规整器，会继续踩同一方言 bug（实机案例：用户原生 `opencode-go-cat-jam-free` channel）**；解法是引导用户改用跨用分组（见下）。

**reference 直连 + 自定义模型 id（2026-10-16 第二轮实机反馈后补齐）**：
- **reference 条目全链路打通**：扫描出的 opencode 提供商（openrouter/deepseek/moonshot·kimi/glm·zai/xai → preset 映射，大小写不敏感）进 `bridge_list_models` 的 `sources`、可绑定、注入时钥匙从 opencode auth.json **现场读**（`strict_api_key_of`：OAuth/混合条目拒绝）——不复制、不落盘。未映射的 provider（google/anthropic/openai 等）凭据页展示但不直连（无核实端点）。
- **自定义模型 id**：preset 清单不穷举（opencode-go 几十个模型、openrouter 清单为空），选择器每个来源分组末尾提供「＋ 自定义模型…」inline 输入；后端对 model_id 只校验非空。绑定恢复时匹配不到清单的 id 直接显示原始 id。
- 契约：`BridgeModelsResult { models, sources: [{entryId, preset, label, fromScan}], note }`；前端按 sources 分组，`fromScan` 带「扫描」徽章，空清单来源显示占位说明。

### 4.5 引擎管理 `src-tauri/src/engine.rs`（新增，M2）

**S1 spike 已给出确切处方（源码级验证）：**

- **拉起命令**：`magpie.exe serve`（官方无头命令，Docker entrypoint 同款；cli 发布版 = `-tags nogui` 纯静态构建，零 GUI 依赖，stdout 可捕获）。
- **围栏（二选一）**：① 在 `~/.marionette/engine/` 放 `.portable` 空标记文件 → 全部状态自动进 `~/.marionette/engine/data/`，**推荐**（零 env）；② 设 `XDG_CONFIG_HOME` + `XDG_CACHE_HOME` 为**绝对路径**（相对路径会被静默忽略、退化到真实用户目录——engine.rs 要做断言）。
- **子进程 env**：`MAGPIE_ADDR=127.0.0.1:3425`（显式钉住端口）；`MAGPIE_NO_STATS=1`（关每日遥测）；**不设** `MAGPIE_NO_ACCOUNTS`（要 discovery）；**保留** `USERPROFILE`/`APPDATA`（discovery 要读真实凭证）。
- **鉴权**：默认只听 loopback，回环客户端任意 token 放行；如需自签令牌，短命进程跑 `magpie.exe gateway-key add marionette`（同样带围栏 env），stdout 收一次性 `sk-magpie-…` 存 DPAPI。
- **安装**：GitHub releases 懒下载 `magpie-cli-windows-amd64.exe`（~48MB）→ `~/.marionette/engine/magpie.exe`；`manifest.json` 记 `{version, sha256, installedAt}`；SHA256SUMS 校验；走 `proxy.rs` 的代理配置；tauri event 报进度，支持断点。
- **健康检查**：起进程后轮询 `GET /v1/models`（首启无订阅凭证时空列表属正常）。**端口冲突**：3425 被占（用户自装 magpie）→ 探测确认后改 `MAGPIE_ADDR` 端口并让注入的 base_url 跟随。
- **退出**：随 Marionette 退出 TerminateProcess 直杀（其 JSON 全部 tmp+rename 原子写，安全）；serve 路径**无自动更新**（引擎确是耗材）；空闲回收 M3 再做。
- **升级**：健康检查失败或每周一次重解析 latest；Marionette 只依赖「本地 OpenAI 兼容端点 + `/v1/models`」契约。
- **记录在案**：serve 前置的 legacy 迁移理论上可写 agent 配置文件，但只清理 magpie/dial 自己的遗留值且幂等（干净机器零写入）。
- **降级路径**：无需触发——S1 结论为完全可行。

### 4.6 前端

- **设置 → 凭据页**：扫描结果 + 手动添加（preset 下拉 + 掩码输入框）；key 默认 `sk-…abcd`，点眼睛显示、失焦收回；删除需确认；subscription 型只显示状态与用量入口，无 reveal。
- **composer 模型面板**：原生列表下加「跨用」分组，条目带来源徽章（`OpenCode Go` / `Grok 订阅`…）；选中即完成接线（必要时后台写 owned-section / 拉起引擎，异步完成后会话可用）。
- **偏好记忆**：复用 per-dialog model memory（c0b9e2a 已落地）。
- **首启提示**：发现 N 个来源的一条 toast，可忽略。
- **引擎下载**：一次性进度条弹层。
- **风险提示**：首次选用订阅型跨用时一次性说明「以 CLI 身份访问厂商私有端点，由 magpie 维护适配」。

### 4.7 Tauri commands（新增清单）

`list_credentials` / `add_credential_key` / `delete_credential` / `reveal_credential`(短暂返回明文) /
`list_cross_models(agentId)` / `select_cross_model(sessionId, modelId)` / `engine_status` / `ensure_engine`。

### 4.8 ACP 层规则（重要；S2/S3 后按 agent 区分）

- **claude-code**：跨用会话不发 `session/set_model`，模型由 env 钉住（避免 adapter 校验未知 id；DSH `Unknown config option` 的教训）。
- **codex**：跨用会话同样**不发** `set_model`——路由由 spawn 后的 `providers/set` 一次定死，模型 id 保持 codex 内建名，Marionette 在 UI 上呈现「已改道到 <来源>」。
- **grok**：**相反，必须发** `set_model`——`marionette/<model>` 是 advertise 出来的合法 option（实测 `models.availableModels` 与 `configOptions` 双载体都带），选中即生效。
- **切换当场生效（2026-10-16 实机反馈后定稿）**：注入是 spawn 级的（env / grok 配置表），活进程改不了——但进程是 Marionette 拉起的。**选中/切回跨用模型时，若会话进程活着，复用挂起机制当场结束进程**（历史保留、下一条消息自动带历史拉起、mid-turn 先封口），banner 说明已切换。这比 magpie 体验好：它改的 env/settings 同样要重启 Claude Code 才生效，而它不拥有进程、只能让用户自己重启。
- 「跨用」分组是 **Marionette 侧目录**，不伪装成 agent advertise 的 option——呈现上分区明确，不混淆来源。

## 5. 安全设计

1. **落盘**：DPAPI(user scope) 加密；原子写；损坏留档不清空。
2. **显示**：默认掩码只露末四位；完整值仅手动 reveal；无「导出全部」。
3. **日志**：`debug_log` 只记事件不记值（现状已如此，如 `DEEPSEEK_API_KEY taken from OpenCode auth.json`）；CI 加一条 grep 守卫（日志 helper 测试断言输出不含 `sk-` 类模式）。
4. **注入面**：key 只进 env / owned-section；agent 磁盘配置零新增明文（grok 内联例外已记录）。
5. **网络**：L2 网关仅 127.0.0.1 + 令牌；真实订阅凭证不写进任何 agent 配置（只写网关令牌）。
6. **扫描**：只读；可整体关闭。

## 6. 与现有代码的衔接（复用清单）

| 现有资产 | 位置 | 复用方式 |
|---|---|---|
| opencode auth.json 原子读写 + OAuth 保护 + 测试模式 | `provider_usage.rs` | 扫描器与存储直接学它；`write_provider_key` 即 opencode 注入 |
| spawn 注入点（env 管线） | `acp.rs:446-506` | L1/L2 注入挂进同一段；`DEEPSEEK_API_KEY` 先例即本设计的微缩版 |
| owned-section 文件桥模式 | `dsh_settings.rs` | codex/grok TOML 段写入照此原则 |
| per-dialog 模型记忆 | `recentModels.ts`（c0b9e2a） | 跨用选择一并记忆 |
| 用量探测 | `provider_usage.rs` | M3 与引擎 `magpie quota` 合并展示 |
| 代理配置 | `proxy.rs` | 引擎下载与（如需）引擎出网走它 |

## 7. 里程碑

### M0 — Spike（✅ 已完成，2026-10-16，结论已写回 §4.4/§4.5/§4.8/§9）

| # | 内容 | 结论 |
|---|---|---|
| S1 | magpie Windows 无头可行性 | ✅ **可行**：`magpie serve` + `.portable` 围栏 + `MAGPIE_ADDR`/`MAGPIE_NO_STATS`；处方见 §4.5 |
| S2 | codex 自定义 provider 行为 | ⚠️ **原设计作废**：`wire_api="chat"` 被 codex 0.137 硬拒、自定义 provider 不 advertise；改用 ACP `providers/set`（Responses-only）→ chat 端点必须走 L2 引擎。见 §4.4 S2 约束 |
| S3 | grok owned 表实测 | ✅ **可行且优于设计**：`$VAR` env 展开有效（明文例外删除）、模型双载体 advertise、`set_model` 可选、隔离 GROK_HOME 成立 |

原始探针脚本与输出：`%TEMP%\marionette-spike\`（假 key，可整目录删除）。

### M1 — L0 + L1（核心场景交付）

**进度（2026-10-16）**：代码已全部落地——`credentials.rs`（扫描/DPAPI）、`model_bridge.rs`（presets/绑定/grok owned 表/去重/注入解析/providers-set 框架）、`acp.rs` 注入段、7 个 `bridge_*` 命令、凭据设置弹窗（`CredentialsDialog.tsx`）、Composer 跨用分组。`cargo test` 174 绿、`npm run build` 零错误。presets 六家端点已核实（opencode-go 以直连探测为准：`…/inference/go/v1` 与 `…/inference/go/anthropic`）。**剩余**：实机验收矩阵（需真实 key + dev 运行）。去重按「reference 对宿主 agent 隐藏」实现，无跨 agent 模糊匹配。

任务：`credentials.rs`（✅ 已落地：扫描/分类/DPAPI 存储 + 10 测试，全量 158 绿）→ presets 表 → 注入模块（claude env ✅设计已定、grok owned 表 ✅S3 验证、opencode 原生复用、codex `providers/set`）→ Tauri commands → 凭据设置页 → composer 跨用分组 → 防泄漏测试。

**验收矩阵**（S2 后修订：codex 的 chat 端点场景移入 M2）：

| 场景 | 预期 |
|---|---|
| OpenCode Go key → Claude Code 会话 | ✅ env 注入，直连 |
| 同 key → Grok 会话 | ✅ owned 表 + `$VAR` 展开，直连 |
| 同 key → OpenCode 会话 | ✅ 原生 auth.json |
| Responses 兼容端点 → Codex 会话 | ✅ `providers/set` 直连 |
| chat 端点（DeepSeek/OpenCode Go…）→ Codex | 🔁 移至 M2（引擎翻译 `/v1/responses`） |
| 终端裸用 codex/claude/grok（未选跨用） | ✅ 行为与改动前一致 |
| `dev.log` 全文无 key 明文 | ✅ grep 守卫 |

### M2 — L2（订阅跨用 + codex 的 chat 端点场景）

任务：`engine.rs`（下载/校验/`.portable` 围栏/`serve` 拉起/退出/端口冲突探测）→ 订阅型模型入 picker → codex 的 `providers/set` 指向引擎（`/v1/responses`）→ 下载进度 UI → 风险提示。

**验收**：Grok 订阅模型在 Codex 会话跑通；**OpenCode Go key 经引擎在 Codex 会话跑通 DeepSeek**（S2 移入的旗舰场景）；引擎随 app 退出；`~/.marionette` 之外无新增可见目录（`.portable` 围栏生效）。

### M3 — 打磨

配额合并展示（provider 探测 ∪ `magpie quota`）、引擎空闲回收与自动升级、扫描开关 UI、文档收尾（CURRENT.md 更新、本稿归档）。

## 8. 测试计划

- **单测**：扫描分类器（每家夹具文件，假 key）；DPAPI 往返；owned-section TOML 幂等写入/删除且保留用户键（学 `Test<Agent>KeepsTheUsersSettings` 的立场）；presets 解析。
- **集成**：spawn 一个假 agent（打印 env 的脚本）断言注入正确。
- **e2e 冒烟**：沿用 AGENTS.md 的 NDJSON 探针法起真 adapter 验证模型列表；M1/M2 验收矩阵人工跑一遍并记 `scripts/`。
- **防泄漏**：日志 helper 测试断言不含密钥模式；`credentials.json` 落盘断言含 `dpapi:` 前缀。

## 9. 风险与未决问题

| 风险 | 缓解 |
|---|---|
| ~~magpie 无头不可行（S1 失败）~~ | ✅ 已证可行（`serve` + nogui 构建），风险关闭 |
| codex 只支持 Responses 协议（S2 实测），chat 端点直连作废 | codex 场景统一走 `providers/set`；chat 端点经引擎翻译（M2）；`providers/set` 是 codex-acp 声明的官方能力，跟踪其版本变化 |
| magpie 迭代极快（v0.1.1267，日更）且不收 PR | 引擎接口收敛到 `/v1` + `/v1/models`；自动升级；随时可换 |
| 引擎端口 3425 被用户自装 magpie 占用 | 拉起前 healthcheck 探测；被占则换端口并让注入 base_url 跟随 |
| XDG 围栏变量必须是绝对路径（相对值被静默忽略） | 默认改用 `.portable` 标记方案（零 env）；若用 XDG 则 engine.rs 断言绝对路径 |
| 订阅跨用的厂商风控/条款风险 | 首次使用一次性提示；用户自担；不做静默伪装 |
| Codex 凭证在 Windows secret storage 读不到 | 显示「检测到登录、存储受限」，不破解 |
| claude env 与 ACP set_model 优先级冲突 | 规则：claude/codex 跨用会话不发 set_model；grok 必须发（§4.8） |
| 端点事实漂移（厂商改 URL/协议） | presets 集中一表，加 `lastVerified` 字段，坏了好修 |
| grok 自改写其 config.toml（marketplace 段） | owned 表写入容忍并发编辑（原文搬运原则 + 重读再写） |
| agent 方言字段被严格端点拒收（grok `model_id`，已实锤） | 方言规整器环回代理剥离（`grok_dialect.rs`）；新方言字段出现时往 strip 清单加条目，不做泛化改写 |

## 10. 附录 A：调研事实（2026-10 快照）

1. **OpenCode Go / Zen**：同一平台同一把 key；Zen 按量、Go 订阅制；端点同时兼容 OpenAI 与 Anthropic API 格式，key 可给任意工具用（来源：opencode 官方 console 文档、Docker 文档、第三方评测）。
2. **Grok 订阅上游**（magpie `internal/provider/grok.go`）：端点 `cli-chat-proxy.grok.com`（OpenAI **Responses** 协议 + 大量 body 适配）；凭证为设备码 OAuth，存 `~/.grok/auth.json`（`key/email/expires_at`）；请求需伪装 CLI 头（`x-xai-token-auth: xai-grok-cli`、`x-grok-client-identifier: grok-shell`、不显过时的版本号）；token 轮换靠拉起 `grok` CLI 刷新。→ 直连不可行，必须引擎。
3. **Claude 订阅**：OAuth token 不能直接调 Anthropic API；magpie 的桥是拉起真实 `claude` 二进制当后端（MCP 回环做工具、run 生命周期管理）。→ 最重的一档，只有引擎能做。
4. **Codex 凭证**：Windows 默认 `secret_auth_storage`（系统 secret store），文件不可读；`cli_auth_credentials_store = "file"` 时落 `~/.codex/auth.json`。magpie 同样读不到，提示改 file。
5. **magpie 工程事实**：MIT；不接受社区 PR；Windows CLI 二进制 ~48.1MB（完整 GUI ~54.2MB）；网关 `127.0.0.1:3425`，四协议端点 + `/v1/models`；Docker 镜像证明网关核心可脱离 UI 运行；自有状态明文 JSON + 0600（Unix 权限，Windows 上无加密）；UI 掩码只露末四位、完整值仅创建时打印一次；写进 agent 配置的是自签网关令牌而非用户真实 key。
6. **magpie 接线事实**（`docs/subsystems/agent-wiring.md`）：Claude Code 写 `ANTHROPIC_BASE_URL` 系 env；Codex 写 TOML `openai_base_url`；OpenCode 写 `providers.magpie` 块；Grok 写 `[model."magpie/…"]` 表（`api_backend="chat_completions"`）且**不设** `[models] default`（否则 Grok App 退出自身登录）。
7. **Marionette 既有先例**：`acp.rs:470-487` 已把 OpenCode auth.json 的 DeepSeek key 经 env 借给 DSH 进程——本设计是该模式的系统化。

**M0 spike 实测补充（2026-10-16，codex 0.137.0 / codex-acp 2.2.2 / grok 1.0.60）：**

8. **codex**：`wire_api="chat"` 已删除（官方 discussion/7782 指路 responses）；`[model_providers.*]` 不进 advertise；顶层 `model=` 钉不住（codex-acp 内建 catalog 逐 turn 覆盖）；`set_model` 拒绝未知 id。**`providers/set`**（initialize 应答 `agentCapabilities.providers` 声明）可把 openai 槽路由到任意 baseUrl + 自定义 headers，key 零落盘；仅 `apiType:"openai"`（Responses）。`CODEX_CONFIG` env 在 codex-acp 中是 inline JSON，禁用。空 CODEX_HOME 需 legacy `auth.json`。
9. **grok**：`[model."…"]` 表的 `api_key` 支持 `$VAR`/`${VAR}` 展开（实测）；模型同时出现在 `models.availableModels` 和 `configOptions`；`set_model` 接受并生效（流量实证）；`GROK_HOME` 重定向有效且无登录态可起；grok 会自写 config.toml（`[marketplace]` 段）与 README/agent_id 等文件。
10. **magpie**：`serve` 为官方无头命令（Docker entrypoint）；cli 发布版 `-tags nogui` 静态构建；`.portable` 标记文件使全部状态进 `<exe 目录>\data\`；`MAGPIE_ADDR` 钉端口；回环零鉴权；serve 路径无自动更新、无 idle 回收；JSON 全部 tmp+rename 原子写，可安全强杀；订阅 discovery 默认开、`MAGPIE_NO_ACCOUNTS` 只能关不能开；`MAGPIE_NO_STATS=1` 关遥测。

**实机验收发现（2026-10-16，M1 首轮真机测试）：**

11. **grok 的 `model_id` 方言**：本地监听全量抓包证实，grok 对第三方 chat_completions 端点重放历史时，每条 assistant 消息对象都带 `model_id`（即使全程同一模型、零切换）；顶层字段全部标准。xAI 自家后端容忍，DeepSeek 等严格端点 400。复现工具：`%TEMP%\marionette-repro\repro6.mjs`（临时 GROK_HOME + 本地 SSE 监听器 + 两轮对话）。顺带证实：grok 会向 channel 端点发后台旁路调用（「Extract durable, reusable observations…」的记忆提取），同样走该 channel。
12. **临时 GROK_HOME 复刻要点**（S3 报告的补充）：只复制 config.toml 不够——需要整套状态（auth.json、agent_id、sessions、trusted_folders 等）且 **cwd 必须是受信任的绝对路径**（TEMP 目录会触发「Authentication required: no auth method id provided」，此前误判为 auth 问题）。

## 附录 B：喜鹊知识转移（2026-10-16，三路并行挖掘）

> 来源：D:\Myself\magpie 全量源码（消费侧接线 / provider 端点 / 网关规整三路），全部带 file:line 证据存档于本次会话。原则引用其 LESSONS.md。

### B1. 已吸收进代码（本轮落地）

| 项 | 内容 | 落点 |
|---|---|---|
| xai anthropic 根形态 | `/v1` 形态会拼出 `/v1/v1/messages`（列表能答、消息全 404） | preset 改 `https://api.x.ai` |
| opencode-go anthropic 根形态 | Messages 路由在网关根，`/anthropic` 路径是从不使用的形态；`/inference/go`≡`/zen/go` | preset 改 `https://opencode.ai/inference/go` |
| OpenCode 会话头 | 网关 2026-09 起缺 `x-opencode-session` 即 400；UA≥1.18；`x-session-affinity` 头反被 403 | 规整器按 preset 注入全套头（`ses_` 形态 id 进程内固定） |
| deepseek `max_completion_tokens` | 无此字段且**静默忽略**（不报错、输出悄悄限 8K） | 规整器改名为 `max_tokens` |
| kimi 锁采样 | k2.5+ 服务端锁死 temperature/top_p，客户端带值即 400 | 规整器剥离两字段 |
| 规整器双协议化 | claude 与 grok 共用 `/p/<preset>` 地址，按路径（`v1/messages` vs `chat/completions`）选 anthropic/chat 上游 | grok_dialect 重构；claude 注入 base_url 同走规整器（回退直连） |
| `zai` 不映射 glm | Z.ai 与 open.bigmodel.cn 是两家 host，映射等于发错厂商 | 映射表摘除 |
| **动态模型目录** | 对齐 opencode 的体验（它就是启动时拉 provider 目录才知道全部模型）：`bridge_list_models` 用已配 key GET `<chat_base>/models`，按 entry 缓存（TTL 10min、失败负缓存 90s、并发拉取 3.5s 预算、>60 截断+提示），与静态清单取并集；label 用模型 id（修复「整组显示同一个 preset 名」的 bug） | `assemble_models` / `fetch_models_from_endpoint` |
| opencode-go 会话头（补充） | 动态目录 GET /models 实测**不带**会话头也放行（规整器注入的头只影响经它转发的请求） | 附录 A 第 11 条互证 |

### B2. 观察名单（未实现，触发即查这里）

**规整器候选（chat 同协议直通）**：deepseek 思考模式要求工具轮回放 `reasoning_content`（锚点：400 "must be passed back"，修法=补空格）；`role:"developer"`→system（grok 不发，防御性）；kimi 拒 `reasoning_details` 流式形状；openrouter 拒重叠 `oneOf`（同长改名 anyOf）；schema `pattern` 的 Rust-regex 语法差（xAI 目标，锚点："is not a \"regex\""）；`tools[].function.strict` 学习式剥离；孤立 `tool_choice` 剥离（xAI 400）。
**claude 侧（anthropic 直通）**：billing 头/行（`x-anthropic-billing-header`，发给非 Anthropic 端点会让前缀缓存从 0 字节 miss——量化过 4% vs 96-99% 命中）；延迟工具块（kimi/deepseek/opencode-go 对 `tool_removal` 等直接 400/422）；deepseek /anthropic 思考模式要 thinking 块。
**claude env 分层（无代理时的静默失效）**：用户 `~/.claude/settings.json` 等三层的 `env.ANTHROPIC_BASE_URL` **压过** spawn env；六个云开关（`CLAUDE_CODE_USE_BEDROCK` 等=on）完全绕过 BASE_URL。→ 计划：spawn 前检测三层冲突并在会话头警告。
**claude 未知模型三配额 env**：`CLAUDE_CODE_MAX_OUTPUT_TOKENS`（默认 32000，思考计入会撞）、`CLAUDE_CODE_MAX_CONTEXT_TOKENS`（默认 200K）、`CLAUDE_CODE_MODEL_CAPABILITIES`（无 xhigh_effort 则 Ultracode 拒）；Claude 系模型 id 必须划线拼写（`claude-opus-4-6`，点号被读成旧模型）。
**codex（M2）**：系统代理劫持 127.0.0.1（Windows `127.*` 豁免无效，Clash 全局=全 502）；OpenAI 专有字段剥离与 relay 反向校验（`codex=as-sent`）；裸 reasoning 项两端相反要求；`joinToolRuns`（tool 结果间插文本项→严格端点 400）；0.162 常驻 daemon 持旧配置。
**opencode**：1.x/2.x 大版本分裂（variants 键序必须弱→强、`service_tier` 双写 options+extraBody、2.x 自造三档变体）——未来写 providers 块时按双版本矩阵实现。
**grok**：xAI campaign 远程改 `[models] default`（2026-09 实发过）——用户默认模型「自己变了」时先想这里；表字段全集（`context_window` 裸整数、`reasoning_efforts` 按 grok 序 `["low","medium","high"]`）留给需要 effort 控件时。
**配额端点补缺**（provider_usage）：kimi `GET /v1/users/me/balance`；glm `GET /api/monitor/usage/quota/limit`（**裸 key 无 Bearer**、拒绝是 200 信封 code 1000-1005）；opencode-go `GET …/v1/usage`（需会话头；403=key 非订阅本人）。xai 无余额 API（不是漏）。

### B3. 运营纪律（抄 LESSONS.md，评审时对照）

夹具用报告者原始字节；拒绝先分层（账号/模型/请求形状）再熔断；厂商规则只作用于该厂商；读失败=未知绝非空；temp+rename+lastgood；会无声失败的字段（静默忽略）比报错的更危险；进 prompt 的附加文本不点名工具/厂商；Windows 只编译不跑测试=Windows 红不可见；tag≠release；固定的下载源对厂商自己的校验文件核。

### B4. 漂移防线（对「opencode v3 / grok 升级又冒问题」的回答）

分层防御，全部对标 magpie 已验证的姿势：
1. **学习式剥离闭环**（抄 unfit 机制）：规整器收到 400 且错误文本形如 `unsupported_parameter: messages[n].<field>` / `Unknown parameter` 时，解析字段路径，会话内自动入 strip 清单并**立刻重发一次**，dev.log 记 `[dialect] auto-strip <field> preset=<id>`——新方言从「用户报错+考古」变成「带纸面轨迹的半自动流程」。这是对升级漂移的第一道自适应防线。
2. **模型目录指纹**：`bridge_list_models` 对各 source 清单做 hash 存 `~/.marionette/`，变化记 `[drift] source=<id> added/removed`——上游 catalog 一变我们有第一手时间线。
3. **retired 记忆**（抄 retired.json）：`preset×model` 级别的「厂商侧已死」信号（410/连续 404）持久化一周，picker 过滤、显式请求放行作金丝雀。
4. **健康检查验形状不验状态码**：200 但 body 是 HTML/信封=不健康（Cloudflare 挑战、智谱 200 拒绝信封都实锤过）；引擎升级走「下载→试启动→成功才切换→失败记 blocked 版本」（Windows Smart App Control 处方）。
5. **订阅侧外包**：M2 引擎化后，grok 伪装头版本号、ChatGPT HTTP/1.1 这类真猫鼠游戏由 magpie 上游（日更）替所有人追。
