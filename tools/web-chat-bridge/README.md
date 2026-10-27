# 网页聊天桥（ChatGPT / DeepSeek 网页版 → Marionette）

把 **ChatGPT 网页版** / **DeepSeek 网页版** 接成 Marionette 里的两个「agent」。你在
Marionette 的输入框里手打一句，桥替你敲进真网页；网页的流式回复原样抄回会话流。
**纯聊天**：不接模型共享引擎、不充当 provider、不给任何 agent 供能、没有任何工具。

- 走的是你在网页上聊天的**同一份额度**（ChatGPT 网页免费/Plus 份额、DeepSeek 网页额度），
  与 API 余额、Codex 窗口互不相干。
- 两个站**共用一个浏览器窗口**（每条 Marionette 会话一个标签页）。桥退出只断开连接，浏览器由你关闭；
  会话挂起、重启和重连都会复用仍然打开的窗口与标签页。
- 日常浏览器已开放 CDP 调试时可以直接连接；`BRIDGE_CDP_URL` 可显式指定地址。
  普通启动的浏览器通常没有调试端口，无法直接接管；Chrome 136+ 还限制默认用户目录
  的远程调试。默认使用独立、固定的专用目录，不修改日常浏览器的安全策略。
- 专用目录首次从同种日常浏览器复制配置。浏览器可能拒绝导入部分扩展；浏览器加密、
  数据库锁和网站认证也可能影响登录迁移，**可能需要在专用窗口登录、安装扩展一次**。
- 作为内置 agent 提供，随会话懒启动，30 分钟挂起规则与其他 agent 一致。
  会话恢复和附件传递需要包含相应支持的 Marionette 主程序，首次升级需重新构建。

## 使用（内置）

ChatGPT 网页版和 DeepSeek 网页版直接出现在新建会话的 agent 列表中。桥文件编译进
Marionette.exe（包括便携版），首次启动会话时自动释放到用户数据目录，无需注册脚本或仓库目录。
需要 Node.js 22+ 与 Edge / Chrome；首次使用在弹出的浏览器中登录。
旧版脚本注册的同名条目由内置条目接管，已有会话仍可继续使用。

以下脚本仅供旧版 Marionette 或桥代码独立开发使用。

### 旧版手动注册

一条命令注册两个 agent（幂等，改了桥的存放位置后重跑即可）：

```bat
node tools\web-chat-bridge\register-agents.mjs
```

它把两个自定义 agent 写进 `~/.marionette/custom_agents.json`（Marionette 设置页
「自定义 agent」读的就是这个文件，也可以在界面里手动加——command 填 node 的
绝对路径，args 填 bridge.mjs 绝对路径 + `chatgpt` / `deepseek`）。**不要**注册成
`.cmd` 垫片命令：Marionette 对 .cmd 有「拆 node」优化，非 npm 形态的垫片会被拆成
裸 node（参数全丢，进程假死）。

然后在 Marionette：**设置 → 自定义 agent** 里就能看到两条新 agent
（`ChatGPT 网页版` / `DeepSeek 网页版`；若软件开着，重启一次让它重读注册表）。

新建会话选其中一个，像普通 agent 一样聊。

## 首次使用

1. 新建会话（agent = ChatGPT 网页版 / DeepSeek 网页版），发第一句话。
2. 桥第一次冷启动会把你日常 Edge（或 Chrome）的浏览器 profile **克隆一份**给桥
   （跳过纯缓存；`BRIDGE_CLONE_FROM` 可指定别的源）。之后一直用这份专用配置，
   不重复复制。未被浏览器接受的扩展需要在专用窗口安装；若登录没有保留下来，登录一次即可；
   后续仍可能因网站会话过期需要重新认证。
3. 只有在撞上登录墙 / 人机验证时才需要到浏览器窗口里手动处理，处理完重发消息。

也可以先打开两个标签页处理登录，以下命令**不发送聊天消息**：

```bat
node tools\web-chat-bridge\bridge.mjs chatgpt --open
node tools\web-chat-bridge\bridge.mjs deepseek --open
```

