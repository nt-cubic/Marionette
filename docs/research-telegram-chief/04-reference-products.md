# 竞品与开源参考

> 调研日期：2026-11-15。产品迭代很快，以下结论以当期官方页面 / 仓库 README 为准。

## 1. 调研问题

本次不是比较“谁的 Telegram 命令更多”，而是比较四个问题：

1. 它把什么当作执行环境：云电脑、本地 CLI、Docker 桌面还是固定 runtime？
2. 用户是在操作一个会话，还是在向一个“助理 / team”交目标？
3. 它有没有任务状态、审批、恢复、后台运行和多 Agent 编排？
4. 它是否适合嵌进 Marionette 现有的 ACP / 项目 / 会话 / 工具模型？

## 2. Grok Bot：产品价值基准

官方页面（2026-08-11）把 Grok Bot 定义为“a team of always-on agents”：

- 每个 Bot 有自己的云电脑；
- 能登录用户已有的应用和网站，在工具、收件箱、网页之间完成端到端工作；
- 用户离开后仍继续，只有需要批准时才回来；
- 用户像给同事发消息一样交任务，手机和桌面可继续同一段对话；
- 多 Bot 可并行，内部常见一个 chief of staff 管理各领域 specialist；
- Bot 可互相传任务、共享上下文，用户只在 judgment call 时介入；
- 记忆与工作方式随长期合作增强。

它真正证明的是：

> “always-on agent + own execution environment + natural-language delegation + approval loop”
> 是一个完整产品，而不是一个聊天壳。

Marionette 不能复制它的云电脑，但可以复制闭环里的本地版本：

- 执行环境 = 用户自己的 Windows / 项目 / 浏览器 / 已共享窗口；
- 常驻 = Marionette 后台运行与任务账本；
- chief = 用户选择的 ACP Agent；
- specialist = 普通 worker sessions；
- approval = 现有 ACP waiter + Telegram 卡片；
- 结果 = changed files、tests、diff、artifacts，而不是只说“完成”。

## 3. 一张表看清

| 产品 | 执行环境 | 远程入口 | 编排方式 | Provider / 模型 | 对 Marionette 的价值 |
|---|---|---|---|---|---|
| Grok Bot | 厂商云电脑 | 手机 / 桌面原生体验 | 多 Bot + chief + specialist | 厂商绑定 | 产品目标和交互基准 |
| Tobs（BotHarness/DeepSeekBot） | 本地 DSH + 本机工具 | Telegram、Slack、Discord、飞书、QQ、微信 | 多个 persona Bot、群聊、委派、定时任务 | 使用 DSH 配置的 provider | 开源 GrokBot 平替、记忆 / 群聊 / 渠道参考 |
| DS Bot（Fei2411/DeepSeek-Bot） | 本地 DSH / 自带 DSH 桌面 | Telegram、微信、飞书等 | 主 Bot Chief、private bot、group goal | DSH 配置 + 本地 CLI | 最接近“Chief + specialists”的现成 UX |
| ccteam | 本机多种 coding CLI / 远端 satellite | Telegram、Lark、Slack、Web | daemon + MCP 委派工具 + 跨机器 | Claude、Codex、Grok、Kimi、DSH、Pi | 最接近的目标架构：任务 / 委派 / ledger / 幂等 |
| Lucarne | 本机 Claude/Codex/Grok 等 | Telegram、微信 | 常驻路由、会话、审批面板 | 复用各 CLI | 远程多会话、权限、Topic / 恢复参考 |
| opencode-telegram-bot | 本机 OpenCode server | Telegram | 单 runtime、多 session、命令面板 | OpenCode / 多 provider | 远程 coding UX、队列、附件、后台通知参考 |
| Angelia | macOS 上的本地 CLI | Telegram / WhatsApp | profile / route / session | Claude、Grok、Codex、Pi | “一 chat 一 route 一 folder”、deny rules 参考 |
| Xiryn | 本机 Docker Linux 桌面 | Telegram | 多 agent、squad、routine、take-over | 多 provider / Ollama | 隔离桌面、可接管、vault、安全 UX 参考 |

## 4. Tobs 与 DS Bot 不是一个项目

这是容易混淆的地方：

