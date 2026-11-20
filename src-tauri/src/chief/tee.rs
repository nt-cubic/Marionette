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

use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufWriter, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// 每会话事件队列容量。正常时远不用满；满了说明磁盘或 writer 出问题，
/// 丢事件计日志而不是把 agent 的 stdout 管道憋死（见 acp.rs 背压注释）。
const QUEUE_CAP: usize = 2048;
/// unregister 时等 writer 线程收尾的最长时间；超时就放弃等待并记日志。
const DRAIN_WAIT: Duration = Duration::from_secs(5);

static TEES: OnceLock<Mutex<HashMap<String, TeeHandle>>> = OnceLock::new();

/// 测试沙箱根（对齐 council store 的 `set_root_override_for_tests`）。
static ROOT_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

struct TeeHandle {
    tx: SyncSender<Value>,
    writer: Option<std::thread::JoinHandle<()>>,
    /// observe 侧因队列满丢掉的事件总数。
    dropped: std::sync::Arc<AtomicU64>,
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
    let (tx, rx) = std::sync::mpsc::sync_channel::<Value>(QUEUE_CAP);
    let dropped = std::sync::Arc::new(AtomicU64::new(0));
    let writer = spawn_writer(session_id.to_string(), path.clone(), rx, seed_seq);

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
        },
    );
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
        guard
            .get(session_id)
            .map(|handle| (handle.tx.clone(), handle.dropped.clone()))
    };
    let Some((tx, dropped)) = sender else { return };
    let event = json!({
        "sessionId": session_id,
        "kind": kind,
        "method": method,
        "data": data,
    });
    if let Err(TrySendError::Full(_)) = tx.try_send(event) {
        dropped.fetch_add(1, Ordering::Relaxed);
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
    Ok((events, bad))
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

fn spawn_writer(session_id: String, path: PathBuf, rx: Receiver<Value>, seed_seq: u64) -> std::thread::JoinHandle<()> {
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
}
