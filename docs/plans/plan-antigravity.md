# 方案：接入 Google Antigravity（agy_acp_server）

状态：**M0–M3 已全部落地（2026-11-19）**——M0 真机核查、M1 最小可用、M2 自动安装+升级链、
M3 usage 回填+历史导入+存量回收全部实现并实测；**登录按钮**（issue #14）已补——Sign in 驱动
一次性 server 进程跑 `initialize → authenticate`（`antigravity_login.rs`），真机 oracle 全通。
**唯一刻意未做**：「贴回式无头登录」（OAuth redirect URL 贴回）——Windows 桌面产品浏览器必在，
该机制属无头 Linux 场景；出现真实需求时按 research §三移植 codeg 完整登录模块。
事实依据与实测记录在
[`../research/research-antigravity.md`](../research/research-antigravity.md)（codeg v0.46.0 深挖 + M0/M1/M2/M3 真机）。

## 0. 为什么接、接的是什么

- 接的是 **`agy_acp_server`**——Antigravity 随包分发的 first-party ACP server（Google 官方，
  dl.google.com 六平台 zip），不是 IDE 本身，也不是 Gemini CLI。协议就是 ACP，和现有
  claude/codex/grok 一条管线。
- **免钥匙免费档**：`oauth-personal` 一个 Google 账号到免费额度（5 小时窗口重置），全程不需要
  API key。对 Marionette 基本盘（不想折腾钥匙的用户）是刚需型卖点；模型面是 Gemini 3.x/4.x。
- **与 2026-10-20 roster cut 裁掉的 `gemini` 不是一回事**（`models.rs:198-205` 注释）：那是
  Gemini CLI 的 npm 适配器，裁由是「无跨用面或用户面小，装不上的条目比没有更伤」。Antigravity
  的差异：①免费档零钥匙门槛；②Google 官方 server（长期维护预期），分发是二进制 zip；
  ③capability 面标准（loadSession/resume/image prompt/MCP http+sse 全有）。
  **如果 M0 真机验证不过关，就按同一条裁由放弃，不硬上**。
- 非目标：不装 IDE、不做 Antigravity 特有的 Manager/artifact UI、不做引擎跨用路由进
  Antigravity（api-key method 是否支持 base URL 覆盖未知，见开放问题）、不做无头贴回登录
  （Windows 桌面有浏览器；留 M3 视需求）。

## 1. 阶段划分

### M0 — 真机事实核查（半天～一天，手动，不改代码或只挂临时分支）

用现有「自定义 agent」机制（`custom_agents.rs`）把 `agy_acp_server.exe` 加进来手工跑通，
回答四个只有真机才能回答的问题（全部记回 research 文档）：

1. **握手与 capability 形状**：initialize 广告的 authMethods 长什么样（id 列表）；model /
   thinking_level / mode 三个 configOptions 在我们 `CapabilitySnapshot`（`acp_capabilities.rs`）
   的通用管线下长什么样——**尤其确认 `gemini-3.1-pro-high` 这类变体 id 不会被
   `acp_capabilities.rs:63-131` 的 model[effort] 剥壳逻辑误伤**。
2. **fs.readTextFile=false 的行为**：我们 `build_client_capabilities_json` 刻意关闭该能力
   （`agent_registry.rs:236-241`）。codeg 给 Antigravity 开 16 MiB 读上限说明它走
   `fs/read_text_file`；关闭后 `client_view_file` 是回落自己的文件工具还是报错？
   （预期：回落、无阻断；若报错 → M1 给该 agent 单开 readTextFile。）
3. **登录最小闭环**：手动把 `auth.type:"oauth-personal"` 写进
   `%USERPROFILE%\.gemini\antigravity-acp\settings.json`，正常起会话触发
   `Authentication required`，确认 agent 自己弹浏览器 + loopback 完成后 token 落
   `acp_token.json`（即验证「桌面环境下 agent 自带的浏览器流本来就 work」）。
4. **泄漏实锤与 scratch 验证**：跑一次会话后看 `%TEMP%\_MEI*`（预期出现 ~1.17 GB 目录），
   杀进程确认不清理；然后手动设 `TMP/TEMP` 指到别的目录再跑，确认解压跟随。

### M1 — 最小可用（roster + 设置同步 + 登录 + scratch 隔离）

目标：用户手动下载 zip 解压、放到 PATH（或 custom agent），Marionette 负责其余一切。

