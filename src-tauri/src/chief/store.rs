//! Chief 本地账本：config.json / tasks.json / events.jsonl。
//!
//! 布局（02-architecture.md §9）：
//!
//! ```text
//! ~/.marionette/chief/
//!   config.json    Chief 配置与允许项目（无凭据）
//!   tasks.json     任务快照（原子写，带 version）
//!   events.jsonl   全量 Host 事件（append-only）
//! ```
//!
//! 纪律：
//! - 快照与事件都是**本地真相**；Telegram 消息不是事实源。
//! - tasks.json 坏档 fail-open 成空表 + dev.log warn（事件日志仍在，
//!   可人工对账）；与 memos index 同姿态。
//! - 写全部走 [`crate::storage::write_atomic`]（进程级 .tmp 写锁 + 重试）。
//! - 测试沙箱：[`set_root_override_for_tests`] + [`SANDBOX_LOCK`]（对齐
//!   council store 模式——broker 测试与 store 测试共用一把锁）。

use super::types::{now_ms, ChiefConfig, Task, TaskEvent, TaskStatus};
use serde_json::{json, Value};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Mutex;

static ROOT_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);
/// chief 账本的单写者锁（config / tasks / events 都走它）。
static STORE_LOCK: Mutex<()> = Mutex::new(());
/// 测试串行锁：store 与 broker 的测试共用（进程级静态，override 互踩）。
#[cfg(test)]
pub(crate) static SANDBOX_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
pub fn set_root_override_for_tests(path: Option<PathBuf>) {
    if let Ok(mut slot) = ROOT_OVERRIDE.lock() {
        *slot = path;
    }
}

fn chief_root() -> Result<PathBuf, String> {
    if let Ok(Some(dir)) = ROOT_OVERRIDE.lock().map(|slot| slot.clone()) {
        return Ok(dir);
    }
    crate::app_paths::global_dir().map(|dir| dir.join("chief"))
}

fn config_path() -> Result<PathBuf, String> {
    chief_root().map(|dir| dir.join("config.json"))
}

fn tasks_path() -> Result<PathBuf, String> {
    chief_root().map(|dir| dir.join("tasks.json"))
}

fn events_path() -> Result<PathBuf, String> {
    chief_root().map(|dir| dir.join("events.jsonl"))
}

fn warn(scope: &str, message: &str, detail: Option<&str>) {
    crate::debug_log::append(scope, "warn", "", message, detail);
}

// ── config ────────────────────────────────────────────────────────────────

/// 读 Chief 配置。坏档 / 无档 fail-open 成默认（mode=off，绝不带凭据）。
pub fn load_config() -> ChiefConfig {
    let _guard = STORE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let path = match config_path() {
        Ok(path) => path,
        Err(_) => return ChiefConfig::default(),
    };
    let config = match std::fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str::<ChiefConfig>(&raw) {
            Ok(config) => config,
            Err(err) => {
                warn("chief", "chief config unreadable — using defaults", Some(&err.to_string()));
                ChiefConfig::default()
            }
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => ChiefConfig::default(),
        Err(err) => {
            warn("chief", "chief config read failed — using defaults", Some(&err.to_string()));
            ChiefConfig::default()
        }
    };
    config
}

pub fn save_config(config: &ChiefConfig) -> Result<(), String> {
    let _guard = STORE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let path = config_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("chief: mkdir failed: {e}"))?;
    }
    let body = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    crate::storage::write_atomic(&path, &body)
}

// ── tasks snapshot ────────────────────────────────────────────────────────

fn decode_tasks(raw: &str) -> Vec<Task> {
    // 顶层既接受 `{version, tasks:[…]}`，也兜底接受裸数组（手改兼容）。
    let parsed: Value = match serde_json::from_str(raw) {
        Ok(value) => value,
        Err(err) => {
            warn("chief", "tasks.json unreadable — starting empty", Some(&err.to_string()));
            return Vec::new();
        }
    };
    let list = if parsed.is_array() {
        parsed
    } else {
        parsed.get("tasks").cloned().unwrap_or(Value::Array(Vec::new()))
    };
    match serde_json::from_value::<Vec<Task>>(list) {
        Ok(tasks) => tasks,
        Err(err) => {
            warn("chief", "tasks.json entries unreadable — starting empty", Some(&err.to_string()));
            Vec::new()
        }
    }
}

