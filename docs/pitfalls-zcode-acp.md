# ZCode ACP 接入踩坑集（2026-10-23）

ZCode（zcode-acp-server 桥 → `zcode app-server`）接入与跨用桥落地过程中的实测雷区。
每条按「症状 → 根因 → 证据 → 修复/规避 → 快速确诊」记录，行号/文件以 2026-10-23 的代码为准。
相关现行契约见 `AGENTS.md`「ZCode 接入」一节；本文是它的详细版与证据链。

按踩中的顺序编号；雷 4、5、7 是最伤的三个（分别吃掉了一轮端到端实测）。

---

## 雷 1：ZCode Protocol 没有 jsonrpc 信封

**症状**：给 `zcode app-server` 发标准 JSON-RPC 2.0（`{"jsonrpc":"2.0",...}`），整包被拒。

**根因**：app-server 说的是自家 "ZCode Protocol v1"（NDJSON），请求就是 `{id, method, params}`，
响应 `{id, result}` / `{id, error}`，服务端→客户端请求同形（id 是 `"server-N"` 字符串）。
信封字段（`jsonrpc`）属于未知键。

**证据**：

```
>> {"jsonrpc":"2.0","id":1,"method":"initialize","params":{...}}
<< {"error":{"code":-32600,"data":{"issues":[{"code":"unrecognized_keys","keys":["jsonrpc"],...}]},"message":"Invalid ZCode Protocol message"},"id":"invalid-message"}
```

**修复**：直连后端时去掉信封；ACP 信封由 zcode-acp-server 负责翻译，Marionette 侧不感知。

---

## 雷 2：session/create 被服务端反向请求挂起

**症状**：`session/create` 发出后永远等不到 `id` 应答，像是死了。

**根因**：create 处理中途，服务端**反向**发来 `session/requestRuntimePreferences`
（params 带 `sessionId` 和 `scope: "runtime-materialization"`），客户端必须应答
`{id, result: {...}}`，create 的应答在那之后才回。`session/send`（scope
`user-execution`）同样会再来一轮。首启还可能有
`interaction/requestOfficialMcpAuthHeaders`（image_search 插件要 zcode.z.ai 的头），
答 `{}` 即可继续。

result 的最小形状（其它字段有默认值）：

```json
{ "nativeSearchEnhancementsEnabled": false, "memoryEnabled": false, "askUserQuestionAutoResolutionEnabled": true }
```

**修复**：NDJSON 探针必须实现「服务端请求 → 应答」的回路；不分方向的话 create 永远挂起。

---

## 雷 3：适配器懒启动——握手探针测不出 spawn 问题

**症状**：`initialize`、`session/new` 全绿，模型目录也拿到了；第一条 `session/prompt` 才炸。

**根因**：zcode-acp-server **首个 prompt 才 spawn** `zcode app-server` 后端。握手阶段的
探针根本没碰后端进程，任何后端启动问题（见雷 4）都测不出来。

**教训**：验证 zcode 链路的探针必须走到 `session/prompt`（一条 `Reply with exactly:
PONG` 就够），停在 session/new 等于没测。

---

## 雷 4：`spawn EFTYPE`——Windows GUI 下 ZCODE_NODE 候选链全空（最伤之一）

**症状**：第一条 prompt 直接报
`{"code":-32603,"message":"Internal error","data":{"details":"spawn EFTYPE"}}`。

**根因**：适配器拿到 `.cjs` 型 `ZCODE_BIN` 时，会自己找一台 sqlite-capable 的 Node 来
前缀启动。候选链（dist/backend/resolve.js `candidateNodeBinaries`）：

1. `ZCODE_NODE` env；
2. `/opt/homebrew/bin/node`、`/usr/local/bin/node`（unix 路径，Windows 上永远空）；
3. `whichSync("node")`（Windows GUI 进程的 PATH 经常解析不到）；
4. Zed 内置 node 的 glob（unix 路径）。

