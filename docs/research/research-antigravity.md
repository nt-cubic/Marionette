# 调研：Google Antigravity（agy_acp_server）接入（2026-11-19）

- 产品背景：Antigravity 是 Google 2025-11-18 与 Gemini 3 同发的 agent-first IDE；2026-05-19
  Google 官宣把 **Gemini CLI 过渡为 Antigravity CLI**（Skills/Hooks/Subagents/Extensions 保留为
  Antigravity 插件）。Google Cloud 官方指引（2026-02）：要 IDE/agent manager 体验选 Antigravity，
  要终端/headless 选 Gemini CLI——两者正在合流，Antigravity 是继承人。
- 对 Marionette 有意义的是**第二条接入路径**：Antigravity 除了 IDE，还随包分发一个
  **first-party ACP server（`agy_acp_server`）**——协议就是我们主打的 ACP，模型是 Gemini 3.x/4.x，
  认证含「普通 Google 账号免费档」。壳不需要 IDE，直接把 server 当一个 ACP agent 跑。
- 参考实现：codeg v0.46.0（本地克隆 `D:\Projects\codeg`，pin 版本 agy-acp-server **1.5.0**）。
  方法：2 个只读探索代理分区深挖（Rust 接入链路 / 登录+解析+前端）+ 主代理核对关键事实
  （registry 条目、scratch 模块头、Marionette roster 历史注释均系原文引用）。
- Marionette 侧接入点对照与改动清单在 [`../plans/plan-antigravity.md`](../plans/plan-antigravity.md)。

## TL;DR

- **能接，而且是少有的「免钥匙」agent**：`oauth-personal` 一个 Google 账号到免费档，
  不需要任何 API key。对 Marionette 的基本盘（不想折腾钥匙的中文用户）价值直白。
- **四个硬骨头**（都有 codeg 现成解法可抄）：
  ①Windows 构建是 PyInstaller onefile，**每次启动往 %TEMP% 解压 1.17 GB**，强杀不留清理
    → 必须 per-launch 重定向 TMP/TEMP（codeg 实测一位用户 7 天漏 107.70 GB）；
  ②`session/new` 前必须把 `auth.type` 写进 `<GEMINI_HOME>/antigravity-acp/settings.json`，
    否则直接 `-32000 Authentication required`；
  ③OAuth 登录走 agent 自己的 loopback 浏览器流（阻塞最长 300s），ACP 顶层 `authenticate`
    请求驱动；无头环境要「刮 URL + 用户贴回 redirect」的完整机制（codeg 2200 行）；
  ④安装是 Google CDN 的 zip **目录树**（server + 必需兄弟 `localharness_external`），
    单文件拷出 =「假成功」（握手正常、agent 是废的）——Marionette 现在只有 npm 安装器。
- **协议面很省心**：initialize 实测广告 `loadSession`、session list+resume、image/audio/embeddedContext
  prompt、MCP http+sse；模型/思考档/会话模式（default/auto_edit/yolo）**全是标准 configOptions**，
  composer 选择器不需要 per-agent 代码。历史轨迹是 SQLite + 手写 protobuf 子集（可后置）。

---

## 一、产品与分发面

| 维度 | 事实 | 证据 |
|---|---|---|
| 产品 | Google 的 agent-first IDE；2026-05 起 Gemini CLI 并入 Antigravity CLI | 官网/官方博客（见文末链接） |
| 我们要的东西 | IDE 内置的 ACP server `agy_acp_server`，可独立于 IDE 运行 | codeg 整个接入就是它 |
| 分发 | Google 自家 CDN zip，六平台；**不是 npm** | `acp/registry.rs:8682-8712` |
| pin 版本 | codeg pin **1.5.0**（1.4.3 起 ACP registry 才带 sha256，codeg 自己 pin 仍是 `None`） | `registry.rs:8667-8668`、`:8573` |
| URL 模板 | `https://dl.google.com/agy-extensions/releases/<os>/agy-acp-server-<version>-<target>.zip` | 同上 |
| 改名史 | 1.0.0 是日期 build id（`agy_acp_server_20260818_01_RC01`）；1.0.0–1.1.x URL 带中缀 `agy-acp-server-agy_acp_server_<v>-<t>.zip`；1.2.0 起去中缀 | `registry.rs:8494-8510` 注释 |
| 启动文件 | Unix `agy_acp_server.par`（内嵌 CPython 的编译二进制）/ Windows `agy_acp_server.exe` | `registry.rs:8714-8728` |
| 必需兄弟 | `localharness_external(.exe)`——server exec 的 Go harness；缺了**只 log "Localharness not found."**，握手正常但 agent 废 | 同上 + `registry.rs:8466-8481` 注释 |
| 平台参数 | Linux 传 `--uid=`（absl/InitGoogle root 降权 flag，root 不传掉到 nobody）；**Windows 一个参数都不传**（未知 flag 是硬启动错误） | `registry.rs:222-226`、`:8669-8680` |
| 版本演进 | 1.4.0 加 `thinking_level` 分离选择器 + `_meta.antigravity`（tier/systemPrompt.append）；1.4.1 Agent Platform 默认 `gemini-4-argon-high`；1.5.0 默认 `gemini-4-carbon-medium`、`client_view_file` 限 800 行但整读上限 16 MiB | `registry.rs:8538-8563`、`:8660-8666` |

