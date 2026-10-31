# 竞品调研：CLI 编码 Agent 的桌面管理壳（2026-10-31）

> 调研方法：两路并行网络调研（产品逐个深挖 + ACP 生态面），信息截至 2026-10-31，来源以内联链接为准。
> 结论落进行动项的部分在 [`plan-competitor-roadmap.md`](./plan-competitor-roadmap.md)；本文是证据底稿。

## 一句话格局

「universal ACP GUI manager」已是**拥挤赛道**：ACP 官方 registry 收录 41 个 agent、clients 页约 150 个项目（桌面/Web 类 ~60 个）。但**Windows 原生 + 多模型网关 + 多模型共识编排**这个组合仍然稀少；品类头部（Conductor/Vibe Kanban）一个不支持 Windows、一个已随公司倒闭 sunset。

## 逐产品速览

| 产品 | 栈/平台 | 状态 | 一句话 |
|---|---|---|---|
| [Conductor](https://www.conductor.build) | Mac 专属，闭源 | 非常活跃（v0.91，2026-10-15） | 品类标杆：并行 workspace + Cloud 沙箱（Vercel microVM）+ iOS + API + Multiplayer 协作；$50/月。用户抱怨：审批/沙箱控制弱、agent 认证失败常见、小仓库太重 |
| [Sculptor](https://imbue.com/product/sculptor)（Imbue） | Mac/Linux，MIT | 维护中 | 安全隔离先驱：每 agent 独立 worktree + Pairing Mode + 插件系统。HN 高票请求：更多 agent、移动端看状态、Electron 资源占用高（"该用 Tauri"） |
| Crystal → [Nimbalyst](https://nimbalyst.com/blog/best-conductor-alternatives-2026) | Electron 全平台 | Crystal 2026-02 弃更 | 继任者：kanban 按任务选引擎、逐文件 diff accept/reject、Plans/Trackers 拆条目逐条开会话（与圆桌思路最接近的竞品） |
| [opcode](https://opcode.sh)（原 Claudia） | **Tauri 2 + React（同栈）** | 22.4k★ 但主仓库无安装包、维护存疑 | 最出名的开源 Claude Code GUI：用量分析（可导出）、MCP 可视化管理、checkpoints/分叉"时间旅行"。抱怨：Windows 支持曾是重灾区、"SO MANY bugs" |
| [Vibe Kanban](https://github.com/BloopAI/vibe-kanban) | Rust+TS Web 看板 | **2026-04 公司倒闭，sunsetting** | 看板编排模式的开创者：任务上看板、agent 在隔离 workspace 执行、diff 行内评论回喂、内置浏览器预览、一键 PR。独立小团队产品存续风险的标志性案例 |
| [AionUi](https://github.com/iOfficeAI/AionUi) | Electron | 活跃（2026-09） | **与 Marionette 技术路线最接近**：ACP 接 20+ CLI agent、自动检测本机 CLI、Cowork 多 agent 组队、v2 支持远程/Web 访问。~33.7k★，中英社区 |
| [Happy](https://github.com/slopus/happy) | Expo 全平台 | 极活跃 24.4k★ | 移动遥控标杆：端到端加密同步、推送审批、Multiplayer 围观共驾、语音编码、任意 ACP agent |
| 厂商原生 | — | 快速收窄中 | Claude Desktop Code 标签页（并行+worktree+diff）、Codex App（云工作区、任务关机继续跑）、**Claude Code Agent Teams**（CLI 原生多 agent 编排）。"GUI 包装单家 agent"的窗口正被上游收窄 |
| 2026 新入场 | — | — | [Pane](https://github.com/greenfield-inc/Pane)（键盘优先+远程）、Emdash（SSH 远程+自动检测 CLI）、Paseo（自托管全客户端）、[Superset/Paradev](https://github.com/superset-sh/superset)（YC P26，可编程驱动）、cmux、Agent HUD（刘海审批）等——长尾众多 |

## 功能矩阵（Marionette 视角）

✅ 独有/领先：**多模型本地网关**（全部竞品都是 BYO key 单 agent 直连）、**圆桌多模型共识编排**（仅 Nimbalyst 的 Plans 沾边）、**收纳盒**、**按轮交付卡**（git 真数的每轮 +/−）、**zh/en 双语**、**Windows + Tauri 轻量栈**（Conductor/Superset/Capsule 等一批 macOS-only；Sculptor 被点名"该用 Tauri"）。

⚠️ 已有但需深化：推送通知（本地）、移动/远程（Telegram 桥已覆盖通知+远程审批+回复会话，属"半差异化"——竞品 Telegram 桥多但深度集成少见）、子代理可视化。

❌ 市场已验证、我们缺的：成本/token 仪表盘、MCP 配置 UI（opcode 有注册表+连接测试）、checkpoint/rewind 暴露、看板任务视图、PR/GitHub review 流、自动化接口（API/CLI/MCP server）、云或远程沙箱、内置浏览器预览。

## 跨竞品最高频痛点（市场验证的机会）

1. **Windows 支持缺口**——Nimbalyst/Pane/Emdash 全部把"Conductor 的 Windows 替代"当核心卖点；我们正中空档。
2. **沙箱/审批控制不足**——Conductor 用户因"agent 无实质限制"弃用；隔离 + 细粒度审批是留存关键。
3. **移动/远程监控**——"babysitting Claude Code sessions"是刚需（Sculptor HN 高票、Conductor 出 iOS、Happy 以此立足）。
4. **成本/配额可见性**——opcode 用量分析是核心卖点；Agent HUD 以"配额一眼可见"切入；ACP `usage_update` 已 stabilized，成本面板成了协议内建能力。
5. **审查-合并工作流质量**——所有产品都在砸 diff review（逐文件 accept/reject、评论回喂 agent）。按轮交付卡是更深一层的答案，值得往 review 工作流延伸。
6. **资源占用（Electron 原罪）**——Tauri 2 是可写的营销点。
7. **配置/启动摩擦**——agent 认证失败、PATH 识别、.env 不随 worktree 复制；"开箱即跑"是低成本口碑抓手。
8. **存续信任**——Bloop 倒闭、Crystal 弃更、getAsterisk 归档后，用户被烧怕了：公开路线图、稳定发版节奏、可审计开源本身即卖点。

## ACP 生态要点（第二路调研）

- 协议 v1 稳定、v2 draft 中：`session/list/resume/delete`、`usage_update`（上下文占用+累计成本）、config-options 标准选择器、Elicitation、registry 一键安装；RFD 进行中：`session-fork`、`mcp-over-acp`、telemetry 导出、WS/HTTP 远程传输（JetBrains+Block 主导）。
- 三大 CLI 的 2026 动向里 GUI 该跟的：Claude Code `/rewind` checkpoints、plan mode、agent teams（lead+teammates 共享任务列表）、Skills marketplace；Codex 云工作区/关机继续跑；Gemini CLI 自动 checkpoint、subagents、`/memory inbox`、Agent Skills + hooks。
- 桌面 ACP host 已 40+（Kepler、AionUi、Codeg、Bellows…）——**差异化要靠纵深，不靠"能跑多 agent"本身**。

## 对 Marionette 的定位启示

1. 基础盘（会话/成本/diff/审批/worktree）要补齐到协议水位——缺一个 table-stakes 就有了放弃理由。
2. 差异化纵深全在我们的独有面上：跨用引擎的模型治理、圆桌可视化编排、Telegram 深度远程、Windows 原生体验、交付卡→review 工作流。
3. 上游收窄（Agent Teams、Codex 云）不可逆——我们的多 agent/多模型/跨厂商中立性是上游做不了的位置。
