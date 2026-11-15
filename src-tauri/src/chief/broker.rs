//! Chief 编排核心：任务生命周期、Chief 会话注册表、唤醒收件箱、心跳。
//!
//! 红线（docs/research-telegram-chief/）：
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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use tauri::{AppHandle, Manager};

use super::policy::{self, Decision};
use super::runner;
use super::store;
use super::tee;
use super::types::{now_ms, short_id, ChiefConfig, RemoteMode, RemoteWorkMode, Task, TaskEvent, TaskStatus};

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

/// 该会话是否是 Chief：运行时注册表 ∪ config.chief_session_id。
/// 这是控制工具可见性的**唯一**判据；普通 worker 永远不在这里。
pub fn is_chief_session(session_id: &str) -> bool {
    if let Ok(set) = chief_sessions().lock() {
        if set.contains(session_id) {
            return true;
        }
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

fn wake_inbox() -> &'static Mutex<HashMap<String, Vec<TaskEvent>>> {
    WAKE_INBOX.get_or_init(|| Mutex::new(HashMap::new()))
}

fn user_queues() -> &'static Mutex<HashMap<String, VecDeque<String>>> {
    USER_QUEUE.get_or_init(|| Mutex::new(HashMap::new()))
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

pub(crate) fn pending_user_count(chief_session_id: &str) -> usize {
    user_queues()
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

fn pop_user_text(chief_session_id: &str) -> Option<String> {
    let mut queues = user_queues().lock().ok()?;
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
    let mut config = store::load_config();
    if let Some(sid) = config.chief_session_id.clone() {
        let exists = with_storage(app, |storage| {
            storage.find_session(&sid).ok().flatten().is_some()
        });
        if exists {
            register_chief_session(&sid);
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
    crate::debug_log::append(
        "chief",
        "info",
        &session.id,
        "chief session created",
        Some(&format!("agent={agent_id}")),
    );
    register_chief_session(&session.id);
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
        "title": task.title,
        "projectId": task.project_id,
        "workerSessionId": task.worker_session_id,
        "agentId": task.agent_id,
        "model": task.model,
        "status": task.status,
        "lastSummary": task.last_summary,
        "error": task.error,
        "createdAt": task.created_at,
        "updatedAt": task.updated_at,
    })
}

/// active = Starting / Running / WaitingUser（Chief 自身不算 worker 并发）。
fn count_active(tasks: &[Task], chief_session_id: &str, project_id: Option<&str>) -> usize {
    tasks
        .iter()
        .filter(|task| task.chief_session_id == chief_session_id)
        .filter(|task| {
            matches!(
                task.status,
                TaskStatus::Starting | TaskStatus::Running | TaskStatus::WaitingUser
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
    let config = store::load_config();
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
    let config = store::load_config();
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
        .filter(|task| task.chief_session_id == chief_session_id)
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

    // 幂等：同一 requestId 返回同一任务。
    if let Some(existing) = store::find_by_request_id(request_id) {
        if existing.chief_session_id == chief_session_id {
            return envelope_ok(json!({
                "task": task_card(&existing),
                "existing": true,
                "note": "同一 requestId 的任务已存在，返回原任务（幂等）。",
            }));
        }
    }

    let config = store::load_config();
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
    let max_minutes = runtime_minutes.unwrap_or(policy_minutes);

    // 并发上限（全局 + 项目级）：超限明确拒绝，不悄悄丢弃。
    let tasks_now = store::load_tasks();
    let global_cap = config.clamped_max_workers() as usize;
    let global_now = count_active(&tasks_now, chief_session_id, None);
    let project_cap = config
        .policy_for(&project_id)
        .map(|policy| policy.max_concurrent_workers as usize)
        .unwrap_or(1);
    let project_now = count_active(&tasks_now, chief_session_id, Some(&project_id));
    if global_now >= global_cap {
        return envelope_reject(
            "concurrency_limit",
            &format!("并发上限 {global_cap} 已满（当前 {global_now} 个进行中）。等任务完成或用 task_cancel 腾位。"),
        );
    }
    if project_now >= project_cap {
        return envelope_reject(
            "concurrency_limit",
            &format!("项目并发上限 {project_cap} 已满（该项目当前 {project_now} 个进行中）。"),
        );
    }

    let now = now_ms();
    let task = Task {
        task_id: short_id("task"),
        request_id: request_id.to_string(),
        chief_session_id: chief_session_id.to_string(),
        title,
        goal,
        project_id: project_id.clone(),
        worker_session_id: None,
        agent_id: agent_id.clone(),
        model: model.clone(),
        status: TaskStatus::Proposed,
        created_at: now,
        updated_at: now,
        last_event_at: now,
        last_summary: None,
        stop_reason: None,
        error: None,
        telegram_message_id: None,
        artifacts: Vec::new(),
    };
    let created_event = emit_event(&task, "task.created", TaskStatus::Proposed, None);
    if let Err(error) = store::append_events(&[created_event]) {
        return envelope_reject("internal", &format!("任务事件落盘失败：{error}"));
    }
    let task_id = task.task_id.clone();
    if let Err(error) = store::with_tasks(|tasks| {
        tasks.insert(0, task.clone());
        Ok(())
    }) {
        return envelope_reject("internal", &format!("任务快照写入失败：{error}"));
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
    // 1) Proposed → Starting
    let task = match store::with_tasks(|tasks| {
        let Some(task) = tasks.iter_mut().find(|t| t.task_id == task_id) else {
            return Err("task vanished".to_string());
        };
        if !task.status.can_transition(TaskStatus::Starting) {
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
            finalize_task(app, chief_session_id, task_id, TaskStatus::Failed, None,
                Some(&format!("创建 worker 会话失败：{error}")));
            return;
        }
    };
    let _ = tee::register(&worker.id);

    // 3) 启动 ACP + 应用模型
    if let Err(error) = runner::ensure_session(app, &worker.id, &task.agent_id, &worker.cwd) {
        runner::stop_session(app, &worker.id);
        tee::unregister(&worker.id);
        finalize_task(app, chief_session_id, task_id, TaskStatus::Failed, None,
            Some(&format!("Agent 启动失败：{error}")));
        return;
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
            finalize_task(app, chief_session_id, task_id, TaskStatus::Failed, None,
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
    notify(&task, "task.started", None);

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
            finalize_task(app, chief_session_id, task_id, TaskStatus::Failed,
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
    finalize_task(app, chief_session_id, task_id, TaskStatus::Completed, None, Some(&combined));
}

/// 任务收尾：状态转移 + 事件 + Telegram 卡 + 唤醒收件箱。幂等（终态跳过）。
fn finalize_task(
    app: &AppHandle,
    chief_session_id: &str,
    task_id: &str,
    status: TaskStatus,
    stop_reason: Option<&str>,
    summary: Option<&str>,
) {
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
        task.updated_at = now_ms();
        task.last_event_at = now_ms();
        task.stop_reason = stop_reason.map(str::to_string);
        task.last_summary = summary.map(str::to_string);
        Ok(Some(task.clone()))
    });
    let Some(task) = finalized.ok().flatten() else { return };
    let kind = match status {
        TaskStatus::Completed => "task.completed",
        TaskStatus::Failed => "task.failed",
        TaskStatus::Cancelled => "task.cancelled",
        TaskStatus::Interrupted => "task.interrupted",
        _ => "task.progress",
    };
    let event = emit_event(&task, kind, status, task.last_summary.clone());
    let _ = store::append_events(&[event.clone()]);
    notify(&task, kind, task.last_summary.as_deref());
    queue_wake(chief_session_id, event);
    // 终态任务把 worker 会话状态放回 exited（UI 列表不再显示运行中）。
    if status.is_terminal() {
        if let Some(worker_sid) = task.worker_session_id.as_deref() {
            with_storage(app, |storage| {
                storage.update_session_status(worker_sid, "exited")
            })
            .ok();
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
    if !command_id.is_empty() && !mark_sent_command(task_id, command_id) {
        return envelope_ok(json!({
            "delivered": true,
            "existing": true,
            "note": "同一 commandId 已送达过（幂等跳过）。",
        }));
    }
    let Some(task) = store::find_task(task_id) else {
        return envelope_reject("task_not_found", "任务不存在（或不属于这个 Chief）。");
    };
    if let Err((error, note)) = policy::validate_task_access(&task, chief_session_id, "send") {
        return envelope_reject(error, &note);
    }
    let Some(worker_sid) = task.worker_session_id.clone() else {
        return envelope_reject("task_not_running", "worker 还没启动。");
    };
    if urgency == "interrupt" {
        runner::cancel_turn(app, &worker_sid);
        let _ = runner::wait_turn_idle_checked(app, &worker_sid, 8_000, || true);
    }
    match runner::send_prompt(app, &worker_sid, message) {
        Ok(_) => {
            let _ = store::with_tasks(|tasks| {
                if let Some(task) = tasks.iter_mut().find(|t| t.task_id == task_id) {
                    task.last_event_at = now_ms();
                }
                Ok(())
            });
            envelope_ok(json!({ "delivered": true }))
        }
        Err(error) if error.contains("already in flight") => envelope_reject(
            "worker_busy",
            "worker 正在回合中。可用 urgency=interrupt 打断，或稍后再发。",
        ),
        Err(error) => envelope_reject("deliver_failed", &error),
    }
}

pub fn chief_task_status(chief_session_id: &str, task_id: &str) -> Value {
    let Some(task) = store::find_task(task_id) else {
        return envelope_reject("task_not_found", "任务不存在。");
    };
    if task.chief_session_id != chief_session_id {
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
    if let Err((error, note)) = policy::validate_task_access(&task, chief_session_id, "cancel") {
        return envelope_reject(error, &note);
    }
    if task.status == TaskStatus::Cancelled {
        return envelope_ok(json!({ "task": task_card(&task), "note": "已取消（幂等）。" }));
    }
    if let Some(worker_sid) = task.worker_session_id.as_deref() {
        runner::cancel_turn(app, worker_sid);
        let _ = runner::wait_turn_idle_checked(app, worker_sid, 8_000, || true);
    }
    finalize_task(app, chief_session_id, task_id, TaskStatus::Cancelled, Some("cancelled"), Some(reason));
    envelope_ok(json!({ "task": store::find_task(task_id).map(|t| task_card(&t)) }))
}

pub fn chief_task_collect(chief_session_id: &str, args: &Value) -> Value {
    let Some(task_id) = args.get("taskId").and_then(Value::as_str) else {
        return envelope_reject("bad_request", "taskId 必填。");
    };
    let Some(task) = store::find_task(task_id) else {
        return envelope_reject("task_not_found", "任务不存在。");
    };
    if task.chief_session_id != chief_session_id {
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
    if task.chief_session_id != chief_session_id {
        return envelope_reject("task_not_owned", "任务不属于这个 Chief。");
    }
    if !task.status.can_transition(TaskStatus::Starting) {
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
    // 标 Starting（resume 转移），后台重启进程并重发指令。
    if let Err(error) = store::with_tasks(|tasks| {
        let Some(t) = tasks.iter_mut().find(|t| t.task_id == task_id) else {
            return Err("task vanished".to_string());
        };
        if !t.status.can_transition(TaskStatus::Starting) {
            return Err(format!("状态 {} 不能恢复", format!("{:?}", t.status).to_lowercase()));
        }
        t.status = TaskStatus::Starting;
        t.updated_at = now_ms();
        t.last_event_at = now_ms();
        Ok(())
    }) {
        return envelope_reject("internal", &error);
    }
    let config = store::load_config();
    let (remote_mode, max_minutes, self_modification) = config
        .policy_for(&task.project_id)
        .map(|policy| (policy.remote_mode, policy.max_task_minutes, policy.self_modification))
        .unwrap_or((RemoteMode::ApproveWrites, 120, false));
    let app_handle = app.clone();
    let chief_sid = chief_session_id.to_string();
    let resume_task_id = task_id.to_string();
    let resume_worker = worker_sid.clone();
    let _ = std::thread::Builder::new()
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
        });
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
    let cwd = with_storage(app, |storage| {
        storage
            .find_session(worker_session_id)
            .ok()
            .flatten()
            .map(|session| session.cwd)
    })
    .unwrap_or_else(|| task.goal.clone());
    if let Err(error) = runner::ensure_session(app, worker_session_id, &task.agent_id, &cwd) {
        finalize_task(app, chief_session_id, task_id, TaskStatus::Failed, None,
            Some(&format!("恢复失败：{error}")));
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
            finalize_task(app, chief_session_id, task_id, TaskStatus::Failed, None,
                Some(&format!("恢复下发失败：{error}")));
            return;
        }
    }
    let _ = store::with_tasks(|tasks| {
        if let Some(t) = tasks.iter_mut().find(|t| t.task_id == task_id) {
            t.status = TaskStatus::Running;
            t.updated_at = now_ms();
            t.last_event_at = now_ms();
        }
        Ok(())
    });
    watch_worker_loop(app, chief_session_id, task_id, worker_session_id, max_minutes);
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
pub fn chief_stop_all(app: &AppHandle) -> usize {
    let config = store::load_config();
    let Some(chief_sid) = config.chief_session_id.clone() else {
        return 0;
    };
    let active: Vec<Task> = store::load_tasks()
        .into_iter()
        .filter(|task| task.chief_session_id == chief_sid)
        .filter(|task| {
            matches!(
                task.status,
                TaskStatus::Starting | TaskStatus::Running | TaskStatus::WaitingUser
            )
        })
        .collect();
    let mut stopped = 0usize;
    for task in active {
        if let Some(worker_sid) = task.worker_session_id.as_deref() {
            runner::cancel_turn(app, worker_sid);
            let _ = runner::wait_turn_idle_checked(app, worker_sid, 8_000, || true);
        }
        finalize_task(app, &chief_sid, &task.task_id, TaskStatus::Cancelled,
            Some("panic_stop"), Some("全局急停。"));
        stopped += 1;
    }
    stopped
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
    ensure_chief_started(app, &chief_session_id)?;
    if !runner::turn_idle_now(app, &chief_session_id) {
        queue_user_text(&chief_session_id, text);
        return Ok(InboundOutcome::Queued);
    }
    match runner::send_prompt(app, &chief_session_id, text) {
        Ok(_) => Ok(InboundOutcome::Sent),
        Err(error) if error.contains("already in flight") => {
            queue_user_text(&chief_session_id, text);
            Ok(InboundOutcome::Queued)
        }
        Err(error) => Err(error),
    }
}

// ── 心跳：唤醒 flush / 用户队列 flush / 超时 / 失联检测 ───────────────────

static HEARTBEAT: OnceLock<()> = OnceLock::new();
static HEARTBEAT_TICKS: AtomicU64 = AtomicU64::new(0);

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
    let config = store::load_config();
    if config.mode != RemoteWorkMode::Chief {
        return;
    }
    let Some(chief_sid) = config.chief_session_id.clone() else {
        return;
    };
    // Chief 进程死了但账面 running：状态回 exited（下次消息懒启动重连）。
    if !runner::session_alive(app, &chief_sid) {
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
            finalize_task(app, &chief_sid, &task.task_id, TaskStatus::Interrupted,
                Some("worker_gone"),
                Some("worker 进程失联，任务标为中断——可用 task_resume 继续。"));
        } else if stale {
            if let Some(worker_sid) = task.worker_session_id.as_deref() {
                runner::cancel_turn(app, worker_sid);
                let _ = runner::wait_turn_idle_checked(app, worker_sid, 8_000, || true);
            }
            finalize_task(app, &chief_sid, &task.task_id, TaskStatus::Failed,
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
