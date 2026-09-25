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
