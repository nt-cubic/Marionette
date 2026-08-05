# Marionette — 当前状态（2026-08）

> 现行产品说明。历史设计稿与已完成路线图见 [`archive/`](./archive/)。

## 一句话

Windows 本地 **多 Agent CLI 图形壳**（Tauri + React）。不替代 OpenCode / Claude / Codex / Grok 等原厂 agent，只负责启动、展示、切换、记录与 handoff。

## 产品底线（仍有效）

- 不做自研 Agent runtime / 模型路由器 / 云同步 / MCP marketplace
- ACP 优先；会话绑定 agent；Clean 为主界面
- 轻、快；不做成 IDE 或 API 管理平台

## 已具备（相对用户可感知）

| 能力 | 说明 |
|------|------|
| 项目 / 会话 | CRUD、搜索 threads、项目拖拽排序、会话按发送 recency 排序 |
| 多 Agent | 内置 + custom ACP agents；懒启动、能力协商、model/mode/effort |
| Clean 对话 | transcript JSONL、edit&resend、权限弹窗、Ask/Plan、@ 派任务 |
| Handoff / Git | handoff.md、changed files、只读 diff |
| 多窗口 | Tab 幽灵撕出 → 松手开 detached 窗；关窗 hide 防 tao 闪退；隐藏壳 LRU 回收 |
| Usage | 上下文、OpenCode 余额探测、Claude/Codex/Grok 限流类数据 |
| 登录 | 多 agent probe + Sign in 横幅 |

## 明确不做 / 暂缓

- 内嵌 llama.cpp / 本地推理 runtime
- 完整文件树 / IDE 级编辑器
- 全 Rust UI 重写
- 把历史 ASP 文档当现行协议实现

## 工程入口

- 前端：`src/`
- 后端：`src-tauri/src/`
- 启动：`start-marionette.bat` / `npm run tauri dev`
- 便携构建：`build-portable.bat`

## 归档说明

`docs/archive/` 保留早期策划、路线图与 superpowers 规格，便于考古，**不以之为当前实现契约**。有冲突时以代码与本文件为准。
