# Marionette

**Agent Harness, All in One**

让模型，在它最适合的 Harness 里生长。

Claude 在 Claude Code · GPT 在 OpenCode  
DeepSeek 在 OpenCode · Grok 在 Grok Build

你不需要一个 Agent 做所有事。  
你只需要一个窗口，让它们各司其职。

> v0.1.0 —— 核心可用，持续打磨

---

## 为什么是 Marionette

每个模型，都有它最适合的 Harness。

Claude 在 Claude Code 里表现最好，GPT 在 OpenCode 里最顺手。不是哪个 Agent 更强——每个厂商都在对自己的模型做 **Harness 级别的专门优化**。强塞一个模型到不兼容的壳里，效果打折扣。

你不需要选边站队。让每个模型，在它最适合的 Harness 里生长。

Marionette 帮你实现这件事。

---

## 八大核心特色

### 选 Agent，像选专家

你不是在选一个 Agent。你是在给每个任务挑最趁手的工具。Composer 下拉框随手切换，进程按需启动——你不打字它不跑，不占资源。

OpenCode、Claude Code、Codex CLI、Grok Build——所有 Agent 一个面板管理。

### 换人，不换脑子

从 Claude Code 切到 OpenCode，上下文不会断。

切换瞬间生成 `handoff.md`，把当前项目状态和对话摘要交给下一个 Agent。不自动发送——你审一遍，再确认。

不靠黑魔法，靠一个纯文本文件。你看得懂、能手动改、能 Git 提交。

### 一套配置，全线打通

你有四个 Agent，就有四套 MCP 配置、四个 Skill 目录、四种存储格式。

Marionette 的 context_inventory 把它们全部扫进同一个清单——自动跨 Agent 扫描、一键启用、去重注入。同一个 Blender MCP，OpenCode 能用、Claude Code 也能用；同一个 xlsx Skill，不用在四家各装一遍。

### 不统一的输出，统一的阅读

所有 Agent 的输出统一渲染为消息卡片：

- **用户消息** · **AI 回复** · **思考过程**（默认折叠） · **工具调用**（默认折叠） · **文件变更**
- 全 Markdown 渲染（GFM），中文段落自动排版
- 任意历史消息可编辑重发，后面的对话自动截断
- 右侧 Message Outline 快速跳转

### 少，但是更好

打开 Marionette——没有营销大图、没有新手弹窗、没有等你来点的装饰。

你看到的是工作台：左侧项目列表、中央对话区、右侧上下文面板。仅此而已。

克制密度，带来的是简洁的信息，每一像素都有它存在的理由。

### 对话，也能被标记

一段对话，是思考的完整轨迹。它值得被批注、被回顾、被引用。

选中任意消息，写下你的想法——下次打开，批注还在。评论可以一键带入 Composer，作为下一轮工作的起点。PR 审稿、方案复盘、异步协作，一个人也完成得了。

### 每一笔消耗，都摆在桌上

右侧栏实时展示各 Agent 用量：

| 数据 | 来源 |
|------|------|
| DeepSeek 余额 | `/user/balance` 实时探针 |
| OpenRouter 剩余额度 | `/credits` + `/key` |
| OpenCode Go / Zen 套餐上限 | 已知套餐 ceiling |
| Codex 5h/周限 | `/status` 解析 |
| Claude 速率限制 | ACP `rateLimit` 事件 |
| 当前会话上下文占比 & 费用 | 实时累加 |

Git 状态每 12 秒自动刷新，改了哪些文件一眼看到。

### 十兆字节，恰到好处

它不占你的硬盘，不抢你的内存，CPU不会产生峰值。
打开即用，关了就忘。

打包不到 10MB。
Agent 进程只在需要时启动——你不打字，它不跑。

轻，但不是简陋。是恰到好处。

---

## 快速上手

### 前置依赖

- Node.js 18+
- Rust 1.80+（构建用）
- 已安装至少一个 Agent（OpenCode / Claude Code / Codex CLI / grok）

### 构建运行

```bash
git clone https://github.com/your/marionette
cd marionette
npm install
npm run tauri dev
```

### 添加第一个项目

1. 打开 Marionette
2. 左侧 Project Shelf → 「Add Project」
3. 选择你的项目目录
4. 在 Composer 里选 Agent，开始对话

---

## 架构一览

```
┌──────────────────────────────────────┐
│             UI (React)               │
│  Clean View · Composer · Context     │
│  Panel · Project Shelf · Comments    │
├──────────────────────────────────────┤
│         ASP (Agent Service Protocol) │
│   Agent · Input · Capabilities ·     │
│   Handoff · Clean Event              │
├──────────────┬───────────────────────┤
│  ACP Agent   │    PTY Agent          │
│ (OpenCode,   │  (Codex, Claude,      │
│  Grok)       │   Grok fallback)      │
└──────────────┴───────────────────────┘
```

ASP 是产品层协议，ACP 和 PTY 都是底层传输。UI 只跟 ASP 对话——无论底层是哪种 Agent，上层体验一致。

---

## 设计理念

- **诚实的降级**：Agent 没有的能力，Composer 就不显示对应控件，不画假按钮
- **Clean View 优先**：所有 Agent 默认看结构化视图，Raw Terminal 是一键切换
- **跨 Agent 上下文靠文件**：handoff.md 是纯文本，用户能看懂、能手动改、能 Git 提交
- **持久化优先**：所有会话存 JSONL 文件，全文可搜索，应用重开不丢
- **你是操控者**：Agent 是木偶，你才是提线师——Marionette 让你掌控全局

---

## 技术栈

| 层 | 技术 |
|---|---|
| 桌面壳 | Tauri v2（Rust） |
| 前端 | React 18 + TypeScript + Vite 6 |
| 图标 | Lucide React |
| 终端 | xterm.js |
| Markdown | react-markdown + GFM |
| 持久化 | JSON 文件（`~/.marionette/`） |
| PTY | portable-pty（Rust） |
| 协议 | ACP JSON-RPC over stdio |

---

## 路线图

详见 [`docs/05-next-roadmap.md`](docs/05-next-roadmap.md)。  
近期方向：ASP 协议正式化、Agent 插件系统、原生二进制分发。

---

## License

MIT
