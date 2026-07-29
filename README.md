# Marionette

**Agent Harness, All in One**

每个模型，都应该运行在它最擅长的环境里。

Claude 用 Claude Code。  
GPT 用 OpenCode。  
Grok 用 Grok Build。

你不需要一个 Agent 做所有事。  
Marionette 把它们放进同一个工作台。

> v0.1.0 —— 核心可用，持续打磨

---

## 为什么存在 Marionette

AI 世界没有真正统一的 Agent。

每一家都在持续优化自己的工作环境。Claude 最懂 Claude Code，GPT 更适合 OpenCode，Grok 生来就是 Grok Build。

真正的问题，不是谁更强——而是为什么一定要在它们之间做选择？

Marionette 不替代它们。Marionette 把它们连接起来。

---

## 四个核心能力

### 一个窗口，所有 Agent

Claude Code、OpenCode、Codex CLI、Grok Build。一个窗口，全部装下。按需启动——不输入，不运行，不占资源。

### 换人，不换脑子

切换 Agent，上下文不会断。

Marionette 自动生成 `handoff.md`。一个普通的 Markdown 文件。看得见，改得了，可以提交 Git。不靠黑魔法。

### 一套配置，全线打通

MCP、Skills、上下文——不需要配置四遍。

扫描一次，统一管理，全部共享。同一个工具，OpenCode 能用，Claude Code 也能用。

### 十兆字节，恰到好处

不到 10MB。没有后台进程。

打开就工作，关闭就消失。轻，但不是简陋。

---

## 还有更多

- **消息卡片** —— 所有 Agent 输出统一渲染，思考过程折叠，工具调用清晰
- **对话标记** —— 选中消息添加批注，下次打开还在，可带入 Composer 引用
- **每一笔消耗，都摆在桌上** —— 余额、额度、速率限制，右侧栏实时刷新
- **持久化** —— 所有会话存 JSONL，全文可搜索，重开不丢
- **Git 感知** —— 项目文件变化自动检测，一眼看到改了什么

---

## 快速上手

```bash
git clone https://github.com/nt-cubic/Marionette
cd marionette
npm install && npm run tauri dev
```

添加项目 → 选择 Agent → 开始对话。

### 便携版（单 exe，无安装包）

在本机双击 `build-portable.bat`（或 `npm run build:portable`），产物：

```
dist-portable/Marionette.exe
```

拷到任意 Windows 电脑双击即可。需要系统已安装 [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/)（Win11 / 较新 Win10 一般自带）。若缺失，启动时会弹窗并给出微软官方下载链接——**不会**静默安装或写开始菜单/卸载项。

目标体积：不到 10MB。

---

## 设计哲学

- **诚实的降级** —— Agent 没有的能力，Composer 不画假按钮
- **少，但是更好** —— 默认结构化视图，Raw Terminal 一键切换
- **跨 Agent 上下文靠文件** —— handoff.md 是纯文本，看得见改得了
- **持久化优先** —— 会话不丢，重开可搜
- **你是操控者** —— Agent 是木偶，你才是提线师

---

## License

MIT