数据落点（与 Gemini CLI 同居但目录语义**不同**，这是第一个坑）：

```text
$GEMINI_HOME（缺省 ~/.gemini；指 .gemini 目录本身！）
└── antigravity-acp/
    ├── settings.json            auth.type + gcp.{project,location}（load-bearing）
    ├── acp_token.json           OAuth 个人档 token（oauth-business 是 acp_business_token.json）
    ├── trusted_workspaces.json  工作区信任列表
    └── conversations/<id>.db/.meta   轨迹（SQLite + protobuf，见 §五）
```

- `GEMINI_HOME` **指向 `.gemini` 目录本身**；Gemini CLI 的 `GEMINI_CLI_HOME` 指父目录再拼
  `.gemini`。空串视为未设；`~` 前缀要展开（codeg 曾用裸 `PathBuf::from` 造出字面 `~` 目录的真
  bug，`parsers/antigravity.rs:96-101` 注释）。
- macOS 默认把凭据写 login keychain，`AGY_ACP_FORCE_FILE_STORAGE=1|true|yes`（大小写不敏感）
  强制文件存储（`antigravity_login.rs:1591-1600`）。Windows 无此问题，但设置了可让凭据路径可预测。

## 二、认证全景

四个 `auth.type`（`connection.rs:367-372`）：

| method | 凭据来源 | 档位 |
|---|---|---|
| `oauth-personal` | loopback 浏览器 OAuth → `acp_token.json` | **免费档**（默认；codeg sync 的缺省值） |
| `oauth-business` | 同上 → `acp_business_token.json`；**额外**读 settings.json 的 `gcp.{project,location}` 做 license resolution（Gemini Enterprise，**只认 settings.json 不认 env**） | 企业订阅 |
| `gemini-api-key` | env `GEMINI_API_KEY` | API 计费 |
| `agent-platform` | env `GOOGLE_API_KEY`，或 `GOOGLE_CLOUD_PROJECT`+`GOOGLE_CLOUD_LOCATION`（settings.json gcp 块作 per-value fallback）；key 一旦设置 server 端压制 project/location | Vertex/Agent Platform |

- `vertex-ai` 是 `agent-platform` 的 pre-rebrand 别名（server 接受，codeg 不写）；
  1.3.0 起 settings.json 还接受 `gateway` 值，codeg 不提供（`registry.rs:8519-8520`）。
- **settings.json 同步是 load-bearing**（`connection.rs:509-590` `sync_antigravity_settings_file`）：
  没有 `auth.type` 时 `session/new` 直接 `-32000 Authentication required`。codeg 在 **spawn 之前**
  做读-改-写、只动 `auth.type`/`gcp.project`/`gcp.location` 三个 key、fail-closed（解析不了就不碰）、
  失败只警告不阻断 launch。**server 认 Hjson、codeg 只认 strict JSON**——手写注释的文件落入
  give-up 分支（`AntigravityAuthType::Unreadable` 刻意区别于 `Absent`：「server 能读而 codeg 不能
  读的文件是存在的」，`connection.rs:834-843`）。写入走同目录临时文件 + rename，先 canonicalize
  解 symlink（Stow/chezmoi 用户）。
