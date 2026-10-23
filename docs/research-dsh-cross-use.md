# 调研：DSH 跨用模型怎么接

> 2026-10-20 · 只调研不改代码。现行契约仍以 [`CURRENT.md`](./CURRENT.md) 为准。

## 1. 现状缺口

Marionette 跨用桥（`model_bridge.rs`）里 `classify_agent("deepseek")` 落在 `BridgeAgent::Other`：

- `bridge_list_models` 返回空目录 + 「该 agent 的模型共享还没接」
- `injection_for_agent` 只给 Claude / Grok 产 `SpawnInjection`；DSH 分支在 `acp.rs` 里是 no-op
- 归档设计稿原文：`deepseek(dsh) | 已有 DEEPSEEK_API_KEY 注入 | 现状不动`

所以 Composer 跨用分组在 DSH 会话下看不到 OpenCode Go / magpie 订阅等来源。

本机实测对照（`dev.log` + `~/.dsh`）：

| 面 | 看到的模型 |
|----|------------|
| DSH Web（`profiles/web/cordis.patch.yml`） | 已配 `opencode-go` + `OPENCODE_GO_API_KEY`，默认 `opencode-go / deepseek-v4.1-flash` |
| Marionette → dsh-acp（`profiles/acp`） | `cordis.patch.yml` 是空数组 `[]`；session/new 只广告 `deepseek-official::*` 两个 |

**一部分「看不到其他模型」不是桥没接，而是 ACP profile 根本没挂上 Web 里已经配好的 provider。**

## 2. DSH 今天怎么发现模型

机制（官方 + `@openma/deepseek-harness-acp` README + 本机）：

1. 运行中的组合（Cordis profile）挂着 `@deepseek-ai/dsh-llm-pi-ai`
2. 路由来自：profile `cordis.patch.yml` ∪ `$DSH_HOME/settings.yaml`（本机尚无 settings.yaml）∪ 目录内置
3. 凭据在 `~/.dsh/.credentials.yaml`，settings 里只留 `apiKeyEnv` 引用
4. ACP 广告的 model option id：`provider::model`（多 provider 时）；可 `session/set_config_option` id=`model`
5. **pi-ai catalog 没有的 model id 会被丢掉**；自定义路由必须自带 `api` + `baseURL` + `models`（不要去改已有目录路由的 `api`，会污染同路由其它模型）

自定义路由官方形态：

```yaml
# settings.yaml 或 profile cordis.patch.yml → llm-pi-ai.config.providers
marionette:
  displayName: Marionette 模型共享
  apiKeyEnv: MARIONETTE_ENGINE_TOKEN   # 或任意 env 名
  api: openai-completions
  baseURL: http://127.0.0.1:3425/v1    # magpie
  models:
    - id: grok/grok-4.7
    - id: deepseek/deepseek-flash
```

Web UI「设置 → 模型 → 自定义提供方」写的就是这一层；热重载对 settings.yaml 成立，profile patch 改完通常要重拉组合 / 重启 ACP 会话。

## 3. 候选注入路径

### A. 单一 magpie 路由（推荐方向）

在 ACP 可见配置里挂一条保留路由 `marionette` → 引擎 `http://127.0.0.1:{port}/v1`，`api: openai-completions`，token 用现有 `ENGINE_TOKEN`（与 OpenCode 的 `OPENCODE_CONFIG_CONTENT` 同款）。

| | |
|--|--|
| 能通什么 | 引擎已注册的 key-plan + 订阅发现（Grok/GPT…），与 OpenCode/Codex 跨用目录对齐 |
| 优点 | 一条路由覆盖订阅+钥匙；不碰用户已有 `opencode-go` 等路由；符合「拆新路由、别改目录 api」 |
| 缺点 | 依赖引擎 Running；模型 id 变成 `marionette::provider/model`，要和 Composer 绑定/set_config 对齐 |
| 侵入 | 写 `settings.yaml` 或 `profiles/acp/cordis.patch.yml` 的托管段；spawn 时注入 env |
| 重启 | 首次写入后需新 session/new；settings 热更能否让**已跑**的 dsh-acp 立刻出目录 → 需探针 |

### B. 按 preset 直连写多条 llm-pi-ai 路由

