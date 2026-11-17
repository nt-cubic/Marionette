# 浏览器双模式：Web 工作区与个人浏览器

- 状态：产品边界已定；Web 工作区标签生命周期 Phase A 已实现
- 目标分支：feat/browser-annotation
- 关联：research-browse-ux.md、plan-browser-workspace.md

## 为什么拆成两模式

它们看起来都是“网页 + Agent 对话”，但用户心理模型、登录状态、风险和市场都不同：

| 模式 | 页面来源 | 登录/状态 | 主要任务 | 默认姿态 |
|---|---|---|---|---|
| Web 工作区 | Marionette 内置 WebView2 独立 profile | 不依赖用户日常登录 | 原型、网站、localhost、视觉批注 | 预览优先，可读、可控制 |
| 个人浏览器 | 用户自己的 Chrome/Edge | 用户已有账号、Cookie、扩展 | 社交、视频、邮件、银行、批量整理 | 用户领地，Agent 是获准进入的客人 |

普通聊天用户不需要看到这两套概念。只有进入 Browser 或连接个人浏览器时才出现明确来源。

## Web 工作区

这是当前已经实现并继续优化的路径：

- 内置独立 profile，不承诺接管日常账号；
- Agent 明确要制作或修改 Web 产物时用 display=workspace；
- 浏览器与当前对话同屏，元素批注、截图、dev server probe 直接可用；
- 标签默认后台打开，空闲 Agent 标签可清理；
- 命名、保留、收回标签属于同一条生命周期；
- 不读取用户私人站点，也不把登录当默认需求。

它的优势是零安装、无账号导入、批注闭环和本地开发预览。它不应为了“看起来更像日常浏览器”而引入 Cookie 同步或账号代理。

## 个人浏览器

这是更大的长期方向：Agent 进入用户已经登录的浏览器，帮助读取、整理、筛选和执行重复操作。

典型任务包括：

- Twitter/X、Reddit、YouTube、Bilibili、Discord 的信息整理、收藏、关注和批量清理；
- 邮箱搜索、分类、归档、批量删除；
- 银行或办事网站的“找到某个账单/交易，读出摘要”，以及对任何不可逆操作先做人工确认；
- 用户示范一次流程，Agent 记录后重复执行。

这里不能把内置 WebView2 当成“个人浏览器”。必须连接用户自己的浏览器会话，否则登录、扩展、Cookie 和多设备状态都不成立。

### 连接形态

推荐复用 browse 证明过的结构，但在 Marionette 内收成用户无需理解的一个连接器：

1. Chrome/Edge 扩展负责 tab 列表、isolated-world probe、CDP 动作和页内 WAIT；
2. Marionette 本地 relay 负责会话、授权、审计和 MCP 工具；
3. 扩展与 relay 只走本机回环，首次连接使用一次性配对；
4. Agent 只能操作用户明确 loan 的 tab 或站点，默认看不到其它浏览历史与标签；
5. 页面→Agent 的数据仍按不可信页面数据处理，URL、密码、支付和 token 字段脱敏。

扩展不是架构负担，而是个人模式的安全边界。Web 工作区继续不依赖扩展。

### 两种来源进入同一 Surface

界面仍是一套 Browser + Agent：

- tab 元数据增加 source=builtin|personal；
- 顶部或 tab 内显示“内置预览 / 我的浏览器”，颜色不承担状态；
- 两个来源绝不静默合并；
- 个人模式 tab 默认 read，Control 必须由用户 loan；
- 同一页面的 activity、busy、WAIT、lastUsed、pin、claim 语义与内置模式一致；
- 关闭 Web 工作区不影响个人浏览器标签；关闭 Marionette 也不应关闭用户浏览器。

## 个人模式的信任模型

### 标签授权

- 用户点击扩展按钮把当前 tab “借给 Agent”；再次点击立即收回；
- 授权绑定 tab、origin、浏览器 profile 和当前用户；
- 导航离开 origin 自动降级为未分享；
- Agent 自开 tab 只在个人 profile 中创建自己的 group，不抢当前 tab；
- 用户 pin/claim 的 tab 永不被 Agent close、cleanup 或会话删除碰到。

