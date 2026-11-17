# 浏览器产品边界：开发工具优先

- 状态：方向已定；Web 工作区标签生命周期 Phase A 已实现
- 目标分支：feat/browser-annotation
- 关联：research-browse-ux.md、plan-browser-workspace.md

## 结论

Marionette 的产品定位是开发工具，不转向“个人生活浏览器 Agent”。

- 内置 WebView2 是主路径：Web 开发、原型、localhost、视觉批注和验收。
- 连接用户现有 Chrome/Edge 只作为可选的开发能力：处理需要已有登录态的 staging、内部后台、
  第三方开发者门户和登录后联调。
- 不做 Twitter/X、Reddit、YouTube、Bilibili、Discord、邮箱或银行的日常账号管理和批量操作。
- browse 是“Agent 进入个人浏览器”的成熟参考，但它的用户定位不是 Marionette 的定位；我们借
  tab loan、WAIT、命名、清理和收回机制，不借生活助手市场。

## 两种开发页面来源

| 来源 | 用途 | 登录态 | 默认 |
|---|---|---|---|
| 内置 Web 工作区 | localhost、原型、网站、静态预览、元素批注 | 独立 profile，不依赖日常账号 | 主路径、零安装 |
| 连接开发 tab | staging、内部后台、登录后联调、第三方开发者门户 | 用户已有 Chrome/Edge profile | 可选、显式 loan |

普通聊天和普通 Web 开发不需要连接器。只有页面确实无法在内置 profile 中复现时，才出现“借一个
现有浏览器 tab 来调试”的入口。

## 内置 Web 工作区

- 独立 profile，不导入用户日常 Cookie；
- Agent 明确制作或修改 Web 产物时通过 display=workspace 进入；
- Browser + 当前会话同屏，元素批注、截图、dev server probe 直接可用；
- 标签默认后台打开；Agent 可命名、pin、claim、清理；
- 页面 WAIT、challenge、错误 nextStep 等状态最终服务于开发和验收；
- 关闭内置 surface 不影响任何个人浏览器 tab。

这是默认体验，也是绝大多数用户唯一会看到的 Browser。

## 连接开发 tab（可选）

browse 证明可行的切入口是浏览器扩展 + 本地 relay。Marionette 可以复用这个技术形态，但产品文案
必须始终是开发调试，不是个人助理：

- 用户在 Chrome/Edge 中点击“借给 Marionette 调试”；
- 只授权当前 tab 和当前 origin；
- Agent 可以 snapshot、读 console、截图，并执行开发相关的点击/输入；
- 导航离开 origin 后授权自动降级；
- 用户随时收回；关闭 Marionette 不关闭浏览器。

适合的页面：

- staging / preview / canary 环境；
- 必须登录才能进入的内部管理后台；
- OAuth、支付沙箱、第三方开发者控制台等登录后联调页；
- 用户需要演示一个难以描述的前端交互或 bug。

不适合的页面：

- 社交账号批量取关、清理、发帖；
- 邮箱批量归档或删除；
- 银行、支付、政务账号的日常管理；
- 抓取、搬运个人历史记录；
- 任何与当前开发任务无关的个人数据读取。

## browse 的取舍

直接借：

- tab loan / reclaim；
- 页内 WAIT 与真实用户点击；
- 命名 tab、pin、idle cleanup；
- read/control 分级；
- challenge 检测和可执行 nextStep；
- 页面快照、console、截图、wait_for、dialog。

不照搬：

- 面向 Chrome 生活账号的安装、配对和个人资料管理；
- 社交、邮箱或银行的批量任务；
- 多浏览器农场、远程 relay 和云中转；
- QuickJS 任意脚本平台、录屏与网络抓包产品化；
- 把日常 Cookie 导入内置 WebView2。

## 连接模式的信任边界

- 默认 read-only：用户 loan 后才能 snapshot / console / screenshot；
- Control 只用于开发动作；首次执行和跨 origin 都需要用户授权；
- 页面文字、WebMCP 公告和 DOM 属性都不是用户指令；
- 密码、OTP、token、支付字段不进入 snapshot；
- 所有 Control 动作记录本地 audit ledger；
- always-approve 只对明确 loan 的开发 origin 生效；
- 关闭 tab、收回权限和 Marionette 退出都必须即时让后续工具调用失败。

## 分阶段

### Phase A：内置浏览器标签生命周期（已完成）

- chosenId；
- pin / claim；
- user touch；
- idle cleanup 10/30/60/never；
- pinned 恢复；
- Agent close 不越过用户 ownership / pin。

### Phase B：开发协作状态（下一步）

- 页面内 WAIT；
- challenge / block 检测；
- 错误 nextStep；
- wait_for 与 JS dialog；
- screenshot diff；
- 内置模式先落地，之后可供连接模式复用。

### Phase C：连接开发 tab 的只读原型

- Chrome/Edge 扩展 + 本地 relay；
- 当前 tab loan；
- snapshot / console / screenshot；
- 连接状态和诊断；
- 不做 click/type。

### Phase D：开发联调操作

- click / type / select / scroll / dialog；
- per-origin read/control；
- 用户立即收回；
- audit ledger；
- 只有真实开发联调需求持续出现时才继续。

## 明确不做

- 不做个人生活浏览器 Agent；
- 不做社交、视频、邮箱、银行的批量账号管理；
- 不把 Marionette 做成第二个 browse；
- 不因为“用户基数更大”改变开发工具定位；
- 不让连接器默认看到用户所有 tab；
- 不提供无审计的任意页面脚本执行。

## 验收

- Web 工作区仍然免安装、独立 profile，不依赖连接器；
- 连接器入口只出现在需要登录态联调的场景；
- 产品文案始终描述 staging、联调、调试，而不是账号管理；
- 用户没有 loan 时不能读取现有浏览器 tab；
- 关闭 Marionette 不关闭用户浏览器；
- 命名/保留/claim 标签不会被会话删除或空闲 cleanup 误关；
- Agent 主动关闭时遵守用户 ownership / pin。
