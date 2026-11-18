//! Chief 编排核心：任务生命周期、Chief 会话注册表、唤醒收件箱、心跳。
//!
//! 红线（docs/research/telegram-chief/）：
//! - `task_start` **立即返回 taskId**，spawn 在后台线程（MCP call 不挂几十分钟）；
//! - 唤醒按事件合并（completed/failed/interrupted 才唤醒 Chief，逐条
//!   token 不唤醒），Chief 忙时进收件箱，心跳在空闲时统一 flush；
//! - 同一 Chief 默认 1 个 active worker（配置上限 3）；超限明确拒绝，
//!   不悄悄丢弃；
//! - 终态无出边；cancelled 幂等；interrupted 才能显式 resume；
//! - worker 是普通会话（origin="chief-worker"，顶层可见），关系只进
//!   任务账本，绝不写 `parent_session_id`。

use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Manager};

use super::policy::{self, Decision};
use super::runner;
use super::staff;
use super::staff_commands;
use super::store;
use super::tee;
use super::types::{now_ms, short_id, RemoteMode, RemoteWorkMode, Task, TaskEvent, TaskStatus};

// ── Chief 会话注册表（MCP 门禁的唯一事实） ────────────────────────────────

static CHIEF_SESSIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn chief_sessions() -> &'static Mutex<HashSet<String>> {
    CHIEF_SESSIONS.get_or_init(|| Mutex::new(HashSet::new()))
}

/// 注册一个 Chief 会话（创建/发现 chief 会话时调用）。MCP 的
/// session-aware tools/list 与 dispatch 门禁都以这里为准。
pub fn register_chief_session(session_id: &str) {
    if let Ok(mut set) = chief_sessions().lock() {
        set.insert(session_id.to_string());
    }
}

pub fn unregister_chief_session(session_id: &str) {
    if let Ok(mut set) = chief_sessions().lock() {
        set.remove(session_id);
    }
}

pub fn forget_session_runtime(session_id: &str) {
    unregister_chief_session(session_id);
    if let Ok(mut inbox) = wake_inbox().lock() {
        inbox.remove(session_id);
    }
    if let Ok(mut queues) = user_queues().lock() {
        queues.remove(session_id);
    }
    if let Ok(mut queues) = host_queues().lock() {
        queues.remove(session_id);
    }
    if let Ok(mut queues) = bootstrap_queues().lock() {
        queues.remove(session_id);
    }
}

/// 该会话是否是 Chief：运行时注册表 ∪ config.chief_session_id。
/// 这是控制工具可见性的**唯一**判据；普通 worker 永远不在这里。
pub fn is_chief_session(session_id: &str) -> bool {
    if let Ok(set) = chief_sessions().lock() {
        if set.contains(session_id) {
            return true;
        }
    }
    let staff_registry = staff::load();
    if staff_registry.is_staff_session(session_id) {
        return true;
    }
    // Once the staff registry has completed its one-time migration, the old
    // singular config field is historical data. A dismissed final employee
    // must not keep control-tool access alive on the old chat row.
    if staff_registry.legacy_migration_completed_at > 0 {
        return false;
    }
    store::load_config()
        .chief_session_id
        .as_deref()
        .map(|sid| sid == session_id)
        .unwrap_or(false)
}

// ── 唤醒收件箱 与 用户消息队列（chief 忙时排队） ─────────────────────────

static WAKE_INBOX: OnceLock<Mutex<HashMap<String, Vec<TaskEvent>>>> = OnceLock::new();
static USER_QUEUE: OnceLock<Mutex<HashMap<String, VecDeque<String>>>> = OnceLock::new();
static HOST_QUEUE: OnceLock<Mutex<HashMap<String, VecDeque<String>>>> = OnceLock::new();
static BOOTSTRAP_QUEUE: OnceLock<Mutex<HashMap<String, VecDeque<String>>>> = OnceLock::new();
static QUEUED_COMMITMENTS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
static WORKER_PROGRESS_WRITES: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();

fn wake_inbox() -> &'static Mutex<HashMap<String, Vec<TaskEvent>>> {
    WAKE_INBOX.get_or_init(|| Mutex::new(HashMap::new()))
}

fn user_queues() -> &'static Mutex<HashMap<String, VecDeque<String>>> {
    USER_QUEUE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn host_queues() -> &'static Mutex<HashMap<String, VecDeque<String>>> {
    HOST_QUEUE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn bootstrap_queues() -> &'static Mutex<HashMap<String, VecDeque<String>>> {
    BOOTSTRAP_QUEUE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn claim_due_commitment(commitment_id: &str) -> bool {
    let claims = QUEUED_COMMITMENTS.get_or_init(|| Mutex::new(HashSet::new()));
    claims
        .lock()
        .map(|mut ids| ids.insert(commitment_id.to_string()))
        .unwrap_or(false)
}

fn touch_worker_progress(session_id: &str) {
    const THROTTLE_MS: u64 = 5_000;
    let now = now_ms();
    let writes = WORKER_PROGRESS_WRITES.get_or_init(|| Mutex::new(HashMap::new()));
    let should_write = writes
        .lock()
        .map(|mut last| {
            if last
                .get(session_id)
                .is_some_and(|previous| now.saturating_sub(*previous) < THROTTLE_MS)
            {
                return false;
            }
            last.insert(session_id.to_string(), now);
            true
        })
        .unwrap_or(false);
    if !should_write {
        return;
    }
    // 热路径守卫：emit_now 对**所有**会话的每个事件都会走到这里。普通会话
    // 不是 worker，不能为它们每 5s 抢 STORE_LOCK 并整档重写 tasks.json
    // （with_tasks 恒落盘）。先无锁只读判定，确有进行中的 worker 任务才写。
    let is_active_worker = store::load_tasks().iter().any(|task| {
        task.worker_session_id.as_deref() == Some(session_id)
            && matches!(task.status, TaskStatus::Starting | TaskStatus::Running)
    });
    if !is_active_worker {
        return;
    }
    let _ = store::with_tasks(|tasks| {
        if let Some(task) = tasks.iter_mut().find(|task| {
            task.worker_session_id.as_deref() == Some(session_id)
                && matches!(task.status, TaskStatus::Starting | TaskStatus::Running)
        }) {
            task.last_event_at = now;
            task.updated_at = now;
        }
        Ok(())
    });
}

/// 把一个 Host 事件放进 Chief 的收件箱（等空闲统一 flush）。
pub fn queue_wake(chief_session_id: &str, event: TaskEvent) {
    if let Ok(mut inbox) = wake_inbox().lock() {
        inbox
            .entry(chief_session_id.to_string())
            .or_default()
            .push(event);
    }
}

pub(crate) fn pending_wake_count(chief_session_id: &str) -> usize {
    wake_inbox()
        .lock()
        .ok()
        .and_then(|inbox| inbox.get(chief_session_id).map(|v| v.len()))
        .unwrap_or(0)
}

/// 用户消息排队（Chief 正在回合中）。FIFO，心跳在空闲时逐条 flush。
pub fn queue_user_text(chief_session_id: &str, text: &str) {
    if let Ok(mut queues) = user_queues().lock() {
        queues
            .entry(chief_session_id.to_string())
            .or_default()
            .push_back(text.to_string());
    }
}

/// Host-originated prompt (bootstrap, commitment due, scheduler). Persistent
/// pause suppresses these while real user messages may still reach the agent.
pub fn queue_host_text(chief_session_id: &str, text: &str) {
    if let Ok(mut queues) = host_queues().lock() {
        queues
            .entry(chief_session_id.to_string())
            .or_default()
            .push_back(text.to_string());
    }
}

/// Bootstrap must be the first prompt in a newly-created staff session.
pub fn queue_bootstrap_text(chief_session_id: &str, text: &str) {
    if let Ok(mut queues) = bootstrap_queues().lock() {
        let queue = queues
            .entry(chief_session_id.to_string())
            .or_default();
        queue.clear();
        queue.push_back(text.to_string());
    }
}

pub(crate) fn pending_user_count(chief_session_id: &str) -> usize {
    user_queues()
        .lock()
        .ok()
        .and_then(|queues| queues.get(chief_session_id).map(|q| q.len()))
        .unwrap_or(0)
        + host_queues()
            .lock()
            .ok()
            .and_then(|queues| queues.get(chief_session_id).map(|q| q.len()))
            .unwrap_or(0)
        + bootstrap_queues()
            .lock()
            .ok()
            .and_then(|queues| queues.get(chief_session_id).map(|q| q.len()))
            .unwrap_or(0)
}

/// `task_send` 的 (taskId, commandId) 去重（内存 LRU；传输可重复，业务幂等）。
static SENT_COMMANDS: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();
const SENT_COMMANDS_CAP: usize = 1024;

fn sent_commands() -> &'static Mutex<VecDeque<String>> {
    SENT_COMMANDS.get_or_init(|| Mutex::new(VecDeque::new()))
}

/// true = 首次见到（应执行）；false = 重复（幂等跳过）。
pub(crate) fn mark_sent_command(task_id: &str, command_id: &str) -> bool {
    let key = format!("{task_id}:{command_id}");
    let mut guard = sent_commands().lock().unwrap_or_else(|p| p.into_inner());
    if guard.iter().any(|seen| seen == &key) {
        return false;
    }
    guard.push_back(key);
    while guard.len() > SENT_COMMANDS_CAP {
        guard.pop_front();
    }
    true
}

fn sent_command_seen(task_id: &str, command_id: &str) -> bool {
    let key = format!("{task_id}:{command_id}");
    sent_commands()
        .lock()
        .map(|guard| guard.iter().any(|seen| seen == &key))
        .unwrap_or(false)
}

fn unmark_sent_command(task_id: &str, command_id: &str) {
    let key = format!("{task_id}:{command_id}");
    if let Ok(mut guard) = sent_commands().lock() {
        guard.retain(|seen| seen != &key);
    }
}

fn pop_user_text(chief_session_id: &str) -> Option<String> {
    let mut queues = user_queues().lock().ok()?;
    queues
        .get_mut(chief_session_id)
        .and_then(|queue| queue.pop_front())
}

fn pop_host_text(chief_session_id: &str) -> Option<String> {
    let mut queues = host_queues().lock().ok()?;
    queues
        .get_mut(chief_session_id)
        .and_then(|queue| queue.pop_front())
}

fn pop_bootstrap_text(chief_session_id: &str) -> Option<String> {
    let mut queues = bootstrap_queues().lock().ok()?;
    queues
        .get_mut(chief_session_id)
        .and_then(|queue| queue.pop_front())
}

// ── 通用工具 ─────────────────────────────────────────────────────────────

fn envelope_ok(extra: Value) -> Value {
    let mut object = json!({ "ok": true });
    if let (Some(base), Some(add)) = (object.as_object_mut(), extra.as_object()) {
        for (key, value) in add {
            base.insert(key.clone(), value.clone());
        }
    }
    object
}

fn envelope_reject(error: &str, note: &str) -> Value {
    json!({ "ok": false, "error": error, "note": note })
}

/// storage 锁内执行（只做短操作；绝不在这里等子进程——storage 锁纪律，
/// 见 AGENTS「内存压力韧性批」第 0 条）。
fn with_storage<T>(app: &AppHandle, f: impl FnOnce(&crate::storage::StorageService) -> T) -> T {
    let state = app.state::<crate::AppState>();
    let guard = state.storage.lock().unwrap_or_else(|p| p.into_inner());
    f(&guard)
}