1. **roster 条目**（`models.rs:135+` `AgentConfig::defaults()`）：
   `id="antigravity"`、`label="Google Antigravity"`、`command="agy_acp_server"`（Windows 下
   解析到 `agy_acp_server.exe`，`resolve_spawn_command` 已能处理裸 exe）、install =
   `AgentInstallSpec::manual`，文案给 dl.google.com 下载页与「解压后整个目录上 PATH，别只拷
   exe」的提示（required_siblings 语义）。
2. **harness_meta**（`agent_registry.rs:84+`）：
   - `requires_commands: ["agy_acp_server"]`（preflight 现成）；
   - `distribution: Manual`（grok 同款）；
   - `launch_env`：`AGY_ACP_FORCE_FILE_STORAGE=1`（Windows 本就文件存储，设置它让凭据路径
     可预测、探测简单）；**不放 GEMINI_HOME**（默认跟随用户 `~/.gemini`，与 codeg 同策）；
   - MCP：走默认 wire 注入（http+sse 都收）；**不**进 `agent_reads_own_mcp_config` 类特判
     （那是 codeg 的用户-MCP 聚合体系，我们没有）。
3. **settings.json 同步**（新函数，acp.rs spawn 前、仅 antigravity 分支调）：
   - 写 `<GEMINI_HOME>/antigravity-acp/settings.json` 的 `auth.type`（缺省
     `oauth-personal`）；gcp 块 M1 不做（免费档用不到）。
   - 形状抄 codeg `sync_antigravity_settings_file`（`connection.rs:509-590`）：read-modify-write
     只动一个 key；**fail-closed**（读不了/解析不了就不碰，返回 Skipped 供诊断）；
     **strict JSON 写入**（server 认 Hjson，我们读不懂的一律不动——用户手写注释的文件归用户）；
     同目录临时文件 + rename。
   - GEMINI_HOME 解析单一来源：新 `antigravity_support.rs`（解析三态 + `~` 展开 +
     `antigravity-acp` 子目录常量），登录探测/设置同步/后端解析共用，**别在两个文件里各写一份**
     （`GEMINI_HOME` 指 `.gemini` 本身、与 `GEMINI_CLI_HOME` 相反，是本接入第一坑）。
4. **登录**：会话连接内驱动，不起一次性子进程：
   - initialize 时解析并留存 `authMethods`（AcpService 侧加一个 per-connection 字段）；
   - `session/new` 返回 `-32000 Authentication required` 时，会话置一个「待登录」态并发事件，
     前端出登录卡（权限卡同族 UI；oauth-personal 单按钮，多 method 时列表）；
   - 用户点登录 → Rust 在**同一条活连接**上发 `authenticate`（`{"methodId": …}`，超时 180s，
     可取消）→ agent 自己 `webbrowser.open` 弹浏览器 → 响应到达后**重试 session/new**；
   - 兜底：stdout/stderr 双路刮 `Open the following link to authenticate the ACP server: `
     marker（stderr 在截断前刮；`PYTHONUNBUFFERED=1` 进 launch env），URL 以可点链接出现在
     登录卡里（浏览器没弹时用户自己点）；
   - `start_agent_login` / `probe_agent_auth`（`commands.rs:1943/:1240`）加臂：probe =
     `acp_token.json` 存在且新于 N 天 + settings.json 有 auth.type；login = 指到上面的会话内
     流（antigravity 没有 CLI login 命令，`CREATE_NEW_CONSOLE` 那条路对它不适用）。
   - 为什么不用 codeg 的一次性子进程：那是为无头 Linux + 设置面板入口设计的；桌面环境
     agent 自带浏览器流本来就通，会话内重试少一套 spawn/槽状态机。若后续要贴回式登录（M3），
     再移植完整模块（`antigravity_login.rs` 2200 行是现成蓝图，槽状态机/代重放/超时链照抄）。
5. **scratch 隔离（红线级）**：per-launch 建目录（`~/.marionette/scratch/antigravity/<uuid>`），
   spawn env 里 `TMP/TEMP/TMPDIR` 指过去（Windows 先删全部大小写变体防双拼写），会话停止
   （stop/退出/进程死）**树死之后**再删目录；只对 antigravity 开（MSYS2 `/tmp` 污染，见
   research §四）。 KillJobGuard 已保证树死；删除挂在 stop 路径 + 启动清扫兜底
   （孤儿 scratch 目录按 mtime > 24h 清）。
6. **前端**：`agentAuth.ts` 加条目（probe/login/errorHint「需要 Google 账号，免费档 5 小时
   重置」）；i18n 走 `locales/parts/`（fe-dialogs 或新 fe-antigravity 片段）。**不加
   acpSupplements**（model/thinking_level/mode 全是标准 configOptions，通用管线够用）。

