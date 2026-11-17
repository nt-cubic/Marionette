# 04. 文件级代码复用地图

## 1. 复用等级

| 等级 | 含义 |
|---|---|
| A. 直接移植 | 纯逻辑、依赖少，保留 MIT 声明后可改写/复制 |
| B. 适配重写 | 设计可借，transport 与平台不同，必须重写 |
| C. 测试参考 | 不复用生产代码，只搬场景、断言和错误分类 |
| D. 只看不用 | 与 WebView2、产品边界或安全模型冲突 |

## 2. 核心文件映射

| ego-lite 文件 | 可借内容 | 等级 | Marionette 目标 | 说明 |
|---|---|---|---|---|
| `snapshot-result.ts` | snapshot 压缩、locator sanitize、frame provenance | A/B | `src-tauri/src/browser/snapshot.rs` 或现有 `surface.rs` 周边 | 纯字符串处理优先；async locator 校验需要 CDP 适配 |
| `page-ref-registry.ts` | ref identity、active/inactive、frame/document 绑定 | B | 新增 `browser/refs.rs` | 不持久化 JS element handle，只保存 host 可验证身份 |
| `page-ledger.ts` | Page label、ownership、reconcile、atomic versioned write | B | `browser/tab_ledger.rs` + 现有 tab registry | 不照搬 Node fs 版本写；借鉴 reconcile 和 finish 语义 |
| `element-resolver.ts` | CSS/role/href/xpath、跨 frame 解析、transient/permanent | B | `browser/resolve.rs` / 页内 JS helper | 当前 engine 已有 ref；此文件主要补 locator 和 frame |
| `driver/page-waits.ts` | waitForSelector、load state、polling | B | `browser/waits.rs` | 所有 CDP 调用必须走现有串行 lease |
| `driver/load.ts` / `driver/nav.ts` | pending navigation、waitUntil、页面可用性 | B/C | `surface.rs` navigation helper | WebView2 事件与 ego native binding 不同 |
| `driver/page-actions.ts` | actionability、click hit target、fill verify、drag | B/C | 现有 `agent.bundle.js` 外围或 probe | 先补语义测试，避免重出 vendored bundle |
| `driver/page-input.ts` | file chooser、set input files | B | 后续 `browser/files.rs` | 依赖 CDP DOM.setFileInputFiles |
| `driver/downloads.ts` | download artifact/result contract | B | 后续 `browser/downloads.rs` | 需要 WebView2 下载事件接管 |
| `driver/observe.ts` | viewport/full page/subtree snapshot 选项 | B | 扩展 `SnapshotRequest` | `subtree` 必须由 engine 先支持 |
| `ego-errors.ts` | stable error code、hard-stop guidance | A/B | `tools_mcp.rs` refusal/error helpers | 保留现有“拒绝是返回值”原则 |
| `public-api-schema.ts` | 单一 API 真源、运行时 help | B | `browser/help.rs` 或 schema generator | 不引入完整 Node harness |
| `skill-body/SKILL.md` | 工作流说明、观察成本、错误恢复 | B | 浏览器 Skill/`browser_help` 文本 | 改成当前 15 工具和 batch 语义 |
| `browser-runtime.ts` | CDP session、event、dialog、OOPIF 处理思路 | D | 无直接目标 | 依赖闭源 `globalThis.ego`，不可复制 |
| `page-model.ts` | TaskSpace/Page 生命周期 | B | 现有 `browser/mod.rs` + tab ledger | 只借语义，不复制完整 Space 层 |
| `learning/*` | 站点经验、manifest 校验 | C/B | 后续项目和 Skill 系统 | 先借验证纪律，不急着建站点工具市场 |
| `real-browser-e2e/cases/*` | 44 个真实浏览器场景 | C | `tools/browser-e2e` 或本地测试 fixture | 最高性价比的测试资产之一 |

## 3. 建议新增的 Marionette 文件

