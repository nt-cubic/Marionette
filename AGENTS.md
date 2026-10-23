# AGENTS.md — Marionette

Marionette 是一个 Windows 本地多 Agent CLI 图形壳（Tauri 2 + React + ACP）。
只负责启动/展示/切换/记录 agent，不实现 agent runtime。

## 常用命令

- 启动开发版：`npm run tauri dev`（或 `start-marionette.bat`）
- 构建便携版：`tools\build-portable.bat`（内置引擎，产物 `release\<版本号>\Marionette.exe`，按版本分目录，运行中的旧版不会挡住新构建）。默认会先 `import-magpie.ps1` 拉上游 latest 稳定版进 `third_party/magpie`（`SKIP_MAGPIE_IMPORT=1` 跳过；`MAGPIE_TAG=v…` 钉版本）。用户侧也可在「模型共享」里单独更新 magpie。发布：`tools\publish-release.bat`
- 前端构建：`npm run build`（`tsc && vite build`）
- Rust 检查：`cargo check`（在 `src-tauri/` 下）
- 发布版本号：`tools\bump-version.ps1`（或 `tools\bump-version.bat`）

## 目录

- 前端：`src/`（React；会话流处理核心在 `src/lib/acpTranscript.ts`）
- 后端：`src-tauri/src/`（ACP 服务 `acp.rs`、终端 `terminal_runtime.rs`）
- 现行产品说明：`docs/CURRENT.md`；索引见 `docs/README.md`，`docs/archive/` 是历史归档，不是现行契约
- 验证脚本：`scripts/`（清单见 `scripts/README.md`）；Windows 启动/构建/发布：`tools/`（清单见 `tools/README.md`）
- 根目录只保留 `start-marionette.bat` 一个入口；改 `tools/` 里的脚本时，仓库根一律用 `%~dp0..`

## 排查「卡退」（崩溃/闪退）—— 第一件事

**用户报「卡退」时，先看 `%USERPROFILE%\.marionette\logs\dev.log`（开发者日志）。**

- 搜 `[panic]` 行，能直接给出 panic 位置和消息，例如：
  `[error] [panic] session=- panicked at src\terminal_runtime.rs:79:25: assertion failed: ...`
- release 配置是 `panic = "abort"`（`src-tauri/Cargo.toml`），**任何线程 panic 都会杀死整个进程**，所以 panic 行 = 崩溃根因，不用再猜。
- 同一时间附近有 `[watchdog] MAIN THREAD STALLED` 表示主线程冻结（假死），一般与 panic 无关，是主线程上的耗时操作。
- Windows 事件日志（Application / Application Error）只提供退出码：`0xc0000409` ≈ Rust panic-abort（fail-fast），`0xc0000005` 是访问违规。
- 崩溃转储：`C:\ProgramData\Microsoft\Windows\WER\ReportArchive\AppCrash_Marionette.exe_*`（有时能拿到 Report.wer 的堆栈模块偏移）。

### 已知崩溃历史

- **PTY 输出截断 mid-codepoint**（`terminal_runtime.rs` 的 `snap.output.drain(..overflow)`）：
  从 PTY 读到的字节截断点可能落在 UTF-8 多字节字符中间（中文/emoji），`drain` 的 `is_char_boundary` 断言 panic → 整个 app abort。
  已修复为回退到字符边界再 drain。**修这类问题不要再写裸 `truncate`/`drain(..n)`**，先保证落在 char boundary。
- **tao 0.35 paint assert**（`flush_paint_messages` 重入）：
  关窗时 WebView2 双 pump paint **以及** CJK IME 在窗口获得焦点/被点击时嵌套泵消息（日语系统 + 中文输入法最容易中）都会踩
  `assert!(flush_paint_messages(..))`。关窗路径：`main.rs` 的 `is_tao_paint_assert` + `shutdown_and_exit`（不要再改那条）。
  点击/IME 路径：`src-tauri/vendor/tao` 把 assert 改成尊重 re-entrant `false`（见 `vendor/tao/MARIONETTE-PATCH.md`）。
  Tauri 升到 tao ≥ 0.36 后删掉 `[patch.crates-io]`。

## 排查「exe 双击没反应 / 构建报 esbuild Access is denied」—— 先查完整性标签

**症状 1：** 双击 `release\<版本号>\Marionette.exe`（或 `target\release\marionette.exe`）完全没反应：没有窗口、没有日志、
退出码 0，`dev.log` 一行都不加。
**症状 2：** `npm run build` / `build-portable.bat` 反复失败：
`[vite:esbuild-transpile] remove C:\Users\...\AppData\Local\Temp\esbuild-<hex>: Access is denied.`，
且在 `%LOCALAPPDATA%\Temp` 里能看到那个删不掉的 1.7 MB `esbuild-*` 残留文件。

