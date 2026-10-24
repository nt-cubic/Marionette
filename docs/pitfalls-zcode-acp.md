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
3. 开始菜单（用户/公共）与桌面的 `ZCode.lnk`——读字节流，扫 **ANSI 与 UTF-16LE**
   可打印串（偶/奇对齐都扫），每个「含盘符 `:` 且含 `zcode`」的候选都拼
   `resources/glm/zcode.cjs` 直到 `is_file`。只扫 UTF-16 且返回第一命中会丢掉
   真实安装（雷 10）。

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

### 雷 7 续：`contextWindow: 0` 同样整层拒绝——引擎缺元数据时必须写兜底值

**症状**（2026-10-23 本机，用户报「所有模型设置都不见了、也没法重新配」）：

- ZCode 模型下拉整个空掉，不只是 `marionette` 的模型；用户原有的 Deepseek /
  BigModel Coding Plan 也一起消失；
- 设置页改不动/加不了 provider（每次保存都落在同一个非法文件上）；
- 磁盘上的 `~/.zcode/v2/provider_config.json` **看起来是好的**（合法 JSON、条目都在），
  所以「文件没坏」这个判断会误导排查。

**根因**：个人层 schema 里 `providerModelRules[].config.properties.contextWindow`
是 `number().positive()`。引擎目录某行没报窗口时，旧代码把 `m.context_window == 0`
原样写了进去；zod 报 `too_small / expected number to be >0`，**整层**
`providerModelRules` 校验失败 → zcode 以内存空配置降级（磁盘文件保留，但 UI 读到的
是空层）。与雷 7 的未知键是同一失效模式，只是这次错在**值**而不是键名。

**证据**（`~/.zcode/v2/logs/<YYYY-MM-DD>.log`）：

```
[provider-config] Personal Provider Config 加载失败，已保留磁盘状态并以内存空配置降级
{"error":{"name":"ZodError","message":"[{\"origin\":\"number\",\"code\":\"too_small\",
\"minimum\":0,\"inclusive\":false,\"path\":[\"providerModelRules\",12,\"config\",
\"properties\",\"contextWindow\"],\"message\":\"Too small: expected number to be >0\"}]"}}
```

**修复**（`model_bridge.rs`）：

- 写入器对 `context_window == 0` 落兜底 `ZCODE_UNKNOWN_CONTEXT_WINDOW = 200_000`
  （与 zcode 内置 `.*` 兜底同值），绝不写 0；`max_output == 0` 时同样不写
  `optionSpecs.maxOutputTokens`（缺省比写 0 安全）。
- 已被写坏的现场：把托管条目里的 `"contextWindow": 0` 改成正数（或删掉托管段让
  Marionette 下次 spawn 重写）。zcode 有文件监听，**通常无需重启**：本机实测
  17:17:10 修好文件、同一主进程（pid 未变）在 17:17:10 就 `model-selection.getView OK`、
  17:17:11 `provider/model 就绪`，之后日志不再出现 `加载失败`。若 UI 仍空白再重启 ZCode。
- 回归测试：`zcode_model_rules_fallback_when_context_window_unknown`。

**快速确诊**：日志 grep `Too small: expected number to be >0` +
`"path":["providerModelRules"`；同时 `provider_config.json` 里 grep `"contextWindow": 0`。

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

## 雷 10：`.lnk` 的 LocalBasePath 是 ANSI，UTF-16 第一命中是假路径

**症状**：Marionette 日志
`ZCode runtime not found (no zcode on PATH, no desktop install)`，适配器
`spawn failed: zcode not found` → 用户面 `zcode create failed: zcode backend
reader exited (backend dead)`——但 `D:\Program Files\ZCode\resources\glm\zcode.cjs`
和开始菜单 `ZCode.lnk` 都在。

