# AGENTS.md — Marionette（宪法 + 指针）

Marionette 是一个 Windows 本地多 Agent CLI 图形壳（Tauri 2 + React + ACP）。
只负责启动/展示/切换/记录 agent，不实现 agent runtime。

> **本文件只保留宪法**（跨域红线 + 命令 + 指针）。**按域的开发手册在
> `docs/manual/<域>.md`**——改任何模块前先读对应手册（是什么/改什么找什么/
> 绝不能做什么/测什么）。历史批次原文：`docs/archive/AGENTS-2026-11-18.md`。
> 产品行为描述：`docs/CURRENT.md`。

## 常用命令

- 启动开发版：`npm run tauri dev`（或 `start-marionette.bat`）
- 前端构建：`npm run build`（`tsc && vite build`）
- Rust 检查/测试：**`cmd /c check.bat …`**（自动带 MSVC 环境；裸 cargo 撞 cl.exe not found）
- 构建便携版：`tools\build-portable.bat`（`MARIONETTE_ENGINE=rust` 换 rust SKU）；
  发布 `tools\publish-release.bat`；版本号 `tools\bump-version.ps1`
- 演示实例/拍图：`tools\demo-instance.bat`（参数与抓图禁忌见 `tools/README.md`）
- 验证脚本：`scripts/`（清单见 `scripts/README.md`）；Windows 脚本：`tools/README.md`

## 工作地点与 git 纪律

- 引擎开发只在 `D:\Project\marionette-rust`（worktree，分支 engine-rust）；
  主树 `D:\Project\Marionette` 是用户的合并现场，绝不碰。push 需用户明示。
- **多代理并行开发时**：每个代理只许改自己简报里的文件；**一律禁止 git 状态操作**
  （stash/checkout/restore/clean/reset）——未提交改动由主会话统一验证提交；
  Edit 报「file modified since read」就重新 Read；测试被兄弟线在途中间态挡住就
  等重试或 `git archive HEAD`+覆盖自己文件建 %TEMP% 隔离副本自验。

## 全局红线（跨域宪法，细则在各域手册）

1. **用户报「卡退」先看 `%USERPROFILE%\.marionette\logs\dev.log` 搜 `[panic]`**
   （release `panic="abort"`，panic 行=根因）。排障全路径：
   [docs/manual/debug-runbook.md](docs/manual/debug-runbook.md)。
2. **持 storage 锁绝不跑子进程/秒级 IO**；读转录走锁外路径版函数。
   [storage-transcripts.md](docs/manual/storage-transcripts.md)。
3. **TrySuspend 必须成对**：每条 hide/show 路径都要 webview 级配对调用——漏 show
   半边=唤醒后白屏。**最小化挂起 check-then-clear**。
   [memory-process.md](docs/manual/memory-process.md)。
4. **tao paint assert**：关窗路径 `is_tao_paint_assert`+`shutdown_and_exit` 别改；
   点击/IME 路径靠 vendor/tao 补丁。PTY 字节切分先保证 char boundary。
5. **vendor/wry 与 vendor/tao 补丁**：升级 wry/tauri 前对着两份 MARIONETTE-PATCH.md
   逐条核对能否撤掉。
6. **Low 完整性标签**（exe 双击没反应/esbuild Access is denied）→ 跑
   `scripts\fix-workspace-integrity.ps1`；会复发。
7. **GUI 进程零 println**（windows 子系统写 stdout 会 panic）；cli 动词走 `run()` 返 Err。
8. **沙箱测试锁只有一把**（engine_gateway 的 `TEST_SANDBOX_LOCK`；council/chief 各有
   同源纪律）——动 `XDG_CONFIG_HOME` 的测试必须拿锁。
9. **不加新依赖**尤其引擎（tokio/hyper/axum/reqwest 禁）；TLS 保持 ureq+native-tls。
10. **还原后键盘失灵**修复（main.rs 还原分支 show 后 `set_focus()`）别删。

## 域手册索引

| 域 | 手册 | 域 | 手册 |
|---|---|---|---|
| ACP agent 接入 | [acp-agents](docs/manual/acp-agents.md) | 会话/标签/撕窗 | [sessions-tabs](docs/manual/sessions-tabs.md) |
| 引擎网关 | [engine-gateway](docs/manual/engine-gateway.md) | 浏览器 | [browser](docs/manual/browser.md) |
| Token 用量 | [usage](docs/manual/usage.md) | Computer Use | [computer-use](docs/manual/computer-use.md) |
| 圆桌 council | [council](docs/manual/council.md) | 贴边停靠 | [dock](docs/manual/dock.md) |
| 存储与转录 | [storage-transcripts](docs/manual/storage-transcripts.md) | 收纳盒 | [memo-box](docs/manual/memo-box.md) |
| 内存与进程 | [memory-process](docs/manual/memory-process.md) | 富文本 | [rich-markdown](docs/manual/rich-markdown.md) |
| 构建与发布 | [build-release](docs/manual/build-release.md) | 对话图片 | [seen-images](docs/manual/seen-images.md) |
| 排障 | [debug-runbook](docs/manual/debug-runbook.md) | 视觉语言 | [ui-language](docs/manual/ui-language.md) |
| 远程助理/员工 | [staff-chief](docs/manual/staff-chief.md) | i18n | [i18n](docs/manual/i18n.md) |

任务级走查（加新东西照着做）：`docs/manual/howto/`——加新 ACP agent /
加订阅账号桥 / 加浏览器工具。

## 批次纪律（2026-11-19 起）

- **新批次落地：更新对应域手册**（红线/坑/检查单），**不再往本文件追加日期段**；
  重大事故进 `docs/pitfalls/`，设计稿进 `docs/plans/`（含偏差记录）。
- 本文件只收：跨域新红线（一行+手册指针）、命令变化、手册结构变化。
- 手册书写规范：`docs/manual/_TEMPLATE.md`（是什么/改什么找什么/红线/坑/检查单/
  验证/源流；不复制行为描述与实现细节——手册是索引+红线+检查单）。

## 引擎分支补充（engine-rust worktree）

- 测试：`cmd /c "check.bat test --features engine-rust --bin marionette"`
  （1139 项基线）。**council 的 `same_model_two_members_distinct_calls` 是高失败率
  flake**（HEAD 对照 4/5 也败）——独跑不绿属正常，别为它回滚别的批。
- 双 SKU 互斥（engine-rust × embedded-engine，compile_error 守门）；rust SKU 发布
  `set MARIONETTE_ENGINE=rust && tools\build-portable.bat`（≈5.36MB）。
- 合同套件唯一验收尺：`node scripts/verify-engine-contract.mts
  src-tauri/target/debug/marionette-engine.exe`。
- 上游跟进=研究克隆+digest（`scripts/upstream-watch.mts`），不是合并代码。
- 剩余队列与账本：`docs/plans/plan-engine-rust.md`；模块差距清单在各模块头注
  （索引见 [docs/manual/engine-gateway.md](docs/manual/engine-gateway.md)）。
