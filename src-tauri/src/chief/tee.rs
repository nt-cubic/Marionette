//! Durable event tee：Chief 驱动会话的 AcpEvent 落盘（M0）。
//!
//! 被后端驱动的会话（Chief / worker，M0 里是探针会话）不经过前端
//! `useSessionRails` 的转录管线，它们的事件由本 tee 原样追加到
//! `~/.marionette/chief/sessions/<sessionId>.events.jsonl`，每行一个
//! 标准 AcpEvent 形状（`{seq, ts, sessionId, kind, method, data}`）。
//! 重启后用 [`replay`] 读回即可重建视图。
//!
//! 纪律（对齐 `emit_now` 的注释）：observe 跑在事件派发线程上，只做
//! 一次 map 查表 + 有界 channel `try_send`；真正的文件 IO 在每会话
//! 一个的 writer 线程里。队列满（磁盘长卡）丢事件并计数，绝不反压
//! ACP 读线程——转录真相永远在 storage 的 transcript 里，tee 是
//! 回放视图的原料，丢了要能从日志看出来。

//! P0-2 OWNERSHIP: this module owns the append-only RAW ledger only. It never
//! normalizes ACP envelopes into SessionEvents and never writes the canonical
//! transcript (`<sid>.jsonl`). The frontend `useSessionRails` pipeline is the
//! sole canonical writer: it replays this ledger through `applyStreamPart`
//! and flushes the result before advancing `consumed_seq`. The cursor below is
//! therefore a replay filter, not an ownership record; `replay()` always
//! returns the full file so a lost canonical transcript can be rebuilt.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// 每会话事件队列容量。正常时远不用满；满了说明磁盘或 writer 出问题，
/// 丢事件计日志而不是把 agent 的 stdout 管道憋死（见 acp.rs 背压注释）。
const QUEUE_CAP: usize = 2048;
/// unregister 时等 writer 线程收尾的最长时间；超时就放弃等待并记日志。
const DRAIN_WAIT: Duration = Duration::from_secs(5);
/// P0-3: upper bound for one segmented recovery read. Keeps a retry from
/// materializing a multi-megabyte ledger in one allocation.
const REPLAY_SEGMENT_MAX: usize = 2000;

static TEES: OnceLock<Mutex<HashMap<String, TeeHandle>>> = OnceLock::new();

/// 测试沙箱根（对齐 council store 的 `set_root_override_for_tests`）。
static ROOT_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// P0-3: durable, content-free health for one tee. Counters are cumulative
/// for the session's lifetime (seeded from this file when the process
/// restarts); `replay_*` describe the most recent read of the ledger. No
/// prompt, tool-output or path data is ever written here.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TeeHealthCounters {
    #[serde(default)]
    pub enqueued: u64,
    #[serde(default)]
    pub written: u64,
    #[serde(default)]
    pub dropped: u64,
    #[serde(default)]
    pub bad_lines: u64,
    #[serde(default)]
    pub replay_start_seq: u64,
    #[serde(default)]
    pub replay_events: u64,
    #[serde(default)]
    pub replay_truncated: bool,
    #[serde(default)]
    pub updated_at: u64,
}

/// What the UI reads when a worker session is opened. `file_lines` and
/// `consumed_seq` are derived now (not stored) so they cannot go stale.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeeHealth {
    pub session_id: String,
    pub file_lines: u64,
    pub enqueued: u64,
    pub written: u64,
    pub dropped: u64,
    pub bad_lines: u64,
    pub consumed_seq: u64,
    pub replay_start_seq: u64,
    pub replay_events: u64,
    pub replay_truncated: bool,
    pub registered: bool,
    pub updated_at: u64,
}

/// One bounded read of the ledger, used by the recovery action.
pub struct ReplaySegment {
    pub events: Vec<Value>,
    pub bad_lines: usize,
    pub has_more: bool,
    pub next_seq: u64,
}

