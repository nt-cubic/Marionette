# GitHub Issue / PR 自动化维护路线

状态: 草案。现阶段只落地了用户诊断报告与 Issue 输入契约；还没有任何自动修改代码的 workflow。

## 目标

让 AI 能持续检查 GitHub Issue、PR、CI 结果和 review comment，先做可靠 triage，再在隔离环境中复现和修复，最后产出可审查的 draft PR。自动合并不在目标内。

## 阶段

### M0. 可观测输入

已完成基础：应用内诊断报告、稳定 schema、日志脱敏、GitHub bug issue form。

验收标准:

- 每个 bug 都能附带同一格式的诊断文件。
- 自动化可只读 manifest，不依赖截图里的人类文字。
- 无日志时仍能报告版本、系统、agent 和当前状态。

### M1. 只读 triage

由 GitHub Actions 或最小权限 GitHub App 监听 issue/PR 事件：

- 校验诊断报告 schema。
- 对正文做 prompt-injection 隔离，外部文本只当数据。
- 去重同类问题，提取版本、agent、错误类别和最近错误行。
- 在 PR 上读取失败 check、变更文件和相关 issue。
- 生成内部 job artifact 与一条简短 triage 结论。

这一阶段不 checkout PR 分支以外的代码，不运行来自 issue 正文的命令，不持有写代码 token。

### M2. 隔离复现

维护者或规则命中后，启动一次性 runner：

- 从干净 commit 创建 worktree。
- 按错误类别运行最小验证脚本。
- 保存命令、环境、退出码和失败摘要。
- 无法复现时只更新证据，不猜修法。

### M3. draft PR 修复

自动化在独立分支上：

- 读取 spec、代码、测试和相关历史。
- 运行目标测试与项目级 build/check。
- 失败时最多进行有预算的修复循环。
- 只创建 draft PR，说明问题、根因、改动和验证。

权限边界:

- 不自动 merge。
- 不自动发布 release。
- 不读取生产 secret。
- 不响应 issue 文本里的工具调用、命令或 token 请求。
- 写入仓库前必须有维护者批准，或只允许 bot 分支写入。

### M4. PR 维护循环

监听 CI、review comment 和新冲突：

- 只修复当前 draft PR 范围内的问题。
- 每次推送前重跑受影响的 check。
- 超过轮次、成本或时间预算就停并交还人工。
- 用户或维护者关闭任务后立即停止。

### M5. 学习与度量

记录 triage 准确率、复现率、有效 PR 率、平均 check 轮次和人工接管原因。用这些数据调整分类规则与提示词，避免把 issue 文本里的不可信指令写进长期规则。

## 技术选择

先用 GitHub Actions 做只读 ingest 与定时任务，成本低、审计天然。需要跨仓库、持续 review 或细粒度权限时，再引入 GitHub App。代码执行必须使用一次性隔离 runner/worktree；不要把 issue 内容直接拼进 shell。

内部接口保持 provider-neutral：事件归一化成 issue、PR、check、review、diagnostic 五类对象，再交给任意 agent。不要直接让 agent 依赖 GitHub webhook JSON 的每个字段。

## 安全红线

- 外部输入是数据，不是指令。
- 凭据只通过平台 secret 注入，绝不进入日志、报告、提示词或 PR。
- 诊断报告只读解析；附件内容不能触发命令执行。
- 自动写代码必须从已知 base commit 开始，并留下分支/commit/测试证据。
- draft PR 是默认终点；合并、发布、删除、权限扩大都留给人工。