**同一个根因：程序以 Low 完整性（Low integrity）运行。** Windows 取「令牌完整性」和「镜像文件完整性标签」中较低者作为
进程完整性；低完整性进程对 `%USERPROFILE%\.marionette`（Medium）和真实 `%TEMP%`（Medium）只能读不能写：

- Marionette 打不开 `instance.lock` → 走单实例交接分支 → **静默 `exit(0)`**（`shell_integration.rs`）；
- esbuild 的 Go 进程删不掉自己写在 `%TEMP%` 的临时文件 → `remove ...: Access is denied.`。

**来源：** Windows 沙箱（如 DSH 的 workspace-write 限制）会给工作区目录加一条**可继承的 Low 完整性标签**，
于是 `D:\Apps\Marionette` 下**所有 .exe**（包括 `Marionette.exe`、`node_modules\@esbuild\win32-x64\esbuild.exe`，
甚至拷进来的 `cmd.exe`）都以 Low 跑起来——沙箱早已结束，标签却留在文件上，双击也一样中招。

**确认（看到 Low 即确诊）：**

```powershell
icacls D:\Apps\Marionette | Select-String 'Mandatory Label'
icacls D:\Apps\Marionette\Marionette.exe | Select-String 'Mandatory Label'
```

**修复：** 跑 `scripts\fix-workspace-integrity.ps1`（等价于下面这条；只改完整性标签，不动文件内容、所有者或访问权限）：

```powershell
icacls D:\Apps\Marionette /setintegritylevel (OI)(CI)Medium
```

改完 `npm run build` 与 `release\<版本号>\Marionette.exe` 立即恢复正常。**只要本工作区再被沙箱以 workspace-write
限制跑过一次，标签就会再来一遍**，所以再次出现上述症状时先跑这个脚本。

**产品侧兜底（已加）：** 启动时 `app_paths::ensure_global_dir_writable()` 会写探测文件，失败则走
`data_dir_gate::prompt_and_exit()` 弹原生对话框并 `exit(1)`，不再出现「静默 exit(0)」；
单实例锁只有真被占用（Windows 32/33 共享冲突）才交接，权限类错误会记一条 `[shell] warn` 后照常启动。

## 交付卡 / 每轮改动追踪（2026-10-23 修）

**别再用 `git status` 或逐文件 `git diff` 指纹当「本轮改了什么」——口径不一致会造假。**

- 本仓库实测：`git status -uall` 报 224~227 条，`git diff HEAD` 真实内容差异只有 52 条
  （`third_party/magpie/**` 这些 `text=auto`/CRLF 文件 stat-dirty 但 diff 0 行）。旧实现「status 列文件 +
  diff 算指纹」于是给 186 个文件留空指纹；再加上基线 `catch { return [] }` 把失败当成「基线是空的」，
  交付卡就成了「整库改动」展示器。
- 现行实现：`git_service::begin_workspace_snapshot` / `end_workspace_snapshot` / `discard_workspace_snapshot`
  —— turn 起止各拍一棵 worktree 内容树（私有 `GIT_INDEX_FILE` + 私有 object dir + alternates），
  `diff-tree -r -M --name-status/--numstat` 出真 `+/-`。**性能要害：私有 index 必须先复制用户
  `.git/index` 当 stat 缓存（0.6–1.2s）；裸 `read-tree`+`add --all` 要 8.6s。** 别写回用户 index。
- 归属规则在 `src/lib/turnDelivery.ts::selectTurnWorkspaceChanges`：起点干净→结束脏无条件算；
  **起点就脏的必须被本轮 edit/write 工具碰过才算**（否则同工作区的另一场对话会串进本卡）。
- **fail closed**：基线失败 / 不是 git 仓库 → 不发布任何 git 真相，卡片只留工具账本。
- 口径与实测数据另见 `docs/CURRENT.md`「交付卡（每轮改动的口径）」；回归用例
  `npx tsx scripts/verify-turn-delivery.mts` + `cargo test --bin marionette git_service`。

## DeepSeek Harness 接入（@openma/deepseek-harness-acp，2026-10-19 起）

Marionette 的 DSH agent 走 `@openma/deepseek-harness-acp`（bin `dsh-acp`），不再用停更的社区适配器
`deepseek-acp`（xintaofei，0.9.0，2026-09-10 停更）。要点：

