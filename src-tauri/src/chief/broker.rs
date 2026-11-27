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
        let queue = queues.entry(chief_session_id.to_string()).or_default();
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
        match update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or("")
        {
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

// ── 回合结局分类（第二期「可信」+ 第三期「过夜」） ────────────────────────
//
// watch 循环在回合空闲时不再直接判「完成」：先从 tee 事件流里取最后一个
// turn/complete 的 stopReason 与错误文本，分成四类——干净完成 / 额度类
// 瞬时失败（排队自动重试）/ 拒答 / 其它错误。连同「零产出」检查一起，
// 消灭「回合报错也被记 completed」的旧口径。

/// 卡死判定：Running 且连续 STALL_MS 无任何事件（touch_worker_progress
/// 会随流式事件 5s 节流刷新 last_event_at）。Mingle 的「20 分钟无声且
/// 有人在等 → 干预」梯子取前两级：先 cancel，30s 宽限仍无声才判死。
const WORKER_STALL_MS: u64 = 20 * 60 * 1000;
const WORKER_STALL_GRACE_MS: u64 = 30 * 1000;
/// 额度/限流类瞬时失败的重试退避（第 1/2/3 次）；超过 3 次判 retry_exhausted。
const RETRY_BACKOFF_MS: [u64; 3] = [5 * 60 * 1000, 15 * 60 * 1000, 30 * 60 * 1000];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TurnFailureClass {
    Clean,
    /// 额度/限流/服务繁忙——可自愈，排队自动重试。
    Transient,
    Refused,
    /// 用户或宿主中止了这个回合（停止按钮 / 取消 / 中止卡死）。
    Cancelled,
    TurnError,
}

/// 从 tee 事件流提取回合结局 (stopReason, 错误文本)。
/// 纯函数（broker 测试直接喂事件）。
///
/// 「被取消」看整条流而不只看最后一条：`AcpService::cancel` 在取消那一刻
/// 就补发一条 `cancelled`，而 agent 之后可能还会再报一条自己的结局——
/// 只认最后一条会把一次中止当成正常完成。一个任务的观测循环里，被取消
/// 的回合必然是最后一个回合（取消即收尾），所以这个判定不会误伤更早的回合。
pub(crate) fn last_turn_outcome(events: &[Value]) -> (Option<String>, Option<String>) {
    let mut stop_reason = None;
    let mut error_text = None;
    for event in events {
        let method = event.get("method").and_then(Value::as_str).unwrap_or("");
        if method != "turn/complete" {
            continue;
        }
        let data = event.get("data").unwrap_or(&Value::Null);
        stop_reason = data
            .get("stopReason")
            .and_then(Value::as_str)
            .map(str::to_string);
        // 错误文本的形状随 emit 点不同：data.error 可能是字符串、
        // {message} 对象，或嵌在 data.data 里——逐层找，找到为止。
        error_text = extract_error_text(data);
        if stop_reason.as_deref() == Some("cancelled") {
            break;
        }
    }
    (stop_reason, error_text)
}

fn extract_error_text(data: &Value) -> Option<String> {
    for holder in [data, data.get("data").unwrap_or(&Value::Null)] {
        if holder.is_null() {
            continue;
        }
        if let Some(error) = holder.get("error") {
            if let Some(text) = error.as_str() {
                if !text.trim().is_empty() {
                    return Some(text.to_string());
                }
            } else if let Some(message) = error.get("message").and_then(Value::as_str) {
                if !message.trim().is_empty() {
                    return Some(message.to_string());
                }
            }
        }
    }
    None
}

/// 额度/限流/繁忙的匹配表（小写包含）。命中即 Transient——这类失败的特点
/// 是「等一会儿就好」，Mingle 的失败分类查表同款思路。
///
/// 刻意**不认裸的数字 `429`**：它和 `main.rs:429:3`、退出码 `4290` 没法
/// 区分，一次普通失败被误判成限流会让任务白白午休三次。真实的限流报错
/// 都带着 "Too Many Requests" / "rate limit" 这类字样，认字样就够。
pub(crate) fn classify_turn_failure(
    stop_reason: Option<&str>,
    error_text: Option<&str>,
) -> TurnFailureClass {
    // 中止优先于一切：用户按了停止，就不能被错误文本里的字样改判成别的。
    if stop_reason == Some("cancelled") {
        return TurnFailureClass::Cancelled;
    }
    let haystack =
        format!("{} {}", stop_reason.unwrap_or(""), error_text.unwrap_or("")).to_ascii_lowercase();
    const TRANSIENT_MARKERS: &[&str] = &[
        "quota",
        "usage limit",
        "rate limit",
        "ratelimit",
        "rate_limit",
        "overloaded",
        "temporarily",
        "try again",
        "too many requests",
        "resource_exhausted",
    ];
    if TRANSIENT_MARKERS
        .iter()
        .any(|marker| haystack.contains(marker))
    {
        return TurnFailureClass::Transient;
    }
    match stop_reason {
        Some("refusal") => TurnFailureClass::Refused,
        // 回合被截断（长度 / 轮数上限）同样不是「完成」。
        Some("error" | "max_tokens" | "max_turn_requests") => TurnFailureClass::TurnError,
        _ => TurnFailureClass::Clean,
    }
}

/// 宿主的判词 + worker 的最后一截输出（有才带）。
fn with_worker_tail(verdict: String, tail: Option<&str>) -> String {
    match tail {
        Some(tail) if !tail.trim().is_empty() => {
            format!("{verdict}\nworker 的最后输出：{tail}")
        }
        _ => verdict,
    }
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
        // （第四期）结构化交付：父会话直接拿到可用 JSON（截 1500 字符，
        // 完整结果在 task.structured_output，可用 delegate_status 取）。
        if let Some(output) = task.structured_output.as_ref() {
            let full = output.to_string();
            let taken: String = full.chars().take(1500).collect();
            let preview = if full.chars().count() > 1500 {
                format!("{taken}…（截断）")
            } else {
                taken
            };
            text.push_str(&format!("output: {preview}\n"));
        }
    }
    text.push_str("untrustedContent: false\n");
    text
}

pub(crate) fn emit_event(
    task: &Task,
    kind: &str,
    status: TaskStatus,
    summary: Option<String>,
) -> TaskEvent {
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
        // 员工自己钉的模型写不进去不致命（员工会话用 agent 默认照常工作）；
        // 派活方点名的模型才是 fail-closed（见 apply_task_settings）。
        let _ = runner::apply_model(app, chief_session_id, model);
    }
    with_storage(app, |storage| {
        storage.update_session_status(chief_session_id, "running")
    })
    .ok();
    Ok(())
}

// ── 任务卡片（envelope 里的 task 形状） ──────────────────────────────────

fn provider_label_from_record(
    provider_id: &str,
    record: Option<&serde_json::Value>,
) -> Option<String> {
    if let Some(record) = record {
        for key in ["catalog", "preset", "id"] {
            let candidate = record
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim();
            if let Some(preset) = crate::model_bridge::find_preset(candidate) {
                return Some(preset.label.to_string());
            }
        }
        if let Some(name) = record
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            return Some(name.to_string());
        }
    }
    crate::model_bridge::find_preset(provider_id).map(|preset| preset.label.to_string())
}