- **Tobs**：仓库 `BotHarness/DeepSeekBot`，现在 npm 包名是 `@botharness/tobs`，原先也叫 DeepSeekBot。它是 DSH 插件，主打有身份、人格、Git 记忆、群聊、定时任务、多个 IM 和 Bot 市场。
- **DS Bot**：仓库 `Fei2411/DeepSeek-Bot`，npm 包名 `ds-bot`。它可以把 DSH 本身变成 Bot 应用，也可自带 DSH 桌面；有主 Bot “Chief”、private bot、group goal、Browser、Computer Use、Connectors 和多个 IM。

两者都使用 DSH 的 provider 与本地运行环境，不能直接复用 Marionette 的 ACP 会话、项目、跨用引擎、
Council、Browser Surface 或 Computer Use grant。

## 5. DS Bot 的启发

DS Bot README 中值得借的产品设计：

- 主 Bot 叫 Chief，负责带用户建立其它 Bot 与群聊；
- 每个 Bot 有自己的模型、thinking level、工作目录和权限预设；
- private bot 有独立工作目录、记忆和会话；
- 群聊有 admin 默认统筹，避免每个 Bot 都抢着回复和烧 token；
- 群任务可进入 Goal，由 admin 对齐完成标准、拆分阶段、验证；
- browser 和 Computer Use 都有范围、审批和 takeover；
- IM 消息只在电脑唤醒且 DS Bot 运行时流动；
- 本地 CLI 可接入为 Bot，但 CLI 自己的工具与宿主工具不自动互通；
- Secret card 不能从聊天软件填写，凭据不直接给 Agent。

这些与 Marionette 的方向高度一致，说明产品心智不是臆想。

## 6. 为什么不直接下载 / 嵌入 DS Bot

### 6.1 runtime 和状态重复

DS Bot 自带或依赖 DSH：

- Bot、群聊、记忆、Goal、任务、Connector 都是 DS Bot / DSH 的数据模型；
- 会话与权限也在 DSH 内部；
- 嵌入后 Marionette 仍要维护自己的 project / session / ACP / transcript；
- 同一用户会拥有两套会话记录、两套模型配置、两套工具授权。

### 6.2 无法自然接管 Marionette 的核心资产

Marionette 的差异能力是：

- 多 Agent ACP 壳；
- 模型 / key / 订阅跨用；
- 项目与会话切换；
- Browser Surface；
- Computer Use grant；
- Council；
- 本地 transcript / usage / diff / handoff。

DS Bot 不能直接把这些当作自己的原生工具。通过外部桥接会变成两个宿主互调，既难保证权限，
也无法让用户在本地 Marionette 中看见同一任务的所有轨迹。

### 6.3 产品边界冲突

Marionette 的底线是“壳不实现 Agent runtime”。DS Bot 在 DSH 之上有自己的 Bot orchestration、
memory、channel、scheduler、marketplace 等一整套产品层。把它嵌进来，实际上是在 Marionette 里再装一个
Marionette，违反轻壳方向。

### 6.4 安全面变大

两套 Bot 系统意味着：

- 两套 token；
- 两套 allowlist；
- 两套 approval；
- 两套日志；
- 两套项目路径规则；
- 两套模型 provider 配置；
- 两套升级与供应链。

安全与维护成本远高于在现有 Host 上加 10 个控制工具。

### 6.5 适合直接用 DS Bot 的情况

如果用户：

- 只要 DSH 生态；
- 不需要 Marionette 项目 / ACP / 跨用 / Council；
- 接受在 DS Bot 里重新配置 Bot、记忆和工作目录；
- 只需要一个独立产品而不是本地 Marionette 的统一控制台；

那么直接安装 DS Bot / Tobs 是合理选择。它不是 Marionette 的实现底座。

## 7. ccteam：最接近目标技术形态

ccteam 把已有 coding agents 变成团队，提供：

- daemon；
- 普通自然语言 session 与 MCP 委派工具；
- 父子 delegation topology；
- Telegram / Lark / Slack / Web 控制；
- 完成通知、HITL 审批、文件交付；
- idempotency key、跨重启通知、journaled run；
- policy hook 在每次委派前裁决；
- 预算、并发、max agents、cost ledger；
- 子任务先落盘再通知父会话。

这正是 Marionette Chief 应当借鉴的“Host 任务 broker”方向。不要抄它的产品体积和所有 host 形态，
但应抄它的几个硬原则：

