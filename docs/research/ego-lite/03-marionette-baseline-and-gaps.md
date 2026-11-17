# 03. Marionette 现状、差距与优先级

## 1. 当前基线不是“简陋的 WebView”

Marionette master 上的浏览器工作已经覆盖完整产品闭环：

- Browser / Document / Memo 共用 [`SurfaceWorkspace`](../../src/components/SurfaceWorkspace.tsx)；
- Agent 默认后台开 tab，只有 `display:"workspace"` 进入同屏；
- tab 有 owner session、busy count、status、lastUsedAt、pin、claim；
- 删除会话/项目会清理其 owned tabs；idle cleanup 跳过 pinned/named/busy/waiting_user/当前可见 tab；
- `browser_handoff` 覆盖登录、2FA、CAPTCHA、支付等人工步骤；
- `none/read/control` 与用户批注、截图、不可信数据声明已接通；
- `browser_page_state` / `browser_page_ready` / console / screenshot 已是低成本观察面；
- tab 懒建，减少常驻 WebView2 进程组。

现行契约见 [`docs/CURRENT.md`](../../CURRENT.md) 的「内置浏览器 Workspace Surface」，工具面见 [`tools_mcp.rs`](../../src-tauri/src/tools_mcp.rs:260)。

## 2. 当前 15 个 `browser_*` 工具

| 类别 | 工具 |
|---|---|
| Tab | `browser_list_tabs`、`browser_open_tab`、`browser_close_tab` |
| 人工交接 | `browser_handoff` |
| 观察 | `browser_snapshot`、`browser_page_state`、`browser_page_ready`、`browser_console_messages`、`browser_screenshot` |
| 控制 | `browser_click`、`browser_hover`、`browser_type`、`browser_press_key`、`browser_select_option`、`browser_set_value` |

问题不在“工具数量不够”，而在一次任务通常仍要：

```text
snapshot -> click -> snapshot -> type -> snapshot -> select -> snapshot
```

ego-lite 真正领先的是把这个链条压缩成更少的观察和一次脚本调用。

## 3. 当前 Snapshot / ref 生命周期

Marionette 使用 vendored Codeg aria engine：

- snapshot 在 isolated world 生成 aria tree 和 DOM ref；
- `generation.nav_epoch` 被同时送给页内 engine 和后续 action；
- 页面导航会推进 `nav_epoch`；
- host 每次 snapshot 后推进 `generation`；
- host 每次 action 后也推进 `generation`，见 [`surface.rs`](../../src-tauri/src/browser/surface.rs:879)。

页内 engine 本身并不是“DOM 一变动所有 ref 全失效”。它会检查：

- snapshot token 一致；
- 当前 URL 一致；
- history 长度和 navigation entry 一致；
- `popstate` / `hashchange` 计数一致；
- 目标 element 仍 connected。

因此当前 host 的“每个 action 都换 generation”比页内实际安全条件更严格，直接造成连续动作必须重新 snapshot。

### P0 快速实验

先验证：保留同一 snapshot generation，在同一 URL、无 history 变化的页面上执行多个动作；只有以下情况要求新 snapshot：

- 导航或 SPA route 变化；
- 目标 element 已断开；
- action 返回 stale/pointer blocked；
- 页面主动要求重新观察。

这一步应作为最小实验，不先重写 aria engine。

### P1 ref lease / ledger

如果快速实验稳定，再增加 host 侧 ref lease：

- current snapshot generation 独立于 action counter；
- tab 级保存当前 active generation；
- navigation event 清空 lease；
- 局部 snapshot 可选择 merge 或 replace；
- 冷启动、tab 重建和 document 替换一律不恢复 ref。

ego-lite 的 `PageRefRegistry` 用 `frameId + backendNodeId + documentId` 保留跨 snapshot identity。Marionette 可在不改闭源依赖的情况下先做更小的 host 版本，再决定是否引入 backend node identity。

## 4. 当前 frame / CDP 约束

Marionette 的 isolated world 明确建立在主 frame，见 [`surface.rs`](../../src-tauri/src/browser/surface.rs:699)。

同时，所有 CDP 调用共用一个进程级 lease，原因不是设计洁癖，而是 WebView2 `call_and_wait` 会泵消息循环；两个 CDP 调用重叠曾实测冻死主线程。相关约束在 [`surface.rs`](../../src-tauri/src/browser/surface.rs:820)。

这带来两个结论：

1. 不能照搬 ego-lite 的 per-frame session 并发模型；
2. OOPIF 必须作为单独实验，所有 frame 调用仍要在一个串行调度器下完成，并做长时间 UI heartbeat 验证。

## 5. 差距矩阵

| 能力 | 当前 Marionette | ego-lite 参考 | 优先级 | 建议 |
|---|---|---|---|---|
| 同一 snapshot 连续动作 | host 每次 action 后换 generation | ref 可连续使用 | P0 | 先做最小 lease 实验 |
| 跨 snapshot ref identity | 新 snapshot 替换旧 ref | registry 按 node identity 合并 | P1 | host ledger，先内存不持久化 |
| 批量多步动作 | 一步一个 MCP tool | 一段脚本执行多步 | P1 | typed `browser_batch` |
| 运行时 API 指南 | 静态 MCP description | schema 生成 `help()` / Skill | P0 | `browser_help` |
| wait / load state | 只有 page ready 与手工轮询 | selector/load/network/idle/pending nav | P1 | 先 selector + load state |
| actionability | 现有 aria engine 有部分检查 | 完整可见/禁用/hit target/fill verify | P1 | 补错误分类和本地 fixture |
| JS dialog | 未接管 | accept/dismiss 明确 API | P1 | 与 waits 同批 |
| download / popup / file chooser | 未完整接管 | 完整生命周期 | P2 | 按真实开发场景加 |
| same-origin iframe | 没有独立 frame ledger | 可操作 frame 节点 | P2 | 先 JS 递归，再 OOPIF |
| OOPIF | 无 | per-frame/OOPIF refs | P2 实验 | 不进入第一版承诺 |
| 任意 evaluate / CDP | 默认没有 | 有 | 暂缓 | 独立开关也不建议先做 |
| 真实 Chrome 登录态 | 明确不做 | 产品核心 | 不借 | 保持内置开发工作区边界 |

## 6. 当前产品已经领先 ego-lite 的部分

- 与多 Agent 会话、Composer、权限卡和 handoff 同屏；
- 元素/区域批量批注、截图裁剪、预填 Composer；
- `none/read/control` 与用户自开 tab/Agent 自开 tab 的区分；
- Browser / Document / Memo 共用 surface 与冷启动恢复；
- Dock 态整页浏览器；
- dev server 发现采用 TCP probe，不凭 localhost 字符串抢界面。

这些都不要为了“更像 ego-lite”而回退。

## 7. 应该测量的指标

对一个固定的本地 fixture，记录改造前后：

- MCP tool 调用次数；
- snapshot 次数和总字符数；
- 总 token；
- 从首条 prompt 到完成动作的墙钟时间；
- stale/retry 次数；
- 权限卡次数；
- 页面状态错误率；
- app UI heartbeat 是否始终正常。

只有这些指标改善，才算“借鉴成功”。不能拿 ego-lite 的 benchmark 数字替代 Marionette 自己的验收。
