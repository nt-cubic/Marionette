# 模型共享与额度面板踩坑集（2026-10-23 全天实弹排查）

同日排掉的一串连环坑：opencode 凭据搬家、双钥匙抢槽、智谱别名缺口、ZCode Coding Plan
钥匙藏位。每条都有实机证据链；改引擎注册 / 凭据扫描 / USAGE 探测前先过一遍。

## 0. 快速确诊表

| 症状 | 根因 | 修法 |
|---|---|---|
| USAGE 面板没有智谱行，脚注报 `No balance probe for 'zhipuai-coding-plan'` | provider_usage 只认 `glm\|zhipu\|bigmodel`，opencode 登录写的键名是 `zhipuai-coding-plan` | 已修：glm 条目别名 + match 分支补齐 |
| 引擎额度里 OpenCode Go 报 `401 api_key_expired`，但 TUI 能对话 | 引擎注册的是 **auth.json 里的死钥匙**；真凭据已迁 `opencode.db` | 已修：凭据读取叠库（见 §1） |
| 共享模型组在 DSH 里整个消失 | `cordis.patch.yml` 的 marionette 路由在上次 spawn 时因引擎 Absent 被移除 | 引擎运行时**新开一个 DSH 会话**即重建 |
| opencode 新出的免费模型别的 agent 里看不到 | 三层缓存 + 只在 spawn 时重建（见 §5），非 bug | 新开会话；magpie 目录最长 24h 跟进 |
| ZCode 里配的智谱 Coding Plan 余额不显示 | 钥匙在 ZCode 个人 provider 层，Marionette 没读 | 已修：注册时扫 `provider_config.json`（见 §4） |

排查额度先分清两条链路：**A** USAGE 的 provider 行（`probe_provider_usage`，只读
opencode 凭据，只在 opencode agent 会话驱动）；**B** 引擎行 + 悬浮球余额
（`engine_quota` → `GET /v1/magpie/quotas`，Header `Authorization: Bearer magpie`）。
「少一行」先打 B 接口看原始 JSON——错误行（windows 空且无 balance）在 UI 上是跳过不显示的。

## 1. opencode 凭据已迁库：auth.json 是尸体（当天最大坑）

2026-10 起 opencode 把凭据写进 **`~/.local/share/opencode/opencode.db` 的 `credential`
表**（`integration_id` = 旧 auth.json 键名；`value` = `{"type":"api","key":…}` JSON；
同一 integration 可多行，`active=1` 活 / `active=0` 被替换 / `NULL` 迁移拷贝）。auth.json
迁移后**不再更新**（本机停在 09-02，库里 opencode-go 已是 10-13 的新钥匙）。

只读 auth.json 的后果：引擎注册死钥匙 → Go 额度 401、跨用 Go 模型断流，而用户 TUI 一切
正常——「程序说钥匙过期、用户说钥匙好好的」这种对峙，先查库。

```sql
-- 只读查询（WAL 下与 TUI 并发安全）
sqlite3 "file:~/.local/share/opencode/opencode.db?mode=ro" \
  "SELECT integration_id, active, time_updated FROM credential"
```

**修复落点**：`credentials.rs` 的 `peek_opencode_db_keys_at`（active=1 在前、NULL 殿后、
active=0 与 OAuth 剔除）+ `merge_opencode_key_sources`（库里有钥匙的 integration 以库为
准，auth.json 只兜库中缺席的 integration），`peek_opencode_api_keys()` 出的就是合并序。

## 2. 同槽位双钥匙与「先到先得」

auth.json / 库里 `opencode`（Zen/Go 平台 key）与 `opencode-go`（Go key）**都**映射到
preset `opencode-go`（`preset_for_opencode_provider`），而 `registration_plan` 是先到
先得、每 magpie provider 只收一把。auth.json 对象序 = 字母序（serde_json 默认 BTreeMap），
`opencode` 恰好排前——纯靠运气定胜负。

**勘误记录**：排查中一度得出「额度端点只认控制台 key」并据此写了 `opencode` 优先的
排序——**错的**。真因始终是死/活，不是 key 类型：拿库里的活 plan key 实测额度端点
直接 200。最终规则：排序上移到凭据源层（库 active=1 → 库 NULL → auth.json 兜底），
`reference_keys_as_presets` 保持保序不定序。教训：**对「哪把钥匙该赢」下结论前，
先把每把候选钥匙单独打到真实端点上对照错误消息**。

## 3. OpenCode Go 额度端点的规矩