把 Marionette 凭据库 / OpenCode auth 扫描结果写成 `opencode-go` / `deepseek` / `glm`… 多条直连（用户 Web 侧已经手写过 `opencode-go`）。

| | |
|--|--|
| 能通什么 | key 型直连；订阅型仍要引擎或用户自己的 OAuth 路由 |
| 优点 | 与 DSH Web 原生体验一致；不经 magpie 少一跳 |
| 缺点 | 多路由合并/去重复杂；和用户手写路由抢同名；订阅仍另做 |
| 侵入 | 改 settings 或 acp patch + `.credentials.yaml`（或 spawn env） |

### C. 只修「Web 有、ACP 没有」——共享 settings 层

把用户已在 `profiles/web/cordis.patch.yml` 里的 `llm-pi-ai` 迁到 `$DSH_HOME/settings.yaml`（全局层），ACP/Web 共用。

| | |
|--|--|
| 能通什么 | 用户已经配过的第三方 provider（本机即 OpenCode Go） |
| 优点 | 改动小；立刻解释并缓解「DSH 下看不到我在 Web 配的模型」 |
| 缺点 | **不是** Marionette 跨用桥；Composer 跨用菜单仍空；订阅/钥匙库仍不进 DSH |
| 定位 | 可作 P0 卫生修复，与完整桥并行 |

### D. 无配置、只靠 set_config / env 钉模型

指望 `session/set_config_option model=…` 或 `DSH_PROVIDER`/`DSH_ACP_MODEL` 塞进 catalog 里没有的 id。

| | |
|--|--|
| 结论 | **不可行作为主路径**。适配器明确：catalog 没有的 id 会丢；env 覆盖的是「已有路由上的默认」，不创造路由 |

### E. Claude 式 ANTHROPIC_* env

DSH 不走 Anthropic 三件套；它吃 pi-ai 路由 + 凭据引用。此路对 DSH 不适用。

## 4. 建议

**主路径：A（magpie 单路由）+ 必要时 C（settings 共享）**

1. **P0（体验卫生，可单独做）**  
   文档/可选迁移：让 ACP 与 Web 共用 `settings.yaml` 的 `llm-pi-ai`，避免「Web 配了、木偶里没有」。

2. **P1（真正的跨用）**  
   - `classify_agent` 增加 `Deepseek` / `Dsh`  
   - `list_models`：与 OpenCode 类似，引擎 Running 时列出 engine 源（+ 可选 key-plan 直连源）  
   - 绑定后 spawn：`ensure_dsh_marionette_route` 写入托管路由（推荐落 `settings.yaml` 的 `llm-pi-ai.providers.marionette`，id 稳定、合并不覆盖用户其它 key）  
   - env：`MARIONETTE_ENGINE_TOKEN=magpie`（或写入 credentials 引用）  
   - 前端：选中跨用模型 → restart ACP → `set_config_option` `model=marionette::<engineModelId>`  
   - **不要**改用户已有 `opencode-go` 路由的 `api`/`baseURL`

与现有 agent 对照：

| Agent | 注入形态 |
|-------|----------|
| Claude | env 三件套 |
| Grok | owned config 表 |
| OpenCode | `OPENCODE_CONFIG_CONTENT` provider.marionette |
| Codex | magpie + providers/set |
| **DSH（建议）** | **settings/patch 托管路由 + env/凭据引用 → magpie** |

## 5. 最小实现草图（尚未动手）

| 步骤 | 文件 | 做什么 |
|------|------|--------|
| 1 | `model_bridge.rs` | `BridgeAgent::Deepseek`；`list_models` 引擎源；`SpawnInjection::DshRoute { … }` 或仅「ensure route」副作用 |
| 2 | 新 `dsh_bridge.rs` 或同文件 | 合并写 `~/.dsh/settings.yaml` 的 `llm-pi-ai.providers.marionette`；幂等；保留用户其它 providers |
| 3 | `acp.rs` DSH spawn | 引擎就绪时 ensure route + 注入 token env；绑定存在则记下目标 model id 供前端 set_config |
| 4 | Composer / bridge UI | deepseek 走现有跨用分组；选中后重启会话（与 OpenCode 同款） |
| 5 | 测试 | YAML 合并夹具；list_models 分类；不覆盖用户 `opencode-go` 段 |

