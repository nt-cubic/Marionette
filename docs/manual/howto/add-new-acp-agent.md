# Howto：接入一个新的 ACP agent（如「谷歌反重力」）

> 场景：一个新 agent CLI 支持 ACP（agent_client_protocol），要把它接进 Marionette：
> 能启动、能选模型、能带 MCP、能记账、能被引擎桥接。参照物：`opencode` / `zcode`
> 两家在 `agent_registry.rs` 的接法（zcode 是社区桥接的完整样板）。
> 深度索引见 [../acp-agents.md](../acp-agents.md)。

## 0. 先摸清对方（决定接入面大小）

1. 它的 ACP 适配器是**原生**还是**桥接脚本**（zcode-acp-server 形态）？
2. session/new 的 zod 收什么形状？（mcpServers 是 stdio 还是 http/sse？——决定
   `should_inject_mcp` / `accepts_stdio_mcp` 怎么答）
3. 模型清单从哪来（initialize 应答 / 自家配置文件 / 引擎桥）？
4. 有没有登录态（凭据发现要不要写）？它自己的会话库在哪（usage 回填 reader）？

## 1. 核心注册（最小可跑）

`src-tauri/src/agent_registry.rs::harness_meta` 加一个 match 臂（~142 起 opencode、
~199 起 zcode 是两个样板）：

- `AgentHarnessMeta`：id / 显示名 / `cmd`（启动命令）/ spawn 形态 / 模型来源。
- `build_client_capabilities_json`（:242）：按对方能力声明放行（zcode 的
  `opencode/child-session-updates` 式特判按需加）。
- `should_inject_mcp`（:286）/ `accepts_stdio_mcp`（:298）：对方的 MCP 形状。
- `launch_env_for`（:303）：要注入的环境变量（zcode 的 ZCODE_BIN 形态）。

## 2. 按需逐项（每项先确认需要再加）

| 项 | 文件 | 什么时候要 |
|---|---|---|
| 凭据发现（只读） | `credentials.rs`（1743 行，四家样例） | 有登录态 |
| 更新链 | `agent_update.rs` | 有自更新机制 |
| 方言补充（effort 档位/模型菜单） | `grok_dialect.rs` + 前端 `acpSupplements.ts` | 档位/菜单不报或要兜底 |
| 引擎唤醒源 | `engine.rs`（spawn wake；`MARIONETTE_*` 相关特判） | 它能用 marionette/* 桥接模型 |
| 引擎侧账号桥 | 见 [add-new-subscription-account.md](add-new-subscription-account.md) | 它是订阅账号 |
| usage 回填 reader | `src-tauri/src/usage_backfill/`（六家样例；契约在 mod.rs 头注释，前端 `usageBackfill.ts` 镜像**字段名/语义两侧同步改**） | 想让分析页算到它的历史 |
| 会话 id 去重 | `nativeSessionIds` 自报（`listSessions`） | 回填接了就必须 |
| 上下文盘点/elicitation | `context_inventory.rs` / `elicitation.rs` | 按其能力 |
| i18n 文案 | fe-locale 片段 | UI 出现它的名字/描述 |

## 3. 坑（接入前必读）

- **session/new 的 MCP 自愈**（acp.rs）：适配器可能广告 `mcpCapabilities.http:true`
  但 zod 只收 sse 形状——注入被整单拒收、agent 起不来。别为每家维护矩阵；
  现有兜底：params 被拒且注入非空时摘掉 mcpServers 重试一次。
- **每 agent 的 spawn wake 是引擎懒启动的合法隐式 enable 入口**——别把新 agent 的
  wake 漏了（引擎睡了模型菜单就空）。
- 前端模型归属：agent 自报 `currentModel` 兜底会话 `preferredModel`，不是账本真相。

## 4. 验证

```bash
cd src-tauri
cargo test --features engine-rust --bin marionette agent_registry   # 注册表测试（含新臂）
cmd /c "..\check.bat check"                                          # 默认构建零变化
cd .. && npm run build                                               # 前端绿
```

真机：spawn 一轮对话（流式 + 工具轮 + 附图）→ 模型菜单 → MCP 注入生效 →
usage 台账出四桶 → 若接了引擎桥：引擎被唤醒、桥接模型可用。
回归脚本相关项（`npx tsx scripts/` 下 agent/usage 相关，先 `ls scripts/` 对照）。