struct TeeHandle {
    tx: SyncSender<Value>,
    writer: Option<std::thread::JoinHandle<()>>,
    /// observe-side events lost because the queue was full or the writer was
    /// already gone (dropped, never blocking the ACP reader).
    dropped: std::sync::Arc<AtomicU64>,
    /// P0-1: events observe successfully handed to the writer queue.
    enqueued: std::sync::Arc<AtomicU64>,
    /// P0-1: events the writer thread flushed to the tee file.
    written: std::sync::Arc<AtomicU64>,
    /// Tee file path for this session (line count is scanned at unregister).
    path: PathBuf,
}

#[cfg(test)]
pub fn set_root_override_for_tests(path: Option<PathBuf>) {
    if let Ok(mut slot) = ROOT_OVERRIDE.lock() {
        *slot = path;
    }
}

fn sessions_root() -> Result<PathBuf, String> {
    if let Ok(Some(dir)) = ROOT_OVERRIDE.lock().map(|slot| slot.clone()) {
        return Ok(dir);
    }
    crate::app_paths::global_dir().map(|dir| dir.join("chief").join("sessions"))
}

/// 会话 id 直接进文件名；Chief 的 id 由 Host 生成（`session_<…>` 形状）。
/// 集合外字符一律拒绝注册（fail-closed），不做静默替换——静默替换会让
/// 两个 id 折叠到同一个文件、事件串台。
pub(crate) fn validate_session_id(session_id: &str) -> Result<(), String> {
    if session_id.is_empty()
        || session_id.len() > 200
        || !session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!("chief tee: unusable session id `{session_id}`"));
    }
    Ok(())
}

fn events_path(root: &std::path::Path, session_id: &str) -> PathBuf {
    root.join(format!("{session_id}.events.jsonl"))
}