1. agent 只能通过 Host 工具 hire、send、stop、collect；
2. 子任务关系是显式、可追踪的；
3. 任务先持久化，再做副作用；
4. 委派有并发 / 预算刹车；
5. 通知在重启后仍可恢复；
6. policy hook 比提示词更可靠；
7. IM 是控制面，不是 runtime。

Marionette 与其不同的地方是：我们已经有 ACP 多 Agent、项目与会话、模型跨用、Browser 与 Computer Use；
因此不需要再建一个完整外部 daemon 才能实现 Chief。

## 8. Lucarne：通知、路由与权限做得清楚

Lucarne 的 README 明确把自己定位为“不 babysit 本地 AI agent”的远程常驻桥：

- 支持 claude、codex、copilot、gemini、pi、grok；
- Telegram 与微信；
- 本地 CLI 在跑，手机接收进度、审批和失败；
- 引用通知回复；
- Telegram 面板可看 agents、workspaces、历史 sessions；
- 常驻进程和自动释放；
- Windows / macOS / Linux 安装与 autostart。

值得借：

- “一 chat 一 workspace / agent”的路由清晰；
- 常驻服务与 idle 释放；
- 审批成为一等事件；
- Telegram Topic / thread 可用于并行会话；
- 用户不需要新移动端。

不适合直接搬：

- 它主要是现有 CLI 的通知 / 控制层，不是 Marionette 的 ACP 任务 broker；
- 仍以命令面板和会话切换为主，不是“Chief 自动选 Agent / project”。

## 9. opencode-telegram-bot：远程 coding 体验完整

它证明了单 runtime 的 Telegram 客户端可以做到很成熟：

- 远程 prompt、完整结果、代码文件；
- session / project / worktree 切换；
- pinned live status；
- model / agent mode 切换；
- 子代理进度；
- 自定义命令、skills、goals、scheduled tasks；
- interactive Q&A 与 Permission inline buttons；
- 消息队列 / steer / detach；
- 附件、voice、文件浏览器；
- 后台 session notifications；
- strict Telegram user whitelist。

Marionette Chief 应借：

- 长任务不阻塞聊天，提供 status / queue / stop；
- 后台完成通知必须绑定 session；
- 附件与最终 artifacts 的交付体验；
- live status 而不是刷一长串日志。

不应照搬：

- 它把 OpenCode 当作唯一 runtime；
- 用户仍需要大量 `/` 命令管理 project、session、model；
- Marionette 的目标是自动选择 Agent，而非把 OpenCode 控制台搬到 Telegram。

## 10. Angelia：安全路由模型

Angelia 的定位是“coding agent CLI 变成 personal assistant”，核心对象很干净：

- Profile：一个工作目录、指令与记忆；
- Route：一个平台 chat → profile；
- Session：一段持久对话；
- Owner：谁可以批准与使用命令。

值得借：

- 固定 route，避免裸文本串台；
- 每个 profile 的 deny rules；
- 只允许 owner 批准；
- chat / platform / user 三层白名单；
- 不把密钥交给 Agent；
- 群聊与私聊可分开策略；
- 有 scheduled check-ins；
- 可在手机中看到 approval 和 final answer。

限制：

- macOS only；
- 更多是“把 CLI 接到聊天”，不是 Marionette 式多项目任务编排；
- 命令面仍然较重。

## 11. Xiryn：隔离执行环境的安全 UX

Xiryn 让每个 agent 拥有自己的 Docker Linux desktop：

- 不碰用户真实桌面；
- 浏览器、文件、shell 在隔离容器；
- Telegram 远程控制；
- 多 provider；
- routine / squad / memory；
- take-over / hand-back；
- vault 与逐次支付审批；
- 页面需要登录、验证码、支付时交还用户。

值得借：

- agent 需要人类接管时的 hand-off 形态；
- 敏感凭据不直接给 Agent，Host 代填；
- 工作环境可回滚 / 可丢弃；
- Telegram 展示 live progress 和 screenshots；
- 明确的“电脑开着才运行”预期。

不适合直接搬到 Marionette：

- Marionette 的目标是使用用户真实项目和本地工具；
- Windows Docker 常驻成本、项目路径映射、GPU / 本地 CLI 兼容性都更复杂；
- 用户明确希望它改当前软件、用当前电脑，而不是只在一台 Linux 虚拟机里工作。

未来可考虑为高风险任务提供 worktree / sandbox profile，但不把整个产品改成容器桌面。

