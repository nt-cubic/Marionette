//! 最小 HTTP/1.1 服务器（loopback-only，std 实现，零新依赖）。
//!
//! 只服务引擎合同面：请求体走 Content-Length（上限 64MB——codex 整轮截图
//! 内联时 30MB+），响应两种形态——完整 JSON（带 Content-Length）和流式
//! SSE 中继（边读边 flush，Connection: close）。不实现 keep-alive/chunked
//! 请求体（账本任务：codex 若发 chunked 再补）。
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::Gateway;

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
/// 停止轮询间隔（进程内形态）：accept 非阻塞轮询的节拍，也是 `shutdown`
/// 置位后监听关闭的最大延迟——engine.rs 的 kill 路径要在下一次端口探测
/// （update → place_and_prep → find_free_port_from）前把端口还回去。
const SHUTDOWN_POLL: Duration = Duration::from_millis(50);

pub struct Request {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Request {
    /// 大小写不敏感取头（名字已统一小写存储）。
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }
}

pub enum Reply {
    /// 完整 JSON 应答（status + body，Content-Length 由写层补）。
    Json { status: u16, body: Vec<u8> },
    /// 流式应答（SSE 中继）：状态与 content-type 透传，reader 的字节随到随写。
    Stream {
        status: u16,
        content_type: String,
        reader: Box<dyn Read + Send>,
    },
}

impl Reply {
    pub fn json(status: u16, value: &serde_json::Value) -> Reply {
        Reply::Json {
            status,
            body: serde_json::to_vec(value).unwrap_or_default(),
        }
    }

    pub fn error(status: u16, message: &str) -> Reply {
        Reply::json(
            status,
            &serde_json::json!({ "error": { "message": message, "type": "engine_error" } }),
        )
    }
}

/// 在 `addr` 上监听并逐连接开线程（argv/合同套件形态：进程由外部杀，
/// 没有停止钮——`serve_on` 的 shutdown 传一个永不置位的标志）。
pub fn serve(addr: &str, gateway: std::sync::Arc<Gateway>) -> Result<(), String> {
    let listener = TcpListener::bind(addr).map_err(|e| format!("bind {addr}: {e}"))?;
    let bound = listener.local_addr().map_err(|e| e.to_string())?;
    println!("magpie gateway on http://{}", bound);
    serve_on(listener, gateway, Arc::new(AtomicBool::new(false)))
}

/// 进程内形态（engine.rs T6 接线）：接管一个**已绑定**的 listener——
/// engine.rs 先 bind 定死端口再交给后台线程，消灭「挑端口→释放→重绑」
/// 的竞态；accept 走非阻塞轮询，`shutdown` 置位后 ≤`SHUTDOWN_POLL` 内
/// 干净退出（kill_engine / 空闲回收 / 引擎更新都要把端口还回去）。
/// **这里绝不 println!**——GUI 进程是 windows 子系统、没有控制台，
/// 写 stdout 会 panic；诊断信息由调用方（engine.rs）记自己的日志。
pub fn serve_on(
    listener: TcpListener,
    gateway: std::sync::Arc<Gateway>,
    shutdown: Arc<AtomicBool>,
) -> Result<(), String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("set listener nonblocking: {e}"))?;
    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                // Linux 上 accept 出的流会继承 O_NONBLOCK（Windows 不会）——
                // 统一拨回阻塞，handler 的读写语义与 argv 形态完全一致。
                let _ = stream.set_nonblocking(false);
                let gw = gateway.clone();
                std::thread::spawn(move || {
                    let _ = handle(stream, &gw);
                });
            }
            // 空闲：睡一个轮询间隔再试（连接在内核 backlog 里排队，不丢）。
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::Interrupted =>
            {
                std::thread::sleep(SHUTDOWN_POLL);
            }
            // 监听套接字坏了（被外部关闭等）——退出，存活语义交调用方
            // （engine.rs 用 exited 标志把死线程翻成 Failed）。
            Err(_) => break,
        }
    }
    Ok(())
}

fn handle(stream: TcpStream, gateway: &Gateway) -> std::io::Result<()> {
    let request = match parse(stream.try_clone()?) {
        Some(r) => r,
        None => {
            let mut s = stream;
            return write_reply(
                &mut s,
                Reply::error(400, "malformed request"),
            );
        }
    };
    let reply = super::routes::route(&request, gateway);
    let mut stream = stream;
    write_reply(&mut stream, reply)
}

fn parse(stream: TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream);
    let mut start = String::new();
    reader.read_line(&mut start).ok()?;
    let mut parts = start.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    let mut headers = Vec::new();
    let mut header_bytes = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        header_bytes += line.len();
        if header_bytes > MAX_HEADER_BYTES {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }

    let length = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    if length > MAX_BODY_BYTES {
        return None;
    }
    let mut body = vec![0u8; length];
    if length > 0 {
        reader.read_exact(&mut body).ok()?;
    }
    Some(Request { method, path, headers, body })
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        204 => "No Content",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        413 => "Payload Too Large",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Response",
    }
}

fn write_reply(stream: &mut TcpStream, reply: Reply) -> std::io::Result<()> {
    match reply {
        Reply::Json { status, body } => {
            let head = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                status,
                status_text(status),
                body.len()
            );
            stream.write_all(head.as_bytes())?;
            stream.write_all(&body)?;
        }
        Reply::Stream { status, content_type, mut reader } => {
            let head = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n",
                status,
                status_text(status),
                content_type
            );
            stream.write_all(head.as_bytes())?;
            let mut buf = [0u8; 16 * 1024];
            loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break,
                };
                stream.write_all(&buf[..n])?;
                stream.flush()?;
            }
        }
    }
    stream.flush()
}
