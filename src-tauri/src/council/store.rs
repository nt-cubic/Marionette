//! Council 持久化:`~/.marionette/council/runs/{run_id}/`。
//!
//! 布局(架构审计 §4):
//! - `spec.json`        不可变 run 配置
//! - `state.json`       可变快照(write_atomic)
//! - `messages.jsonl`   append-only,每行一条消息(恢复/UI 回放/审计)
//! - `transitions.jsonl` append-only 状态转移审计
//! - `plan.json`        PlanReady 后的结构化计划
//!
//! 单进程单写者(orchestrator 线程),append 用追加句柄;`message_id`
//! 幂等键在 append 入口拒绝重复(验收 #5:重复投递不重复入账)。

use super::types::*;
use crate::app_paths;
use std::fs::{self, OpenOptions};
use std::io::Write as IoWrite;
use std::path::{Path, PathBuf};

/// 测试沙箱:覆写 council 根目录,生产恒为 None(对齐 model_bridge 的
/// `FETCH_BASE_OVERRIDE` seam 模式)。cargo test 并行跑不碰真实
/// `~/.marionette`。
static ROOT_OVERRIDE: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

#[cfg(test)]
pub fn set_root_override_for_tests(path: Option<PathBuf>) {
    *ROOT_OVERRIDE.lock().unwrap() = path;
}

pub fn council_root() -> Result<PathBuf, String> {
    if let Some(dir) = ROOT_OVERRIDE.lock().unwrap().clone() {
        return Ok(dir);
    }
    Ok(app_paths::global_dir()?.join("council"))
}

pub fn runs_root() -> Result<PathBuf, String> {
    Ok(council_root()?.join("runs"))
}

pub fn run_dir(run_id: &str) -> Result<PathBuf, String> {
    // run_id 由我们生成(c{hex}-{seq}),但防御外部传入的路径注入。
    if run_id.is_empty()
        || !run_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("非法 run id:{run_id}"));
    }
    Ok(runs_root()?.join(run_id))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path, what: &str) -> Result<T, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("读取 {what} 失败:{e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("解析 {what} 失败:{e}"))
}

fn write_json_atomic(path: &Path, value: &impl serde::Serialize, what: &str) -> Result<(), String> {
    let body = serde_json::to_string_pretty(value).map_err(|e| format!("序列化 {what} 失败:{e}"))?;
    crate::storage::write_atomic(path, &body).map_err(|e| format!("写入 {what} 失败:{e}"))
}

// ---------------------------------------------------------------------------
// spec(不可变)
// ---------------------------------------------------------------------------

pub fn save_spec(spec: &CouncilSpec) -> Result<(), String> {
    let dir = run_dir(&spec.run_id)?;
    write_json_atomic(&dir.join("spec.json"), spec, "spec")
}

pub fn load_spec(run_id: &str) -> Result<CouncilSpec, String> {
    read_json(&run_dir(run_id)?.join("spec.json"), "spec")
}

// ---------------------------------------------------------------------------
// state(快照)
// ---------------------------------------------------------------------------

pub fn save_state(state: &CouncilState) -> Result<(), String> {
    let dir = run_dir(&state.run_id)?;
    write_json_atomic(&dir.join("state.json"), state, "state")
}

pub fn load_state(run_id: &str) -> Result<CouncilState, String> {
    read_json(&run_dir(run_id)?.join("state.json"), "state")
}

// ---------------------------------------------------------------------------
// messages / transitions(append-only JSONL)
// ---------------------------------------------------------------------------

fn append_jsonl(dir: &Path, file: &str, line: &str, what: &str) -> Result<(), String> {
    // 防御性建目录:正常路径 spec/state 先落盘已建好,但恢复/测试直接
    // append 时目录可能缺席(OpenOptions 的 create 只建文件不建目录)。
    std::fs::create_dir_all(dir).map_err(|e| format!("创建 run 目录失败:{e}"))?;
    let path = dir.join(file);
    let mut handle = OpenOptions::new().create(true).append(true).open(&path)
        .map_err(|e| format!("打开 {what} 失败:{e}"))?;
    writeln!(handle, "{line}").map_err(|e| format!("追加 {what} 失败:{e}"))
}

