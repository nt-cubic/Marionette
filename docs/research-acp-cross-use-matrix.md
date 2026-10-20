# 调研：其余 ACP agent 的跨用桥可接性矩阵

> 2026-10-20 · 只调研不改代码。DSH 的跨用已落地（`dsh_bridge.rs`，见
> [research-dsh-cross-use.md](./research-dsh-cross-use.md) §8）；本文回答
> 「剩下 9 个 Tier 2 agent 能不能同样接，怎么接，接不了的就放弃」。
> 网关面 = magpie 的 `/v1`（openai-completions）+ `/v1/messages`（anthropic），
> Bearer 令牌常量 `magpie`。

## 结论一页

| agent | 注入面 | ACP 共用 | 结论 | 实现模式 |
|---|---|---|---|---|
| **pi** | `~/.pi/agent/models.json` providers 段（官方 schema） | 是 | **可接（最干净）** | 托管 JSON 合并（同 OpenCode 姿势） |
| **omp**（oh-my-pi） | `~/.omp/agent/models.yml` providers 段 | 是（"Same engine, four wrappers"） | **可接** | 托管 YAML 写入器（同 dsh_bridge 姿势） |
| **kimi**（@moonshot-ai/kimi-code） | `KIMI_MODEL_*` env 家族（内存合成临时 provider） | 是（ACP 同引擎，envOverlay 全局） | **可接** | 纯 env 注入（零配置文件） |
| **codebuddy** | `CODEBUDDY_BASE_URL`+`API_KEY`+`MODEL` env，或 `~/.codebuddy/models.json` | 是（官方 ACP 文档认可 spawn env） | **可接** | 纯 env 注入 |
| **hermes**（NousResearch） | `~/.hermes/config.yaml` providers（transport 双面）+ `~/.hermes/.env` | 是（`hermes acp` 同 core） | **可接** | 托管 YAML + env 文件 |
| **openclaw** | `~/.openclaw/openclaw.json` models.providers（JSON5） | 是（bridge→Gateway 共用），**但 ACP 无 per-session 选模型** | **可接（有坑）** | 托管 JSON5 写入 + Gateway 存活依赖 |
| **cline** | `cline auth` 落 `~/.cline/data/settings/providers.json` + `CLINE_*` env 钉死 | 是（providers.json 自动复用） | **勉强可接（先探针）** | 预存 + env 钉死 |
| **gemini** | 只有 Gemini 协议（`GOOGLE_GEMINI_BASE_URL` 走 generateContent） | — | **放弃**（除非给 magpie 加 Gemini 面） | — |
| **cursor** | 无 BYO 端点；BYOK 仅对 chat，agent 流量钉死后端 | — | **放弃** | — |

前端两种接管形态（机制都已存在）：有原生模型菜单的（pi/omp/kimi/hermes/cline）
→ 钉原生偏好，chip 退位（同 grok/opencode/dsh）；没有菜单或不可切的
（openclaw/codebuddy）→ env 钉默认 + 跨用 chip 常驻（同 claude 的模式）。

## 逐家处方

### pi — 可接（本机已装，源码级核实）

- 配置：`~/.pi/agent/models.json`（`PI_CODING_AGENT_DIR` 可挪）：

```json
{
  "providers": {
    "marionette": {
      "baseUrl": "http://127.0.0.1:{port}/v1",
      "api": "openai-completions",
      "apiKey": "$MARIONETTE_ENGINE_TOKEN",
      "models": [{ "id": "group/auto-grok-4-7" }]
    }
  }
}
```

- `apiKey` 官方支持 `$NAME`/`${NAME}` env 插值 → spawn 注 env，凭据不落盘。
- JSON + 「同 id 增改替换」合并语义 → serde_json 只动 `providers.marionette`
  键（同 OpenCode config 合并），`/model` 菜单打开即重载。
- ACP：model option id = `provider/id`，**首个 `/`** 切分 → 原生 id
  `marionette/group/auto-grok-4-7`，`set_config_option` 直接切；前端钉偏好用
  `marionette/` 前缀（与 grok/opencode 同款）。

### omp（oh-my-pi，pi 的 fork）— 可接

- 不复用 `~/.pi`，home 是 `~/.omp/`，模型配置从 JSON 变 YAML
  （`~/.omp/agent/models.yml` providers + `config.yml` 的 `modelRoles.default`）。
- `api` 支持 openai-completions / anthropic-messages 等；模型 id 任意。
- 写入器注意 YAML 文本手术纪律（同 dsh_bridge）；实现前 `omp models spark`
  式验证 + NDJSON 探针确认 ACP 形态与 pi 一致。

### kimi — 可接（顺滑）

- spawn env 四件套，**零配置文件侵入**：

```
KIMI_MODEL_NAME=<引擎模型id>
KIMI_MODEL_PROVIDER_TYPE=openai        # 或 anthropic 走 /v1/messages
KIMI_MODEL_BASE_URL=http://127.0.0.1:{port}/v1
KIMI_MODEL_API_KEY=magpie
```

- env 在内存合成临时 provider 并设为 defaultModel；ACP 同引擎生效，
  configOptions 的 model 菜单含合成模型、可 set_model。
- 坑：config.toml 声明的 provider 不回退读 shell env（env 路径是唯一免写文件
  通道）；env 合成只收 `kimi|anthropic|openai` 三种 type。

### codebuddy — 可接

- spawn env：`CODEBUDDY_BASE_URL=http://127.0.0.1:{port}/v1` +
  `CODEBUDDY_API_KEY` + `CODEBUDDY_MODEL`（官方拿 DeepSeek 做过第三方接入示例）。