全空后走 fallback：**裸 exec 那个 .cjs**——Windows CreateProcess 拒绝非可执行镜像，
报 EFTYPE。openma 的 CI（windows-latest）测不出这类 GUI PATH 差异，和 DSH 符号链接
坑（AGENTS.md）同源。

**修复**：宿主把「起适配器的那台 node」显式递过去。Marionette 的 spawn 分支
（`acp.rs` zcode 段）：注入 `ZCODE_BIN` 的同时，若 `resolved.program` 文件名以 `node`
开头且用户未自设 `ZCODE_NODE`，注入 `ZCODE_NODE = resolved.program`。

**快速确诊**：E2E 探针配 `ZCODE_BIN: <cjs>, ZCODE_NODE: process.execPath` 重跑，
若由挂变通即是此雷。

---

## 雷 5：一条 stdio MCP 条目毒化整个 session/new（最伤之二）

**症状**：`session/new` 的 `mcpServers` 里放一条 stdio 型
（`{name, command, args}`），**整个调用** -32602，会话创建失败——不只是那条 server 不生效。

**证据**：

```
{"code":-32602,"message":"Invalid params","data":{"_errors":[],"mcpServers":{"0":{"_errors":[],
 "url":{"_errors":["Invalid input: expected string, received undefined",...]},
 "headers":{"_errors":["Invalid input: expected array, received undefined",...]},
 "type":{"_errors":["Invalid input: expected \"http\"","Invalid input: expected \"sse\"","Invalid input: expected \"acp..."]}}}}}
```

**根因**：适配器 session/new 的 zod schema 对每条 mcpServer 只收 **url 型**
（`url` + `headers` + `type ∈ http/sse/acp`），stdio 的 `command/args` 根本不在 schema，
一个未知形状让整包校验失败。

**修复**：Marionette 侧 `agent_registry::accepts_stdio_mcp("zcode") = false`，
`mcp_payload_for_agent` 对 zcode 跳过 stdio 条目（记 skipped note），远程条目照常注入——
适配器把它们 per-load 带给后端，与 zcode 自己 `~/.zcode` 的 MCP/插件合流，实测模型能
真实调用借贷的 http MCP 工具（E2E `PONG:marionette-http-mcp-ok`）。

**教训**：「这个字段我不支持」有两种实现——忽略，或者拒绝整包。写适配器/客户端时
必须实测区分，后者意味着一条坏数据能杀死整个会话。

---

## 雷 6：ZCode 桌面版装哪了——注册表、PATH、默认目录全找不到

**症状**：`where zcode` 空、卸载注册表（HKLM/HKCU/WOW6432Node 的 Uninstall，按键名和
DisplayName 双查）全空、`%LOCALAPPDATA%\Programs\ZCode` 不存在——但 app 明明装着
（本机在 `D:\Program Files\ZCode`）。

**根因**：桌面版安装器允许任意目录，不登记标准卸载键，CLI 不上 PATH；适配器自己的
默认探测只覆盖 `%LOCALAPPDATA%\Programs\ZCode`。

**修复**：`acp.rs::zcode_bin_path()` 三级探测：

1. `%LOCALAPPDATA%\Programs\ZCode\resources\glm\zcode.cjs`；
2. `%ProgramFiles%\ZCode\resources\glm\zcode.cjs`；
3. 开始菜单（用户/公共）与公共桌面的 `ZCode.lnk`——读字节流，扫 **UTF-16LE 可打印串**，
   取「含盘符 `:` 且含 `zcode`」的那段；结尾是 `.exe` 就取父目录。LocalBasePath 就是
   以这种形态躺在 .lnk 里的，不用引 shell-link 解析库。

**快速确诊**（node 一行版）：

```js
const b = require("fs").readFileSync(process.env.APPDATA +
  "/Microsoft/Windows/Start Menu/Programs/ZCode.lnk");
let s = ""; for (let i = 0; i + 1 < b.length; i += 2) {
  const c = b.readUInt16LE(i);
  if (c >= 32 && c < 127) s += String.fromCharCode(c); else { if (s.length > 8) console.log(s); s = ""; }
}
```