- `GET https://opencode.ai/zen/go/v1/usage`，Bearer + **`x-opencode-session`（+
  `x-opencode-session-id`）必带**，形状 `ses_` + 12 位小写 hex + 14 位 base62；缺头 400
  「Request is missing x-opencode-session」。
- 完整客户端形态见 magpie `OpenCodeClient`（`opencode_client.go`）：UA
  `opencode/1.19.0 ai-sdk/… runtime/bun/…`、`x-opencode-request`、`x-opencode-client:
  cli`、`x-opencode-project: global`。额度端点对 UA 不挑（浏览器 UA 也过）。
- **两种 401 可判断钥匙死活**：`api_key_expired`（过期的平台 key，官方明确「重新登录
  不续期，去 Console 重生成」）vs `AuthError: Unauthorized`（被替换的旧 key / 不认的
  key）。本机两把死钥匙各中一种，库里的活 key 直接 200。
- 响应：`usage.rolling / weekly / monthly {status, percent, resetsAt}`。
- chat 端点对 zen 的免费模型名报 400「Model is unavailable」——免费模型在 zen 不在 go，
  **不是**认证失败（401/403 才是）。

## 4. ZCode Coding Plan 钥匙的藏位（UI 与存储分离）

ZCode 设置页「Bigmodel - Coding Plan」填的 API key **不落** `v2/config.json`——那里
provider 条目的 `options.apiKey` 恒为空串。本体在**个人 provider 层**
`~/.zcode/v2/provider_config.json`（`ZCODE_HOME` 可挪根）：

```json
config.providerConfigRules.providerRules[].config.access =
  {"type": "zhipu-coding-plan-api-key", "apiKey": "<钥匙>"}
```

providerId 形如 `bigmodel-api`。该文件同时是跨用桥 model_bridge 的托管位——**只读不写**。
`config.json` 的空 `apiKey` + 设置页的遮罩值曾经把排查带偏到「账号 OAuth」（`setting.json`
的 `modelProviderFamilyModes.zai=oauth` 只关乎 zai 家族）；全盘按智谱 key 的独有形状
（32 hex + `.` + 16 位）扫描才定位。修复：`credentials.rs`
`peek_zcode_coding_plan_keys` → preset `glm`，`register_key_providers` 里排在显式
key-plan 之后、opencode 引用之前。注册后 magpie `zhipu`（region=coding）出 5h/7d/MCP
月度三窗口 + 套餐档位/有效期。

## 5. 目录与模型的刷新链（高频误解，非 bug）

- 三层缓存：magpie 的 models.dev 目录 **24h 过期 + 每小时 KeepFresh**；Marionette
  `/v1/models` 60s TTL；前端 `bridge.ts` per-agent 模块缓存（`engine-state-changed` /
  凭证变更才失效）。**打开模型对话框不触发刷新**，也没有刷新按钮。
- DSH 的 `marionette::` 模型只在 **spawn 时刻**重写 `cordis.patch.yml`；引擎 Absent →
  移除路由，Starting/Downloading → 文件不动（第一次 spawn 可能赶不上唤醒，第二次收敛）。
  已开的 DSH 会话永不热更新。
- 引擎 30 分钟无绑定空闲回收是常态——「共享模型/额度突然没了」先想这个。

## 6. pi-ai catalog 限制的准确边界（勘误）

「pi-ai catalog 没有的 model id 会被 DSH 丢掉」**只适用于 catalog 命名路由**。声明路由
（marionette：自带 `api` + `baseURL` + `models`）不依赖 catalog——`resolveRouteModels`
里 catalog 条目只是字段兜底，id 不在 catalog 照样解析、照样真跑（`dsh-llm-pi-ai/lib/index.js`
+ research-dsh §8.3 探针双证）。代价：没声明 `reasoningEfforts` 的模型 reasoning 落
false（无 effort 控制）、价格显示无——`dsh_bridge` 目前不声明档位，这是已知留白。

## 7. 引擎侧钥匙操作速查

```powershell
# 看引擎注册了什么、额度原始 JSON
curl.exe -H "Authorization: Bearer magpie" http://127.0.0.1:3425/v1/magpie/quotas
# 换钥匙（已存在的 provider；running 时可热换）
~\.marionette\engine\magpie.exe provider key opencode-go <key>
# 新增（glm preset 要带 region=coding，否则落到按量端点报无余额）
~\.marionette\engine\magpie.exe provider add zhipu <key> region=coding
# 引擎实际持有的钥匙（指纹对照用）
#   ~\.marionette\engine\data\providers.json
```
