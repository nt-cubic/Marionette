//! Council 执行器:PlanDoc → 并行/串行任务执行 → worktree 隔离 → 合并。
//!
//! Harness 抽象(对齐 worker.rs 的 ModelClient 模式):生产 `AcpHarness`
//! 走 AcpService(真 coding agent),测试 `ScriptedHarness` 注入结果。
//! 执行判据是**工作区变更**而非 agent 的"done"声明(任务书 §4.3:
//! 成功的工具调用不是成功证据);文字回复不进 council 账本,M3 的
//! 验证命令才是最终判据。
//!
//! 隔离策略(验收 #10):并行任务各持一个 `git worktree`(`council/`
//! 分支),完成后按计划拓扑序**顺序合并**——冲突显式上报为 Failed
//! (证据 = 冲突文件),绝不静默覆盖。非 git 项目退化为同 cwd 串行
//! 执行 + 显著警告。

use super::tasks::{self, TaskProgress, TaskStatus};
use super::types::PlanStep;
use std::path::{Path, PathBuf};

/// 单步执行的输入。
pub struct StepContext {
    pub session_id: String,
    pub prompt: String,
    /// 执行 cwd(worktree 或项目根)。
    pub cwd: String,
    pub timeout_secs: u64,
}

/// 单步执行结果。
#[derive(Clone, Debug, Default)]
pub struct StepOutcome {
    pub turn_completed: bool,
    /// harness 侧的人类可读备注(变更摘要/失败原因)。
    pub notes: String,
}

/// 执行 harness 抽象。生产 ACP / 测试脚本化。
pub trait TaskHarness: Send + Sync {
    fn name(&self) -> &'static str;
    /// 阻塞执行一步:spawn 会话 → prompt → 等待 turn → 清理。
    fn run_step(&self, ctx: &StepContext) -> Result<StepOutcome, String>;
}

// ---------------------------------------------------------------------------
// 生产:AcpHarness
// ---------------------------------------------------------------------------

pub struct AcpHarness {
    pub app: tauri::AppHandle,
    pub acp: crate::acp::AcpService,
    pub agent_id: String,
    pub command: String,
    pub args: Vec<String>,
}

impl TaskHarness for AcpHarness {
    fn name(&self) -> &'static str {
        "acp"
    }

    fn run_step(&self, ctx: &StepContext) -> Result<StepOutcome, String> {
        self.acp.start(
            self.app.clone(),
            ctx.session_id.clone(),
            self.command.clone(),
            self.args.clone(),
            ctx.cwd.clone(),
            Some(self.agent_id.clone()),
        )?;
        let prompt_result =
            self.acp.send_prompt(&ctx.session_id, ctx.prompt.clone(), Vec::new(), Vec::new());
        if let Err(e) = prompt_result {
            let _ = self.acp.stop(&ctx.session_id);
            return Err(format!("发送任务失败:{e}"));
        }
        let idle = self.acp.wait_turn_idle(&ctx.session_id, std::time::Duration::from_secs(ctx.timeout_secs.max(1)));
        let changes = crate::git_service::get_changed_files(Path::new(&ctx.cwd))
            .map(|files| files.len())
            .unwrap_or(0);
        let _ = self.acp.stop(&ctx.session_id);
        if !idle {
            return Err(format!("任务超时({}s)未完成", ctx.timeout_secs));
        }
        Ok(StepOutcome { turn_completed: true, notes: format!("回合完成;工作区变更文件 {changes} 个") })
    }
}

// ---------------------------------------------------------------------------
// 测试:ScriptedHarness
// ---------------------------------------------------------------------------

/// 脚本化 harness:按 step_id 给结果;同时执行文件写动作模拟真实编码
/// (验收 #10 用:两个并行任务写同一文件 → 合并冲突)。
#[cfg(test)]
pub struct ScriptedHarness {
    pub timeout_secs: u64,
}

#[cfg(test)]
impl TaskHarness for ScriptedHarness {
    fn name(&self) -> &'static str {
        "scripted"
    }

    fn run_step(&self, ctx: &StepContext) -> Result<StepOutcome, String> {
        // prompt 约定:最后一行 "WRITE <path>|<content>" 时执行写入。
        if let Some(line) = ctx.prompt.lines().rev().find(|l| l.starts_with("WRITE ")) {
            let rest = &line["WRITE ".len()..];
            if let Some((path, content)) = rest.split_once('|') {
                let target = PathBuf::from(&ctx.cwd).join(path);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                }
                std::fs::write(&target, content).map_err(|e| format!("写入 {path} 失败:{e}"))?;
                return Ok(StepOutcome { turn_completed: true, notes: format!("已写入 {path}") });
            }
        }
        if ctx.prompt.contains("FAIL_STEP") {
            return Err("注入的步骤失败".into());
        }
        Ok(StepOutcome { turn_completed: true, notes: "脚本执行完成".into() })
    }
}