**根因**：Windows LinkInfo 的 `LocalBasePath` 是 **ANSI**，`LocalBasePathUnicode`
才是 UTF-16。按偶对齐扫 UTF-16 会把 ANSI 字节读成一段看起来像路径的串（本机
实测 `D:\Program Files\ZCode``，多一个 backtick）。`lnk_utf16_dir` 返回第一个
匹配就停，调用方发现 `zcode.cjs` 不存在就放弃**整个快捷方式**，同一 .lnk 里
真正的 ANSI `D:\Program Files\ZCode\ZCode.exe` 根本没被看到。

**证据**（本机 `ZCode.lnk`，2026-10-23 16:04 会话）：

```
UTF-16 even: "D:\\Program Files\\ZCode`"
ASCII:       "D:\\Program Files\\ZCode\\ZCode.exe"
```

**修复**：同一 .lnk 扫 ANSI + UTF-16 偶/奇对齐，每个候选都拼
`resources/glm/zcode.cjs` 直到 `is_file`（`acp.rs::lnk_candidate_dirs`）。

**快速确诊**：dev.log 有 `ZCode runtime not found`，同时
`icacls`/`dir` 看得到桌面版 cjs 和 `ZCode.lnk` → 中此雷。

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

## 雷 13：postinstall 是 Unix-only——`npm i -g` 在 Windows 上必然回滚成空壳（2026-10-24）

**症状**：`npm install -g zcode-acp-server`（Marionette 应用内安装/升级按钮同通道）失败；
或装完发现适配器是**空壳**——`node_modules\zcode-acp-server` 递归 0 个文件、
`npm ls -g` 显示 `zcode-acp-server@`（版本号空）、`%APPDATA%\npm` 的 shim `.cmd` 还在
但指向不存在的 bin。UI 表现：ZCode agent 永远起不来，且没有直观报错。

**根因**：包的 postinstall 写死了 Unix 语法：
`node dist/remote/hub-upgrade-notify.js 2>/dev/null || true`。npm 在 Windows 用
cmd.exe 跑 lifecycle scripts：`/dev/null` 不存在（`The system cannot find the path
specified.`）、`true` 不是命令（`'true' is not recognized as an internal or external
command`）→ postinstall **恒败** → npm 回滚整个安装。回滚清理又常撞 EPERM（目录被
占用/杀软扫描），最终留下 0 文件的空壳目录 + 死 shim。

**证据**（2026-10-24 本机重装实测）：

```
npm error command C:\WINDOWS\system32\cmd.exe /d /s /c node dist/remote/hub-upgrade-notify.js 2>/dev/null || true
npm error The system cannot find the path specified.
npm error 'true' is not recognized as an internal or external command
npm warn cleanup Failed to remove some directories [ ... EPERM rmdir ... zod ...]
```

`--ignore-scripts` 重装 3 秒完成（30 包），NDJSON 探针 initialize + session/new 全绿
（0.76.2，configOptions：model=67 / mode=5 / thought=3）——适配器不依赖任何
lifecycle script。

**修复/规避**：

- 手工：`npm install -g zcode-acp-server --ignore-scripts`，然后照例重跑
  `scripts/patch-zcode-acp-personal-models.mjs`（雷 12）。
- 产品（`commands.rs::npm_install_args`）：对 `zcode-acp-server` 一个包追加
  `--ignore-scripts`——应用内安装按钮与启动时 auto-update 走同一条
  `npm_install_global`，一并生效。**其它 npm agent 不动**（opencode-ai 的
  postinstall 负责放置平台二进制，跳过会得到 16 位应用假 exe，见 opencode 事故）。
- 同轮修 UI 误报：`zcode_desktop_present()` 原来认 `~/.zcode/v2` 目录——跨用桥
  自己就写这个目录（provider_config.json），从没装过桌面版的机器也会被判
  「已安装」。改为 `acp::zcode_bin_path().is_some()`（与 spawn 注入同一条探测链）。

**快速确诊**：`npm ls -g zcode-acp-server` 版本号为空 / 包目录 0 文件 → 此雷。
每次装完必查 `dist\config\options.js` 里有 `builtinModelIds` 补丁标记。

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
| 有桌面版 + ZCode.lnk，仍报 `backend dead` / `zcode not found` | dev.log `ZCode runtime not found`；.lnk 是否只扫了 UTF-16 | 雷 10 |
| 模型目录归零（用户自己的 provider 也一起消失） | zcode 日志 `Personal Provider Config 加载失败` + 报错里的 `code` | 雷 7（`unrecognized_keys`）/ 雷 7 续（`too_small`，`contextWindow` 为 0） |
| 窗口 200K / 不收图 | providerModelRules 是否写了元数据 | 雷 8 |
| 默认模型被换 | providerOrder 是否前插 / 是否写了账号层 | 雷 9 |
| 跨用 opencode-go 模型全 401 `api_key_expired`，opencode agent 自己正常 | `magpie providers` 指纹 vs opencode.db active=1 行 vs auth.json | 雷 11 |
| 桌面版能选的 key，ACP 模型菜单里一条都没有 | `provider_config.json` 里该 provider 有没有 `providerModelRules`（模板建的 provider 通常没有） | 雷 12 |
| `npm ls -g` 显示 `zcode-acp-server@`（空版本）/ 包目录空壳、agent 起不来 | postinstall 是否 Unix-only；用 `--ignore-scripts` 重装 | 雷 13 |
| 重装桌面版后每条 prompt 报 `Select a model before continuing (CONFIGURATION_ERROR)` | zcode 日志 `加载失败` 的 path 是否 `manualProviderModelRules`；`provider_config.json` 的 `modelConfigRules` 有无该键 | 雷 14 |

---

## 雷 11：引擎读空 opencode.db——`type:"key"` vs `"api"`（跨用侧，2026-10-23 追加）

**症状**：所有走引擎的 `opencode-go/*` 跨用模型 401 `api_key_expired`（错误文本带
「Signing in again will not renew this key」）；用户的 opencode agent 里用同样模型一切
正常；`magpie providers` 显示引擎持有的钥匙指纹**既不是** opencode.db 里 active=1 的
活钥匙、**也不是** active=0 的死钥匙，而是 **auth.json 里 `opencode` 条目**的那把
（本机实锤：指纹 `sk-8…yCJ2` = auth.json `opencode` 行，而 db 活钥匙是 `oc_sk_…`）。

**根因**：`credentials.rs::peek_opencode_db_keys_at` 的类型过滤只认 `"type":"api"`
（老 auth.json 格式），而**现行 opencode.db 的 credential.value 写 `"type":"key"`**
（2026-10-23 实测本机全部 11 行）——整个新库被过滤成空 → 合并回退 09-02 的
auth.json → `opencode` 条目（9 月旧 Go 钥匙，服务端已轮换过期）排在 `opencode-go`
前、两者映射同一个 preset → 先到先得，死钥匙注册进引擎。opencode agent 正常是因为
它直接读 db 的 active=1 行，不走这段代码。注意引擎的注册时机：enable 流水线一次 +
凭据页刷新/增删 key 时补注册（`reregister_key_providers`）——**代码修了但引擎没重启
也没人开凭据页时，旧注册会一直残留**。

**修复**：类型过滤同时收 `"api"` 与 `"key"`（拦 `"oauth"`）。现场恢复 = 把 db 活钥匙
重新注册进引擎（`magpie provider key opencode-go <key>`，与 `engine.rs` 的
`run_magpie_cli` 同款动作；实测注册后 `provider test` 三端点全绿、引擎对话恢复）。

**教训**：修「读不到新存储」这类 bug，**fixture 必须从真机 dump**——当时的单测手写
`"type":"api"`，测试绿但真机整库读空；「上游说 key 过期」也不一定是钥匙真过期，
先比对**指纹**确认用出去的到底是哪把，别急着让用户去换 key。

---

## 雷 12：桌面版能选的 provider，ACP 菜单里一个模型都没有（2026-10-23 追加）

**症状**：ZCode 桌面版里「BigModel Coding Plan」这个 provider 能正常选 GLM-5.3，
但在 Marionette 的 ZCode ACP 模式里，模型菜单只有账号层 provider（DeepSeek 2 条）
和桥接的 `marionette\…`，这个 key 的模型**一条都不出现**。

**根因**：`zcode-acp-server` 的 `dist/config/options.js::loadAllModels()` 装配下拉
列表只有两个来源：

1. `~/.zcode/v2/config.json`（账号层）里 `providerSelectable` 的 provider；
2. `provider_config.json` 的 `modelConfigRules.providerModelRules`。

3.12+ 桌面版把**所有用户自己加的 key** 写进个人层（`provider_config.json`），而这类
provider 的模型**只走第 2 条**。从内置模板（`zcode-builtin.json` 的
`templateRules[].config.builtinModelIds`）创建的 provider **没有任何 providerModelRules**
——桌面版直接拿模板渲染，所以桌面正常；ACP 侧 `modelsByProvider.get(pid)` 是 undefined
→ 0 条模型 → `session/new` 的 configOptions 里整个 provider 消失。`modelContextWindow()`
同源，所以就算把模型补出来，上下文也是 0（客户端用量表读 `used / 0`）。

**证据**（本机 2026-10-23）：

```
# 探针 session/new 的 model 选项：74 = 2 (账号层 DeepSeek) + 72 (marionette 桥)
model option count: 74
[bigmodel-api] 不存在

# 个人层里这条 provider 是 keyed 的，但没有模型规则
bigmodel-api  access.type=zhipu-coding-plan-api-key  personalModelIds: []  providerModelRules: 无

# 桌面/后端自己认得它（所以不是 key 或端点的问题）
[zcode-agent-service] provider/model 就绪 {"modelId":"GLM-5.3","providerId":"bigmodel-api"}
```

**修复**：`scripts/patch-zcode-acp-personal-models.mjs` 给适配器补回退链——
`providerModelRules → rule.config.personalModelIds → 模板 builtinModelIds`，并把
`modelContextWindow()` 的兜底接上模板 `modelConfigRules.modelRules`（按表序取最后一条
声明了正窗口的规则；大小写不敏感 + 锚定，否则 `GLM-5.3` 匹配不到小写规则，会掉回 200K）。
补完探针 74 → 76，`bigmodel-api\GLM-5.3` / `GLM-5.3-Flash` 出现，`session/set_config_option`
切过去成功（后端 registry 认这两个 id）。

**注意**：`npm i -g zcode-acp-server`（Marionette 的 agent 升级按钮就是它）会整体替换
`dist/`，**升级后重跑这个脚本**。脚本是分段幂等的：缺哪段补哪段，旧版补丁会被就地升级
（第一版脚本曾插入过一条更窄的 import，重复插入会让适配器 ESM 重声明直接崩，
`import-reset` 那段就是为它准备的）。

**前端同轮补课**（`src/lib/modelLabel.ts`）：适配器的选项名是 `Provider › Model`
（`BigModel Coding Plan › GLM-5.3`），旧逻辑只认 `::` / `/` / `\`，会把这个名字
整条当行标签、分组落到裸 `bigmodel-api` 上。现在 `providerPrefix` / `pathPrefix`
都收 `›`，并且 `\` 优先于 `/`（`marionette\zhipu/glm-5.3` 的 provider 段是
`marionette` 而不是 `marionette\zhipu`）。副作用是账号层那条 UUID provider 也从
「裸 UUID 分组」变成 `DeepSeek`。

---

## 雷 14：`manualProviderModelRules` 必填——桥从头建文件缺该键，整层拒载、turn 死在 model_creation（2026-10-24）

**症状**：重装 ZCode 桌面版后（`~/.zcode` 全新、个人层只剩跨用桥写的条目），Marionette 的
ZCode 会话**每条 prompt** 都报
`ZCode turn failed: CONFIGURATION_ERROR Select a model before continuing (Model creation failed)`。
zcode 协议日志（`~/.zcode/cli/log/zcode-<日期>.jsonl`）里 `turn.failed` 的
`turnPhase: "model_creation"`，且 `session/create` 的 `hasInitialModel: false`。

**根因**：桌面宿主的个人层 zod schema 把 `config.modelConfigRules.manualProviderModelRules`
当**必填数组**。跨用桥（`model_bridge.rs::zcode_merge_provider_config`）在文件不存在时从头
建 JSON，只写 `providerOrder / providerRules / providerModelRules`。10-23 之前没事，是因为
桌面写过的文件里本来就有这个键、合并路径会原样保留未知键；重装后桥先写，键缺失 → 每次读层
`Invalid input: expected array, received undefined` → **整层个人配置内存降级为空** → 后端
注册表里连 `marionette` provider 都没有 → 会话没有任何可选模型（适配器侧 configOptions
也空、无 currentValue 兜底）。

**证据**：`~/.zcode/v2/logs/<日期>.log` 每分钟一条
`[provider-config] Personal Provider Config 加载失败，已保留磁盘状态并以内存空配置降级` +
ZodError（`"path": ["manualProviderModelRules"]`，12:06 起持续到修复）。schema 形状的最好
参照仍是适配器 `dist/settings/provider-config.js` 的空文件默认：
`{ providerConfigRules: { providerRules: [] }, modelConfigRules: { providerModelRules: [], manualProviderModelRules: [] } }`。

**修复**：合并体对 `manualProviderModelRules` 缺失/非数组补 `[]`（已有值原样保留），回归
测试 `zcode_merge_backfills_manual_provider_model_rules`。现场恢复：给
`provider_config.json` 的 `modelConfigRules` 补 `"manualProviderModelRules": []` 即可——
桌面有文件监听/周期重读（本机 12:26 补上，12:25:41 起每分钟告警即停），随后探针
session/new 直接得到 67 条模型 + `currentValue: "marionette\auto/deepseek-v4-pro"` 默认值，
不选芯片直接 prompt PONG。

**附带观察**：重装后账号层 `~/.zcode/v2/config.json` 不存在（桌面还没写过），适配器
`loadAllModels` 的账号层来源为空、启动时报 `provider-registry: sync threw (ENOENT ...
v2\config.json)`——非致命，桥模型照常工作；桌面设置页动一次 provider 后该文件才会出现。

**快速确诊**：zcode 日志 grep `manualProviderModelRules`；`provider_config.json` 的
`modelConfigRules` 里有没有 `"manualProviderModelRules"` 键。