---

## 雷 7：个人 provider 层的 zod 整层拒绝——一个未知键毁掉全部 provider（最伤之三）

**症状**：把跨用 provider 写进 `~/.zcode/v2/provider_config.json` 后，zcode 的模型目录
**变成 0 条**——不只是托管 provider 没出现，用户自己原有的 provider 也一起消失。

**根因**：个人层的 schema **不收** `modelConfigRules.modelRules`。`modelRules`
（regex `modelMatch` 匹配式）是**内置表** `resources/config/provider/zcode-builtin.json`
的形状，个人层是另一套：`modelConfigRules.providerModelRules[]`，条目**显式**
`{providerId, modelId, config}`。zod 严格模式下任何一个未知键都会让整层解析失败，
然后**内存降级为空配置**（磁盘文件不动）——用户层整个报废。

**证据**（zcode 自己的日志，`~/.zcode/v2/logs/<YYYY-MM-DD>.log`，grep
`Personal Provider Config`）：

```
[provider-config] Personal Provider Config 加载失败，已保留磁盘状态并以内存空配置降级
{"error":{"name":"ZodError","message":"[{\"code\":\"unrecognized_keys\",\"keys\":[\"modelRules\"],\"path\":[],\"message\":\"Unrecognized key: \\\"modelRules\\\"\"}]"}}
```

**正解**（`model_bridge.rs::zcode_marionette_model_rules`，实测加载成功）：

```json
{
  "providerId": "marionette",
  "modelId": "glm-5.3",
  "config": {
    "enabled": true,
    "properties": {
      "contextWindow": 1000000,
      "inputFormat": { "supportsText": true, "supportsImage": true, "supportsVideo": false, "supportsAudio": false, "supportsPdf": false },
      "outputFormat": { "supportsText": true },
      "supportsToolCall": true
    },
    "optionSpecs": { "maxOutputTokens": { "max": 65536 } }
  }
}
```

**方法论两条**：

1. zod 的报错**逐键**报（一次一个 unrecognized key），可以拿最小复现迭代探测出整张
   schema——每轮改一个键，看日志报下一个键名；
2. zcode-acp-server 的 `dist/config/options.js::loadPersonalModels` 是个人层 schema 的
   最好参照——它读什么字段，后端就收什么字段（`providerModelRules` /
   `manualProviderModelRules` 就是这么反推出来的）。

**快速确诊**：改完 provider_config.json 后必看当日 zcode 日志有无
`Personal Provider Config 加载失败`；模型目录归零 ≈ 中此雷。

---

## 雷 8：模型元数据不写就落兜底——200K 窗口、不收图

**症状**：托管模型出现了，但 contextWindow 显示 200000、图片输入不支持（1M 模型被
显示错、附图被换成「不收图」占位）。

**根因**：zcode 内置表有一条 `.*` 兜底规则（contextWindow 200000、supportsImage
false），任何没有专属规则的陌生模型都吃它。与 DSH 的
`DEFAULT_CONTEXT_WINDOW = 262144`（AGENTS.md DSH 节）同一课：**目录里的元数据必须
随模型一起写**，光列 model id 不够。

**修复**：跨用桥把引擎目录的 `context_window / max_output / images` 逐模型写进
providerModelRules（见雷 7 的正解形状）。实测写入后
`marionette\glm-5.3-e2e | ctx:1000000 | out:65536 | img:true`。

---

## 雷 9：两个 provider 层的分工——写错层会劫持默认 provider

zcode 有两层 provider 配置，跨用托管必须选对：

| 层 | 文件 | 谁写 | 托管风险 |
|---|---|---|---|
| 账号层 | `~/.zcode/v2/config.json` | 桌面版自己重写（账号同步） | 写入会被冲掉；**且**适配器的凭据注入只扫这层（`loadZcodeCredentials` 的 "first enabled provider wins"）——写这层可能劫持默认 provider 的 env 注入 |
| 个人层 | `~/.zcode/v2/provider_config.json` | 桌面版 3.12+ 新增 provider 的正位，用户/工具层 | 可托管；适配器 env 注入不读这层，无劫持 |

