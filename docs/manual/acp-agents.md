# ACP Agent 运行时开发手册（acp-agents）

一句话：外部 agent CLI（codex/dsh/opencode/zcode/grok…）经 ACP 接进 Marionette 的
全部链路——注册、spawn 注入、能力快照、MCP 注入、凭据、re-entry、插件层。

## 文件地图（改 X 找 Y）

| 想改什么 | 去哪 | 备注 |
|---|---|---|
| 注册一个 agent（spawn 命令/能力/环境） | `src-tauri/src/agent_registry.rs::harness_meta` | opencode(:142)/zcode(:199) 是样板；**howto：[howto/add-new-acp-agent.md](howto/add-new-acp-agent.md)** |
| 自定义 agent | `custom_agents.rs` | 用户自建的启动项 |
| spawn 注入/session 生命周期/MCP 自愈 | `acp.rs` | DSH 分支（DSH_ACP_HOST/DSH_SESSION_ROOT）、zcode 分支（ZCODE_BIN/ZCODE_NODE/ZCODE_ACP_RUNTIME）都在这 |
| 能力快照（image 等） | `acp_capabilities.rs` + `acp.rs` 的 CapabilitySnapshot | agent 层 promptCapabilities.image；None=没说≠不能 |
| MCP 注入形状 | `agent_registry.rs` 的 `should_inject_mcp`/`accepts_stdio_mcp` + `tools_mcp.rs` | zcode 只收 url 型（stdio 让 session/new -32602） |
| 传输层 | `acp_transport.rs` | stdio/remote |
| 凭据发现（只读） | `credentials.rs` | 各家登录态 |
| agent 更新 | `agent_update.rs` | |
| elicitation/上下文盘点 | `elicitation.rs` / `context_inventory.rs` | 跨 agent 扫 MCP/技能 |
| 转录流解析 | `src/lib/acpTranscript.ts` | 会话流处理核心 |
| re-entry（自续跑） | `src/lib/agentReentry.ts` + App.tsx | 见下「机制」 |
| 空闲自退 UX | 前端会话状态族 + `sessionSuspend` | 2h 自动挂起、trim 豁免 |
| 插件层 | `plugins.rs` / `plugin_host.rs` / `tools_mcp.rs` | 见下「插件层」 |
| DSH 跨用桥 | `dsh_bridge.rs` | cordis.patch.yml 文本外科手术 |
| zcode stdio 桥 | `zcode_mcp_bridge.rs` | 借 ZCode 后端原生加载 |

## 关键机制（改前先懂）

### re-entry（agent 在 turn/complete 后自己续跑）
- 判定：会话 `waiting` 且距上次回合边界 ≥ `REENTRY_GRACE_MS`（2.5s，滤迟到回声）时，
  assistant/thought 分块武装（状态回 running）；武装后任何分块刷新安静计时。
- 收尾两路：真 `turn/complete` → 正常收尾；没发的（zcode 类）→ 静默
  `REENTRY_IDLE_MS`（6 分钟）后 `sealAgentSegmentForIdle` 封尾卡+状态回 waiting。
- 红线：用户 prompt 一律接管生命周期（performSend 开头解除武装）；冷启动恢复的
  会话永不武装。回归 `npx tsx scripts/verify-agent-reentry.mts`。

### MCP 注入三态
1. stdio 型：多数 agent 直接收；zcode **不收**（`accepts_stdio_mcp=false`）→
   走 `zcode_mcp_bridge.rs`（写 `~/.zcode/cli/config.json` 让 ZCode 后端原生加载）。
2. url 型（http/sse）：走 `tools_mcp.rs` 的 localhost server（每会话 Bearer token）。
3. **session/new MCP 自愈**（acp.rs）：适配器广告 `mcpCapabilities.http:true` 但 zod
   只收 sse 形状 → 注入整单被拒、agent 起不来。兜底：params 被拒且注入非空时
   **摘掉 mcpServers 重试一次**——别为每家维护 per-agent 矩阵。

### 插件层（安装一次、所有 agent 生效）
- 归一成 url 型条目挂 `tools_mcp`（`/mcp/p/<id>` 路由）；条目形状与
  `tools_mcp::session_entry` 同形。**不写任何 agent 自有配置**（codeg 式 per-agent
  扇出已被设计稿推翻，别加回来）。
- 密钥不外流：env/header 值永不进 debug_log、永不进 url 注入条目。
- `plugin_host::release_session` 两个挂点成对（`AcpService::stop()` + EOF teardown）；
  持 `CHILDREN` 锁绝不等待子进程（HelperIo 自锁教训）。
- 插件 JSON-RPC error 原样透传；只有宿主侧失败才编 -32000；child→agent 的主动请求
  回 -32601。
- 技能：`~/.marionette/plugins/skills/` 目录即登记处（不进 index.json）；全局停用 =
  目录内 `.disabled` 标记文件。右栏统一入口只有插件卡（DeepSeek Harness 专属块已删，
  **别加回来**）。

## 坑（接入新 agent 前必读）

- **DSH**：`DSH_ACP_HOST` 必须指全局 dsh 包——适配器自带 runtime 解压符号链接在普通
  Windows 直接 EPERM 崩。`DSH_SESSION_ROOT` 指 `$DSH_HOME/sessions`（与会话导入同源）。
- **DSH 跨用桥**：cordis.patch.yml 每模型必须带引擎目录的 `contextWindow`/`maxTokens`
  + 能看图的补 `input:[text,image]`——不写就是 256K 兜底/图变占位文本（两处实测）。
- **zcode**：ZCODE_BIN 探测链含 lnk 解析（只扫 UTF-16 会命中假路径）；ZCODE_NODE
  必须注入（裸 exec .cjs = `spawn EFTYPE`）；`ZCODE_ACP_RUNTIME=node` 无条件钉。
  完整十一雷：`docs/pitfalls/pitfalls-zcode-acp.md`。
- **zcode 懒启动**：首个 session/prompt 才 spawn 后端——超时/探活别按别的 agent 假设。

## 验证（改完必跑）

```bash
cd src-tauri && cmd /c "..\check.bat test --features engine-rust --bin marionette agent_registry"
cmd /c "check.bat test --bin marionette"   # plugins/plugin_host/tools_mcp/acp 相关
cd .. && npx tsc --noEmit && npm run build
npx tsx scripts/verify-agent-reentry.mts   # 改 re-entry 时
```
真机：新 agent spawn 一轮（流式+工具轮+附图）→ 模型菜单 → MCP 借贷生效。

## 已知差距与源流

- 差距清单位置：各 agent 分支的 acp.rs 注释；zcode 雷 表
  `docs/pitfalls/pitfalls-zcode-acp.md`；DSH 背景 `docs/CURRENT.md`。
- 本手册蒸馏自：AGENTS.md「再次进入对话/共通插件层/DeepSeek Harness/ZCode 接入/
  Agent 进程空闲自退」段 + agent_registry.rs 代码实读。