/// 追加一条消息。`message_id` 已存在 → `Err`(幂等键,重复投递在这里
/// 被挡住,不会产生重复状态)。
pub fn append_message(run_id: &str, message: &CouncilMessage) -> Result<(), String> {
    let dir = run_dir(run_id)?;
    if message_exists_at(&dir, &message.message_id)? {
        return Err(format!("消息 {} 已存在(重复投递被拒绝)", message.message_id));
    }
    let line = serde_json::to_string(message).map_err(|e| format!("序列化消息失败:{e}"))?;
    append_jsonl(&dir, "messages.jsonl", &line, "messages")
}

fn message_exists_at(dir: &Path, message_id: &str) -> Result<bool, String> {
    let path = dir.join("messages.jsonl");
    if !path.exists() {
        return Ok(false);
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("读取 messages 失败:{e}"))?;
    // 逐行解析只取 id;坏行跳过(对齐 storage.rs transcript 加载的容错)。
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if v.get("messageId").and_then(|id| id.as_str()) == Some(message_id) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub fn load_messages(run_id: &str) -> Result<Vec<CouncilMessage>, String> {
    let path = run_dir(run_id)?.join("messages.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("读取 messages 失败:{e}"))?;
    let mut out = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<CouncilMessage>(line) {
            Ok(m) => out.push(m),
            // 崩溃残留的半行(最后一行没写完整)静默丢弃,与恢复语义兼容。
            Err(_) => continue,
        }
    }
    Ok(out)
}

pub fn append_transition(run_id: &str, record: &TransitionRecord) -> Result<(), String> {
    let dir = run_dir(run_id)?;
    let line = serde_json::to_string(record).map_err(|e| format!("序列化转移失败:{e}"))?;
    append_jsonl(&dir, "transitions.jsonl", &line, "transitions")
}

pub fn load_transitions(run_id: &str) -> Result<Vec<TransitionRecord>, String> {
    let path = run_dir(run_id)?.join("transitions.jsonl");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path).map_err(|e| format!("读取 transitions 失败:{e}"))?;
    let mut out = Vec::new();
    for line in text.lines() {
        if let Ok(r) = serde_json::from_str::<TransitionRecord>(line) {
            out.push(r);
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// plan
// ---------------------------------------------------------------------------

pub fn save_plan(run_id: &str, plan: &PlanDoc) -> Result<(), String> {
    let dir = run_dir(run_id)?;
    write_json_atomic(&dir.join("plan.json"), plan, "plan")
}

pub fn load_plan(run_id: &str) -> Result<Option<PlanDoc>, String> {
    let path = run_dir(run_id)?.join("plan.json");
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(read_json(&path, "plan")?))
}

// ---------------------------------------------------------------------------
// run 列表 / 删除
// ---------------------------------------------------------------------------

pub fn list_runs() -> Result<Vec<RunSummary>, String> {
    let root = runs_root()?;
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(&root).map_err(|e| format!("扫描 runs 失败:{e}"))? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if !path.is_dir() || !path.join("spec.json").exists() {
            continue;
        }
        let run_id = match path.file_name().and_then(|n| n.to_str()) {
            Some(id) => id.to_string(),
            None => continue,
        };
        let spec = match load_spec(&run_id) {
            Ok(s) => s,
            Err(_) => continue, // 损坏的 run 不阻塞列表
        };
        let state = load_state(&run_id).ok();
        let messages = load_messages(&run_id).unwrap_or_default().len();
        out.push(RunSummary {
            run_id,
            title: spec.title,
            phase: state.as_ref().map(|s| s.phase).unwrap_or(RunPhase::Created),
            mode: spec.mode,
            member_count: spec.members.len(),
            rounds: spec.rounds,
            message_count: messages,
            created_at: spec.created_at,
            updated_at: state.as_ref().map(|s| s.updated_at).unwrap_or(spec.created_at),
        });
    }
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(out)
}

pub fn delete_run(run_id: &str) -> Result<(), String> {
    let dir = run_dir(run_id)?;
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| format!("删除 run 失败:{e}"))?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::council::types::tests::sample_spec;

    /// 每个测试一个独立沙箱目录(ROOT_OVERRIDE),结束不清理系统盘
    /// (temp 目录由系统回收)。串行化 ROOT_OVERRIDE 的取放:RAII guard。
    /// 并行测试会互踩全局 ROOT_OVERRIDE——全局 Mutex 把 Sandbox 的
    /// 生命周期(设置→测试体→清理)整体串行化,不影响其它模块测试。
    /// orchestrator 的测试沙箱共用同一把锁(同一个 override)。
    pub(crate) static SANDBOX_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct Sandbox(PathBuf, Option<std::sync::MutexGuard<'static, ()>>);

    impl Sandbox {
        fn new(tag: &str) -> Self {
            // 'static guard:SANDBOX_LOCK 是静态项,守卫可以安全延伸。
            let guard: std::sync::MutexGuard<'static, ()> = SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let dir = std::env::temp_dir().join(format!("marionette-council-test-{tag}-{}", new_run_id()));
            set_root_override_for_tests(Some(dir.clone()));
            Sandbox(dir, Some(guard))
        }
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            set_root_override_for_tests(None);
            let _ = fs::remove_dir_all(&self.0);
            drop(self.1.take());
        }
    }

    fn temp_run() -> (CouncilSpec, CouncilState) {
        let mut spec = sample_spec();
        spec.run_id = format!("test-{}", new_run_id());
        let state = CouncilState {
            run_id: spec.run_id.clone(),
            phase: RunPhase::Created,
            round: 1,
            paused_from: None,
            member_status: spec.members.iter().map(|m| (m.id.clone(), MemberRuntimeStatus::Idle)).collect(),
            budget_usage: Default::default(),
            tasks: Vec::new(),
            verify_attempts: 0,
            updated_at: now_millis(),
            error: None,
        };
        (spec, state)
    }

    fn cleanup(run_id: &str) {
        let _ = delete_run(run_id);
    }

    #[test]
    fn spec_state_round_trip() {
        let _sandbox = Sandbox::new("round-trip");
        let (spec, state) = temp_run();
        save_spec(&spec).unwrap();
        save_state(&state).unwrap();
        let loaded_spec = load_spec(&spec.run_id).unwrap();
        assert_eq!(loaded_spec.run_id, spec.run_id);
        assert_eq!(loaded_spec.members.len(), 3);
        let loaded_state = load_state(&spec.run_id).unwrap();
        assert_eq!(loaded_state.phase, RunPhase::Created);
        cleanup(&spec.run_id);
    }

    #[test]
    fn message_append_and_dedup() {
        let _sandbox = Sandbox::new("t0");
        let (spec, _state) = temp_run();
        let msg = CouncilMessage {
            message_id: "m-1".into(),
            run_id: spec.run_id.clone(),
            round: 1,
            from: "glm-architect".into(),
            to: "all".into(),
            kind: MessageKind::Proposal,
            reply_to: None,
            content: "方案A".into(),
            timestamp: now_millis(),
            meta: None,
        };
        append_message(&spec.run_id, &msg).unwrap();
        // 同 id 再 append 被拒(幂等键)。
        assert!(append_message(&spec.run_id, &msg).is_err());
        let messages = load_messages(&spec.run_id).unwrap();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].kind, MessageKind::Proposal);
        cleanup(&spec.run_id);
    }

    #[test]
    fn transitions_append_only() {
        let _sandbox = Sandbox::new("t1");
        let (spec, _state) = temp_run();
        let rec = TransitionRecord { at: now_millis(), from: RunPhase::Created, to: RunPhase::Analyzing, reason: None };
        append_transition(&spec.run_id, &rec).unwrap();
        append_transition(&spec.run_id, &rec).unwrap(); // 审计日志允许同记录出现两次(真实时间戳不同)
        assert_eq!(load_transitions(&spec.run_id).unwrap().len(), 2);
        cleanup(&spec.run_id);
    }

    #[test]
    fn plan_round_trip_optional() {
        let _sandbox = Sandbox::new("t2");
        let (spec, _state) = temp_run();
        assert!(load_plan(&spec.run_id).unwrap().is_none());
        let plan = PlanDoc {
            summary: "s".into(),
            steps: vec![PlanStep { id: "s1".into(), title: "t".into(), detail: "d".into(), depends_on: vec![], acceptance: "a".into(), owner_hint: None }],
            open_questions: vec!["q".into()],
            acceptance: vec![],
            verification: vec!["cargo test".into()],
        };
        save_plan(&spec.run_id, &plan).unwrap();
        let loaded = load_plan(&spec.run_id).unwrap().unwrap();
        assert_eq!(loaded.steps.len(), 1);
        assert_eq!(loaded.verification[0], "cargo test");
        cleanup(&spec.run_id);
    }

    #[test]
    fn run_id_rejects_path_injection() {
        assert!(run_dir("../escape").is_err());
        assert!(run_dir("a/b").is_err());
        assert!(run_dir("").is_err());
        assert!(run_dir("c123-0").is_ok());
    }
}