### 读取与操作

- read：snapshot、console、screenshot、元素元信息；
- control：click、type、select、scroll、dialog；
- 银行、邮箱、社交后台等高风险 origin 可以进一步要求逐次确认；
- 批量删除、批量取关、批量归档不能复用单次 always-approve；
- 页面脚本、WebMCP 公告和页面文字永远不是用户指令。

### 批量操作的 dry-run

“批量删除/整理”先返回计划而不是直接执行：

1. 列出确切对象：标题、时间、账号、目标 URL/ID、数量；
2. 展示将执行的动作和不可逆项；
3. 用户确认后 Agent 才执行；
4. 支持逐项取消、数量上限、节流和失败清单；
5. 平台 API 支持 undo 时提供撤销入口，不支持时明确写“不可撤销”；
6. 永不提供“对当前页面所有按钮执行 JS”这种绕过计划的通道。

### 敏感数据

- 密码、OTP、支付卡号、安全问题的答案不进入 snapshot；
- 银行页面可以做摘要读取，但交易、转账、删除等动作必须 handoff 或明确确认；
- Agent 不得把整个 Cookie、localStorage、网络包导入对话；
- 模型上下文只包含为当前任务选择的最小字段；
- 所有 Control 动作写本地 audit ledger：时间、tab、origin、动作、结果、会话；
- audit ledger 默认不包含输入内容，密码字段永远不记录。

## 工具模型

个人模式不复制第二套 browser tools。现有工具继续使用 tab 作为 handle，只增加来源和授权信息：

- tabs list 返回 source、browserName、owner、lent、readOnly、group、lastUsed、busy；
- browser_open_tab 在个人模式可指定 browser 与 group；
- read/control 仍走同一套 gate；
- handoff 同一套 WAIT 语义；
- 新增 wait/dialog/screenshot_diff 等工具时两种模式共享实现，但个人模式额外做敏感数据策略。

## 分阶段

### Phase A：内置浏览器标签生命周期（本轮）

- chosenId；
- pin / claim；
- user touch；
- idle cleanup 10/30/60/never；
- pinned 恢复；
- 个人模式只落文档，不开假入口。

### Phase B：个人浏览器只读连接器

- Chrome/Edge 扩展 + 本地 relay；
- 配对、连接状态、browser 名称；
- loan 当前 tab；
- snapshot / screenshot / console 只读；
- 页内 WAIT 和通知；
- 不做 click/type。

### Phase C：受控操作

- click/type/select/scroll/dialog；
- per-origin read/control；
- audit ledger；
- 用户立即收回；
- 高风险站点逐次确认。

### Phase D：批量管理

- 平台无关的 dry-run 计划卡；
- 邮箱/社交平台的批量删除、归档、取关；
- 失败与重试清单；
- 可选平台适配器，提升准确性和撤销能力。

## 明确不做

- 不把日常浏览器 Cookie 导入内置 WebView2 profile；
- 不让 Agent 在个人模式默认看到用户所有 tab；
- 不用页面内文字当授权；
- 不把 Control always-approve 自动扩展到批量不可逆操作；
- 不在第一版支持远程浏览器、多浏览器农场或云端中转；
- 不在没有 dry-run 与审计时开放银行/邮箱的批量写操作。

## 验收

- Web 工作区仍然免安装、独立 profile，不依赖个人浏览器连接器；
- 个人浏览器断开时不影响内置标签和会话；
- 关闭 Marionette 不关闭用户浏览器；
- Agent 没有 loan 时不能读取个人 tab；
- read 级工具不能通过页面 JS 绕过成 click/type；
- 用户收回后，未开始的动作立即拒绝；
- 批量计划在执行前能看到完整对象清单和不可逆警告；
- 命名/保留/claim 标签不会被会话删除或空闲 cleanup 误关；Agent 主动关闭时仍遵守用户的保留状态。