| 目标文件 | 职责 | 首个里程碑 |
|---|---|---|
| `src-tauri/src/browser/ref_lease.rs` | 保存 tab 当前 snapshot generation、失效原因和 action lease | P0 |
| `src-tauri/src/browser/ref_ledger.rs` | 后续跨 snapshot identity / locator mapping | P1 |
| `src-tauri/src/browser/batch.rs` | typed step 校验、执行、失败中止、审计 | P1 |
| `src-tauri/src/browser/waits.rs` | selector / load / page-ready polling | P1 |
| `src-tauri/src/browser/dialogs.rs` | JavaScript dialog accept/dismiss | P1 |
| `src-tauri/src/browser/help.rs` | 从 schema 生成浏览器 Agent 指南 | P0 |
| `tools/probe-browser-agent.py` | 真机 fixture / CDP 实验入口 | P0/P1 |
| `scripts/verify-browser-refs.mts` | ref 生命周期静态/契约测试 | P0 |
| `scripts/verify-browser-batch.mts` | batch schema、授权和错误测试 | P1 |

文件名是建议，最终可按现有模块大小合并；不要为了“对齐 ego”制造过多模块。

## 4. 可以直接借的纯逻辑

### 4.1 Snapshot 文本压缩

优点：

- 不依赖 Node 或 native binding；
- 输入输出是纯字符串/JSON；
- 有现成单测；
- 立即降低 transcript/tool result 体积。

建议：先以 TypeScript 参考为准在 Rust 实现，不把 Node runtime 带进发布包。

### 4.2 Locator honesty

保留原则：

- locator 不唯一就省略；
- locator 指向别的 node 就省略；
- 不向 Agent 输出 `loc=unstable` / `loc=ambiguous`；
- ref 仍可短期使用，但不要承诺长期稳定。

### 4.3 错误分类

把错误分成：

- `stale_ref`；
- `not_found`；
- `not_visible`；
- `disabled`；
- `pointer_blocked`；
- `navigation_pending`；
- `page_unresponsive`；
- `user_control`；
- `grant_required`；
- `tab_gone`。

每类错误都应回答：是否可重试、下一步观察什么、是否必须停止并找用户。

## 5. 需要重写的部分

### 5.1 CDP transport

ego-lite 的 transport 以 `globalThis.ego.sendCDPMessage` 和 Node callback 为中心。Marionette 使用 WebView2 COM/CDP、主线程消息泵和进程级 `CdpLease`，两者不能共用实现。

### 5.2 Page ledger 持久化

ego-lite 每个 Agent round 是新 Node 进程，所以需要把 Page label/ref 状态写盘。Marionette 的 tab registry 和 MCP server 在 app 生命周期内常驻，第一版 ref lease 只需要进程内状态。

不要为了“照搬”先引入版本化 JSON 文件和跨进程锁；现有 tab registry 已经承担 tab 生命周期。

### 5.3 TaskSpace

Marionette 的会话、项目、split workspace 和 browser owner session 已有自己的状态模型。直接引入 TaskSpace 会造成第三套 tab/workspace identity。只借：

- stable label；
- Agent/user origin；
- claim/release；
- finish 时 keep/close 规则。

## 6. 不建议借

| ego-lite 文件/能力 | 原因 |
|---|---|
| `browser-runtime.ts` 及其 native callback 生命周期 | 依赖闭源 binding，和 WebView2 事件模型不一致 |
| 原生 Snapshot requirement 中“browser side should”部分 | 没有源码，无法实现或 patch 闭源浏览器 |
| profile 选择、Chrome 数据迁移 | 超出 Marionette 开发浏览器边界 |
| 任意 `page.evaluate` / `page.cdp` | 高权限调试能力，需单独产品决策 |
| Node 站点 learning 运行时 | 当前发布物不保证 Node runtime；先做项目/技能层经验 |

## 7. Vendor 纪律

如果复制任何 MIT 代码：

1. 新增 `VENDOR.md`，记录 tag/commit 和上游路径；
2. 保留 CitroLabs copyright 与 MIT license；
3. 文件头注明“adapted from”；
4. 修改行为时同步本地测试，不把上游文件当不可改的黑盒；
5. 不复制或重分发 ego-lite 安装包中的闭源二进制。

## 8. 结论

最适合直接拿的是：**snapshot 压缩、locator 诚实性、错误分类和测试场景**。

最值得借设计但必须重写的是：**Page/ref ledger、waits、batch executor、Page 生命周期语义**。

完全不能借的是：**浏览器内核、native binding、闭源 Snapshot 和真实 Chrome profile 层**。