// ---------------------------------------------------------------------------
// 执行循环(run_loop 的 Executing 分支调用)
// ---------------------------------------------------------------------------

pub struct ExecuteContext<'a> {
    pub run_id: &'a str,
    pub project_root: PathBuf,
    pub plan: &'a super::types::PlanDoc,
    pub harness: &'a dyn TaskHarness,
    pub max_concurrent: u32,
    pub step_timeout_secs: u64,
    /// 是否用 worktree 隔离(git 仓库且 ready 批次可能并行)。
    pub use_worktrees: bool,
    pub cancel: &'a std::sync::atomic::AtomicBool,
}

/// 执行一个 ready 批次(frontier 语义:一次只放行当前就绪任务,批内
/// 并行)。任务状态直接写回 `tasks`;进度落盘与事件由调用方(run_loop)
/// 在批次边界做。返回本批是否有进展(用于退出判定)。
pub fn execute_ready_batch(
    ctx: &ExecuteContext,
    tasks: &mut Vec<TaskProgress>,
    on_task_update: &mut dyn FnMut(&TaskProgress),
) -> bool {
    crate::debug_log::append(
        "council",
        "info",
        "",
        "execute batch",
        Some(&format!("run={} harness={}", ctx.run_id, ctx.harness.name())),
    );
    let ready: Vec<PlanStep> = tasks::ready_steps(ctx.plan, tasks)
        .into_iter()
        .filter(|s| ctx.plan.steps.iter().any(|p| p.id == s.id))
        .cloned()
        .collect();
    if ready.is_empty() {
        return false;
    }
    let parallel = ready.len() > 1 && ctx.use_worktrees;
    let mut progressed = false;

    for step in &ready {
        let prog = tasks.iter_mut().find(|t| t.step_id == step.id).unwrap();
        prog.status = TaskStatus::Running;
        prog.attempts += 1;
    }
    // 批次开始时通知一次(整批 running)。
    if let Some(first) = ready.first() {
        if let Some(snapshot) = tasks.iter().find(|t| t.step_id == first.id).cloned() {
            on_task_update(&snapshot);
        }
    }

    // scope 并发执行整批(信号量在批内限流)。
    let sem = super::orchestrator::Semaphore::new(ctx.max_concurrent.max(1));
    let results: std::sync::Mutex<Vec<(String, Result<StepOutcome, String>, Option<String>, Option<String>)>> =
        std::sync::Mutex::new(Vec::new()); // (step_id, outcome, worktree, branch)
    let worktree_root = ctx.project_root.parent().map(|p| p.join(".marionette-council-worktrees")).unwrap_or_else(|| ctx.project_root.clone());

    std::thread::scope(|scope| {
        for step in &ready {
            let isolated = parallel && ctx.use_worktrees;
            let (worktree, branch) = if isolated {
                let branch = format!("council/{}/{}", ctx.run_id, step.id);
                let path = worktree_root.join(format!("{}-{}", ctx.run_id, step.id));
                match crate::git_service::create_worktree(&ctx.project_root, &path, &branch) {
                    Ok(()) => (Some(path.to_string_lossy().to_string()), Some(branch)),
                    Err(e) => {
                        // worktree 建不了(并发窗口/权限):记录为该步失败,
                        // 不悄悄退回共享 cwd(那才是静默覆盖的温床)。
                        results.lock().unwrap().push((step.id.clone(), Err(format!("创建隔离 worktree 失败:{e}")), None, None));
                        continue;
                    }
                }
            } else {
                (None, None)
            };
            let cwd = worktree.clone().unwrap_or_else(|| ctx.project_root.to_string_lossy().to_string());
            let session_id = format!("council-{}-{}", ctx.run_id, step.id);
            let prompt = step_prompt(ctx.plan, step, parallel);
            let step_ctx = StepContext {
                session_id,
                prompt,
                cwd,
                timeout_secs: ctx.step_timeout_secs,
            };
            let sem = &sem;
            let results = &results;
            let cancel = ctx.cancel;
            scope.spawn(move || {
                if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    results.lock().unwrap().push((step.id.clone(), Err("已取消".into()), worktree, branch));
                    return;
                }
                sem.acquire();
                let out = ctx.harness.run_step(&step_ctx);
                sem.release();
                results.lock().unwrap().push((step.id.clone(), out, worktree, branch));
            });
        }
    });

    // 回收结果(单写者)。
    let mut collected = results.into_inner().unwrap();
    for (step_id, outcome, worktree, branch) in collected.drain(..) {
        let prog = tasks.iter_mut().find(|t| t.step_id == step_id).unwrap();
        prog.worktree = worktree.clone();
        prog.branch = branch.clone();
        match outcome {
            Ok(out) if out.turn_completed => {
                let changes = crate::git_service::get_changed_files(Path::new(
                    worktree.as_deref().unwrap_or(&ctx.project_root.to_string_lossy()),
                ))
                .map(|files| files.into_iter().map(|f| f.path).collect())
                .unwrap_or_default();
                prog.changes = changes;
                prog.notes = out.notes;
                prog.status = TaskStatus::Staged;
                prog.error = None;
                progressed = true;
            }
            Ok(out) => {
                let reason = if out.notes.is_empty() { "回合未完成".to_string() } else { out.notes };
                prog.status = TaskStatus::Failed { reason: reason.clone() };
                prog.error = Some(reason);
                if let (Some(w), Some(_b)) = (worktree.as_deref(), branch.as_deref()) {
                    let _ = crate::git_service::remove_worktree(&ctx.project_root, Path::new(w));
                }
            }
            Err(e) => {
                prog.status = TaskStatus::Failed { reason: e.clone() };
                prog.error = Some(e);
                if let (Some(w), Some(_b)) = (worktree.as_deref(), branch.as_deref()) {
                    let _ = crate::git_service::remove_worktree(&ctx.project_root, Path::new(w));
                }
            }
        }
        on_task_update(prog);
    }
    progressed
}