- **env 防串档**（`apply_antigravity_env_policy`，`connection.rs:446-470`）：面板记录了 method 后，
  `GEMINI_API_KEY / GOOGLE_API_KEY / GOOGLE_CLOUD_PROJECT / GOOGLE_CLOUD_LOCATION` 四个变量里
  **只有所选 method 消费且面板实际存了非空值的才保留**，其余置空串（spawn 层据此 `env_remove`）——
  防 shell/容器里继承的旧凭据静默顶替用户选择（`:406-445` 大注释）。

## 三、无浏览器登录（antigravity_login.rs，2224 行）

机制（模块头 `:1-100` 就是完整设计文档，此处摘要）：

- **问题**：两个 OAuth method 是唯一能到免费档的通道，而它们靠 agent 进程自己起的一次性
  loopback HTTP server（`127.0.0.1:<随机端口>`）+ `webbrowser.open(auth_url)` 完成授权，阻塞最长
  300s。无桌面机器上 `webbrowser.open` 返回 `False` **不抛错**，agent 的"could not open a browser"
  日志永远不出现——用户看到的是会话挂五分钟然后
  `Onboarding failed: Timed out waiting for the authentication flow to complete.`。
- **两个事实构成解法**：① agent 会把授权 URL 打印出来（`Open the following link to authenticate
  the ACP server: <url>`）——**打在哪条管道是版本细节**（1.0.0 打 stdout、1.1.1 挪 stderr），
  所以 stdout/stderr **每行都查**（`AuthUrlSink` 是共享 oneshot 槽，先到先得；
  stderr 的 offer 在截断/脱敏**之前**做，否则 ~510 字符的 consent URL 会被 `MAX_LINE_BYTES`
  切半，`read_agent_stderr:1505-1515`）；stdout 情形要 `PYTHONUNBUFFERED=1`（CPython 管道块缓冲），
  unix 上把 `BROWSER` 指向 `/usr/bin/true`（防 lynx/w3m 被当 GenericBrowser 并 `wait()` 卡死 ACP
  管道）。
- **驱动方式**：一次性子进程，跑 ACP `initialize` → `authenticate`（参数只有
  `{"methodId": <id>}`，**没有 authMethods 列表**——方法集合是 initialize 时 agent 自己广告的）。
  子进程环境用与真实 launch **逐字节相同**的 `antigravity_launch_env`（`GEMINI_HOME` 决定 token
  写哪、`AGY_ACP_FORCE_FILE_STORAGE` 决定文件还是 keychain）。URL 与 authenticate 响应赛跑
  （`tokio::select! { biased; … }`），产出 `Url / Authenticated（缓存 token 命中 already_signed_in）/
  Failed / Gone（进程死）/ TimedOut`。
- **代为重放**：loopback server 接受**任何人**的单次 redirect。用户在任意浏览器完成 consent 后
  落在打不开的 `http://127.0.0.1:<port>/?state=…&code=…` 页，把地址贴回来；codeg 用
  `reqwest::Client::builder().no_proxy().redirect(Policy::none()).timeout(20s)` 发一个 GET——
  **loopback 绝不过代理**（中文用户常挂代理是实锤前提）、**不跟重定向**（防响应方把请求引出
  loopback）。请求目标完全由捕获的 `redirect_uri` 重建，端口必须纯数字（
  `127.0.0.1:8080@evil.example` 在 RFC 3986 userinfo 规则下 host 是 `evil.example`，
  `is_loopback_redirect:1295-1304`）。贴入值接受三种形状：完整 URL / 裸 query / 光秃秃的 code
  （`4/0A…` 开头足够独特）。`state` 不匹配的拒绝**不消耗尝试**；用户按 deny 的 `error` 参数
  **转发给 agent**（"the agent needs to hear it to stop waiting"）。
- **槽状态机**：全局单例槽（凭据 per-`GEMINI_HOME`，两个并发登录会赛跑到同一个 token 文件）；
  `Starting / Waiting / Finishing` 三态 + generation 计数防慢 start 装进过期槽；`FINISH_BUDGET`
  230s 防被 drop 的 future 永久占槽。
- **超时链**：INITIALIZE_WAIT 60s（PAR 里的 CPython 冷启动要解压整个解释器）/ URL_WAIT 90s
  （agent 自己等 300s，90s 还没出现就不会来了）/ AUTHENTICATE_WAIT 180s（token exchange +
  onboarding + Enterprise license resolution）/ REDIRECT_WAIT 20s。进程清理：drop stdin（EOF 是
  ACP server 自己的关机信号）→ 1.5s 优雅 → SIGTERM → 2s → SIGKILL。
