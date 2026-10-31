# Council Mode — 测试与验收

> 状态:M1–M3(2026-10-31)。真模型(live 引擎)冒烟尚未执行——本机
> 引擎未配置可用 provider;全部自动化验证在 mock 层完成(验收 #14:
> mock 与 live 结果明确区分)。

## 1. 测试入口

```bash
# Rust 单测(状态机/协议/恢复/预算/取消/隔离/执行/验证回路)
cd src-tauri && cargo test --bin marionette council  # 43 项

# 前端回归(展示逻辑/类型契约)
npx tsx scripts/verify-council.mts

# 全量回归(含既有全部 verify 脚本)
npm run verify
npm run build
cd src-tauri && cargo test --bin marionette
```

## 2. Rust 测试清单(`cargo test council`)

| 测试 | 验证点 | 任务书验收项 |
|---|---|---|
| `state::happy_path_transitions` | created→…→completed 全链合法 | — |
| `state::execution_gate_blocks_orchestrator_paths` | plan_ready 不可跳批准直上执行态 | #9 |
| `state::terminal_states_frozen` | 四终态无出边 | — |
| `state::pause_resume_round_trip` | paused→原状态,别的目标被拒 | — |
| `state::paused_can_be_cancelled` | 暂停态可直接取消 | — |
| `state::revision_loop_allowed` | reviewing⇄revising 环 | — |
| `store::spec_state_round_trip` | 快照持久化往返 | #7 前置 |
| `store::message_append_and_dedup` | 同 messageId 重复 append 被拒 | #5 |
| `store::run_id_rejects_path_injection` | run id 路径注入防御 | — |
| `worker::mock_client_distinguishes_models` | mock 按模型区分响应 | #1/#14 |
| `worker::engine_client_reports_missing_engine` | 引擎缺席给人话错误 | #15 |
| `prompts::proposal_prompt_isolated_from_other_proposals` | Phase A prompt 不含他人提案 | #3 |
| `prompts::latest_positions_prefers_revision` | 评审看最新版本 | #4 |
| `prompts::pending_critiques_clear_after_revision` | 修订后质询视为已回应 | #4 |
| `orchestrator::full_protocol_run` | 3 模型全协议到 plan_ready,3 模型都收到请求 | #1 |
| `orchestrator::proposal_phase_information_isolation` | 提案调用 prompt 无他人标记 | #3 |
| `orchestrator::critique_targets_all_other_authors` | 质询覆盖其余全部作者 | #4 |
| `orchestrator::single_worker_failure_does_not_kill_run` | 1/3 失败,run 到 plan_ready | #6 |
| `orchestrator::resume_does_not_replay_completed_phase` | 崩溃后重启不重调 briefing/proposal | #7 |
| `orchestrator::cancel_stops_new_calls` | 取消后调用数冻结 | #8 |
| `orchestrator::plan_gate_blocks_unapproved_execution` | 状态机执行门 | #9 |
| `orchestrator::budget_exhaustion_is_terminal` | 请求预算触顶→终态 | §6 |
| `orchestrator::same_model_two_members_distinct_calls` | 同模型两成员各自调用 | #2 |
| `orchestrator::parse_plan_accepts_fenced_json` | 计划围栏解析+空步骤拒绝 | — |
| `tasks::cycle_detection_rejects_circular_plan` | 环/自依赖拒绝;图外依赖不误报 | §4.2 |
| `tasks::frontier_unlocks_only_when_deps_complete` | frontier 调度语义 | §4.2 |
| `tasks::failure_does_not_cascade_but_finalizes_skipped` | 失败不级联、收尾 skip | §4.2 |
| `executor::parallel_same_file_conflicts_are_explicit` | 并行同文件 → 显式冲突,不静默覆盖 | **#10** |
| `executor::parallel_distinct_files_merge_cleanly` | 并行不同文件 → 干净合并 + 依赖解锁 | §4.2 |
| `executor::failed_step_skips_downstream` | 步骤失败 → 下游 skip | §4.2 |
| `executor::step_prompt_contains_plan_and_constraints` | 执行 prompt 自足性 | §4.1 |
| `verify::success_and_failure_commands` | 命令 exit code 语义 | §4.3 |
| `verify::timeout_kills_command` | 超时终止 | §4.3 |
| `verify::dangerous_commands_are_intercepted` | 破坏性命令拦截(词边界不误杀) | §4.4 |
| `verify::evidence_rendering` | 证据渲染 | §4.3 |
| `orchestrator::verification_failure_triggers_bounded_repair` | 端到端修复回路(恰 1 轮) | **#11** |
| `orchestrator::repair_loop_is_bounded` | 恒败 → 恰 3 次 → Failed | **#11** |
| `types::spec_validation_rejects_bad_input` | 创建期 spec 校验 | — |

## 3. 前端回归(`scripts/verify-council.mts`,7 项)

时间线分组(简报独立/同轮同类聚组/跨轮分开)、成员配色稳定、阶段/
消息类型文案、终态判定、成员状态文案(含失败分支)、派生 id 去重、
PlanDoc camelCase 契约锁定。

## 4. 验收项覆盖状态(任务书 §8,15 项)

| # | 验收项 | 状态 | 证据 |
|---|---|---|---|
| 1 | 不同模型触发不同请求 | ✅ mock 层 | full_protocol_run(3 模型各收到请求) |
| 2 | 一模型多成员各自独立 | ✅ mock 层 | same_model_two_members_distinct_calls |
| 3 | 提案不互相泄露 | ✅ | proposal_phase_information_isolation + prompts 单测 |
| 4 | 质询路由正确 | ✅ | critique_targets_all_other_authors + prompts 路由单测 |
| 5 | 重复投递不重复入账 | ✅ | store::message_append_and_dedup + persist_message 双挡 |
| 6 | 单 worker 崩溃不毒化 run | ✅ | single_worker_failure_does_not_kill_run |
| 7 | 重启恢复不重放 | ✅ | resume_does_not_replay_completed_phase |
| 8 | 取消阻止新调用 | ✅ | cancel_stops_new_calls |
| 9 | 计划门前无执行 | ✅ | plan_gate_blocks + 状态机表(Executing 唯一入口 = approve_plan) |
| 10 | 并行执行不互相覆盖 | ✅ mock harness + 真实 git | executor::parallel_same_file_conflicts_are_explicit(真 git worktree) |
| 11 | 验证失败→受限修订 | ✅ | verification_failure_triggers_bounded_repair + repair_loop_is_bounded |
| 12 | 既有会话不受影响 | ✅ | npm run verify 全量 + cargo test 全量回归 |
| 13 | Magpie 配置不变 | ✅ | 只读消费 catalog/completions,零配置写入 |
| 14 | mock/live 明确区分 | ✅ | ModelClient trait 双实现;live 冒烟未跑(见 §5) |
| 15 | 认证/预算/权限失败上浮 | ✅ 部分 | 预算终态+worker 错误进消息流;引擎未就绪有引导;危险命令拦截为 needs-human |

## 5. live 冒烟(未执行,待环境)

前置:本机 Marionette 引擎 + 至少一个可用模型来源(订阅或 key)。
步骤:`npm run tauri dev` → 侧栏「圆桌」→ 选 3 个真实模型 →
小任务(如"给 LRU 缓存设计并发淘汰策略")→ 观察五阶段 → plan_ready
后批准。失败模式重点观察:引擎端口漂移、真模型不吐合法 plan JSON
(修复链路)、token 估算与实际用量的偏差。