如果你已有开放调试的浏览器，让两座桥都设置相同的 `BRIDGE_CDP_URL`，例如
`http://127.0.0.1:9222` 或浏览器级的 `ws://127.0.0.1:端口/devtools/browser/...`。
连接失败会报错，不会悄悄另开窗口。未设置时优先复用专用窗口，再尝试同种日常浏览器
用户目录的 `DevToolsActivePort`。不扫描其他端口，也不接管其他软件启动的浏览器。

## 行为细节

- **ChatGPT 原排版回复**：桥复制本轮网页回复的 HTML 与已加载 CSS，在现有 Reply 卡片里的
  Shadow DOM 中显示；列表、表格、代码高亮、卡片和网格图保留网页结构，正文与中性色跟随
  Marionette 字体和主题。整页统一纵向滚动，宽内容可横向滚动；链接交给系统浏览器打开，
  代码复制和图片放大由 Marionette 接管。引用浮窗等原站脚本交互不迁移。
  HTML 随消息流式更新并写入转录，拖选时暂缓更新；另保留纯文本供复制、搜索与历史注入。
  CSS 与可导出的图片在 `BRIDGE_STATE_DIR/rich-replies/` 按内容哈希存储并复用，网页 SVG
  图标尽量内联。缓存丢失或提取失败时回退文字。旧记录不自动补 HTML。
  本功能只用于 ChatGPT；首次升级需要重新构建前端与 Rust，再重启桥。
- **一句一句**：回合制。你发一条 → 网页生成完整回复 → 抄回来。回复流式渐进显示，
  底部输入框在回合结束前是锁住的（和其它 agent 一样）。
- **DeepSeek 思考**：网页开启深度思考时，思考区单独通过 ACP 思考事件流式传回，
  使用 Marionette 现有的思考预览与折叠详情；正式回答仍单独显示。每轮只读取新增内容，
  不重抄历史思考；等待正式回答时，思考中的短暂停顿不会结束回合。
- **DeepSeek 正文排版**：将网页正文的语义结构转为 Markdown，保留段落、标题、加粗、
  斜体、嵌套列表、引用、链接、表格、行内代码和带语言的代码块，使用 Marionette 的
  字体与主题。过滤代码复制等网页控件；公式暂保留为 TeX 文本。生成中以完整正文快照
  更新同一张回复卡，避免网页补格式时重复追加文字；思考仍走原有独立通道。
  首次升级需重新构建前端与 Rust 并重启，旧记录不自动补回格式。
- **会话隔离与续聊**：新建 Marionette 会话，首次发送时打开新的网页对话；同一会话的
  后续消息继续对应的网页对话。主程序用 `MARIONETTE_SESSION_ID` 传入稳定会话编号，
  对话 URL 和标签页编号按「站点 + 会话编号」分别保存在
  `%LOCALAPPDATA%\MarionetteWebBridge\state-<站点>-<会话哈希>.json`，挂起或重启后恢复。
  首次升级此机制需要重新构建主程序；旧版 exe 没有稳定编号，只能保证进程内续聊，
  桥重启后会打开新对话。旧的每站 `state-<站点>.json` 不自动关联给任何会话，
  避免新会话误接旧对话；旧网页对话仍在网站历史中。`--open` 仅用于打开登录页面。
- **图片与文件**：通过输入框的 ＋ 或拖入选择附件，截图可直接粘贴；支持只发图片。
  图片通过 ACP 图片块传入；普通文件用独立元数据传入，仅显式选择的文件会上传，
  不会扫描正文、历史记录中提到的本地路径。发送队列与路径授权等待都会保留附件。
  桥通过网页原生文件上传入口选择文件，等附件预览出现、上传/解析结束且发送按钮可用后
  再发消息；上传失败或超时会停止发送并显示提示。中途取消或失败后，如网页还有残留附件，
  请移除附件并刷新该标签页，再重发。
  ChatGPT 首次加载时会等待附件入口挂载，必要时打开输入框的＋菜单；发送兼容新版「发送」按钮。
  - 常用图片：PNG、JPEG、WebP、GIF、BMP、SVG；单张图片最多 12 MB（宿主图片链路限制）。
  - 文档：TXT、Markdown、PDF、Word、CSV、JSON、代码文件；DS 当前上传入口还接受
    Excel、PowerPoint 等。具体格式、文件大小、数量、额度以各网站和当前模型限制为准，
    接受上传不等于模型具备原生视觉能力（DS 网页可能仅提取图片中的文字）。
  - 文件夹不上传；选择其中的文件。首次升级附件链路需要重新构建前端与 Rust 主程序。