fn health_path(root: &std::path::Path, session_id: &str) -> PathBuf {
    root.join(format!("{session_id}.tee-health.json"))
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Read the durable counters. A missing/corrupt file is an empty record, not an
/// error: health must never take a session down or block its replay.
fn load_health(root: &Path, session_id: &str) -> TeeHealthCounters {
    std::fs::read_to_string(health_path(root, session_id))
        .ok()
        .and_then(|raw| serde_json::from_str::<TeeHealthCounters>(&raw).ok())
        .unwrap_or_default()
}

fn save_health(root: &Path, session_id: &str, counters: &TeeHealthCounters) {
    let mut next = *counters;
    next.updated_at = now_ms();
    let Ok(body) = serde_json::to_string(&next) else {
        return;
    };
    if let Err(err) = crate::storage::write_atomic(&health_path(root, session_id), &body) {
        // Health is advisory; a failed write must not affect tee/replay.
        crate::debug_log::append(
            "chief",
            "warn",
            session_id,
            "tee health write failed",
            Some(&err.to_string()),
        );
    }
}

/// Live counters for a registered session (None = no writer in this process).
fn live_counters(session_id: &str) -> Option<(u64, u64, u64)> {
    let map = TEES.get()?;
    let guard = map.lock().ok()?;
    let handle = guard.get(session_id)?;
    Some((
        handle.enqueued.load(Ordering::Relaxed),
        handle.written.load(Ordering::Relaxed),
        handle.dropped.load(Ordering::Relaxed),
    ))
}

/// Persist the outcome of a ledger read. Merges live counters first so a
/// replay can never roll the durable numbers backwards.
fn record_replay(
    session_id: &str,
    start_seq: u64,
    events: u64,
    bad_lines: u64,
    truncated: bool,
) {
    let Ok(root) = sessions_root() else { return };
    let mut counters = load_health(&root, session_id);
    if let Some((enqueued, written, dropped)) = live_counters(session_id) {
        counters.enqueued = counters.enqueued.max(enqueued);
        counters.written = counters.written.max(written);
        counters.dropped = counters.dropped.max(dropped);
    }
    counters.replay_start_seq = start_seq;
    counters.replay_events = events;
    counters.bad_lines = bad_lines;
    counters.replay_truncated = truncated;
    save_health(&root, session_id, &counters);
}

/// P0-3 read entry: content-free tee health for the worker UI.
pub fn health(session_id: &str) -> Result<TeeHealth, String> {
    validate_session_id(session_id)?;
    let root = sessions_root()?;
    let counters = load_health(&root, session_id);
    let path = events_path(&root, session_id);
    let file_lines = count_lines(&path);
    let live = live_counters(session_id);
    Ok(TeeHealth {
        session_id: session_id.to_string(),
        file_lines,
        enqueued: counters
            .enqueued
            .max(live.map(|v| v.0).unwrap_or(0))
            .max(file_lines),
        written: counters
            .written
            .max(live.map(|v| v.1).unwrap_or(0))
            .max(file_lines),
        dropped: counters.dropped.max(live.map(|v| v.2).unwrap_or(0)),
        bad_lines: counters.bad_lines,
        consumed_seq: consumed_seq(session_id),
        replay_start_seq: counters.replay_start_seq,
        replay_events: counters.replay_events,
        replay_truncated: counters.replay_truncated,
        registered: live.is_some(),
        updated_at: counters.updated_at,
    })
}

/// 开始 tee 一个会话。幂等：已注册的会话返回现有路径，不再开第二条
/// writer（重复开会把同一个文件写花）。
///
/// 文件 IO（mkdir、数行、开文件）全部在 registry 锁**外**做——observe
/// 是每个 UI 事件都要过的一道闸，锁内绝不能等磁盘。
pub fn register(session_id: &str) -> Result<PathBuf, String> {
    validate_session_id(session_id)?;
    let root = sessions_root()?;
    std::fs::create_dir_all(&root).map_err(|e| format!("chief tee: mkdir failed: {e}"))?;
    let path = events_path(&root, session_id);

    let map = TEES.get_or_init(|| Mutex::new(HashMap::new()));
    // 快路径：已注册（或并发注册先赢）直接返回。
    if let Ok(guard) = map.lock() {
        if guard.contains_key(session_id) {
            return Ok(path);
        }
    }

    // 断点续记：重新注册（重启恢复）时 seq 从现有行数接着数。
    let seed_seq = count_lines(&path);
    // P0-3: counters survive a process restart. The file line count is the
    // authoritative "written" floor (a crash can leave the health file behind).
    let persisted = load_health(&root, session_id);
    let base_written = persisted.written.max(seed_seq);
    let base_enqueued = persisted.enqueued.max(base_written);
    let (tx, rx) = std::sync::mpsc::sync_channel::<Value>(QUEUE_CAP);
    let dropped = std::sync::Arc::new(AtomicU64::new(persisted.dropped));
    let enqueued = std::sync::Arc::new(AtomicU64::new(base_enqueued));
    let written = std::sync::Arc::new(AtomicU64::new(base_written));
    let writer = spawn_writer(
        session_id.to_string(),
        path.clone(),
        rx,
        seed_seq,
        std::sync::Arc::clone(&written),
    );

    let mut guard = map.lock().map_err(|_| "chief tee registry poisoned")?;
    if guard.contains_key(session_id) {
        // 输给了并发注册：丢 tx 让自己的 writer 空跑退出。
        drop(tx);
        return Ok(path);
    }
    guard.insert(
        session_id.to_string(),
        TeeHandle {
            tx,
            writer: Some(writer),
            dropped,
            enqueued,
            written,
            path: path.clone(),
        },
    );
    // P0-1 stage 3 marker: resumed sessions start from the existing line count.
    crate::subagent_observe::log_tee_registered(session_id, seed_seq);
    Ok(path)
}

/// 停止 tee 一个会话：丢发送端，等 writer 把队列收干后退出。
/// 未注册的会话静默成功（与 acp `unregister_emitter` 同宽容）。
/// 收尾等待在锁外做——最长 5s，不能让全体事件线程陪等。
pub fn unregister(session_id: &str) {
    let Some(map) = TEES.get() else { return };
    let handle = {
        let mut guard = match map.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        guard.remove(session_id)
    };
    let Some(handle) = handle else { return };
    drop(handle.tx); // writer 的 recv 循环随之结束
    if let Some(writer) = handle.writer {
        let deadline = std::time::Instant::now() + DRAIN_WAIT;
        while !writer.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if !writer.is_finished() {
            crate::debug_log::append(
                "chief",
                "warn",
                session_id,
                "tee writer did not drain in time; remaining events may be lost",
                None,
            );
        }
    }
    let dropped = handle.dropped.load(Ordering::Relaxed);
    if dropped > 0 {
        crate::debug_log::append(
            "chief",
            "warn",
            session_id,
            "tee dropped events (queue full)",
            Some(&dropped.to_string()),
        );
    }
    // P0-1 stage 3 summary: enqueued vs written vs dropped, plus the actual tee
    // file line count. Scanned once at close (never on the hot path).
    let enqueued = handle.enqueued.load(Ordering::Relaxed);
    let written = handle.written.load(Ordering::Relaxed);
    let file_lines = count_lines(&handle.path);
    crate::subagent_observe::log_tee_summary(session_id, enqueued, written, dropped, file_lines);
    // P0-3: durable health. This is the only place the close-time counters
    // currently land; without it a dropped count vanished with runtime state.
    if let Ok(root) = sessions_root() {
        let mut counters = load_health(&root, session_id);
        counters.enqueued = counters.enqueued.max(enqueued);
        counters.written = counters.written.max(written).max(file_lines);
        counters.dropped = counters.dropped.max(dropped);
        save_health(&root, session_id, &counters);
    }
}

/// 事件收口（emit_now 调用）。未注册的会话一次 map miss 就返回。
/// 锁内只 clone 发送端；`data` 的克隆与 try_send 都在锁外——payload
/// 可能是几 MB 的工具输出，不该攥着全局闸门做拷贝。
pub fn observe(session_id: &str, kind: &str, method: Option<&str>, data: &Value) {
    let Some(map) = TEES.get() else { return };
    let sender = {
        let guard = match map.lock() {
            Ok(guard) => guard,
            Err(_) => return,
        };
        guard.get(session_id).map(|handle| {
            (
                handle.tx.clone(),
                handle.dropped.clone(),
                handle.enqueued.clone(),
            )
        })
    };
    let Some((tx, dropped, enqueued)) = sender else {
        return;
    };
    let event = json!({
        "sessionId": session_id,
        "kind": kind,
        "method": method,
        "data": data,
    });
    match tx.try_send(event) {
        Ok(()) => {
            enqueued.fetch_add(1, Ordering::Relaxed);
        }
        // Full = disk/writer fell behind; Disconnected = unregistered between
        // the map lookup and the send. Both are real losses for this tee.
        Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
            dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// 读回一个会话的 tee 事件（重启后回放重建视图）。坏行跳过并记数。
/// 返回 `(事件列表, 坏行数)`。
pub fn replay(session_id: &str) -> Result<(Vec<Value>, usize), String> {
    validate_session_id(session_id)?;
    let path = events_path(&sessions_root()?, session_id);
    let file = File::open(&path).map_err(|e| format!("chief tee: open replay failed: {e}"))?;
    let mut events = Vec::new();
    let mut bad = 0usize;
    for line in std::io::BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(value) => events.push(value),
            Err(_) => bad += 1,
        }
    }
    if bad > 0 {
        crate::debug_log::append(
            "chief",
            "warn",
            session_id,
            "replay skipped malformed tee lines",
            Some(&bad.to_string()),
        );
    }
    // P0-3: remember that a full replay happened (start seq, size, bad lines).
    // A full read is never truncated.
    record_replay(
        session_id,
        consumed_seq(session_id),
        events.len() as u64,
        bad as u64,
        false,
    );
    Ok((events, bad))
}

/// P0-3 recovery read: at most `limit` events with `seq > after_seq`.
///
/// The ledger is append-only, so a caller can walk it in bounded segments and
/// feed each batch through the same `applyStreamPart` path. `has_more` is
/// observed (not guessed): after the limit is reached we keep scanning for one
/// more non-empty line.
pub fn replay_segment(
    session_id: &str,
    after_seq: u64,
    limit: usize,
) -> Result<ReplaySegment, String> {
    validate_session_id(session_id)?;
    let limit = limit.clamp(1, REPLAY_SEGMENT_MAX);
    let path = events_path(&sessions_root()?, session_id);
    let file = File::open(&path).map_err(|e| format!("chief tee: open replay failed: {e}"))?;
    let mut events = Vec::with_capacity(limit.min(256));
    let mut bad = 0usize;
    let mut has_more = false;
    let mut next_seq = after_seq;
    let mut lines = std::io::BufReader::new(file).lines();
    while let Some(line) = lines.next() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(&line) {
            Ok(value) => {
                let seq = value.get("seq").and_then(Value::as_u64).unwrap_or(0);
                if seq <= after_seq {
                    continue;
                }
                if events.len() >= limit {
                    // One more real record exists beyond this segment.
                    has_more = true;
                    break;
                }
                next_seq = next_seq.max(seq);
                events.push(value);
            }
            Err(_) => bad += 1,
        }
    }
    if bad > 0 {
        crate::debug_log::append(
            "chief",
            "warn",
            session_id,
            "replay skipped malformed tee lines",
            Some(&bad.to_string()),
        );
    }
    record_replay(session_id, after_seq, events.len() as u64, bad as u64, has_more);
    Ok(ReplaySegment {
        events,
        bad_lines: bad,
        has_more,
        next_seq,
    })
}