## 6. 编码前必须探针的问题

1. **热更新**：改 `settings.yaml` 后，**已在跑的** `dsh-acp` 会不会立刻在下一次 `session/new` 里冒出 `marionette::*`？还是必须杀进程重拉组合？
2. **引擎模型 id**：magpie 目录是 `grok/grok-4.7` 这种斜杠 id；DSH models 列表里写这个 id 后，option 是否稳定为 `marionette::grok/grok-4.7`？斜杠是否被拒绝？
3. **协议**：magpie `/v1` 对 DSH `openai-completions` 是否足够（含工具调用 / reasoning）？若某订阅只要 Anthropic Messages，是否要第二条 `marionette-anthropic` 路由指引擎根？
4. **凭据**：`apiKeyEnv` 只认 `~/.dsh/.credentials.yaml` 还是也认进程 env？若只认文件，spawn 每次写 credentials 的卫生与并发要设计。
5. **与用户 Web 配置共存**：用户已有 `profiles/web` 的 `opencode-go`；托管段必须命名空间隔离（`marionette`），合并策略要有单测。
6. **Preset 锁定**：官方说 agent preset 首条消息后锁定；模型切换是否同限制？跨用换模是否必须新会话？

## 7. 本机证据摘录

- `~/.dsh/profiles/web/cordis.patch.yml`：已有 `llm-pi-ai.providers.opencode-go` + default model  
- `~/.dsh/profiles/acp/cordis.patch.yml`：`[]`  
- `~/.dsh/.credentials.yaml`：已有 `OPENCODE_GO_API_KEY`  
- `dev.log` session/new（dsh-acp 0.4.39）：`model` 选项仅 `deepseek-official::deepseek-flash|v4-pro`  
- OpenCode 先例：`opencode_engine_config_content` → provider `marionette` → `127.0.0.1:{port}/v1`

---

**结论一句话**：DSH 跨用最像样的接法是「像 OpenCode 一样挂一条指向 magpie 的 openai-compatible 路由」，配置面用 `settings.yaml`/`acp` patch 的托管段而不是 ANTHROPIC env；另可先做 ACP/Web 配置共享，让你已经在 Web 配好的 OpenCode Go 出现在木偶里。

## 8. 复核补记（ZCode，2026-10-20，静态读源 + NDJSON 实弹探针后定稿）

**拍板：走 A（magpie 单路由），但写入目标是 `~/.dsh/cordis.patch.yml`，不是 §3-A 表里写的 settings.yaml / profiles/acp。C 不做成产品功能。** §6 的四个探针全部关闭：

### 8.1 对上文的更正

- **standalone `dsh-acp` 不组装 `profiles/acp/cordis.patch.yml`，也不挂 settings.yaml 层。**
  `bootAcpProfile`（`@openma/deepseek-harness-acp` dist/index.js）组装的是
  `dsh-base` bundle + 自身 patch + **home 层 `join(home, "cordis.patch.yml")`**（即 `~/.dsh/cordis.patch.yml`）。
  dsh-base 的 `settings` / `config-editor` / `hmr` / `plugin-manager` 行全是
  `disabled: !!js "!ctx.get('profileContext')"` —— standalone 没有 profileContext，
  **settings.yaml 层在 dsh-acp 里根本不存在**（README 的「settings.yaml 热更」只对 Web/desktop 成立）。
  `profiles/acp/` 属于桌面 app 的 profile 体系，Marionette spawn 的 standalone 适配器不看它。
- **§3-C（ACP·Web 共享 settings.yaml）对 standalone dsh-acp 不成立**：settings.yaml 不被加载。
  Web 里配好的 provider 想进 Marionette 的 DSH 会话，同样要写 `~/.dsh/cordis.patch.yml`。
  C 的价值被 A 覆盖（引擎路由本来就出全部目录），不值得单做。