- **安装**：`install.package = @openma/deepseek-harness-acp` + 依赖 `@deepseek-ai/dsh`（引擎，
  与 codex-acp/@openai/codex 同款「适配器 + 引擎」模式）。Node ≥ 22.15。
- **spawn 环境两件事**（`acp.rs` 的 DSH 分支）：
  1. `DSH_ACP_HOST` = 全局 `@deepseek-ai/dsh` 包目录（`dsh_runtime_dir()`：从 PATH 上的 `dsh`
     启动器旁推 `<prefix>/node_modules/@deepseek-ai/dsh`，再兜底 `%APPDATA%\npm` 布局）。
     **不要让适配器落回自带 runtime**：首启会把带符号链接的 tar 解压到 `%LOCALAPPDATA%\dsh-acp`，
     普通 Windows（无开发者模式）建符号链接直接 EPERM 崩（实测 2026-10-19，dsh-acp 0.4.39；
     openma CI 的 windows-latest 开着开发者模式，所以上游测不出来）。用户自设
     `DSH_ACP_HOST`/`DSH_PATH` 时尊重不覆盖。
  2. `DSH_SESSION_ROOT` = `$DSH_HOME/sessions`：standalone 默认是 `~/.dsh-acp/sessions`，
     不指过去会和 dsh web、Marionette 的会话导入（`parsers/deepseek.rs`）分家。
- **登录**：`dsh-acp login`（独立控制台窗口跑交互输入）；凭据仍落 `~/.dsh/.credentials.yaml`，
  或 `DEEPSEEK_API_KEY` env（`acp.rs` 会从 OpenCode auth.json 兜底注入）。
- **模型目录 / 推理档位**：适配器从**运行中的组合**出 live catalog（Web UI 里加的第三方 provider
  立即出现，id 稳定为 `provider::model`），effort（off/low/high/max）是独立 configOption。
  旧的「settings.yaml 桥接」hack（`dsh_settings.rs`，724 行）已整体删除；「pi-ai 路由模型丢失」
  「无推理档位」两个老问题随换适配器消失——DSH 0.2.x 把设置搬进 profile patch 的历史背景
  见 git 历史（本次提交前的 AGENTS.md）。
- **跨用桥（2026-10-20，`dsh_bridge.rs`）**：spawn 时把 llm-pi-ai 托管路由 `marionette`
  （openai-completions → 引擎 `/v1`）写进 **`~/.dsh/cordis.patch.yml`**（standalone dsh-acp 组装的
  home 层；settings.yaml / `profiles/acp/` 它**都不加载**——`settings` 行在无 profileContext 时
  disabled），令牌常量 `magpie` 同步进 `.credentials.yaml` refs + spawn env（解析顺序 env → 文件）。
  文本外科手术合并（home patch 允许 `!!js`，禁 serde_yaml 整档重写）、幂等、引擎停用即移除。
  每个模型带引擎目录的 `contextWindow`/`maxTokens`（`engine::catalog_direct_entries()`，引擎
  `/v1/models` 的 `context_window`/`max_output_tokens`）——**不写的话 dsh-llm-pi-ai 对所有
  marionette 路由模型落 `DEFAULT_CONTEXT_WINDOW = 262144`（256K）兜底**，1M 的 DeepSeek 会
  被显示成 256K、自动压缩阈值跟着错（2026-10-21 实测修复）。
  能看图的模型再补 `input: [text, image]`（`CatalogModelRow.images`，引擎 `modalities.input`）：
  **不写就是 pi-ai 默认 `[text]`，DSH 会把附图换成「image omitted because this model accepts
  text only」占位文本**，图根本不出适配器（2026-10-21 实弹复验，见 §8.6）。
  模型 id 广告为 `marionette::provider/model`（首个 `::` 切分，斜杠安全）。定稿与探针记录：
  `docs/research-dsh-cross-use.md` §8。

**NDJSON 探针**（不起 GUI 直接问适配器）：`spawn dsh-acp`（记得设 `DSH_ACP_HOST` 或会踩上面的
符号链接坑），stdin 依次发 `initialize`（`protocolVersion: 1`）和 `session/new`（`cwd` 必须是
绝对路径、`mcpServers: []`），读 `id=2` 应答里的 `configOptions` → `id: "model"` 的 `options`。