/// 读任务快照（无锁；调用方在 [`with_tasks`] 里用，或只读场景直接用）。
pub fn load_tasks() -> Vec<Task> {
    let path = match tasks_path() {
        Ok(path) => path,
        Err(_) => return Vec::new(),
    };
    match std::fs::read_to_string(&path) {
        Ok(raw) => decode_tasks(&raw),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(err) => {
            warn("chief", "tasks.json read failed — starting empty", Some(&err.to_string()));
            Vec::new()
        }
    }
}

fn encode_tasks(tasks: &[Task]) -> String {
    serde_json::to_string_pretty(&json!({
        "version": 1,
        "tasks": tasks,
    }))
    .unwrap_or_else(|_| "[]".to_string())
}

/// 锁内读-改-写任务快照。转移/幂等检查全部应经由这里，保证单写者。
pub fn with_tasks<T>(f: impl FnOnce(&mut Vec<Task>) -> Result<T, String>) -> Result<T, String> {
    let _guard = STORE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut tasks = load_tasks();
    let result = f(&mut tasks)?;
    save_tasks(&tasks)?;
    Ok(result)
}

pub fn save_tasks(tasks: &[Task]) -> Result<(), String> {
    let path = tasks_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("chief: mkdir failed: {e}"))?;
    }
    crate::storage::write_atomic(&path, &encode_tasks(tasks))
}

#[cfg(test)]
pub fn find_by_request_id(request_id: &str) -> Option<Task> {
    load_tasks()
        .into_iter()
        .find(|task| task.request_id == request_id)
}

pub fn find_task(task_id: &str) -> Option<Task> {
    load_tasks().into_iter().find(|task| task.task_id == task_id)
}

/// 启动恢复：所有非终态任务标 `interrupted`（02-architecture.md §10.1：
/// 没有活进程的任务标 interrupted，绝不标 completed）。返回被改写的任务。
/// 注意：`append_events` 与 `with_tasks` 共用一把非重入锁，事件必须在
/// 状态转移的锁**释放后**补写。
pub fn mark_interrupted_on_startup() -> Result<Vec<Task>, String> {
    let marked = with_tasks(|tasks| {
        let mut marked = Vec::new();
        let ts = now_ms();
        for task in tasks.iter_mut() {
            if matches!(
                task.status,
                TaskStatus::Starting | TaskStatus::Running | TaskStatus::WaitingUser
            ) {
                task.status = TaskStatus::Interrupted;
                task.updated_at = ts;
                task.last_event_at = ts;
                task.last_summary =
                    Some("应用重启，worker 失联 — 可用 task_resume 继续".to_string());
                marked.push(task.clone());
            }
        }
        Ok(marked)
    })?;
    if marked.is_empty() {
        return Ok(marked);
    }
    let ts = now_ms();
    let events: Vec<TaskEvent> = marked
        .iter()
        .map(|task| {
            let seq = load_events_for_task(&task.task_id).len() as u64 + 1;
            TaskEvent {
                event_id: super::types::short_id("evt"),
                seq,
                task_id: task.task_id.clone(),
                kind: "task.interrupted".into(),
                status: Some(TaskStatus::Interrupted),
                summary: task.last_summary.clone(),
                delivered_at: None,
                ts,
            }
        })
        .collect();
    append_events(&events)?;
    Ok(marked)
}

// ── events ledger ─────────────────────────────────────────────────────────

/// 追加事件行（单写者锁内）。事件先落盘再谈副作用（ccteam 硬原则 3）。
pub fn append_events(events: &[TaskEvent]) -> Result<(), String> {
    if events.is_empty() {
        return Ok(());
    }
    let _guard = STORE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let path = events_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("chief: mkdir failed: {e}"))?;
    }
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("chief: open events failed: {e}"))?;
    let mut body = String::new();
    for event in events {
        let line = serde_json::to_string(event).map_err(|e| e.to_string())?;
        body.push_str(&line);
        body.push('\n');
    }
    file.write_all(body.as_bytes())
        .map_err(|e| format!("chief: append events failed: {e}"))?;
    file.flush().ok();
    Ok(())
}

