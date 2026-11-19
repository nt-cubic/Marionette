# Howto：给浏览器加一个 agent 工具（tools_mcp）

> 场景：给 agent 新增一个浏览器工具（如新的读取/操作面）。参照物：`browser_page_state`
> /`browser_set_value`/`browser_dialog`/`browser_batch` 的接法。
> 深度索引见 [../browser.md](../browser.md)——**先读它的「红线」节**（CDP 租约/grant
> 一处/封闭 op 集），本 howto 只给步骤。

## 步骤

1. **schema**：`tools_mcp.rs::tool_schemas()` 加条目（name/description/参数表）——
   这是单一真源，`browser_help` 的内容从这里生成；**必须带 topic**（或明确缓办），
   `help_covers_every_browser_tool_and_names_no_ghosts` 双向钉死（help 不列不存在
   的工具、不提不存在的工具名）。
2. **dispatch**：`tools_mcp.rs` 的 dispatch 分支——读级工具在函数开头调
   `browser::tool_read`，Control 级走 `control_gate`。**别写第二套授权判断**；
   拒绝是返回值（`{"error":"browser_grant_required","note":…}`），不是报错。
3. **CDP 纪律**：发 CDP 就必须 `CdpLease::take()`（进程级；忙时返回「retry in a
   moment」可转述值，别排队）。等待中泵消息循环——**绝不持 registry borrow 等 CDP**、
   绝不在事件分支里发 CDP、绝不睡主线程（waits 轮询睡 MCP worker 线程）。
4. **表达式构建**：页内脚本执行的表达式全 serde_json 构建（`pick.rs`/`probe.rs`
   同款），**禁字符串拼接**；自有脚本放 `browser/js/`（probe.js 形态：自装 IIFE、
   每个 eval 内联整份脚本+三元守卫）；**别往 agent.bundle.js 加任何东西**（vendored
   产物，改=重放上游构建）。
5. **dialog 门**：会发 renderer eval 的新工具要处理 dialog 打开态（快速失败
   `dialog_blocked`——被阻塞 renderer 上的 eval 会挂死在进程级 lease 里）。
6. **文档**：`browser_help` 的 topic 覆盖；工具描述里的行为约定写清（超时语义/
   失败返回形状）。

## 验证

```bash
cargo test --features engine-rust --bin marionette browser tools_mcp
npx tsx scripts/verify-browser-refs.mts   # 源码钉死断言（新钉断言也加在这）
node tools/serve-browser-fixtures.mjs     # 真机手工过一遍：正常路径 + grant 拒绝 + CDP 忙
```