**`cordis.patch.yml` 的自锁坑（2026-10-23 修）**：移除托管路由时我们落的是
「说明注释 + `[]`」，而合并入口以前只认字面 `[]` —— 引擎闲置回收/停过一次之后，这个文件
被永久判成「不是顶层数组」拒写（dev.log 只有一条 `dsh marionette route refresh failed;
bridge skipped` warn，界面表现是「DSH 没有共享模型」）。现在空 patch 判定按语义来
（只有注释/空行/`[]` 即当空文件重写）、移除时连带收走说明行，回归测试
`rewrites_comment_only_empty_patch` / `remove_then_readd_round_trip`。
前端同轮补课：跨用目录缓存现在会随 `invalidateBridgeModels()` 通知订阅者（Composer 当场重拉），
引擎在菜单打开期间醒来不再需要切会话才刷新。踩坑定稿：`docs/research-dsh-cross-use.md` §8.7。

**仍成立的上游限制：pi-ai catalog 没有的 model id 会被 DSH 丢掉。** `dsh-llm-pi-ai` 是 DSH 官方唯一的多
provider 层，只服务 catalog 描述得出的模型（本机 catalog 在
`@earendil-works/pi-ai/dist/providers/data/opencode*.json`）。**不要给既有路由补 `api:` 来救** —— 路由级
`api` 优先级高于每个模型自己的协议（`api = request.api ?? base?.api ?? routeApi`），而 `opencode` 这类
catalog 路由横跨 `openai-completions` / `openai-responses` / `anthropic-messages`：补一个 `api` 会把同路由
里别的模型的协议一起改掉。**上游处方是拆路由**（README「Known Limitations」）：新开一个 catalog 不收录的
route key，自己声明 `api` + `baseURL` + `apiKeyEnv` + 只列这一个模型——实测有效，且不动原路由上其它模型的协议。
## 模型共享引擎的凭据源（2026-10-23）

引擎注册（`engine.rs` 的 `register_key_providers`）钥匙来源按优先级：**Marionette 凭据库
key-plan → ZCode 个人 provider 层（`~/.zcode/v2/provider_config.json` 的
`access.type=zhipu-coding-plan-api-key` → glm preset）→ opencode 扫描**，同 preset 先到
先得。两个必记事实：**opencode 凭据已迁 `opencode.db` 的 credential 表**（auth.json 迁移
后不再更新，只读它会拿到死钥匙；合并逻辑在 `credentials.rs` 的
`peek_opencode_db_keys_at` / `merge_opencode_key_sources`，active=1 优先）；**ZCode 设置页
配的 Coding Plan key 不落 v2/config.json**（那里 `options.apiKey` 恒空，本体在个人层）。
完整踩坑链、额度端点规矩与排查速查：`docs/pitfalls-model-share-quota.md`。

## ZCode 接入（社区适配器 `zcode-acp-server`，2026-10-23 起）

> 完整踩坑集（十一雷 + 快速确诊表，含日志证据链）：`docs/pitfalls-zcode-acp.md`。改 zcode 相关代码前先过一遍。

ZCode 官方**没有** ACP（CLI 0.16.x 无 `--acp`，ACP registry 未收录）；走的是社区桥
`zcode-acp-server`（npm，Apache-2.0，bin `zcode-acp-server`）：ACP ↔ `zcode app-server`
（ZCode 私有 stdio 协议 "ZCode Protocol v1"，NDJSON、**无 jsonrpc 信封**，`{id, method, params}`）。
底层协议已实测：`session/create` 需 `workspace{workspacePath,workspaceKey}` 且要应答服务端反向请求
`session/requestRuntimePreferences`；事件流 `state.updated`（revision 补丁）/`v4/telemetry/event`；
转录走 `session/messages`。**懒启动**：适配器首个 `session/prompt` 才 spawn 后端。

- **spawn 注入两件事**（`acp.rs` zcode 分支，用户自设 `ZCODE_BIN`/PATH `zcode` 时不覆盖）：
  1. `ZCODE_BIN` = 桌面版内置 `resources\glm\zcode.cjs`。桌面版可装任意目录、**注册表卸载键不登记**
     （本机 D:\Program Files\ZCode），探测链：`%LOCALAPPDATA%\Programs\ZCode` → `%ProgramFiles%\ZCode`
     → 开始菜单/桌面 `ZCode.lnk` 里抽 ANSI LocalBasePath 与 UTF-16 路径（`zcode_bin_path()`；只扫 UTF-16 会命中假路径，见 `docs/pitfalls-zcode-acp.md` 雷 10）。
  2. `ZCODE_NODE` = 起适配器的那台 node。适配器对 .cjs 型 ZCODE_BIN 自己找 sqlite-capable node，
     候选链（ZCODE_NODE → unix 路径 → which node → Zed bundle）在 Windows GUI PATH 下全空，
     最后裸 exec .cjs → **`spawn EFTYPE`**（实测 2026-10-23，0.76.2）。
