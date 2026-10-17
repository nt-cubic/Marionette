# AGENTS.md — Marionette

Marionette 是一个 Windows 本地多 Agent CLI 图形壳（Tauri 2 + React + ACP）。
只负责启动/展示/切换/记录 agent，不实现 agent runtime。

## 常用命令

- 启动开发版：`npm run tauri dev`（或 `start-marionette.bat`）
- 构建便携版：`tools\build-portable.bat`（默认一次打出普通版 + 内置引擎版，产物在 `dist-portable/`：`Marionette-<ver>-windows-x64.exe` 与 `Marionette-Engine-<ver>-windows-x64.exe`）。`--plain` / `--engine` 只打其中一个。引擎源码在 `third_party/magpie`，升级用 `tools\import-magpie.ps1 -Tag <v>`。发布：`tools\publish-release.bat`
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

**症状 1：** 双击 `dist-portable\Marionette.exe`（或 `target\release\marionette.exe`）完全没反应：没有窗口、没有日志、
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
icacls D:\Apps\Marionette\dist-portable\Marionette.exe | Select-String 'Mandatory Label'
```

**修复：** 跑 `scripts\fix-workspace-integrity.ps1`（等价于下面这条；只改完整性标签，不动文件内容、所有者或访问权限）：

```powershell
icacls D:\Apps\Marionette /setintegritylevel (OI)(CI)Medium
```

改完 `npm run build` 与 `dist-portable\Marionette.exe` 立即恢复正常。**只要本工作区再被沙箱以 workspace-write
限制跑过一次，标签就会再来一遍**，所以再次出现上述症状时先跑这个脚本。

**产品侧兜底（已加）：** 启动时 `app_paths::ensure_global_dir_writable()` 会写探测文件，失败则走
`data_dir_gate::prompt_and_exit()` 弹原生对话框并 `exit(1)`，不再出现「静默 exit(0)」；
单实例锁只有真被占用（Windows 32/33 共享冲突）才交接，权限类错误会记一条 `[shell] warn` 后照常启动。

## 排查「DeepSeek Harness 只剩官方两个模型 / 看不到 opencode-go 等路由模型」—— 先看设置文档

**症状：** Marionette 里 DSH 会话的模型下拉只有 `DeepSeek-V4.1-Flash`、`DeepSeek-V4-Pro` 两个（或不带 provider
前缀的两个 id），用户配的 `opencode-go` / `opencode` / `xai` 全部不见；已保存的偏好模型切过去会回
`config option "model" has no value "opencode-go::…"`。`dev.log` 里 `session/new` 的 `configOptions` 就是那两个。

**根因（两层，都与 Marionette 代码无关，但只有 Marionette 能补）：**

1. DSH 0.2.0-rc.2 把设置从 `$DSH_HOME/settings.yaml` 搬进了 **profile patch**
   （`$DSH_HOME/profiles/<name>/cordis.patch.yml`），旧文档改名成 `settings.yaml.imported` 且不再读取。
2. Marionette 走的是社区适配器 `deepseek-acp`（0.9.0，已是 npm 最新）。它 compose 的是 `dsh-settings-file`，
   设置文档**死锁在 `<DSH_HOME>/settings.yaml`**；而且只有该文档原文里含 `llm-pi-ai` 这个子串时，它才挂载把
   `llm-pi-ai.providers` 变成可选模型的 pi-ai 插件（`settingsMentionsPiAi` 的惰性加载预筛，省 5 秒 import）。
   文件被改走之后预筛恒为假 → 路由全丢，只剩 `dsh-llm-deepseek` 内建的两个。

**修复（已加）：** `src-tauri/src/dsh_settings.rs` 的 `ensure_settings_bridge()`，在 `acp.rs` 的 DSH 启动分支里调用。
它把 profile patch 里的 `llm-pi-ai` 段**原文搬运**（不 parse/不重新 emit，`!!js`、block scalar、引号都原样保留）
到 `$DSH_HOME/settings.yaml`：只读 DSH 自己的文件、只写这一个段、带所有权头注释，非自家文件绝不覆盖，profile
变新了才刷新。每台机器上 DSH 应用一启动又会把该文件改名搬走，所以**每次启动都补一次**才是正确姿势。

**验证（不用起 GUI）：**

```powershell
# 1. 桥接是否发生：起一次 DSH 会话后看这行
Select-String "$env:USERPROFILE\.marionette\logs\dev.log" -Pattern 'DSH settings bridge' | Select-Object -Last 1
# 2. 镜像文件存在且带所有权头
Get-Content "$env:USERPROFILE\.dsh\settings.yaml" -TotalCount 2
# 3. session/new 报出来的模型：分组形态里能看到 opencode-go 等 provider 名
Select-String "$env:USERPROFILE\.marionette\logs\dev.log" -Pattern 'opencode-go::' | Select-Object -Last 1
```

想脱离 GUI 直接问适配器，就自己写个 NDJSON 探针：`spawn deepseek-acp`（`lib/bin.js`），stdin 依次发
`initialize`（`protocolVersion: 1`）和 `session/new`（`cwd` 必须是绝对路径、`mcpServers: []`），
读 `id=2` 应答里的 `configOptions` → `id: "model"` 的 `options`。

**顺带两条上游限制（都不是 Marionette 的 bug，也不要拿配置去"修"）：**

1. **catalog 里没有的 model id 会被 DSH 丢掉。** 先澄清一点：`llm-pi-ai` 不是 Marionette 引入的，它是 **DSH 官方唯一的多
   provider 层** —— DSH 只有两类 LLM 适配器（`dsh-llm-deepseek*` 官方直连、`dsh-llm-pi-ai` 其余全部，见
   `@deepseek-ai/dsh-llm-pi-ai/README.md`「When to choose it」），部署要接第三方网关就只能用它。
   pi-ai 只服务它描述得出的模型：本机 catalog（`@earendil-works/pi-ai/dist/providers/data/opencode*.json`）有
   `longcat-2.0`、`ling-3.0-flash-fin-free` 等，但**没有** `deepseek-v4.1-flash`、`mimo-v2.6-flash-free`。
   **不要给既有路由补 `api:` 来救** —— 路由级 `api` 优先级高于每个模型自己的协议
   （`api = request.api ?? base?.api ?? routeApi`），而 `opencode` 这类 catalog 路由横跨
   `openai-completions` / `openai-responses` / `anthropic-messages`：补一个 `api` 会把同路由里别的模型的协议
   一起改掉（实测其 `muse-spark-1.2/1.3` 是 `openai-responses`，补 `openai-completions` 就是打错端点）。
   **上游给的处方是拆路由**（README「Known Limitations」：*One wire protocol per route — a mixed-protocol
   catalog route cannot host a model of the other protocol; **splitting the provider across two route keys is the
   workaround***；README「Resolve the model catalog」也说明"加一个比 catalog 更新的模型"属受支持的一行改动）。
   即：新开一个 **catalog 不收录的 route key**，自己声明 `api` + `baseURL` + `apiKeyEnv` + 只列这一个模型。
   实测有效，且不动原路由上其它模型的协议。**注意：拆路由只解决"模型进不进下拉"，不解决"有没有推理档位"。**

2. **pi-ai 路由一律没有"推理档位"控件，只有 `deepseek-official` 有。** 供应商侧的档位其实存在
   （直接问 pi-ai：`xai/grok-4.6` → low/medium/high/xhigh，`opencode-go/longcat-2.0` →
   off/minimal/low/medium/high），断点在 harness：`deepseek-acp` 取
   `resolveModelInfo(provider, model).reasoning`，`dsh-llm-pi-ai` 只在路由成功绑成 pi-ai provider 且该模型无
   per-model 错误时才给，否则 `modelOf()`（`lib/index.js:1769`）抛错；且该错被 `deepseek-acp`
   （`lib/port/in-process.js:318` 的 `() => undefined`）**静默吞成"无此能力"**，stderr 一个字都没有。
   另外 `deepseek-acp` 只在 `efforts.length > 1` 时才发这个选项（`lib/config/options.js:165`）。
   **不要在配置里手写 `reasoningEfforts` 顶替**：那是替供应商断言 wire 映射（写错会被 `invalid()` 拒），
   而且实测即使写了也不会出现控件。Marionette 只渲染 agent 声明过的 option，且**刻意不伪造** effort
   （伪造会换来 `Unknown config option: effort`）——所以这里 Marionette 的正确做法就是什么都不做。

**什么时候可以删掉 `dsh_settings.rs`：** 满足任一条即可 —— `deepseek-acp` 改为读 profile patch
（即 `settingsMentionsPiAi` 那条路不再依赖 `$DSH_HOME/settings.yaml`）、或 Marionette 改用官方
profile 化的 ACP app（`@deepseek-ai/dsh-acp-app` 挂在某个 profile 上）。删之前先确认
`$DSH_HOME/settings.yaml` 不再是适配器唯一的设置来源。
