# 改进路线图：来自竞品调研的行动项（2026-10-31）

> 证据底稿见 [`research-competitors-2026-10.md`](./research-competitors-2026-10.md)。
> 原则延续 [`CURRENT.md`](./CURRENT.md) 产品底线：不做自研 runtime、不做 IDE、不做云同步；
> 所有「补齐」都以 ACP/引擎已有的能力为前提，不自造协议。

## P0 — 本轮已落地

- **会话导出为 Markdown**（`src/lib/exportTranscript.ts` + Ctrl K「导出当前对话为 Markdown」）：
  对话主干 + thought/tool 折叠块 + 连续 file_change 合成交付表 + plan/ask 卡裁决行。
  调研判定 transcript 导出是「正在成为 table-stakes」的能力（opcode 可导出、各家聊天壳标配），
  且 `api.ts::loadTranscript` 的注释本来就登记了「导出」这个全量消费者。纯前端、零新依赖。

## P1 — 补齐 table-stakes（协议水位）

1. **成本/token 仪表盘**：ACP `usage_update` 已 stabilized（上下文占用 + 累计成本），按会话/按日聚合，
   与引擎额度面板（`/v1/magpie/quotas`）并成一张「花了多少 + 还剩多少」的视图。opcode 的用量分析是其核心卖点之一。
2. **checkpoint / rewind 暴露**：三大 CLI 均已内建（Claude `/rewind`、Gemini 自动 checkpoint）；
   GUI 不露出即落后。先从适配器透传开始，UI 上做「回到这轮之前」。
3. **MCP 配置 UI 一等化**：「能力与上下文」已有借贷/清单基础（`context_inventory`），
   补连接状态测试与失败原因展示（opcode 的注册表+连接测试是参照）。
4. **ACP registry 一键安装 agent**：自定义 agent 现在要手填命令；registry 元数据（41 个 agent 带版本+安装方式）
   可以变成「发现 → 安装 → 登录」三步。

## P2 — 差异化纵深（独有面上加厚）

1. **交付卡 → review 工作流**：逐文件 accept/reject（回滚单文件）、diff 行内评论回喂给 agent、
   一键开 PR（Vibe Kanban/Nimbalyst 已验证该模式的留存价值；我们的按轮 git 真数是更深的地基）。
2. **圆桌可视化编排**：M5 能力感知组队（画像→组队→分层背书）之上，补运行中泳道/成员视图，
   把「谁在提案、谁背书了谁」画出来——品类里多模型共识编排近乎空白。
3. **定时/后台任务**：cron 式无人值守会话 + 跑完汇报（桌面端几乎空白；与已有定时发送、挂起巡检同族）。
4. **Telegram 远程深化**：从「通知+审批+回复」延伸到远程列会话/切会话/看交付卡摘要——
   竞品的 Telegram 桥多而浅，深度集成（定位到会话、共享 waiter）是我们的既有优势。

## P3 — 按需跟进（市场起来了再做）

- 看板任务视图（Vibe Kanban sunset 后社区维护，模式仍被需要）。
- live 会话分享 / Multiplayer 围观（Conductor Multiplayer、Happy 已验证，工程量大）。
- 自动化接口：CLI/MCP server 形态驱动 Marionette（Superset 的「可编程」路线）。
- 云/远程沙箱（关机继续跑）：与「不做云同步」底线冲突，仅评估借力上游（Codex 云工作区）的呈现方式。

## 明确不做

- 自研 agent runtime / 模型路由器（底线不变）
- IDE 化（完整文件树/编辑器）
- 云同步 / 自建账号体系

## 风险与观察

- **品类拥挤**：ACP clients ~150、桌面 host 40+。胜出靠纵深与留存，不靠功能清单长度。
- **上游收窄**：Claude Agent Teams、Codex 云原生收走「单家 GUI 包装」的空间——跨厂商中立性是上游做不到的位置，也是我们必须守住的。
- **存续信任**：品类里连续出现弃更/倒闭（Crystal、Vibe Kanban、getAsterisk）；公开路线图、稳定发版、可审计（council 全消息 append-only 落盘这类设计）本身是竞争力。
- **协议跟进**：v2 draft 的 `session-fork`、`mcp-over-acp`、telemetry RFD 落地后重估 P1/P3 优先级。

## 执行纪律（沿用 plan-codeg-borrow 的规矩）

- 每个行动项先在 `docs/` 开调研/设计稿，落地后回写 `CURRENT.md`；过时挪 `archive/`。
- P1 各项相互独立，可并行；P2.1 依赖交付卡现状（已具备），P2.3 依赖定时发送基建（已具备）。
- 每批带 verify 脚本回归（`scripts/verify-*.mts`），i18n 过 `scripts/inventory-zh.mts --check`。
