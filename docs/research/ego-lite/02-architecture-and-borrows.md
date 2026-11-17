# 02. ego-lite 架构与可借鉴机制

## 1. 两条执行链路

ego-lite 的 Agent 链路：

```text
Agent CLI
  -> ego-browser Skill / heredoc JS
  -> Node.js harness（开源）
  -> globalThis.ego native binding（闭源）
  -> Chromium TaskSpace / Page / CDP / 原生 Snapshot
```

Marionette 当前链路：

```text
Agent CLI
  -> localhost MCP（每会话 token）
  -> tools_mcp.rs
  -> Rust browser module
  -> child WebView2 + WebView2 CDP
```

两边相同点是“Agent 看不到浏览器 UI 细节，只拿到受控 API”；不同点是 ego-lite 把多步动作写成一段脚本，Marionette 目前一步一个 MCP tool。

## 2. 值得借鉴的七个机制

### 2.1 Code-shaped workflow

ego-lite Agent 可以在一段 JavaScript 中组合多步：

- 打开或复用 Page；
- 等待元素；
- 连续 click/fill/select；
- 只在关键分叉点观察；
- 最后输出一次 snapshot 或结果。

**价值：** 降低 MCP 往返次数和中间结果 token，不是单纯“可以写 JS”。

**Marionette 采用方式：** 第一阶段做受限 typed steps，不做任意 JS。多步动作在 Rust 内顺序执行，保留现有 Control 授权。

### 2.2 TaskSpace / Page 生命周期

ego-lite 区分：

- TaskSpace：一个 Agent 目标的工作空间；
- Page：空间中带永久 label 的 tab；
- managed Page 与 unmanaged tab；
- `adopt`、`release`、`claim`、`finish({keep})`；
- 用户控制期间新增或变化的 tab 不自动归 Agent。

**价值：** 多 Agent 并行时，tab 归属、清理和接管边界可解释。

**Marionette 采用方式：** 已有 owner session、pin、claim 和 idle cleanup。只需补 Page label / task group / finish cleanup 语义，不复制 Space UI。

### 2.3 稳定 Snapshot ref

ego-lite 的 ref 是 SDK 分配的稳定 id，绑定到：

- frame；
- document；
- backend node；
- role/name/stable locator 元数据。

完整 snapshot 替换 active set；局部 snapshot 可以保留未出现的 ref；节点身份不变时 id 不变。

**价值：** Agent 可以在一次 snapshot 后执行多步动作，局部观察也不必丢掉已有 ref。

**Marionette 采用方式：** 先保留当前 snapshot lease 供连续动作，再做跨 snapshot identity merge。完整 backend node ledger 作为第二阶段。

### 2.4 Snapshot 压缩与 locator 校验

ego-lite 在 host 侧继续处理原生 snapshot：

- 去掉空白文本、空 container；
- 单子 container 折叠；
- 过滤 `unstable` / `ambiguous` locator；
- 校验 locator 唯一且指向同一个 backend node；
- 保留 iframe provenance。

**价值：** 减少 token，同时避免给 Agent 一个“看起来稳定、实际会点错”的 locator。

**Marionette 采用方式：** 纯逻辑最适合直接移植；即使暂时不做 OOPIF，也可先改善快照密度和 locator 诚实性。

### 2.5 Wait、actionability 与错误语义

ego-lite 明确区分：

- selector 未出现、出现但不可见、可见但被遮挡、禁用、导航中、页面无响应；
- click 的真实 hit target；
- fill 是否真的生效；
- pending navigation 超时后如何停住并保留可用页面；
- stable error code 与 agent-facing guidance。

**价值：** Agent 不需要靠固定 sleep 和反复完整 snapshot 猜页面状态。

**Marionette 采用方式：** 复用错误分类和测试场景，重写为 WebView2 CDP 版本；保留“拒绝是值不是 transport error”的现有原则。

### 2.6 控制权边界

ego-lite 把浏览器权限弹窗、用户接管和 TaskSpace handoff 统一成 user-control boundary：

- Agent 操作被暂停；
- 不得自动重试绕过；
- 用户明确恢复后才继续；
- page-authored JS dialog 与用户权限提示分开处理。

**价值：** 防止 Agent 在登录、权限和人工接管场景中抢控制权。

**Marionette 采用方式：** 保留现有 `none/read/control` 和 `browser_handoff`，只借“硬停而不是可重试障碍”的表达方式。

### 2.7 运行时对齐的 Skill / help

ego-lite 的 `public-api-schema.ts` 是 API 单一真源，运行时校验、`help()`、Skill 文档和版本输出保持同步。

**价值：** 模型看到的是当前安装版本能用的 API，而不是过时的静态提示。

**Marionette 采用方式：** 从 `tools_mcp.rs` schema 生成 `browser_help` 或浏览器操作 Skill 片段，避免第二套手写说明漂移。

## 3. 只在特定阶段值得借鉴

| 机制 | 建议阶段 | 原因 |
|---|---|---|
| Page label / task group | P1 | 对多 Agent tab 清理有用，但不是当前最大瓶颈 |
| site learnings | P2 | 可复用站点经验，但需要维护、版本和来源验证 |
| download / file chooser | P2 | 开发验收有价值，优先级低于 ref 和 waits |
| OOPIF per-frame session | P2 | 工作量大，受 WebView2 单消息泵与全局 CDP lease 约束 |
| 任意 `evaluate` / `cdp` | 暂缓 | 高风险、调试型需求，和开发工作区主路径不同 |

## 4. 明确不借

- ego-lite 的闭源 Chromium 和原生 Snapshot。
- 用户真实 Chrome profile、Cookie、扩展、书签迁移。
- 一整套用户/Agent 共用 Space 的产品壳。
- 以代码执行为由绕过现有 Control 权限卡。
- 把 ego-lite benchmark 作为 Marionette 性能背书。

## 5. 借鉴原则

1. 先借**可验证的行为契约**，再借代码。
2. 先做**单文档、单 tab 内**的效率提升，再做跨 frame/session。
3. 每新增一步能力，都必须保留 grant、handoff、tab ownership 和不可信页面数据边界。
4. 任何来自页面的 selector、ref、locator 和文本都只当数据，不当指令。
