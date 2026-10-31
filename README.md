<p align="center">
  <img src="docs/logo.png" width="128" alt="Marionette" />
</p>

<h1 align="center">Marionette</h1>

<p align="center">
  <strong>Agent Harness, All in One</strong><br>
  每个模型，都应该运行在它最擅长的环境里。
</p>

<p align="center">
  <a href="https://github.com/nt-cubic/Marionette/releases/latest"><img src="https://img.shields.io/github/v/release/nt-cubic/Marionette?style=flat-square&label=%E5%8F%91%E8%A1%8C" alt="release"></a>
  <a href="https://github.com/nt-cubic/Marionette/actions/workflows/ci.yml"><img src="https://img.shields.io/github/actions/workflow/status/nt-cubic/Marionette/ci.yml?branch=master&style=flat-square&label=CI" alt="CI"></a>
  <a href="https://github.com/nt-cubic/Marionette/releases"><img src="https://img.shields.io/github/downloads/nt-cubic/Marionette/total?style=flat-square&label=%E4%B8%8B%E8%BD%BD" alt="downloads"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/nt-cubic/Marionette?style=flat-square" alt="license"></a>
  <img src="https://img.shields.io/badge/%E5%B9%B3%E5%8F%B0-Windows%2010%2F11%20x64-blue?style=flat-square" alt="platform">
  <img src="https://img.shields.io/badge/%E7%95%8C%E9%9D%A2-%E4%B8%AD%E6%96%87%C2%B7English-green?style=flat-square" alt="languages">
</p>

<p align="center">
  <b>简体中文</b> · <a href="README.en.md">English</a>
</p>

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/screenshots/main-dark.png">
  <img src="docs/screenshots/main.png" alt="Marionette 主界面">
</picture>

---

Claude 用 Claude Code，Grok 用 Grok Build，GPT 用 Codex，DeepSeek 用 DeepSeek Harness。

你不需要一个 Agent 做所有事。你只需要一个窗口，让它们各司其职。

Marionette 是一个 Windows 本地多 Agent 图形壳：只负责启动、展示、切换与记录 Agent，不实现 Agent runtime。**单文件便携版，下载双击即用。**

## 一个窗口，所有 Agent

OpenCode、Claude Code、Codex、Grok Build、ZCode、DeepSeek Harness、Pi、Hermes——一个窗口，全部装下。

ChatGPT 和 DeepSeek 的网页版也在里面：登录一次网页，它们就是两个纯聊天 Agent。

- **按需启动**——你不输入，它不运行
- **会话即标签页**，最多四格分屏，不同 Agent 并排干活
- **Ctrl K 命令面板**——跳转、新建、窗口行为、外观、退出，全在这一个入口（详见下文「Ctrl K 面板」）

## 两条侧栏，就是工作台

左边找，右边看。

- 左栏：项目、会话、聊天、[收纳盒](#还有更多)
- 右栏：Plan、Todo、用量与本轮改动

所有东西都在该在的位置，不用在聊天流里翻找。

## 把桌面还给你

桌面被终端和 IDE 占满太久了。把 Marionette 拖到屏幕左右边缘，它收成一条窄带，只留一线把手；想用就滑出来，用完自己退回去。

Agent 在边上继续跑：谁在跑、跑到哪一步、刚改了哪几个文件，扫一眼就有；想派活，就在窄带里发。窗口关掉它还在跑——真正退出才叫退出。

## Ctrl K 面板

任何地方按 **Ctrl K** 唤出命令面板，键盘优先，不打断手上的活：

- **跳转**：按会话名搜索并切换，方向键选择、回车进入
- **新建对话**：在默认文件夹开一个新会话
- **贴边 / 解除贴边**：切换窄带模式（与标题栏贴边按钮等效）
- **关闭行为**：选择点 × 是**直接退出**（默认）还是**后台常驻**（窗口藏起来，会话与引擎继续跑，再启动一次唤回）
- **侧栏常驻**：贴边时左侧面板不自动收回
- **外观**：主题（深/浅/跟随系统）、中英语言一键切换
- **退出**：结束全部 Agent 与引擎后退出

标签页快捷键与浏览器一致：**Ctrl T** 新对话、**Ctrl W** 关标签、**Ctrl Tab / Ctrl Shift Tab** 切换、**Ctrl 1-9** 跳到第 N 个、**Ctrl Shift T** 恢复刚关掉的。

## 换人，不换脑子

从 Claude 切到 OpenCode，上下文不会断。Marionette 生成一份普通的 `handoff.md`：看得见，改得了，能提交 Git，不靠黑魔法。

命令行里聊过的，点一下就收进列表，按时间排在一起，接着聊。

## 一个来源，喂给任何 Agent

OpenCode Go、DeepSeek、GLM、Kimi、OpenRouter、xAI 的钥匙，Grok 与 ChatGPT 的订阅，都能出现在任意一个会话的模型菜单里。不用为每个 Agent 配一遍。

MCP、Skills、长期记忆——扫描一次，统一管理。同一个工具，这个 Agent 能用，那个也能用；你的偏好在换人时跟着走。

## 还有更多

- **收纳盒**：拖进来，再整理成对话
- **交付卡**：每轮改了什么，一眼看到
- **中英双语**：Ctrl K 或侧栏按钮一键切换
- **并行子代理**：各干各的，不刷屏
- **Telegram**：手机上收到，也能答
- **图片批注**：圈出来，直说要改哪儿

## 快速上手

> [下载](https://github.com/nt-cubic/Marionette/releases/latest) `Marionette.exe`，双击即可。需要 WebView2 Runtime（Win11 与更新过的 Win10 自带）。

<details>
<summary>从源码运行</summary>

```bash
git clone https://github.com/nt-cubic/Marionette
cd Marionette
npm install && npm run tauri dev
```

</details>

<details>
<summary>文档地图</summary>

| 想了解 | 去看 |
|---|---|
| 产品现状与每项能力的细节 | [docs/CURRENT.md](docs/CURRENT.md) |
| 全部文档索引 | [docs/README.md](docs/README.md) |
| i18n（中英双语）怎么改文案 | [docs/i18n-guide.md](docs/i18n-guide.md) |

</details>

---

## License

[MIT](LICENSE) · © 2026 nt-cubic