fn count_lines(path: &std::path::Path) -> u64 {
    match File::open(path) {
        Ok(file) => std::io::BufReader::new(file).lines().count() as u64,
        Err(_) => 0,
    }
}

// ── 回填消费游标 ──────────────────────────────────────────────────────────
//
// 前端把 tee 事件回填进会话流后推进游标；下次回放只给 seq 更大的事件。
// 纪律（App 侧回填）：先回填成功（rail + 落盘挂起）再 consume——崩溃窗口
// 最坏是「回放过但没 consume」= 重复一次回放，宁可重复不可丢。

static CONSUMED: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();

fn consumed_map() -> &'static Mutex<HashMap<String, u64>> {
    CONSUMED.get_or_init(|| Mutex::new(HashMap::new()))
}

fn consumed_path() -> Result<PathBuf, String> {
    sessions_root().map(|dir| dir.join(".consumed.json"))
}

fn consumed_cache(session_id: &str) -> u64 {
    if let Ok(map) = consumed_map().lock() {
        if let Some(seq) = map.get(session_id) {
            return *seq;
        }
    }
    // 内存 miss：读盘（fail-open 成 0）。
    let from_disk = consumed_path()
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|raw| serde_json::from_str::<HashMap<String, u64>>(&raw).ok())
        .and_then(|map| map.get(session_id).copied())
        .unwrap_or(0);
    if let Ok(mut map) = consumed_map().lock() {
        map.insert(session_id.to_string(), from_disk);
    }
    from_disk
}

