# tools — Windows 启动 / 构建 / 发布

根目录只保留一个双击入口（`..\start-marionette.bat`），其余批处理都在这里。
本目录同时用来放本地二进制：`build-portable.bat` 会优先找 `upx.exe` 或 `upx\upx.exe`。

| 脚本 | 用途 |
|---|---|
| `build-portable.bat` | 打便携版单文件：release 构建 → `..\dist-portable\Marionette.exe`，有 UPX 则压到 ~1.2 MB。环境变量：`SKIP_UPX=1`、`REQUIRE_UPX=1`、`INSTALL_UPX=1`、`NOPAUSE=1` |
| `rebuild-quick.bat` | 增量重建 debug 二进制并重启；Vite 还在跑就只重启 exe |
| `start-marionette-quick.bat` | 已有 debug 构建时的快速启动：只拉起 Vite + `marionette.exe`，跳过工具链检查 |
| `bump-version.bat` | 版本号提升的包装（转发给 `bump-version.ps1`；不带参数会交互询问） |
| `bump-version.ps1` | 一次改完 `package.json`、`package-lock.json`、`src-tauri\tauri.conf.json`、`Cargo.toml`、`Cargo.lock` |
| `ensure-msvc.bat` | 检查 MSVC / Windows SDK，缺失时可经 winget 安装 |
| `ensure-rust.bat` | 检查 Rust 工具链，缺失时可经 rustup 安装 |
| `demo-instance.bat` / `.ps1` | 起一个隔离的演示实例：`USERPROFILE` 与 `WEBVIEW2_USER_DATA_FOLDER` 都指向临时 profile，只看得见预置的 `demo-app`，用于截图与试用 |

`ensure-*.bat` 由其它脚本 `call`，一般不单独双击。

## 演示实例（截图用）

`demo-instance.bat` 准备一个丢弃式的 profile（默认 `%TEMP%\marionette-demo`）并在其中启动一个实例：

- **隔离**：`USERPROFILE` 与 `WEBVIEW2_USER_DATA_FOLDER` 都指向该 profile，所以 `projects.json`、会话、日志、单实例锁、WebView2 存储（主题 / 布局 / localStorage）全在临时目录里 —— 真实数据不会被读写，正在用的实例也不会被踢（单实例锁在 `<USERPROFILE>\.marionette\instance.lock`，换 profile 就互不相干）。
- **预置**：`demo-app` 演示项目（一个小 git 仓库，带一个未提交改动）已经写进演示 profile 的 `projects.json`，所以窗口里看不到任何真实项目或对话。
- **agent**：默认把 `~/.grok/config.toml` 拷进演示 profile，否则实例里没有可用的 agent 能回答。**这份副本含凭据**，用完删掉整个演示目录即可；`-AgentConfig none` 可跳过。
- 重复运行会复用同一个 profile（对话历史保留）；同一 profile 再次启动会按单实例逻辑交给已在运行的那个，不会开第二个窗口。

参数：`-DemoRoot <path>`、`-AgentConfig grok|none`、`-Exe <path>`。

抓图：对目标窗口句柄用 `PrintWindow(hwnd, hdc, PW_RENDERFULLCONTENT)`。不要用 `SetForegroundWindow` + 屏幕截图 —— Windows 会拒绝后台进程抢前台，结果抓到的是上层窗口（也就是你真实在用的那个实例）的画面。

改这些脚本时注意：脚本现在位于 `tools\`，仓库根要用 `%~dp0..` 表示。
