//! 最小 HTTP/1.1 服务器（loopback-only，std 实现，零新依赖）。
//!
//! 只服务引擎合同面：请求体支持 Content-Length 与 Transfer-Encoding:
//! chunked（codex 可能发 chunked；解码成完整 body 再路由，上限同 64MB
//! ——codex 整轮截图内联时 30MB+），响应两种形态——完整 JSON（带
//! Content-Length）和流式 SSE 中继（边读边 flush，Connection: close）。
//!
//! 加固（账本 T11）：请求行/头读 30s、头读完**体不限**、写 60s；并发连接
//! 上限 64（超过直接 503 关闭）；不做 keep-alive（维持 close 语义）。
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use super::Gateway;

const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;
/// 停止轮询间隔（进程内形态）：accept 非阻塞轮询的节拍，也是 `shutdown`
/// 置位后监听关闭的最大延迟——engine.rs 的 kill 路径要在下一次端口探测
/// （update → place_and_prep → find_free_port_from）前把端口还回去。
const SHUTDOWN_POLL: Duration = Duration::from_millis(50);
/// 请求行/头读超时（慢连接兜底；头读完体不限——大体 + 慢客户端合法）。
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);
/// 写超时（应答头/体/SSE 每次写各算一次；客户端断开靠写错误退出流循环）。
const WRITE_TIMEOUT: Duration = Duration::from_secs(60);
/// 并发连接上限：超过的直接 503 关闭（thread-per-connection 的兜底）。
const MAX_CONNECTIONS: usize = 64;
/// chunked 尺寸行/尾随头的单行长度护栏（尺寸行按 RFC 只需几位 hex）。
const MAX_CHUNK_LINE_BYTES: usize = 8 * 1024;

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
    // 并发连接计数：超过上限的直接 503 关闭（不进 handle、不占线程）。
    let live = Arc::new(AtomicUsize::new(0));
    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                // Linux 上 accept 出的流会继承 O_NONBLOCK（Windows 不会）——
                // 统一拨回阻塞，handler 的读写语义与 argv 形态完全一致。
                let _ = stream.set_nonblocking(false);
                // 先占位再判（fetch_add 返回旧值）；超限的退位 + 503 关闭。
                if live.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
                    live.fetch_sub(1, Ordering::AcqRel);
                    let mut stream = stream;
                    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
                    let _ = write_reply(
                        &mut stream,
                        Reply::error(503, "engine busy: too many connections"),
                    );
                    continue;
                }
                let gw = gateway.clone();
                let live = live.clone();
                std::thread::spawn(move || {
                    let _ = handle(stream, &gw);
                    live.fetch_sub(1, Ordering::AcqRel);
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
    // 读超时只罩请求行与头（parse 在头读完后自行解除）；写超时罩整个应答。
    let _ = stream.set_read_timeout(Some(HEADER_READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
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
    // 头读完：体不限（大体 + 慢客户端合法），解除读超时。
    let _ = reader.get_ref().set_read_timeout(None);

    // Transfer-Encoding: chunked 优先于 Content-Length（RFC 7230 §3.3.3；
    // codex 可能发 chunked）。解码成完整 body 再路由，上限同 64MB。
    let chunked = headers
        .iter()
        .any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
    let body = if chunked {
        decode_chunked(&mut reader, MAX_BODY_BYTES)?
    } else {
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
        body
    };
    Some(Request { method, path, headers, body })
}

/// 解码 chunked 传输体（RFC 7230 §4.1）：尺寸行（hex，可带 `;扩展`）→ 数据
/// → CRLF，0 尺寸块之后读尾随头直到空行（读完即弃）。累计超过 `max`、尺寸
/// 行非法/溢出、数据后缺 CRLF → None（调用方按坏请求处理）。
/// 做成纯函数（BufRead 进 BufRead 出）便于直接单测。
fn decode_chunked<R: BufRead>(reader: &mut R, max: usize) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).ok()?;
        if line.len() > MAX_CHUNK_LINE_BYTES {
            return None;
        }
        // chunk-size [ chunk-ext ]：扩展从 `;` 起丢弃
        let size_part = line.split(';').next()?.trim();
        let size = usize::from_str_radix(size_part, 16).ok()?;
        if size == 0 {
            // 尾随头段：头行直到空行（不进 Request，读完即弃）
            let mut trailer_bytes = 0usize;
            loop {
                let mut t = String::new();
                reader.read_line(&mut t).ok()?;
                trailer_bytes += t.len();
                if trailer_bytes > MAX_CHUNK_LINE_BYTES {
                    return None;
                }
                if t.trim_end().is_empty() {
                    return Some(body);
                }
            }
        }
        let new_len = body.len().checked_add(size)?;
        if new_len > max {
            return None;
        }
        let start = body.len();
        body.resize(new_len, 0);
        reader.read_exact(&mut body[start..]).ok()?;
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).ok()?;
        if &crlf != b"\r\n" {
            return None;
        }
    }
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
                // 客户端断开 → 写错误 → `?` 带 Err 出函数、连接关闭，SSE
                // 循环随之退出（不做 keep-alive，维持 Connection: close 语义）。
                stream.write_all(&buf[..n])?;
                stream.flush()?;
            }
        }
    }
    stream.flush()
}

