# OpenBot（CopilotKit）对标：Marionette Chief / 员工模式

日期：2026-11-18。范围：只读调研，未改代码。对象是 **CopilotKit/OpenBot**（owner 确认的 "openBot"），
不是 OpenClaw。`docs/research/openbot-benchmark.md` 那份对标的是 OpenClaw，结论不适用于本稿。

## 0. 结论先行

- OpenBot 是 CopilotKit 做的**自托管「AI 同事」工作台**，技术栈 TypeScript + PostgreSQL + Docker/Kubernetes，
  核心卖点是**治理**：每个动作先过网关、策略 deny-first、先写审计再执行。它是一个 3 个月大、增长很快的项目，
  不是成熟产品（多份二手评测与其 CHANGELOG 都标注 alpha/早期）。
- 它和 Chief **不是同一类东西**：OpenBot 的 Bot 本质是「模型 + 工具/MCP + 每人一个 Chromium 容器」，
  面向团队/企业的 SaaS 式部署；Chief 是「本地桌面 + 真 coding agent（ACP）+ Telegram 远程」，面向一个人。
  两边重叠的是「员工/同事」的产品语言和「治理」的设计意识。
- Chief 在**本地优先、真 agent 运行时、Host 硬门禁、急停/预算/记忆可检视**上明显更强，也更轻（无服务端、无 Postgres、
  无容器、无第三方托管依赖）。
- 最值得借的是**机制层**而不是产品面：策略 dry-run 与拒绝原因、initiator 字段（谁触发的动作）、
  「本会话允许/始终允许」粒度的审批、routine 连续失败自动停用、统一审计页。这些都能落在现有 Rust 账本里，
  不需要引入 CEL、容器或 Intelligence 服务。

## 1. OpenBot 是什么（事实，附来源）

| 项 | 值 | 来源 |
|---|---|---|
| 仓库 | CopilotKit/OpenBot（主页 open.bot） | GitHub API |
| 创建 | 2026-08-17 | GitHub API |
| Stars / Forks | 12,745 / 1,580（2026-11-18 API 读数；页面显示 12.7k / 1.6k） | GitHub API、仓库页 |
| 近况 | pushed_at 2026-11-18 01:33Z，仍高频提交 | GitHub API |
| 未关 issue | API 128 = 63 issues + 65 PR（页面分拆数字） | 仓库页、API |
| 语言 | TypeScript | GitHub API |
| 许可 | 应用源码 MIT；CopilotKit Intelligence 单独授权 | GitHub API、README |
| 增长参考 | 二手评测称 9 月 1 日已过 3,500 星（未核实） | WebSearch 二手评测 |
| 版本 | CHANGELOG 可见 1.0.x、1.1.0，**日期均未标注**；一篇二手文章称 1.0 于 11-09 发布（未核实） | CHANGELOG |

定位（README）：「the open, self-hostable alternative to closed AI bots」，built on CopilotKit 与 AG-UI。
对外有三层：个人版（桌面安装包，无登录，单用户）、团队版（公司登录、角色、Admin、审计，附 General Assistant 与 Knowledge 两个 Bot）、
CopilotKit Intelligence（单独授权的云/本地服务，提供持久线程与记忆）。

## 2. 架构

- **前端**：React/Vite；桌面端签名安装包（macOS 双架构、Windows），另有 web、iOS、Android、Slack、Teams、短信（团队版）。
  Slack/Teams/移动端标注为 early access。
- **网关**：所有浏览器、文件、shell、MCP 动作先经服务端网关：解析目标、按 CEL 规则评估（deny 先于 allow，策略缺失则放行为零，
  规则出错则拒绝）、写审计行、再执行；失败再写一行。拒绝带原因。
- **Agent 运行时**：内建 Bot 跑在 OpenBot 服务端（`BOT_PROVIDER` 选 OpenAI/Anthropic/Google，或 OpenAI 兼容端点）；
  外部 Agent 通过 **AG-UI** 接入（任何 AG-UI 端点都能当 Bot，类型包括 built-in、remote-ag-ui、remote-mastra）。
  README 没有点名内建 Bot 用的 SDK，不能确认是哪家。