- **MCP 借贷只借远程**：适配器 `session/new` 的 zod schema 只收 url 型（http/sse/acp）条目，
  **stdio 条目让整个 session/new -32602**——所以 `agent_registry::accepts_stdio_mcp("zcode") = false`，
  stdio 在 `mcp_payload_for_agent` 里跳过并记 note。远程条目 per-load 合并，ZCode 自己 `~/.zcode`
  的 MCP/插件（node_repl 等）照常加载；实测 http MCP 借出后模型能真实调用工具（E2E
  PONG:marionette-http-mcp-ok）。反向共通：`context_inventory` 扫 `~/.zcode/cli/config.json`
  的 `mcp.servers` 与 `~/.zcode/cli/skills`（用户层；插件技能不入清单，与其它 agent 的插件口径一致）。
- **登录/凭据**：与桌面版共享 `~/.zcode`——provider 密钥在 `v2/config.json`（`provider.*`），
  OAuth 令牌在加密的 `v2/credentials.json`（`probe_zcode_auth` 探这两个）。登录 = `zcode login`
  （PATH 没有 zcode 时 `commands.rs` 自动改跑 `node <zcode.cjs> login`，独立控制台）。
- **configOptions 全广告**（model/mode/thought → 模型/模式/推理档位芯片直出，无需 acpSupplements）；
  Node ≥ 22（node:sqlite）；ZCode CLI ≥ 0.14.8；权限经 `session/request_permission` 桥接。
- **跨用桥（2026-10-23，`model_bridge.rs`）**：spawn 时把托管 provider「marionette」
  （openai-chat-completions → 引擎 `/v1`，apiKey 用引擎常量令牌）合并进 **`~/.zcode/v2/provider_config.json`**
  ——zcode 的**个人** provider 层（桌面版 3.12+ 新增 provider 的正位；账号层 `v2/config.json`
  桌面版自己重写不能托管，且适配器的 env 注入只扫账号层、不会被劫持）。三处托管位：
  `providerOrder`（**追加**尾部，不动默认模型）、`providerConfigRules.providerRules`、
  `modelConfigRules.providerModelRules`（每模型 contextWindow/maxOutput/图片输入——不写会落
  内置 `.*` 兜底 200K + 不收图，同 DSH 的 256K 教训）。**个人层 schema 不收内置表那种
  regex `modelRules`**——一个未知键整层被 zod 拒掉、内存降级空配置（实测，zcode 日志
  `Personal Provider Config 加载失败`）；按 `providerId` 识别托管条目，幂等、引擎停用即移除。
  模型 id 广告为 `marionette\<engineId>`（适配器 configOptions 的 `provider\model` 反斜杠拼法，
  前端钉原生偏好用此前缀，`isBridgeNativeModel`/`modelLabel` 已收 `\` 形态）。E2E 实测：
  写入后 `model.available` 出现 `marionette\glm-5.3-e2e | ctx:1000000 | out:65536 | img:true`。
- **会话导入（2026-10-23，`parsers/zcode.rs`）**：ZCode 的对话主库是
  `~/.zcode/cli/db/db.sqlite`（`ZCODE_HOME` 可挪根）——`session.directory` 做项目
  过滤；转录在 `message`（时间线事件，role + semantics.kind）+ `part`（内容分片，
  按 message_id 关联）两表。只导出真实对话轮（user/user_prompt、
  assistant/assistant_response），system_reminder/todo_reminder/timeline_event 是合成
  事件跳过；`sess_subagent_*` / 带 parent_id / 已归档的行不进列表。实机冒烟：
  `cargo test zcode -- --ignored`。
- **NDJSON 探针**：`npx -y zcode-acp-server`（配 `ZCODE_BIN`/`ZCODE_NODE` env），stdin 发
  `initialize` → `session/new`（`cwd` 绝对路径），读 `id=2` 的 `configOptions`（id: model/mode/thought）。
- **个人层 provider 的模型要 `providerModelRules` 才进 ACP 菜单**（2026-10-23）：适配器
  `loadAllModels()` 对只在 `provider_config.json` 里的 provider 只认
  `modelConfigRules.providerModelRules`，从内置模板建的 provider（如桌面版「BigModel
  Coding Plan」）一条都没有 → 桌面能选、ACP 菜单里整家消失。已用
  `scripts/patch-zcode-acp-personal-models.mjs` 给适配器补回退链（→ `personalModelIds`
  → 模板 `builtinModelIds`，上下文窗口同源兜底）。**`npm i -g zcode-acp-server`
  会覆盖 `dist/`，升级后重跑该脚本**；细节见 `docs/pitfalls-zcode-acp.md` 雷 12。