fn known_enabled_agents() -> Vec<String> {
    crate::custom_agents::all_agents_merged()
        .into_iter()
        .filter(|agent| agent.enabled)
        .map(|agent| agent.id)
        .collect()
}

/// 从 tee 事件流折出 worker 的可读产出。纯函数（broker 测试直接喂事件）。
/// - summary = 全部 assistant chunk 文本的尾部 400 字符；
/// - files = tool_call 输入里 path/file/file_path/absolutePath 的去重前 20 个；
/// - 返回 (summary, tool_calls, files, tool_failures)。
pub(crate) fn summarize_worker_events(events: &[Value]) -> (Option<String>, u64, Vec<String>, u64) {
    use std::collections::BTreeSet;
    let mut text = String::new();
    let mut tool_calls = 0u64;
    let mut tool_failures = 0u64;
    let mut files: Vec<String> = Vec::new();
    for event in events {
        let method = event.get("method").and_then(Value::as_str).unwrap_or("");
        if method != "session/update" {
            continue;
        }
        let Some(update) = event.pointer("/data/params/update") else {
            continue;
        };
        match update.get("sessionUpdate").and_then(Value::as_str).unwrap_or("") {
            "agent_message_chunk" => {
                if let Some(part) = update.pointer("/content/text").and_then(Value::as_str) {
                    text.push_str(part);
                }
            }
            "tool_call" => {
                tool_calls += 1;
                for key in ["path", "file", "file_path", "absolutePath"] {
                    if let Some(path) = update
                        .pointer("/rawInput")
                        .and_then(|input| input.get(key))
                        .and_then(Value::as_str)
                    {
                        files.push(path.to_string());
                    }
                }
            }
            "tool_call_update" => {
                let status = update.get("status").and_then(Value::as_str).unwrap_or("");
                if matches!(status, "failed" | "error") {
                    tool_failures += 1;
                }
            }
            _ => {}
        }
    }
    let mut unique = BTreeSet::new();
    let files: Vec<String> = files
        .into_iter()
        .filter(|path| unique.insert(path.clone()))
        .take(20)
        .collect();
    let summary = if text.is_empty() {
        None
    } else {
        let chars: Vec<char> = text.chars().collect();
        let start = chars.len().saturating_sub(400);
        Some(chars[start..].iter().collect::<String>())
    };
    (summary, tool_calls, files, tool_failures)
}

fn worker_reported_cost_usd(worker_session_id: &str) -> f64 {
    crate::usage_store::read_session(worker_session_id)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
        .filter_map(|row| {
            row.get("usd")
                .and_then(Value::as_f64)
                .or_else(|| row.get("reportedUsd").and_then(Value::as_f64))
        })
        .filter(|cost| cost.is_finite() && *cost > 0.0)
        .sum()
}

fn worker_reported_model_calls(worker_session_id: &str) -> u32 {
    crate::usage_store::read_session(worker_session_id)
        .unwrap_or_default()
        .into_iter()
        .filter(|line| serde_json::from_str::<Value>(line).is_ok())
        .count()
        .min(u32::MAX as usize) as u32
}

fn host_event_text(event: &TaskEvent, task: Option<&Task>) -> String {
    let mut text = format!(
        "[HOST EVENT: {}]\ntaskId: {}\nstatus: {}\n",
        event.kind,
        event.task_id,
        event
            .status
            .map(|s| format!("{s:?}").to_lowercase())
            .unwrap_or_else(|| "unknown".to_string())
    );
    if let Some(summary) = event.summary.as_deref().filter(|s| !s.is_empty()) {
        text.push_str(&format!("summary: {summary}\n"));
    }
    if let Some(task) = task {
        if !task.artifacts.is_empty() {
            text.push_str(&format!("artifacts: {}\n", task.artifacts.join("; ")));
        }
    }
    text.push_str("untrustedContent: false\n");
    text
}

fn emit_event(task: &Task, kind: &str, status: TaskStatus, summary: Option<String>) -> TaskEvent {
    let seq = store::load_events_for_task(&task.task_id).len() as u64 + 1;
    TaskEvent {
        event_id: short_id("evt"),
        seq,
        task_id: task.task_id.clone(),
        kind: kind.to_string(),
        status: Some(status),
        summary,
        delivered_at: None,
        ts: now_ms(),
    }
}

fn notify(task: &Task, kind: &str, summary: Option<&str>) {
    crate::telegram_remote::chief_task_notify(task, kind, summary);
}

// ── Chief 会话引导 ───────────────────────────────────────────────────────

static BOOTSTRAP: Mutex<()> = Mutex::new(());

/// 确保存在 chief 会话（有则复用，无则建）。返回 (sessionId, 是否新建)。
/// 双重检查在 BOOTSTRAP 锁内完成，两个入口并发不会建出两个 Chief。
fn ensure_chief_session(app: &AppHandle) -> Result<(String, bool), String> {
    let _guard = BOOTSTRAP.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(profile) = staff_commands::default_staff() {
        let sid = staff_commands::chief_staff_ensure_session(app.clone(), profile.staff_id)?;
        register_chief_session(&sid);
        let _ = tee::register(&sid);
        return Ok((sid, false));
    }
    let migrated_registry = staff::load();
    if migrated_registry.legacy_migration_completed_at > 0 {
        return Err("还没有员工。请先在本地员工组雇一位员工。".to_string());
    }
    let mut config = store::load_config();
    if let Some(sid) = config.chief_session_id.clone() {
        let exists = with_storage(app, |storage| {
            storage.find_session(&sid).ok().flatten().is_some()
        });
        if exists {
            register_chief_session(&sid);
            // 复用的旧 Chief 会话也要补挂 tee（幂等）——修复前创建的会话
            // 没有账本，本地打开时回填不到 Chief 自己的回复。
            let _ = tee::register(&sid);
            return Ok((sid, false));
        }
        // 会话被删了：清掉引用走新建。显式重建，绝不静默把旧聊天接到新会话。
        config.chief_session_id = None;
    }
    let agent_id = config
        .chief_agent_id
        .clone()
        .or_else(|| known_enabled_agents().first().cloned())
        .ok_or_else(|| "没有可用 Agent——请先在本地安装/启用一个 ACP Agent".to_string())?;
    let cwd = crate::app_paths::global_dir()?;
    let session = with_storage(app, |storage| {
        storage.create_chat_session(
            agent_id.clone(),
            "Chief".to_string(),
            &cwd.to_string_lossy(),
            Some("chief"),
        )
    })?;
    config.chief_session_id = Some(session.id.clone());
    config.chief_agent_id = Some(agent_id.clone());
    store::save_config(&config)?;
    let migrated = staff::StaffRegistry::from_legacy_config(&config);
    if !migrated.staff.is_empty() {
        let _ = staff::save(&migrated);
    }
    crate::debug_log::append(
        "chief",
        "info",
        &session.id,
        "chief session created",
        Some(&format!("agent={agent_id}")),
    );
    register_chief_session(&session.id);
    // Chief 自己也是后端驱动会话：它的回复/工具轨迹同样进事件账本，
    // 否则本地打开 Chief 会话时回填不到这段（worker 有账本、Chief 没有
    // 的不对称是实测抓到的）。
    let _ = tee::register(&session.id);
    Ok((session.id, true))
}

/// Chief 的 ACP 进程懒启动（首条消息 / 首个唤醒时）。成功后置 running 状态。
fn ensure_chief_started(app: &AppHandle, chief_session_id: &str) -> Result<(), String> {
    if runner::session_alive(app, chief_session_id) {
        return Ok(());
    }
    let (agent_id, cwd) = with_storage(app, |storage| {
        let session = storage
            .find_session(chief_session_id)
            .ok()
            .flatten()
            .ok_or_else(|| "chief session row missing".to_string())?;
        Ok::<_, String>((session.agent_id, session.cwd))
    })?;
    runner::ensure_session(app, chief_session_id, &agent_id, &cwd)?;
    if staff_commands::profile_for_session(chief_session_id).is_some() {
        runner::enforce_read_only(app, chief_session_id).map_err(|error| {
            runner::stop_session(app, chief_session_id);
            format!("员工会话无法强制只读/plan 权限：{error}")
        })?;
    }
    // 配置里钉了 Chief 模型（chief_model）就推一次；失败只记日志，
    // 会话用 agent 默认模型照常工作。
    let pinned_model = staff_commands::profile_for_session(chief_session_id)
        .and_then(|profile| profile.model)
        .or_else(|| store::load_config().chief_model);
    if let Some(model) = pinned_model.as_deref() {
        runner::apply_model(app, chief_session_id, model);
    }
    with_storage(app, |storage| {
        storage.update_session_status(chief_session_id, "running")
    })
    .ok();
    Ok(())
}

// ── 任务卡片（envelope 里的 task 形状） ──────────────────────────────────

fn task_card(task: &Task) -> Value {
    json!({
        "taskId": task.task_id,
        "staffId": task.staff_id,
        "title": task.title,
        "projectId": task.project_id,
        "workerSessionId": task.worker_session_id,
        "agentId": task.agent_id,
        "model": task.model,
        "status": task.status,
        "waitingReason": task.waiting_reason,
        "turnCount": task.turn_count,
        "costUsd": task.cost_usd,
        "lastSummary": task.last_summary,
        "error": task.error,
        "createdAt": task.created_at,
        "updatedAt": task.updated_at,
    })
}

fn task_belongs_to_session(task: &Task, chief_session_id: &str) -> bool {
    if task.chief_session_id == chief_session_id {
        return true;
    }
    let Some(profile) = staff_commands::profile_for_session(chief_session_id) else {
        return false;
    };
    task.staff_id.as_deref() == Some(profile.staff_id.as_str())
}

/// active = Starting / Running / WaitingUser（Chief 自身不算 worker 并发）。
#[cfg(test)]
fn count_active(tasks: &[Task], chief_session_id: &str, project_id: Option<&str>) -> usize {
    tasks
        .iter()
        .filter(|task| task_belongs_to_session(task, chief_session_id))
        .filter(|task| {
            matches!(
                task.status,
                TaskStatus::Proposed
                    | TaskStatus::Starting
                    | TaskStatus::Running
                    | TaskStatus::WaitingUser
            )
        })
        .filter(|task| match project_id {
            Some(pid) => task.project_id == pid,
            None => true,
        })
        .count()
}

// ── 控制工具实现（envelope 返回值） ──────────────────────────────────────

pub fn chief_projects_list(_chief_session_id: &str) -> Value {
    let config = staff_commands::profile_for_session(_chief_session_id)
        .map(|profile| staff_commands::config_for_staff(&profile))
        .unwrap_or_else(store::load_config);
    let projects: Vec<Value> = config
        .allowed_projects
        .iter()
        .map(|policy| {
            json!({
                "projectId": policy.project_id,
                "enabled": policy.enabled,
                "remoteMode": policy.remote_mode,
                "allowedAgents": policy.allowed_agents,
                "defaultAgentId": policy.default_agent_id,
                "defaultModel": policy.default_model,
                "maxConcurrentWorkers": policy.max_concurrent_workers,
                "maxTaskMinutes": policy.max_task_minutes,
            })
        })
        .collect();
    envelope_ok(json!({
        "projects": projects,
        "note": "只列出 Host 授权给 Chief 的项目；未授权的不会出现。",
    }))
}

