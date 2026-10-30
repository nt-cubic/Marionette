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

## M2 — 原生 ACP 执行(未开始)

1. `council/executor.rs`:PlanDoc → TaskGraph(id/depends_on/owner/
   status);frontier 调度(依赖就绪即入队)。
2. 每 task:`storage::create_child_session`(origin:"council")→
   `acp::start`(cwd = 项目根或专属 worktree)→ prompt 注入
   (step detail + acceptance + 计划全文摘要)→ 轮询 turn/complete。
3. `git_service::create_worktree`(薄封装 `git worktree add --detach`);
   重叠文件集的任务强制隔离(验收 #10)。
4. 交付证据:复用 `begin/end_workspace_snapshot` per task。
5. 验收 #10 的测试:两个并行 task 改同一文件 → worktree 隔离下合并
   冲突显式暴露而非静默覆盖。

## M3 — 自治验证回路(未开始)

1. `council/verify.rs`:verification 命令逐条跑(std Command,超时,
   输出截断),证据消息(kind=evidence)回注。
2. 失败 → targeted revision:失败证据 + 相关 step 回质询(受
   `repair_limit=3` 约束)→ 更新计划 → 只重跑受影响 task。
3. AwaitingApproval 用于破坏性验证命令的人工确认。
4. 验收 #11 测试:注入失败 verification → 断言修订回路触发且 ≤3 次。

## M4 — 生产加固(未开始)

- 增量事件(消息 diff)、模板保存/加载、成本归因 per member、
  capability-based 模型推荐(目录 images/context 字段)、
  Windows 长路径/杀毒扫描下的 store 压测、文档完善。

## 外部依赖与风险记录

| 风险 | 影响 | 缓解 |
|---|---|---|
| 本机无 MSVC;构建走 GNU toolchain | 与发布产物(MSVC)的二进制差异 | 单测/check 语义一致;发布构建仍需 MSVC 环境(打包机) |
| 引擎未配置 provider | live 冒烟阻塞(M1 mock 层不受影响) | testing.md §5 记录前置条件 |
| 真模型 plan JSON 合规率未知 | plan_ready 达成率 | 修复调用一次 + 失败显式 failed,不静默降级 |
