# AgentShell 轻量版 Zed 参考清单

## 目标

把 Zed 当作交互和信息密度的标尺，做一个只服务于本地 Agent session 的轻量壳。目标不是复制 Zed 编辑器，而是复制它对线程、侧栏、面板和状态的克制处理。

## 当前本地参考

Zed 源码副本：`_research/repos/zed/`

重点阅读位置：

- `docs/src/ai/agents.md`
- `docs/src/ai/external-agents.md`
- `docs/src/ai/agent-panel.md`
- `docs/src/ai/parallel-agents.md`
- `crates/acp_thread/src/connection.rs`
- `crates/acp_thread/src/acp_thread.rs`
- `crates/agent_ui/src/conversation_view/thread_view.rs`

## 本轮源码复核结论

1. `project_panel` 把项目内容做成树节点，折叠状态由节点自身维护；行级动作只在 hover/focus 时出现，面板级动作放在 dock 的底部区域。
2. 主题由全局 `ThemeSettings` 驱动，界面只保留一个低频入口，不把主题控制塞进项目卡片或顶部工具栏。
3. `agent_connection_store` 按 Agent 保存连接状态，线程只持有当前 Agent/连接路径；这不是把所有 CLI 进程强行改名成同一个“模型”。

AgentShell 采用同样的边界：

- 左侧项目树支持项目级折叠；`+`、设置和折叠按钮属于行或 dock 控件，默认隐藏在 hover/focus 状态。
- 主题切换保留在左侧栏底部，并持久化当前明暗选择。
- 对话底部 Agent 选择器修改当前 Session 的 `agentId`，因此后续启动/发送会使用所选 Agent。
- 正在运行的真实 PTY/ACP 不能被 UI 假装“热切换”。M3 需要把 Session manifest、进程生命周期和 transcript 接上；之后切换应创建目标 Agent 的新连接，并通过 transcript/handoff 继续上下文，而不是篡改旧进程的归属。

## 对 AgentShell 的直接结论

Zed 的稳定抽象是 `project -> thread -> agent path`。AgentShell 应沿用这个层级：左侧只负责项目和 session，中央只负责当前 conversation，右侧只承载 Todo、usage 和未来的低频辅助信息。

ACP 只解决 Agent 与宿主之间的协议通信，不统一 Agent 的认证、模型、订阅、工具权限和思考档位。每个 adapter 都需要声明能力：

```text
auth: native | host | unavailable
model_select: supported | native_only | unavailable
thinking_effort: supported | native_only | unavailable
thread_usage: supported | unavailable
provider_quota: supported | unavailable
resume: supported | best_effort | unavailable
```

主界面只显示 `supported` 的控制项；`native_only` 显示状态和跳转入口；`unavailable` 不显示伪造的下拉菜单。

## M2 优先级

1. 建立 session manifest 和崩溃恢复状态机。
2. 把 Generic PTY 适配器与 ACP 适配器分开。
3. 接入一个 ACP agent，验证认证、session/update、tool call、plan、mode 和 config option。
4. 让 Clean View 从结构化事件生成消息、工具调用、思考和错误块，并支持统一折叠。
5. 增加 usage provider 接口：线程上下文用量先实现，provider quota 允许返回 unavailable。
6. 右侧 Todo 保持常驻，Changed Files 继续留给独立的 Git/Changes 页面。

## 不应在 M2 做的事

- 不克隆 Zed 的编辑器、GPUI、LSP 或 Git review 实现。
- 不把所有 Agent 的模型和思考强行做成统一 UI。
- 不把 provider quota 当成 token context usage。
- 不把 ACP 调试日志直接塞进普通对话时间线。
- 不为了展示能力而增加顶部工具栏、运行状态徽章或第三侧栏卡片。
