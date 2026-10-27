# Marionette

**Agent Harness, All in One**

每个模型，都应该运行在它最擅长的环境里。

Claude 用 Claude Code，Grok 用 Grok Build，GPT 用 Codex，DeepSeek 用 DeepSeek Harness。

你不需要一个 Agent 做所有事。你只需要一个窗口，让它们各司其职。

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screenshots/main-dark.png">
  <img src="docs/screenshots/main.png" alt="Marionette 主界面">
</picture>

`v0.5.1` · Windows 10 / 11 x64 · 单文件便携版，打开即用

[下载](https://github.com/nt-cubic/Marionette/releases/latest) · [现状与细节](docs/CURRENT.md)

---

## 为什么存在 Marionette

AI 世界没有统一的 Agent。

Claude 最懂 Claude Code，Grok 生来就是 Grok Build，Codex 与 GPT 是一家人。强塞进别人的壳里，效果都要打折扣。

真正的问题不是谁更强，而是为什么一定要在它们之间做选择。

Marionette 不替代它们。Marionette 把它们连接起来。

## 一个窗口，所有 Agent

OpenCode、Claude Code、Codex、Grok Build、ZCode、DeepSeek Harness、Pi、Hermes。一个窗口，全部装下。

按需启动——你不输入，它不运行。会话是标签页，最多四格分屏，不同的 Agent 可以并排干活。

## 两条侧栏，就是工作台

左边找，右边看。

项目、会话、聊天、收纳盒在左；Plan、Todo、用量与本轮改动在右。所有东西都在该在的位置，不用在聊天流里翻找。

## 把桌面还给你

桌面被终端和 IDE 占满太久了。

Marionette 不要求你为它让出屏幕。拖到屏幕左右边缘，它收成一条窄带，只留一线把手；想用就滑出来，用完自己退回去。

Agent 在边上继续跑。谁在跑、跑到哪一步、刚改了哪几个文件，扫一眼就有；想派活，就在窄带里发。

窗口关掉它还在跑——真正退出才叫退出。手机上的 Telegram 也能收到、也能答。

## 换人，不换脑子

从 Claude 切到 OpenCode，上下文不会断。

Marionette 生成一份普通的 `handoff.md`。看得见，改得了，能提交 Git。不靠黑魔法。

命令行里聊过的，点一下就收进列表，按时间排在一起，接着聊。

## 一个来源，喂给任何 Agent

OpenCode Go、DeepSeek、GLM、Kimi、OpenRouter、xAI 的钥匙，Grok 与 ChatGPT 的订阅，都能出现在任意一个会话的模型菜单里。

不用为每个 Agent 配一遍。

## 一套配置，全线打通

MCP、Skills、长期记忆——不需要配四遍。

扫描一次，统一管理。同一个工具，这个 Agent 能用，那个也能用；你的偏好在换人时跟着走。

## 还有更多

- **收纳盒**：拖进来，再整理成对话
- **交付卡**：每轮改了什么，一眼看到
- **Ctrl K**：跳转、新建、切主题、退出
- **并行子代理**：各干各的，不刷屏
- **Telegram**：手机上收到，也能答
- **图片批注**：圈出来，直说要改哪儿

## 快速上手

下载后双击即可。从源码跑：

```bash
git clone https://github.com/nt-cubic/Marionette
cd Marionette
npm install && npm run tauri dev
```

添加项目 → 选择 Agent → 开始对话。需要 WebView2 Runtime（Win11 与更新过的 Win10 自带）。

---

## License

MIT，全文见 [LICENSE](LICENSE)。
