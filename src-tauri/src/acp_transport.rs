//! ACP transport: JSON-RPC requests over the agent's stdio pipe.
//!
//! Pure pipe layer cut out of acp.rs (request / notify / stdin writer /
//! write_* helpers). Child module of acp: `AcpProcess` and friends live in
//! `super`. The stdout reader + method dispatch stay in acp.rs (business logic).

use super::*;
use serde_json::{json, Value};
use std::io::Write;
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

pub(super) fn request(process: &Arc<AcpProcess>, method: &str, params: Value) -> Result<Value, String> {
    request_with_timeout(process, method, params, Duration::from_secs(60), false)
}

/// Like `request`, but a failure is not emitted as a user-visible ACP error.
///
/// Used for capability probes: the caller inspects the Err and either retries
/// on a different transport or surfaces a real error itself.
pub(super) fn request_quiet(process: &Arc<AcpProcess>, method: &str, params: Value) -> Result<Value, String> {
    request_with_timeout(process, method, params, Duration::from_secs(60), true)
}

pub(super) fn request_with_timeout(
    process: &Arc<AcpProcess>,
    method: &str,
    params: Value,
    timeout: Duration,
    quiet: bool,
) -> Result<Value, String> {
    let id = process.next_id.fetch_add(1, Ordering::Relaxed);
    if quiet {
        if let Ok(mut quiet_ids) = process.quiet_ids.lock() {
            quiet_ids.insert(id);
        }
    }
    let (sender, receiver) = channel();
    process
        .pending
        .lock()
        .map_err(|_| "ACP pending lock poisoned".to_string())?
        .insert(id, sender);
    if let Err(error) = write_message(
        process,
        json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }),
    ) {
        let _ = process
            .pending
            .lock()
            .map(|mut pending| pending.remove(&id));
        let _ = process
            .quiet_ids
            .lock()
            .map(|mut quiet_ids| quiet_ids.remove(&id));
        return Err(error);
    }
    match receiver.recv_timeout(timeout) {
        Ok(result) => {
            // Successful quiet probes still clean up; failures are cleaned in
            // read_stdout when deciding whether to emit the error.
            let _ = process
                .quiet_ids
                .lock()
                .map(|mut quiet_ids| quiet_ids.remove(&id));
            result
        }
        Err(error) => {
            let _ = process
                .pending
                .lock()
                .map(|mut pending| pending.remove(&id));
            let _ = process
                .quiet_ids
                .lock()
                .map(|mut quiet_ids| quiet_ids.remove(&id));
            // Disconnected 不是超时：是应答通道关闭（agent stdin 写线程已退出/
            // 进程已死）。以前统一报 "timed out"，排障时方向全错。
            let reason = match error {
                RecvTimeoutError::Timeout => format!("timed out after {}s", timeout.as_secs()),
                RecvTimeoutError::Disconnected => {
                    "response channel closed before answering (agent gone?)".to_string()
                }
            };
            Err(format!("ACP request `{method}` {reason}"))
        }
    }
}

pub(super) fn json_rpc_id_as_u64(id: &Value) -> Option<u64> {
    id.as_u64()
        .or_else(|| id.as_i64().and_then(|n| u64::try_from(n).ok()))
        .or_else(|| id.as_str()?.parse().ok())
}

pub(super) fn notify(process: &Arc<AcpProcess>, method: &str, params: Value) -> Result<(), String> {
    write_message(
        process,
        json!({ "jsonrpc": "2.0", "method": method, "params": params }),
    )
}

/// Own the agent's stdin on a dedicated thread and feed it from a queue.
///
/// The pipe write is the one operation here that can block for an unbounded
/// time: if the agent stops draining stdin (stuck in a slow model call, hung
/// tool, deadlocked runtime) the OS pipe buffer fills and `write` parks. When
/// that happened on a caller's thread it took the stdin mutex down with it and
/// wedged every other writer. Isolating it here means callers only ever enqueue.
///
/// The trade-off is that enqueueing succeeds before the write is attempted, so
/// a write failure has no caller left to return an error to — `on_fail` is how
/// that surfaces instead. Without it a dropped `session/prompt` would leave the
/// UI streaming forever against a pipe nobody is reading.
pub(super) fn spawn_stdin_writer(
    mut stdin: impl Write + Send + 'static,
    session_id: String,
    on_fail: impl FnOnce(String) + Send + 'static,
) -> Sender<Value> {
    let (tx, rx) = channel::<Value>();
    thread::spawn(move || {
        // Ends when every Sender drops (process torn down) or the pipe breaks
        // (child killed) — no other shutdown signal is needed.
        for message in rx {
            let line = match serde_json::to_string(&message) {
                Ok(line) => line,
                Err(error) => {
                    crate::debug_log::append(
                        "acp",
                        "warn",
                        &session_id,
                        "drop unserializable outbound message",
                        Some(&error.to_string()),
                    );
                    continue;
                }
            };
            if let Err(error) = writeln!(stdin, "{line}").and_then(|()| stdin.flush()) {
                crate::debug_log::append(
                    "acp",
                    "warn",
                    &session_id,
                    "stdin writer stopped",
                    Some(&error.to_string()),
                );
                on_fail(error.to_string());
                return;
            }
        }
    });
    tx
}

/// Enqueue an outbound message. Never blocks: the queue is unbounded and the
/// lock is only held for the hand-off, never across the pipe write.
pub(super) fn write_message(process: &Arc<AcpProcess>, message: Value) -> Result<(), String> {
    process
        .stdin_tx
        .lock()
        .map_err(|_| "ACP stdin lock poisoned".to_string())?
        .send(message)
        .map_err(|_| "ACP agent stdin is closed (process gone?)".to_string())
}

pub(super) fn write_response(process: &Arc<AcpProcess>, id: Value, result: Value) {
    let _ = write_message(
        process,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": result
        }),
    );
}

pub(super) fn write_error_response(process: &Arc<AcpProcess>, id: Value, code: i64, message: &str) {
    let _ = write_message(
        process,
        json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        }),
    );
}