## 12. 共同规律

### 12.1 Telegram 不是护城河

所有产品都能接 Telegram。真正的差距在：

- 执行环境是否可靠；
- Agent / project / model 选择是否自然；
- 长任务是否能持续和恢复；
- 审批是否可操作；
- 结果是否可见、可验证；
- 权限是否可信。

### 12.2 “一个助理 + 多个可替换执行器”比“一个万能 runtime”更接近 Grok Bot

Grok Bot 的产品语言是 team / chief / specialist；DS Bot 明确用 Chief 管理其它 Bot；ccteam 的 MCP
delegation 直接暴露 agent hire / read / stop。Marionette 应沿用这个层级，而不是强迫用户先学
`/new opencode`、`/use s2` 之类的控制台语法。

### 12.3 任务必须比聊天消息更长命

竞品里真正让远程可靠的设计都包括：

- persistent task / run；
- resume / journal；
- idempotency；
- status / ownership；
- final artifact；
- 重启后恢复。

如果只在 Telegram 里转发一段文本，不叫常驻助理。

### 12.4 安全不是一个开关

成熟产品都把：

- owner allowlist；
- per-profile / per-bot folder；
- permission levels；
- prompt injection warning；
- secrets isolation；
- approval；
- emergency stop；

拆成多个独立边界。Marionette Chief 也应这样。

## 13. 借鉴 / 不借鉴清单

| 借鉴 | 来源 | 如何落到 Marionette |
|---|---|---|
| Chief 固定对话 | Grok Bot、DS Bot | Telegram 私聊永不默认切到 worker |
| 任务即团队工作流 | Grok Bot、Tobs、DS Bot | task ledger + worker sessions |
| 多 Agent 显式关系 | ccteam | Chief → worker，记录 parent/owner，不靠猜 |
| 幂等与重启恢复 | ccteam、Xiryn | requestId、event sequence、interrupted |
| 远程 approvals | Lucarne、Angelia、opencode-telegram | 复用现有 ACP waiter 和 inline buttons |
| live status / queue / steer | opencode-telegram | 任务卡、追加指令、停止、后台完成 |
| 一 chat 一 route | Angelia、Lucarne | chat → chiefSession 固定映射 |
| 可接管 / hand-back | Xiryn、DS Bot | 本地 UI 接管 worker，手机收到状态 |
| 安全 deny rules | Angelia、Xiryn | project policy + remote capability tiers |

| 不借鉴 | 原因 |
|---|---|
| 把 DSH 作为第二 runtime | 与 Marionette ACP 壳重复 |
| 把 Telegram 做成完整控制台 | 与“自然语言 Chief”冲突 |
| Bot 市场 / persona 系统作为首发 | 分散注意力，先证明可靠执行 |
| 任意递归 fan-out | 成本和失控风险高 |
| 云电脑或整机默认开放 | 与用户真实本机和最小授权冲突 |

## 14. 结论

市场已经验证了三件事：

1. 用户愿意从手机给本地 / 云端 Agent 交真实工作；
2. 远程 coding bot 本身不稀缺，真正稀缺的是稳定任务闭环；
3. “Chief + 多个 specialist + 可见审批”是比“命令列表”更自然的产品形态。

Marionette 做这件事有独特优势：它已经拥有多 Agent、Provider 跨用、项目、会话、Browser、Computer Use、
usage 和 Council。只要补上 Host 任务编排与固定 Chief 路由，就能形成一个区别于 DS Bot / 单一 OpenCode bot
的本地远程助理。

## 15. 资料

- xAI, “Introducing Grok Bot”, 2026-08-11: <https://x.ai/news/introducing-grok-bot>
- BotHarness/DeepSeekBot（Tobs）: <https://github.com/BotHarness/DeepSeekBot>
- Fei2411/DeepSeek-Bot（DS Bot）: <https://github.com/Fei2411/DeepSeek-Bot>
- DS Bot 插件说明: <https://github.com/Fei2411/DeepSeek-Bot/blob/main/ds-bot/README.md>
- ccteam: <https://github.com/firstintent/ccteam>
- Lucarne: <https://github.com/tuchg/Lucarne>
- opencode-telegram-bot: <https://github.com/grinev/opencode-telegram-bot>
- Angelia: <https://github.com/korengast/angelia>
- Xiryn: <https://github.com/SouravBeraAkaSpeed/xiryn>
