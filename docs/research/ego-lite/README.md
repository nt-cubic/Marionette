# ego-lite 借鉴与移植文档集（2026-11-17）

- 参考项目：<https://github.com/citrolabs/ego-lite>
- 分析快照：`main` = `f499162d1b911ba9b320ed31b741dba7ebc2e4d6`，tag `v2.2.1`（2026-11-16）
- Marionette 基线：`origin/master` = `6545082f8931f6db75b1615563d5164492358ed1`，并对照 `feat/browser-annotation` 上的浏览器 P1–P4 工作树
- 文档状态：M0–M3 已于 2026-11-18 施工（见下方「施工偏差记录」）；M4/M5 按下述偏差记录处理——可做部分已做，engine 阻塞与真机实验项如实缓办，未假装完成
- 产品范围：只服务 Marionette 的内置 WebView2 开发工作区，不扩成个人浏览器或外部浏览器连接器

## 施工偏差记录（2026-11-18，M0–M3）

- **03 文档对 host bump 的诊断不完全准确**：`surface.rs` 每次 action 后推进
  generation 属实，但 host 从不校验模型传入的 generation 串，bump 也不改写页内
  `refsToken`——它**不产生任何即时失效**，只是搅动下一次 snapshot 的 epoch。真正
  逼模型重拍的是引导缺位：工具描述从未说 generation 跨 action 有效。因此 M0 的
  实际改动是「去掉无效果的 bump + host 侧 lease 记录 + 响应注记 + 描述更新」，
  页内六重校验一字未动，比原方案更保守。
- lease 落在 `src-tauri/src/browser/ref_lease.rs`（原 04 文档建议并入 surface
  周边，实际独立成纯逻辑模块便于单测）。**只判死不判活**：host 亲眼所见的最长
  一代前置 token 之外一概交页内裁决；已知死因直接短路应答（省一次 CDP、给真
  reason），不存在假阳性。
- `pick_resolve`（用户批注流）的隐藏重拍今天就会顶掉 agent 的 refs——lease 把
  这件事显式化为 `reason: "user_annotation"`，不再让页内的 truncation 文案误导。
- M1 的 `browser_help` 从 `tool_schemas()` 生成（单一真源）；M2/M3 落地后 topic
  扩到八个（waits/batch 随工具上线一起进 help）。
- **M2 落地形状**（`browser/batch.rs` + tools_mcp 执行循环）：封闭 op 集与
  05 文档一致；额外定死两条 05 只暗示的规则——**snapshot 只能做最后一步**（它
  换掉整张 ref 表，后继 ref 步必死）；**权限拒绝永远停批**（不受 stopOnError
  影响，拒绝不是普通失败）。权限等待冻结批预算；轮询睡在 MCP worker 线程
  （主线程零睡眠红线）。无「一次计划批准」模式（05 的第二版方案，未做）。
- **M3 落地形状**：waits 走页内 probe 轮询（`refState` 分类 found/connected/
  visible），不是 CDP 事件流；networkidle 明确不做（05 的后续实验）。dialog
  走 `Page.javascriptDialogOpening` 事件镜像 + `Page.handleJavaScriptDialog`
  应答，**不压掉 WebView2 默认弹窗**——用户始终有人工出口，这是与 ego-lite
  「接管 dialog」的刻意偏差；renderer-bound eval 在 dialog 打开时快速失败
  `dialog_blocked`（被阻塞 renderer 上的 eval 会挂死在进程级 CDP lease 里）。
- **M4 ref ledger：被 vendored engine 阻塞，如实缓办**。跨 snapshot identity
  需要 engine 暴露 backend node 身份（05 §7），而 bundle 是 vendored 产物
  （js/VENDOR.md：改 = 重放上游构建）；M0 的 lease 就是 05 §3 预期的「更小的
  host 版本」。等上游 engine 支持再评估。
- **M4 Page label/group：现有能力已覆盖，不再新增**。Marionette 已有 stable
  chosen id（`browser_open_tab` 的 `id`，即 ego 的 Page label）、pin、claim
  （「转为我的标签」= adopt 反向）、idle cleanup 的 keep 语义、会话删除收走
  owned tabs（= finish close）；05 §7 自己要求「先测量 tab 混乱是否真实存在
  再决定」，没有测量信号前不加第三套 label。
- **M5：只做了可静态确认的部分**。①快照 iframe 行为已实锤——vendored engine
  不跨 frame（bundle `contentDocument` 零命中），快照只覆盖主 frame；picker 的
  同源 iframe 递归命中不受影响。fixture 集已建（`tools/browser-fixtures/` 10 页
  + `tools/serve-browser-fixtures.mjs`）。②same-origin frame 快照补齐需要
  engine 改动 → 随 M4 缓办；③OOPIF / download / popup / file chooser 按其
  「实验 + 真机 heartbeat 回归」门槛不做，不进入承诺。