### M2 — 自动安装与升级

1. **dir-tree 下载器**（新模块 `agent_downloads.rs`，或并入现有安装命令层）：
   落 `~/.marionette/agents/antigravity-acp/<version>/<platform>/`；下载 dl.google.com zip →
   解压（复用 PowerShell Expand-Archive 或加 zip crate，与 `import-magpie.ps1` 的解压策略
   对齐）→ 校验 entry（`agy_acp_server.exe`）+ **required_siblings（`localharness_external.exe`
   必须在，缺了报安装失败而不是装完才发现）** → chmod 等价（Windows 无需）。失败整个
   version 目录删除重来；读侧缓存命中同样校验 siblings（防半截安装被当完好）。
   下载走现有代理层；UI 进度与引擎下载同款。
2. **版本与升级**：pin 起步 1.5.0；升级查 ACP 官方 registry（`cdn.agentclientprotocol.com`
   的 registry.json——`acp_catalog.rs` 已在拉，复用）拿 `antigravity-acp` 条目的 version +
   archive URL，**对着 pin URL 模板解版本校验**（`agy-acp-server-<version>-<target>.zip`，
   URL 不合模板 = Google 又改名 = 不提供升级而不是给 404，codeg `latest_release.rs:17-19`
   同策）。semver 比较用现有 `agent_update.rs` 的 `is_newer`。
3. roster 的 install spec 从 manual 切到新的 binary 安装类型；`install_agent`
   （`commands.rs:470-545`）目前只认 npm，加 binary 分支。

### M3 — 增强（按需排期）

- **usage 回填 + 历史导入**：`usage_backfill` 加槽位 + `parsers/` 加 antigravity 解析器
  （prost 依赖；0 字节 DB、未应答调用 settle、MCP sentinel、usage 求和 vs 窗口取末值——
  research §五全有）。 Marionette 的 `Session.native_session_ids` 去重锚点 = ACP sessionId，
  与 `.db` 文件名一致，天然对上。
- **贴回式无头登录**：完整移植 codeg `antigravity_login.rs`（槽状态机 + 代重放 + 三形状
  贴入解析）。仅当出现真实需求（远程桌面/浏览器坏）再做。
- **存量 %TEMP% 回收扫描**：`_MEI*` + `__py_binary_resource_` 判据 + 15 分钟年龄 + 非变异
  存活探测（research §四）；M1 的 scratch 隔离上线前的泄漏不会发生，此条只服务「用户在
  接入前手动跑过 server」的场景，优先级低。

## 2. 红线（改前先记）

1. **scratch 重定向是生存条件**：不重定向 TMP/TEMP，每次会话泄漏 1.17 GB（实测 7 天
   107.70 GB）。删除必须发生在进程树死后（KillJobGuard 之后），启动清扫兜底孤儿。
   **只对 antigravity 开**——给所有 agent 开会把用户 Git Bash 的 `/tmp` 借走（MSYS2 共享
   内存，codeg #1025 实锤）。
2. **settings.json 同步 fail-closed**：读不懂（Hjson/损坏）就不碰、只警告；只动我们拥有的
   key；strict JSON 写。**不发明「删了重建」**——那会吃掉用户手写的 gcp/注释。
3. **env 防串档**：面板/配置确定了 auth method 后，其余凭据变量
   （GEMINI_API_KEY/GOOGLE_API_KEY/GOOGLE_CLOUD_PROJECT/GOOGLE_CLOUD_LOCATION）置空移除——
   继承来的旧 key 静默把免费档变计费档是最贵的错。
4. **Windows 零启动参数**：`--uid=` 只 Linux；Windows 传未知 flag 是硬启动错误。
5. **不猜模型映射**：变体 id（`gemini-3.1-pro-high`）与 family 选择器两代并存，只有 server
   知道映射；保存的旧 id 原样重放，剥壳逻辑（`acp_capabilities.rs:63-131`）不得误伤（M0 验证）。
6. **required_siblings 是安装正确性的一部分**：缺 `localharness_external` 的安装是「假成功」
   （握手正常 agent 废）；读侧缓存命中同样要校验。
7. **登录重放的 loopback 请求绝不过代理、绝不跟重定向**（M3 才会写；现在记下）。
8. **GEMINI_HOME 解析只有一个函数**：三态（非空用之/空串=未设/缺失继承）+ `~` 展开 +
   指 `.gemini` 本身（与 GEMINI_CLI_HOME 相反）——写两份必错。