配套规则：

- `providerOrder` 只能**追加**尾部。它影响默认选择顺序，前插等于抢用户的默认模型。
- 托管条目按 `providerId`（`marionette`）识别、整体替换，不靠字符串匹配；引擎停用
  即三处全清（order / providerRules / providerModelRules），不留连不上的死条目。
- `ZCODE_PROVIDER` env 可以钉 provider（适配器与后端都认）；Marionette 不设，
  尊重用户默认。

---

## 次要记录（不致命但值得留）

- **会话导入的存储不在 rollout、不在 tasks-index**：zcode 的对话主库是
  `~/.zcode/cli/db/db.sqlite` 的 `session`/`message`/`part` 三表（`rollout/*.jsonl`
  只是模型 I/O 轨迹日志，`v2/tasks-index.sqlite` 是任务索引——都不是对话）。
  转录是**两段式**：`message` 存时间线事件（role + semantics.kind，合成事件
  如 system_reminder/todo_reminder 也在里面），内容分片在 `part` 表按
  `message_id` 关联（type: text/reasoning/tool 有内容，step-*/timeline 跳过）。
  只看 message 表会以为没有正文——正文根本不在那。parser：`parsers/zcode.rs`。

- **`resolve_spawn_command` 不认 `.cjs`**：它只把 `.cmd/.bat` 解成 node、`.ps1` 给
  powershell、无扩展名找兄弟 .cmd/.exe；`.cjs` 会原样返回，Windows CreateProcess
  跑不动。适配器场景无所谓（Marionette 只递路径，真正 spawn 的是适配器）；要直跑
  `.cjs` 的场合（登录流程）得自己拼 `node <cjs>`（`commands.rs::start_zcode_cjs_login`）。
- **`model.available` 是懒枚举**：session/create 应答的 `settings.model` 里没有（或为空），
  第一回合结束后的 `state.updated`（rev 2）才推完整目录。探针别在 create 后就下结论。
- **直连 raw app-server 要显式传两个 env**：`ZCODE_PERSONAL_PROVIDER_CONFIG_FILE` /
  `ZCODE_BUILTIN_PROVIDER_CONFIG_FILE`（适配器就是这么递的，显式传保证 revision 稳定）。
- **服务端交互请求清单**（探针要能应答，否则挂起）：`session/requestRuntimePreferences`、
  `interaction/requestOfficialMcpAuthHeaders`（答 `{}` 即可）、`interaction/requestPermission`
  （权限流）、`interaction/requestUserInput`。
- **zcode 日志是第一诊断现场**：`~/.zcode/v2/logs/<日期>.log`（provider 层加载、凭据）
  与 `~/.zcode/cli/log/zcode-acp-<日期>.log`（适配器）。provider/配置类问题先 grep
  `加载失败`，比猜快得多。

---

## 快速确诊表

| 症状 | 先看 | 结论 |
|---|---|---|
| `Invalid ZCode Protocol message` | 请求体里有没有 `jsonrpc` | 雷 1 |
| session/create 无应答 | 是否漏应答服务端反向请求 | 雷 2 |
| 握手绿、首条 prompt 炸 | stderr/错误里的 `spawn ...` | 雷 3 + 4 |
| `spawn EFTYPE` | `ZCODE_NODE` 是否注入 | 雷 4 |
| session/new -32602 | mcpServers 里有无 stdio 条目 | 雷 5 |
| 找不到 zcode.cjs | 注册表/PATH/lnk 探测链 | 雷 6 |
| 模型目录归零 | zcode 日志 `Personal Provider Config 加载失败` | 雷 7 |
| 窗口 200K / 不收图 | providerModelRules 是否写了元数据 | 雷 8 |
| 默认模型被换 | providerOrder 是否前插 / 是否写了账号层 | 雷 9 |