pub fn chief_agents_list(_chief_session_id: &str, project_id: Option<&str>) -> Value {
    let config = staff_commands::profile_for_session(_chief_session_id)
        .map(|profile| staff_commands::config_for_staff(&profile))
        .unwrap_or_else(store::load_config);
    let agents: Vec<Value> = crate::custom_agents::all_agents_merged()
        .into_iter()
        .map(|agent| {
            let allowed = match project_id.and_then(|pid| config.policy_for(pid)) {
                Some(policy) if !policy.allowed_agents.is_empty() => {
                    policy.allowed_agents.contains(&agent.id)
                }
                _ => true,
            };
            json!({
                "agentId": agent.id,
                "label": agent.label,
                "enabled": agent.enabled,
                "allowedForProject": allowed,
            })
        })
        .collect();
    envelope_ok(json!({
        "agents": agents,
        "note": "allowedForProject=false 的 Agent 不能被该项目任务选用。",
    }))
}

pub fn chief_models_list(agent_id: &str) -> Value {
    // 跨用引擎目录是模型窗口的真相源（AGENTS「逐模型宣告」节）；
    // agent 原生模型跟 Agent 自己的默认，这里如实说明而不是编造清单。
    let engine_models: Vec<Value> = crate::engine::catalog_direct_entries()
        .into_iter()
        .take(50)
        .map(|row| {
            json!({
                "model": row.id,
                "contextWindow": row.context_window,
                "maxOutput": row.max_output,
                "images": row.images,
            })
        })
        .collect();
    envelope_ok(json!({
        "agentId": agent_id,
        "engineModels": engine_models,
        "note": "engineModels 是跨用引擎可路由的模型；Agent 原生模型清单跟 Agent 自身默认，task_start 不传 model 即用默认。",
    }))
}

pub fn chief_tasks_list(chief_session_id: &str, status_filter: Option<&str>, limit: usize) -> Value {
    let tasks: Vec<Value> = store::load_tasks()
        .into_iter()
        .filter(|task| task_belongs_to_session(task, chief_session_id))
        .filter(|task| {
            match status_filter {
                Some(status) => format!("{:?}", task.status).to_lowercase() == status.to_lowercase(),
                None => true,
            }
        })
        .take(limit.clamp(1, 50))
        .map(|task| task_card(&task))
        .collect();
    envelope_ok(json!({ "tasks": tasks }))
}

pub fn chief_task_start(app: &AppHandle, chief_session_id: &str, args: &Value) -> Value {
    let request_id = args.get("requestId").and_then(Value::as_str).unwrap_or("");
    if request_id.is_empty() {
        return envelope_reject("bad_request", "requestId 必填（幂等键）。");
    }
    let title = args
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("远程任务")
        .chars()
        .take(120)
        .collect::<String>();
    let goal = args.get("goal").and_then(Value::as_str).unwrap_or("").to_string();
    if goal.trim().is_empty() {
        return envelope_reject("bad_request", "goal 必填（完整自然语言目标）。");
    }
    let Some(project_id) = args.get("projectId").and_then(Value::as_str) else {
        return envelope_reject(
            "bad_request",
            "projectId 必填——先用 marionette_projects_list 选一个允许项目。",
        );
    };
    let requested_agent = args.get("agentId").and_then(Value::as_str);
    let requested_model = args.get("model").and_then(Value::as_str);
    let runtime_minutes = args
        .get("maxRuntimeMinutes")
        .and_then(Value::as_u64)
        .map(|value| value.clamp(1, 480) as u32);

    let Some(profile) = staff_commands::profile_for_session(chief_session_id) else {
        return envelope_reject("staff_missing", "当前会话没有绑定员工档案。");
    };
    // Fast path for a normal retry. The authoritative check is repeated inside
    // the task-ledger lock immediately before insertion.
    let loaded_tasks = store::load_tasks();
    if let Some(existing) = loaded_tasks.iter().find(|task| task.request_id == request_id) {
        let same_owner = existing.chief_session_id == chief_session_id
            || existing.staff_id.as_deref() == Some(profile.staff_id.as_str());
        if same_owner {
            return envelope_ok(json!({
                "task": task_card(existing),
                "existing": true,
                "note": "同一 requestId 的任务已存在，返回原任务（幂等）。",
            }));
        }
        return envelope_reject(
            "request_id_conflict",
            "requestId 已属于另一位员工；幂等键必须在员工范围内唯一。",
        );
    }
    if profile.policy.paused_until_user_resumes {
        return envelope_reject(
            "staff_paused",
            "员工已持久暂停。请在本地员工面板恢复后才能启动新任务。",
        );
    }
    if !profile.policy.allow_background_work {
        return envelope_reject(
            "background_work_disabled",
            "该员工已关闭后台工作；请先在员工档案中允许后台工作。",
        );
    }
    let config = staff_commands::config_for_staff(&profile);
    let agents = known_enabled_agents();
    let (project_id, agent_id, model, remote_mode, policy_minutes, self_modification) =
        match policy::validate_task_start(&config, project_id, requested_agent, requested_model, &agents)
        {
            Decision::Grant {
                project_id,
                agent_id,
                model,
                remote_mode,
                max_task_minutes,
                self_modification,
            } => (project_id, agent_id, model, remote_mode, max_task_minutes, self_modification),
            Decision::Reject { error, note } => return envelope_reject(error, &note),
        };
    let max_minutes = runtime_minutes
        .unwrap_or(policy_minutes)
        .min(profile.policy.max_wall_minutes_per_task);

    let global_cap = config.clamped_max_workers() as usize;
    let project_cap = config
        .policy_for(&project_id)
        .map(|policy| policy.max_concurrent_workers as usize)
        .unwrap_or(1);

    let now = now_ms();
    let task = Task {
        task_id: short_id("task"),
        request_id: request_id.to_string(),
        staff_id: Some(profile.staff_id.clone()),
        chief_session_id: chief_session_id.to_string(),
        title,
        goal,
        project_id: project_id.clone(),
        worker_session_id: None,
        agent_id: agent_id.clone(),
        model: model.clone(),
        status: TaskStatus::Proposed,
        waiting_reason: None,
        turn_count: 0,
        cost_usd: 0.0,
        created_at: now,
        updated_at: now,
        last_event_at: now,
        last_summary: None,
        stop_reason: None,
        error: None,
        telegram_message_id: None,
        artifacts: Vec::new(),
    };
    enum ReserveOutcome {
        Existing(Task),
        Inserted,
        Rejected(Value),
    }
    let task_id = task.task_id.clone();
    let owner_staff_id = profile.staff_id.clone();
    let reservation = store::with_tasks(|tasks| {
        if let Some(existing) = tasks.iter().find(|task| task.request_id == request_id) {
            let same_owner = existing.chief_session_id == chief_session_id
                || existing.staff_id.as_deref() == Some(owner_staff_id.as_str());
            if same_owner {
                return Ok(ReserveOutcome::Existing(existing.clone()));
            }
            return Ok(ReserveOutcome::Rejected(envelope_reject(
                "request_id_conflict",
                "requestId 已属于另一位员工；幂等键必须在员工范围内唯一。",
            )));
        }
        let global_now = tasks
            .iter()
            .filter(|task| {
                (task.chief_session_id == chief_session_id
                    || task.staff_id.as_deref() == Some(owner_staff_id.as_str()))
                    && matches!(
                        task.status,
                        TaskStatus::Proposed
                            | TaskStatus::Starting
                            | TaskStatus::Running
                            | TaskStatus::WaitingUser
                    )
            })
            .count();
        if global_now >= global_cap {
            return Ok(ReserveOutcome::Rejected(envelope_reject(
                "concurrency_limit",
                &format!("并发上限 {global_cap} 已满（当前 {global_now} 个进行中）。等任务完成或用 task_cancel 腾位。"),
            )));
        }
        let project_now = tasks
            .iter()
            .filter(|task| {
                (task.chief_session_id == chief_session_id
                    || task.staff_id.as_deref() == Some(owner_staff_id.as_str()))
                    && matches!(
                        task.status,
                        TaskStatus::Proposed
                            | TaskStatus::Starting
                            | TaskStatus::Running
                            | TaskStatus::WaitingUser
                    )
                    && task.project_id == project_id
            })
            .count();
        if project_now >= project_cap {
            return Ok(ReserveOutcome::Rejected(envelope_reject(
                "concurrency_limit",
                &format!("项目并发上限 {project_cap} 已满（该项目当前 {project_now} 个进行中）。"),
            )));
        }
        tasks.insert(0, task.clone());
        Ok(ReserveOutcome::Inserted)
    });
    match reservation {
        Ok(ReserveOutcome::Existing(existing)) => {
            return envelope_ok(json!({
                "task": task_card(&existing),
                "existing": true,
                "note": "同一 requestId 的任务已存在，返回原任务（幂等）。",
            }));
        }
        Ok(ReserveOutcome::Rejected(value)) => return value,
        Ok(ReserveOutcome::Inserted) => {}
        Err(error) => return envelope_reject("internal", &format!("任务预留失败：{error}")),
    }
    let created_event = emit_event(&task, "task.created", TaskStatus::Proposed, None);
    if let Err(error) = store::append_events(&[created_event]) {
        let _ = store::with_tasks(|tasks| {
            tasks.retain(|candidate| candidate.task_id != task_id);
            Ok(())
        });
        return envelope_reject("internal", &format!("任务事件落盘失败：{error}"));
    }
    // Close the pause-with-new-task race: stop_all may have scanned the ledger
    // just before this Proposed row became visible.
    let policy_block = match staff_commands::profile_for_staff_id(&profile.staff_id) {
        Some(current) if current.policy.paused_until_user_resumes => {
            Some(("staff_paused", "员工在任务预留期间被暂停，任务已取消。".to_string()))
        }
        Some(current) if !current.policy.allow_background_work => Some((
            "background_work_disabled",
            "员工在任务预留期间关闭了后台工作，任务已取消。".to_string(),
        )),
        Some(current) => match current.policy.policy_for(&task.project_id) {
            Some(policy) if policy.enabled && policy.remote_mode != RemoteMode::Blocked => None,
            Some(_) => Some((
                "project_blocked",
                "项目在任务预留期间被禁用，任务已取消。".to_string(),
            )),
            None => Some((
                "project_not_allowed",
                "项目在任务预留期间被移除授权，任务已取消。".to_string(),
            )),
        },
        None => Some((
            "staff_missing",
            "员工在任务预留期间被辞退，任务已取消。".to_string(),
        )),
    };
    if let Some((error, note)) = policy_block {
        let _ = finalize_task(
            app,
            chief_session_id,
            &task_id,
            TaskStatus::Cancelled,
            Some(error),
            Some(&note),
        );
        return envelope_reject(error, &note);
    }

    // 后台 spawn：task_start 立即返回（02-architecture.md §5.3）。
    let app_handle = app.clone();
    let chief_sid = chief_session_id.to_string();
    let spawn_task_id = task_id.clone();
    if std::thread::Builder::new()
        .name(format!("chief-spawn-{task_id}"))
        .spawn(move || {
            spawn_and_watch_worker(
                &app_handle,
                &chief_sid,
                &spawn_task_id,
                remote_mode,
                max_minutes,
                self_modification,
            );
        })
        .is_err()
    {
        let _ = store::with_tasks(|tasks| {
            if let Some(task) = tasks.iter_mut().find(|t| t.task_id == task_id) {
                task.status = TaskStatus::Failed;
                task.error = Some("spawn thread failed".into());
            }
            Ok(())
        });
        return envelope_reject("internal", "后台任务线程创建失败。");
    }
    envelope_ok(json!({
        "task": task_card(&task),
        "note": if self_modification {
            "这是自我修改仓库：默认只产出 worktree/diff/tests，不会自动发布。"
        } else {
            "已接单，worker 正在启动。"
        },
    }))
}

