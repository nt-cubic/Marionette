# Telegram 远程首席助理（Marionette Chief）研究

> 状态：调研 / 方案，不是现行实现契约。
>
> 日期：2026-11-15
>
> 结论：值得做；应作为 Marionette 的一等产品能力，不把 DS Bot 或其它外部 Bot 嵌入为底座。

## 一句话

用户在电脑上配置一个自己指定 Agent、模型和提供商的 **Chief**；电脑保持开机、Marionette 在后台运行时，
用户可以在 Telegram 私聊里用自然语言把目标交给它，由它选择允许的项目、普通 Agent 会话和工具去执行，
必要时把审批送回手机，任务完成后交付结果。

这里的 Chief 不是“Telegram 命令解析器”，而是 Grok Bot 式的远程首席助理：

- 用户只说目标和结果，不需要记命令、会话 ID、项目路径或 Agent 参数。
- Chief 自己选择已有项目、Agent、模型与工具；能力不足时明确说明，不假装完成。
- 真正干活的是 Marionette 里一个个正常、可见、可恢复的 ACP 工作会话。
- 手机是远程入口，本地 Marionette UI 仍是完整控制台；两边看到的是同一个 Chief 和同一批任务。
- 高风险动作、项目根目录、凭据、系统设置和发布操作由本地 Host 管，不由 Chief 给自己授权。

## Grok Bot 给出的真正启示

Grok Bot 官方把产品描述为“一队常驻 agents”，每个 Bot 有自己的一台电脑，能在用户离开后继续使用
应用、网页和工具做完整工作，完成或需要批准时才回来找用户；它支持从手机或桌面继续同一段对话，
也支持多个 Bot 并行协作。价值不在聊天入口本身，而在下面这条闭环：

1. 用户交付一个真实目标；
2. assistant 记住上下文并自行拆解；
3. assistant 调用可执行环境持续工作；
4. 遇到判断点才请示，非阻塞工作继续；
5. 最终结果落进用户真正使用的工具，而不是只给一段“建议”。

Marionette 已经有闭环所需的大部分本地能力：ACP 多 Agent、项目与会话、模型 / key / 订阅跨用、
浏览器与 Computer Use、本地 localhost MCP、Council、持久化转录和现有 Telegram 通知 / 审批桥。
缺的不是另一个 Agent runtime，而是 **Chief 与工作会话之间的任务编排层**。

## 关键决策

| 决策 | 结论 | 原因 |
|---|---|---|
| 是否把 DS Bot 当作底座 | 不作为底座 | DS Bot 是 DSH 插件，有自己的 Bot、记忆、会话和状态；嵌进来会形成两套 runtime、两套权限和两套记忆 |
| 产品形态 | Marionette Chief / 远程首席助理 | 与用户“随时叫一个 AI 用自己的电脑做事”的心智一致 |
| Chief 是什么 | 一个普通 ACP Agent 会话 | 延续“壳不实现 Agent runtime”的产品底线；用户可自选 Agent / 模型 / 提供商 |
| 工作执行者是什么 | 普通工作会话（worker session） | 可在本地 UI 看见、观察、接管、停止和恢复 |
| Chief 如何调度 | 只给 Chief 注入 Marionette 控制 MCP 工具 | 复用现有每会话 localhost MCP 与权限体系，不新增私有总线 |
| Telegram 路由 | 一个已配对私聊始终对应一个 Chief | 防止裸文本在三四个会话之间串台，也符合“和一个同事聊天”的体验 |
| 第一版编排深度 | 只允许 Chief → worker 一层 | 控制成本、递归失控和排障复杂度；暂不做 worker 再开 worker |
| 第一版并发 | 默认 1 个 worker，可配置到 3 个 | 足够覆盖并行调研 / 修改 / 验证，又避免“Agent 风暴” |
| 危险权限由谁持有 | Host / 本地用户 | Chief 可以请求，但不能修改自己的权限、批准自己的危险动作或扩大项目范围 |
| 数据与恢复 | 任务账本 + 事件账本 + 现有会话转录 | Telegram 消息不是事实源；应用重启后任务与 Chief 上下文仍可恢复 |
| 首发渠道 | Telegram 私有聊天 | 复用现有纯出站长轮询、配对、DPAPI token 和审批 waiter；其它 IM 以后再加 |

## 与现有 Telegram 桥的关系

现有 `docs/telegram-bridge.md` 解决的是“多会话远程通知 / 回复 / 批准”：裸文本去最近活跃会话，
引用消息定向到某条通知所属会话，用户仍然自己充当会话路由器。

Chief 模式是在此之上的产品层：

- 兼容模式：保留现有远程桥，适合用户已经明确知道要操作哪个会话。
- Chief 模式：Telegram 私聊是 Chief；Chief 通过任务工具决定谁来做，用户不再手动切会话。
- 两种模式不能同时抢同一个裸文本。切换由 Host 上明确的“远程工作模式”决定，并在 Telegram 顶部状态中可见。
- 进入 Chief 模式后，worker 的逐条通知应被任务聚合；不能让旧桥和 Chief 各推一遍。

## 文档索引

| 文档 | 内容 |
|---|---|
| [01-product.md](./01-product.md) | 产品定义、用户场景、自然语言体验、原则、非目标与验收标准 |
| [02-architecture.md](./02-architecture.md) | Chief 会话、控制 MCP、任务 broker、Telegram 映射、事件、持久化、恢复和建议代码边界 |
| [03-security-permissions.md](./03-security-permissions.md) | 威胁模型、信任边界、远程能力分级、审批矩阵、审计、急停和隐私 |
| [04-reference-products.md](./04-reference-products.md) | Grok Bot、Tobs、DS Bot、ccteam、Lucarne、opencode-telegram、Angelia、Xiryn 对比与取舍 |
| [05-mvp-plan.md](./05-mvp-plan.md) | M0–M3 分期、工具 schema、施工清单、测试矩阵、风险与开放问题 |
| [06-m0-progress.md](./06-m0-progress.md) | **施工台账（进行中）**：M0 逐 PR 落地状态、tee 纪律、探针用法与剩余清单 |

## 成功标准

第一版不是以“Telegram 能发消息”为成功，而是下面这些场景稳定成立：

- 用户只说“检查 Marionette 最近为什么构建失败，修好并跑验证”，Chief 能选对允许项目、启动一个
  worker、在需要时请求批准、完成后给出改动和测试结果。
- 用户离开电脑数小时，主窗隐藏或最小化，Telegram 仍能开始、观察、继续、停止和收取任务。
- Marionette 重启后，Chief 会话、任务状态、worker 会话关系、待审批事项和最后交付不会丢。
- 任一 worker 卡住、失败或退出，Chief 与手机都能看到明确状态，不会把“沉默”当作“还在工作”。
- 恶意网页、仓库文本或 worker 输出即使要求 Chief 扩大权限，也被 Host 的项目白名单和审批矩阵拦住。
- 用户本地打开 Marionette 时，可以看见 Chief 发起的每一个 worker 会话、工具轨迹、改动与批准记录。

## 本专题不改什么

- 不把 Marionette 改成云端 Agent 平台。
- 不做自研模型、Agent runtime 或新的工具协议。
- 不把 Telegram 变成完整 IDE 或所有设置页面。
- 不自动开放整台电脑、项目根目录、凭据、发布权限或自身升级权限。
- 不在第一版实现群聊 Bot 团队、Bot 市场、递归 fan-out、人格系统和多 IM 同时在线。

实施方案与施工边界见 [05-mvp-plan.md](./05-mvp-plan.md)。