- 或 `~/.codebuddy/models.json`（注意字段名 `vendor`/`url`，**url 要写全路径到
  `/chat/completions`**，支持 `${ENV}` 展开，1s 防抖热重载；无 Anthropic 面）。
- 坑：ACP 有 authenticate 握手，已给 API key 时是否可跳过需 NDJSON 探针；
  ACP 扩展无客户端侧选模型口子 → 跨用 chip 常驻形态。

### hermes — 可接（双面）

- `~/.hermes/config.yaml`：

```yaml
providers:
  marionette:
    api: http://127.0.0.1:{port}/v1
    key_env: MARIONETTE_ENGINE_TOKEN
    transport: chat_completions        # 或 anthropic_messages
```

- 凭据在 `~/.hermes/.env`（`key_env` 引用）；模型 `custom:<name>:<model>`，
  providers.models 列表或 `/model custom` 发现。ACP 只列已配好凭据的 provider。

### openclaw — 可接（两个坑）

- `~/.openclaw/openclaw.json`（**JSON5，可能有注释** → 文本手术或保留注释的
  合并）`models.providers.marionette`：`baseUrl` + `apiKey`（`${ENV}` 插值）+
  `api: openai-completions` + models[]（任意 id，无 catalog 丢弃）。
- 坑 1：`openclaw acp` 是 bridge → 需要 **OpenClaw Gateway 守护进程在跑**，
  spawn 前要保证存活（或 Marionette 拉起）。
- 坑 2：ACP 明确不暴露 per-session 选模型（模型 = agent 的 primary）→ 跨用 =
  把 primary 指到网关，chip 常驻形态，换模型走重绑 + 重启。

### cline — 勉强可接（接之前先探针）

- 处方：`cline auth --provider openai-compatible --apikey magpie --baseurl
  http://127.0.0.1:{port}/v1 --modelid <id>`（或直接写
  `~/.cline/data/settings/providers.json`）+ spawn env
  `CLINE_PROVIDER=openai-compatible` / `CLINE_MODEL=<id>` / `CLINE_API_KEY=magpie`。
- 坑 1：**必须 CLI ≥ 3.0.8**（旧版 openai-compatible 在 CLI 是坏的）。
- 坑 2：ACP 的 availableModels 来自**静态 catalog**，自定义 model id 可能不进
  菜单、默认解析可能回落 stock id（`@cline/core` 闭源，推不到底）→
  实现前必做 NDJSON 探针（session/new 的 configOptions + 实发 model id）。
- 坑 3：ACP 内切换被限制为 cline 系 auth method → 必须 env 钉死，chip 常驻。

### gemini — 放弃（协议不通）

- 只说 Gemini 协议（`{base}/v1beta/models/{m}:generateContent` + `x-goog-api-key`），
  全源码无 OpenAI-compat 客户端，官方支持请求仍是 open issue
  （google-gemini/gemini-cli#23385）。extension 机制换不了 LLM 端点。
- 唯一接法 = 给 magpie 增设 Gemini 协议面（再谈），agent 侧无口子。

### cursor — 放弃（无口子）

- BYOK/Override Base URL 仅对 **chat** 生效，Agent/Composer/CLI 流量强制走
  Cursor 自家后端（官方帮助页 + 论坛计费实证）；CLI 的 `-e <endpoint>` 是
  Cursor 私有协议端点，不是 OpenAI 兼容面。ACP 同样钉死。

## 建议落地顺序

> **2026-10-20 决定（已执行）**：roster 收束为「五跨用 + pi + hermes」，
> 本矩阵中 cline / gemini / kimi / codebuddy / openclaw / cursor / omp 的
> 接入处方**封存不排期**（要重启时从本文取方即可）；**pi 桥已实现并实弹
> 验证**（`model_bridge.rs` 的 `refresh_pi_marionette_route`，NDJSON 探针
> 确认 `marionette/<id>` 广告与 set_config 回显）；hermes 桥待本机装好
> hermes 后按上文处方接。

1. **pi**（机制最干净、本机已装、源码核实过）——顺手把 omp 的写入器骨架
   留出来（同构 YAML 版）。✅ 已完成
2. **kimi、codebuddy**（纯 env，改动最小；codebuddy 探针 authenticate）。
3. **omp、hermes、openclaw**（各需配置文件写入器；openclaw 多一个 Gateway
   存活依赖）。
4. **cline**：先跑 NDJSON 探针，静态 catalog 不吞自定义 id 再排期。
5. gemini、cursor：关闭。

## 来源（关键几条）

- pi：本机 `@earendil-works/pi-coding-agent` 的 `docs/models.md`
  （configure-a-compatible-endpoint）、`schemas/models.schema.json`、
  `pi-acp` dist（model option `provider/id`）。
- omp：github.com/can1357/oh-my-pi README（"Fork of Pi" + `~/.omp` + models.yml）。
- kimi：MoonshotAI/kimi-code `docs/en/configuration/{providers,env-vars}.md`、
  `packages/node-sdk/src/config/env-model.ts`、`docs/en/reference/kimi-acp.md`。
- codebuddy：codebuddy.cn/docs/cli/{models,env-vars}、腾讯云 ACP 集成文档。
- hermes：hermes-agent.nousresearch.com/docs/integrations/providers、
  .../features/acp。
- openclaw：docs.openclaw.ai/{cli/acp, gateway/config-tools/custom-providers,
  provider-examples}。
- cline：docs.cline.bot/{cli/cli-reference, usage/acp, sdk/model-providers}、
  cline/cline `apps/cli/src/acp/acpAgent.ts`、issue #10750。
- gemini：google-gemini/gemini-cli `docs/reference/configuration.md`、
  `packages/core/src/core/contentGenerator.ts`、issue #23385。
- cursor：cursor.com/docs/cli/acp、cursor.com/help/models-and-usage/api-keys。