#[allow(clippy::too_many_arguments)]
fn spawn_and_watch_worker(
    app: &AppHandle,
    chief_session_id: &str,
    task_id: &str,
    remote_mode: RemoteMode,
    max_minutes: u32,
    self_modification: bool,
) {
    let Some(preview) = store::find_task(task_id) else {
        return;
    };
    let spawn_profile = preview
        .staff_id
        .as_deref()
        .and_then(staff_commands::profile_for_staff_id)
        .or_else(|| staff_commands::profile_for_session(chief_session_id));
    let blocked = match spawn_profile {
        Some(profile) if profile.policy.paused_until_user_resumes => {
            Some(("staff_paused", "员工已暂停，任务未启动。"))
        }
        Some(profile) if !profile.policy.allow_background_work => {
            Some(("background_work_disabled", "员工已关闭后台工作，任务未启动。"))
        }
        Some(profile) => match profile.policy.policy_for(&preview.project_id) {
            Some(policy) if policy.enabled && policy.remote_mode != RemoteMode::Blocked => None,
            Some(_) => Some(("project_blocked", "项目授权已禁用，任务未启动。")),
            None => Some(("project_not_allowed", "项目授权已移除，任务未启动。")),
        },
        None => Some(("staff_missing", "员工档案不存在，任务未启动。")),
    };
    if let Some((reason, note)) = blocked {
        let _ = finalize_task(
            app,
            chief_session_id,
            task_id,
            TaskStatus::Cancelled,
            Some(reason),
            Some(note),
        );
        return;
    }
    // 1) Proposed → Starting
    let task = match store::with_tasks(|tasks| {
        let Some(task) = tasks.iter_mut().find(|t| t.task_id == task_id) else {
            return Err("task vanished".to_string());
        };
        if task.status != TaskStatus::Proposed {
            return Err(format!(
                "task status {} cannot start",
                format!("{:?}", task.status).to_lowercase()
            ));
        }
        task.status = TaskStatus::Starting;
        task.updated_at = now_ms();
        Ok(task.clone())
    }) {
        Ok(task) => task,
        Err(_) => return,
    };

    // 2) 建 worker 会话行（普通项目会话，顶层可见）
    let worker = match with_storage(app, |storage| {
        storage.create_session_with_origin(
            &task.project_id,
            task.agent_id.clone(),
            format!("{}（Chief 任务）", task.title),
            Some("chief-worker"),
        )
    }) {
        Ok(session) => session,
        Err(error) => {
            let _ = finalize_task(app, chief_session_id, task_id, TaskStatus::Failed, None,
                Some(&format!("创建 worker 会话失败：{error}")));
            return;
        }
    };
    let _ = tee::register(&worker.id);
    // Link the task before ACP starts: tools/list can arrive during agent
    // initialization, and staff policy (notably Computer Use) must already be
    // resolvable from the worker session id at that point.
    if let Err(error) = store::with_tasks(|tasks| {
        if let Some(task) = tasks.iter_mut().find(|task| task.task_id == task_id) {
            task.worker_session_id = Some(worker.id.clone());
            task.updated_at = now_ms();
        } else {
            return Err("task vanished before worker policy mapping".to_string());
        }
        Ok(())
    }) {
        runner::stop_session(app, &worker.id);
        tee::unregister(&worker.id);
        let _ = finalize_task(
            app,
            chief_session_id,
            task_id,
            TaskStatus::Failed,
            Some("policy_mapping_failed"),
            Some(&format!("worker 策略映射写入失败，拒绝启动 ACP：{error}")),
        );
        return;
    }

    // 3) 启动 ACP + 应用模型
    if let Err(error) = runner::ensure_session(app, &worker.id, &task.agent_id, &worker.cwd) {
        runner::stop_session(app, &worker.id);
        tee::unregister(&worker.id);
        let _ = finalize_task(app, chief_session_id, task_id, TaskStatus::Failed, None,
            Some(&format!("Agent 启动失败：{error}")));
        return;
    }
    if remote_mode == RemoteMode::ReadOnly {
        if let Err(error) = runner::enforce_read_only(app, &worker.id) {
            runner::stop_session(app, &worker.id);
            tee::unregister(&worker.id);
            let _ = finalize_task(
                app,
                chief_session_id,
                task_id,
                TaskStatus::Failed,
                Some("read_only_not_enforceable"),
                Some(&format!("Agent 无法强制只读权限，已拒绝启动任务：{error}")),
            );
            return;
        }
    }
    with_storage(app, |storage| {
        storage.update_session_status(&worker.id, "running")
    })
    .ok();
    if let Some(model) = task.model.as_deref() {
        runner::apply_model(app, &worker.id, model);
    }

    // 4) 发目标（read_only / 自我修改的边界在提示里明示；写权限仍由权限卡兜底）
    let mut prompt = task.goal.clone();
    if remote_mode == RemoteMode::ReadOnly {
        prompt.push_str("\n\n[HOST 约束] 本任务为只读模式：只分析/检索/读取，不要修改任何文件。");
    }
    if self_modification {
        prompt.push_str(
            "\n\n[HOST 约束] 这是 Marionette 自我修改仓库：在独立 git worktree/branch 上做，\
             允许 build/test/diff；不 push、不合并主分支、不打 tag、不发布、不替换运行中的 exe。\
             最终交付 diff 与测试结果。",
        );
    }
    match runner::send_prompt(app, &worker.id, &prompt) {
        Ok(_) => {}
        Err(error) => {
            let _ = finalize_task(app, chief_session_id, task_id, TaskStatus::Failed, None,
                Some(&format!("目标下发失败：{error}")));
            return;
        }
    }

    // 5) Starting → Running；worker ACP ready 才算 task.started（事件表口径）
    let task = match store::with_tasks(|tasks| {
        let Some(task) = tasks.iter_mut().find(|t| t.task_id == task_id) else {
            return Err("task vanished".to_string());
        };
        task.worker_session_id = Some(worker.id.clone());
        if !task.status.can_transition(TaskStatus::Running) {
            return Err(format!(
                "task status {} cannot run",
                format!("{:?}", task.status).to_lowercase()
            ));
        }
        task.status = TaskStatus::Running;
        task.turn_count = task.turn_count.saturating_add(1);
        task.updated_at = now_ms();
        task.last_event_at = now_ms();
        Ok(task.clone())
    }) {
        Ok(task) => task,
        Err(_) => return,
    };
    let running_event = emit_event(
        &task,
        "task.started",
        TaskStatus::Running,
        Some(format!("worker 已启动（{} · {}）", task.agent_id, worker.id)),
    );
    let _ = store::append_events(&[running_event]);

    // 6) 观测直到空闲 / 超时 / 被取消
    watch_worker_loop(app, chief_session_id, task_id, &worker.id, max_minutes);
}

/// 观测循环（spawn 与 resume 共用）：任务状态驱动退出 + 超时停止 + 空闲收尾。
fn watch_worker_loop(
    app: &AppHandle,
    chief_session_id: &str,
    task_id: &str,
    worker_session_id: &str,
    max_minutes: u32,
) {
    let deadline_ms = (max_minutes as u64) * 60_000;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(deadline_ms);
    loop {
        // 任务被外部取消/恢复：状态驱动退出。
        let Some(current) = store::find_task(task_id) else { return };
        match current.status {
            TaskStatus::Cancelled | TaskStatus::Failed | TaskStatus::Completed
            | TaskStatus::Interrupted => return, // 终态已由相应路径落账
            _ => {}
        }
        if std::time::Instant::now() >= deadline {
            runner::cancel_turn(app, worker_session_id);
            let _ = runner::wait_turn_idle_checked(app, worker_session_id, 10_000, || true);
            let _ = finalize_task(app, chief_session_id, task_id, TaskStatus::Failed,
                Some("timeout"), Some(&format!("任务超过 {max_minutes} 分钟上限，已停止。")));
            return;
        }
        if runner::turn_idle_now(app, worker_session_id) {
            // 空闲即认为回合完成；worker 正文从 tee 折出。
            // 给 tee writer 一点收尾时间再读回。
            std::thread::sleep(std::time::Duration::from_millis(600));
            finish_completed(app, chief_session_id, task_id, worker_session_id);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2000));
    }
}

fn finish_completed(app: &AppHandle, chief_session_id: &str, task_id: &str, worker_session_id: &str) {
    let cost_cap = store::find_task(task_id)
        .and_then(|task| {
            let staff_id = task.staff_id?;
            let profile = staff_commands::profile_for_staff_id(&staff_id)?;
            Some((profile.policy.max_cost_per_task_usd, task.project_id))
        });
    let reported_cost = worker_reported_cost_usd(worker_session_id);
    if let Some((cap, project_id)) = cost_cap {
        if cap > 0.0 && reported_cost >= cap {
            let _ = finalize_task(
                app,
                chief_session_id,
                task_id,
                TaskStatus::Failed,
                Some("cost_budget_exhausted"),
                Some(&format!(
                    "任务 {project_id} 已报告 {reported_cost:.4} USD，达到 {cap:.4} USD 成本上限，已停止。"
                )),
            );
            return;
        }
    }
    let events = tee::replay(worker_session_id)
        .map(|(events, _)| events)
        .unwrap_or_default();
    let (summary, tool_calls, files, tool_failures) = summarize_worker_events(&events);
    let mut artifacts = Vec::new();
    if !files.is_empty() {
        artifacts.push(format!("触及文件：{}", files.join(", ")));
    }
    artifacts.push(format!("工具调用 {tool_calls} 次（失败 {tool_failures}）"));
    let status_line = if tool_failures > 0 {
        format!("完成，但有 {tool_failures} 个工具步骤失败——请核对。")
    } else {
        "完成。".to_string()
    };
    let combined = match summary.as_deref() {
        Some(text) => format!("{status_line} 摘要：{text}"),
        None => format!("{status_line}（worker 未产出文本回复）"),
    };
    let _ = store::with_tasks(|tasks| {
        if let Some(task) = tasks.iter_mut().find(|t| t.task_id == task_id) {
            task.artifacts = artifacts.clone();
        }
        Ok(())
    });
    let _ = finalize_task(
        app,
        chief_session_id,
        task_id,
        TaskStatus::Completed,
        None,
        Some(&combined),
    );
}