- **记忆卡 / 历史注入**：Marionette 注入的跨 agent 记忆卡与挂起重连的历史块会被桥
  剥掉，网页只收到你手打的那句话。
- **⏹ 停止**：会停止键会让网页停止生成（按钮或 Escape），已生成的部分照常收下。
- **文件访问范围**：网页只收到本轮显式添加的附件，桥不提供通用文件读取、命令或引擎接口。

## 风险与维护（先读）

- **账号风险**：两个网站的条款都禁止自动化访问网页界面。人速、一句一句、人守在
  旁边、验证码手动点掉，实践中多年少见出事，但风险始终在且落在账号上。介意的用
  官方通道（ChatGPT → 任意会话选 `codex/*` 共享模型；DeepSeek → API key 或
  OpenCode Go 的 deepseek 模型）。
- **页面改版**：选择器失效时，会话里会出现「注入失败 / 没接受这条消息」的提示，
  详细原因在 `%LOCALAPPDATA%\MarionetteWebBridge\bridge-<站点>.log`。改
  `bridge.mjs` 里对应站点片段的选择器即可（文件内有中文注释标明每段职责）。
- **验证码 / 登录过期**：提示出现时到浏览器窗口手动处理，然后重发消息。

## 环境变量（可选）

| 变量 | 作用 |
|---|---|
| `BRIDGE_BROWSER` | 指定浏览器 exe 绝对路径（默认自动找 Edge → Chrome） |
| `BRIDGE_CDP_URL` | 连接已开放调试的浏览器，仅接受本机 HTTP / 浏览器级 WebSocket 地址；两个站填相同地址 |
| `BRIDGE_HEADLESS=1` | 无头模式（不弹窗口；人机验证没法手动救，不建议） |
| `BRIDGE_POLL_MS` | 回复轮询间隔，默认 700ms |
| `BRIDGE_MAX_WAIT_S` | 单回合最长等待，默认 300s |
| `BRIDGE_STATE_DIR` | 状态/用户目录根，默认 `%LOCALAPPDATA%\MarionetteWebBridge` |
| `BRIDGE_CLONE_FROM` | 首装复制源 `User Data` 目录（默认选择与浏览器 exe 对应的 Edge 或 Chrome） |
| `MARIONETTE_SESSION_ID` | 主程序自动注入的稳定会话编号；直接运行 ACP 探针时不设置则每次建立新会话 |

## 浏览器 profile（克隆机制）

- 桥窗口的 profile 在 `%LOCALAPPDATA%\MarionetteWebBridge\bridge-profile`，首装时
  从日常浏览器克隆（`.seeded-from` 记账，只做一次）。日常浏览器的后续改动**不会**
  同步进来——两边各聊各的。需要重新初始化时，先关闭专用浏览器，再把 `bridge-profile`
  改名备份，下次启动会创建新目录；原目录中的登录和设置仍可保留恢复。
- 日常浏览器**正在运行**时也能克隆（robocopy 对源只读）；个别被锁文件会跳过，
  Local State / Preferences 齐全只表示基本配置存在，不代表登录迁移完整。
- 旧的分站目录（`profile-chatgpt` / `profile-deepseek`）保留备查，不再自动启动它们
  迁移 cookie，也不自动删除。日常浏览器的文件只作为复制源。

## 开发自测

```bat
node tools\web-chat-bridge\probe.mjs echo
```

echo 是内置的本地假聊天页（流式 + 代码块），完整走 ACP 握手 → 记忆卡剥除 →
历史注入剥除 → **两轮对话（第 2 轮断言不重抄第 1 轮的回复）** → end_turn →
cancel 通知，**无外网、无登录**。探针通过即桥的协议面正确；真站点的选择器
需要登录后人工冒烟（也可 `node probe.mjs chatgpt "第一句" "第二句"` 直接真站跑两轮）。

> 2026-10 起注意：ChatGPT 新版 UI 的回复节点是
> `div[data-markdown-text-style="assistant-message"]`（旧版
> `data-message-author-role="assistant"`）——桥里两条都挂着，改版再失效时
> 照上一节改 `CHATGPT_ASSISTANT_SEL` 即可。
