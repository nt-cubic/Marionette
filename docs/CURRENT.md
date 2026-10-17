# Marionette — 当前状态（2026-10）

> 现行产品说明。历史设计稿与已完成路线图见 [`archive/`](./archive/)。

## 一句话

Windows 本地 **多 Agent CLI 图形壳**（Tauri + React）。不替代 OpenCode / Claude / Codex / Grok 等原厂 agent，只负责启动、展示、切换、记录与 handoff——外加把「任何一个来源的模型 / key / 订阅」喂给「任何一个 agent」的**跨用引擎**。

## 产品底线（仍有效）

- 不做自研 Agent runtime / 模型路由器 / 云同步 / MCP marketplace
- ACP 优先；会话绑定 agent；Clean 为主界面
- 轻、快；不做成 IDE 或 API 管理平台

## 已具备（相对用户可感知）

| 能力 | 说明 |
|------|------|
| 项目 / 会话 | 项目 CRUD、搜索 threads、项目拖拽排序、会话按发送 recency 排序；空闲 30 分钟的会话自动「挂起」（结束 agent 进程，下次发消息自动接上历史）；行内 ⏸ 只在进程还活着时画（starting / running / waiting / error），按下去就是停——正在跑的直接中断并结束进程，说出口的半截回复留着并标「已挂起」；已挂起的会话不画按钮；冷启动只恢复上次焦点（无存档则不打开对话，点底部输入框会在 Chat 新建） |
| Chat 对话 | 侧栏项目列表下的独立对话列表；使用进程默认工作目录，不创建项目或工作区 `.marionette`；资源管理器「在此处打开」遇到已有 `.marionette` 才按项目打开 |
| 多 Agent | 内置 + custom ACP agents；懒启动、能力协商、model/mode/effort/文件权限（agent 自己 advertise 的旋钮才出现，按 option id 判定，category 只当提示） |
| Clean 对话 | transcript JSONL、edit&resend、权限弹窗、Ask/Plan、@ 派任务、从任意一条回复分叉出新会话（复制到该条为止，首次发送自动带上这段历史）、回复里的图片按会话工作目录解析（文件名差一个字符时找同目录里唯一近似的那张）；默认闭眼折叠 tool/thinking，开眼状态会记住 |
| Handoff / Git | handoff.md、changed files、只读 diff |
| 多窗口 | Tab 幽灵撕出 → 松手开 detached 窗；关窗 hide 防 tao 闪退；隐藏壳 LRU 回收 |
| Usage | 上下文、OpenCode 余额探测、Claude/Codex/Grok 限流类数据、**订阅配额（引擎 `magpie quota`，跨 agent 可见的订阅窗口并入同一面板）** |
| 登录 | 多 agent probe + Sign in 横幅 |
| **跨用（模型/key/订阅互通）** | 见下节 |

## 跨用引擎（2026-10 定稿形态）

目标：**用户无脑打开软件就能用**——任何一个来源的模型，任何一个 agent 的会话里直接选。

- **key 型直连**（OpenCode Go / DeepSeek / GLM / Kimi / OpenRouter / xAI）：claude-code 走 spawn env、grok 走 owned 配置表，不过网关；来源含凭据库手存的 key 和 opencode `auth.json` 的只读扫描引用（引用只在直连注入瞬间读钥匙，不复制）。
- **订阅型 + codex 全场景**：经内嵌翻译引擎（[yetone/magpie](https://github.com/yetone/magpie)，MIT，Go）。**引擎随应用自动启动**；key-plan 与扫描引用的 api key 自动注册进引擎；Grok / ChatGPT 订阅由引擎 discovery 自己发现。codex 只接引擎源（协议约束），经 `providers/set` 改道 + 路由组把内建模型名别名到所选引擎模型。
- **两种便携版**（`tools\build-portable.bat` 默认一次打出两个，文件名对齐 GitHub Release）：普通版 `Marionette-<ver>-windows-x64.exe`（~2MB，引擎首次启用再下载）；内置引擎版 `Marionette-Engine-<ver>-windows-x64.exe`（~16MB）——magpie 源码吸纳在 `third_party/magpie`（`tools\import-magpie.ps1 -Tag <v>` 换版），UPX 内嵌、首启离线落位。发布用 `tools\publish-release.bat`。
- **入口**：左下角互联图标 = 网关状态/一键启动（点击不再弹窗）；凭据管理退居其浮层底部链接。
- **模型菜单**：原生模型 + 单一「跨用模型」段（全部来源拍平排序，每行带来源徽章：Grok 订阅 / OpenCode Go / DeepSeek …）。
- **卫生**：删除凭据条目会从引擎回收对应 provider（注册账本对账）；30 分钟无绑定且无需求的空闲引擎自动停 serve，任何跨用入口（菜单/绑定/按钮/启动）自动唤醒。
- 安全底线不变：明文 key 只进 env / owned-section / magpie CLI 命令行，绝不进日志与 agent 配置；扫描只读。

## 明确不做 / 暂缓

- 内嵌 llama.cpp / 本地推理 runtime
- 完整文件树 / IDE 级编辑器
- 全 Rust UI 重写
- 把历史 ASP 文档当现行协议实现
- 裸 CLI 全局配置接管（终端里裸跑 claude/codex 不经 Marionette 也能用引擎——已明确不做，见归档设计稿）

## 工程入口

- 前端：`src/`
- 后端：`src-tauri/src/`
- 启动：`start-marionette.bat`（根目录唯一入口）/ `npm run tauri dev`
- 便携构建：`tools/build-portable.bat` → `dist-portable/`（默认普通版 + 内置引擎版；`--plain` / `--engine` 只打其中一个）
- 引擎源码吸纳/升级：`tools\import-magpie.ps1 -Tag <上游tag>`
- 验证脚本：`scripts/`；Windows 脚本：`tools/`（各目录索引见其中的 `README.md`）

## 归档说明

`docs/archive/` 保留早期策划、路线图与 superpowers 规格，便于考古，**不以之为当前实现契约**。有冲突时以代码与本文件为准。跨用引擎的完整设计稿（M0-M3 过程记录）已归档至 [`archive/cross-agent-bridge.md`](./archive/cross-agent-bridge.md)。
