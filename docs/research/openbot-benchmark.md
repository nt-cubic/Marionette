# openBot 对标：Marionette Chief（员工模式）vs 开源个人助理网关

> **注意：本文对标的是 OpenClaw（误认为 openBot）。** 对 CopilotKit/OpenBot 的对标见 [openbot-copilotkit-benchmark.md](openbot-copilotkit-benchmark.md)，本文关于 openbot 的结论不适用于该项目。

日期：2026-11-18。范围：只读调研，未改任何代码。

## 0. 先定义「openBot」

「openBot」不是一个确定的项目名。按「能在聊天应用里对话、在自己机器上跑 agent 与工具、星数最多」的条件检索，
最匹配的是 **OpenClaw**（早期名 Clawdbot / Moltbot，仓库 `openclaw/openclaw`，MIT，TypeScript）。本报告**假设 openBot = OpenClaw**。
第二、第三名按同一条件取 **Hermes Agent**（`NousResearch/hermes-agent`，Python）与 **nanobot**（`HKUDS/nanobot`，Python，README 自称受 OpenClaw 启发）。
Talon、OpenCrust、Lilo 等星数在个位数到几百，不纳入对标。如果 owner 指的是别的项目，请告知，本报告的对标表需要重做。

## 1. 星数（检查日 2026-11-18）