/// 读某任务的事件（坏行跳过）。
pub fn load_events_for_task(task_id: &str) -> Vec<TaskEvent> {
    let path = match events_path() {
        Ok(path) => path,
        Err(_) => return Vec::new(),
    };
    let Ok(file) = std::fs::File::open(&path) else {
        return Vec::new();
    };
    let mut events = Vec::new();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<TaskEvent>(&line) else {
            continue;
        };
        if value.task_id == task_id {
            events.push(value);
        }
    }
    events
}

/// 读全部事件（唤醒队列重建 / 审计用）。
#[cfg(test)]
pub fn load_all_events() -> Vec<TaskEvent> {
    let path = match events_path() {
        Ok(path) => path,
        Err(_) => return Vec::new(),
    };
    let Ok(file) = std::fs::File::open(&path) else {
        return Vec::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<TaskEvent>(&line).ok())
        .collect()
}

/// 事件回填送达时间（唤醒消费后调用；幂等，只填 None）。
pub fn mark_events_delivered(event_ids: &[String], delivered_at: u64) -> Result<(), String> {
    if event_ids.is_empty() {
        return Ok(());
    }
    let _guard = STORE_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let path = events_path()?;
    let Ok(content) = std::fs::read_to_string(&path) else {
        return Ok(());
    };
    let mut out = String::with_capacity(content.len());
    for line in content.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<TaskEvent>(line) {
            Ok(mut event) => {
                if event_ids.iter().any(|id| id == &event.event_id) && event.delivered_at.is_none()
                {
                    event.delivered_at = Some(delivered_at);
                    out.push_str(&serde_json::to_string(&event).map_err(|e| e.to_string())?);
                } else {
                    out.push_str(line);
                }
                out.push('\n');
            }
            Err(_) => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    crate::storage::write_atomic(&path, &out)
}

#[cfg(test)]
mod tests {
    use super::super::types::{short_id, TaskStatus};
    use super::*;
    use std::path::PathBuf;

    fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "chief-store-test-{}-{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn lock_sandbox() -> std::sync::MutexGuard<'static, ()> {
        SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn sample_task(project: &str) -> Task {
        Task {
            task_id: short_id("task"),
            request_id: short_id("req"),
            chief_session_id: "session_chief".into(),
            title: "修复构建".into(),
            goal: "goal text".into(),
            project_id: project.into(),
            agent_id: "codex".into(),
            status: TaskStatus::Proposed,
            ..Default::default()
        }
    }

    #[test]
    fn config_roundtrip_and_fail_open() {
        let guard = lock_sandbox();
        let root = sandbox("config");
        set_root_override_for_tests(Some(root.clone()));
        save_config(&ChiefConfig {
            mode: super::super::types::RemoteWorkMode::Chief,
            chief_agent_id: Some("codex".into()),
            max_concurrent_workers: 2,
            ..Default::default()
        })
        .unwrap();
        let loaded = load_config();
        assert_eq!(
            loaded.mode,
            super::super::types::RemoteWorkMode::Chief
        );
        assert_eq!(loaded.max_concurrent_workers, 2);
        // 坏档 fail-open
        std::fs::write(root.join("config.json"), "{not json").unwrap();
        assert_eq!(load_config().mode, super::super::types::RemoteWorkMode::Off);
        set_root_override_for_tests(None);
        drop(guard);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn tasks_roundtrip_request_id_idempotency() {
        let guard = lock_sandbox();
        let root = sandbox("tasks");
        set_root_override_for_tests(Some(root.clone()));
        let task = sample_task("project_a");
        let request_id = task.request_id.clone();
        with_tasks(|tasks| {
            tasks.push(task.clone());
            Ok(())
        })
        .unwrap();
        let found = find_by_request_id(&request_id).expect("request id lookup");
        assert_eq!(found.task_id, task.task_id);
        assert!(find_by_request_id("req_missing").is_none());
        // 裸数组兜底
        std::fs::write(
            root.join("tasks.json"),
            serde_json::to_string(&vec![sample_task("b")]).unwrap(),
        )
        .unwrap();
        assert_eq!(load_tasks().len(), 1);
        // 坏档 fail-open
        std::fs::write(root.join("tasks.json"), "nope").unwrap();
        assert!(load_tasks().is_empty());
        set_root_override_for_tests(None);
        drop(guard);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn startup_marks_running_tasks_interrupted() {
        let guard = lock_sandbox();
        let root = sandbox("recover");
        set_root_override_for_tests(Some(root.clone()));
        let mut running = sample_task("project_a");
        running.status = TaskStatus::Running;
        let mut waiting = sample_task("project_b");
        waiting.status = TaskStatus::WaitingUser;
        let mut done = sample_task("project_c");
        done.status = TaskStatus::Completed;
        with_tasks(|tasks| {
            tasks.extend([running.clone(), waiting.clone(), done.clone()]);
            Ok(())
        })
        .unwrap();
        let marked = mark_interrupted_on_startup().unwrap();
        assert_eq!(marked.len(), 2);
        assert!(marked.iter().all(|t| t.status == TaskStatus::Interrupted));
        let statuses: Vec<TaskStatus> = load_tasks()
            .into_iter()
            .filter(|t| t.task_id == running.task_id || t.task_id == waiting.task_id)
            .map(|t| t.status)
            .collect();
        assert_eq!(statuses, vec![TaskStatus::Interrupted, TaskStatus::Interrupted]);
        // completed 不被碰
        let done_now = find_task(&done.task_id).unwrap();
        assert_eq!(done_now.status, TaskStatus::Completed);
        // 事件账本收到 task.interrupted
        let events = load_events_for_task(&running.task_id);
        assert!(events.iter().any(|e| e.kind == "task.interrupted"));
        set_root_override_for_tests(None);
        drop(guard);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn events_append_filter_and_delivery_mark() {
        let guard = lock_sandbox();
        let root = sandbox("events");
        set_root_override_for_tests(Some(root.clone()));
        let mut event_a = TaskEvent {
            event_id: short_id("evt"),
            seq: 1,
            task_id: "task_a".into(),
            kind: "task.started".into(),
            status: Some(TaskStatus::Running),
            summary: None,
            delivered_at: None,
            ts: now_ms(),
        };
        append_events(&[event_a.clone()]).unwrap();
        let event_b = TaskEvent {
            event_id: short_id("evt"),
            seq: 2,
            task_id: "task_b".into(),
            kind: "task.completed".into(),
            status: Some(TaskStatus::Completed),
            summary: Some("done".into()),
            delivered_at: None,
            ts: now_ms(),
        };
        append_events(&[event_b]).unwrap();
        assert_eq!(load_events_for_task("task_a").len(), 1);
        assert_eq!(load_events_for_task("task_b").len(), 1);
        assert_eq!(load_all_events().len(), 2);
        // 送达回填：只改目标事件，幂等
        mark_events_delivered(&[event_a.event_id.clone()], 123).unwrap();
        let back = load_events_for_task("task_a");
        assert_eq!(back[0].delivered_at, Some(123));
        mark_events_delivered(&[event_a.event_id.clone()], 999).unwrap();
        let again = load_events_for_task("task_a");
        assert_eq!(again[0].delivered_at, Some(123), "幂等：只填 None");
        let _ = &mut event_a;
        set_root_override_for_tests(None);
        drop(guard);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn sandbox_does_not_touch_real_chief_dir() {
        // override 为 None 时路径指向真实全局目录；测试里绝不走到写路径，
        // 这里只断言 override 生效（读测试沙箱）。
        let guard = lock_sandbox();
        let root = sandbox("pathing");
        set_root_override_for_tests(Some(root.clone()));
        let path = config_path().unwrap();
        assert!(path.starts_with(&root));
        assert!(path.ends_with("config.json"));
        set_root_override_for_tests(None);
        drop(guard);
        std::fs::remove_dir_all(&root).ok();
    }
}