/// 把 Staged 的任务按计划拓扑序合并回项目工作区。冲突 → 该任务
/// Failed(证据 = 冲突文件),其余继续;返回 (merged_count, conflict_count)。
pub fn merge_staged(
    ctx: &ExecuteContext,
    tasks: &mut Vec<TaskProgress>,
    on_task_update: &mut dyn FnMut(&TaskProgress),
) -> (usize, usize) {
    let mut merged = 0;
    let mut conflicts = 0;
    for step_id in tasks::merge_order(ctx.plan) {
        let Some(prog) = tasks.iter_mut().find(|t| t.step_id == step_id && t.status == TaskStatus::Staged) else {
            continue;
        };
        let Some(branch) = prog.branch.clone() else {
            // 无 worktree 的串行任务:改动已在项目 cwd,直接记完成。
            prog.status = TaskStatus::Merged;
            on_task_update(prog);
            merged += 1;
            continue;
        };
        let worktree = prog.worktree.clone().unwrap_or_default();
        let title = prog.title.clone();
        match crate::git_service::merge_worktree_branch(&ctx.project_root, Path::new(&worktree), &branch, &format!("council({}): {}", ctx.run_id, title)) {
            Ok(crate::git_service::MergeOutcome::Clean { files }) => {
                prog.status = TaskStatus::Merged;
                if !files.is_empty() {
                    prog.changes = files;
                }
                let _ = crate::git_service::remove_worktree(&ctx.project_root, Path::new(&worktree));
                merged += 1;
            }
            Ok(crate::git_service::MergeOutcome::Conflict { files }) => {
                // 验收 #10:并行改动冲突显式暴露,不静默覆盖。
                prog.status = TaskStatus::Failed { reason: format!("合并冲突:{}", files.join(", ")) };
                prog.error = Some(format!("合并冲突:{}", files.join(", ")));
                let _ = crate::git_service::remove_worktree(&ctx.project_root, Path::new(&worktree));
                conflicts += 1;
            }
            Err(e) => {
                prog.status = TaskStatus::Failed { reason: format!("合并失败:{e}") };
                prog.error = Some(format!("合并失败:{e}"));
                let _ = crate::git_service::remove_worktree(&ctx.project_root, Path::new(&worktree));
                conflicts += 1;
            }
        }
        on_task_update(prog);
    }
    (merged, conflicts)
}