- **根层 home patch 是全体共享的**：`dsh-app-boot` 的 `readProfilePatches` 对**每个** profile
  （web、desktop、standalone）都叠加 `join(context.home, "cordis.patch.yml")`（lib/index.js:1028）。
  所以托管路由写进这个文件后，dsh Web 的模型菜单也会出现 `marionette::*` 条目——
  这不是泄漏是特性：`ENGINE_TOKEN` 是常量 `magpie`（本地回环、非机密），
  把 `MARIONETTE_ENGINE_TOKEN: magpie` 同步写进 `~/.dsh/.credentials.yaml` 后，
  **引擎在跑时 dsh Web 也能真实走这条路由**；引擎没跑请求会连不上（可理解的失败）。

### 8.2 探针结论（对应 §6）

| # | 问题 | 结论（证据） |
|---|------|--------------|
| 1 | settings 热更 | standalone 无 hmr/settings → 路由变更需进程重启。对 Marionette 无影响：spawn 前幂等重写 + 跨用换模本来就重启会话（与 OpenCode 同款） |
| 2 | 斜杠模型 id | 安全。适配器 `splitChoice` 按**首个** `::` 切分；实弹探针 `marionette::group/auto-claude-haiku-5-5` 原样出现在 option 列表 |
| 3 | 协议 | `openai-completions` 是 llm-pi-ai 受支持的路由 api（`supportedProtocols`）；magpie `/v1` 即 OpenAI 兼容（OpenCode 先例同端点）。**一条路由够，不需要 anthropic 第二条**。实现时跑一轮真实对话作最终验证 |
| 4 | 凭据 | **进程 env 优先于文件**：`dsh-credentials-local.resolve` 先查 `inherited()`（完整 `process.env`、大小写不敏感），再查 `.credentials.yaml`。spawn 注入 env 即可用；注意 env 存在时 Web 侧对该 ref 的写入会被拒（"shadowed"），所以文件由 Marionette 直接写、不走 Web |
| 5 | 与用户配置共存 | home patch 行语义是**整行 config 替换不合并**（dsh-base 头注释），合并必须在单条 `llm-pi-ai` 行内做：保留 `config.providers` 下其它 key、保留文件里其它行。合并夹具必须进单测 |
| 6 | preset 锁定 | `session/set_config_option id=model` 会话中可切（适配器 setConfig + saveDefaultSelection）；沿用 OpenCode 的「绑定→重启→原生菜单/DSH_MODEL」流程规避边角 |

### 8.3 实弹探针记录（已清理现场）

写 `~/.dsh/cordis.patch.yml` 挂 `marionette` 路由（api: openai-completions、baseURL: `http://127.0.0.1:3425/v1`、apiKeyEnv: MARIONETTE_ENGINE_TOKEN、models 两个引擎 id），spawn `dsh-acp`（DSH_ACP_HOST 指向 ambient 引擎）+ env 注入 token：

- `initialize` OK（dsh-acp 0.4.39 / harness 0.2.0-rc.2）；
- `session/new` 的 model option 里出现
  `marionette::group/auto-claude-haiku-5-5`、`marionette::opencode-go/grok-4.7`（多 provider 下自动加 `provider::` 前缀）；
- 发现一个**与本桥无关但要注意的坑**：默认 provider（deepseek-official）一个凭据都没有时，
  `session/new` 直接报 `Authentication required` 挡住——现有 DEEPSEEK_API_KEY 注入 / `dsh-acp login` 已覆盖，桥不引入新认证步骤；
- 探针后 `~/.dsh/cordis.patch.yml` 已删（原本不存在），临时 session root 已清。

**实现后复验（2026-10-20，同日）**：用 `dsh_bridge.rs` 真实写入器产出的 cordis.patch.yml +
最小 credentials 文档落进 `~/.dsh`，同款探针全链路通过——`session/new` 广告 4 个
`marionette::` 引擎模型（斜杠 id 原样），`session/set_config_option model=marionette::
group/auto-grok-4-7` 应答 echo `currentValue` 正确回显。注意 set_config 会把选中模型写进
`~/.dsh/acp-standalone-model.json`（适配器的默认选择持久化），探针后已还原成 deepseek 官方默认。

### 8.4 定稿实现面（替代 §5 草图的落点）