- **计算**：每人一个隔离容器（Chromium + /workspace + 浏览器 profile，绑定 127.0.0.1，单容器 token）；
  可选 gVisor（runsc）。Kubernetes 为默认部署形态（Helm）。
- **数据**：PostgreSQL 存产品数据、策略与审计；凭据加密存储、不回显、审计中脱敏。
- **记忆与线程**：依赖 CopilotKit Intelligence（free 计划可本地用，但是预览、macOS + Docker Desktop；生产需单独授权）。
- **生成式 UI**：组件库 `app/src/components/gallery/`，渲染前服务端校验组件存在、已发布、未对该 Bot 隐藏；
  数据函数按组件授权；`/admin/playground` 可写沙箱组件并免部署发布。这是 AG-UI/CopilotKit 的主要用途。
- **技能与工具**：Skill = 指令（`/` 调用，个人技能只挂作者的 Bot）；能力由工具授权决定。工具经 MCP，目录含 Google Drive、
  Notion、Parallel/Exa/Firecrawl，Composio（可选 Arcade）接大量其它应用。
- **定时**：Routine 以发起人身份运行，在提问所在频道回报；最短 15 分钟、最多 20 条启用、连续失败 10 次自动停用。
- **人机接管**：Bot 遇到登录/2FA 可请求人接管，接管期间 Bot 动作被拒。
- **安全默认**：`OPENBOT_SINGLE_USER=true` 在 `.env.example` 中默认开启（单用户模式下任何能访问者都是管理员）；
  私有地址需白名单；Helm 需要许可证与默认 StorageClass。

## 3. 用户反馈（好评 / 抱怨）

**好评（二手评测 + 仓库）**：治理是产品核心，网关先于动作；deny-first、缺策略则放行为零的 fail-closed；
`initiator.kind` 区分人、部署、routine、handoff 发起的动作；任意 AG-UI 端点即插即用；发版节奏快。

**抱怨 / 风险（二手评测与仓库 issue）**：
- **alpha**，只能当模板 fork，无 npm 包、无托管版；需固定版本而非 `:latest`。
- **强依赖 Intelligence**：服务端没有 Intelligence 凭据起不来；自托管 OpenBot 不等于摆脱供应商依赖。
- **默认策略偏宽松**（评测称 bundled launch policy 允许全部动作；本稿未核实当前版本默认值，1.0.1 已把发送/删除/分享/发帖改为可询问）。
- **资源重**：每个 Bot 一个 Chromium 容器，笔记本上「加起来很快」。
- **策略文档薄**，缺 cookbook。
- Issue 区可见的实际问题（截至 2026-11-17）：
  - Windows 授权文件夹挂载失败（#1763，路径前缀 `\\?\` 问题，#1766 修复中）；
  - macOS 顶部条会拖动整窗（#1751，草稿）；Linux 桌面版上线 PR 在途（#1536）；
  - ChatGPT 订阅登录的 Bot 对简单消息不回答（#1690）；
  - Composio 请求头不匹配导致连接失败（#817）；
  - 「内置」员工无法移交工作（#399）；
  - Shopify 订单备注被策略当成「转账」误判（#890）、13 个授权生命周期转移缺审计（#876）；
  - 中文界面需求（#767）。
- 社区评价被二手评测描述为「热情但浅」，未见独立基准。

## 4. 安装 / 上手 UX

- **桌面版**：签名安装包（macOS、Windows）。但它仍依赖 Docker Desktop 或 Podman（1.0.2 起可自动装/升级，校验签名）。
  Windows 上 1.0.2 会建一台名为 `openbot` 的 rootful Podman 机器。
- **源码版**：`git clone` → `cp .env.example .env` → `bun install`（要求 Bun ≥1.4，1.3 有测试失败）→
  `npx copilotkit login` → `local setup` → `local connect` → `bash scripts/start.sh` → 填模型 key → 打开 localhost:3010。
  Windows 必须走 WSL2 + Ubuntu。
- **单容器**：`ghcr.io/copilotkit/openbot`（内嵌 Postgres）。
- **Kubernetes**：Helm（约 3.15 vCPU + 0.4 vCPU 安装任务），本地只能 port-forward 试用。
- 上手摩擦：需要 CopilotKit 账号、Intelligence 本地 license（30 天，可续）、模型 key、Docker，多步命令；
  不是双击即用。个人版首启无登录，适合单机。

