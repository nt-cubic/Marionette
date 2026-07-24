# AgentShell

**统一的多 Agent 桌面终端**

把 OpenCode、Claude Code、Codex CLI、Grok Build…… 全部放进一个窗口。  
切换 Agent 不断上下文，用量一目了然，对话结构化呈现。

> 目前版本 **0.1.0** —— 核心流程可用了，界面在持续打磨。

---

## 为什么要有 AgentShell

AI 编码 Agent 爆发了一年，格局是这样的：

- **OpenCode** 体验最好，但绑定自家生态
- **Claude Code** 代码质量高，但 CLI 交互简陋
- **Codex CLI** 免费额度香，但终端输出全靠眼
- **Grok Build** 速度快，但功能单一

每个都是 CLI，各有各的认证、模型、界面风格。  
想在同一个项目里混合使用？上下文断了，体验支离破碎。

**AgentShell 不做新的 Agent**——它做一个**管理 Agent 的桌面 Shell**，让所有 Agent 在统一的界面里工作。

---

## 核心特色

### 多 Agent 同窗

一个窗口随时切换 OpenCode / Claude Code / Codex CLI / Grok。  
Agent 进程**按需启动**——你不打字它不跑，不占资源。

```
Composer 下拉框 → 选 Agent → 自动检测可用性
             → ACP 协议握手（结构化通信）
             → 或 PTY 回退（兼容所有 CLI）
```

### 上下文不丢失

切换 Agent 的瞬间，自动生成 `handoff.md`：

- 把之前的对话总结塞给下一个 Agent
- 不自动发送，你审一遍再确认
- 新 Agent 读得到上一个 Agent 做了什么

**不靠黑魔法，靠一个 markdown 文件。**

### 结构化 Clean View

所有 Agent 的输出统一渲染为消息卡片：

- **用户消息** · **AI 回复** · **思考过程**（默认折叠） · **工具调用**（默认折叠） · **文件变更**
- 全 Markdown 渲染（GFM），中文段落自动优化
- 任意历史消息可**编辑重发**，后面的对话自动截断
- 右侧 Message Outline 快速跳转

不想看 Clean View？一键切回 Raw Terminal，Agent 保持运行。

### 内联评论系统

选中一段对话 → 浮动「评论」按钮 → 输入批注 → 对话上留下标记脚注。  
下次打开还能看到。评论可以一键带入 Composer 作为引用。

适合：PR 审稿、对话复盘、跟同事协作。

### 用量一目了然

右边栏实时展示：

| 数据 | 来源 |
|------|------|
| DeepSeek 余额 | `/user/balance` 实时探针 |
| OpenRouter 剩余额度 | `/credits` + `/key` |
| OpenCode Go / Zen 套餐上限 | 已知套餐 ceiling |
| Codex 5h/周限 | `/status` 解析 |
| Claude 速率限制 | ACP `rateLimit` 事件 |
| 当前会话上下文占比 & 费用 | 实时累加 |

项目改了哪些文件？——Git 状态每 12 秒自动刷新，点击直接看 diff。

### 轻量克制

- Tauri v2（Rust + React），打包后 ~10MB
- 自研 Design Tokens，Frameless 窗口，无多余 UI chrome
- 默认 1280×820，开箱即用
- Lazy ACP Warm-up：只有聚焦 Composer 才启动 Agent 进程

---

## 快速上手

### 前置依赖

- Node.js 18+
- Rust 1.80+（构建用）
- 已安装至少一个 Agent（OpenCode / Claude Code / Codex CLI / grok）

### 构建运行

```bash
git clone https://github.com/your/agentshell
cd agentshell
npm install
npm run tauri dev
```

### 添加第一个项目

1. 打开 AgentShell
2. 左侧 Project Shelf → 「Add Project」
3. 选择你的项目目录
4. 在 Composer 里选 Agent，开始对话

---

## 架构一览

```
┌──────────────────────────────────────┐
│              UI (React)              │
│   Clean View · Composer · Context    │
│   Panel · Project Shelf · Comments   │
├──────────────────────────────────────┤
│          ASP (AgentShell Protocol)   │
│    Agent · Input · Capabilities ·    │
│    Handoff · Clean Event             │
├──────────────┬───────────────────────┤
│   ACP Agent  │    PTY Agent          │
│  (OpenCode,  │  (Codex, Claude,      │
│   Grok)      │   Grok fallback)      │
└──────────────┴───────────────────────┘
```

**ASP 是产品层协议**，ACP 和 PTY 都是底层传输。  
UI 只跟 ASP 对话——无论底层是哪种 Agent，上层体验一致。

---

## 技术栈

| 层 | 技术 |
|---|---|
| 桌面壳 | Tauri v2（Rust） |
| 前端 | React 18 + TypeScript + Vite 6 |
| 图标 | Lucide React |
| 终端 | xterm.js |
| Markdown | react-markdown + GFM |
| 持久化 | JSON 文件（`~/.agentshell/`） |
| PTY | portable-pty（Rust） |
| 协议 | ACP JSON-RPC over stdio |

---

## 开发

```bash
# 前端热更新
npm run dev

# Tauri 桌面调试
npm run tauri dev

# 构建
npm run tauri build

# 浏览器预览模式（有 mock 数据）
npm run dev:mock
```

调试日志写到 `~/.agentshell/logs/dev.log`，4MB 自动轮转。

---

## 设计理念

- **诚实的降级**：Agent 没有的能力，Composer 就不显示对应控件，不画假按钮
- **Clean View 优先**：所有 Agent 默认看结构化视图，Raw Terminal 是 toggle
- **跨 Agent 上下文靠文件**：`handoff.md` 是纯文本，用户能看懂、能手动改、能 Git 提交
- **持久化优先**：所有会话存 JSONL 文件，全文可搜索，应用重开不丢

---

## 路线图

详见 [`docs/05-next-roadmap.md`](docs/05-next-roadmap.md)。  
近期方向：ASP 协议正式化、Agent 插件系统、原生二进制分发。

---

## License

MIT
