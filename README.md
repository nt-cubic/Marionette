# Marionette

**你 -> Marionette -> 所有 Agents**

鞋子每天在变，但人没有理由削足适履。

使用提线木偶（Marionette），让它帮你操作所有 Agents。

Opencode、Claude Code、Grok build、Codex……将他们配置进同一个壳里。

让所有 Agents 坐在一起，共享同样的知识和经验。

你不用适应工具，让工具来适应你不断精进的做事方法。

在即将到来的时代，让 AI 成为你的部下，开始真正的运筹帷幄。

> v0.1.0 —— 核心可用，持续打磨

---

## 为什么存在 Marionette

Agents 更新 + 定价变化 + 用量限制 = 总是需要换用 Agents

Marionette 让你在同一个工具里，轻松操纵所有 Agents。

关注你作为人，使用 AI 的能力，而不是让人来适应持续变化的 Agents。

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
- **@ 委派** —— 消息里 @ 另一个 Agent 转交任务，上下文自动交接
- **拖进去，就懂了** —— 拖入文件、图片直接进对话，Agent 不用等描述
- **选中即批注** —— 选中消息文字快速加注，随下一条消息一起发出
- **智能回复** —— 文件刚拖进来、AI 给了选项、上轮报错……芯片一键接话

---

## 快速上手

```bash
git clone https://github.com/nt-cubic/Marionette
cd marionette
npm install && npm run tauri dev
```

添加项目 → 选择 Agent → 开始对话。

### 直接下载

```bash
gh release download --clobber -p Marionette.exe
```

拿到最新的 `Marionette.exe`，双击即用。

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