Marionette 对照：便携版 exe（`tools\build-portable.bat`）与 `install.ps1` 一行安装，免管理员、无 Docker、无账号；
中英界面齐备（OpenBot 中文 UI 仍在需求中）。

## 5. Marionette 侧事实（Chief / 员工模式，2026-11-17 工作树）

- **形态**：Tauri 桌面单机；远程入口只有 Telegram（`telegram_remote.rs`，长轮询 + 置顶总控面板），底层仍是**单个配对 chat_id**，
  多员工不能同时绑定不同私聊（plan §偏差）。
- **运行时**：员工与 worker 都是真 ACP 会话（Claude Code、Codex、Grok 等），worker 是 `origin="chief-worker"` 的项目会话，
  不写 `parent_session_id`。
- **门禁**：项目白名单 fail-closed、`remote_mode`（只读/审批写/预授权写/阻断）、并发与墙钟上限、员工只读由 Agent 的 plan/只读档强制
  （不支持则 fail-closed）；控制工具 schema 永不含 cwd/permissionMode/env 等。`staff_commands` 工具数以 plan 为准为 16 个
  （AGENTS.md 旧表述仍写 10 个，文档漂移）。
- **急停**：`pausedUntilUserResumes` 持久，重启仍生效；「全部停止」连 Proposed 任务、worker 浏览器标签、Computer Use 共享窗口一起收。
- **记忆**：`pinned.md` / `state.md` / `history.jsonl` / journal / proposals，原子写、来源引用、tombstone、可检视可改可忘；
  注入有文件级上限，无总预算（plan 偏差）。
- **承诺**：`commitments.json`，到期唤醒，条件只认 Host 产生的 task 完成/失败（不是通用表达式）。
- **通知闸门**：`notification_allowed` 渠道无关，普通通知有冷却与小时额度，持久化；无完整渠道矩阵。
- **审计**：task 账本（`~/.marionette/chief/`）+ tee 事件流；**没有**统一的审计/健康页（plan 列为未施工）。
- **浏览器与电脑**：内置浏览器标签（grant、handoff Continue/Deny）；Computer Use 封闭 op 集、exe SHA 校验、黑名单。
- **代码规模**：`src-tauri/src/chief/` 约 8k 行 + `telegram_remote.rs` 5.2k 行；测试 55 项（chief::）。

## 6. (a) 功能差距表

判定：**Chief 更强** / **持平** / **差距（值得补）** / **不做（定位不同）**。