/// Provider/model split for task-card metadata. The provider comes from the
/// engine's registered record, never from the model family, so an OpenCode Go
/// route to DeepSeek cannot masquerade as the official DeepSeek account.
fn task_display_parts(task: &Task) -> (Option<String>, Option<String>) {
    let raw = task
        .bridge
        .as_ref()
        .map(|bridge| bridge.model_id.as_str())
        .or(task.model.as_deref())
        .unwrap_or("")
        .trim();
    if raw.is_empty() {
        return (None, None);
    }
    let Some((provider_id, model_name)) = raw.split_once('/') else {
        return (None, Some(raw.to_string()));
    };
    let record = crate::engine::engine_dir().ok().and_then(|dir| {
        std::fs::read(dir.join("data").join("providers.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|root| {
                root.get("providers")
                    .and_then(serde_json::Value::as_array)
                    .and_then(|rows| {
                        rows.iter().find(|row| {
                            row.get("id").and_then(serde_json::Value::as_str) == Some(provider_id)
                        })
                    })
                    .cloned()
            })
    });
    (
        provider_label_from_record(provider_id, record.as_ref()),
        Some(model_name.to_string()),
    )
}

fn insert_task_display(map: &mut serde_json::Map<String, serde_json::Value>, task: &Task) {
    let (provider, model) = task_display_parts(task);
    map.insert("displayProvider".into(), json!(provider));
    map.insert("displayModelName".into(), json!(model));
    map.insert("displayEffort".into(), json!(task.preferred_effort));
}

/// Full Task plus derived display fields for historical delegated-task cards.
pub(crate) fn task_with_presentation(task: &Task) -> serde_json::Value {
    let mut value = serde_json::to_value(task).unwrap_or_else(|_| json!({}));
    if let Some(map) = value.as_object_mut() {
        insert_task_display(map, task);
    }
    value
}

pub(crate) fn task_card(task: &Task) -> Value {
    let mut card = json!({
        "taskId": task.task_id,
        "staffId": task.staff_id,
        "title": task.title,
        "projectId": task.project_id,
        "workerSessionId": task.worker_session_id,
        "agentId": task.agent_id,
        "model": task.model,
        // 跨用引擎的真实模型 id（UI 用它显示「OpenCode Go / DeepSeek …」，
        // 而不是 Grok 原生的 `marionette/...` 写法）。
        "bridgeModel": task.bridge.as_ref().map(|bridge| bridge.model_id.clone()),
        "displayModel": task
            .bridge
            .as_ref()
            .map(|bridge| bridge.model_id.clone())
            .or_else(|| task.model.clone()),
        "status": task.status,
        "waitingReason": task.waiting_reason,
        "turnCount": task.turn_count,
        "costUsd": task.cost_usd,
        "lastSummary": task.last_summary,
        "error": task.error,
        "createdAt": task.created_at,
        "updatedAt": task.updated_at,
        // 二/三/四/五期字段（旧账无这些键，前端全按可选消费）。
        "retryAt": task.retry_at,
        "retryCount": task.retry_count,
        "hasOutputSchema": task.output_schema.is_some(),
        "structuredOutput": task.structured_output,
        "checkpointBase": task.checkpoint_base_commit,
        "preferredEffort": task.preferred_effort,
    });
    if let Some(map) = card.as_object_mut() {
        insert_task_display(map, task);
    }
    card
}