- 回归：`scripts/verify-browser-refs.mts`（M0 钉死 + M2/M3 新钉点：batch 32 步
  上限与 terminal snapshot、轮询线程归属、dialog 门控与自愈、fixtures 存在）+
  cargo browser/tools_mcp 套件 + 真机手工清单（`docs/CURRENT.md` 浏览器节，
  fixture server 一条命令拉起）。

## 结论先行

ego-lite 值得借鉴，但不能整套搬。

1. **ego-lite 的浏览器内核和原生 binding 没有开源。** 公开仓库是 MIT 授权的 JavaScript harness、Agent Skill、测试和工具；浏览器应用、Chromium 定制、原生 Snapshot、TaskSpace 原生层不在仓库里。
2. **Marionette 的浏览器产品壳已经补齐了大半。** `SurfaceWorkspace`、后台/workspace tab、owner session、pin/claim、idle cleanup、handoff、元素批注和 none/read/control 授权都已经落地，不需要重做。
3. **真正值得借的是 Agent 操作层的语义。** 稳定 ref、连续动作、受限批量步骤、等待与动作可执行性、JS dialog、错误码、真实浏览器回归案例，这些能直接降低工具轮次、token 和重试率。
4. **第一刀不是 OOPIF，也不是任意 JavaScript。** 先用最小改动验证“同一张 snapshot 支持连续动作”，再做 host 侧 ref lease / ledger，最后才评估跨 frame/OOPIF 和代码式工作流。
5. **ego-lite 的性能宣传不能直接外推。** 它的速度优势来自定制 Chromium、原生 Snapshot 和代码式 harness 的组合；只搬 harness 不会自动得到同一组数字。

## 推荐优先级

| 优先级 | 工作 | 预估风险 | 直接收益 |
|---|---|---|---|
| P0 | 同一 snapshot 连续动作 | 低 | 立刻减少 snapshot 次数和工具往返 |
| P0 | 运行时可读的 `browser_help` / Agent 操作指南 | 低 | 降低模型猜参数、猜 ref 生命周期 |
| P1 | host 侧 ref lease / ledger | 中 | ref 跨轮次、跨局部 snapshot 更稳定 |
| P1 | 受限 `browser_batch` typed steps | 中 | 一次 MCP 调用完成多步，显著降 token/延迟 |
| P1 | wait / actionability / JS dialog 语义 | 中 | 减少固定 sleep、盲重试和假失败 |
| P2 | same-origin iframe，再评估 OOPIF | 中高 | 覆盖嵌入式表单、编辑器和登录组件 |
| P2 | download / popup / file chooser | 中 | 补全真实 Web 调试链路 |
| 暂缓 | 任意 JS、任意 CDP、真实 Chrome profile 导入 | 高 | 与当前产品边界和授权模型冲突 |

## 文档导航

| 文档 | 内容 |
|---|---|
| [`01-source-boundary.md`](./01-source-boundary.md) | ego-lite 哪些部分开源、哪些闭源、许可证和技术边界 |
| [`02-architecture-and-borrows.md`](./02-architecture-and-borrows.md) | ego-lite 的架构、七个可借鉴机制及不借项 |
| [`03-marionette-baseline-and-gaps.md`](./03-marionette-baseline-and-gaps.md) | Marionette master 已有什么、真实缺口和优先级 |
| [`04-code-reuse-map.md`](./04-code-reuse-map.md) | 文件级复用地图：直接移植、适配重写、只看不抄 |
| [`05-implementation-roadmap.md`](./05-implementation-roadmap.md) | M0–M5 路线、接口草案、状态模型、落地文件和退出门槛 |
| [`06-test-and-acceptance.md`](./06-test-and-acceptance.md) | 单元/脚本/真实 WebView2 测试矩阵、指标、红线和发布验收 |
| [`07-extended-borrow-catalog.md`](./07-extended-borrow-catalog.md) | 完整扩展目录：Agent 行为、可见化、hard stop、生命周期、事件媒体、运行时卫生、学习系统和 benchmark |

## 明确不做

- 不引入 ego-lite 的闭源浏览器或安装包作为 Marionette 内核。
- 不把用户日常 Chrome/Edge 的 Cookie、扩展、标签和登录态接进内置 Browser surface。
- 不在第一版提供任意 `page.evaluate`、任意 CDP 或任意 Node 脚本执行。
- 不复制一套 TaskSpace/Space 用户界面；只借 Page label、ownership、claim/release/finish 的状态语义。
- 不以 ego-lite benchmark 结果作为 Marionette 的验收指标；使用自己的工具轮次、snapshot 数、token 和成功率。

更细的完整候选池与阶段映射见 [`07-extended-borrow-catalog.md`](./07-extended-borrow-catalog.md)。

## 一句话决策

**拿 ego-lite 的页面状态机、SDK 设计纪律和测试资产；重写 transport、Snapshot 和 Marionette 集成；不要拿它的浏览器内核和产品边界。**