9. **内存压制批的 trim 不动它的新规**：现有红线「只打 waiting 会话」已覆盖；注意 antigravity
   waiting 会话的 EmptyWorkingSet 会把 1.17 GB 解压页推去 standby，软缺页回热是预期行为，
   不要因此给 antigravity 开 trim 豁免。
10. **若 M0 验证不过 → 放弃接入**，文档记死因；不带着「装不上的条目」进 roster（2026-10-20
    裁员时的定案理由）。

## 3. 改动清单（M1，文件级）

| # | 文件 | 内容 |
|---|---|---|
| 1 | `src-tauri/src/models.rs` | `AgentConfig::defaults()` 加 antigravity 条目（manual 安装文案） |
| 2 | `src-tauri/src/agent_registry.rs` | `harness_meta()` 加分支（requires/Manual/launch_env）；McpWirePolicy 默认 |
| 3 | `src-tauri/src/antigravity_support.rs`（新） | GEMINI_HOME 三态解析、settings.json 同步、token 探测、scratch 目录管理 |
| 4 | `src-tauri/src/acp.rs`（start 内） | antigravity 分支：settings 同步 + scratch env + `PYTHONUNBUFFERED=1`；initialize 留存 authMethods；session/new 撞 auth_required 的待登录态 + `authenticate` 驱动 + 重试；URL 兜底事件 |
| 5 | `src-tauri/src/commands.rs` | `start_agent_login`/`probe_agent_auth` 加臂 |
| 6 | `src/lib/agentAuth.ts` | AGENT_AUTH 加条目 |
| 7 | `locales/parts/fe-*.ts` | 登录卡/安装提示文案（中英） |
| 8 | `src/components/Composer.tsx` 菜单区 | 无需新面：安装/预检走现有 agent 菜单；登录卡走会话流 |

M2 追加：`agent_downloads.rs`（或并入 commands 安装层）、`models.rs` install spec 换型、
`acp_catalog.rs` 复用拉 registry。M3 追加：`usage_backfill/antigravity.rs` + `parsers/antigravity.rs`
+ Cargo.toml 加 prost。

## 4. 测试与回归

- `cargo test --bin marionette antigravity`（新）：GEMINI_HOME 三态与 `~` 展开；settings
  read-modify-write 三态（无文件/有 auth.type/读不懂的文件）；siblings 校验（M2）；
  scratch 目录生命周期（建→注册→树死→删；孤儿清扫只按 mtime）。
- `npx tsx scripts/verify-antigravity.mts`（新，M1/M3 视范围）：agentAuth 表完整性、i18n 键
  双语并集；M3 再加 parser 的 fixture 断言（fixture 用 Python protobuf 从真
  `agy_acp_server.exe` 生成的 db——**不要用自家 decoder 写 fixture**，那是同义反复，
  codeg `tests/antigravity_trajectory.rs:3-19` 的教训原样适用）。
- 真机手册（M0/M1 各一遍）：握手 + 模型菜单三 configOptions；登录全流程（浏览器自动弹；
   拔网/关浏览器时的失败文案）；首会话后 `%TEMP%` 无新增 `_MEI*`；强杀后 scratch 目录被清；
   免费档窗口耗尽时的报错可读；`~/.gemini` 与 Gemini CLI 共存（互不踩目录）。

## 5. 开放问题（M0 逐条回答，答案回写 research 文档）

1. `fs.readTextFile=false` 时 `client_view_file` 的真实行为（回落 or 报错）。
2. Antigravity IDE 已登录用户：IDE 的凭据与 `agy_acp_server` 的 `~/.gemini/antigravity-acp/`
   是否共享（预期**不共享**，server 要自己登录——需在 UI 文案里说清）。
3. initialize `authMethods` 的实际形状与排序（决定登录卡默认项逻辑）。
4. 免费档限额（5h 窗口）有没有可读的余量信号（`/usage` 是 server 的 slash 命令，壳能不能
   拿到结构化数据；拿不到就只在 errorHint 里写静态说明）。
5. `trusted_workspaces.json` 会不会对未信任 cwd 弹交互（`interaction_*` 权限请求）——若是，
   走通用权限卡即可，但要真机确认形状。
6. 引擎跨用：`gemini-api-key` method 是否吃 base URL 覆盖（吃的话跨用桥能给它路由引擎模型；
   不吃就不做，非目标）。
7. 1.17 GB 解压对 HDD 用户的首启动耗时（只影响观感，决定要不要在安装文案里预告）。