/// 执行一步的 prompt:完整计划上下文 + 本步详情 + 约束。
pub fn step_prompt(plan: &super::types::PlanDoc, step: &PlanStep, parallel: bool) -> String {
    let mut deps = String::new();
    for s in &plan.steps {
        deps.push_str(&format!("- [{}] {}({}依赖)\n", s.id, s.title, if s.depends_on.is_empty() { "无".into() } else { s.depends_on.join("+") }));
    }
    let scope = if parallel {
        "**你在一个隔离的 git worktree 里工作**,只改动本任务涉及的文件;其他任务并行进行,不要触碰它们负责的文件。"
    } else {
        "直接在项目目录工作。"
    };
    format!(
        "你是 Council 审议产出的实施计划的一名执行者。整个计划:\n{deps}\n\
         ## 你要执行的任务\n[{id}] {title}\n{detail}\n\n验收标准:{acceptance}\n\
         {scope}\n完成该任务所需的全部代码修改;完成后简述你改了什么。不要执行计划中其他步骤。",
        id = step.id,
        title = step.title,
        detail = step.detail,
        acceptance = step.acceptance
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::council::types::{PlanDoc, PlanStep};

    fn step(id: &str, detail: &str, deps: Vec<&str>) -> PlanStep {
        PlanStep {
            id: id.into(),
            title: format!("step {id}"),
            detail: detail.into(),
            depends_on: deps.into_iter().map(|s| s.to_string()).collect(),
            acceptance: "文件存在".into(),
            owner_hint: None,
        }
    }

    fn doc(steps: Vec<PlanStep>) -> PlanDoc {
        PlanDoc { summary: "s".into(), steps, open_questions: vec![], acceptance: vec![], verification: vec![] }
    }

    /// 真实 git 临时仓库端到端:两个并行任务写同一文件 → 合并冲突显式
    /// 上报(验收 #10);不同文件 → 双双合并成功。
    #[test]
    fn parallel_same_file_conflicts_are_explicit() {
        let root = std::env::temp_dir().join(format!("marionette-council-exe-{}", crate::council::types::new_run_id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(&root).status().unwrap().success());
        for cfg in [["user.email", "t@t"], ["user.name", "t"]] {
            assert!(std::process::Command::new("git").args(["config", cfg[0], cfg[1]]).current_dir(&root).status().unwrap().success());
        }
        std::fs::write(root.join("base.txt"), "base\n").unwrap();
        assert!(std::process::Command::new("git").args(["add", "--all"]).current_dir(&root).status().unwrap().success());
        assert!(std::process::Command::new("git").args(["commit", "-q", "-m", "init"]).current_dir(&root).status().unwrap().success());

        let plan = doc(vec![
            step("a", "WRITE same.txt|from-a\n", vec![]),
            step("b", "WRITE same.txt|from-b\n", vec![]),
        ]);
        let mut tasks = crate::council::tasks::build_task_progress(&plan).unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let harness = ScriptedHarness { timeout_secs: 30 };
        let ctx = ExecuteContext {
            run_id: "testconflict",
            project_root: root.clone(),
            plan: &plan,
            harness: &harness,
            max_concurrent: 2,
            step_timeout_secs: 30,
            use_worktrees: true,
            cancel: &cancel,
        };
        let mut updates = 0;
        let progressed = execute_ready_batch(&ctx, &mut tasks, &mut |_| updates += 1);
        assert!(progressed);
        assert_eq!(tasks.iter().filter(|t| t.status == TaskStatus::Staged).count(), 2, "两任务都应在各自 worktree 完成");
        // 各自的 worktree 里文件不同——隔离生效。
        let (merged, conflicts) = merge_staged(&ctx, &mut tasks, &mut |_| {});
        assert_eq!(merged, 1, "先合并的一方成功");
        assert_eq!(conflicts, 1, "后者必然冲突(显式,非静默覆盖)");
        let failed = tasks.iter().find(|t| matches!(t.status, TaskStatus::Failed { .. })).unwrap();
        assert!(failed.error.as_deref().unwrap().contains("same.txt"), "冲突证据点名文件:{:?}", failed.error);
        // 项目根的 same.txt 只含先合并方的内容(没有被覆盖成两份混合)。
        let content = std::fs::read_to_string(root.join("same.txt")).unwrap();
        let trimmed = content.trim_end();
        assert!(trimmed == "from-a" || trimmed == "from-b", "内容应是单一来源:{content}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 端到端链路:两个不同文件的并行任务 → 全部合并;串行依赖解锁顺序。
    #[test]
    fn parallel_distinct_files_merge_cleanly() {
        let root = std::env::temp_dir().join(format!("marionette-council-exe2-{}", crate::council::types::new_run_id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert!(std::process::Command::new("git").args(["init", "-q"]).current_dir(&root).status().unwrap().success());
        for cfg in [["user.email", "t@t"], ["user.name", "t"]] {
            assert!(std::process::Command::new("git").args(["config", cfg[0], cfg[1]]).current_dir(&root).status().unwrap().success());
        }
        std::fs::write(root.join("base.txt"), "base\n").unwrap();
        assert!(std::process::Command::new("git").args(["add", "--all"]).current_dir(&root).status().unwrap().success());
        assert!(std::process::Command::new("git").args(["commit", "-q", "-m", "init"]).current_dir(&root).status().unwrap().success());

        let plan = doc(vec![
            step("a", "WRITE a.txt|A\n", vec![]),
            step("b", "WRITE b.txt|B\n", vec![]),
            step("c", "WRITE c.txt|C\n", vec!["a", "b"]),
        ]);
        let mut tasks = crate::council::tasks::build_task_progress(&plan).unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let harness = ScriptedHarness { timeout_secs: 30 };
        let ctx = ExecuteContext {
            run_id: "testclean",
            project_root: root.clone(),
            plan: &plan,
            harness: &harness,
            max_concurrent: 2,
            step_timeout_secs: 30,
            use_worktrees: true,
            cancel: &cancel,
        };
        // 批 1:a+b 并行。
        assert!(execute_ready_batch(&ctx, &mut tasks, &mut |_| {}));
        let (m1, c1) = merge_staged(&ctx, &mut tasks, &mut |_| {});
        assert_eq!((m1, c1), (2, 0), "两个隔离任务先后干净合并");
        // 批 2:c 依赖解锁。
        assert!(execute_ready_batch(&ctx, &mut tasks, &mut |_| {}));
        let (m2, c2) = merge_staged(&ctx, &mut tasks, &mut |_| {});
        assert_eq!((m2, c2), (1, 0));
        assert!(root.join("a.txt").exists() && root.join("b.txt").exists() && root.join("c.txt").exists());
        assert!(!crate::council::tasks::work_remaining(&plan, &tasks));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 失败步骤:下游 skip,不假成功。
    #[test]
    fn failed_step_skips_downstream() {
        let plan = doc(vec![
            step("a", "FAIL_STEP", vec![]),
            step("b", "WRITE b.txt|B\n", vec!["a"]),
        ]);
        let mut tasks = crate::council::tasks::build_task_progress(&plan).unwrap();
        let root = std::env::temp_dir().join(format!("marionette-council-exe3-{}", crate::council::types::new_run_id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let harness = ScriptedHarness { timeout_secs: 30 };
        let ctx = ExecuteContext {
            run_id: "testfail",
            project_root: root.clone(),
            plan: &plan,
            harness: &harness,
            max_concurrent: 2,
            step_timeout_secs: 30,
            use_worktrees: true, // 非 git 目录:worktree 失败 → 显式失败路径
            cancel: &cancel,
        };
        execute_ready_batch(&ctx, &mut tasks, &mut |_| {});
        // 非 git 仓库且需要隔离 → a 因 worktree 创建失败而 Failed(worktree
        // 路径);或者(单任务不隔离)FAIL_STEP 注入失败。二者必居其一,
        // 都不能是 Completed。
        let a = tasks.iter().find(|t| t.step_id == "a").unwrap();
        assert!(matches!(a.status, TaskStatus::Failed { .. }), "a 必须失败:{:?}", a.status);
        crate::council::tasks::finalize_skipped(&plan, &mut tasks);
        assert!(matches!(tasks.iter().find(|t| t.step_id == "b").unwrap().status, TaskStatus::Skipped { .. }));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn step_prompt_contains_plan_and_constraints() {
        let plan = doc(vec![step("s1", "做 X", vec![]), step("s2", "做 Y", vec!["s1"])]);
        let text = step_prompt(&plan, &plan.steps[1], true);
        assert!(text.contains("s2"));
        assert!(text.contains("隔离的 git worktree"));
        assert!(text.contains("不要执行计划中其他步骤"));
    }
}