1. `model_bridge.rs`：`classify_agent` 加 `deepseek` 分支，`list_models` 复用 OpenCode 的引擎源写法（引擎没跑给指引）。
2. 托管路由写 `~/.dsh/cordis.patch.yml` 的 `llm-pi-ai` 行 `config.providers.marionette`（幂等、每次 spawn 前用当前引擎目录重刷 models 列表、保留用户其它内容）；同步把 `MARIONETTE_ENGINE_TOKEN: magpie` 合并进 `~/.dsh/.credentials.yaml`（保用户条目），spawn 仍注入同名 env 兜底。
3. `acp.rs` DSH spawn 分支：引擎就绪时 ensure 路由 + 注入 env（对齐 OpenCode 的 `wake_if_installed` 姿势）。
4. 前端：跨用选中 → 重启会话 → 目标 id 即广告值 `marionette::<engineModelId>`（set_config_option 或 spawn env `DSH_MODEL` 钉默认均可，按现有桥 UI 流程选）。
5. 单测：home patch 合并夹具（已有 llm-pi-ai 行 / 已有其它 provider / 空文件）、credentials 合并、不碰用户 `opencode-go`。

### 8.5 上线后补课：适配器预检的 `<provider>_API_KEY` 命名约定（2026-10-20 当天）

首日真用就踩了一个 §8 全部探针都没覆盖到的坑：**模型广告、跨用切换全部正常，
真发第一条消息却在请求前 2ms 被拒 "Authentication required"**。

根因（`@openma/deepseek-harness-acp` dist/plugin.js 的 `credentialEnvNames`）：
适配器在每次请求前有自己的预检闸门 `requireCredential(provider)`，它**按
`<provider>_API_KEY` 约定自己推凭据 ref 名**（`marionette` → `MARIONETTE_API_KEY`），
**完全不读路由 config 里的 `apiKeyEnv`**。我们当初起的 `MARIONETTE_ENGINE_TOKEN`
预检不认 → 闸门先于 llm-pi-ai 的路由解析（那套 env→文件解析本来是通的）把请求拒了。

修复：ref 名统一改成 **`MARIONETTE_API_KEY`**（dsh_bridge 常量 `MARIONETTE_TOKEN_ENV`）
——预检约定和路由 apiKeyEnv 殊途同归，一个名字两处都认；已实弹复验
session/new → set_config → **真实 turn**（经引擎/订阅回 "ok"）。

**两条教训入档：**
1. **NDJSON 探针必须含一条真实 `session/prompt`**（注意 prompt 是 content-block
   数组不是字符串）——目录/切换探针过 ≠ 请求链路通，预检这类「请求前闸门」
   只有真请求能暴露。
2. 给 dsh 挂自定义 provider 的凭据 ref，**永远用 `<provider>_API_KEY` 形态**；
   web Models 页手配的第三方 provider 也是这么存名的。

### 8.6 上线后补课二：定制路由的模型默认「看不见图」（2026-10-21）

**症状**：DSH 里给 `marionette` 路由的 `opencode-go/deepseek-v4.1-flash` 附图，
模型回一句「图被省略」；DSH 会话/请求里的用户消息变成

```
[image omitted because this model accepts text only; attachment sha256:25c19047]
```

——图根本没出 Marionette，也没到引擎。

**根因（全在 DSH 侧，与引擎无关）**：`dsh-llm` 在发请求前按
`modelInfo.inputModalities` 投影内容，**不含 `"image"` 就把 image block 换成上面
那段占位文本**（`projectImagesForTextModel` / `textOnlyImageText`，dsh 0.2.1-rc.2
`dsh-llm/lib/index.js` 的 `adapterStream`）。而 `inputModalities` 来自
`dsh-llm-pi-ai` 解析出的模型描述符：`input = entry.input ?? base?.input ??
request.defaultInput`（`DEFAULT_INPUT = ["text"]`）。`marionette` 是**目录外的
自定义路由**，`base` 不存在；桥此前每条模型只写 `- id:`，于是所有模型（包括
引擎目录明明标了 `modalities.input: ["text","image"]` 的）都被 DSH 当成盲模型。

**A/B 实弹（临时 `DSH_HOME` + 假 OpenAI 端点抓请求体）**：

