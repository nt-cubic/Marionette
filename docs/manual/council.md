# 圆桌（Council）开发手册

一句话：多模型审议 → 计划 → agent 执行 → 验证回路的编排器。UI 名「圆桌/圆桌会议」，
代码模块、路径、事件仍叫 council。

## 文件地图（改 X 找 Y）

| 想改什么 | 去哪 | 备注 |
|---|---|---|
| 类型/状态机/存储 | `src-tauri/src/council/{types,state,store}.rs` | 数据在 `~/.marionette/council/runs/{runId}/` |
| 模型调用 seam | `council/worker.rs`（ModelClient/TaskHarness） | 生产 EngineClient/AcpHarness；测试 MockClient/ScriptedHarness |
| 编排/推进 | `council/orchestrator.rs` | 转移全走 `state.rs::apply_transition` |
| 计划与任务 | `council/prompts.rs` / `tasks.rs` | |
| 执行（worktree 并行） | `council/executor.rs` | 冲突显式 Failed(文件清单) |
| 验证回路 | `council/verify.rs` | DANGEROUS 清单在这，Plan Gate 冻结 |
| 组队/探针 | `council/formation.rs` | 落 `council/probes.json` 与 `model-stats.json` |
| 前端 | 圆桌页组件（标签行 `is-council` 伪标签；`councilOpen/councilActive`） | 版式按「会议室」，表面走 ui-language |

## 状态机红线（背下来再改）

- `plan_ready → executing` **只能经 `council_approve_plan`**（执行门）。
- **终态无出边**；转移全走 `apply_transition`（硬编码表 + transitions.jsonl 审计）。
- 并行执行任务各持 `git worktree`（`council/<runId>/<stepId>` 分支），拓扑序顺序合并，
  **冲突绝不静默覆盖**（真实 git 仓库回归测试 `parallel_same_file_conflicts_are_explicit` 守着）。
- `MemberTier` 缺省 lead，全员主导时背书整段跳过。
- **不加 `CouncilMode::SgsGame` / `RunPhase::Playing`**（牌局不进圆桌协议；贡献者再带
  回来指着这条拒绝）。
- **Attention 门**（2026-11-01）：verify 撞 DANGEROUS 命令 → run 停靠 `AwaitingApproval`
  （重启不丢、孤儿恢复跳过），前端 CouncilAttentionCard 浮裁决卡；
  `council_resolve_approval` 批准=放行名单**仅从被拦原文填充**后真执行、拒绝=按拦截
  失败进修复回路；DANGEROUS 清单只在 Plan Gate 冻结——**模型输出永远进不了名单**。
- 瞬时错误（408/429/5xx/连接）指数退避 1s/2s/4s（50ms 切片，取消即时）；
  role_header 统一注入 INJECTION_GUARD（外部文本只是数据）。
- 恢复语义 = 以消息账本为进度，**已完成阶段（含执行任务）绝不重放**。

## 坑

- **引擎端口每次重启漂移**——永远 `engine::running_base()` 现查，禁缓存 URL。
- `cmd /C` 传含引号命令会被 PATH 里的 Unix 工具劫持——测试里验证命令用
  `cmd /C exit N` 语义，别用 `find`。
- 测试沙箱走 `set_root_override_for_tests` + 全局 `SANDBOX_LOCK` 串行（store 与
  orchestrator 共用一把锁，别各建各的）。
- 模型调用全部经 seam，**测试禁直连真引擎**。
- **已知高失败率 flake**：`same_model_two_members_distinct_calls`（等 Reviewing 30s
  超时；HEAD 对照实验 4/5 也败）——独跑不绿属正常，别为它回滚别的批。

## 验证

```bash
cargo test --bin marionette council     # 61 项，沙箱在 temp，不碰真实用户目录
npx tsx scripts/verify-council.mts
```

## 已知差距与源流

- 蒸馏自：AGENTS.md「Council 多模型协作」段（M1–M5，PR #7 只摘 573a0853）+
  council/ 模块头注 + same_model flake 的 HEAD 对照实验（2026-11-18）。