/// 启动时从事件账本重建唤醒收件箱（第二期「可信」）：`delivered_at=None`
/// 的终态事件重新入队——收件箱原先只在内存里，重启即丢，「没被答的回执」
/// 从此跨重启存活。目标会话按 staff 档案优先、任务 chief 会话兜底
/// （与 finalize_task 同一口径）。剪过的任务（事件已随账本修剪）自然跳过。
pub fn rebuild_wake_inbox_from_ledger() {
    let mut queued = 0usize;
    for event in store::undelivered_wake_events() {
        let Some(task) = store::find_task(&event.task_id) else {
            continue;
        };
        let target = task
            .staff_id
            .as_deref()
            .and_then(staff_commands::profile_for_staff_id)
            .map(|profile| profile.session_id)
            .filter(|session| !session.is_empty())
            .unwrap_or_else(|| task.chief_session_id.clone());
        queue_wake(&target, event);
        queued += 1;
    }
    if queued > 0 {
        crate::debug_log::append(
            "chief",
            "info",
            "",
            "wake inbox rebuilt from ledger",
            Some(&format!("{queued} undelivered events re-queued")),
        );
    }
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

pub fn chief_tasks_list(
    chief_session_id: &str,
    status_filter: Option<&str>,
    limit: usize,
) -> Value {
    let tasks: Vec<Value> = store::load_tasks()
        .into_iter()
        .filter(|task| task_belongs_to_session(task, chief_session_id))
        .filter(|task| match status_filter {
            Some(status) => format!("{:?}", task.status).to_lowercase() == status.to_lowercase(),
            None => true,
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
    let goal = args
        .get("goal")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
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
    if let Some(existing) = loaded_tasks
        .iter()
        .find(|task| task.request_id == request_id)
    {
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
        match policy::validate_task_start(
            &config,
            project_id,
            requested_agent,
            requested_model,
            &agents,
        ) {
            Decision::Grant {
                project_id,
                agent_id,
                model,
                remote_mode,
                max_task_minutes,
                self_modification,
            } => (
                project_id,
                agent_id,
                model,
                remote_mode,
                max_task_minutes,
                self_modification,
            ),
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
        retry_at: None,
        retry_count: 0,
        output_schema: None,
        structured_output: None,
        checkpoint_base_commit: None,
        checkpoint_repo_root: None,
        preferred_effort: None,
        bridge: None,
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
        Some(current) if current.policy.paused_until_user_resumes => Some((
            "staff_paused",
            "员工在任务预留期间被暂停，任务已取消。".to_string(),
        )),
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
                Some(max_minutes),
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

/// 应用任务设置时的致命错误（调用方拒绝启动 / 停掉任务）。
#[derive(Debug)]
pub(crate) enum SettingsError {
    /// 只读天花板强制不了。
    ReadOnly(String),
    /// 调用方**点名**的模型这个 agent 不接受——不偷偷换成默认模型：用户
    /// 要的是免费/便宜的模型，静默落到旗舰是最糟的结果（2026-11-18 那次
    /// 错 id 让 OpenCode 落到 OpenRouter 上花了 $9，同一条红线）。
    Model {
        model: String,
        reason: String,
        /// 这个 agent 自己广告的模型 id——随判词带回给派活的 AI，照清单重派。
        available: Vec<String>,
    },
}

impl SettingsError {
    /// 给父会话 / 面板看的完整判词。
    pub(crate) fn verdict(&self, agent_id: &str) -> String {
        match self {
            SettingsError::ReadOnly(error) => {
                format!("Agent 无法强制只读权限，已拒绝启动任务：{error}")
            }
            SettingsError::Model {
                model,
                reason,
                available,
            } => {
                let mut text =
                    format!("{agent_id} 不接受模型「{model}」（{reason}）。没有改用默认模型。");
                if available.is_empty() {
                    text.push_str("这个 agent 没有广告任何模型清单——用 marionette_agents_list / marionette_models_list 核对，或不指定 model 重派。");
                } else {
                    let shown = available
                        .iter()
                        .take(40)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("、");
                    if available.len() > 40 {
                        text.push_str(&format!(
                            "它接受的模型（共 {} 个，列出前 40 个）：{shown}",
                            available.len()
                        ));
                    } else {
                        text.push_str(&format!("它接受的模型：{shown}"));
                    }
                }
                text
            }
        }
    }
}

/// 应用设置后值得让人看见、但不值得拦任务的事（写进任务行，右栏面板
/// 在它跑动的整段时间里都显示）。
#[derive(Debug, Default)]
pub(crate) struct SettingsNotes {
    pub permission: Option<String>,
    pub warnings: Vec<String>,
}

/// 判词里最多带回这么多模型 id。
const AVAILABLE_MODELS_CAP: usize = 60;

/// agent 回报的当前模型与要求的是否一致。Codex 会把档位编进模型名
/// （`gpt-5.6-luna[xhigh]`），所以去掉尾部 `[...]` 再比。
pub(crate) fn model_echo_matches(requested: &str, current: &str) -> bool {
    fn strip(id: &str) -> &str {
        let id = id.trim();
        // Antigravity gateway 目录可能回显 `models/<id>`。
        let id = id.strip_prefix("models/").unwrap_or(id);
        match (id.find('['), id.ends_with(']')) {
            (Some(index), true) => id[..index].trim(),
            _ => id.trim(),
        }
    }
    strip(requested) == strip(current)
}

/// worker 启动前把跨用引擎绑定写好：Claude 的环境变量、Grok 的 owned 表、
/// Codex 的 providers.set 都在 spawn 阶段读它；OpenCode/DSH/Pi/ZCode 的
/// 全局路由也由各自的 spawn 分支刷新。绑定写失败 = 拒绝启动，绝不静默
/// 落到 agent 自家默认模型。
fn prepare_worker_bridge(
    app: &AppHandle,
    task: &Task,
    worker_session_id: &str,
) -> Result<(), String> {
    let Some(selection) = task.bridge.as_ref() else {
        return Ok(());
    };
    if selection.entry_id == crate::engine::ENGINE_ENTRY_ID {
        crate::engine::wake_if_installed();
        if !crate::engine::bridge_ready() {
            crate::engine::ensure_bridge_ready(std::time::Duration::from_secs(30));
        }
        if !crate::engine::bridge_ready() {
            return Err(crate::ts!(
                "翻译引擎未就绪，无法为 worker 建立跨用模型绑定",
                "The translation engine is not ready; cannot bind the worker's cross-use model"
            ));
        }
        // A reachable loopback port is not the same as a populated catalog.
        // OpenCode/Pi used to launch in this window and reject `marionette/*`
        // or report auth-required before the provider was injected. Wait for
        // the exact bound model (live or last-good cache) first.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let (rows, _) = crate::engine::catalog_direct_or_cached_entries();
            if rows
                .iter()
                .any(|row| row.id.as_str() == selection.model_id.as_str())
            {
                break;
            }
            if std::time::Instant::now() >= deadline {
                return Err(crate::ts!(
                    "翻译引擎目录尚未暴露请求的模型，无法为 worker 建立跨用模型绑定",
                    "The translation engine catalog did not expose the requested model; cannot bind the worker"
                ));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
    crate::model_bridge::bind_session(
        worker_session_id,
        Some(task.agent_id.as_str()),
        Some(selection.clone()),
    )?;
    let _ = app;
    Ok(())
}

/// 把任务的设置应用到一条**活着的** worker 会话上。spawn 与额度重试
/// 复活共用——复活出来的进程是全新的，模型/档位/只读天花板/继承来的
/// 权限档全都要重放，否则重试一次就把它们全丢了。
///
/// 顺序有讲究：只读天花板 → 模型（换模型会重置 agent 侧的档位）→ 档位
/// → 权限继承（Grok 的 `/always-approve on` 自己占一个回合，必须排在
/// 其它设置之后、目标之前）。
/// `Err` = 致命（只读档强制失败 / 点名的模型不被接受）；`Ok` 里的备注
/// 不拦任务。
fn apply_task_settings(
    app: &AppHandle,
    task: &Task,
    worker_session_id: &str,
    remote_mode: RemoteMode,
) -> Result<SettingsNotes, SettingsError> {
    let mut notes = SettingsNotes::default();
    if remote_mode == RemoteMode::ReadOnly {
        runner::enforce_read_only(app, worker_session_id).map_err(SettingsError::ReadOnly)?;
    }
    if let Some(model) = task.model.as_deref().filter(|_| {
        // Claude 的桥接模型由 spawn 环境变量钉住（§4.8：跨用会话不发
        // set_model）；再发一次会被它按自家菜单拒绝，反而把任务判死。
        !(task.bridge.is_some()
            && crate::model_bridge::classify_agent(&task.agent_id)
                == crate::model_bridge::BridgeAgent::ClaudeCode)
    }) {
        let antigravity = task.bridge.is_some()
            && crate::model_bridge::classify_agent(&task.agent_id)
                == crate::model_bridge::BridgeAgent::Antigravity;
        let applied = match runner::apply_model(app, worker_session_id, model) {
            Ok(()) => Ok(()),
            Err(first) if antigravity && !model.starts_with("models/") => {
                // Antigravity 的 gateway 目录可能用 `models/<id>` 形式广告；
                // 第一次裸 id 被拒时再试一次前缀写法，仍失败才判死。
                runner::apply_model(app, worker_session_id, &format!("models/{model}"))
                    .map_err(|second| format!("{first} / {second}"))
            }
            Err(error) => Err(error),
        };
        applied.map_err(|reason| SettingsError::Model {
            model: model.to_string(),
            reason,
            available: advertised_model_ids(app, worker_session_id),
        })?;
        // 写入被接受了，但 agent 回报的当前模型不是它——不拦任务（有的 agent
        // 回报有延迟），只留一句让人看得见。
        let echoed = app
            .state::<crate::AppState>()
            .acp
            .get_capabilities(worker_session_id)
            .and_then(|caps| caps.current_model);
        if let Some(current) = echoed {
            if !model_echo_matches(model, &current) {
                notes.warnings.push(format!(
                    "模型「{model}」已提交，但 agent 回报的当前模型是「{current}」。"
                ));
            }
        }
    }
    if let Some(effort) = task.preferred_effort.as_deref() {
        if let Err(_) = runner::apply_effort(app, worker_session_id, effort) {
            notes.warnings.push(format!(
                "要求的档位「{effort}」没有生效（这个 agent 不接受它），用的是默认档。"
            ));
        } else if runner::effort_will_be_ignored(
            app,
            worker_session_id,
            &task.agent_id,
            task.model.as_deref().unwrap_or(""),
        ) {
            let model = task.model.as_deref().unwrap_or("当前模型");
            notes.warnings.push(format!(
                "要求的档位「{effort}」对「{model}」不生效（这个模型没有档位菜单），用的是默认档。"
            ));
        }
    }
    // 权限继承只属于 AI 派活（员工的 worker 走项目策略）；父会话是只读时
    // 天花板已经强制完，没有什么可继承的。
    if task.staff_id.is_some() || remote_mode == RemoteMode::ReadOnly {
        return Ok(notes);
    }
    let parent = with_storage(app, |storage| {
        storage.find_session(&task.chief_session_id).ok().flatten()
    });
    let Some(parent) = parent else {
        return Ok(notes);
    };
    let tier = crate::delegation::parent_permission_tier(
        parent.preferred_mode.as_deref(),
        parent.preferred_always_approve,
    );
    notes.permission =
        crate::delegation::inherit_permission_tier(app, worker_session_id, &task.agent_id, tier);
    Ok(notes)
}

/// worker 的 agent 自己广告的模型 id（点名模型不被接受时随判词带回）。
fn advertised_model_ids(app: &AppHandle, worker_session_id: &str) -> Vec<String> {
    app.state::<crate::AppState>()
        .acp
        .get_capabilities(worker_session_id)
        .map(|caps| {
            caps.models
                .iter()
                .take(AVAILABLE_MODELS_CAP)
                .map(|model| model.id.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// AI 派活任务此刻的权限天花板（重试复活时重算：父会话可能在午休期间
/// 被切到了只读）。父会话行没了 = 没有可依据的策略，不该再启动 worker。
/// 只对 AI 派活有意义——自动重试不开放给员工任务（见 handle_turn_end）。
fn effective_remote_mode(app: &AppHandle, task: &Task) -> Option<RemoteMode> {
    let parent = with_storage(app, |storage| {
        storage.find_session(&task.chief_session_id).ok().flatten()
    })?;
    Some(crate::delegation::parent_remote_mode(
        parent.preferred_mode.as_deref(),
        parent.preferred_always_approve,
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_and_watch_worker(
    app: &AppHandle,
    chief_session_id: &str,
    task_id: &str,
    remote_mode: RemoteMode,
    max_minutes: Option<u32>,
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
        Some(profile) if !profile.policy.allow_background_work => Some((
            "background_work_disabled",
            "员工已关闭后台工作，任务未启动。",
        )),
        Some(profile) => match profile.policy.policy_for(&preview.project_id) {
            Some(policy) if policy.enabled && policy.remote_mode != RemoteMode::Blocked => None,
            Some(_) => Some(("project_blocked", "项目授权已禁用，任务未启动。")),
            None => Some(("project_not_allowed", "项目授权已移除，任务未启动。")),
        },
        // AI 派活（普通会话，staff_id=None）：没有员工档案可查，也没有
        // 员工级的暂停/项目闸——能力天花板在 delegate_task 入口已经定死
        // （项目=父会话项目、并发上限 2），这里直接放行。
        None if preview.staff_id.is_none() => None,
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

    // 1.5) 存档点（第五期「可反悔」）：真实 git 项目在 worker 动手前记
    //      HEAD——「回滚此任务」= 把任务期间动过的 tracked 文件恢复到该
    //      提交（untracked 不碰、如实报告）。Chat 虚拟项目没有仓库，跳过。
    //      git 子进程跑在后台线程（storage 锁纪律：锁外跑）。
    if task.project_id != crate::app_paths::CHAT_PROJECT_ID {
        let repo_root = with_storage(app, |storage| {
            storage
                .project_by_id(&task.project_id)
                .ok()
                .map(|project| project.root_path)
        });
        if let Some(root) = repo_root {
            let mut cmd = std::process::Command::new("git");
            cmd.arg("-C").arg(&root).args(["rev-parse", "HEAD"]);
            let base = crate::process_util::output_with_kill_timeout(
                cmd,
                std::time::Duration::from_secs(10),
            )
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| {
                String::from_utf8(output.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            });
            if let Some(commit) = base {
                let _ = store::with_tasks(|tasks| {
                    if let Some(task) = tasks.iter_mut().find(|t| t.task_id == task_id) {
                        task.checkpoint_repo_root = Some(root);
                        task.checkpoint_base_commit = Some(commit);
                    }
                    Ok(())
                });
            }
        }
    }

    // 2) 建 worker 会话行（普通项目会话，顶层可见）。staff 任务沿用
    //    chief-worker；AI 派活（staff_id=None）用 agent-worker——侧栏角标、
    //    挂起豁免、tee 回填都按来源家族区分。Chat 虚拟项目的父会话派活时
    //    worker 也落 Chat 库，cwd 跟父会话（不是进程 current_dir）。
    let is_staff_task = task.staff_id.is_some();
    let worker_origin = if is_staff_task {
        "chief-worker"
    } else {
        "agent-worker"
    };
    let worker_label = format!(
        "{}（{}）",
        task.title,
        if is_staff_task {
            "Chief 任务"
        } else {
            "AI 派活"
        }
    );
    let worker = match with_storage(app, |storage| {
        let parent = storage.find_session(chief_session_id).ok().flatten();
        let lineage = parent.as_ref().map(|parent| crate::models::SessionLineage {
            parent_session_id: parent.id.clone(),
            root_session_id: parent
                .lineage
                .as_ref()
                .map(|lineage| lineage.root_session_id.clone())
                .unwrap_or_else(|| parent.id.clone()),
            relationship: "subagent".to_string(),
        });
        if task.project_id == crate::app_paths::CHAT_PROJECT_ID {
            let parent_cwd = parent.map(|session| session.cwd).unwrap_or_default();
            let cwd = if parent_cwd.is_empty() {
                crate::app_paths::current_dir()?
            } else {
                parent_cwd
            };
            storage.create_chat_session_with_lineage(
                task.agent_id.clone(),
                worker_label,
                &cwd,
                Some(worker_origin),
                lineage,
            )
        } else {
            storage.create_session_with_lineage(
                &task.project_id,
                task.agent_id.clone(),
                worker_label,
                Some(worker_origin),
                lineage,
            )
        }
    }) {
        Ok(session) => session,
        Err(error) => {
            let _ = finalize_task(
                app,
                chief_session_id,
                task_id,
                TaskStatus::Failed,
                None,
                Some(&format!("创建 worker 会话失败：{error}")),
            );
            return;
        }
    };
    // P0-2 ownership: the broker owns the worker lifecycle and the RAW tee
    // registration only. It never normalizes events and never writes the
    // canonical transcript JSONL -- that is the frontend `useSessionRails`
    // writer, driven by live events or by this tee replay. Do not add a
    // transcript append here: broker + tee + rails must not compete.
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

    // 3) 启动 ACP + 应用任务的全部设置
    if let Err(error) = prepare_worker_bridge(app, &task, &worker.id) {
        runner::stop_session(app, &worker.id);
        tee::unregister(&worker.id);
        let _ = finalize_task(
            app,
            chief_session_id,
            task_id,
            TaskStatus::Failed,
            Some("bridge_unavailable"),
            Some(&format!("跨用模型绑定失败：{error}")),
        );
        return;
    }
    if let Err(error) = runner::ensure_session(app, &worker.id, &task.agent_id, &worker.cwd) {
        runner::stop_session(app, &worker.id);
        tee::unregister(&worker.id);
        let _ = finalize_task(
            app,
            chief_session_id,
            task_id,
            TaskStatus::Failed,
            None,
            Some(&format!("Agent 启动失败：{error}")),
        );
        return;
    }
    let settings = match apply_task_settings(app, &task, &worker.id, remote_mode) {
        Ok(settings) => settings,
        Err(error) => {
            runner::stop_session(app, &worker.id);
            tee::unregister(&worker.id);
            // 只读强制不了 / 点名的模型不被接受——两种都不启动，判词写清
            // 原因（模型那种会带回 agent 的真实清单，派活的 AI 照着重派）。
            let stop_reason = match &error {
                SettingsError::ReadOnly(_) => "read_only_not_enforceable",
                SettingsError::Model { .. } => "model_unavailable",
            };
            let _ = finalize_task(
                app,
                chief_session_id,
                task_id,
                TaskStatus::Failed,
                Some(stop_reason),
                Some(&error.verdict(&task.agent_id)),
            );
            return;
        }
    };
    with_storage(app, |storage| {
        storage.update_session_status(&worker.id, "running")
    })
    .ok();
    // 权限说明与档位备注写进任务行：右栏后台任务面板在它跑动的整段时间
    // 里都显示（「已继承免审批」「要求的档位不生效，用的是默认档」…）。
    let notes: Vec<String> = settings
        .permission
        .into_iter()
        .chain(settings.warnings)
        .collect();
    if !notes.is_empty() {
        let _ = store::with_tasks(|tasks| {
            if let Some(task) = tasks.iter_mut().find(|t| t.task_id == task_id) {
                task.last_summary = Some(notes.join("\n"));
            }
            Ok(())
        });
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
    // （第四期）交付模板：有 output_schema 的任务教 worker 用 submit 工具交结果。
    if let Some(schema) = task.output_schema.as_ref() {
        prompt.push_str(&format!(
            "\n\n[交付方式] 完成时调用工具 marionette_submit_output 提交最终结果，\
             内容必须是符合以下 JSON Schema 的 JSON 对象：\n{schema}\n\
             先组织好完整结果再提交；提交被拒会带回原因，修正后可重新提交。"
        ));
    }
    match runner::send_prompt(app, &worker.id, &prompt) {
        Ok(_) => {}
        Err(error) => {
            let _ = finalize_task(
                app,
                chief_session_id,
                task_id,
                TaskStatus::Failed,
                None,
                Some(&format!("目标下发失败：{error}")),
            );
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
        Some(format!(
            "worker 已启动（{} · {}）",
            task.agent_id, worker.id
        )),
    );
    let _ = store::append_events(&[running_event]);
    crate::delegation::emit_delegate_ui_event(app, &task, "task.started");

    // 6) 观测直到空闲 / 超时 / 被取消
    watch_worker_loop(app, chief_session_id, task_id, &worker.id, max_minutes);
}

/// 观测循环（spawn 与 resume 共用）：任务状态驱动退出 + 超时停止 +
/// 卡死解药 + 额度重试排程 + 回合结局分类收尾（二/三期）。
fn watch_worker_loop(
    app: &AppHandle,
    chief_session_id: &str,
    task_id: &str,
    worker_session_id: &str,
    max_minutes: Option<u32>,
) {
    let deadline = max_minutes.map(|minutes| {
        std::time::Instant::now() + std::time::Duration::from_millis((minutes as u64) * 60_000)
    });
    loop {
        // 任务被外部取消/恢复：状态驱动退出。
        let Some(current) = store::find_task(task_id) else {
            return;
        };
        match current.status {
            TaskStatus::Cancelled
            | TaskStatus::Failed
            | TaskStatus::Completed
            | TaskStatus::Interrupted => return, // 终态已由相应路径落账
            _ => {}
        }
        // 额度/限流重试排程（第三期）：到点就重发目标（worker 死了先复活），
        // 没到点就安静等——这期间不算超时也不算卡死（它在午休，不在干活）。
        if let Some(retry_at) = current.retry_at {
            if now_ms() < retry_at {
                std::thread::sleep(std::time::Duration::from_millis(2000));
                continue;
            }
            if !reissue_goal_for_retry(app, task_id, worker_session_id) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2000));
            continue;
        }
        if let Some(deadline) = deadline {
            if std::time::Instant::now() >= deadline {
                let max_minutes = max_minutes.unwrap_or_default();
                runner::cancel_turn(app, worker_session_id);
                let _ = runner::wait_turn_idle_checked(app, worker_session_id, 10_000, || true);
                let _ = finalize_task(
                    app,
                    chief_session_id,
                    task_id,
                    TaskStatus::Failed,
                    Some("timeout"),
                    Some(&format!("任务超过 {max_minutes} 分钟上限，已停止。")),
                );
                return;
            }
            // 卡死解药（第三期，只对 AI 派活——员工任务有自己的「无进展」超时）：
            // Running 且 STALL_MS 无任何事件 → 中止这个回合；宽限期内仍不收尾
            // 就连进程一起停掉。无论哪条路都记 Failed(stalled)：这一下是我们
            // 掐掉的，不能顺着回合结局记成「完成」或「已取消」。
        }
        if current.staff_id.is_none()
            && current.status == TaskStatus::Running
            && now_ms().saturating_sub(current.last_event_at) > WORKER_STALL_MS
        {
            runner::cancel_turn(app, worker_session_id);
            if !runner::wait_turn_idle_checked(
                app,
                worker_session_id,
                WORKER_STALL_GRACE_MS,
                || true,
            ) {
                runner::stop_session(app, worker_session_id);
            }
            let _ = finalize_task(
                app,
                chief_session_id,
                task_id,
                TaskStatus::Failed,
                Some("stalled"),
                Some("worker 连续 20 分钟没有任何动静，已中止。可以重新派活。"),
            );
            return;
        }
        if current.status == TaskStatus::WaitingUser {
            // 等用户指示 / 权限卡：不判超时、不把回合结束当任务完成。
            std::thread::sleep(std::time::Duration::from_millis(2000));
            continue;
        }
        if runner::turn_idle_now(app, worker_session_id) {
            // 沉默不证明完成（orca 铁律）：turn_idle 对缺席进程也返回 true，
            // 进程没了 = 失败，绝不猜「完成」。活着才走空闲收尾。
            if !runner::session_alive(app, worker_session_id) {
                let _ = finalize_task(
                    app,
                    chief_session_id,
                    task_id,
                    TaskStatus::Failed,
                    Some("worker_gone"),
                    Some("worker 进程退出，任务未完成——可重新派活。"),
                );
                return;
            }
            // 空闲即回合结束；worker 正文从 tee 折出。给 tee writer 一点
            // 收尾时间再读回。收尾前先分类：额度类瞬时失败 → 排队重试，
            // 本循环继续守；其余走终态收尾。
            std::thread::sleep(std::time::Duration::from_millis(600));
            if handle_turn_end(app, chief_session_id, task_id, worker_session_id) {
                continue;
            }
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2000));
    }
}

/// 回合结束的分类收尾。返回 true = 已排程自动重试，watch 循环继续守；
/// false = 终态已落（或收尾失败），循环退出。
fn handle_turn_end(
    app: &AppHandle,
    chief_session_id: &str,
    task_id: &str,
    worker_session_id: &str,
) -> bool {
    let events = tee::replay(worker_session_id)
        .map(|(events, _)| events)
        .unwrap_or_default();
    let (stop_reason, error_text) = last_turn_outcome(&events);
    // 失败/中止时把 worker 已经说过的最后一截话带上——父会话要据此判断
    // 做到了哪一步，而不是只收到一句宿主的判词。
    let worker_tail = summarize_worker_events(&events).0;
    let detail = error_text
        .as_deref()
        .filter(|text| !text.trim().is_empty())
        .unwrap_or("（无错误详情）")
        .to_string();
    match classify_turn_failure(stop_reason.as_deref(), error_text.as_deref()) {
        TurnFailureClass::Transient => {
            // 自动重试只对 AI 派活开放：员工的 worker 由员工自己决定重派
            // （它有自己的预算与 task_resume 语义），这里只如实记失败。
            let auto_retry = store::find_task(task_id).is_some_and(|task| task.staff_id.is_none());
            if !auto_retry {
                let _ = finalize_task(
                    app,
                    chief_session_id,
                    task_id,
                    TaskStatus::Failed,
                    Some("transient_error"),
                    Some(&with_worker_tail(
                        format!("额度/限流/服务繁忙，任务未完成：{detail}"),
                        worker_tail.as_deref(),
                    )),
                );
                return false;
            }
            enum RetryPlan {
                Scheduled,
                Exhausted,
                NotPossible,
            }
            let plan = store::with_tasks(|tasks| {
                let Some(task) = tasks.iter_mut().find(|t| t.task_id == task_id) else {
                    return Ok(RetryPlan::NotPossible);
                };
                if task.status.is_terminal() || task.retry_at.is_some() {
                    return Ok(RetryPlan::NotPossible);
                }
                let Some(delay) = RETRY_BACKOFF_MS.get(task.retry_count as usize).copied() else {
                    return Ok(RetryPlan::Exhausted);
                };
                task.retry_count += 1;
                task.retry_at = Some(now_ms() + delay);
                task.waiting_reason = Some("auto_retry".to_string());
                task.updated_at = now_ms();
                task.last_event_at = now_ms();
                task.last_summary = Some(format!(
                    "额度/限流受限，{} 分钟后自动重试（第 {} 次）。",
                    delay / 60_000,
                    task.retry_count
                ));
                Ok(RetryPlan::Scheduled)
            })
            .unwrap_or(RetryPlan::NotPossible);
            match plan {
                RetryPlan::Scheduled => {
                    if let Some(task) = store::find_task(task_id) {
                        let event = emit_event(
                            &task,
                            "task.progress",
                            task.status,
                            task.last_summary.clone(),
                        );
                        let _ = store::append_events(&[event]);
                    }
                    true
                }
                RetryPlan::Exhausted => {
                    let _ = finalize_task(
                        app,
                        chief_session_id,
                        task_id,
                        TaskStatus::Failed,
                        Some("retry_exhausted"),
                        Some(&with_worker_tail(
                            format!("额度/限流自动重试 3 次后仍未成功，任务停止。最后一次的错误：{detail}"),
                            worker_tail.as_deref(),
                        )),
                    );
                    false
                }
                RetryPlan::NotPossible => false,
            }
        }
        TurnFailureClass::Cancelled => {
            let _ = finalize_task(
                app,
                chief_session_id,
                task_id,
                TaskStatus::Cancelled,
                Some("turn_cancelled"),
                Some(&with_worker_tail(
                    "worker 的回合被中止（用户停止或宿主取消）。".to_string(),
                    worker_tail.as_deref(),
                )),
            );
            false
        }
        TurnFailureClass::Refused => {
            let _ = finalize_task(
                app,
                chief_session_id,
                task_id,
                TaskStatus::Failed,
                Some("refused"),
                Some(&with_worker_tail(
                    "worker 拒绝执行这个任务。".to_string(),
                    worker_tail.as_deref(),
                )),
            );
            false
        }
        TurnFailureClass::TurnError => {
            let reason = match (error_text.as_deref(), stop_reason.as_deref()) {
                (Some(text), _) if !text.trim().is_empty() => text.to_string(),
                (_, Some("max_tokens")) => "回复达到长度上限，被截断".to_string(),
                (_, Some("max_turn_requests")) => "达到单回合的请求/工具轮数上限".to_string(),
                (_, Some(reason)) => format!("回合异常结束（{reason}）"),
                _ => "无错误详情".to_string(),
            };
            let _ = finalize_task(
                app,
                chief_session_id,
                task_id,
                TaskStatus::Failed,
                Some("turn_error"),
                Some(&with_worker_tail(
                    format!("worker 回合出错，任务未完成：{reason}。"),
                    worker_tail.as_deref(),
                )),
            );
            false
        }
        TurnFailureClass::Clean => {
            finish_completed(app, chief_session_id, task_id, worker_session_id);
            false
        }
    }
}

/// 重试到点：清标记、复活 worker（进程没了就 ensure_session 并重放全部
/// 设置）、重发目标。返回 false = 任务已不在可重试状态（被取消/账本写
/// 失败/复活失败），watch 退出。
fn reissue_goal_for_retry(app: &AppHandle, task_id: &str, worker_session_id: &str) -> bool {
    let Some(task) = store::find_task(task_id) else {
        return false;
    };
    if task.status.is_terminal() {
        return false;
    }
    let mut remote_mode = RemoteMode::ApproveWrites;
    if !runner::session_alive(app, worker_session_id) {
        let cwd = with_storage(app, |storage| {
            storage
                .find_session(worker_session_id)
                .ok()
                .flatten()
                .map(|session| session.cwd)
        });
        let Some(cwd) = cwd else {
            // 会话行没了就别留一个永远到点却没人重试的僵尸。
            let _ = finalize_task(
                app,
                &task.chief_session_id,
                task_id,
                TaskStatus::Failed,
                Some("worker_session_missing"),
                Some("重试时找不到 worker 会话，任务停止。"),
            );
            return false;
        };
        // 策略可能在午休期间变了（员工的项目授权被改 / 父会话切到了
        // 只读）——复活前按当前策略重算，不能沿用派活那一刻的。
        let Some(current_mode) = effective_remote_mode(app, &task) else {
            let _ = finalize_task(
                app,
                &task.chief_session_id,
                task_id,
                TaskStatus::Failed,
                Some("policy_changed"),
                Some("重试前权限策略已变化（授权被移除或禁用），任务停止。"),
            );
            return false;
        };
        remote_mode = current_mode;
        if let Err(error) = runner::ensure_session(app, worker_session_id, &task.agent_id, &cwd) {
            let _ = finalize_task(
                app,
                &task.chief_session_id,
                task_id,
                TaskStatus::Failed,
                Some("retry_respawn_failed"),
                Some(&format!(
                    "额度重试时 worker 无法复活（Agent 启动失败：{error}）。"
                )),
            );
            return false;
        }
        // 复活出来的是全新进程：只读天花板 / 模型 / 档位 / 继承的权限档
        // 全部重放，否则重试一次就把它们丢了。点名的模型在午休期间从
        // agent 的目录里消失了（zcode 的账号目录会浮动）也在这里如实失败。
        if let Err(error) = apply_task_settings(app, &task, worker_session_id, remote_mode) {
            runner::stop_session(app, worker_session_id);
            let (stop_reason, verdict) = match &error {
                SettingsError::ReadOnly(reason) => (
                    "read_only_not_enforceable",
                    format!("重试时无法强制只读权限，任务停止：{reason}"),
                ),
                SettingsError::Model { .. } => ("model_unavailable", error.verdict(&task.agent_id)),
            };
            let _ = finalize_task(
                app,
                &task.chief_session_id,
                task_id,
                TaskStatus::Failed,
                Some(stop_reason),
                Some(&verdict),
            );
            return false;
        }
    }
    let cleared = store::with_tasks(|tasks| {
        let Some(task) = tasks.iter_mut().find(|t| t.task_id == task_id) else {
            return Ok(false);
        };
        if task.retry_at.is_none() || task.status.is_terminal() {
            return Ok(false);
        }
        task.retry_at = None;
        task.waiting_reason = None;
        task.status = TaskStatus::Running;
        task.turn_count = task.turn_count.saturating_add(1);
        task.updated_at = now_ms();
        task.last_event_at = now_ms();
        task.last_summary = Some(format!(
            "额度重试（第 {} 次）已重新下发目标。",
            task.retry_count
        ));
        Ok(true)
    })
    .unwrap_or(false);
    if !cleared {
        return false;
    }
    let mut prompt = format!(
        "[HOST EVENT: task.retry]\n额度窗口已恢复，继续之前的任务「{}」。原始目标：\n{}\n",
        task.title, task.goal
    );
    if remote_mode == RemoteMode::ReadOnly {
        prompt.push_str("[HOST 约束] 本任务为只读模式：只分析/检索/读取，不要修改任何文件。\n");
    }
    if let Some(schema) = task.output_schema.as_ref() {
        prompt.push_str(&format!(
            "\n交付方式不变：完成时用 marionette_submit_output 提交符合此 Schema 的对象：\n{schema}\n"
        ));
    }
    match runner::send_prompt(app, worker_session_id, &prompt) {
        Ok(_) => {
            let task = store::find_task(task_id);
            if let Some(task) = task {
                let event = emit_event(
                    &task,
                    "task.progress",
                    task.status,
                    task.last_summary.clone(),
                );
                let _ = store::append_events(&[event]);
            }
            true
        }
        Err(error) => {
            // 发不出去：回退成等待重试（下一个退避档），不烧掉这次机会。
            let _ = store::with_tasks(|tasks| {
                if let Some(task) = tasks.iter_mut().find(|t| t.task_id == task_id) {
                    let delay = RETRY_BACKOFF_MS
                        .get(task.retry_count.saturating_sub(1) as usize)
                        .copied()
                        .unwrap_or(30 * 60 * 1000);
                    task.retry_at = Some(now_ms() + delay);
                    task.waiting_reason = Some("auto_retry".to_string());
                    task.error = Some(error);
                }
                Ok(())
            });
            true
        }
    }
}

fn finish_completed(
    app: &AppHandle,
    chief_session_id: &str,
    task_id: &str,
    worker_session_id: &str,
) {
    let cost_cap = store::find_task(task_id).and_then(|task| {
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
    // 零产出检查（第二期「可信」）：既没有 assistant 正文也没有任何工具
    // 调用的「完成」不叫完成——如实标失败，不猜。
    if summary.is_none() && tool_calls == 0 {
        let _ = finalize_task(
            app,
            chief_session_id,
            task_id,
            TaskStatus::Failed,
            Some("no_output"),
            Some("worker 回合结束但没有任何产出（无正文、无工具调用）——不猜完成。"),
        );
        return;
    }
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
        Some(text) if tool_failures > 0 => format!("{status_line} {text}"),
        Some(text) => text.to_string(),
        None => status_line.to_string(),
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
pub(crate) fn finalize_task(
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
        task.retry_at = None;
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
    crate::delegation::emit_delegate_ui_event(app, &task, kind);
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
            // AI 派活的 worker 没有前端替它管生命周期（后端驱动家族豁免
            // 自动挂起）——任务一终结就把进程收掉，否则每派过一次活就
            // 留一个 agent 进程。员工的 worker 维持原状（resume 语义不变）。
            if task.staff_id.is_none() {
                runner::stop_session(app, worker_sid);
            }
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
    let urgency = args
        .get("urgency")
        .and_then(Value::as_str)
        .unwrap_or("normal");
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
    // 「读取即送达」（第二期，T3 语义）：查过就算看过，不再补唤醒。
    let undelivered: Vec<String> = store::load_events_for_task(task_id)
        .iter()
        .filter(|event| event.delivered_at.is_none())
        .map(|event| event.event_id.clone())
        .collect();
    if !undelivered.is_empty() {
        let _ = store::mark_events_delivered(&undelivered, now_ms());
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
    let reason = args
        .get("reason")
        .and_then(Value::as_str)
        .unwrap_or("用户要求停止");
    let Some(task) = store::find_task(task_id) else {
        return envelope_reject("task_not_found", "任务不存在。");
    };
    let staff_id =
        staff_commands::profile_for_session(chief_session_id).map(|profile| profile.staff_id);
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
    let note = args
        .get("note")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
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
        return envelope_reject(
            "not_resumable",
            "worker 从未启动，没有可恢复的会话。请重新发起任务。",
        );
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
            return Err(format!(
                "状态 {} 不能恢复",
                format!("{:?}", t.status).to_lowercase()
            ));
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
    let Some(task) = store::find_task(task_id) else {
        return;
    };
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
        let _ = finalize_task(
            app,
            chief_session_id,
            task_id,
            TaskStatus::Failed,
            None,
            Some(&format!("恢复失败：{error}")),
        );
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
            let _ = finalize_task(
                app,
                chief_session_id,
                task_id,
                TaskStatus::Failed,
                None,
                Some(&format!("恢复下发失败：{error}")),
            );
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
    watch_worker_loop(
        app,
        chief_session_id,
        task_id,
        worker_session_id,
        Some(max_minutes),
    );
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
pub fn observe_chief_event(app: &AppHandle, session_id: &str, method: Option<&str>, data: &Value) {
    use TaskStatus::*;
    let Some(method) = method else { return };
    let entered = matches!(
        method,
        "permission/prompt" | "question/prompt" | "plan/approval"
    );
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
    let Some(task) = store::load_tasks()
        .into_iter()
        .find(|task| task.worker_session_id.as_deref() == Some(session_id))
    else {
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
            task.waiting_reason = Some(
                match method {
                    "permission/prompt" => "permission",
                    "question/prompt" => "question",
                    "plan/approval" => "plan_approval",
                    _ => "user_input",
                }
                .to_string(),
            );
            task.updated_at = now_ms();
            task.last_event_at = now_ms();
            if summary.is_some() {
                task.last_summary = summary.clone();
            }
            Ok(Some(task.clone()))
        });
        let Some(task) = updated.ok().flatten() else {
            return;
        };
        let event = emit_event(
            &task,
            "task.waiting_user",
            WaitingUser,
            task.last_summary.clone(),
        );
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
        let Some(task) = updated.ok().flatten() else {
            return;
        };
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
    if staff_sessions.is_empty() && registry.legacy_migration_completed_at == 0 {
        // 旧单 Chief 兼容：没有员工档案时补旧 chief 会话进过滤集。
        // 注意不能在这里提前 return——AI 派活任务（staff_id=None）不受
        // 员工有无影响，急停必须照扫（2026-11 第一期拆门引入）。
        if let Some(chief_sid) = config.chief_session_id.clone() {
            staff_sessions.insert(chief_sid);
        }
    }
    let active: Vec<Task> = store::load_tasks()
        .into_iter()
        .filter(|task| {
            // AI 派活的任务（staff_id=None）同样归急停管——全局停就是全局停。
            task.staff_id.is_none()
                || staff_sessions.contains(&task.chief_session_id)
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
        Some(error) => Err(format!(
            "已停止 {stopped} 个，但仍有任务账本写入失败：{error}"
        )),
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
            task.staff_id.as_deref() == Some(staff_id) || task.chief_session_id == chief_session_id
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
    let pending_work = pending_user_count(chief_sid) > 0 || pending_wake_count(chief_sid) > 0;
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
                with_storage(app, |storage| {
                    storage.update_session_status(chief_sid, "exited")
                })
                .ok();
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
                    let ids = events
                        .iter()
                        .map(|event| event.event_id.clone())
                        .collect::<Vec<_>>();
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
/// Persist worker lineage from the task ledger before the retention sweep can
/// remove it. New workers write this at creation; this migrates existing rows.
pub fn backfill_worker_lineage(app: &AppHandle) -> usize {
    let tasks = store::load_tasks();
    with_storage(app, |storage| {
        let mut updated = 0;
        for task in &tasks {
            let Some(worker_id) = task.worker_session_id.as_deref() else {
                continue;
            };
            let Some(parent) = storage.find_session(&task.chief_session_id).ok().flatten() else {
                continue;
            };
            let root_id = parent
                .lineage
                .as_ref()
                .map(|lineage| lineage.root_session_id.clone())
                .unwrap_or_else(|| parent.id.clone());
            let lineage = crate::models::SessionLineage {
                parent_session_id: parent.id,
                root_session_id: root_id,
                relationship: "subagent".to_string(),
            };
            if storage
                .set_session_lineage(worker_id, lineage)
                .unwrap_or(false)
            {
                updated += 1;
            }
        }
        Ok::<usize, String>(updated)
    })
    .unwrap_or(0)
}

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

/// AI 派活的唤醒投递（第一期最小版）：WAKE_INBOX 里凡是不属于员工/旧
/// Chief 的会话（即普通父会话），进程**已经在跑**且空闲时合并 ≤5 条事件
/// 成一条 host prompt 发出。刻意**不懒启动**普通会话——用户没打开的会话
/// 不自动唤起花钱；进程不在就留着，用户重新打开的下一个 tick 自然补投。
fn flush_agent_wake_inboxes(app: &AppHandle, covered: &HashSet<String>) {
    let keys: Vec<String> = match wake_inbox().lock() {
        Ok(inbox) => inbox.keys().cloned().collect(),
        Err(_) => return,
    };
    for sid in keys {
        if covered.contains(&sid) || is_chief_session(&sid) {
            continue;
        }
        if !runner::session_alive(app, &sid) || !runner::turn_idle_now(app, &sid) {
            continue;
        }
        let batch = wake_inbox().lock().ok().and_then(|mut inbox| {
            let entry = inbox.get_mut(&sid)?;
            if entry.is_empty() {
                return None;
            }
            Some(entry.drain(..5.min(entry.len())).collect::<Vec<_>>())
        });
        let Some(events) = batch else { continue };
        let text = events
            .iter()
            .map(|event| host_event_text(event, store::find_task(&event.task_id).as_ref()))
            .collect::<Vec<_>>()
            .join("\n");
        match runner::send_prompt(app, &sid, &text) {
            Ok(_) => {
                let ids = events
                    .iter()
                    .map(|event| event.event_id.clone())
                    .collect::<Vec<_>>();
                let _ = store::mark_events_delivered(&ids, now_ms());
            }
            Err(error) => {
                if let Ok(mut inbox) = wake_inbox().lock() {
                    let entry = inbox.entry(sid.clone()).or_default();
                    for event in events.into_iter().rev() {
                        entry.insert(0, event);
                    }
                }
                crate::debug_log::append(
                    "delegation",
                    "warn",
                    &sid,
                    "agent wake flush failed; events re-queued",
                    Some(&error),
                );
            }
        }
    }
}

fn heartbeat_tick(app: &AppHandle) {
    let staff_registry = staff::load();
    // AI 派活的唤醒 flush 放最前（覆盖下方所有早退路径）：员工会话由
    // heartbeat_staff 自己 flush、旧单 Chief 会话由旧路径处理，两类都进
    // 排除集合，避免双投。
    {
        let mut covered: HashSet<String> = staff_registry
            .staff
            .iter()
            .map(|profile| profile.session_id.clone())
            .filter(|session| !session.is_empty())
            .collect();
        let config = store::load_config();
        if staff_registry.staff.is_empty() && config.mode == RemoteWorkMode::Chief {
            if let Some(legacy_chief) = config.chief_session_id {
                covered.insert(legacy_chief);
            }
        }
        flush_agent_wake_inboxes(app, &covered);
    }
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
    let pending_work = pending_user_count(&chief_sid) > 0 || pending_wake_count(&chief_sid) > 0;
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
        let Some(policy) = config.policy_for(&task.project_id) else {
            continue;
        };
        let max_ms = (policy.max_task_minutes as u64) * 60_000;
        let stale = now_ms().saturating_sub(task.last_event_at) > max_ms;
        let worker_gone = task
            .worker_session_id
            .as_deref()
            .map(|sid| runner::process_gone(app, sid))
            .unwrap_or(false);
        if worker_gone && task.status == TaskStatus::Running {
            let _ = finalize_task(
                app,
                &chief_sid,
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
                &chief_sid,
                &task.task_id,
                TaskStatus::Failed,
                Some("timeout"),
                Some(&format!(
                    "任务超过 {} 分钟无进展，已停止。",
                    policy.max_task_minutes
                )),
            );
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