| 项目 | 星数（检查日） | 来源 | 备注 |
|---|---|---|---|
| OpenClaw | 394.7k（GitHub 页 About，forks 82.9k） | [github.com/openclaw/openclaw](https://github.com/openclaw/openclaw) | 搜索摘要快照从 2026-02 约 10 万、04 约 35 万、06 约 378k、08 约 387k，与页面吻合 |
| Hermes Agent | 264.2k（forks 59.6k） | [github.com/NousResearch/hermes-agent](https://github.com/NousResearch/hermes-agent) | [Star History](https://star-history.com/nousresearch/hermes-agent) 10-29 快照约 258.4k |
| nanobot | 50.2k（forks 8.8k，最新 release v0.4.1） | [github.com/HKUDS/nanobot](https://github.com/HKUDS/nanobot) | 2026-02-02 起步 |

星数取自 GitHub 页面的 fetch 结果，缓存口径可能有数分钟误差；第三方快照互相有出入，以上以 GitHub 页为准。

## 2. 三个对标对象的关键事实

### 2.1 OpenClaw（[docs](https://docs.openclaw.ai)，[repo](https://github.com/openclaw/openclaw)）

- **网关架构**：本机常驻 Gateway 作为会话、路由、工具、通道连接的单一真相源；CLI、Web Control UI（默认 127.0.0.1:18789）、macOS 与 iOS/Android 节点都连它。设计口号是「可信网关 + 不可信执行，策略由代码强制」。
- **通道**：内置 Discord、Google Chat、iMessage、Matrix、Teams、Signal、Slack、Telegram、WhatsApp、Zalo、WebChat；插件另有 Nostr、Twitch 等，合计 20 余个。
- **会话**：DM 默认共享主会话，每个群独立会话；按 agent / workspace / sender 隔离。
- **配对**：未知 DM 发送者需要运营者批准 pairing（新手引导里有配对流程）；WhatsApp 有 `allowFrom` 白名单；群聊可要求 @ 提及。
- **技能与插件**：skill 是按需加载的可重复流程；插件市场 ClawHub 负责发布与信任指引；模型与 agent harness（Claude、Codex、本地模型）都是可替换插件。
- **自动化**：cron 任务、hooks、webhooks；文档未提 heartbeat（但安全公告里有 heartbeat 上下文继承问题，见 §5）。
- **沙箱**：文档说主会话的工具默认在宿主机执行，沙箱需要另行配置。
- **多 agent**：按 workspace/sender 路由；文档对「multiplayer」只有一段描述。
- 来源：[GitHub 页](https://github.com/openclaw/openclaw)、[docs 索引](https://docs.openclaw.ai)。

### 2.2 Hermes Agent（[repo](https://github.com/NousResearch/hermes-agent)）

- **通道**：README 列 Telegram、Discord、Slack、WhatsApp、Signal、CLI，另有 Email、Home Assistant。
- **定时**：内置 cron 调度器，用自然语言设定，输出可投递到任一支持的平台，面向无人值守（日报、夜间备份）。
- **记忆**：agent 自己整理的记忆，周期性提醒持久化；FTS5 跨会话搜索 + LLM 摘要；Honcho 用户建模。
- **技能**：复杂任务后 agent 自动生成技能，使用中改进；兼容 agentskills.io 开放标准，有 Skills Hub。
- **执行后端**：6 种终端后端（local、Docker、SSH、Modal、Daytona、Vercel Sandbox），插件后端如 Singularity。
- **安全**：文档表格列出「命令审批、DM 配对、容器隔离」；迁移文档提到命令白名单与审批模式，API key 需显式 `--migrate-secrets` 才导入。审批机制细节文档未展开。
- 来源：[GitHub 页](https://github.com/NousResearch/hermes-agent)。

### 2.3 nanobot（[repo](https://github.com/HKUDS/nanobot)）

- **通道**：Telegram、Discord、Slack、WeChat、Email、Mattermost、Linear、Feishu，「Why nanobot」里另列 Teams；WebUI 是浏览器界面，不算聊天通道。
- **工具**：shell、web 搜索与抓取、MCP、cron、图片生成、多 agent 委派；文件工具可读图片、PDF、Word、Excel、PPT。
- **语音**：Telegram 语音消息配置 token 后自动转写（来源为 Gitee 镜像上的 README，未在主仓核实）。
- **权限**：「Default Permission」定义为「工作区受限的访问，不是 OS 沙箱」；v0.4.1 说明里提到「sandboxed commands」但页面没解释。
- **自动化**：cron 作为工具能力；WebUI 有 Tasks 与 Calendar 视图；未见 heartbeat。
- 来源：[GitHub 页](https://github.com/HKUDS/nanobot)、[Gitee 镜像 README](https://gitee.com/ipcun/nanobot)（转写一条仅来自镜像）。

## 3. 安全事件（必须读）

OpenClaw 是这一类产品里安全事故最多的，且有公开可核对的数据。

**供应链：ClawHub 恶意技能**

- ClawHavoc：Koi Security 审计 ClawHub 的 2,857 个技能，标记 341 个恶意，其中 335 个属于同一活动，始于 2026-01-27；载荷包括窃取凭据与 macOS 上的 Atomic Stealer（AMOS）。来源：[SecureBlink](https://www.secureblink.com/threat-feeds/open-claw-marketplace-flooded-with-341-malicious-skills-in-major-supply-chain-attack)、[eSecurityPlanet](https://esecurityplanet.com/threats/hundreds-of-malicious-skills-found-in-openclaws-clawhub)、[Antiy 分析](https://www.antiy.net/p/clawhavoc-analysis-of-large-scale-poisoning-campaign-targeting-the-openclaw-skill-market-for-ai-agents/)。
- Snyk ToxicSkills（2026-02-05）：3,984 个技能中 1,467 个有缺陷、76 个确认恶意、283 个在上下文里透传 API key。Bitdefender 估计约 900 个（约 20%）。数字方法论不同，不能互相换算。
- 排名操纵：下载计数可被刷，恶意集成可冲到搜索第一。
- 政策后续：发布需要 14 天以上的 GitHub 账号；发布带审计状态。来源：[clawdocs 已知漏洞页](https://clawdocs.org/security/known-vulnerabilities)。

**网关与认证**

- **CVE-2026-25253**（CVSS 8.8，修复于 2026.1.29）：Control UI 自动连接 query string 里的 `gatewayUrl` 并发送已存 token，可一键 RCE。注意部分二手报道把它错挂到供应链事件上，以 [clawdocs](https://clawdocs.org/security/known-vulnerabilities) 与 GHSA 为准。
- 关键级公告（14 条，多为 9+）包括：配对作用域可铸造 admin 并达到节点 RCE（9.9）、WebSocket 共享认证可自报高权限（9.9）、`/pair approve` 未做作用域子集检查（9.4）、heartbeat 上下文继承绕过沙箱（9.5）、宿主 `.env` 注入绕过环境策略（9.6）、包管理器 override 供应链重定向（9.6）、节点 invoke 审批绕过导致 RCE（9.9）。来源同上。
- 2026-09-11 一批共 75 条，其中 30 条 High，例：cron 工具接受大小写混合的命令载荷（8.8）、Codex 绑定漏掉 owner 授权（8.8）、WhatsApp 登录工具可进入非 owner 回合（8.1）、Slack 群 DM 跳过发送者白名单（8.8）、Exec 白名单解析被转义换行混淆（8.0）、Exec 审批可以超出被审查的工作目录（High）、Unicode 回退可逃出 workspaceOnly 根目录。来源：[GitHub 安全公告页](https://github.com/openclaw/openclaw/security/advisories)（该页首页列出上述 10 条，合计 73 页；CVE 号几乎都没有，仅 GHSA）。
- 其他：`pinclaw` 社区插件监听所有网卡且无认证（CVE-2026-108757，无修复版本）；Gateway 媒体根白名单让沙箱会话读到兄弟沙箱（CVE-2026-102806）。

**模式归纳**：多数高危不是「模型被骗」，而是 **授权只在入口检查一次，在 cron/heartbeat/插件/子 agent/通道之间没有传递或重验**。这正是 Marionette Chief 需要防的类型。

## 4. 用户口碑（有限，多来自二手报道）

- 喜欢：通道数量多、本机常驻 24/7、ClawHub 技能生态、「用手机发指令让电脑干活」的体验；星数增长极快。
- 抱怨：安全事故与补丁节奏（二手报道提到更新不稳定，未逐条核实）；技能市场质量参差；配置复杂，入门要 CLI 与 Node。
- Hermes：自动生成技能与记忆是卖点，有评论批评「星数快但技术深度有限」（来源 [heyuan110 评测](https://www.heyuan110.com/posts/ai/2026-04-24-hermes-agent-v010-deep-review/)，单篇观点）。
- 本次没有抓取 Reddit / HN 的一手讨论，口碑部分的可信度低于安全部分，结论里不作为强证据。

## 5. Marionette Chief 现状（代码与文档核对）

依据：`AGENTS.md` 的「远程助理 / Chief」节、`docs/CURRENT.md` 第 330–358 行、`docs/plans/plan-employee-mode.md`（2026-11-17 对账）、`docs/plans/telegram-bridge.md`、`src-tauri/src/chief/*.rs`、`src-tauri/src/telegram_remote.rs`。

- **员工模型**：员工 = 普通 Chat 会话（`origin="chief"`）；worker = 普通项目会话（`origin="chief-worker"`），**不写 parent**，关系只进 task/event 账本。
- **控制面**：16 个 `marionette_*` 工具，只对员工会话的 `tools/list` 可见，dispatch 双保险；**没有 hire / approve 工具**（`chief/tools.rs` 的测试断言 `marionette_task_approve` 不存在，与文档一致）。schema 禁止 cwd、permissionMode、mcpServers、env、shellCommand 等参数。
- **策略**：逐项目白名单，只读 / 审批写 / 预授权写三档；只读靠 Agent 自报的 plan/read-only 模式硬落地，不支持的 Agent 拒绝启动员工会话；Computer Use 按项目开关。
- **急停**：注册表级 `pausedUntilUserResumes`，`/stop`、Telegram「急停」、右栏「暂停全部」同一路径；新员工继承；恢复必须显式操作。
- **任务账本**：requestId 幂等、并发预留与插入在同一锁内；resume 只接受 `Interrupted → Starting` 的精确 CAS；启动恢复把非终态标 interrupted，绝不标 completed。
- **记忆**：pinned / state / history / journal / index / proposals 分层；task 终态由 Host 自动写 journal；模型只能 propose，用户 accept/reject；清空留 tombstone。
- **承诺**：`commitments.json`，支持到期唤醒与 `task:<id>:completed|failed` 条件触发；暂停、manual、禁后台时不消费到期事件。
- **通知**：普通通知冷却与小时额度持久化，写盘失败则不发；waiting_user、失败、权限/提问/plan、承诺到期可绕过勿扰与频率预算；永久暂停永远优先。
- **Telegram**：配置 BotFather token 后 getMe 校验，token 用 DPAPI 落盘；首个私聊作为待配对请求（TTL 5 分钟）在 UI 接受；**只认一个 chat**；权限/ask/plan 带内联按钮，与 UI 共享 waiter，先答先得；纯出站长轮询，无端口、无 webhook。
- **工具面**：另有浏览器标签（grant 单点授权、handoff、元素批注）与 Computer Use（SHA 校验的 driver、按键白名单、自身窗口黑名单）。
- **明确缺口（文档自承）**：Telegram 只支持单 chat，多员工不能同时绑不同私聊；承诺条件只识别 Host 产生的精确字符串；记忆无总预算、无 7/60 天 gist/dormant 分层；自我修改依赖 prompt 与权限卡，没有 Host 侧 git 强制；无容器或远程执行后端。
- **代码核对**：`telegram_remote.rs` 中没有 voice、photo、document、getFile、audio 相关处理，入站仅文本。`chief/` 里没有面向用户的 cron 表达式或定时任务 UI，只有 `commitments` 的到期时间与 heartbeat 心跳（2s tick，用于唤醒与超时）。

## 6. (a) 功能差距表

判定：**缺失** = 对方有、我们没有；**较弱** = 有但覆盖面或成熟度低；**持平**；**更好** = 我们在可核对的治理上明显更强。

| 能力 | 对方 | Marionette Chief | 判定 |
|---|---|---|---|
| 通道广度 | OpenClaw 20+；Hermes 6+；nanobot 8+ | Telegram 单通道，单 chat | 缺失 |
| 常驻 | OpenClaw 本机 Gateway 常驻 | 桌面壳，关机即断；窗口可贴边收起继续跑 | 较弱（设计取舍） |
| 配对与陌生人控制 | OpenClaw 陌生 DM 需 pairing 批准 + allowFrom | 首个私聊待配对，UI 接受，只认一个 chat | 持平（窄，但更简单） |
| 会话模型 | DM 共享主会话，群独立；按 workspace/sender 隔离 | 员工=持久会话，worker=任务会话且不继承父关系 | 更好（关系可审计） |
| 记忆 | Hermes：agent 自整理 + FTS5 搜索 + 用户建模；OpenClaw 文档未展开 | 分层 + 模型只能提议 + 人工 accept + tombstone + Host 写 journal | 更好（可审计）；较弱（无自动学习、无全文跨会话搜索） |
| 技能生态 | OpenClaw ClawHub；Hermes 自生成技能 + Skills Hub | 技能目录可从界面装，全局开关；无市场、无审计 | 缺失（且有风险，见 §9） |
| 插件 / MCP | OpenClaw 插件 SDK；模型与 harness 可替换 | 共通插件层，远程 MCP 中继，stdio 借贷 | 持平 |
| 定时任务 | OpenClaw cron + hooks + webhooks；Hermes 自然语言 cron 可投递到聊天；nanobot cron + Tasks 视图 | 承诺的到期唤醒，无自然语言定时、无按时开任务的 UI | 较弱 / 缺失 |
| 主动性 / heartbeat | OpenClaw heartbeat（有公告）；Hermes 周期提醒 | 2s 心跳唤醒收件箱；承诺到期；task 完成推送 | 持平 |
| 执行隔离 | Hermes 6 种后端含 Docker、SSH、Modal；OpenClaw 沙箱可选 | ACP 权限卡 + 只读硬门禁；无容器后端；无 Host 侧 git 强制 | 缺失（隔离）；更好（策略表达） |
| 授权模型 | OpenClaw 多层作用域（已多次被绕过） | Host 注册表 + 逐项目策略 + 权限卡先答先得；模型参数不能扩权 | 更好（设计上，需保持） |
| 多 agent 委派 | OpenClaw 按 workspace 路由；nanobot 委派；Hermes delegate | 员工名册 + 并发上限（全局 1–3 + 项目级）+ 任务账本 | 更好（治理）；较弱（广度，单通道） |
| 语音 | nanobot 自动转写 Telegram 语音；OpenClaw 语音节点与语音通话插件 | 无 | 缺失 |
| 图片 / 文件入站 | 多数对方支持（具体通道未逐一核实） | 入站仅文本 | 缺失 |
| 浏览器 | OpenClaw 浏览器 + relay（有 CVE） | 内置浏览器标签，grant 单点、handoff、元素批注 | 持平（能力），更好（授权） |
| 电脑操作 | OpenClaw 节点（屏幕、摄像头）；Hermes 后端 | Computer Use：SHA 校验 driver、按键白名单、自身窗口黑名单；Windows only | 持平（能力窄）；更好（安全姿态） |
| 上手体验 | OpenClaw CLI 引导 + Node；Hermes 安装脚本 | GUI：粘 token → getMe → 私聊接受，DPAPI | 更好（桌面用户） |
| 通知治理 | 文档未描述额度与抑制 | 冷却 + 小时额度持久化、写盘失败不发、等待类绕过、勿扰 | 更好（有据可查） |
| 远程急停 | 未见同等机制 | `/stop` / 「急停」持久化，恢复须显式 | 更好 |
| 任务状态投影 | 未见 | task 为事实，员工行为投影，模型报告不写回状态 | 更好 |

## 7. (b) 值得吸收的优势（按优先级）

估算按一名熟悉本仓库的开发者计，粗略到半周量级。

1. **Telegram 入站媒体（语音 / 图片 / 文件）**。`getFile` 下载 → 语音走 STT（本地 whisper 或用户已配的 provider）转文字后作为普通员工消息送入；图片走 ACP 已有的 image block。架构契合：`telegram_remote.rs` 已有入站文本路径，只需扩展解析与附件落盘（`~/.marionette/` 下 hash 命名）。注意：语音转写会把隐私数据送出本机，需要用户开关。**工作量：S（图片/文件）到 M（语音，含 STT 选型与隐私开关）。**
2. **定时任务（NL cron 的最小版）**。复用现有 commitments 的到期 tick，新增「按时间开一个 worker 任务」的承诺类型，由员工或用户在 UI 设定。关键：**定时触发时必须重新跑项目策略校验**，不能沿用创建时的授权快照（见 §9）。投递结果走现有通知策略。**工作量：M。**
3. **多 chat transport（一个 bot，多员工各绑私聊）**。文档已列为待办。用 `chat_id → staffId` 映射替换单值白名单，入站路由按映射分派；Telegram 论坛话题（topic）也可以一员工一话题。**工作量：M。** 收益：与「员工」产品形态直接吻合。
4. **技能安装前的静态扫描与权限清单**。不必做市场，但在「技能可从界面装」这条路上加：安装时列出技能请求的工具/命令/网络域名，扫描常见窃取模式（读 `~/.ssh`、浏览器 profile、钱包路径、`curl | sh`），命中则需二次确认；技能目录加 hash 固定，变更后重新确认。**工作量：M。** 直接对应 ClawHavoc 的失败模式。
5. **会话与转录的全文搜索**。Hermes 的 FTS5 思路：用 SQLite FTS5 索引转录的文本事件（仅索引 user/assistant 文本，不索引工具输出），供 `marionette_memory_recall` 之外的「我之前问过什么」使用。**工作量：M。** 注意不能破坏「转录按文件落盘」的既有约定，索引是派生物可重建。
6. **Docker 执行后端（只读 worker 或受限 worker）**。给 ACP 子进程套容器，仅对「预授权写」之外的高风险项目可选。**工作量：L**，且 Windows 上 Docker Desktop 依赖明显，建议先做只读沙箱的探针而不是通用后端。
7. **通道抽象（transport trait）**。为 Discord/Slack/WhatsApp 预留接口，但不在本期实现；先把 `telegram_remote.rs` 里的鉴权、节流、通知策略与传输分开。**工作量：M（纯重构）；每接一个通道再 M–L。** 排在最后，因为通道数量不是 Chief 的核心卖点。

## 8. (c) Marionette Chief 做得更好、必须保留的部分

- **能力只来自 Host 注册表，模型参数不能扩权**：`task_start` 的 schema 永远没有 cwd、permissionMode、env 等字段。OpenClaw 的大量 CVE 正是「授权在别处被信任或被继承」。保留这条红线，并加进代码评审清单。
- **单点授权检查**：浏览器与 Computer Use 的 grant 只在 `ensure_read`/`ensure_control` 一处判定；MCP dispatch 只调它。不要为新工具另写一套判断。
- **持久化的急停，恢复必须显式**：`pausedUntilUserResumes` 跨重启保留，新员工自动继承；远程「急停」不能被任何自动唤醒绕过。
- **事实与投影分离**：task 与 event 是事实，员工行、状态条、通知都是投影；模型自报不能改写任务状态（OpenClaw 的 approval 状态有过「审批超出原始上下文仍有效」的问题，正是缺少这层）。
- **requestId 幂等 + CAS 恢复**：`task_start` 与 `task_resume` 都有明确的幂等键和精确状态转移，避免重复派单。
- **记忆是提议制**：模型只能 propose，用户 accept/reject，且有 tombstone 防止被清空的内容被模型复活。
- **通知治理可解释**：冷却与额度持久化，写盘失败则不发（fail-closed），等待类事件的绕过规则写在计划里而非隐式。
- **员工会话不能派生有父子关系的 worker**：worker 不写 `parent_session_id`，避免「子会话继承父授权」的链路。
- **密钥存储**：Telegram token 经 DPAPI 落盘，并且不进 URL。
- **无入站端口**：长轮询出站，没有 webhook、没有监听 0.0.0.0 的控制面。OpenClaw 的 pinclaw 与 Gateway 暴露问题都来自入站监听。

## 9. (d) 他们踩过、Marionette 必须避开的坑

1. **技能市场即供应链**（ClawHavoc、ToxicSkills、排名操纵）。Marionette 目前「技能可从界面装」且无扫描、无哈希固定。在做市场之前，先做 §7.4 的安装确认。
2. **定时与心跳继承授权**（OpenClaw GHSA「heartbeat 上下文继承绕过沙箱」9.5；cron 接受大小写混合载荷绕过校验；隔离 cron 任务重新获得被拒绝的 exec 工具）。**Marionette 的任何定时任务必须在触发时重新读取项目策略、员工暂停状态和 Computer Use 开关**，不能缓存创建时的权限快照；定时任务不能调用 Chief 控制工具本身。
3. **审批绑定到宽泛上下文**（GHSA「Exec 审批可超出被审查的工作目录」High；「flock 包装绕过持久审批绑定」8.8）。权限卡应把批准绑定到**精确命令 + cwd + worktree 路径 + 项目 id**；任何路径改变都重新审批。
4. **路径规范化**（GHSA「Unicode 回退逃出 workspaceOnly 根」；「媒体根白名单允许读兄弟沙箱」）。Marionette 的 worktree、项目根、收纳盒 `files/` 路径比较必须先 canonicalize，Windows 上还要处理 `\\?\` 前缀、8.3 短名、大小写与 NTFS 替代数据流。
5. **允许列表的解析**（Exec 白名单被转义换行混淆；包装器信任内层命令）。Marionette 目前把 shell 命令交给 agent 的权限卡，风险在 agent 自身；若将来做「预授权写」的命令白名单，**不要自己解析 shell**，只允许按 argv 数组匹配，或直接禁止。
6. **每个通道各自实现鉴权**（Slack 群 DM 跳过发送者白名单；WhatsApp 登录工具进入非 owner 回合；Codex 绑定漏 owner 检查）。新增通道或新增入口时，**鉴权只能走一个函数**，并且有一个针对「非白名单发送者」的回归测试。
7. **配对作用域升级**（配对作用域令牌可铸造 admin；`/pair approve` 跳过作用域子集检查）。Marionette 的 Telegram 配对只有一个 chat，但若做多 chat 映射（§7.3），**绑定员工必须是「只能绑定本人的私聊」**，不能用配对码升级为控制面权限。
8. **本地监听与 URL 带 token**（CVE-2026-25253 把 token 放进 `gatewayUrl` 查询串自动连接；pinclaw 监听所有网卡无认证）。Marionette 的 `tools_mcp` 是 localhost 上的 Bearer token；要保证：token 不进 URL、不进日志、不进 `window.location`，且监听只绑 127.0.0.1。
9. **更新节奏**（二手报道的「更新不稳定」）。Chief 的 `staff.json`、`tasks.json`、`events.jsonl` 有迁移屏障与坏档 fail-closed 设计，升级时保持这一点，不要为了快速修复跳过迁移。
10. **自我修改无 Host 强制**（Marionette 自身的已知缺口）。目前靠 prompt 与权限卡。如果不补，至少要在 Host 侧对 `git push`、`git tag`、发布脚本做命令级拦截，而不是只写在 prompt 里。

## 10. 结论与下一步

- Marionette Chief 在**授权、急停、账本、通知治理、记忆可审计性**上明显强于 OpenClaw 这类「通道优先」的网关，这些恰是 OpenClaw 安全公告最密集的区域。
- 在**通道广度、媒体入站、定时任务、技能生态、执行隔离**上明显弱。前三项可以按 §7 的 1–3 名补齐，且都能在现有 Rust/Tauri 架构内完成，不需要引入常驻网关进程。
- 优先级建议：先做 §7.1（图片/文件入站）与 §7.4（技能安装确认）；再做 §7.2 定时任务，但必须先满足 §9.2 的「触发时重验」；§7.3 多 chat 映射是产品形态的关键，可与定时任务并行。
- 未核实项（需要时再查）：OpenClaw 各通道的媒体支持细节；OpenClaw 的审批在聊天里的交互形态；Hermes 的审批机制细节；用户口碑的一手讨论。

## 附：本报告引用的本地文件

- `D:\Myself\Marionette\AGENTS.md`（「远程助理 / Chief」节）
- `D:\Myself\Marionette\docs\CURRENT.md`（第 28 行 Telegram 远程；第 330–358 行员工模式）
- `D:\Myself\Marionette\docs\plans\plan-employee-mode.md`（§0.1 施工对账，2026-11-17）
- `D:\Myself\Marionette\docs\plans\telegram-bridge.md`
- `D:\Myself\Marionette\src-tauri\src\chief\tools.rs`（16 个工具；第 410 行为「不得有 approve」测试）
- `D:\Myself\Marionette\src-tauri\src\chief\broker.rs`、`staff.rs`、`staff_commands.rs`、`commitments.rs`、`memory.rs`、`policy.rs`、`tee.rs`
- `D:\Myself\Marionette\src-tauri\src\telegram_remote.rs`（入站仅文本；急停 `/stop` 约在 2585 与 2708 行）

## 附：外部来源

- [OpenClaw GitHub](https://github.com/openclaw/openclaw)（394.7k 星，检查日 2026-11-18）
- [OpenClaw docs](https://docs.openclaw.ai)
- [OpenClaw 已知漏洞（第三方整理）](https://clawdocs.org/security/known-vulnerabilities)
- [OpenClaw 安全公告](https://github.com/openclaw/openclaw/security/advisories)
- [ClawHavoc 报道 SecureBlink](https://www.secureblink.com/threat-feeds/open-claw-marketplace-flooded-with-341-malicious-skills-in-major-supply-chain-attack)
- [ClawHavoc 报道 eSecurityPlanet](https://esecurityplanet.com/threats/hundreds-of-malicious-skills-found-in-openclaws-clawhub)
- [ClawHavoc 分析 Antiy](https://www.antiy.net/p/clawhavoc-analysis-of-large-scale-poisoning-campaign-targeting-the-openclaw-skill-market-for-ai-agents/)
- [OpenClaw 星数追踪 OSS Insight](https://ossinsight.io/analyze/openclaw/openclaw)
- [Hermes Agent GitHub](https://github.com/NousResearch/hermes-agent)（264.2k 星，检查日 2026-11-18）
- [Hermes Star History](https://star-history.com/nousresearch/hermes-agent)
- [Hermes v0.10 评测（单篇观点）](https://www.heyuan110.com/posts/ai/2026-04-24-hermes-agent-v010-deep-review/)
- [nanobot GitHub](https://github.com/HKUDS/nanobot)（50.2k 星，检查日 2026-11-18）
- [nanobot README 镜像 Gitee](https://gitee.com/ipcun/nanobot)
