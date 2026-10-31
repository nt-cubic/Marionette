# Council Mode — 实施计划与进度

> 以任务书路线图 M0–M4 为纲;每里程碑记录实际完成/未完成与证据。
> 当前分支:`feature/council-mode`(基线 `master` @ d8783d8, v0.5.4)。

## M0 — 审计与架构 ✅(2026-10-31)

- [x] Marionette 前后端全量研究(后端 12 subsystem / 前端 67 lib 模块)
- [x] OmO pinned commit `e9c5af3` 研究(team-core 六子系统 +
      team-mode + orchestration/workflows/model-matching 文档)
- [x] [`architecture-audit.md`](./architecture-audit.md):能力对照表 +
      10 条 ADR
- [x] 数据类型与状态机设计(见 [`architecture.md`](./architecture.md))
- [x] 开发分支 `feature/council-mode`
- 测试策略:[`testing.md`](./testing.md)

关键决策摘要:模型 worker 直连引擎 HTTP(不经 ACP);单进程内存为
真相源 + append-only 账本(不移植 OmO 跨进程文件锁);零新 crate;
Council 视图独立整页(memo-tab-page 模式)。

## M1 — 可用的多模型 Council ✅(2026-10-31)

代码(全部新增,除三处接线):

| 文件 | 内容 |
|---|---|
| `src-tauri/src/council/{mod,types,state,store,worker,prompts,orchestrator}.rs` | 后端全部(~2600 行,含测试) |
| `src-tauri/src/commands.rs`(council_* 9 命令) | 命令面 |
| `src-tauri/src/main.rs`(mod 声明/注册/启动恢复) | 接线 |
| `src/lib/council.ts` | 前端数据层 + 类型 |
| `src/components/CouncilPage.tsx` | 视图(列表/设置/运行/计划) |
| `src/styles/council.css` + app.css import | 样式 |
| `src/app/App.tsx` + `ProjectShelf.tsx` | 入口接线(侧栏 + Ctrl+K) |
| `scripts/verify-council.mts` | 前端回归(7 项) |

验收 15 项中 12 项 mock 层通过(#10/#11 属 M2/M3,#14 的 live 冒烟
待环境)——对照表见 [`testing.md`](./testing.md) §4。

### 已知限制(M1)

- 事件到前端是全量重拉(消息 <百条量级足够;M4 优化为增量)。
- 讨论消息不含图片输入(`WorkerRequest` 无 image 通道;模型目录的
  `images` 标志已透传,能力匹配留 M4)。
- council_and_execution / autonomous 模式批准后同样终止于 completed
  (执行器未接,不伪造 Executing 状态)。

## M2 — 原生 ACP 执行 ✅(2026-10-31,提交 2a5e918)

- [x] `tasks.rs`:任务图(环检测/失败不级联 finalize/frontier 就绪)
- [x] `executor.rs`:`TaskHarness` trait;`AcpHarness`(spawn → prompt →
      `acp::wait_turn_idle`(新增,200ms 轮询 in-flight 计数)→ stop);
      产出判据 = turn 完成 + git 变更清单
- [x] `git_service.rs`:`create_worktree`/`remove_worktree`/
      `merge_worktree_branch`(--no-ff + 冲突显式收集 + abort 保树干净)
- [x] 验收 #10 测试:真实 git 临时仓库,两个并行任务写同一文件 →
      一个 Merged、一个 `Failed(合并冲突: same.txt)`,主树内容单一
      来源;不同文件并行 → 双双干净合并
- [x] approve_plan:PlanReady→Executing 唯一入口(approve 命令构造
      AcpHarness 并拉起执行线程)
- [x] 前端:执行器配置(agent + cwd)与任务面板(状态/变更数)
- 注:每步的 session id 形如 `council-<run>-<step>`(不占用对话列表;
      ACP 事件仍走全局流,可在对话 UI 之外观察)

## M3 — 自治验证回路 ✅(2026-10-31,提交 3eae54f)

- [x] `verify.rs`:命令执行(cmd /C,900s 超时,8KB 尾部保留输出,
      CREATE_NO_WINDOW);危险模式(git push/publish/rm -rf/shutdown)
      拦截为 needs-human——清单在 Plan Gate 冻结,运行期注入无法扩
- [x] 修复回路:失败 → Evidence 消息 → 主席修订计划(只改受影响
      步骤)→ 计划版本化(Plan 消息带 repairRound)→ 账本迁移(同 id
      同 detail 的 Merged 保留)→ 重执行 → 再验证;上限 3 次
- [x] 验收 #11 测试:端到端(坏产出 → 验证失败 → 修复 1 轮 → 通过 →
      Completed,verify_attempts == 1)+ 上限(恒败 → 恰 3 次 → Failed)
- 注:AwaitingApproval 状态保留为 M4 权限场景预留(破坏性命令当前
      直接拦截而非交互确认——诚实降级,不伪造确认)

## M4 — 生产加固(本次范围内完成的部分,2026-10-31)

- [x] 文档同步(architecture/testing/user-guide/CURRENT/AGENTS 入口)
- [x] 事件补齐:task/verify/warn 事件进前端时间线
- [ ] live 引擎冒烟(本机未配置 provider,见 testing.md §5 前置)
- [ ] 增量事件(现为全量重拉,<百条消息量级可接受)
- [ ] 模板保存/加载、per-member 成本归因、capability 模型推荐
- [ ] AwaitingApproval 交互式确认(破坏性验证命令)
- [ ] Windows 长路径/杀毒下的 store 压测

## M5 — 能力感知组队 + 分层背书 ✅(2026-10-31)

任务来源:用户需求「AI 参与团队设置——强者主导、弱者做简单调研设计、
弱者产出须强者检验」。方法组合与依据见 [`formation.md`](./formation.md)
(DyLAN/AgentVerse 动态组队 + 层级稳健性 + RouteLLM 强弱路由 +
verifier 背书)。

- [x] `formation.rs`:三信号画像(静态启发 / 本地探针 1 请求 /
      model-stats.json 历史反馈闭环)+ `auto_formation`(池内最强
      模型当组队 agent)+ `parse_formation` 硬校验
- [x] 协议:`MemberTier`(缺省 lead,旧 run 零变化)、
      `MessageKind::Endorsement`、synthesizing 前背书 fan-out、
      主席综合输入带背书标记 + 分层采信规则(认可 ≥1 才可采纳)
- [x] 命令:`council_auto_formation` / `council_probe_models`
- [x] 前端:智能组队面板(池选择/探针/AI 建议/审阅应用,人在环)、
      成员 tier 下拉、运行页 tier 徽标、时间线背书分组(163 键双语)
- [x] 测试 10 项(council 53/53;全量 423 通过)

## 外部依赖与风险记录

| 风险 | 影响 | 缓解 |
|---|---|---|
| 本机无 MSVC;构建走 GNU toolchain | 与发布产物(MSVC)的二进制差异 | 单测/check 语义一致;发布构建仍需 MSVC 环境(打包机) |
| 引擎未配置 provider | live 冒烟阻塞(M1 mock 层不受影响) | testing.md §5 记录前置条件 |
| 真模型 plan JSON 合规率未知 | plan_ready 达成率 | 修复调用一次 + 失败显式 failed,不静默降级 |