/// 已消费到的 seq（回放侧用它过滤）。
pub fn consumed_seq(session_id: &str) -> u64 {
    validate_session_id(session_id).ok();
    consumed_cache(session_id)
}

/// 推进消费游标（幂等：只前进不后退）。
pub fn set_consumed_seq(session_id: &str, upto_seq: u64) -> Result<(), String> {
    validate_session_id(session_id)?;
    let mut persist = HashMap::new();
    {
        let mut map = consumed_map()
            .lock()
            .map_err(|_| "chief tee cursor map poisoned")?;
        let current = map.entry(session_id.to_string()).or_insert(0);
        if upto_seq > *current {
            *current = upto_seq;
            // 把整表带到磁盘（表小；单写者 = 前端 consume 命令）。
            for (key, value) in map.iter() {
                persist.insert(key.clone(), *value);
            }
        }
    }
    if persist.is_empty() {
        return Ok(());
    }
    let path = consumed_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("chief tee: mkdir failed: {e}"))?;
    }
    let body = serde_json::to_string(&persist).map_err(|e| e.to_string())?;
    crate::storage::write_atomic(&path, &body)
}

fn spawn_writer(
    session_id: String,
    path: PathBuf,
    rx: Receiver<Value>,
    seed_seq: u64,
    written: std::sync::Arc<AtomicU64>,
) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name(format!("chief-tee-{session_id}"))
        .spawn(move || {
            let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
                Ok(file) => file,
                Err(err) => {
                    crate::debug_log::append(
                        "chief",
                        "error",
                        &session_id,
                        "tee file open failed; events for this session are not persisted",
                        Some(&err.to_string()),
                    );
                    return;
                }
            };
            let mut writer = BufWriter::new(&mut file);
            let mut seq = seed_seq;
            for mut event in rx {
                seq += 1;
                if let Some(obj) = event.as_object_mut() {
                    obj.insert("seq".into(), json!(seq));
                    obj.insert(
                        "ts".into(),
                        json!(std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0)),
                    );
                }
                match serde_json::to_string(&event) {
                    Ok(line) => {
                        if writeln!(writer, "{line}").is_ok() && writer.flush().is_ok() {
                            written.fetch_add(1, Ordering::Relaxed);
                            continue;
                        }
                        // 写失败（盘满/句柄失效）：记一次后继续收干队列，
                        // 后续事件同样落不下去，但 agent 流不受影响。
                        crate::debug_log::append(
                            "chief",
                            "error",
                            &session_id,
                            "tee write failed",
                            None,
                        );
                    }
                    Err(_) => {}
                }
            }
            let _ = writer.flush();
            drop(writer); // 先归还 `&mut file`，才能 sync_all
            let _ = file.sync_all();
        })
        .expect("spawn chief tee writer")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// tee 测试共用一把锁：registry 与 override 都是进程级静态，互踩。
    static SANDBOX_LOCK: Mutex<()> = Mutex::new(());

    fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "chief-tee-test-{}-{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn observe_simple(session_id: &str, kind: &str) {
        observe(session_id, kind, Some("session/update"), &json!({"n": 1}));
    }

    fn seqs_of(events: &[Value]) -> Vec<u64> {
        events
            .iter()
            .map(|e| e.get("seq").and_then(Value::as_u64).unwrap())
            .collect()
    }

    #[test]
    fn roundtrip_preserves_order_and_shape() {
        let guard = SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("roundtrip");
        set_root_override_for_tests(Some(root.clone()));
        let sid = "session_tee_roundtrip";
        register(sid).unwrap();
        observe_simple(sid, "status");
        observe_simple(sid, "acp");
        observe_simple(sid, "acp");
        unregister(sid);
        let (events, bad) = replay(sid).unwrap();
        assert_eq!(bad, 0);
        assert_eq!(events.len(), 3);
        assert_eq!(seqs_of(&events), vec![1, 2, 3]);
        for event in &events {
            assert_eq!(event.get("sessionId").and_then(Value::as_str), Some(sid));
            assert!(event.get("ts").and_then(Value::as_u64).unwrap() > 0);
            assert_eq!(
                event.get("method").and_then(Value::as_str),
                Some("session/update")
            );
            assert_eq!(event.pointer("/data/n").and_then(Value::as_i64), Some(1));
        }
        set_root_override_for_tests(None);
        drop(guard);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn reopen_continues_seq() {
        let guard = SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("reopen");
        set_root_override_for_tests(Some(root.clone()));
        let sid = "session_tee_reopen";
        register(sid).unwrap();
        observe_simple(sid, "status");
        unregister(sid);
        // 模拟应用重启后重新挂 tee：seq 接着数，不回 1。
        register(sid).unwrap();
        observe_simple(sid, "acp");
        unregister(sid);
        let (events, _) = replay(sid).unwrap();
        assert_eq!(seqs_of(&events), vec![1, 2]);
        set_root_override_for_tests(None);
        drop(guard);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn rejects_unusable_session_ids() {
        let guard = SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("badid");
        set_root_override_for_tests(Some(root.clone()));
        assert!(register("../evil").is_err());
        assert!(register("").is_err());
        assert!(register("a b").is_err());
        // 未注册的 observe 是纯 no-op，不 panic。
        observe_simple("../evil", "status");
        set_root_override_for_tests(None);
        drop(guard);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn register_is_idempotent() {
        let guard = SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("idem");
        set_root_override_for_tests(Some(root.clone()));
        let sid = "session_tee_idempotent";
        let path1 = register(sid).unwrap();
        let path2 = register(sid).unwrap();
        assert_eq!(path1, path2);
        observe_simple(sid, "status");
        unregister(sid);
        // 幂等注册不该写出重复行。
        let (events, _) = replay(sid).unwrap();
        assert_eq!(events.len(), 1);
        set_root_override_for_tests(None);
        drop(guard);
        std::fs::remove_dir_all(&root).ok();
    }

    /// 队列满时 observe 丢事件并计数，绝不阻塞（能跑完本身就是断言）。
    /// writer 写盘的速度与调度有关，丢多少不确定；只验证 no-deadlock
    /// 与对账总量不超过发送量。
    #[test]
    fn full_queue_drops_without_blocking() {
        let guard = SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("drop");
        set_root_override_for_tests(Some(root.clone()));
        let sid = "session_tee_drop";
        register(sid).unwrap();
        let blob = "x".repeat(8 * 1024);
        let sent = QUEUE_CAP * 2;
        for _ in 0..sent {
            observe(sid, "acp", None, &json!({"blob": blob}));
        }
        unregister(sid);
        let (events, _) = replay(sid).unwrap();
        assert!(!events.is_empty());
        assert!(events.len() <= sent);
        set_root_override_for_tests(None);
        drop(guard);
        std::fs::remove_dir_all(&root).ok();
    }

    /// P0-2: the consume cursor is a one-way replay filter. `replay()` must
    /// keep returning the whole raw ledger, and re-registering a session must
    /// continue seq from the file, never from the cursor. The frontend relies
    /// on this to rebuild an empty canonical transcript from seq 0.
    #[test]
    fn cursor_is_monotonic_and_replay_stays_append_only() {
        let guard = SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("cursor");
        set_root_override_for_tests(Some(root.clone()));
        let sid = "session_tee_cursor";
        register(sid).unwrap();
        observe_simple(sid, "status");
        observe_simple(sid, "acp");
        observe_simple(sid, "acp");
        unregister(sid);

        set_consumed_seq(sid, 2).unwrap();
        set_consumed_seq(sid, 1).unwrap(); // backwards is ignored
        assert_eq!(consumed_seq(sid), 2);

        // Cursor never deletes evidence: a rebuild from an empty transcript
        // starts at 0 and gets every raw event back.
        let (events, _) = replay(sid).unwrap();
        assert_eq!(events.len(), 3);
        let after_zero = events
            .iter()
            .filter(|e| e.get("seq").and_then(Value::as_u64).unwrap_or(0) > 0)
            .count();
        let after_two = events
            .iter()
            .filter(|e| e.get("seq").and_then(Value::as_u64).unwrap_or(0) > 2)
            .count();
        assert_eq!((after_zero, after_two), (3, 1));

        // Re-register continues seq from the file line count, not the cursor.
        register(sid).unwrap();
        observe_simple(sid, "acp");
        unregister(sid);
        let (events, _) = replay(sid).unwrap();
        assert_eq!(seqs_of(&events), vec![1, 2, 3, 4]);

        set_root_override_for_tests(None);
        drop(guard);
        std::fs::remove_dir_all(&root).ok();
    }

    /// P0-3: health is durable across a process restart and reports the same
    /// facts the close-time log used to hold only in memory.
    #[test]
    fn health_persists_counters_and_survives_reregister() {
        let guard = SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("health");
        set_root_override_for_tests(Some(root.clone()));
        let sid = "session_tee_health";

        register(sid).unwrap();
        observe_simple(sid, "status");
        observe_simple(sid, "acp");
        observe_simple(sid, "acp");
        unregister(sid);

        let first = health(sid).unwrap();
        assert_eq!(first.file_lines, 3);
        assert!(first.enqueued >= 3);
        assert!(first.written >= 3);
        assert_eq!(first.dropped, 0);
        assert!(!first.registered);
        assert!(first.updated_at > 0);

        // A later run must continue the lifetime counters, not restart at 0.
        register(sid).unwrap();
        let live = health(sid).unwrap();
        assert!(live.registered);
        assert!(live.enqueued >= 3);
        observe_simple(sid, "acp");
        unregister(sid);

        let second = health(sid).unwrap();
        assert_eq!(second.file_lines, 4);
        assert!(second.enqueued >= 4);
        assert!(second.written >= 4);

        // The dropped count is a persisted fact even when no writer is alive.
        let persisted = TeeHealthCounters {
            dropped: 7,
            ..Default::default()
        };
        save_health(&root, sid, &persisted);
        assert_eq!(health(sid).unwrap().dropped, 7);

        set_root_override_for_tests(None);
        drop(guard);
        std::fs::remove_dir_all(&root).ok();
    }

    /// P0-3: replay records start seq / size / bad lines, and the segment
    /// reader reports truncation honestly.
    #[test]
    fn replay_metrics_and_segments_are_honest() {
        let guard = SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("health-replay");
        set_root_override_for_tests(Some(root.clone()));
        let sid = "session_tee_health_replay";

        register(sid).unwrap();
        for _ in 0..4 {
            observe_simple(sid, "acp");
        }
        unregister(sid);
        set_consumed_seq(sid, 2).unwrap();

        let (events, bad) = replay(sid).unwrap();
        assert_eq!((events.len(), bad), (4, 0));
        let after_full = health(sid).unwrap();
        assert_eq!(after_full.replay_start_seq, 2);
        assert_eq!(after_full.replay_events, 4);
        assert!(!after_full.replay_truncated);
        assert_eq!(after_full.consumed_seq, 2);

        let first = replay_segment(sid, 2, 1).unwrap();
        assert_eq!(first.events.len(), 1);
        assert_eq!(first.next_seq, 3);
        assert!(first.has_more, "more records exist after the first segment");
        let after_segment = health(sid).unwrap();
        assert!(!after_segment.replay_truncated || after_segment.replay_events == 1);
        assert_eq!(after_segment.replay_start_seq, 2);

        let rest = replay_segment(sid, 3, 10).unwrap();
        assert_eq!(rest.events.len(), 1);
        assert!(!rest.has_more);
        assert_eq!(rest.next_seq, 4);
        assert!(!health(sid).unwrap().replay_truncated);

        // A malformed line is counted (and skipped), never fabricated away.
        let path = events_path(&root, sid);
        let mut body = std::fs::read_to_string(&path).unwrap();
        body.push_str("{not json}\n");
        std::fs::write(&path, body).unwrap();
        let (events, bad) = replay(sid).unwrap();
        assert_eq!(events.len(), 4);
        assert_eq!(bad, 1);
        assert_eq!(health(sid).unwrap().bad_lines, 1);

        set_root_override_for_tests(None);
        drop(guard);
        std::fs::remove_dir_all(&root).ok();
    }

    /// P0-3: an unflushed run (health file missing/stale) still reports the
    /// file's lines as written; health never claims less than the disk holds.
    #[test]
    fn health_written_floor_matches_the_ledger() {
        let guard = SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("health-floor");
        set_root_override_for_tests(Some(root.clone()));
        let sid = "session_tee_health_floor";
        register(sid).unwrap();
        observe_simple(sid, "status");
        observe_simple(sid, "acp");
        unregister(sid);
        // Simulate a crash that lost the health file but kept the ledger.
        std::fs::remove_file(health_path(&root, sid)).ok();
        let after = health(sid).unwrap();
        assert_eq!(after.file_lines, 2);
        assert!(after.enqueued >= 2);
        assert!(after.written >= 2);
        set_root_override_for_tests(None);
        drop(guard);
        std::fs::remove_dir_all(&root).ok();
    }
}
