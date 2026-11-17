# ego-lite 借鉴与移植文档集（2026-11-17）

- 参考项目：<https://github.com/citrolabs/ego-lite>
- 分析快照：`main` = `f499162d1b911ba9b320ed31b741dba7ebc2e4d6`，tag `v2.2.1`（2026-11-16）
- Marionette 基线：`origin/master` = `6545082f8931f6db75b1615563d5164492358ed1`，并对照 `feat/browser-annotation` 上的浏览器 P1–P4 工作树
- 文档状态：调研与实施方案，尚未按本文施工
- 产品范围：只服务 Marionette 的内置 WebView2 开发工作区，不扩成个人浏览器或外部浏览器连接器

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
