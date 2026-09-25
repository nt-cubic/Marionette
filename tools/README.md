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

`ensure-*.bat` 由其它脚本 `call`，一般不单独双击。

改这些脚本时注意：脚本现在位于 `tools\`，仓库根要用 `%~dp0..` 表示。