/// 任务收尾：状态转移 + 事件 + Telegram 卡 + 唤醒收件箱。幂等（终态跳过）。
fn finalize_task(
    app: &AppHandle,
    chief_session_id: &str,
    task_id: &str,
    status: TaskStatus,
    stop_reason: Option<&str>,
    summary: Option<&str>,
) -> Result<bool, String> {
    let cost_usd = store::find_task(task_id)
        .and_then(|task| task.worker_session_id)
        .map(|worker_sid| worker_reported_cost_usd(&worker_sid))
        .unwrap_or(0.0);
    let finalized = store::with_tasks(|tasks| {
        let Some(task) = tasks.iter_mut().find(|t| t.task_id == task_id) else {
            return Ok::<Option<Task>, String>(None);
        };
        if task.status.is_terminal() || task.status == TaskStatus::Interrupted {
            return Ok(None); // 已收过尾（幂等）
        }
        if !task.status.can_transition(status) {
            return Ok(None);
        }
        task.status = status;
        task.cost_usd = cost_usd;
        task.waiting_reason = None;
        task.updated_at = now_ms();
        task.last_event_at = now_ms();
        task.stop_reason = stop_reason.map(str::to_string);
        task.last_summary = summary.map(str::to_string);
        Ok(Some(task.clone()))
    });
    let Some(task) = finalized? else {
        return Ok(false);
    };
    let kind = match status {
        TaskStatus::Completed => "task.completed",
        TaskStatus::Failed => "task.failed",
        TaskStatus::Cancelled => "task.cancelled",
        TaskStatus::Interrupted => "task.interrupted",
        _ => "task.progress",
    };
    let event = emit_event(&task, kind, status, task.last_summary.clone());
    store::append_events(&[event.clone()])?;
    record_task_journal(&task, status);
    notify(&task, kind, task.last_summary.as_deref());
    let wake_session_id = task
        .staff_id
        .as_deref()
        .and_then(staff_commands::profile_for_staff_id)
        .map(|profile| profile.session_id)
        .unwrap_or_else(|| chief_session_id.to_string());
    queue_wake(&wake_session_id, event);
    if matches!(status, TaskStatus::Completed | TaskStatus::Failed) {
        activate_task_condition(&task, status);
    }
    // 终态任务把 worker 会话状态放回 exited（UI 列表不再显示运行中）。
    if status.is_terminal() {
        if let Some(worker_sid) = task.worker_session_id.as_deref() {
            with_storage(app, |storage| {
                storage.update_session_status(worker_sid, "exited")
            })
            .ok();
        }
    }
    Ok(true)
}

fn record_task_journal(task: &Task, status: TaskStatus) {
    let Some(staff_id) = task.staff_id.as_deref() else {
        return;
    };
    let date = super::memory::journal_date_for_ms(task.updated_at);
    let entry = format!(
        "### {} · {}\n- taskId: `{}`\n- project: `{}`\n- status: `{}`\n- summary: {}",
        task.title,
        super::memory::journal_date_for_ms(task.updated_at),
        task.task_id,
        task.project_id,
        format!("{status:?}").to_lowercase(),
        task.last_summary.as_deref().unwrap_or("（无摘要）"),
    );
    if let Err(error) = super::memory::append_journal_with_source_refs(
        staff_id,
        &date,
        &entry,
        vec![super::memory::MemorySourceRef::task(&task.task_id)],
    ) {
        crate::debug_log::append(
            "chief",
            "warn",
            &task.task_id,
            "task journal append failed",
            Some(&error),
        );
    }
}

fn activate_task_condition(task: &Task, status: TaskStatus) {
    let outcome = if status == TaskStatus::Completed {
        "completed"
    } else {
        "failed"
    };
    let condition = format!("task:{}:{outcome}", task.task_id);
    let changed = super::commitments::with_ledger(|ledger| {
        let now = now_ms();
        let mut changed = Vec::new();
        for commitment in &mut ledger.commitments {
            if commitment.status == super::commitments::CommitmentStatus::Open
                && commitment.condition.as_deref() == Some(condition.as_str())
            {
                commitment.status = super::commitments::CommitmentStatus::Due;
                commitment.updated_at = now;
                changed.push(commitment.clone());
            }
        }
        Ok(changed)
    })
    .unwrap_or_default();
    for commitment in changed {
        if let Some(profile) = staff_commands::profile_for_staff_id(&commitment.staff_id) {
            if !profile.policy.paused_until_user_resumes
                && profile.policy.allow_background_work
                && profile.policy.autonomy != staff::StaffAutonomy::Manual
            {
                queue_host_text(
                    &profile.session_id,
                    &format!(
                        "[HOST COMMITMENT DUE: condition]\ncommitmentId: {}\ncondition: {}\n{}",
                        commitment.commitment_id, condition, commitment.text
                    ),
                );
            }
        }
    }
}

// ── task_send / status / cancel / collect / resume ───────────────────────

pub fn chief_task_send(app: &AppHandle, chief_session_id: &str, args: &Value) -> Value {
    let Some(task_id) = args.get("taskId").and_then(Value::as_str) else {
        return envelope_reject("bad_request", "taskId 必填。");
    };
    let message = args.get("message").and_then(Value::as_str).unwrap_or("");
    if message.trim().is_empty() {
        return envelope_reject("bad_request", "message 必填。");
    }
    let urgency = args.get("urgency").and_then(Value::as_str).unwrap_or("normal");
    let command_id = args.get("commandId").and_then(Value::as_str).unwrap_or("");
    if !command_id.is_empty() && sent_command_seen(task_id, command_id) {
        return envelope_ok(json!({
            "delivered": true,
            "existing": true,
            "note": "同一 commandId 已送达过（幂等跳过）。",
        }));
    }
    let Some(task) = store::find_task(task_id) else {
        return envelope_reject("task_not_found", "任务不存在（或不属于这个 Chief）。");
    };
    let Some(profile) = staff_commands::profile_for_session(chief_session_id) else {
        return envelope_reject("staff_missing", "当前会话没有绑定员工档案。");
    };
    if profile.policy.paused_until_user_resumes {
        return envelope_reject(
            "staff_paused",
            "员工已持久暂停；恢复后才能继续向 worker 追加指令。",
        );
    }
    if !profile.policy.allow_background_work {
        return envelope_reject(
            "background_work_disabled",
            "员工已关闭后台工作，不能再向 worker 追加指令。",
        );
    }
    if let Err((error, note)) = policy::validate_task_access_for_staff(
        &task,
        chief_session_id,
        Some(&profile.staff_id),
        "send",
    ) {
        return envelope_reject(error, &note);
    }
    let Some(project_policy) = profile.policy.policy_for(&task.project_id) else {
        return envelope_reject(
            "project_not_allowed",
            "项目已从员工白名单移除，不能继续向该任务发送指令。",
        );
    };
    if !project_policy.enabled || project_policy.remote_mode == RemoteMode::Blocked {
        return envelope_reject("project_blocked", "项目授权已禁用，不能继续该任务。");
    }
    if task.turn_count >= profile.policy.max_turns_per_task {
        return envelope_reject(
            "turn_budget_exhausted",
            &format!(
                "任务已达到 {} 个 worker turn 上限。请新建任务或由用户提高预算。",
                profile.policy.max_turns_per_task
            ),
        );
    }
    let Some(worker_sid) = task.worker_session_id.clone() else {
        return envelope_reject("task_not_running", "worker 还没启动。");
    };
    let model_calls = worker_reported_model_calls(&worker_sid).max(task.turn_count);
    if profile.policy.max_model_calls_per_task > 0
        && model_calls >= profile.policy.max_model_calls_per_task
    {
        return envelope_reject(
            "model_call_budget_exhausted",
            &format!(
                "任务已记录 {model_calls} 个模型回合，达到 {} 次上限。",
                profile.policy.max_model_calls_per_task
            ),
        );
    }
    let spent_usd = worker_reported_cost_usd(&worker_sid);
    if profile.policy.max_cost_per_task_usd > 0.0
        && spent_usd >= profile.policy.max_cost_per_task_usd
    {
        return envelope_reject(
            "cost_budget_exhausted",
            &format!(
                "任务已报告 {:.4} USD，达到 {:.4} USD 成本上限。",
                spent_usd, profile.policy.max_cost_per_task_usd
            ),
        );
    }
    if urgency == "interrupt" {
        runner::cancel_turn(app, &worker_sid);
        let _ = runner::wait_turn_idle_checked(app, &worker_sid, 8_000, || true);
    }
    if !command_id.is_empty() && !mark_sent_command(task_id, command_id) {
        return envelope_ok(json!({
            "delivered": true,
            "existing": true,
            "note": "同一 commandId 已送达过（幂等跳过）。",
        }));
    }
    let mut effective_message = message.to_string();
    if project_policy.remote_mode == RemoteMode::ReadOnly {
        effective_message.push_str("\n\n[HOST 约束] 本任务为只读模式：不要修改任何文件。");
    }
    if project_policy.self_modification {
        effective_message.push_str(
            "\n\n[HOST 约束] 自我修改仓库：只产出 worktree/diff/tests，不 push、不合并、不发布。",
        );
    }
    match runner::send_prompt(app, &worker_sid, &effective_message) {
        Ok(_) => {
            let _ = store::with_tasks(|tasks| {
                if let Some(task) = tasks.iter_mut().find(|t| t.task_id == task_id) {
                    task.last_event_at = now_ms();
                    task.turn_count = task.turn_count.saturating_add(1);
                    task.updated_at = now_ms();
                }
                Ok(())
            });
            envelope_ok(json!({ "delivered": true }))
        }
        Err(error) if error.contains("already in flight") => {
            if !command_id.is_empty() {
                unmark_sent_command(task_id, command_id);
            }
            envelope_reject(
                "worker_busy",
                "worker 正在回合中。可用 urgency=interrupt 打断，或稍后再发。",
            )
        }
        Err(error) => {
            if !command_id.is_empty() {
                unmark_sent_command(task_id, command_id);
            }
            envelope_reject("deliver_failed", &error)
        }
    }
}

pub fn chief_task_status(chief_session_id: &str, task_id: &str) -> Value {
    let Some(task) = store::find_task(task_id) else {
        return envelope_reject("task_not_found", "任务不存在。");
    };
    if !task_belongs_to_session(&task, chief_session_id) {
        return envelope_reject("task_not_owned", "任务不属于这个 Chief。");
    }
    envelope_ok(json!({
        "task": task_card(&task),
        "waitingUser": task.status == TaskStatus::WaitingUser,
    }))
}

pub fn chief_task_cancel(app: &AppHandle, chief_session_id: &str, args: &Value) -> Value {
    let Some(task_id) = args.get("taskId").and_then(Value::as_str) else {
        return envelope_reject("bad_request", "taskId 必填。");
    };
    let reason = args.get("reason").and_then(Value::as_str).unwrap_or("用户要求停止");
    let Some(task) = store::find_task(task_id) else {
        return envelope_reject("task_not_found", "任务不存在。");
    };
    let staff_id = staff_commands::profile_for_session(chief_session_id)
        .map(|profile| profile.staff_id);
    if let Err((error, note)) = policy::validate_task_access_for_staff(
        &task,
        chief_session_id,
        staff_id.as_deref(),
        "cancel",
    ) {
        return envelope_reject(error, &note);
    }
    if task.status == TaskStatus::Cancelled {
        return envelope_ok(json!({ "task": task_card(&task), "note": "已取消（幂等）。" }));
    }
    if let Some(worker_sid) = task.worker_session_id.as_deref() {
        runner::cancel_turn(app, worker_sid);
        let _ = runner::wait_turn_idle_checked(app, worker_sid, 8_000, || true);
        let _ = crate::browser::close_tabs_for_session(app, worker_sid);
    }
    match finalize_task(
        app,
        chief_session_id,
        task_id,
        TaskStatus::Cancelled,
        Some("cancelled"),
        Some(reason),
    ) {
        Ok(_) => {}
        Err(error) => {
            return envelope_reject(
                "cancel_persist_failed",
                &format!("取消失败：任务账本未确认写入（{error}）。"),
            )
        }
    }
    envelope_ok(json!({ "task": store::find_task(task_id).map(|t| task_card(&t)) }))
}