- **登出**走 agent 自己的 ACP `logout`（先查 `agentCapabilities.auth/logout` 广告，协议要求 MUST
  NOT 未广告就调），**不删文件**——凭据存哪是 agent 内部规则决定，它的 clear 会同时清
  keychain 与文件两份；logout 对 api-key 方法返回 `{}` 但什么都没清（1.1.1 实测），所以 codeg
  只对两个 OAuth 方法提供登出按钮。登出前后要 `lock_out_new_connections()` + 按类型断连：
  **活进程的内存缓存 token 会在刷新时把被删的账号写回文件**。

**对 Marionette 的启示**：Marionette 是 Windows 桌面产品，`webbrowser.open` 本身能工作——
完整贴回机制是为无头 Linux 服务的。桌面最小版只需：起一次性子进程（或复用会话连接）发
`authenticate`，agent 自己开浏览器，等 `authenticate` 响应即可；URL 刮取留作兜底展示。
详见 plan §M1。

## 四、运行面：launch env 与 scratch（Windows 红线）

- **scratch 重定向是生存条件**（`scratch_dir.rs:1-46` 模块头，全部实测数字）：
  - Windows 构建是 PyInstaller **onefile**，archive 每次 startup 解压 **1.17 GB**
    （8261 个 TOC 项，1192.3 MB `b` entries + 8.7 MB PYZ）。codeg 用 `kill_tree` 结束进程
    （Windows 下无条件终止）→ 优雅清理永不执行。**一位用户 7 天积累 113 个孤儿
    `%TEMP%\_MEI*` 共 107.70 GB，把系统盘吃满。**
  - 杠杆完全在宿主侧：bootloader 经 `GetTempPathW` 定解压根（读 `TMP`→`TEMP`→`USERPROFILE`），
    重定向这三个变量就把无界泄漏变成自己可删的目录。Marionette 的 stop 同样是杀树
    （KillJobGuard），**不重定向 = 同款泄漏**。
  - **为什么 Windows 只对自解压 agent 隔离**（#1025 教训）：`TMP` 不会留在拿到它的进程树里。
    Git for Windows 默认 `/etc/fstab` 把 `/tmp` 挂成 `usertemp`，MSYS2 运行时用「用户第一个启动
    的 MSYS 进程」的 `GetTempPathW` 解析并写进 per-user 共享内存——agent 跑的 bash 若是用户的
    第一个 MSYS 进程，**用户自己的 Git Bash 的 /tmp 也会被借走**指向 launch 目录，目录被回收
    后用户每条命令都报 `bash.exe: warning: could not find /tmp`。所以只给真正自解压的 agent
    （Antigravity）开隔离，其余 agent 不动 TMP。
  - macOS 同 bug 小两个数量级（每次 ~172 KB 只读资源文件），所以非 Windows 平台对所有 agent
    隔离。
  - 隔离目录生命周期：register-before-mkdir / unregister-after-gone，锁下 diff 锁外删；
    macOS 版有 `r-xr-xr-x` 只读 CA bundle，删除要带只读清除重试。
