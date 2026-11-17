# 05. 分阶段实施路线

## 1. 工程原则

- 先验证生命周期，再增加 API。
- 先减少重复 snapshot，再讨论代码式脚本。
- 所有 Control action 继续经过现有授权检查。
- 所有 CDP 调用继续服从主线程和全局 `CdpLease` 约束。
- 任何页面数据都带不可信标记。
- 不因借鉴 ego-lite 改变 Browser 的产品范围。

## 2. 里程碑总览

| 里程碑 | 内容 | 依赖 | 预期收益 |
|---|---|---|---|
| M0 | 同一 snapshot 连续动作 | 无 | 立刻减少 snapshot 和工具往返 |
| M1 | `browser_help` 与单一 schema 真源 | 无 | 降低模型猜错 |
| M2 | 受限 typed `browser_batch` | M0 | 一次调用完成多步 |
| M3 | waits / dialog / actionability | M0/M2 | 减少 sleep 和盲重试 |
| M4 | ref identity ledger + Page label/group | M0 | 跨快照 ref 与 tab 生命周期更稳 |
| M5 | same-origin iframe / OOPIF 实验、download/popup/file chooser | M3/M4 | 覆盖复杂真实网页 |

## 3. M0：同一 snapshot 连续动作

### 目标

一个 snapshot 后，在同一 document、同一 URL、无 history 变化时，可以对不同 connected element 连续执行多个动作，不需要每次重新 snapshot。

### 当前行为

- `snapshot()` 后 `generation += 1`；
- 每次 `act()` 后 `generation += 1`；
- 页内 engine 实际已经检查 URL/history/element connected，因此 host 的 action bump 是更严格但不必要的失效。

### 实现建议

1. 新增 `SurfaceRefLease`，至少记录：
   - `snapshot_generation`；
   - `nav_epoch`；
   - `created_at`；
   - `last_used_at`；
   - `invalidation_reason`。
2. action 不再修改 snapshot generation。
3. 保留以下失效点：
   - snapshot 被新 snapshot 替换；
   - main-frame document navigation；
   - same-document route/history/navigation entry 变化；
   - tab close/recreate；
   - action 自己返回 stale；
   - target disconnected；
   - page/context destroyed。
4. tool 返回可选 `refLease` 状态，不必新增工具参数；现有 `generation` 继续作为不透明 token。
5. 如果某类 action 确实需要主动失效，建立显式 `invalidates_lease` 表，不用默认全失效。

### 退出门槛

- 一张 snapshot 连续完成 click + type + select；
- DOM 局部更新但目标节点仍 connected 时允许继续；
- 目标节点被替换后返回 `stale_ref`；
- SPA pushState / hash / history back 后返回 stale；
- full navigation 后返回 stale；
- 不出现 CDP 重叠或 UI 冻死。

### 回退

保留 `MARIONETTE_BROWSER_REF_LEASE=0` 开关，一旦出现错误复用或 stale 漏判，立即恢复每次 action bump。

## 4. M1：`browser_help` 与单一 schema 真源

### 目标

Agent 能查询当前版本真实可用的 browser API、参数、ref 生命周期和推荐工作流，避免静态提示与新 schema 漂移。

### 实现建议

- 新增只读工具 `browser_help`，参数可选 `topic`；
- 内容由 Rust 中的工具 schema/常量生成，或至少由同一数据表生成；
- 初始主题：
  - `overview`
  - `refs`
  - `snapshot`
  - `actions`
  - `waits`
  - `handoff`
  - `batch`
- 工具说明只给短提示，详细内容按需返回，避免每次 `tools/list` 膨胀。
- browser Skill 只保留入口和高层规则，运行时仍以 `browser_help` 为准。

### 退出门槛

- `tools/list` 中每个工具都能在 help 找到；
- schema 测试能发现缺失/多余 topic；
- help 不暴露 token、路径、内部 CDP 参数或用户页面数据。

## 5. M2：受限 typed `browser_batch`

### 为什么不是任意 JS

任意 JS 需要额外 runtime、沙箱、取消、输出限制和授权模型；Marionette 的便携版不能假设 Node 存在。第一阶段先拿“多步组合”的主要收益。

### 建议 schema

```json
{
  "tabId": "b7",
  "generation": "3.7",
  "steps": [
    { "op": "waitForSelector", "ref": "e12", "state": "visible", "timeoutMs": 3000 },
    { "op": "click", "ref": "e12" },
    { "op": "setValue", "ref": "e18", "value": "demo@example.com" },
    { "op": "pressKey", "key": "Enter" },
    { "op": "waitForLoadState", "state": "domcontentloaded", "timeoutMs": 5000 },
    { "op": "snapshot", "maxChars": 20000 }
  ],
  "stopOnError": true,
  "includeFinalState": true
}
```

### 初始封闭 op 集

- `click`
- `hover`
- `type`
- `setValue`
- `pressKey`
- `selectOption`
- `waitForSelector`
- `waitForLoadState`
- `pageState`
- `pageReady`
- `snapshot`