pub fn chief_task_collect(chief_session_id: &str, args: &Value) -> Value {
    let Some(task_id) = args.get("taskId").and_then(Value::as_str) else {
        return envelope_reject("bad_request", "taskId 必填。");
    };
    let Some(task) = store::find_task(task_id) else {
        return envelope_reject("task_not_found", "任务不存在。");
    };
    if !task_belongs_to_session(&task, chief_session_id) {
        return envelope_reject("task_not_owned", "任务不属于这个 Chief。");
    }
    let Some(worker_sid) = task.worker_session_id.clone() else {
        return envelope_ok(json!({
            "task": task_card(&task),
            "interim": true,
            "summary": Option::<String>::None,
            "note": "worker 还没启动，没有可收的产出。",
        }));
    };
    let events = tee::replay(&worker_sid)
        .map(|(events, _)| events)
        .unwrap_or_default();
    let (summary, tool_calls, files, tool_failures) = summarize_worker_events(&events);
    let artifact_limit = args
        .get("artifactLimit")
        .and_then(Value::as_u64)
        .unwrap_or(20)
        .min(20) as usize;
    let files = files.into_iter().take(artifact_limit).collect::<Vec<_>>();
    envelope_ok(json!({
        "task": task_card(&task),
        "interim": !task.status.is_terminal(),
        "summary": summary,
        "toolCalls": tool_calls,
        "toolFailures": tool_failures,
        "files": files,
    }))
}

pub fn chief_task_resume(app: &AppHandle, chief_session_id: &str, args: &Value) -> Value {
    let Some(task_id) = args.get("taskId").and_then(Value::as_str) else {
        return envelope_reject("bad_request", "taskId 必填。");
    };
    let note = args.get("note").and_then(Value::as_str).unwrap_or("").to_string();
    let Some(task) = store::find_task(task_id) else {
        return envelope_reject("task_not_found", "任务不存在。");
    };
    if !task_belongs_to_session(&task, chief_session_id) {
        return envelope_reject("task_not_owned", "任务不属于这个 Chief。");
    }
    let Some(profile) = staff_commands::profile_for_session(chief_session_id) else {
        return envelope_reject("staff_missing", "当前会话没有绑定员工档案。");
    };
    if profile.policy.paused_until_user_resumes {
        return envelope_reject("staff_paused", "员工已持久暂停；恢复后才能 resume。");
    }
    if !profile.policy.allow_background_work {
        return envelope_reject(
            "background_work_disabled",
            "员工已关闭后台工作，不能 resume。",
        );
    }
    if task.turn_count >= profile.policy.max_turns_per_task {
        return envelope_reject(
            "turn_budget_exhausted",
            &format!(
                "任务已达到 {} 个 worker turn 上限，不能 resume。",
                profile.policy.max_turns_per_task
            ),
        );
    }
    if task.status != TaskStatus::Interrupted {
        return envelope_reject(
            "not_resumable",
            &format!(
                "任务状态 {} 不能恢复——只有 interrupted 可 resume；completed 请重新发起任务。",
                format!("{:?}", task.status).to_lowercase()
            ),
        );
    }
    let Some(worker_sid) = task.worker_session_id.clone() else {
        return envelope_reject("not_resumable", "worker 从未启动，没有可恢复的会话。请重新发起任务。");
    };
    let model_calls = worker_reported_model_calls(&worker_sid).max(task.turn_count);
    if profile.policy.max_model_calls_per_task > 0
        && model_calls >= profile.policy.max_model_calls_per_task
    {
        return envelope_reject(
            "model_call_budget_exhausted",
            &format!(
                "任务已记录 {model_calls} 个模型回合，达到 {} 次上限，不能 resume。",
                profile.policy.max_model_calls_per_task
            ),
        );
    }
    let spent_usd = worker_reported_cost_usd(&worker_sid);
    if profile.policy.max_cost_per_task_usd > 0.0
        && spent_usd >= profile.policy.max_cost_per_task_usd
    {
        return envelope_reject(
            "cost_budget_exhausted",
            &format!(
                "任务已报告 {:.4} USD，达到 {:.4} USD 成本上限，不能 resume。",
                spent_usd, profile.policy.max_cost_per_task_usd
            ),
        );
    }
    let Some(project_policy) = profile.policy.policy_for(&task.project_id) else {
        return envelope_reject(
            "project_not_allowed",
            "项目已从该员工的白名单移除，不能 resume。",
        );
    };
    if !project_policy.enabled || project_policy.remote_mode == RemoteMode::Blocked {
        return envelope_reject("project_blocked", "项目授权已禁用，不能 resume。");
    }
    let remote_mode = project_policy.remote_mode;
    let max_minutes = project_policy
        .max_task_minutes
        .min(profile.policy.max_wall_minutes_per_task);
    let self_modification = project_policy.self_modification;
    // 标 Starting（resume 转移），后台重启进程并重发指令。
    if let Err(error) = store::with_tasks(|tasks| {
        let Some(t) = tasks.iter_mut().find(|t| t.task_id == task_id) else {
            return Err("task vanished".to_string());
        };
        if t.status == TaskStatus::Starting {
            return Err("__resume_in_progress__".to_string());
        }
        if t.status != TaskStatus::Interrupted {
            return Err(format!("状态 {} 不能恢复", format!("{:?}", t.status).to_lowercase()));
        }
        t.status = TaskStatus::Starting;
        t.chief_session_id = chief_session_id.to_string();
        if t.staff_id.is_none() {
            t.staff_id = Some(profile.staff_id.clone());
        }
        t.turn_count = t.turn_count.saturating_add(1);
        t.updated_at = now_ms();
        t.last_event_at = now_ms();
        Ok(())
    }) {
        if error == "__resume_in_progress__" {
            return envelope_ok(json!({
                "resuming": true,
                "existing": true,
                "taskId": task_id,
                "note": "该任务已经在恢复中（幂等）。"
            }));
        }
        return envelope_reject("internal", &error);
    }
    let app_handle = app.clone();
    let chief_sid = chief_session_id.to_string();
    let resume_task_id = task_id.to_string();
    let resume_worker = worker_sid.clone();
    if std::thread::Builder::new()
        .name(format!("chief-resume-{task_id}"))
        .spawn(move || {
            resume_and_watch(
                &app_handle,
                &chief_sid,
                &resume_task_id,
                &resume_worker,
                remote_mode,
                max_minutes,
                self_modification,
                &note,
            );
        })
        .is_err()
    {
        let _ = store::with_tasks(|tasks| {
            if let Some(task) = tasks.iter_mut().find(|task| task.task_id == task_id) {
                if task.status == TaskStatus::Starting {
                    task.status = TaskStatus::Interrupted;
                    task.turn_count = task.turn_count.saturating_sub(1);
                    task.error = Some("resume thread spawn failed".to_string());
                }
            }
            Ok(())
        });
        return envelope_reject("internal", "恢复线程创建失败，任务已回到 interrupted。");
    }
    envelope_ok(json!({ "resuming": true, "taskId": task_id }))
}

#[allow(clippy::too_many_arguments)]
fn resume_and_watch(
    app: &AppHandle,
    chief_session_id: &str,
    task_id: &str,
    worker_session_id: &str,
    remote_mode: RemoteMode,
    max_minutes: u32,
    self_modification: bool,
    note: &str,
) {
    let Some(task) = store::find_task(task_id) else { return };
    if task.status != TaskStatus::Starting {
        return;
    }
    let Some(cwd) = with_storage(app, |storage| {
        storage
            .find_session(worker_session_id)
            .ok()
            .flatten()
            .map(|session| session.cwd)
    }) else {
        let _ = store::with_tasks(|tasks| {
            if let Some(task) = tasks.iter_mut().find(|task| task.task_id == task_id) {
                if task.status == TaskStatus::Starting {
                    task.status = TaskStatus::Interrupted;
                    task.error = Some("worker session disappeared before resume".to_string());
                }
            }
            Ok(())
        });
        return;
    };
    if let Err(error) = runner::ensure_session(app, worker_session_id, &task.agent_id, &cwd) {
        let _ = finalize_task(app, chief_session_id, task_id, TaskStatus::Failed, None,
            Some(&format!("恢复失败：{error}")));
        return;
    }
    if remote_mode == RemoteMode::ReadOnly {
        if let Err(error) = runner::enforce_read_only(app, worker_session_id) {
            runner::stop_session(app, worker_session_id);
            let _ = finalize_task(
                app,
                chief_session_id,
                task_id,
                TaskStatus::Failed,
                Some("read_only_not_enforceable"),
                Some(&format!("Agent 无法强制只读权限，已拒绝恢复：{error}")),
            );
            return;
        }
    }
    if !store::find_task(task_id).is_some_and(|task| task.status == TaskStatus::Starting) {
        runner::stop_session(app, worker_session_id);
        return;
    }
    with_storage(app, |storage| {
        storage.update_session_status(worker_session_id, "running")
    })
    .ok();
    let mut prompt = format!(
        "[HOST EVENT: task.resume]\n继续之前的任务「{}」。原始目标：\n{}\n",
        task.title, task.goal
    );
    if !note.is_empty() {
        prompt.push_str(&format!("用户补充：{note}\n"));
    }
    if remote_mode == RemoteMode::ReadOnly {
        prompt.push_str("[HOST 约束] 本任务为只读模式：不要修改文件。\n");
    }
    if self_modification {
        prompt.push_str("[HOST 约束] 自我修改仓库：只产出 worktree/diff/tests，不 push 不发布。\n");
    }
    match runner::send_prompt(app, worker_session_id, &prompt) {
        Ok(_) => {}
        Err(error) => {
            let _ = finalize_task(app, chief_session_id, task_id, TaskStatus::Failed, None,
                Some(&format!("恢复下发失败：{error}")));
            return;
        }
    }
    let transitioned = store::with_tasks(|tasks| {
        if let Some(t) = tasks.iter_mut().find(|t| t.task_id == task_id) {
            if t.status != TaskStatus::Starting {
                return Ok(false);
            }
            t.status = TaskStatus::Running;
            t.updated_at = now_ms();
            t.last_event_at = now_ms();
            return Ok(true);
        }
        Ok(false)
    })
    .unwrap_or(false);
    if !transitioned {
        runner::cancel_turn(app, worker_session_id);
        return;
    }
    watch_worker_loop(app, chief_session_id, task_id, worker_session_id, max_minutes);
}

/// 确保 Chief 会话存在并返回其 id（供 E2E 入口用；产品路径走
/// chief_inbound_text 内部同款引导）。
pub fn chief_session_ensure(app: &AppHandle) -> Result<String, String> {
    ensure_chief_session(app).map(|(sid, _created)| sid)
}

// ── Telegram 入站（chief 模式唯一文本入口） ──────────────────────────────

/// 该会话是否是某任务的 worker（Telegram 聚合判定用：worker 的逐条回合
/// 推送被任务卡取代——调用频率是「每回合结束一次」，读 tasks.json 可接受）。
pub fn is_chief_worker(session_id: &str) -> bool {
    store::load_tasks()
        .iter()
        .any(|task| task.worker_session_id.as_deref() == Some(session_id))
}