| 路由里的模型条目 | 主请求里有什么 |
|---|---|
| `- id: "…"` | `[image omitted because this model accepts text only; …]`，无 image block |
| `- id: "…"` + `input: [text, image]` | `image_url: "data:image/png;base64,…"`，无占位文本 |

**修复**：`dsh_bridge.rs` 生成 `models:` 时按引擎 `/v1/models` 的
`modalities.input` 给模型补 `input: [text, image]`（`model_bridge::CatalogModelRow.images`
→ `parse_models_catalog_rows` 解析，`engine::catalog_direct_entries` 原样带出）。
没标图能力的模型不写 `input`，保持 pi-ai 默认 `[text]`：DSH 就地省略图片，而不是
把图打到上游换一个 400（magpie 对文本模型收图正是 400
`model "…" does not support image input`）。

**上线复验（真 ACP 链路，非探针假端点）**：`initialize` → `session/new` →
`set_config_option(model=marionette::opencode-go/deepseek-v4.1-flash)` →
`session/prompt`（内容块数组：文字 + `{"type":"image","mimeType":"image/png",data}`），
一张写着 `MARIONETTE-7421` 的 PNG 被原样读出，`stopReason: end_turn`，无占位文本。

**教训**：给 pi-ai 自定义路由广告模型，**`id` 不是全部**——`input`（以及
`contextWindow`/`maxTokens`）不写就是 pi-ai 的默认值，DSH 会按默认值做内容投影。
桥目录以后加能力位，先看 pi-ai `modelProfile` 有哪些字段。

### 8.7 上线后补课三：`cordis.patch.yml` 被自己的「空 patch」形态卡死（2026-10-23）

**症状**：DSH 会话的模型菜单里始终没有 `marionette::*`（跨用分组可点，但钉上去不生效），
dev.log 每次 DSH spawn 都有一条：

```
[warn] [bridge] session=… dsh marionette route refresh failed; bridge skipped |
  cordis.patch.yml 不是顶层数组形态（找不到 column-0 的 `- ` 条目）— 拒绝写入
```

**根因（自锁环）**：`dsh_bridge::remove_marionette_route_at` 在「我们的 entry 是文件里
唯一条目」时把 entry 删干净、落一个 `[]` 占位——但**说明注释行留着**，文件变成
「`MANAGED_HEADER` + `[]`」。下一次 spawn 走 `merge_home_patch` 时，空文件判定只认
字面 `[]`，这个形态既不空、又没有 `- ` 条目 → 拒写。引擎只要闲置回收（或用户手动停）过一次，
路由就**永久挂不上**，且失败的只是 warn，界面只表现为「DSH 没有共享模型」。

**修复**（`src-tauri/src/dsh_bridge.rs`）：

1. 空 patch 判定按语义来——只有注释/空行/`[]` 的文件一律当空 patch 重写（保留用户注释，
   占位行被托管块取代）；`MANAGED_HEADER` 提成常量。
2. 移除托管 entry 时连带收走 `MANAGED_HEADER`，不留「块在下面」的孤儿说明。
3. 回归测试：`rewrites_comment_only_empty_patch`（三种骨架，含幂等）、
   `remove_then_readd_round_trip`（移除 → 清成 `[]` → 再挂上）。

**实弹复验**：拿本机那份坏文件（`MANAGED_HEADER` + `[]`）过修复后的合并函数产出
patch + 临时 `DSH_HOME`，同款 NDJSON 探针（`initialize` → `session/new`）广告出
82 条 `marionette::…`（`model option count: 84`，另两条是 `deepseek-official::*`）。

**同一轮的前端补课**：`lib/bridge.ts` 的目录缓存以前只在会话挂载时拉一次，
引擎在菜单打开期间醒来的话，已打开的菜单会一直显示「模型共享未运行」——对
DSH/Pi/Codex 这种**只有引擎源**的 agent 就是「列表空着不刷新」（Claude/Grok 还有
key-plan 分组，看着像刷新了）。现在 `invalidateBridgeModels()` 会通知订阅者，
Composer 订阅后当场重拉（引擎状态翻转、凭据增删都会触发）。
另：`bridge_list_models` 的「菜单打开先唤醒引擎」名单补上 `Zcode`
（DSH/Pi 那次是同款漏登记）。