// ---------------------------------------------------------------------------
// 测试（任务 T11 验收：chunked 解码为主，纯函数直测 + 两例真 TCP 走 parse）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn chunked_decodes_basic() {
        let mut r = BufReader::new(Cursor::new(b"4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n".to_vec()));
        assert_eq!(decode_chunked(&mut r, 1024).as_deref(), Some(b"Wikipedia".as_slice()));
    }

    #[test]
    fn chunked_empty_body_and_single() {
        let mut r = BufReader::new(Cursor::new(b"0\r\n\r\n".to_vec()));
        assert_eq!(decode_chunked(&mut r, 1024), Some(Vec::new()));
        let mut r = BufReader::new(Cursor::new(b"5\r\nhello\r\n0\r\n\r\n".to_vec()));
        assert_eq!(decode_chunked(&mut r, 1024).as_deref(), Some(b"hello".as_slice()));
    }

    #[test]
    fn chunked_accepts_hex_uppercase_extensions_and_trailers() {
        // 1A = 26；分号扩展丢弃；0 块后带尾随头
        let raw =
            b"1A;name=value\r\n0123456789abcdefghijklmnop\r\n0\r\nX-Trailer: v\r\n\r\n".to_vec();
        let mut r = BufReader::new(Cursor::new(raw));
        assert_eq!(
            decode_chunked(&mut r, 1024).as_deref(),
            Some(b"0123456789abcdefghijklmnop".as_slice())
        );
    }

    #[test]
    fn chunked_rejects_bad_size_line() {
        let mut r = BufReader::new(Cursor::new(b"zz\r\nhello\r\n0\r\n\r\n".to_vec()));
        assert_eq!(decode_chunked(&mut r, 1024), None);
        // 流提前断掉（连尺寸行都没有）
        let mut r = BufReader::new(Cursor::new(Vec::new()));
        assert_eq!(decode_chunked(&mut r, 1024), None);
    }

    #[test]
    fn chunked_rejects_missing_crlf_after_data() {
        let mut r = BufReader::new(Cursor::new(b"5\r\nhelloXX0\r\n\r\n".to_vec()));
        assert_eq!(decode_chunked(&mut r, 1024), None);
    }

    #[test]
    fn chunked_enforces_cap() {
        // 两块各 10 字节，cap 15 → 超限拒绝
        let raw = b"a\r\n0123456789\r\na\r\n0123456789\r\n0\r\n\r\n".to_vec();
        let mut r = BufReader::new(Cursor::new(raw));
        assert_eq!(decode_chunked(&mut r, 15), None);
        // 尺寸行数值溢出 usize 也不许 panic（from_str_radix 拒掉）
        let mut r =
            BufReader::new(Cursor::new(b"FFFFFFFFFFFFFFFFFFFF\r\nx\r\n0\r\n\r\n".to_vec()));
        assert_eq!(decode_chunked(&mut r, 1024), None);
    }

    #[test]
    fn parse_reads_chunked_request_over_tcp() {
        // 真 TCP：临时监听 + 手写客户端，覆盖 parse 的头 → chunked 分流
        // （Content-Length 与 Transfer-Encoding 并存时 chunked 赢，RFC 同款）。
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            let mut c = TcpStream::connect(addr).unwrap();
            c.write_all(
                b"POST /v1/chat/completions HTTP/1.1\r\nHost: localhost\r\n\
                  Content-Length: 999\r\nTransfer-Encoding: chunked\r\n\r\n\
                  5\r\nhello\r\n3\r\nabc\r\n0\r\n\r\n",
            )
            .unwrap();
        });
        let (server, _) = listener.accept().unwrap();
        let req = parse(server).expect("parse chunked request");
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/v1/chat/completions");
        assert_eq!(req.body, b"helloabc".to_vec());
        client.join().unwrap();
    }

    #[test]
    fn parse_reads_content_length_request_over_tcp() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().unwrap();
        let client = std::thread::spawn(move || {
            let mut c = TcpStream::connect(addr).unwrap();
            c.write_all(b"GET /v1/models HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\nhi")
                .unwrap();
        });
        let (server, _) = listener.accept().unwrap();
        let req = parse(server).expect("parse content-length request");
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/v1/models");
        assert_eq!(req.body, b"hi".to_vec());
        client.join().unwrap();
    }

    /// T6 停止机制验收：`serve_on` 收到 shutdown 后干净返回、监听端口释放
    /// （kill_engine / 空闲回收 / 引擎更新都要把端口还回去），且停止前的
    /// 请求在轮询循环下正常应答（accept 不吞连接）。网关的 Store 走
    /// XDG 沙箱（quotas/messages 同款共享锁 + env 包裹，绝不碰真实
    /// ~/.config/magpie）。
    #[test]
    fn serve_on_stops_on_flag_and_frees_port() {
        let _lock = crate::engine_gateway::TEST_SANDBOX_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "marionette_test_httpd_stop_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let prior_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        // panic 也不能漏掉 env 恢复与沙箱清理（quotas.rs 同款包裹）。
        let result = std::panic::catch_unwind(|| {
            let gateway = std::sync::Arc::new(Gateway::open().expect("gateway"));
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
            let addr = listener.local_addr().unwrap();
            let shutdown = Arc::new(AtomicBool::new(false));
            let flag = shutdown.clone();
            let gw = gateway.clone();
            let server = std::thread::spawn(move || serve_on(listener, gw, flag));
            // 停止前应答一次 /v1/models（空 store → {"object":"list","data":[]}）。
            let mut c = TcpStream::connect(addr).unwrap();
            c.write_all(b"GET /v1/models HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
                .unwrap();
            let mut body = String::new();
            c.read_to_string(&mut body).unwrap();
            assert!(body.contains("\"data\""), "models reply missing: {body}");
            // 置位 → serve_on 必须干净返回（轮询节拍 50ms，给 5s 余量）。
            shutdown.store(true, Ordering::Release);
            server.join().expect("serve_on panicked").expect("serve_on error");
            // 端口已释放：同一地址能重新 bind（没有残监听挂着端口）。
            assert!(TcpListener::bind(addr).is_ok(), "port {addr} still held after stop");
        });
        match prior_xdg {
            Some(p) => std::env::set_var("XDG_CONFIG_HOME", p),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        result.unwrap();
    }
}