/// Worker 权限/提问/计划卡的等待与解除 → 任务状态机（02-architecture.md
/// §7.1：task.waiting_user 直接发批准卡，不经 Chief 决定）。挂在 emit_now
/// 的事件收口上；非 worker 会话 / 非等待类事件一次查表即返回。
pub fn observe_chief_event(
    app: &AppHandle,
    session_id: &str,
    method: Option<&str>,
    data: &Value,
) {
    use TaskStatus::*;
    let Some(method) = method else { return };
    let entered = matches!(method, "permission/prompt" | "question/prompt" | "plan/approval");
    let cleared = matches!(
        method,
        "permission/resolved"
            | "permission/timeout"
            | "question/resolved"
            | "question/timeout"
            | "plan/resolved"
            | "plan/timeout"
    );
    if !entered && !cleared {
        touch_worker_progress(session_id);
        return;
    }
    let Some(task) = store::load_tasks().into_iter().find(|task| {
        task.worker_session_id.as_deref() == Some(session_id)
    }) else {
        return;
    };
    if entered {
        // permission/prompt 的 payload 顶层就是 {requestId, title, detail, …}
        // （acp.rs emit_event 形状）。
        let summary = data
            .get("title")
            .and_then(Value::as_str)
            .map(|title| format!("任务在等你确认：{title}"));
        let updated = store::with_tasks(|tasks| {
            let Some(task) = tasks.iter_mut().find(|t| t.task_id == task.task_id) else {
                return Ok(None);
            };
            if !task.status.can_transition(WaitingUser) {
                return Ok(None);
            }
            task.status = WaitingUser;
            task.waiting_reason = Some(match method {
                "permission/prompt" => "permission",
                "question/prompt" => "question",
                "plan/approval" => "plan_approval",
                _ => "user_input",
            }
            .to_string());
            task.updated_at = now_ms();
            task.last_event_at = now_ms();
            if summary.is_some() {
                task.last_summary = summary.clone();
            }
            Ok(Some(task.clone()))
        });
        let Some(task) = updated.ok().flatten() else { return };
        let event = emit_event(&task, "task.waiting_user", WaitingUser, task.last_summary.clone());
        let _ = store::append_events(&[event]);
        notify(&task, "task.waiting_user", task.last_summary.as_deref());
        // worker 会话行状态同步（UI 侧栏显示「等你」）。
        if let Some(worker_sid) = task.worker_session_id.as_deref() {
            app.state::<crate::AppState>()
                .storage
                .lock()
                .ok()
                .map(|storage| storage.update_session_status(worker_sid, "waiting"));
        }
    } else {
        // 解除（answered 本地或手机先答先得 / 超时拒绝）：回到 running。
        let updated = store::with_tasks(|tasks| {
            let Some(task) = tasks.iter_mut().find(|t| t.task_id == task.task_id) else {
                return Ok(None);
            };
            if task.status != WaitingUser {
                return Ok(None);
            }
            task.status = Running;
            task.waiting_reason = None;
            task.updated_at = now_ms();
            task.last_event_at = now_ms();
            Ok(Some(task.clone()))
        });
        let Some(task) = updated.ok().flatten() else { return };
        let event = emit_event(&task, "task.progress", Running, None);
        let _ = store::append_events(&[event]);
        if let Some(worker_sid) = task.worker_session_id.as_deref() {
            app.state::<crate::AppState>()
                .storage
                .lock()
                .ok()
                .map(|storage| storage.update_session_status(worker_sid, "running"));
        }
    }
}

/// 全局急停（03-security-permissions.md §13 第一层）：取消 Chief 全部
/// 进行中任务。幂等；不清转录与审计。返回停止数。
pub fn chief_stop_all(app: &AppHandle) -> Result<usize, String> {
    let registry = staff::load();
    let mut staff_sessions: HashSet<String> = registry
        .staff
        .iter()
        .map(|profile| profile.session_id.clone())
        .filter(|session| !session.is_empty())
        .collect();
    let staff_ids: HashSet<String> = registry
        .staff
        .iter()
        .map(|profile| profile.staff_id.clone())
        .collect();
    if !registry.staff.is_empty() {
        staff::with_registry(|registry| {
            registry.paused_until_user_resumes = true;
            for profile in &mut registry.staff {
                profile.policy.paused_until_user_resumes = true;
                profile.updated_at = now_ms();
            }
            Ok(())
        })?;
    }
    // Persistent pause is an emergency boundary, not only a task-queue flag.
    // Revoke every shared Computer Use window so an already-authorized action
    // cannot continue after `/stop`.
    crate::computer::revoke_all();
    let config = store::load_config();
    if staff_sessions.is_empty() {
        if registry.legacy_migration_completed_at > 0 {
            return Ok(0);
        }
        let Some(chief_sid) = config.chief_session_id.clone() else {
            return Ok(0);
        };
        staff_sessions.insert(chief_sid);
    }
    let active: Vec<Task> = store::load_tasks()
        .into_iter()
        .filter(|task| {
            staff_sessions.contains(&task.chief_session_id)
                || task
                    .staff_id
                    .as_ref()
                    .is_some_and(|staff_id| staff_ids.contains(staff_id))
        })
        .filter(|task| {
            matches!(
                task.status,
                TaskStatus::Proposed
                    | TaskStatus::Starting
                    | TaskStatus::Running
                    | TaskStatus::WaitingUser
            )
        })
        .collect();
    let mut stopped = 0usize;
    let mut persist_error: Option<String> = None;
    for task in active {
        if let Some(worker_sid) = task.worker_session_id.as_deref() {
            runner::cancel_turn(app, worker_sid);
            let _ = runner::wait_turn_idle_checked(app, worker_sid, 8_000, || true);
            let _ = crate::browser::close_tabs_for_session(app, worker_sid);
        }
        match finalize_task(
            app,
            &task.chief_session_id,
            &task.task_id,
            TaskStatus::Cancelled,
            Some("panic_stop"),
            Some("全局急停。"),
        ) {
            Ok(true) => stopped += 1,
            Ok(false) => {}
            Err(error) => {
                if persist_error.is_none() {
                    persist_error = Some(format!("任务 {} 取消失败：{error}", task.task_id));
                }
            }
        }
    }
    match persist_error {
        Some(error) => Err(format!("已停止 {stopped} 个，但仍有任务账本写入失败：{error}")),
        None => Ok(stopped),
    }
}

/// Stop only one dismissed employee's active tasks. The task ledger remains as
/// audit history, but no worker is left running after its owner is removed.
pub fn chief_cancel_staff_tasks(
    app: &AppHandle,
    staff_id: &str,
    chief_session_id: &str,
    reason: &str,
) -> Result<usize, String> {
    let active: Vec<Task> = store::load_tasks()
        .into_iter()
        .filter(|task| {
            task.staff_id.as_deref() == Some(staff_id)
                || task.chief_session_id == chief_session_id
        })
        .filter(|task| {
            matches!(
                task.status,
                TaskStatus::Proposed
                    | TaskStatus::Starting
                    | TaskStatus::Running
                    | TaskStatus::WaitingUser
            )
        })
        .collect();
    let mut stopped = 0usize;
    let mut pending_error: Option<String> = None;
    for task in active {
        if let Some(worker_sid) = task.worker_session_id.as_deref() {
            runner::cancel_turn(app, worker_sid);
            let _ = runner::wait_turn_idle_checked(app, worker_sid, 8_000, || true);
            let _ = crate::browser::close_tabs_for_session(app, worker_sid);
        }
        match finalize_task(
            app,
            &task.chief_session_id,
            &task.task_id,
            TaskStatus::Cancelled,
            Some("staff_dismissed"),
            Some(reason),
        ) {
            Ok(true) => stopped += 1,
            Ok(false) => {}
            Err(error) => {
                if pending_error.is_none() {
                    pending_error = Some(format!("任务 {} 取消失败：{error}", task.task_id));
                }
            }
        }
    }
    match pending_error {
        Some(error) => Err(format!("员工仍有活动任务未确认取消：{error}")),
        None => Ok(stopped),
    }
}

pub enum InboundOutcome {
    /// 已作为新回合发给 Chief。
    Sent,
    /// Chief 忙，消息已排队（心跳空闲时发）。
    Queued,
}

/// Telegram 裸文本 → Chief 会话。mode != Chief 时返回 Err（由调用方回落
/// relay 路径）。
pub fn chief_inbound_text(app: &AppHandle, text: &str) -> Result<InboundOutcome, String> {
    let config = store::load_config();
    if config.mode != RemoteWorkMode::Chief {
        return Err("chief mode off".to_string());
    }
    let (chief_session_id, _created) = ensure_chief_session(app)?;
    inbound_to_session(app, &chief_session_id, text)
}

/// Telegram 私聊 → 绑定员工（档案 telegramChatId）；未绑定则使用第一位员工。
pub fn chief_inbound_text_for_chat(
    app: &AppHandle,
    chat_id: i64,
    text: &str,
) -> Result<InboundOutcome, String> {
    let config = store::load_config();
    if config.mode != RemoteWorkMode::Chief
        && !staff_commands::telegram_chat_routes_to_staff(chat_id)
    {
        return Err("chief mode off".to_string());
    }
    let Some(profile) = staff_commands::profile_for_chat(chat_id) else {
        return Err("这个 Telegram 私聊还没有绑定员工；请先在本地员工档案里绑定。".to_string());
    };
    let session_id =
        staff_commands::chief_staff_ensure_session(app.clone(), profile.staff_id.clone())?;
    register_chief_session(&session_id);
    let _ = tee::register(&session_id);
    ensure_chief_started(app, &session_id)?;
    inbound_to_session(app, &session_id, text)
}

fn inbound_to_session(
    app: &AppHandle,
    chief_session_id: &str,
    text: &str,
) -> Result<InboundOutcome, String> {
    ensure_chief_started(app, &chief_session_id)?;
    if !runner::turn_idle_now(app, chief_session_id) {
        queue_user_text(chief_session_id, text);
        return Ok(InboundOutcome::Queued);
    }
    match runner::send_prompt(app, chief_session_id, text) {
        Ok(_) => Ok(InboundOutcome::Sent),
        Err(error) if error.contains("already in flight") => {
            queue_user_text(chief_session_id, text);
            Ok(InboundOutcome::Queued)
        }
        Err(error) => Err(error),
    }
}

// ── 心跳：唤醒 flush / 用户队列 flush / 超时 / 失联检测 ───────────────────

static HEARTBEAT: OnceLock<()> = OnceLock::new();
static HEARTBEAT_TICKS: AtomicU64 = AtomicU64::new(0);
/// 懒启动失败只记一次 warn（成功后复位），避免未登录 agent 每 2s 刷屏。
static CHIEF_LAZYSTART_WARNED: AtomicBool = AtomicBool::new(false);