不在第一批：`evaluate`、`cdp`、任意 URL 跳转、下载、文件选择。

### 执行约束

- 最多 32 步，总 timeout 有上限；
- Rust 内严格串行执行；
- 同一个 tab，默认禁止跨 origin 导航；
- 任意 Control 步骤仍走 `control_gate`；
- 默认首错即停；
- 每一步产生审计记录，结果只返回摘要和最终观察；
- 若中间发生 document navigation，后续依赖旧 ref 的步骤直接 `stale_ref`。

### 权限策略

第一版最安全：单次 MCP 调用内仍按现有规则逐个 Control step 检查。

第二版可讨论“一次计划批准”：

- 冻结 step list；
- 只允许同 tab、同 origin；
- 用户在一次卡里看到全部动作摘要；
- 一旦出现非预期导航或 origin 变化立即终止；
- 该模式必须先有审计和回归，不先默认开启。

### 退出门槛

- 4–8 步表单在一次 tool call 内完成；
- 中间失败能给出准确 step index/op/ref/error；
- 没有授权绕过；
- batch 结果不超过合理大小，默认只返回 final state + 可选 snapshot；
- 单步工具仍保留，可独立使用。

## 6. M3：wait、dialog 与 actionability

### 第一批 API

- `browser_wait_for`：
  - selector/ref；
  - `attached|visible|hidden|detached`；
  - timeout；
- `browser_wait_for_load_state`：
  - `domcontentloaded|load`；
  - networkidle 放到后续实验；
- `browser_dialog`：
  - `accept`，可选 promptText；
  - `dismiss`；
- action error 细分：
  - `not_found`
  - `not_visible`
  - `disabled`
  - `pointer_blocked`
  - `navigation_pending`
  - `stale_ref`

### 实现纪律

- 轮询优先用页内 probe 和 CDP 事件，不在 Rust 睡眠里持锁；
- 每次等待都有硬 deadline；
- 页面 modal dialog 打开时，不能继续发需要 renderer 响应的 CDP；
- dialog accept/dismiss 必须显式返回是否真的存在 dialog；
- 不自动接受 confirm/prompt。

### 退出门槛

- 延迟 500ms 出现的按钮可被 wait；
- 不可见和 pointer-blocked 返回不同错误；
- confirm 不会被自动批准；
- 导航超时后页面仍可观察和恢复；
- 等待期间 UI heartbeat 正常。

## 7. M4：ref identity ledger 与 Page label/group

### ref ledger

在 M0 lease 稳定后，增加：

- `ref -> {frame, document, backendNodeId, role, name, active}`；
- full snapshot replace；
- partial snapshot merge（等 engine 支持 subtree/局部结果后再开）；
- document 变化时批量 inactive；
- ref id 不复用；
- 只保存在进程内，不写 localStorage / JSON，不跨启动恢复。

ego-lite 的版本化文件持久化是因为每轮 Node 进程会退出；Marionette 不需要照抄。

### Page label/group

可选轻量实现：

- Agent 打开的 tab 支持 short label；
- 一个 session 可列出自己的 managed tabs；
- `release` 表示交给用户，不关闭；
- session finish 时可传 `keep: all | labels`；
- user-owned/claimed/pinned tab 永不自动关闭。

现有 stable chosen id 已覆盖部分能力，先测量 tab 混乱是否真实存在，再决定是否加 label。

## 8. M5：frame、download、popup 和文件

### Frame 顺序

1. 测清当前 aria engine 对 same-origin iframe 的实际行为；
2. 用 JS 递归补 same-origin frame，不引入新 CDP session；
3. 建立 OOPIF 实验：single-owner CDP scheduler，禁止并发；
4. 在真实 fixture 里测 `Target.setAutoAttach`、isolated world、ref provenance；
5. 长时间 UI heartbeat、双 tab、快照/动作并发回归；
6. 不达标就停在 same-origin，不承诺 OOPIF。

### Download / popup / file chooser

- popup 要保留 `opener` 归属与 same task/session；
- download 要明确临时路径、最终路径、取消、失败和文件大小；
- file chooser 只允许项目目录或用户显式选择范围；
- 所有路径都在 UI 显示，不静默写任意位置。

## 9. 任意代码模式是否做

暂不作为路线图承诺。

未来若做，候选顺序：

1. typed batch（M2）；
2. 受限 workflow DSL（变量 + 条件）；
3. 可选 sidecar/JS sandbox，独立开关；
4. 任意 `page.evaluate` / raw CDP，仍保持默认关闭。

不能把“Agent 能执行 shell”当成不设边界的理由；浏览器工具仍有用户授权、页面数据和组织边界。

## 10. 推荐施工顺序

```text
M0 连续动作快速实验
  -> M1 browser_help
  -> M2 typed batch
  -> M3 waits/dialog/actionability
  -> M4 ref ledger + Page label（按测量结果裁剪）
  -> M5 frame/download/popup 实验
```

每个里程碑独立可发布、可回退。不要等“大而全 Agent Browser SDK”一次上线。