| 能力 | OpenBot | Marionette Chief | 判定 |
|---|---|---|---|
| 运行时 | 内建 LLM Bot（工具循环）或 AG-UI 端点 | 真 coding agent（ACP，多家） | Chief 更强 |
| 执行隔离 | 每人一个容器（Chromium+shell，可选 gVisor） | 宿主进程；Job Object 收树；worktree 仅 prompt 约束 | **差距（安全边界）** |
| 渠道 | Web/桌面/iOS/Android/Slack/Teams/短信 | Telegram（单 chat） | 差距（Telegram 对个人已够；Slack/Teams 不必） |
| 多用户/组织/SSO/角色 | 团队版有 | 单用户本地 | 不做（定位不同） |
| 策略语言 | CEL，deny 先于 allow，fail-closed | Rust 硬校验函数，fail-closed | 持平（OpenBot 表达力强，Chief 更简单可审） |
| 策略 dry-run、拒绝原因 | 有（dry-run 对历史审计） | 有拒绝返回值，无 dry-run 面板 | **差距（可补）** |
| 审计页 | `/admin/audit`，含拒绝与失败 | 账本 + tee，无统一页 | **差距（可补）** |
| 发起人（initiator） | 人/部署/routine/handoff | 有 origin 标记，未统一记录「谁触发」 | **差距（可补，小）** |
| 审批粒度 | 本会话允许 / 始终允许（按发送/删除/分享/发帖） | 权限卡逐次批准 + remote_mode 项目级 | **差距（可补）** |
| 定时/routine | 15 分钟最小间隔、上限 20、连续失败 10 次自动停 | commitments 到期唤醒，无失败自动停 | 差距（小，但 Chief 更克制） |
| 记忆 | Intelligence 云端记忆 + 自动学习 | 本地分层、可检视、proposal 审核、tombstone | **Chief 更强**（OpenBot 依赖外部服务） |
| 人机接管 | take the wheel，接管期间拒绝 Bot 动作 | browser handoff（Continue/Deny） | 持平（Chief 缺实时接管画面） |
| 急停 | 未见文档 | 持久 `pausedUntilUserResumes` + 全部停止 | **Chief 更强** |
| 通知闸门/预算 | 未见文档 | 闸门 + 预算（部分） | **Chief 更强**（但未完整） |
| 生成式 UI | CopilotKit gallery，免部署发布组件 | spec 卡（card/table/steps/chart） | OpenBot 更强（Chief 不需要） |
| MCP 应用目录 | Drive/Notion/搜索 + Composio/Arcade 众多应用 | 插件层 http MCP 中继，手动配 | **差距（低成本可补）** |
| 本地模型/任意 provider | 需 Intelligence；本地模型未官方测试 | 本地引擎 + 任意 ACP agent | Chief 更强 |
| 安装 | 桌面包 + Docker/Podman，或源码 + Bun + WSL | 便携 exe / install.ps1，无 Docker | Chief 更强 |
| 语言 | 英文为主，中文需求中 | 中英双语 | Chief 更强 |
| 服务端/数据库 | 必需 PostgreSQL | 无 | Chief 更轻 |
| 计算机使用（桌面控制） | 无 | Computer Use（封闭 op 集） | Chief 更强 |

## 7. (b) 值得借鉴的优先级（按 收益 / 与架构契合 / 工作量）

1. **统一拒绝原因 + 策略 dry-run（S–M）**。把 `validate_task_start`、`validate_task_access` 等的拒绝统一成
   `{rule, reason, initiator}` 结构，在员工档案里提供「如果这样派活会怎样」的只读试算。契合：Chief 策略已是纯函数，
   天然可试算。不引入 CEL，只借「可解释的拒绝」。
2. **initiator 字段（S）**。task、worker、tee 事件、commitment 唤醒统一记录发起方：`user` / `telegram` /
   `commitment:<id>` / `task:<id>` / `host`。UI 上能看出「这件事是谁让做的」。契合：origin 已有雏形。
3. **统一审计页（M）**。把 task 账本、拒绝、通知抑制、承诺唤醒按时间线合并，支持按员工/项目/结果过滤。
   plan §10 已列为未施工，OpenBot 的 `/admin/audit` 给了明确的验收形状（permitted/refused/failed 三态）。
4. **审批「本员工本项目始终允许」（M）**。仅对低危动作粒度（例如同一 worker 的同类编辑、同一项目的读取），
   写进员工档案，由 Host 持久化，可在档案里撤销。**绝不**允许模型自己写入这条规则（红线 3）。
5. **routine 连续失败自动停用（S）**。commitment 唤醒连续 N 次失败后自动置为 paused 并通知用户，
   契合「不做无依据工作」红线，且防止失败循环烧预算。
6. **MCP 应用目录的一键接入（S–M）**。插件层已有 http MCP 中继与密钥不外流规则，可加「推荐 MCP」清单
   （Drive/Notion/搜索/Composio 类）；注意 Composio 的请求头差异（#817）与凭据只进 Host。
7. **Telegram 多绑定（M）**。把 transport 从单 chat_id 扩为员工→chat 映射，解除 plan 已列的偏差。
   这是 Chief 自身的待办，不是借 OpenBot。
8. **生成式卡片的组件白名单发布（L，暂缓）**。spec 卡已有 schema 校验，可借「组件必须在白名单内才能渲染」的思路，
   但没有迫切需求，不建议现在做。

明确**不借**：Intelligence 式云端记忆、每人一个容器、Postgres/Helm 多租户、CEL 引擎、AG-UI 全量接入。

## 8. (c) Chief 做得更好、必须保留的

