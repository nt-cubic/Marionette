# 06. 测试、指标与验收

## 1. 测试金字塔

| 层 | 目标 | 建议入口 |
|---|---|---|
| Rust 单元 | ref lease、batch schema、权限、错误分类 | `cargo test --bin marionette browser` |
| TypeScript 契约 | schema/help、前端 tab 状态、batch 展示 | `npx tsx scripts/verify-browser-*.mts` |
| 真实 WebView2 fixture | DOM、导航、iframe、dialog、下载等行为 | `tools/probe-browser-agent.py` |
| 端到端 Agent | 某 agent 通过 MCP 完成固定任务 | `tools/browser-tab-repro.py` 扩展或新 probe |

现有可复用回归包括：

- `scripts/verify-browser-pick.mts`
- `scripts/verify-browser-prefs.mts`
- `scripts/verify-workspace-surface.mts`
- `scripts/verify-ui-restore.mts`
- `tools/browser-tab-repro.py`

## 2. M0 ref lease 测试

### 单元

- action 不推进 snapshot generation；
- snapshot 推进 generation；
- navigation 清空 lease；
- tab close 清空 lease；
- feature flag 关闭时恢复旧行为；
- stale response 会触发 lease 标记失效。

### 真实页面 fixture

| 场景 | 预期 |
|---|---|
| 一个 snapshot，连续点两个不同按钮 | 两次动作使用同一 generation |
| 点按钮后更新局部列表，目标输入框仍 connected | 继续 type 成功 |
| 点击把目标节点替换成新 DOM | 返回 `stale_ref` |
| `pushState` 到新 route | 返回 `stale_ref` |
| history.back | 返回 `stale_ref` |
| 真正导航到新 document | 返回 `stale_ref`，旧 world 被丢弃 |
| 两个 tab 同时 action | 全局 CDP lease 保持串行，不冻 UI |

### 指标

固定表单任务从 `snapshot + 5 action + snapshot` 降到 `snapshot + 5 action + state`。

最低成功标准：

- snapshot 次数下降 ≥ 50%；
- stale 误杀不明显增加；
- tool 总字符数下降；
- 无 UI heartbeat 中断。

## 3. M1 help 测试

- `tools/list` 中所有工具都有 help topic；
- help 中的必填参数与 schema 一致；
- `batch` / `waits` 未启用时不宣传；
- help 不包含 token、绝对 profile 路径和内部 CDP method；
- 中英文术语不混成两套不同参数名。

## 4. M2 batch 测试

### 校验

- 拒绝空 steps；
- 拒绝 > 32 步；
- 拒绝未知 op；
- 拒绝 `evaluate` / `cdp` / 任意 shell 字段；
- timeout 有最大值；
- 只能操作指定 tab；
- 默认禁止 batch 内跳转到新 origin。

### 执行与授权

- 首个 Control step 走现有 `control_gate`；
- 拒绝后续步骤不执行；
- 权限卡超时后 batch 停止；
- always-approve 行为与单步工具一致；
- 每个 Control step 有审计摘要；
- 失败结果包含 `stepIndex`、`op`、`ref`、`error`。

### 真实场景

1. 登录表单：type email -> type password -> click submit -> wait load；
2. 创建项目：wait button -> click -> set value -> select -> snapshot；
3. 失败中止：第二步 ref stale，第三步绝不能继续；
4. tab 被用户关闭：返回 `tab_gone`，不创建新 tab 偷跑。

### 指标

- 5–8 步任务 MCP call 数从 N 降到 1（读取最终结果另计）；
- 总 token 下降；
- 权限次数不增加；
- 成功率不下降。

## 5. M3 wait / dialog 测试

| Fixture | 断言 |
|---|---|
| 元素延迟 500ms 出现 | wait visible 在 timeout 内成功 |
| 元素永久隐藏 | 返回 `not_visible` / timeout 语义明确 |
| 覆盖层挡住按钮 | 返回 `pointer_blocked` |
| disabled 控件 | 返回 `disabled` |
| `alert` | dismiss/accept 明确处理 |
| `confirm` | 不自动接受 |
| `prompt` | accept 可带文本，dismiss 不需要 |
| 点击触发 pending navigation | 返回稳定状态，不永久挂死 |
| networkidle 实验 | 只在本 Page/OOPIF 请求状态内计算 |

## 6. M4 ref ledger 测试

- 相同 backend node 跨两次 snapshot 保持同一 public ref；
- 节点被替换产生新 ref，旧 ref inactive；
- partial snapshot 未出现的 ref 不无故删除；
- document 变化全部旧 ref inactive；
- ref id 不复用；
- ledger 损坏/未知版本时 fail closed：要求新 snapshot，不猜；
- 冷启动后不复用上次 session 的 ref；
- 两个 tab 的同名角色不会串 ref。

## 7. M5 frame / advanced 测试

### Frame

- same-origin iframe 内按钮可 snapshot/action；
- cross-origin OOPIF 内 ref 带 frame provenance；
- top document 和 frame 中同名按钮不串；
- frame reload 只失效该 frame refs；
- 多 iframe、多 tab、snapshot+action 并发不冻主线程；
- frame 探测不 focus、不 reload、不激活 tab。

### Download / popup / file

- popup 归属同一 session；
- user-owned tab 弹出窗口不自动归 Agent；
- download 有临时路径、最终路径、cancel/failure；
- 文件名防路径穿越；
- file chooser 不越过用户授权目录；
- 页面关闭时未完成 download 明确失败。

## 8. 真实浏览器 fixture 最小集合

建议在本地启动一个静态/动态 fixture server，至少包含：

1. `basic-form.html`：text、checkbox、select、button；
2. `late-render.html`：延迟出现/隐藏/禁用；
3. `overlay.html`：pointer interception；
4. `spa-routes.html`：pushState/hash/history；
5. `dom-replace.html`：节点替换；
6. `dialogs.html`：alert/confirm/prompt；
7. `iframe-same.html`；
8. `iframe-cross.html`；
9. `popup-download.html`；
10. `shadow-dom.html`。

fixture 不应依赖真实第三方网站、登录态或网络波动。

## 9. 每次发布的通用红线

- 不绕过 `none/read/control`；
- 不自动接受 confirm/prompt；
- 不把页面文字当系统指令；
- 不在持有 surfaces borrow 时等待 CDP；
- 不并发发起 WebView2 CDP；
- 不恢复冷启动前 ref；
- 不让 Agent 关闭 user-owned/claimed/pinned tab；
- 不从 localhost 字符串直接切换 workspace；
- 不把失败伪装成成功或静默重试到另一个 origin。

## 10. 发布验收命令

每个里程碑至少运行：

```powershell
npm run build
cargo check --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --bin marionette browser
npx tsx scripts/verify-browser-prefs.mts
npx tsx scripts/verify-browser-pick.mts
npx tsx scripts/verify-workspace-surface.mts
```

真实 WebView2 fixture 在支持桌面 UI 的环境单独跑，并记录：

- app 版本/commit；
- WebView2 Runtime 版本；
- Windows 版本和 DPI；
- fixture case 列表；
- 失败截图/JSON；
- `ui-heartbeat` 是否中断。

## 11. 最终验收口径

这套借鉴算成功，不是因为“用了 ego-lite 的代码”，而是因为：

- 固定开发任务需要更少 snapshot 和 tool call；
- ref stale/重试更可解释；
- waits 和 batch 没有扩大权限面；
- iframe 等复杂页面可测、可回退；
- 页面故障不会拖死 Marionette 主 UI。