- **存量回收**（`temp_reclaim.rs`）：隔离机制上线**之前**已漏的产物，用户触发 scan→确认→删。
  可回收判据：Windows `_MEI` 前缀目录；任意平台文件名含 `__py_binary_resource_`
  （非 Windows 版每次启动丢的 `cacert__py_binary_resource_agy_acp_server__*.pem`）。
  防误删三重：15 分钟 MIN_AGE；Windows 用 `dwShareMode=0` 打开被映射文件（.dll/.pyd/.exe/
  base_library.zip）做**非变异**存活探测（rename 探测在 NTFS 上是错的）；reclaim 对前端传回的
  每个 path 重校验 `parent == temp root`（防 `..\` 逃逸）+ 名字形状 + 年龄 + idle。
- `antigravity_launch_env`（`connection.rs:802-817`）本身：registry env 为空，`GEMINI_HOME` /
  `AGY_ACP_FORCE_FILE_STORAGE` 都来自用户 per-agent env 或继承环境；该函数的存在意义是让登录
  子进程拿到与真实 launch 逐字节相同的环境。**codeg 不隔离 GEMINI_HOME**——默认就用用户的
  `~/.gemini`（三态解析：runtime_env 非空用之 / 空串显式移除回落 `~/.gemini` / 缺失继承 codeg
  进程自己的值，`connection.rs:626-678`）。

## 五、轨迹解析（parsers/antigravity.rs，2362 行）

- **布局**：`<GEMINI_HOME>/antigravity-acp/conversations/<sessionId>.db` + 同名 `.meta` sidecar。
  `.db` 是单表 SQLite：`steps(idx, step_payload)`，payload 是序列化的 `gemini_coder.Step`
  protobuf；`.meta` 带 `cwd`（无 fallback）与 `mode_id`，**没有标题**——server 自己只会合成
  `Session <前 8 字符>`，标题从第一条 user prompt 取。
- **0 字节 DB 合法**：新会话先落 0 字节文件，Go harness 之后才写 schema——
  `no such table: steps` 是常态不是损坏。打开策略 read-only → WAL 恢复错误降级 read-write。
- **手写 proto 子集**（prost）：真实 `Step` ~130 字段、oneof `step` 枚举 118 值；codeg 只声明
  转录需要的十几个字段（真实字段号）：
  - `4 status`（只查 `STEP_STATUS_ERROR=7`）、`5 metadata`、`31 error`、`24 error_message`；
  - `19 user_input`（query/user_response/items）、`20 planner_response`
    （response=1 文本 / thinking=3 思考 / tool_calls=7 调用**公告**）；
  - `13/14/15/23/28/40/42/47/98/116/154` 各工具执行臂（grep_search/view_file/list_directory/
    write_to_file/run_command/read_url_content/search_web/mcp_tool/file_change/agency_tool_call/
    ask_question）。
  - `CortexStepMetadata`：`created_at=1` / `tool_call=4`（**公告↔执行关联键**）/ `completed_at=8` /
    `model_usage=9` / `model_info=24`；`ModelUsageStats`：input=2/output=3/cache_write=4/cache_read=5；
    `ModelInfo`：model_name=8/display_name=20。
  - **分派只看哪个 oneof 臂有值，从不看 `type` 枚举**（臂是自描述的，枚举改号是上游的自由）。
- **四个独有机制**（对比 Marionette 的 zcode 结构化表 / grok JSONL）：
  1. **调用/结果两段式**：`planner_response.tool_calls` 是公告，执行步靠
     `metadata.tool_call` id 关联；`announced` 去重防双卡；**未应答的调用要 settle**——
     磁盘上的步不可能"还在跑"，合成空 ToolResult，否则渲染永不收敛的 spinner。
  2. **MCP sentinel**：所有 MCP 调用叫 `call_mcp_tool`，真实身份在 PascalCase envelope
     `{"ServerName","ToolName","Arguments"}` 里；镜像 server 自己的 unwrap：改名 `<server>_<tool>`、
     参数包 `{"arguments":{…}}`、按 `MCP_PROMPT_KEYS` 抽人话句子；超 8k input 上限时先丢派生的
     `prompt` 副本再截断。
  3. **agency_tool_call**：Go localharness 自己跑的工具包在 `google.protobuf.Any` 里；
     `type_url` **最后一段**精确等于 `antigravity.localharness.ToolResponse` 才解码
     （后缀匹配会让 `evil.antigravity.localharness.ToolResponse` 也过）。同一工具可能走专属臂
     **或** Any 信封两条路。
  4. **usage 口径**：per-step `model_usage` **求和进 turn**；窗口占用只取**最新一步的
     input 侧**（`input + cache_read`）——"每步都重发整个前缀，求和会把缓存前缀重计一次每步"。
     与 Marionette usage v2「本轮自己的事件」口径天然同构。
  - 时间戳：`created_at`/`completed_at` 分离（长命令会隔几分钟）；**user 步先 flush 挂起中的
    assistant turn**（否则 9 小时空闲会被算成响应时长）。
- **测试**（`tests/antigravity_trajectory.rs`）：fixture **不是** decoder 自己写的（防同义反复），
  是 Python protobuf runtime 驱动从 `agy_acp_server.par`（build `20260818_01_RC01`）里抽出的
  FileDescriptorProto 生成的——断言覆盖 user/assistant turn、未公告 run_command 合成调用卡、
  MCP 解包、harness Any、**未建模字段（tag 148/155）按 wire type 跳过**、usage 数字。

## 六、capability / wire 层（live 会话）

- **initialize 实测广告**（`registry.rs:8457-8464`）：`loadSession`、sessionCapabilities list+resume、
  **image/audio/embeddedContext** prompt、MCP http+sse，`agentInfo.name = "antigravity-acp"`。
  模型、thinking level（1.4.0 起）、session mode（default/auto_edit/yolo）全是标准 configOptions
  ——**composer 选择器零 per-agent 代码**。
- **模型变体陷阱**（`connection.rs:12333-12371`）：1.4.0 的 `model` selector 只列 family
  （`gemini-3.1-pro`），变体（`gemini-3.1-pro-high`）是 1.3.0 的平铺 id；**只有 server 知道
  映射**——codeg 对保存的旧变体 id 原样重放（`send_capturing_models` 捕获平铺 `models` 列表）。
  1.5.0 Agent Platform 默认 `gemini-4-carbon-medium`、Gemini 4 Carbon 排第一。
- **MCP**：`agent_reads_own_mcp_config` 含 Antigravity——用户 MCP 不从 wire 转发，因为 server
  自己读 `<GEMINI_HOME>/config/mcp_config.json` 并**按名 merge（wire 赢）**；codeg-mcp 伴生进程
  仍走 wire 注入（转发其实不会双挂，只是"一个 server 经两条通道定义"是噪音，跳过）。
- **live rawInput 折叠**（`connection.rs:20702-20736`）：1.2 起 Antigravity 会把 MCP 工具的
  `arguments` 平铺一份到顶层；`_meta.is_mcp_tool_call == true` 且旁键与 `arguments` 内逐字相等时
  折回 `{"arguments":…, "prompt":…}`，防卡片重复渲染/超 TOOL_INPUT_CAP。
- **native 问答桥**：`session/request_permission` 的 `interaction_<hex>` 桥接成问答卡
  （`try_bridge_antigravity_ask:10785-10853`），已答卡片记账 + 丢弃 server 随后的
  "Response received" 关闭帧防覆盖（`record_antigravity_answer_card:21216-21248`）。
- **fs 读**：`read_response_limit` 对 Antigravity 提到 **16 MiB**（其余 agent 2 MiB）——1.4.5 起
  `client_view_file` 整文件读后自己切片（`file_system_runtime.rs:323-330`）。前提是宿主实现了
  `fs/readTextFile`；Marionette 的 `build_client_capabilities_json` **刻意关闭**该能力
  （`agent_registry.rs:236-241`），关闭后 agent 的行为需真机验证（大概率回落自己的文件工具，
  无阻断，见 plan 开放问题）。
- **slash 命令**：只广告内建 `/plan` `/logout` `/usage`，从不广告 skill（技能进技能目录、
  由模型侧点名）；`/plan` 会被 server 截走，模型看不到——技能菜单要与命令去重。

## 七、坑清单（Marionette 视角汇总）

| # | 坑 | 后果 | 对策 |
|---|---|---|---|
| 1 | %TEMP% 自解压 1.17GB/次，杀树不清 | 磁盘吃满（实测 7 天 107GB） | per-launch scratch（TMP/TEMP/TMPDIR 重定向）+ 树死后回收；只对此 agent 开（MSYS2 /tmp 污染） |
| 2 | 缺 `auth.type` 的 settings.json | session/new 直接 -32000 | spawn 前同步 settings.json（read-modify-write、fail-closed、strict JSON vs Hjson） |
| 3 | 继承的 GEMINI_API_KEY 等旧凭据 | 静默串档（免费档变计费档） | env policy：非所选 method 的凭据变量全清 |
| 4 | 单文件拷出安装（漏 localharness_external） | 「假成功」：握手正常 agent 废 | dir-tree 安装 + required_siblings 校验 + 读侧缓存守卫 |
| 5 | URL 打印管道会漂移（stdout↔stderr） | 登录永远超时无提示 | 双路每行刮 marker；stderr 在截断前刮 |
| 6 | loopback redirect 过系统代理 | 登录死在代理上 | no_proxy + 不跟重定向 + 端口纯数字校验 |
| 7 | GEMINI_HOME vs GEMINI_CLI_HOME 语义相反 | 读写到错误目录 | 解析函数单一来源；`~` 展开；空串=未设 |
| 8 | `--uid=` 在 Windows 是未知 flag | 硬启动错误 | Windows 零参数；仅 Linux 传 |
| 9 | 0 字节 DB / `no such table` | 误判损坏 | 当空会话读 |
| 10 | 未应答工具调用不 settle | 永久 spinner | 关联键去重 + 磁盘步合成空结果 |
| 11 | 模型变体 id 与 family 选择器两代并存 | 保存的模型 id 失效 | 旧 id 原样重放；不猜映射 |
| 12 | release notes 不可信（URL 两次改名都不在 notes） | 升级链接 404 | URL 模板解版本，不匹配就不提供升级 |

## M0 真机实测（2026-11-19，本机 Windows x64，agy-acp-server 1.5.0）

zip 从 dl.google.com 下载（105 MB），解压为**扁平两文件**：`agy_acp_server.exe`（58 MB）+
`localharness_external.exe`（155 MB）——与 codeg registry 记录一致。裸 stdio JSON-RPC 探针
（`D:\tmp\agy-m0-probe\`）逐项验证：

| 问题 | 实测答案 |
|---|---|
| 握手 | **即时成功**（无冷启动等待）。`agentInfo = {name:"antigravity-acp", title:"Google Antigravity", version:"1.5.0"}`；capability 面与 codeg 记录逐字段一致（loadSession/image+audio+embeddedContext/mcp http+sse/session list+resume+additionalDirectories/auth.logout/`_meta.antigravity.tier+systemPrompt.append`） |
| authMethods 形状 | 恰好 4 个、按序：`oauth-personal`（"Log in with Google"）→ `oauth-business` → `gemini-api-key` → `agent-platform`；**第一个就是免费档默认**，登录卡无需选择逻辑 |
| 无 settings.json 的 session/new | `-32000 "Authentication required"`，`data.message` 自带全部方法的文档（含 env 变量名）——可直接转述给用户。**该文案还暴露第五个方法 `gateway`（`AGY_LLM_GATEWAY_URL` 自定义 AI API Gateway）**——引擎跨用的潜在钩子（开放问题 6 的新线索） |
| settings.json 写入后 | `auth.type:"oauth-personal"` → **session/new 直接成功**（不等登录），OAuth 在后台自动触发：stderr 打印 marker 行（`Open the following link to authenticate the ACP server: <url>`）+ `webbrowser.open` 自动弹浏览器。本机浏览器闭环后 token 落 `acp_token.json`（refresh_token + scopes cloud-platform/userinfo.email/aicode + project `aicode-consumers`） |
| 端到端 | prompt → 精确回复 ✅；免费档默认模型 **`gemini-3.8-flash`**；stderr 可见内部凭据注入代理按 `gemini-3.8-flash-high` 路由（family+thinking_level 组合 id 实锤） |
| configOptions | 全标准 select：`model`（免费档 1 项）/ `thinking_level`（low/medium/high，cur=high，cat=thought_level）/ `mode`（default/auto_edit/yolo，cat=mode）。响应另有平铺 `models` 与 `modes` 键 |
| **turn/complete** | **不作为通知发**——但 `session/prompt` 请求的**响应**（result）就带 `stopReason:"end_turn"` + 完整 usage（totalTokens/inputTokens/outputTokens/thoughtTokens/cachedRead/cachedWrite）。Marionette acp.rs 早有「prompt 响应即回合完成」的标准处理（`emit_turn_complete`，acp.rs:3705-3725），**回合生命周期与 usage 记账天然走通**，不需要 agentReentry 兜底（初判「无 turn/complete 会缺 usage 记账」是错的——通知与响应是两回事） |
| %TEMP% 泄漏 | **1.5.0 每次启动解压 ~150 MB `_MEI*`**（codeg 记录的 1.17 GB 是老版本），且优雅关闭后仍残留——TMP 重定向红线维持，量级缩小 |
| fs.readTextFile=false | **agent 用自己的 `view_file` 工具读文件**（tool_call 的 rawInput 带 `file_path/start_line/end_line`，end_line=799 与 codeg 记录的 800 行上限吻合），读出内容精确。**Marionette 无需为此 agent 打开客户端 fs 能力** |
| 额外写盘 | 会话启动往 `~/.gemini/antigravity/bin/webm_encoder.exe` 装二进制（注意目录名是 `antigravity` 不是 `antigravity-acp`）；conversations/ 落 `<uuid>.db`+`.meta`（与 parser 布局一致） |
| stderr URL 管道 | 1.5.0 打 **stderr**（与 codeg「1.1.1 起 stderr」记录一致）；stdout/stderr 双路刮 marker 的策略维持 |

M0 结论：**四个核心开放问题（握手/settings/OAuth 闭环/文件读）全部通过，无阻断项**。
剩余开放问题（IDE 凭据共享、限额信号、trusted_workspaces 交互）不阻塞 M1。

### M1 真机追加发现（2026-11-19，隔离实例 E2E）

- **会话内 E2E 四轮全通**（`MARIONETTE_CHIEF_PROBE` 驱动 start→prompt→idle→stop）：
  settings 同步分支在进程内执行、caps 快照 `models:1/modes:3/gemini-3.8-flash`、
  回复精确命中指令 token、stop 干净。
- **scratch 重定向实测生效**：服务解压 42M→97M 落进
  `~/.marionette/scratch/antigravity/<sessionId>`，stop 后连空父目录一起清掉。
- **新坑：探测路径执行 = 泄漏**——`all_version_info`（agent 菜单每次打开都会跑）对每个
  agent 执行 `--version`；agy 的 PyInstaller onefile **即使 `--version` 秒退也先整包解压
  ~90-100MB**，实测每次开菜单漏 2–3 个 `_MEI*` 目录。修复：`installed_version` 对
  `is_server_command()`（stem 精确匹配 `agy_acp_server`，大小写不敏感、剥路径与可选
  `.exe`）直接返回 None，永不执行；守卫生效后真实 `%TEMP%` **零新增** `_MEI`。
- **终态**：全量 `cargo test --bin marionette`（675 项）+ tsc + build 全绿。

### M2/M3 真机实测（2026-11-19）

- **安装器 live 实测**（`cargo test live_install -- --ignored`，测试自隔离 USERPROFILE）：
  105MB zip 下载 + 系统 tar.exe 解压 + required_siblings 校验 + 落位 **2.78s 全通**，
  幂等重装秒回。sha256 用现成 `sha2` 依赖（引擎下载同款）；registry 带 sha256（1.4.3 起），
  校验失败报 mismatch 不落盘。
- **升级链**：`version_info` binary 分支——installed=安装目录名、latest=ACP registry
  （`distribution.binary[platform].archive`，实测 shape 与 codeg 记录一致）、URL 模板
  （dl.google.com + `agy-acp-server-` + `-{platform}.zip`）不符即不提供升级。
- **真 bug 一枚（已修）**：`gemini_home()` 在 `GEMINI_HOME` 未设时漏了 `.join(".gemini")`——
  E2E 期间一直显式传着 GEMINI_HOME 所以没暴露，真库 oracle 一跑就现形（解析器 0 会话）。
  教训：**环境变量覆盖的测试路径 ≠ 默认路径**，两者都要有 oracle。
- **解析器真库 oracle**：6 个真实会话全列（cwd 正反斜杠都对、WAL 库读通、0 字节库=空），
  事件投影 user/assistant/thought/tool 正确（标题取首条 prompt）。
- **usage 回填真库 oracle**：5 会话（0 字节跳过），input/output 数字与 ACP 侧 tee 记录
  精确吻合（13041/38 vs totalTokens 13079=input 13041+output 38）——**四桶口径对齐**。
- **certutil 教训（已换 sha2）**：曾用 certutil 算哈希，日语系统输出无空行+本地化文案
  打破位置解析——系统工具输出的解析要本地化无关，最终直接用 sha2 crate。
>>>>>>> f691dc32 (feat(antigravity): M2 自动安装+升级链 / M3 usage 回填+历史导入+存量回收——接入全量收官)
## 附：官方来源

- <https://antigravity.google> · <https://antigravity.google/blog/introducing-google-antigravity>
- Google Cloud「Choosing Antigravity or Gemini CLI」（2026-02）：
  <https://cloud.google.com/blog/topics/developers-practitioners/choosing-antigravity-or-gemini-cli>
- 「Transitioning Gemini CLI to Antigravity CLI」（2026-05-19）：
  <https://developers.googleblog.com/an-important-update-transitioning-gemini-cli-to-antigravity-cli>
- ACP 官方 registry（`antigravity-acp` 条目，升级链的数据源）：
  <https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json>