- **本地优先、零服务端**：无 Postgres、无 Intelligence、无账号；便携 exe 一行安装。这是 OpenBot 最大的部署成本。
- **真 coding agent 运行时**：worker 是 Claude Code/Codex 等完整 agent（ACP），不是自写工具循环；能力与 agent 自身生态对齐。
- **Host 硬门禁**：只读由 Agent 的只读档强制，不支持就 fail-closed；控制工具 schema 无 cwd/permissionMode/env；
  「员工不能雇员工」「没有 approve 工具」（写走权限卡）。这些比 OpenBot 的「策略 + 工具授权」更直接，且不依赖表达式正确性。
- **持久急停与预算闸门**：`pausedUntilUserResumes` 重启仍生效；「全部停止」连带收 Proposed 任务与浏览器标签。
- **记忆可检视可改可忘**：本地文件、来源引用、tombstone、proposal 逐条接受/拒绝。OpenBot 的记忆在 Intelligence 里，用户看不见文件。
- **事实与投影分离**：员工行状态是投影，task/tee 是事实，模型报告不能覆盖。
- **Computer Use 的封闭 op 集与 exe SHA 校验**：OpenBot 靠容器隔离，Chief 靠协议收窄，两者取舍不同，Chief 的方式对本地用户更省资源。
- **克制的主动性**：红线 13「没有依据不工作」「展示主动性不是任务来源」；OpenBot 的 routine 是用户主动配的定时任务，没有这层约束。

## 9. (d) 风险

1. **隔离差距是真风险**。worker 在宿主进程里跑真 shell，无人值守时（员工自主模式）一旦策略或 prompt 失守，
   影响面大于容器方案。缓解：优先把只读与审批默认值做硬；worktree 目前只是 prompt 约束（plan 偏差），应补 Host 侧强制。
2. **借 CEL 或表达式策略会扩大攻击面**，还会让策略不可审。只借 dry-run 与可解释拒绝，不引入表达式引擎。
3. **概念竞争**：OpenBot 的「AI 同事 + 治理」叙事与 Chief 的员工模式高度重叠，12.7k 星带来的社区注意力会吸走早期用户。
   Chief 需要把「本地、真 agent、Telegram 远程」讲成清晰的差异，而不是跟着它做团队版。
4. **功能蔓延**：OpenBot 的 Slack/Teams/短信/移动端/Knowledge Bot 很容易让 Chief 偏离单人远程助理的定位，违背 AGENTS.md 的
   「只负责启动/展示/切换/记录 agent，不实现 agent runtime」边界（Chief 已经越界到编排，需守住线）。
5. **文档漂移**：AGENTS.md「远程助理 / Chief」仍写 10 个控制工具，plan 与代码是 16 个；本批借鉴落地前应先对齐。
6. **许可**：OpenBot 为 MIT，若复制代码需保留版权与许可声明。建议只借概念、按 Marionette 的 Rust 结构自行实现。
7. **证据质量**：本稿 OpenBot 的架构与安装信息主要来自 README（一手）与 CHANGELOG（一手，但无日期）；好评/抱怨、增长曲线、
   1.0 发布日期来自二手评测（部分带推广性质），标注为未核实。默认策略是否仍宽松未在当前版本核实。
8. **没有实际运行**：本稿未安装或真机跑 OpenBot，所有判断基于源码结构与文档，不是性能或体验基准。

## 10. 证据与局限

- 仓库元数据：GitHub API `repos/CopilotKit/OpenBot`、`git/trees/main`（只读到 17 个顶层目录，树被截断）、`issues`（前 40 条按评论排序，截断到 #1686 附近）。
- 一手文档：`README.md`、`CHANGELOG.md`（前 4 个版本段）、仓库主页。
- 二手：andrew.ooo 评测（2026-09-18，状态可能已过时）；WebSearch 命中的 wavect.io、runtimewire.com 等文章，
  数字未逐一核对，仅用于定性。
- 未读：`app/src` 与 `api/openapi`、`api/asyncapi.yaml` 的具体实现、`agent-claude-sdk` 的集成代码、
  `.claude/skills`；这些是下一步若要深挖需要补读的内容。
- 本稿未发送任何用户或项目数据到外部，所有外部请求均为公开页面的通用读取。