fn heartbeat_staff(app: &AppHandle, profile: &staff::StaffProfile) {
    let chief_sid = profile.session_id.as_str();
    if chief_sid.is_empty() {
        return;
    }
    if profile.bootstrap_version < staff_commands::CURRENT_BOOTSTRAP_VERSION
        && pending_user_count(chief_sid) == 0
        && runner::turn_idle_now(app, chief_sid)
    {
        staff_commands::maybe_queue_bootstrap(profile);
    }
    let pending_work =
        pending_user_count(chief_sid) > 0 || pending_wake_count(chief_sid) > 0;
    if !runner::session_alive(app, chief_sid) {
        if pending_work {
            if let Err(error) = ensure_chief_started(app, chief_sid) {
                if !CHIEF_LAZYSTART_WARNED.swap(true, Ordering::Relaxed) {
                    crate::debug_log::append(
                        "chief",
                        "warn",
                        chief_sid,
                        "staff heartbeat lazy-start failed; will retry",
                        Some(&error),
                    );
                }
                return;
            }
            CHIEF_LAZYSTART_WARNED.store(false, Ordering::Relaxed);
        } else {
            let running = with_storage(app, |storage| {
                storage
                    .find_session(chief_sid)
                    .ok()
                    .flatten()
                    .map(|session| session.status == "running")
                    .unwrap_or(false)
            });
            if running {
                with_storage(app, |storage| storage.update_session_status(chief_sid, "exited")).ok();
            }
        }
    }

    if !profile.policy.paused_until_user_resumes && runner::turn_idle_now(app, chief_sid) {
        if let Some(text) = pop_bootstrap_text(chief_sid) {
            if runner::send_prompt(app, chief_sid, &text).is_ok() {
                staff_commands::mark_bootstrap_sent(chief_sid);
            } else {
                queue_bootstrap_text(chief_sid, &text);
            }
            return;
        }
    }

    if runner::turn_idle_now(app, chief_sid) {
        if let Some(text) = pop_user_text(chief_sid) {
            if runner::send_prompt(app, chief_sid, &text).is_err() {
                queue_user_text(chief_sid, &text);
            }
            return;
        }
    }

    if !profile.policy.paused_until_user_resumes && runner::turn_idle_now(app, chief_sid) {
        if let Some(text) = pop_host_text(chief_sid) {
            if runner::send_prompt(app, chief_sid, &text).is_err() {
                queue_host_text(chief_sid, &text);
            }
            return;
        }
    }

    if !profile.policy.paused_until_user_resumes && runner::turn_idle_now(app, chief_sid) {
        let batch = wake_inbox().lock().ok().and_then(|mut inbox| {
            let entry = inbox.get_mut(chief_sid)?;
            if entry.is_empty() {
                return None;
            }
            Some(entry.drain(..5.min(entry.len())).collect::<Vec<_>>())
        });
        if let Some(events) = batch {
            let text = events
                .iter()
                .map(|event| host_event_text(event, store::find_task(&event.task_id).as_ref()))
                .collect::<Vec<_>>()
                .join("\n");
            match runner::send_prompt(app, chief_sid, &text) {
                Ok(_) => {
                    let ids = events.iter().map(|event| event.event_id.clone()).collect::<Vec<_>>();
                    let _ = store::mark_events_delivered(&ids, now_ms());
                }
                Err(error) => {
                    if let Ok(mut inbox) = wake_inbox().lock() {
                        let entry = inbox.entry(chief_sid.to_string()).or_default();
                        for event in events.into_iter().rev() {
                            entry.insert(0, event);
                        }
                    }
                    crate::debug_log::append(
                        "chief",
                        "warn",
                        chief_sid,
                        "staff wake flush failed; events re-queued",
                        Some(&error),
                    );
                }
            }
        }
    }

    let tasks = store::load_tasks();
    for task in tasks {
        if !task_belongs_to_session(&task, chief_sid) {
            continue;
        }
        if !matches!(
            task.status,
            TaskStatus::Starting | TaskStatus::Running | TaskStatus::WaitingUser
        ) {
            continue;
        }
        let Some(project_policy) = profile.policy.policy_for(&task.project_id) else {
            continue;
        };
        let max_minutes = profile
            .policy
            .max_wall_minutes_per_task
            .min(project_policy.max_task_minutes);
        let stale = now_ms().saturating_sub(task.last_event_at) > (max_minutes as u64) * 60_000;
        let worker_gone = task
            .worker_session_id
            .as_deref()
            .map(|sid| runner::process_gone(app, sid))
            .unwrap_or(false);
        if worker_gone && task.status == TaskStatus::Running {
            let _ = finalize_task(
                app,
                chief_sid,
                &task.task_id,
                TaskStatus::Interrupted,
                Some("worker_gone"),
                Some("worker 进程失联，任务标为中断——可用 task_resume 继续。"),
            );
        } else if stale {
            if let Some(worker_sid) = task.worker_session_id.as_deref() {
                runner::cancel_turn(app, worker_sid);
                let _ = runner::wait_turn_idle_checked(app, worker_sid, 8_000, || true);
            }
            let _ = finalize_task(
                app,
                chief_sid,
                &task.task_id,
                TaskStatus::Failed,
                Some("timeout"),
                Some(&format!("任务超过 {max_minutes} 分钟无进展，已停止。")),
            );
        }
    }
}

/// 启动心跳线程（幂等）。tick 2s。
pub fn ensure_heartbeat(app: AppHandle) {
    HEARTBEAT.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("chief-heartbeat".to_string())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                HEARTBEAT_TICKS.fetch_add(1, Ordering::Relaxed);
                heartbeat_tick(&app);
            });
    });
}

fn heartbeat_tick(app: &AppHandle) {
    let staff_registry = staff::load();
    if !staff_registry.staff.is_empty() {
        let now = now_ms();
        let _ = super::commitments::mark_due(now);
        let due = super::commitments::due(now);
        for profile in &staff_registry.staff {
            if !profile.policy.paused_until_user_resumes
                && profile.policy.allow_background_work
                && profile.policy.autonomy != staff::StaffAutonomy::Manual
            {
                for commitment in due
                    .iter()
                    .filter(|commitment| commitment.staff_id == profile.staff_id)
                {
                    if claim_due_commitment(&commitment.commitment_id) {
                        queue_host_text(
                            &profile.session_id,
                            &format!(
                                "[HOST COMMITMENT DUE]\ncommitmentId: {}\nkind: {:?}\ndueAt: {}\n{}",
                                commitment.commitment_id,
                                commitment.kind,
                                commitment.due_at,
                                commitment.text
                            ),
                        );
                    }
                }
            }
            heartbeat_staff(app, profile);
        }
        return;
    }
    let config = store::load_config();
    if config.mode != RemoteWorkMode::Chief {
        return;
    }
    let Some(chief_sid) = config.chief_session_id.clone() else {
        return;
    };
    // Chief 进程不在时的两种处理：
    // - 有待办（用户队列 / 唤醒收件箱）→ 懒启动重连（「下一条 Chief 消息」
    //   语义；实测抓到的坑：turn_idle 对缺席进程返回 true，不先启动的话
    //   flush 直接 send_prompt 会报 not running 并每 2s 无限重试）；
    // - 没有待办 → 只把账面 running 放回 exited（空转不养 agent 进程），
    //   照常做 worker 超时/失联检测。
    let pending_work =
        pending_user_count(&chief_sid) > 0 || pending_wake_count(&chief_sid) > 0;
    if !runner::session_alive(app, &chief_sid) {
        if pending_work {
            if let Err(error) = ensure_chief_started(app, &chief_sid) {
                if !CHIEF_LAZYSTART_WARNED.swap(true, Ordering::Relaxed) {
                    crate::debug_log::append(
                        "chief",
                        "warn",
                        &chief_sid,
                        "heartbeat chief lazy-start failed; will retry",
                        Some(&error),
                    );
                }
                return;
            }
            CHIEF_LAZYSTART_WARNED.store(false, Ordering::Relaxed);
        } else {
            let running = with_storage(app, |storage| {
                storage
                    .find_session(&chief_sid)
                    .ok()
                    .flatten()
                    .map(|session| session.status == "running")
                    .unwrap_or(false)
            });
            if running {
                with_storage(app, |storage| {
                    storage.update_session_status(&chief_sid, "exited")
                })
                .ok();
            }
        }
    }

    // 用户消息 flush（优先于唤醒——人在说话，事件等下一轮）。
    if runner::turn_idle_now(app, &chief_sid) {
        if let Some(text) = pop_user_text(&chief_sid) {
            let _ = runner::send_prompt(app, &chief_sid, &text);
            return; // 一 tick 只发一条
        }
    }

    // 唤醒 flush：合并最多 5 条事件成一条 host prompt。
    if runner::turn_idle_now(app, &chief_sid) {
        let batch = wake_inbox().lock().ok().and_then(|mut inbox| {
            let entry = inbox.get_mut(&chief_sid)?;
            if entry.is_empty() {
                return None;
            }
            let take: Vec<TaskEvent> = entry.drain(..5.min(entry.len())).collect();
            Some(take)
        });
        if let Some(events) = batch {
            let text = events
                .iter()
                .map(|event| {
                    let task = store::find_task(&event.task_id);
                    host_event_text(event, task.as_ref())
                })
                .collect::<Vec<_>>()
                .join("\n");
            match runner::send_prompt(app, &chief_sid, &text) {
                Ok(_) => {
                    let ids: Vec<String> = events.iter().map(|e| e.event_id.clone()).collect();
                    let _ = store::mark_events_delivered(&ids, now_ms());
                }
                Err(error) => {
                    // 事件放回收件箱，下一 tick 重试。
                    if let Ok(mut inbox) = wake_inbox().lock() {
                        let entry = inbox.entry(chief_sid.clone()).or_default();
                        for event in events.into_iter().rev() {
                            entry.insert(0, event);
                        }
                    }
                    crate::debug_log::append(
                        "chief",
                        "warn",
                        &chief_sid,
                        "wake flush failed; events re-queued",
                        Some(&error),
                    );
                }
            }
        }
    }

    // worker 超时与失联检测。
    let tasks = store::load_tasks();
    for task in tasks {
        if task.chief_session_id != chief_sid {
            continue;
        }
        match task.status {
            TaskStatus::Starting | TaskStatus::Running | TaskStatus::WaitingUser => {}
            _ => continue,
        }
        let Some(policy) = config.policy_for(&task.project_id) else { continue };
        let max_ms = (policy.max_task_minutes as u64) * 60_000;
        let stale = now_ms().saturating_sub(task.last_event_at) > max_ms;
        let worker_gone = task
            .worker_session_id
            .as_deref()
            .map(|sid| runner::process_gone(app, sid))
            .unwrap_or(false);
        if worker_gone && task.status == TaskStatus::Running {
            let _ = finalize_task(app, &chief_sid, &task.task_id, TaskStatus::Interrupted,
                Some("worker_gone"),
                Some("worker 进程失联，任务标为中断——可用 task_resume 继续。"));
        } else if stale {
            if let Some(worker_sid) = task.worker_session_id.as_deref() {
                runner::cancel_turn(app, worker_sid);
                let _ = runner::wait_turn_idle_checked(app, worker_sid, 8_000, || true);
            }
            let _ = finalize_task(app, &chief_sid, &task.task_id, TaskStatus::Failed,
                Some("timeout"),
                Some(&format!("任务超过 {} 分钟无进展，已停止。", policy.max_task_minutes)));
        }
    }
}

/// 测试口：注册表与队列清空（进程级静态，测试必须清理）。
#[cfg(test)]
pub(crate) fn reset_for_tests() {
    if let Ok(mut set) = chief_sessions().lock() {
        set.clear();
    }
    if let Ok(mut inbox) = wake_inbox().lock() {
        inbox.clear();
    }
    if let Ok(mut queues) = user_queues().lock() {
        queues.clear();
    }
}

#[cfg(test)]
mod tests;
