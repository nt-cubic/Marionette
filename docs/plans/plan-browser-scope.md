# 浏览器范围：只做内置开发工作区

- 状态：2026-11-17 定稿
- 目标分支：feat/browser-annotation
- 关联：research-browse-ux.md、plan-browser-workspace.md

## 决定

Marionette 只做内置 WebView2 浏览器，定位是开发工具。

不做：

- Chrome/Edge 扩展；
- 浏览器 relay / pairing；
- 可配置的外部 MCP 浏览器连接；
- 多浏览器连接；
- 个人浏览器 Agent；
- 社交、视频、邮箱、银行等账号管理或批量操作。

如果用户需要 Agent 进入自己日常使用的浏览器，应自行在 Codex、Claude、OpenCode 等 Agent 中安装
browse。browse 本身就是“浏览器扩展 + 本地 relay + MCP server”的组合；Marionette 不另做桥接入口，
但不会禁止底层 Agent 通过通用 MCP 使用它。

### 用户已经安装 browse 时

只给浏览器装 browse 扩展还不等于任何 Agent 都能控制浏览器。完整执行 `browse install` 后，它会把
`browse mcp` 写进所选 Agent 自己的 MCP 配置；用户也可以选择安装到所有支持的 Agent。此后，在
Marionette 中启动的对应 Agent 可以按自己的 MCP 配置调用 browse，工具调用作为普通 MCP 工具出现在
对话流里。Marionette 的项目上下文清单也会按通用 MCP 规则扫描并允许用户把该条目借给其它 Agent；
MCP 默认不自动借出，需要用户在该项目中显式启用。

这不会创建 Marionette 的 Browser surface，也不会把用户浏览器接到 SurfaceWorkspace。

- Marionette 不探测 browse；
- 不读取 browse relay / pairing；
- 不把 browse tab 导入内置 Browser；
- 不提供 browse 的批准、WAIT、批注或清理界面；
- 不替底层 Agent 判断它是否加载了自己的全局 MCP 配置；Agent 不支持 MCP、没有写入对应配置，
  或适配器无法传递该 transport 时，browse 仍不可用。

如果底层 Agent 因为自己的全局配置或 Marionette 的通用 MCP 借贷而调用了 browse，调用仍在用户自己的
浏览器中执行。Marionette 最多在对话流里展示普通工具调用；browse 的 tab group、WAIT、批注和配对状态
都留在 browse 自己的扩展与 Agent 中，不会变成我们的 Browser surface。

## 我们唯一的 Browser

- 内置 WebView2，独立 profile；
- 服务于 Web 开发、原型、localhost、staging 和视觉验收；
- 与当前 Agent 会话同屏；
- 元素/区域批注、截图、console、snapshot、页面操作都走现有内置工具通道；
- Agent 打开 Web 产物时使用 display=workspace；
- 普通网页研究默认后台打开，不抢工作区；
- 关闭 Browser surface 不关闭会话、文档或收纳盒。

这是普通用户和开发者看到的唯一 Browser 模式，不再区分“个人浏览器 / 内置浏览器”两套入口。

## 从 browse 借什么

只借用户交互和可靠性经验，不借产品边界：

- Agent tab 命名；
- pin / claim；
- 用户收回标签；
- idle cleanup；
- 页内 WAIT；
- challenge detection；
- nextStep；
- wait_for / dialog / screenshot diff。

这些机制都直接改善“开发页面怎样被 Agent 操作和验收”。

## 不借什么

- 不接用户 Chrome/Edge 的登录态；
- 不读取日常 tab、Cookie、localStorage；
- 不安装扩展；
- 不做本地 relay 或配对码；
- 不提供外部 MCP server 配置界面；
- 不做生活账号自动化。

## 当前已落地

### 标签生命周期

- Agent 可传稳定 tab id；
- 同名重复打开返回已有标签；
- 标签可 pin；
- Agent tab 可 claim 为用户标签；
- 用户选择、导航和分享会刷新 lastUsed；
- idle cleanup 支持 10 / 30 / 60 / 分钟或永不；
- pinned、named、busy、waiting_user、handoff 和当前可见 tab 不自动关闭；
- pinned 状态冷启动恢复；
- Agent close 不越过用户 ownership / pin。

### Web Workspace

- Browser / Document / Memo 共用 SurfaceWorkspace；
- Browser 与当前 Agent 对话同屏；
- 支持后台 tab、workspace tab、dev server probe 和 handoff；
- ContextPanel 自动让位；
- session-only 分屏树保持独立。

## 下一步

只沿开发工具方向继续：

1. 页面内 WAIT；
2. challenge 检测与可执行 nextStep；
3. wait_for 与 JS dialog；
4. screenshot diff；
5. tab 按会话折叠和整组关闭。

不再规划扩展、relay、外部 MCP 或个人浏览器连接器。

## 验收

- 产品中没有任何“连接我的浏览器”入口；
- 没有扩展安装、配对码或外部 MCP 配置；
- Web 开发、localhost 和 staging 全程使用内置 WebView2；
- 命名、pin、claim、cleanup 均在内置浏览器内完成；
- 用户需要个人浏览器能力时，明确引导其使用 browse，而不是在 Marionette 内制造半个连接器；
- 产品定位始终是开发工具。
