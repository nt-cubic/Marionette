# 01. 源码与许可证边界

## 1. 官方事实

ego-lite 不是“整个浏览器开源”。官方仓库对边界写得很直接：

- [AGENTS.md](https://github.com/citrolabs/ego-lite/blob/main/AGENTS.md#L5-L7)：仓库是 Node.js CDP harness 和 Agent Skill，**not the browser itself**；原生 `globalThis.ego` binding 由 closed-source app 提供。
- [CONTRIBUTING.md](https://github.com/citrolabs/ego-lite/blob/main/CONTRIBUTING.md#L3-L5)：仓库包含 JavaScript SDK 和 Skill；浏览器应用及其 native bindings 由安装后的 ego lite 应用单独提供。
- [发布说明](https://github.com/citrolabs/ego-lite/blob/main/CONTRIBUTING.md#L260-L262)：Release 发布的是 SDK/Skill payload，不是浏览器应用或安装器。
- [CitroLabs 公开仓库](https://github.com/orgs/citrolabs/repositories)：分析时只有 `ego-lite`、`.github` 和 benchmark framework，没有另一个公开的浏览器内核仓库。

当前仓库没有 Chromium 子模块，也没有浏览器原生工程。公开 Release `v2.2.1` 只有一个 `ego-browser-v2.2.1.zip` SDK/Skill 包。

## 2. 分层的真实状态

| 层 | 是否公开 | 对 Marionette 的意义 |
|---|---|---|
| Node.js harness / TypeScript SDK | 是，MIT | 可以按许可证移植、改写和测试 |
| Agent Skill、API schema、错误语义 | 是，MIT | 可以复用文档结构和接口设计 |
| 真实浏览器 E2E case | 是，MIT | 可以改造成 WebView2 fixture 和验收矩阵 |
| ego-lite 桌面应用 / UI | 否 | 不能作为源码依赖 |
| Chromium 定制与产品 patchset | 否 | 不能复刻其原生 Snapshot 与空间隔离实现 |
| `globalThis.ego` native binding | 否 | harness 的 transport 不能直接接到 WebView2 |
| TaskSpace / Page 的原生会话管理 | 否 | 只能按行为重新实现 Marionette 版本 |
| 用户 profile、登录态迁移、扩展 | 否 | 与 Marionette 的产品边界冲突 |

**底层 Chromium 上游本身开源，不等于 ego-lite 的 Chromium 定制开源。** 我们能拿到 Chromium 的公共代码，但拿不到 CitroLabs 对其做的 Snapshot、spaces、权限和控制权扩展。

## 3. MIT 覆盖范围

仓库根 [`LICENSE`](https://github.com/citrolabs/ego-lite/blob/main/LICENSE) 是 MIT。对仓库内文件，可以：

- 使用、复制、修改、合并、发布；
- 用于商业产品；
- 分发修改后的版本。

前提是保留版权声明和许可证文本。若正式复制代码，应像现有 Codeg vendor 一样增加 `VENDOR.md`，记录：

- 上游仓库 URL；
- 精确 commit/tag；
- 原始文件路径；
- 本地目标路径；
- 修改摘要；
- MIT 文本与 copyright notice。

MIT 许可只覆盖公开仓库内容，不授予 ego-lite 应用二进制、商标、闭源 native binding 或未公开 patchset 的源码权利。

## 4. 对技术选型的直接影响

| 想法 | 是否可行 | 原因 |
|---|---|---|
| 直接把 `ego-browser` npm 包接到 WebView2 | 否 | 它假设 `globalThis.ego.sendCDPMessage` 等原生 binding 已存在 |
| 将整个 harness 作为 sidecar 运行 | 技术上可做，但需要自己实现 native binding；不等于复用浏览器 |
| 复制 Page/ref/错误/等待的纯逻辑 | 可以 | MIT，且很多逻辑与 transport 可分层 |
| 复制真实浏览器测试场景 | 可以 | 改成 WebView2 fixture 后直接作为回归资产 |
| 复制原生 Snapshot / Chromium patch | 不可以 | 仓库没有源码 |

## 5. 结论

ego-lite 是“闭源浏览器 + 开源 Agent SDK”的分层产品。

Marionette 应该把 ego-lite 当成**协议与测试参考实现**，而不是依赖项，更不能把安装包当作可维护的内核。
