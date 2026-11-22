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
/// 入站体上限（**解码后**计）：同 Go request_bounds.go 的 defaultBodyLimit
/// 512MB——codex 整轮截图内联 base64、长会话轻松过 64MB。Content-Length
/// 超限在读体前就 413；chunked / 压缩体边读边数，越线即停（zstd 炸弹也
/// 出不了这条线）。读入按实际到达增长，不信声明值预分配。
const MAX_BODY_BYTES: usize = 512 * 1024 * 1024;
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

/// 入站请求（读体 + Content-Encoding 解码之后的形态）。Clone 是给
/// bodyprep::prepared_request 用的：体被预处理改写时按需重建请求。
#[derive(Clone)]
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
/// `token`：Bearer 闸（2026-11-22 安全批）；None = 不鉴权（仅测试用）。
pub fn serve(addr: &str, gateway: std::sync::Arc<Gateway>, token: Option<String>) -> Result<(), String> {
    let listener = TcpListener::bind(addr).map_err(|e| format!("bind {addr}: {e}"))?;
    let bound = listener.local_addr().map_err(|e| e.to_string())?;
    println!("magpie gateway on http://{}", bound);
    serve_on(listener, gateway, Arc::new(AtomicBool::new(false)), token)
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
    token: Option<String>,
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
                        &[],
                    );
                    continue;
                }
                let gw = gateway.clone();
                let live = live.clone();
                let token = token.clone();
                std::thread::spawn(move || {
                    let _ = handle(stream, &gw, token.as_deref());
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

fn handle(stream: TcpStream, gateway: &Gateway, token: Option<&str>) -> std::io::Result<()> {
    // 读超时只罩请求行与头（头读完后解除）；写超时罩整个应答。
    let _ = stream.set_read_timeout(Some(HEADER_READ_TIMEOUT));
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut stream = stream;
    let Some(head) = read_head(&mut reader) else {
        return write_reply(&mut stream, Reply::error(400, "malformed request"), &[]);
    };
    // 外站网页防护先于读体（Go corsGuard 在最外层）：被拒的请求连体都不读。
    let mut extra: Vec<(String, String)> = Vec::new();
    match super::cors::judge(&head.method, &head.headers) {
        super::cors::Verdict::Pass => {}
        super::cors::Verdict::PassWith(h) => extra = h,
        super::cors::Verdict::Preflight(h) => {
            let reply = Reply::Json { status: 204, body: Vec::new() };
            return write_reply(&mut stream, reply, &h);
        }
        super::cors::Verdict::Refuse(message) => {
            return write_reply(&mut stream, guard_error(&head.path, 403, &message), &[]);
        }
    }
    // Bearer 闸（2026-11-22 安全批）：环回上任何本地进程 / 浏览器插件都打得
    // 进来，网关曾零鉴权（S1「任意 token 放行」废止）。除 preflight（CORS
    // 规范不带凭证，上面已经回了 204）外，所有请求都要带
    // `Authorization: Bearer <本安装令牌>`；缺失/错误一律 401，且在读体之前。
    if let Some(expected) = token {
        if !bearer_ok(&head.headers, expected) {
            return write_reply(
                &mut stream,
                guard_error(&head.path, 401, "engine bearer token missing or wrong"),
                &[],
            );
        }
    }
    // 头读完：体不限时（大体 + 慢客户端合法），解除读超时。
    let _ = stream.set_read_timeout(None);
    let body = match read_body(&mut reader, &head.headers) {
        Ok(b) => b,
        Err(e) => {
            let reply = guard_error(&head.path, e.status(), &e.message());
            return write_reply(&mut stream, reply, &extra);
        }
    };
    let mut headers = head.headers;
    // 已解码：Content-Encoding 之后谁都不该再看到/转发它（Go decodedBody 同款）。
    if body.decoded {
        headers.retain(|(k, _)| k != "content-encoding");
    }
    let request = Request { method: head.method, path: head.path, headers, body: body.bytes };
    clear_passthrough();
    let reply = super::routes::route(&request, gateway);
    extra.extend(take_passthrough());
    write_reply(&mut stream, reply, &extra)
}

// ---------------------------------------------------------------------------
// 上游头透传（审计 #9）：Reply 不带头，上游的 Retry-After / request-id /
// ratelimit 头经线程局部侧道交给写层——handler 与写层在同一条连接线程上，
// 每个把上游应答变成 Reply 的点覆盖写一次（最后一次 = 终局应答的头）。
// ---------------------------------------------------------------------------

thread_local! {
    static PASSTHROUGH: std::cell::RefCell<Vec<(String, String)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn clear_passthrough() {
    PASSTHROUGH.with(|p| p.borrow_mut().clear());
}

fn take_passthrough() -> Vec<(String, String)> {
    PASSTHROUGH.with(|p| std::mem::take(&mut *p.borrow_mut()))
}

/// 该透传的上游头：Retry-After（含 -ms 变体）、request id、任何 ratelimit
/// 头（Go keepRetry 只带 retry-after 与 ratelimit…reset，成功应答带 request
/// id；这里取并集——客户端退避与排障都吃得到）。
fn passes_through(name_lower: &str) -> bool {
    name_lower == "retry-after"
        || name_lower == "retry-after-ms"
        || name_lower == "request-id"
        || name_lower == "x-request-id"
        || name_lower.contains("ratelimit")
}

/// 记下 `response` 里该透传的头（覆盖上一次）。每个上游应答被转成 Reply
/// 的地方调一次。值里带控制字符的丢弃（响应拆分防护）。
pub fn note_upstream(response: &ureq::Response) {
    let mut out = Vec::new();
    for name in response.headers_names() {
        let lower = name.to_ascii_lowercase();
        if !passes_through(&lower) {
            continue;
        }
        if let Some(value) = response.header(&name) {
            if !value.bytes().any(|b| b < 0x20 && b != b'\t') {
                out.push((name.clone(), value.to_string()));
            }
        }
    }
    PASSTHROUGH.with(|p| *p.borrow_mut() = out);
}

/// 路由层自己加一个应答头（如 `X-Magpie-Left-Out`），随本次应答写出。
pub fn note_header(name: &str, value: &str) {
    PASSTHROUGH.with(|p| p.borrow_mut().push((name.to_string(), value.to_string())));
}

/// Bearer 闸（见 handle）：`Authorization: Bearer <token>` 对不对。
fn bearer_ok(headers: &[(String, String)], expected: &str) -> bool {
    let Some(value) = headers
        .iter()
        .find(|(k, _)| k == "authorization")
        .map(|(_, v)| v.trim())
    else {
        return false;
    };
    let Some(token) = value.strip_prefix("Bearer ").map(str::trim_start) else {
        return false;
    };
    constant_time_eq(token.as_bytes(), expected.as_bytes())
}

/// 长度不同的先比长度再折叠异或——环回令牌不是高价值机密，但比较仍取
/// 常数时间形状，不做 `==` 的早退（教学式计时侧道就别留了）。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 网关自己的拒绝应答，按入站方言说话（anthropic 端点给 anthropic 形状，
/// 其余 OpenAI 形状；Go writeError(w, from, …)）。类型表同 Go errorTypeFor。
fn guard_error(path: &str, status: u16, message: &str) -> Reply {
    let typ = match status {
        400 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        413 => "request_too_large",
        429 => "rate_limit_error",
        529 => "overloaded_error",
        _ => "api_error",
    };
    let path = path.split('?').next().unwrap_or("");
    if path == "/v1/messages" || path == "/messages" || path == "/v1/messages/count_tokens" {
        Reply::json(
            status,
            &serde_json::json!({ "type": "error", "error": { "type": typ, "message": message } }),
        )
    } else {
        Reply::json(
            status,
            &serde_json::json!({ "error": { "message": message, "type": typ, "code": null } }),
        )
    }
}

/// 读体失败的几种死法（各自对应 Go readBoundedRequestBody 的状态码）。
#[derive(Debug, PartialEq)]
enum BodyErr {
    /// 超过网关上限（解码后）→ 413。
    TooLarge,
    /// 读不完 / chunked 坏 → 400。
    Malformed,
    /// Content-Encoding 不认 → 415。
    UnsupportedEncoding(String),
    /// 声明了 gzip/zstd 但内容不是 → 400。
    BadEncoding(&'static str),
}

impl BodyErr {
    fn status(&self) -> u16 {
        match self {
            BodyErr::TooLarge => 413,
            BodyErr::Malformed | BodyErr::BadEncoding(_) => 400,
            BodyErr::UnsupportedEncoding(_) => 415,
        }
    }

    fn message(&self) -> String {
        match self {
            BodyErr::TooLarge => format!(
                "request body is over magpie's gateway limit of {} MB (decoded); the provider was not asked",
                MAX_BODY_BYTES >> 20
            ),
            BodyErr::Malformed => "malformed request".to_string(),
            BodyErr::UnsupportedEncoding(enc) => format!(
                "unsupported Content-Encoding: magpie can't read a \"{enc}\" body; send it as identity, gzip or zstd"
            ),
            BodyErr::BadEncoding(kind) => format!("request body is not {kind}"),
        }
    }
}

struct Head {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
}

#[derive(Debug)]
struct Body {
    bytes: Vec<u8>,
    /// 经过了 Content-Encoding 解码。
    decoded: bool,
}

fn read_head<R: BufRead>(reader: &mut R) -> Option<Head> {
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
    Some(Head { method, path, headers })
}

enum Codec {
    Identity,
    Zstd,
    Gzip,
}

/// 读体：Content-Encoding 先判（不认的在读体前就拒，Go 同款 415）→
/// Content-Length 越线直接 413 → 读（chunked 优先于 Content-Length，
/// RFC 7230 §3.3.3）→ 按编码解码（解码后同样受上限约束）。
fn read_body<R: BufRead>(reader: &mut R, headers: &[(String, String)]) -> Result<Body, BodyErr> {
    let encoding = headers
        .iter()
        .find(|(k, _)| k == "content-encoding")
        .map(|(_, v)| v.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let codec = match encoding.as_str() {
        "" | "identity" => Codec::Identity,
        "zstd" => Codec::Zstd,
        "gzip" | "x-gzip" => Codec::Gzip,
        other => return Err(BodyErr::UnsupportedEncoding(other.to_string())),
    };
    let chunked = headers
        .iter()
        .any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
    let wire = if chunked {
        decode_chunked_inner(reader, MAX_BODY_BYTES)?
    } else {
        let length = headers
            .iter()
            .find(|(k, _)| k == "content-length")
            .and_then(|(_, v)| v.parse::<usize>().ok())
            .unwrap_or(0);
        if length > MAX_BODY_BYTES {
            return Err(BodyErr::TooLarge);
        }
        read_exact_growing(reader, length)?
    };
    match codec {
        Codec::Identity => Ok(Body { bytes: wire, decoded: false }),
        Codec::Zstd => Ok(Body { bytes: unzstd(&wire, MAX_BODY_BYTES)?, decoded: true }),
        Codec::Gzip => Ok(Body { bytes: ungzip(&wire, MAX_BODY_BYTES)?, decoded: true }),
    }
}

/// 读恰好 `length` 字节，缓冲随到达增长（不信声明值预分配：客户端声明
/// 500MB 却不发，网关只占已到的那点）。少于 length = Malformed。
fn read_exact_growing<R: Read>(reader: &mut R, length: usize) -> Result<Vec<u8>, BodyErr> {
    let mut body = Vec::with_capacity(length.min(1 << 20));
    let got = reader
        .take(length as u64)
        .read_to_end(&mut body)
        .map_err(|_| BodyErr::Malformed)?;
    if got != length {
        return Err(BodyErr::Malformed);
    }
    Ok(body)
}

/// zstd 解码（一个或多个串接帧——append 式归档也认），解码后总量 > `cap` →
/// TooLarge。ruzstd 的 StreamingDecoder 一次只解一帧，按已消耗字节接着找
/// 下一帧魔数。
fn unzstd(wire: &[u8], cap: usize) -> Result<Vec<u8>, BodyErr> {
    const MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
    let mut out: Vec<u8> = Vec::new();
    let mut rest = wire;
    let mut first = true;
    while !rest.is_empty() {
        if rest.len() < 4 || rest[..4] != MAGIC {
            if first {
                return Err(BodyErr::BadEncoding("zstd"));
            }
            break; // 尾随杂物：已解出的算数
        }
        first = false;
        let mut cursor = std::io::Cursor::new(rest);
        let consumed = {
            let mut decoder = ruzstd::StreamingDecoder::new(&mut cursor)
                .map_err(|_| BodyErr::BadEncoding("zstd"))?;
            let room = cap.saturating_sub(out.len()) as u64 + 1;
            (&mut decoder)
                .take(room)
                .read_to_end(&mut out)
                .map_err(|_| BodyErr::BadEncoding("zstd"))?;
            if out.len() > cap {
                return Err(BodyErr::TooLarge);
            }
            cursor.position() as usize
        };
        if consumed == 0 {
            break; // 防御：没有进展
        }
        rest = &rest[consumed.min(rest.len())..];
    }
    Ok(out)
}

/// gzip 解码（多 member 也认，Go gzip.NewReader 默认同款），> `cap` → TooLarge。
fn ungzip(wire: &[u8], cap: usize) -> Result<Vec<u8>, BodyErr> {
    let decoder = flate2::read::MultiGzDecoder::new(wire);
    let mut out = Vec::new();
    decoder
        .take(cap as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|_| BodyErr::BadEncoding("gzip"))?;
    if out.len() > cap {
        return Err(BodyErr::TooLarge);
    }
    Ok(out)
}

/// 整体入口（测试用）：头 + 体，失败 None；压缩体照常解码。
#[cfg(test)]
fn parse(stream: TcpStream) -> Option<Request> {
    let mut reader = BufReader::new(stream);
    let head = read_head(&mut reader)?;
    let body = read_body(&mut reader, &head.headers).ok()?;
    Some(Request { method: head.method, path: head.path, headers: head.headers, body: body.bytes })
}

/// 解码 chunked 传输体（RFC 7230 §4.1）：尺寸行（hex，可带 `;扩展`）→ 数据
/// → CRLF，0 尺寸块之后读尾随头直到空行（读完即弃）。累计超过 `max`、尺寸
/// 行非法/溢出、数据后缺 CRLF → None（调用方按坏请求处理）。
/// 做成纯函数（BufRead 进 BufRead 出）便于直接单测。
fn decode_chunked<R: BufRead>(reader: &mut R, max: usize) -> Option<Vec<u8>> {
    decode_chunked_inner(reader, max).ok()
}

/// [`decode_chunked`] 的带死因版本：越线 = TooLarge（413），其余 Malformed。
/// 块数据按到达增长读入（不按尺寸行声明值预分配）。
fn decode_chunked_inner<R: BufRead>(reader: &mut R, max: usize) -> Result<Vec<u8>, BodyErr> {
    fn bad<E>(_: E) -> BodyErr {
        BodyErr::Malformed
    }
    let mut body = Vec::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(bad)?;
        if line.len() > MAX_CHUNK_LINE_BYTES {
            return Err(BodyErr::Malformed);
        }
        // chunk-size [ chunk-ext ]：扩展从 `;` 起丢弃
        let size_part = line.split(';').next().ok_or(BodyErr::Malformed)?.trim();
        let size = usize::from_str_radix(size_part, 16).map_err(bad)?;
        if size == 0 {
            // 尾随头段：头行直到空行（不进 Request，读完即弃）
            let mut trailer_bytes = 0usize;
            loop {
                let mut t = String::new();
                reader.read_line(&mut t).map_err(bad)?;
                trailer_bytes += t.len();
                if trailer_bytes > MAX_CHUNK_LINE_BYTES {
                    return Err(BodyErr::Malformed);
                }
                if t.trim_end().is_empty() {
                    return Ok(body);
                }
            }
        }
        let new_len = body.len().checked_add(size).ok_or(BodyErr::TooLarge)?;
        if new_len > max {
            return Err(BodyErr::TooLarge);
        }
        let got = reader
            .by_ref()
            .take(size as u64)
            .read_to_end(&mut body)
            .map_err(bad)?;
        if got != size {
            return Err(BodyErr::Malformed);
        }
        let mut crlf = [0u8; 2];
        reader.read_exact(&mut crlf).map_err(bad)?;
        if &crlf != b"\r\n" {
            return Err(BodyErr::Malformed);
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
        415 => "Unsupported Media Type",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Response",
    }
}

/// 额外应答头（CORS / 上游透传）序列化成 `Name: value\r\n` 串；名或值含控制
/// 字符的丢弃（应答拆分防护）。
fn extra_header_lines(extra: &[(String, String)]) -> String {
    let clean = |s: &str| !s.is_empty() && !s.bytes().any(|b| (b < 0x20 && b != b'\t') || b == 0x7f);
    let mut out = String::new();
    for (k, v) in extra {
        if clean(k) && !k.contains(':') && clean(v) {
            out.push_str(k);
            out.push_str(": ");
            out.push_str(v);
            out.push_str("\r\n");
        }
    }
    out
}

fn write_reply(
    stream: &mut TcpStream,
    reply: Reply,
    extra: &[(String, String)],
) -> std::io::Result<()> {
    let extra = extra_header_lines(extra);
    match reply {
        Reply::Json { status, body } => {
            let head = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{}Connection: close\r\n\r\n",
                status,
                status_text(status),
                body.len(),
                extra
            );
            stream.write_all(head.as_bytes())?;
            stream.write_all(&body)?;
        }
        Reply::Stream { status, content_type, mut reader } => {
            let head = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nCache-Control: no-cache\r\n{}Connection: close\r\n\r\n",
                status,
                status_text(status),
                content_type,
                extra
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
            let server = std::thread::spawn(move || serve_on(listener, gw, flag, None));
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

    // ----- 入站体：压缩 / 上限 / 编码（审计 #1 #12） -----

    /// 手搓 zstd 帧（Raw block，单段）：魔数 + 帧头 + 一个最后的 raw 块。
    /// 只够测试用（n < 65792）；不需要引入压缩器。
    fn zstd_raw_frame(data: &[u8]) -> Vec<u8> {
        assert!(data.len() < 65_792 && !data.is_empty());
        let mut out = vec![0x28, 0xb5, 0x2f, 0xfd];
        if data.len() <= 255 {
            out.push(0x20); // Single_Segment，FCS 1 字节
            out.push(data.len() as u8);
        } else {
            out.push(0x60); // Single_Segment，FCS 2 字节（值 = n - 256）
            out.extend_from_slice(&((data.len() - 256) as u16).to_le_bytes());
        }
        let header = ((data.len() as u32) << 3) | 1; // Last_Block，Raw
        out.extend_from_slice(&header.to_le_bytes()[..3]);
        out.extend_from_slice(data);
        out
    }

    fn headers_of(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    fn body_of(headers: &[(&str, &str)], wire: &[u8]) -> Result<Body, BodyErr> {
        let mut r = BufReader::new(Cursor::new(wire.to_vec()));
        read_body(&mut r, &headers_of(headers))
    }

    #[test]
    fn zstd_body_is_decoded_and_flagged() {
        let json = br#"{"model":"deepseek/deepseek-v4","input":"hi"}"#;
        let wire = zstd_raw_frame(json);
        let len = wire.len().to_string();
        let body = body_of(&[("content-encoding", "zstd"), ("content-length", &len)], &wire).unwrap();
        assert_eq!(body.bytes, json.to_vec());
        assert!(body.decoded);
        // 大小写 / 空白不敏感
        let body = body_of(&[("content-encoding", " ZSTD "), ("content-length", &len)], &wire).unwrap();
        assert_eq!(body.bytes, json.to_vec());
    }

    #[test]
    fn zstd_concatenated_frames_are_joined() {
        let mut wire = zstd_raw_frame(b"hello ");
        wire.extend(zstd_raw_frame(b"world"));
        let len = wire.len().to_string();
        let body = body_of(&[("content-encoding", "zstd"), ("content-length", &len)], &wire).unwrap();
        assert_eq!(body.bytes, b"hello world".to_vec());
    }

    #[test]
    fn zstd_larger_than_255_uses_two_byte_fcs() {
        let data = vec![b'x'; 1000];
        let wire = zstd_raw_frame(&data);
        let len = wire.len().to_string();
        let body = body_of(&[("content-encoding", "zstd"), ("content-length", &len)], &wire).unwrap();
        assert_eq!(body.bytes, data);
    }

    #[test]
    fn zstd_decoded_size_cap_is_enforced() {
        let wire = zstd_raw_frame(&vec![b'x'; 1000]);
        assert_eq!(unzstd(&wire, 999), Err(BodyErr::TooLarge));
        assert!(unzstd(&wire, 1000).is_ok());
        // 多帧累计也算
        let mut two = zstd_raw_frame(&vec![b'a'; 600]);
        two.extend(zstd_raw_frame(&vec![b'b'; 600]));
        assert_eq!(unzstd(&two, 1000), Err(BodyErr::TooLarge));
    }

    #[test]
    fn zstd_garbage_is_a_400_not_a_panic() {
        let err = body_of(&[("content-encoding", "zstd"), ("content-length", "5")], b"nope!").unwrap_err();
        assert_eq!(err.status(), 400);
        // 魔数对、内容坏
        let err = unzstd(&[0x28, 0xb5, 0x2f, 0xfd, 0xff, 0xff], 100).unwrap_err();
        assert_eq!(err.status(), 400);
    }

    #[test]
    fn gzip_body_is_decoded_and_cap_enforced() {
        use flate2::write::GzEncoder;
        let json = br#"{"model":"m","messages":[]}"#;
        let mut enc = GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(json).unwrap();
        let wire = enc.finish().unwrap();
        let len = wire.len().to_string();
        for name in ["gzip", "x-gzip"] {
            let body = body_of(&[("content-encoding", name), ("content-length", &len)], &wire).unwrap();
            assert_eq!(body.bytes, json.to_vec());
            assert!(body.decoded);
        }
        // 解压炸弹：1MB 的 0 压成几百字节，cap 比它小就 413
        let mut enc = GzEncoder::new(Vec::new(), flate2::Compression::best());
        enc.write_all(&vec![0u8; 1 << 20]).unwrap();
        let bomb = enc.finish().unwrap();
        assert!(bomb.len() < 4096);
        assert_eq!(ungzip(&bomb, 1 << 19), Err(BodyErr::TooLarge));
        assert_eq!(ungzip(&bomb, 1 << 20).unwrap().len(), 1 << 20);
        // 不是 gzip
        assert_eq!(ungzip(b"plain json", 100).unwrap_err().status(), 400);
    }

    #[test]
    fn unknown_encodings_are_415_before_the_body_is_read() {
        for enc in ["deflate", "br", "gzip, zstd"] {
            // 体根本没给：编码不认在读体前就拒（不会因读不到体变成 400）
            let err = body_of(&[("content-encoding", enc), ("content-length", "10")], b"").unwrap_err();
            assert_eq!(err.status(), 415, "{enc}");
            assert!(err.message().contains("identity, gzip or zstd"));
        }
        // identity = 没编码，不标 decoded
        let body = body_of(&[("content-encoding", "identity"), ("content-length", "2")], b"hi").unwrap();
        assert_eq!(body.bytes, b"hi".to_vec());
        assert!(!body.decoded);
    }

    #[test]
    fn content_length_over_the_cap_is_413_without_reading() {
        let over = (MAX_BODY_BYTES + 1).to_string();
        let err = body_of(&[("content-length", &over)], b"").unwrap_err();
        assert_eq!(err, BodyErr::TooLarge);
        assert_eq!(err.status(), 413);
        assert!(err.message().contains("512 MB"));
        // 声明满额但体没给：读不满 = 400，且不会为声明值预分配 512MB
        let at = MAX_BODY_BYTES.to_string();
        assert_eq!(body_of(&[("content-length", &at)], b"short").unwrap_err(), BodyErr::Malformed);
    }

    #[test]
    fn short_body_is_malformed_not_a_hang_or_overallocation() {
        let err = body_of(&[("content-length", "400000000")], b"abc").unwrap_err();
        assert_eq!(err, BodyErr::Malformed);
    }

    #[test]
    fn chunked_over_cap_is_413_and_does_not_preallocate() {
        // 尺寸行声明 0x7fffffff，没有数据：越线判定先于任何分配
        let mut r = BufReader::new(Cursor::new(b"7fffffff\r\nx".to_vec()));
        assert_eq!(decode_chunked_inner(&mut r, 1024), Err(BodyErr::TooLarge));
        // 上限内声明大块但数据不足 = Malformed
        let mut r = BufReader::new(Cursor::new(b"400\r\nxyz".to_vec()));
        assert_eq!(decode_chunked_inner(&mut r, 1 << 20), Err(BodyErr::Malformed));
    }

    #[test]
    fn guard_errors_speak_the_inbound_dialect() {
        let Reply::Json { status, body } = guard_error("/v1/messages", 413, "too big") else { panic!() };
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(status, 413);
        assert_eq!(v["type"], "error");
        assert_eq!(v["error"]["type"], "request_too_large");
        let Reply::Json { body, .. } = guard_error("/v1/chat/completions?x=1", 403, "no") else { panic!() };
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["type"], "permission_error");
        assert_eq!(v["error"]["message"], "no");
    }

    #[test]
    fn extra_headers_are_sanitised() {
        let lines = extra_header_lines(&[
            ("Retry-After".into(), "7".into()),
            ("X-Bad".into(), "a\r\nSet-Cookie: x=1".into()),
            ("Bad Name:".into(), "v".into()),
        ]);
        assert_eq!(lines, "Retry-After: 7\r\n");
    }

    /// 沙箱网关 + 真 serve_on：在回调里对 addr 发原始请求。
    fn with_live_gateway(f: impl FnOnce(std::net::SocketAddr) + std::panic::UnwindSafe) {
        with_live_gateway_opt(None, |addr, _| f(addr));
    }

    /// 同 `with_live_gateway`，但可挂 Bearer 闸（f 拿到 (addr, token)）。
    fn with_live_gateway_opt(
        token: Option<String>,
        f: impl FnOnce(std::net::SocketAddr, &str) + std::panic::UnwindSafe,
    ) {
        let _lock = crate::engine_gateway::TEST_SANDBOX_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "marionette_test_httpd_live_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let prior_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let token_for_cb = token.clone().unwrap_or_default();
        let result = std::panic::catch_unwind(move || {
            let gateway = std::sync::Arc::new(Gateway::open().expect("gateway"));
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
            let addr = listener.local_addr().unwrap();
            let shutdown = Arc::new(AtomicBool::new(false));
            let flag = shutdown.clone();
            let server = std::thread::spawn(move || serve_on(listener, gateway, flag, token));
            f(addr, &token_for_cb);
            shutdown.store(true, Ordering::Release);
            let _ = server.join();
        });
        match prior_xdg {
            Some(p) => std::env::set_var("XDG_CONFIG_HOME", p),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let _ = std::fs::remove_dir_all(&dir);
        result.unwrap();
    }

    fn raw_exchange(addr: std::net::SocketAddr, request: &[u8]) -> String {
        let mut c = TcpStream::connect(addr).unwrap();
        c.write_all(request).unwrap();
        let mut out = Vec::new();
        let _ = c.read_to_end(&mut out);
        String::from_utf8_lossy(&out).into_owned()
    }

    #[test]
    fn live_foreign_origin_is_403_on_every_route_and_local_calls_pass() {
        with_live_gateway(|addr| {
            for (method, path) in [
                ("GET", "/v1/models"),
                ("GET", "/v1/magpie/quotas"),
                ("POST", "/v1/chat/completions"),
                ("POST", "/v1/responses"),
                ("POST", "/v1/messages"),
            ] {
                let req = format!(
                    "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: https://evil.example\r\n\
                     Content-Type: text/plain\r\nContent-Length: 2\r\n\r\n{{}}",
                    addr.port()
                );
                let reply = raw_exchange(addr, req.as_bytes());
                assert!(reply.starts_with("HTTP/1.1 403"), "{method} {path}: {reply}");
                assert!(reply.contains("permission_error"), "{reply}");
            }
            // 同一路由：没有 Origin 照常（models 是 200）
            let reply = raw_exchange(
                addr,
                b"GET /v1/models HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            );
            assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
            // 本机网页（dev server）放行
            let reply = raw_exchange(
                addr,
                b"GET /v1/models HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: http://localhost:5173\r\n\r\n",
            );
            assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
            // Origin: null 拒
            let reply = raw_exchange(
                addr,
                b"GET /v1/models HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: null\r\n\r\n",
            );
            assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        });
    }

    #[test]
    fn live_preflight_for_obsidian_is_204_and_foreign_is_403() {
        with_live_gateway(|addr| {
            let reply = raw_exchange(
                addr,
                b"OPTIONS /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1\r\n\
                  Origin: app://obsidian.md\r\nAccess-Control-Request-Method: POST\r\n\
                  Access-Control-Request-Headers: content-type\r\n\r\n",
            );
            assert!(reply.starts_with("HTTP/1.1 204"), "{reply}");
            assert!(reply.contains("Access-Control-Allow-Origin: app://obsidian.md"), "{reply}");
            assert!(reply.contains("Access-Control-Allow-Headers: content-type"), "{reply}");
            let reply = raw_exchange(
                addr,
                b"OPTIONS /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1\r\n\
                  Origin: https://evil.example\r\nAccess-Control-Request-Method: POST\r\n\r\n",
            );
            assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        });
    }

    /// 2026-11-22 安全批：Bearer 闸——无令牌/错令牌一律 401（先于读体），
    /// 对令牌照常服务；preflight 仍然免令牌（CORS 规范不带凭证）；
    /// CORS 与令牌各管各的（外站 origin 拿着对令牌也 403）。
    #[test]
    fn live_bearer_gate_blocks_tokenless_and_serves_holders() {
        with_live_gateway_opt(Some("test-gateway-token-48hex".to_string()), |addr, token| {
            for (method, path) in [
                ("GET", "/v1/models"),
                ("GET", "/v1/magpie/quotas"),
                ("POST", "/v1/chat/completions"),
                ("POST", "/v1/responses"),
                ("POST", "/v1/messages"),
            ] {
                // 缺 Authorization 头
                let req = format!(
                    "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\n{{}}"
                );
                let reply = raw_exchange(addr, req.as_bytes());
                assert!(reply.starts_with("HTTP/1.1 401"), "{method} {path}: {reply}");
                assert!(reply.contains("authentication_error"), "{reply}");
                // 错令牌同罪
                let req = format!(
                    "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer magpie\r\nContent-Type: text/plain\r\nContent-Length: 2\r\n\r\n{{}}"
                );
                let reply = raw_exchange(addr, req.as_bytes());
                assert!(reply.starts_with("HTTP/1.1 401"), "{method} {path}: {reply}");
            }
            // 对令牌：照常 200。
            let req = format!(
                "GET /v1/models HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer {token}\r\nConnection: close\r\n\r\n"
            );
            let reply = raw_exchange(addr, req.as_bytes());
            assert!(reply.starts_with("HTTP/1.1 200"), "{reply}");
            // preflight 免令牌（CORS 规范）。
            let reply = raw_exchange(
                addr,
                b"OPTIONS /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1\r\n\
                  Origin: app://obsidian.md\r\nAccess-Control-Request-Method: POST\r\n\r\n",
            );
            assert!(reply.starts_with("HTTP/1.1 204"), "{reply}");
            // 外站 origin 即使带着对令牌也 403：CORS 判定在前。
            let req = format!(
                "GET /v1/models HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: https://evil.example\r\nAuthorization: Bearer {token}\r\n\r\n"
            );
            let reply = raw_exchange(addr, req.as_bytes());
            assert!(reply.starts_with("HTTP/1.1 403"), "{reply}");
        });
    }

    #[test]
    fn live_zstd_and_gzip_bodies_reach_the_router_decoded() {
        with_live_gateway(|addr| {
            // 没有该来源 -> 404「no provider for model: <解码后的 model>」——
            // 看得到模型名 = 体已解码（压缩字节解析不出 model，旧行为是空名）。
            let json = br#"{"model":"zz-nonexistent/model-x","messages":[]}"#;
            let wire = zstd_raw_frame(json);
            let mut req = format!(
                "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Encoding: zstd\r\n\
                 Content-Length: {}\r\n\r\n",
                wire.len()
            )
            .into_bytes();
            req.extend_from_slice(&wire);
            let reply = raw_exchange(addr, &req);
            assert!(reply.starts_with("HTTP/1.1 404"), "{reply}");
            assert!(reply.contains("zz-nonexistent/model-x"), "{reply}");

            use flate2::write::GzEncoder;
            let mut enc = GzEncoder::new(Vec::new(), flate2::Compression::fast());
            enc.write_all(json).unwrap();
            let wire = enc.finish().unwrap();
            let mut req = format!(
                "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Encoding: gzip\r\n\
                 Content-Length: {}\r\n\r\n",
                wire.len()
            )
            .into_bytes();
            req.extend_from_slice(&wire);
            let reply = raw_exchange(addr, &req);
            assert!(reply.contains("zz-nonexistent/model-x"), "{reply}");

            // 不认的编码：415，体不读
            let reply = raw_exchange(
                addr,
                b"POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Encoding: br\r\nContent-Length: 3\r\n\r\nabc",
            );
            assert!(reply.starts_with("HTTP/1.1 415"), "{reply}");
            assert!(reply.contains("\"type\":\"error\""), "anthropic shape: {reply}");
        });
    }

    #[test]
    fn live_oversize_content_length_is_413() {
        with_live_gateway(|addr| {
            let req = format!(
                "POST /v1/responses HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n",
                MAX_BODY_BYTES + 1
            );
            let reply = raw_exchange(addr, req.as_bytes());
            assert!(reply.starts_with("HTTP/1.1 413"), "{reply}");
            assert!(reply.contains("request_too_large"), "{reply}");
        });
    }

    // ----- bodyprep 接线（routes.rs 对话臂；Go base64_input.go + body_text.go）-----

    /// GBK 字节混进 JSON 串：没有 bodyprep 时 messages::handle 的 serde_json
    /// 解析直接失败（400 invalid request body）；修过之后体能解析，走到组
    /// 路由的 404——模型名透出来 = 体确实在进任何处理器**之前**被修好了。
    #[test]
    fn live_bodyprep_mends_bad_bytes_before_routing() {
        with_live_gateway(|addr| {
            let mut body = br#"{"model":"zz-none/gbk-x","messages":[{"role":"user","content":""#.to_vec();
            body.extend_from_slice(&[0xc4, 0xe3, 0xba, 0xc3]); // GBK「你好」
            body.extend_from_slice(b"\"}]}");
            let mut req = format!(
                "POST /v1/messages HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes();
            req.extend_from_slice(&body);
            let reply = raw_exchange(addr, &req);
            assert!(reply.starts_with("HTTP/1.1 404"), "{reply}");
            assert!(reply.contains("zz-none/gbk-x"), "{reply}");
        });
    }

    /// 孤代理转义（`\ud800`，ASCII 合法 UTF-8 但 serde_json 拒绝）：chat 臂
    /// 解析失败时模型名为空（404 里没有名字）；修成 `\ufffd` 后 404 带模型
    /// 名——同一接线对另一类缺陷、另一个端点也成立。
    #[test]
    fn live_bodyprep_mends_lone_surrogate_escape() {
        with_live_gateway(|addr| {
            let body = br#"{"model":"zz-none/surro-x","messages":[{"role":"user","content":"before \ud800 after"}]}"#;
            let mut req = format!(
                "POST /v1/chat/completions HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                body.len()
            )
            .into_bytes();
            req.extend_from_slice(body);
            let reply = raw_exchange(addr, &req);
            assert!(reply.starts_with("HTTP/1.1 404"), "{reply}");
            assert!(reply.contains("zz-none/surro-x"), "{reply}");
        });
    }
}
