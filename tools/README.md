# tools — Windows 启动 / 构建 / 发布

根目录只保留一个双击入口（`..\start-marionette.bat`），其余批处理都在这里。
本目录同时用来放本地二进制：`build-portable.bat` 会优先找 `upx.exe` 或 `upx\upx.exe`。

| 脚本 | 用途 |
|---|---|
| `build-portable.bat` | 打便携版：内置引擎，产物在 `release\<版本号>\Marionette.exe`（按版本分目录，运行中的旧版不会锁住新构建）。打开即拉起 magpie。默认会先 `import-magpie.ps1` 拉 **GitHub latest 稳定版** 再编。环境变量：`SKIP_UPX=1`（内嵌 magpie 仍需要 UPX）、`REQUIRE_UPX=1`、`INSTALL_UPX=1`、`NOPAUSE=1`、`SKIP_MAGPIE_IMPORT=1`（沿用已有 `third_party\magpie`）、`MAGPIE_TAG=v…`（钉死种子版本） |
| `launch-latest.vbs` / `.ps1` | Stable shortcut launcher; reads `release/current-build.txt`, which is updated after a successful portable build. Use `.ps1 -ShowTarget` to inspect the target; exit an older instance before launching a new one. |
| `publish-release.bat` | 把 `release\<版本号>\Marionette.exe` 发到 GitHub Release（`gh`）。已有同 tag 则 `--clobber` 覆盖资产。可选 `-NotesFile`、`-Draft` |
| `import-magpie.ps1` | 吸纳/更新内置引擎源码进 `third_party\magpie`（只留构建子集）。不传 `-Tag` 时取上游 **latest 稳定 release**；可 `-Tag v…` 或环境变量 `MAGPIE_TAG`。`build-portable.bat` 默认会调用。**导入后自动重放引擎种子补丁**（`magpie-seed\main_seed.go` → `main_seed.go` + 给上游 `main.go` 加 `!marionette_seed` guard）：便携版内嵌的是精简"纯网关"magpie（绕过 `internal/cli` 整个 agent 管理器，UPX 载荷 ~12MB vs 完整 ~19.6MB），上游重构导致补丁失配会在编译时报错，不会静默 |
| `magpie-seed/` | 引擎种子补丁源：`main_seed.go`（build tag `marionette_seed`）只挂 gateway/routing/usage 三个插件，跑 `serve` 以及精简 `provider`/`group`/`models`（自定义来源登记不需要上游完整 magpie）。改插件集/启动序列在这里改，`import-magpie.ps1` 会把它拷进 `third_party\magpie` |
| `rebuild-quick.bat` | 增量重建 debug 二进制并重启；Vite 还在跑就只重启 exe |
| `start-marionette-quick.bat` | 已有 debug 构建时的快速启动：只拉起 Vite + `marionette.exe`，跳过工具链检查 |
| `bump-version.bat` | 版本号提升的包装（转发给 `bump-version.ps1`；不带参数会交互询问）。完成后提示接着 `build-portable`、再 `publish-release` |
| `bump-version.ps1` | 一次改完 `package.json`、`package-lock.json`、`src-tauri\tauri.conf.json`、`Cargo.toml`、`Cargo.lock` |
| `ensure-msvc.bat` | 检查 MSVC / Windows SDK，缺失时可经 winget 安装 |
| `ensure-rust.bat` | 检查 Rust 工具链，缺失时可经 rustup 安装 |
| `probe-computer-helper.py` | 自检：对一个构建产物跑 CU helper 协议四连（Ready/未知 op 拒绝/假 configure 拒绝/stdin 关净退）。用法 `python tools/probe-computer-helper.py <Marionette.exe>` |
| `probe-cu-e2e.py` | 自检：helper → cua-driver → list-windows 端到端（用真机已装的驱动，打印窗口清单与 grantable 标记） |
| `capture-window.py` | PrintWindow 拍任意顶层窗（注意：GPU 合成内容在部分沙箱/虚拟 GPU 下拍不出，验证浏览器/CU 画面优先用 CDP screenshot 或 DOM 几何） |
| 环境钩子 `MARIONETTE_AUTO_BROWSER_TAB=1` / `MARIONETTE_AUTO_CU_CHECK=1` | E2E 演练：开机自动开浏览器标签（冻死复现）/ 自动跑 CU 面板全流程（日志看 `[cu-check]`；冻死检测口径 = 数 `ui-heartbeat`，60s 一条，停跳即 JS 冻死） |
| `demo-instance.bat` / `.ps1` | 起一个隔离的演示实例：`USERPROFILE` 与 `WEBVIEW2_USER_DATA_FOLDER` 都指向临时 profile，只看得见预置的 `demo-app`，用于截图与试用 |
| `build-now.ps1` | 本地快捷打便携版（设 `SKIP_MAGPIE_IMPORT=1` + `NOPAUSE=1` 后转发 `build-portable.bat`）；路径基于 `$PSScriptRoot`，可从任意工作目录调用 |
| `web-chat-bridge\` | 内置 ChatGPT / DeepSeek **网页版**聊天桥的源码（随 exe 编译携带，无需手动注册；CDP 驱动浏览器，不经引擎、无工具）。见其 `README.md` |

`ensure-*.bat` 由其它脚本 `call`，一般不单独双击。
`REQUIRE_UPX=1` 现在同时拦「UPX 未安装」和「压缩失败」：两种情况都直接 fail，不再静默发出未压缩产物；exe 被占用写 `.new.exe` 时该模式也会 fail（发布管线要的是真产物）。

## 演示实例（截图用）

`demo-instance.bat` 准备一个丢弃式的 profile（默认 `%TEMP%\marionette-demo`）并在其中启动一个实例：

- **隔离**：`USERPROFILE` 与 `WEBVIEW2_USER_DATA_FOLDER` 都指向该 profile，所以 `projects.json`、会话、日志、单实例锁、WebView2 存储（主题 / 布局 / localStorage）全在临时目录里 —— 真实数据不会被读写，正在用的实例也不会被踢（单实例锁在 `<USERPROFILE>\.marionette\instance.lock`，换 profile 就互不相干）。
- **预置**：`demo-app` 演示项目（一个小 git 仓库，带一个未提交改动）已经写进演示 profile 的 `projects.json`，所以窗口里看不到任何真实项目或对话。
- **拍图状态**：默认再写一份预置对话——`demo-seed\session.json.tmpl` 落成 `<demo-app>\.marionette\sessions\index.json` 里的一条会话，`demo-seed\transcript.jsonl.tmpl` 落成同目录 `transcripts\<id>.jsonl`。内容是「读文件 → 改文件 → 跑命令 → 回复带代码块」的一整回合（SessionEvent JSONL，`parseTranscriptEvents` 直接吃得下），**不跑 agent、不耗额度、每次构图一样**；模板里的中文和文案随便改，时间戳由脚本填。`-NoSeed` 跳过。
- **语言**：`-Lang zh-CN`（默认）给 WebView2 传 `--lang`，时钟按中文排版；机器是日文/英文区域设置时也不会冒出「午後 0:26」。
- **agent**：默认把 `~/.grok/config.toml` 拷进演示 profile，否则实例里没有可用的 agent 能回答。**这份副本含凭据**，用完删掉整个演示目录即可；`-AgentConfig none` 可跳过。
- 重复运行会复用同一个 profile（对话历史保留）；同一 profile 再次启动会按单实例逻辑交给已在运行的那个，不会开第二个窗口。预置会话用固定 id，重跑会原地替换，不会长出第二条。

参数：`-DemoRoot <path>`、`-AgentConfig grok|none`、`-Exe <path>`、`-Lang <tag>`、`-NoSeed`。

抓图：对目标窗口句柄用 `PrintWindow(hwnd, hdc, PW_RENDERFULLCONTENT)`。不要用 `SetForegroundWindow` + 屏幕截图 —— Windows 会拒绝后台进程抢前台，结果抓到的是上层窗口（也就是你真实在用的那个实例）的画面。

改这些脚本时注意：脚本现在位于 `tools\`，仓库根要用 `%~dp0..` 表示。
