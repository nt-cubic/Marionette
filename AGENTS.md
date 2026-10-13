# AGENTS.md — Marionette

Marionette 是一个 Windows 本地多 Agent CLI 图形壳（Tauri 2 + React + ACP）。
只负责启动/展示/切换/记录 agent，不实现 agent runtime。

## 常用命令

- 启动开发版：`npm run tauri dev`（或 `start-marionette.bat`）
- 构建便携版：`tools\build-portable.bat`（release 产物在 `dist-portable/`）
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
