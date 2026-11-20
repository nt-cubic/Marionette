//! 出站 WebSocket 最小客户端（RFC 6455 客户端侧，无第三方依赖）。
//!
//! 服务的是 ChatGPT 后端（Codex 订阅）的 **出站** WebSocket：HTTP 流式回合在
//! ~120s 被中间层掐断、同一条回合走 WebSocket 能跑完（Go 源
//! `third_party/magpie/internal/gateway/codex_socket.go` 头注释，Discord
//! brickhao 实测 120.7s/121.5s 都被掐、124.5s/131.4s 完整跑完）。协议形状
//! 照抄 Codex 自己的客户端（codex-rs `ResponseCreateWsRequest` /
//! `build_websocket_headers`，Go 注释 :36）：
//!
//! - 握手带回合的鉴权头，`OpenAI-Beta: responses_websockets=2026-02-06`
//!   （Go codex_socket.go :61 `codexSocketBeta` / :334 `codexSocketHeaders`）；
//! - 请求 = HTTP 体前面接 `{"type":"response.create",`，**一帧 text 发完**
//!   （Go :265 `codexCreate`）；
//! - 应答 = **每个事件一帧** text（Go :392 `turn` / :653 `codexSocketBody`），
//!   由 codex_account.rs 还原成 SSE 行流。
//!
//! 依赖红线：**不加 crate**。SHA-1 手写（sha1 不在依赖、只用于握手校验非
//! 安全用途）；TLS 复用 ureq 的 native-tls 转出口（`ureq::native_tls`，
//! mod.rs `outbound_agent` 2026-11-18 真机实锤的同款连接器）；随机数用
//! `RandomState` 哈希熵（握手 Key / 掩码只需要不重复，不需要密码学强度）。
//!
//! ## 与 Go（coder/websocket + codex_socket_keep.go）的差距，如实列出
//!
//! - **keep 连接复用 / shun 名单 / 1009 尺寸记忆：已移植**（2026-11-18，
//!   状态机与池在 codex_account.rs，对照 codex_socket_keep.go 整文件 +
//!   codex_socket.go :63 `codexSocketShun` / :83 `refusedTooBig`）：会话的
//!   socket 回合结束不关、留在池里，后续回合以 `previous_response_id` 只发
//!   增量输入（消息保持小、躲后端 1009 message too big，Lamrfu 实测）；握手
//!   被拒的主机 10 分钟 shun、期内直接 HTTP；1009 拒收按主机记最小尺寸，
//!   期内同等大小直接 HTTP。本模块只负责把「读到底但不断连」的连接形态
//!   做出来（`read_message` 不关流、close 帧/对端断开/读错才由持有方回收），
//!   池/名单/增量的语义见 codex_account.rs。
//! - **keep 池的 idle 收割没有定时器线程**：Go 用 `time.AfterFunc` 在
//!   `codexKeepIdle`（10 分钟）时主动关；我们惰性判定（取用/归还时发现超时
//!   才弃用，语义等价——过期连接绝不会被复用——只是空连接在池里多躺到
//!   下一次取用/put 清扫）。std-only 无后台线程，如实不搬定时器。
//! - **permessage-deflate 不协商**：Go Dial 带
//!   `CompressionMode: CompressionNoContextTakeover`（:366）；我们不发
//!   `Sec-WebSocket-Extensions`，服务端按 RFC 6455 §5.1 不得启用未协商的
//!   扩展——帧恒明文，只是上行体积不压缩（Go 的出站压缩是 zstd over HTTP，
//!   WS 路径同样发明文，差距只在 deflate 这一层）。
//! - **分片延续帧（FIN=0 / continuation）不支持**：收到即报错（ChatGPT 的
//!   事件帧都是单帧 text；coder/websocket 支持，但我们只做最小集）。
//! - **代理**：与 `gateway::outbound_agent` 同源（mod.rs `outbound_proxy`），
//!   只做 HTTP CONNECT 隧道（含 Basic 认证）；SOCKS / https 代理不支持
//!   （HTTP 路径 ureq 同样没开 socks feature，两路一致）。
//! - **断线重连退避 / 并发连接数上限**：Go 也没有，如实不移植。
//! - **读超时**：握手 30s（Go :68 `codexSocketDial` 同值）；握手完成后不设
//!   限——长回合帧间隔可以分钟级，HTTP 路径（ureq 无读超时）同款暴露。
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use base64::Engine as _;

/// RFC 6455 §1.3：Sec-WebSocket-Accept = base64(SHA1(Key + GUID))。
pub(crate) const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

const OP_CONTINUATION: u8 = 0x0;
pub(crate) const OP_TEXT: u8 = 0x1;
pub(crate) const OP_BINARY: u8 = 0x2;
pub(crate) const OP_CLOSE: u8 = 0x8;
const OP_PING: u8 = 0x9;
const OP_PONG: u8 = 0xA;

/// 单帧载荷上限（防御性；Go `SetReadLimit(-1)` 不设限，256MiB 覆盖任何回合）。
const MAX_FRAME: u64 = 256 << 20;
/// 握手应答头（状态行 + 头字段）的读取上限。
const MAX_HEAD: usize = 64 * 1024;

// =====================================================================
// SHA-1（手写，FIPS 180-1；只用于握手校验，非安全用途）
// =====================================================================

/// SHA-1 摘要。测试向量：FIPS/RFC 3174 的 "" / "abc" / fox 句子 + RFC 6455
/// 的 GUID 拼接（见 tests）。
pub(crate) fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];
    // 填充：0x80 + 0…0 + 8 字节大端位长，到 64 字节对齐的 56 偏移。
    let bit_len = (data.len() as u64) * 8;
    let mut msg = data.to_vec();
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in msg.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (i, word) in chunk.chunks_exact(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, &wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5A82_7999_u32),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let tmp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// RFC 6455 §4.2.2：base64(SHA1(key + GUID))——握手应答的期望值。
pub(crate) fn accept_for(key: &str) -> String {
    let mut data = key.as_bytes().to_vec();
    data.extend_from_slice(WS_GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(sha1(&data))
}

// =====================================================================
// 随机源（std-only：RandomState 的每次实例种子不同；用途只要不重复）
// =====================================================================

fn entropy(n: usize) -> Vec<u8> {
    use std::hash::{BuildHasher, Hasher};
    let mut out = Vec::with_capacity(n + 8);
    let mut counter: u64 = 0;
    let started = std::time::Instant::now();
    while out.len() < n {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u64(counter);
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        h.write_u64(started.elapsed().as_nanos() as u64);
        h.write_usize(&counter as *const u64 as usize);
        counter = counter.wrapping_add(1);
        out.extend_from_slice(&h.finish().to_le_bytes());
    }
    out.truncate(n);
    out
}

// =====================================================================
// 帧编解码（纯函数，字节级断言用 RFC 6455 §5.7 向量）
// =====================================================================

/// 客户端出站帧：恒 FIN=1、恒掩码（RFC 6455 §5.3 客户端→服务端必须掩码）。
/// `mask` 由调用方给（发送路径用随机源、测试用固定值对表）。
pub(crate) fn encode_client_frame(opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(0x80 | opcode);
    let len = payload.len();
    if len < 126 {
        out.push(0x80 | len as u8);
    } else if len <= u16::MAX as usize {
        out.push(0x80 | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0x80 | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(&mask);
    for (i, b) in payload.iter().enumerate() {
        out.push(b ^ mask[i & 3]);
    }
    out
}

/// 服务端入站帧（无掩码；测试与假服务端构造用——服务端→客户端禁掩码）。
pub(crate) fn encode_server_frame(opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 10);
    out.push(0x80 | opcode);
    let len = payload.len();
    if len < 126 {
        out.push(len as u8);
    } else if len <= u16::MAX as usize {
        out.push(126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    out.extend_from_slice(payload);
    out
}

/// 收到的一条完整消息。pong 在 [`read_message`] 内部就地吞掉。
#[derive(Debug)]
pub(crate) enum Incoming {
    Text(Vec<u8>),
    Binary(Vec<u8>),
    /// 服务端 close 帧（含关闭码；None = 无载荷）。
    Close(Option<u16>),
}

/// 从流里读下一条消息：ping 自动回 pong（同载荷）、pong 忽略；close /
/// text / binary 返回。分片延续帧（FIN=0）、扩展协商位、服务端掩码都是
/// 协议错误（我们从不协商扩展、ChatGPT 不分片）。
pub(crate) fn read_message<S: Read + Write>(stream: &mut S) -> Result<Incoming, String> {
    loop {
        let header = read_exact_vec(stream, 2).map_err(|e| format!("ws read: {e}"))?;
        let fin = header[0] & 0x80 != 0;
        let rsv = header[0] & 0x70;
        let opcode = header[0] & 0x0F;
        let masked = header[1] & 0x80 != 0;
        let len7 = header[1] & 0x7F;
        if rsv != 0 {
            return Err("ws protocol: RSV bits set (extension not negotiated)".into());
        }
        let len: u64 = match len7 {
            126 => {
                let b = read_exact_vec(stream, 2).map_err(|e| format!("ws read: {e}"))?;
                u16::from_be_bytes([b[0], b[1]]) as u64
            }
            127 => {
                let b = read_exact_vec(stream, 8).map_err(|e| format!("ws read: {e}"))?;
                u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
            }
            n => n as u64,
        };
        if len > MAX_FRAME {
            return Err(format!("ws protocol: frame of {len} bytes over limit"));
        }
        // RFC 6455 §5.1：服务端→客户端的帧不得掩码。
        if masked {
            return Err("ws protocol: server frame must not be masked".into());
        }
        let mut payload = vec![0u8; len as usize];
        stream.read_exact(&mut payload).map_err(|e| format!("ws read: {e}"))?;
        match opcode {
            OP_PING => {
                // §5.5.2：控制帧 FIN=1 且 ≤125 字节；回 pong 带同载荷。
                if !fin || len > 125 {
                    return Err("ws protocol: malformed ping".into());
                }
                let mask: [u8; 4] = entropy(4).try_into().unwrap();
                let pong = encode_client_frame(OP_PONG, &payload, mask);
                stream.write_all(&pong).map_err(|e| format!("ws pong: {e}"))?;
                continue;
            }
            OP_PONG => continue,
            OP_CLOSE => {
                if !fin || len > 125 {
                    return Err("ws protocol: malformed close".into());
                }
                let code = if payload.len() >= 2 {
                    Some(u16::from_be_bytes([payload[0], payload[1]]))
                } else {
                    None
                };
                return Ok(Incoming::Close(code));
            }
            OP_TEXT | OP_BINARY => {
                if !fin {
                    return Err("ws protocol: fragmented frame not supported".into());
                }
                return Ok(if opcode == OP_TEXT { Incoming::Text(payload) } else { Incoming::Binary(payload) });
            }
            OP_CONTINUATION => {
                return Err("ws protocol: continuation frame without a start".into());
            }
            other => return Err(format!("ws protocol: unknown opcode {other:#x}")),
        }
    }
}

fn read_exact_vec<S: Read>(stream: &mut S, n: usize) -> io::Result<Vec<u8>> {
    let mut buf = vec![0u8; n];
    stream.read_exact(&mut buf)?;
    Ok(buf)
}

// =====================================================================
// HTTP Upgrade 握手（在任意双工流上，可测）
// =====================================================================

/// 在已连接的流上完成 WebSocket 升级握手：写 GET Upgrade 请求（带 `headers`
/// 的鉴权/_originator_ 头），读 101 应答并校验 `Sec-WebSocket-Accept`。
/// 逐字节读应答头（头很小，不值得缓冲层）。
pub(crate) fn handshake_over<S: Read + Write>(
    stream: &mut S,
    host: &str,
    target: &str,
    headers: &[(String, String)],
) -> Result<(), String> {
    let key = base64::engine::general_purpose::STANDARD.encode(entropy(16));
    let mut req = format!(
        "GET {target} HTTP/1.1\r\n\
         Host: {host}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {key}\r\n\
         Sec-WebSocket-Version: 13\r\n"
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).map_err(|e| format!("ws handshake write: {e}"))?;
    stream.flush().map_err(|e| format!("ws handshake write: {e}"))?;

    // 头读到 \r\n\r\n 为止（上限 MAX_HEAD）。
    let mut head: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).map_err(|e| format!("ws handshake read: {e}"))?;
        if n == 0 {
            return Err("ws handshake: connection closed before 101".into());
        }
        head.push(byte[0]);
        if head.len() > MAX_HEAD {
            return Err("ws handshake: response head too large".into());
        }
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&head[..head.len() - 4]);
    let mut lines = text.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let mut parts = status_line.splitn(3, ' ');
    let _version = parts.next().unwrap_or("");
    let code = parts.next().unwrap_or("");
    if code != "101" {
        let reason = parts.next().unwrap_or("").trim();
        return Err(format!("ws handshake refused: HTTP {code} {reason}"));
    }
    let mut accept: Option<String> = None;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            if k.trim().eq_ignore_ascii_case("sec-websocket-accept") {
                accept = Some(v.trim().to_string());
            }
        }
    }
    if accept.as_deref() != Some(accept_for(&key).as_str()) {
        return Err("ws handshake: Sec-WebSocket-Accept mismatch".into());
    }
    Ok(())
}

// =====================================================================
// 连接（dial）：直连或 HTTP CONNECT 代理隧道 + 可选 TLS + 握手
// =====================================================================

pub(crate) trait ReadWrite: Read + Write {}
impl<T: Read + Write> ReadWrite for T {}

/// 已完成握手的 WebSocket 连接。`Send`：TcpStream/TlsStream 均满足。
pub(crate) struct WsConn {
    stream: Box<dyn ReadWrite + Send>,
}

impl WsConn {
    /// 包一个现成的流（dial 用；测试用内存双工构造假对端）。**不**做握手。
    pub(crate) fn over(stream: Box<dyn ReadWrite + Send>) -> WsConn {
        WsConn { stream }
    }

    /// 一帧 text 发出（回合请求体；Go codex_socket.go :427
    /// `k.c.Write(req.Context(), websocket.MessageText, msg)`）。UTF-8 由
    /// 调用方保证（我们的载荷是 serde JSON）。
    pub(crate) fn send_text(&mut self, payload: &[u8]) -> Result<(), String> {
        let mask: [u8; 4] = entropy(4).try_into().unwrap();
        let frame = encode_client_frame(OP_TEXT, payload, mask);
        self.stream.write_all(&frame).map_err(|e| format!("ws write: {e}"))?;
        self.stream.flush().map_err(|e| format!("ws write: {e}"))
    }

    /// 正常收尾（1000；Go codexKept.closeNormally :91）。尽力而为。
    pub(crate) fn close_normally(&mut self) {
        let mask: [u8; 4] = entropy(4).try_into().unwrap();
        let frame = encode_client_frame(OP_CLOSE, &1000u16.to_be_bytes(), mask);
        let _ = self.stream.write_all(&frame);
        let _ = self.stream.flush();
    }

    pub(crate) fn read_message(&mut self) -> Result<Incoming, String> {
        read_message(&mut self.stream)
    }
}

struct Target {
    secure: bool,
    host: String,
    port: u16,
    /// path?query（URL 里 / 之后的一切；缺省 /）。
    path: String,
}

fn parse_ws_url(url: &str) -> Result<Target, String> {
    let (scheme, rest) = url.split_once("://").ok_or_else(|| format!("ws url: no scheme in {url}"))?;
    let secure = match scheme.to_ascii_lowercase().as_str() {
        "wss" => true,
        "ws" => false,
        other => return Err(format!("ws url: scheme {other} is not ws/wss")),
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, "/".to_string()),
    };
    if authority.is_empty() {
        return Err(format!("ws url: no host in {url}"));
    }
    // IPv6 字面量 [::1]:port；我们的 URL 不带 userinfo。
    let (host, port) = if let Some(authority) = authority.strip_prefix('[') {
        let (h, r) = authority.split_once(']').ok_or_else(|| format!("ws url: unterminated IPv6 in {url}"))?;
        let port = r.strip_prefix(':').map(|p| p.parse::<u16>()).transpose().map_err(|_| format!("ws url: bad port in {url}"))?;
        (h.to_string(), port)
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => {
                let port = p.parse::<u16>().map_err(|_| format!("ws url: bad port in {url}"))?;
                (h.to_string(), Some(port))
            }
            // 带了冒号却不是数字端口 = 坏 URL（不静默当 host 名）。
            Some(_) => return Err(format!("ws url: bad port in {url}")),
            None => (authority.to_string(), None),
        }
    };
    let port = port.unwrap_or(if secure { 443 } else { 80 });
    Ok(Target { secure, host, port, path })
}

fn tcp_connect(host: &str, port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let addrs: Vec<SocketAddr> = (host, port).to_socket_addrs()?.collect();
    let mut last = io::Error::new(io::ErrorKind::AddrNotAvailable, format!("{host}:{port}: no address resolved"));
    for addr in addrs {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(s) => {
                // 保留 connect 超时作写超时；读超时由调用阶段决定。
                let _ = s.set_write_timeout(Some(timeout));
                return Ok(s);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// 走 HTTP 代理的 CONNECT 隧道（与 mod.rs `outbound_proxy` 同源配置——
/// ureq 的代理同配置只服务 HTTP 路径，WS 的裸 TCP 得自己打隧道）。
/// 只支持 http:// 代理（含 userinfo → Proxy-Authorization: Basic）；
/// SOCKS 不支持（见模块头差距清单）。
fn connect_via_proxy(proxy_url: &str, host: &str, port: u16, timeout: Duration) -> Result<TcpStream, String> {
    let (scheme, rest) = proxy_url.split_once("://").ok_or_else(|| format!("proxy url: no scheme in {proxy_url}"))?;
    if !scheme.eq_ignore_ascii_case("http") {
        return Err(format!("ws proxy: {scheme} proxy is not supported (http only)"));
    }
    let (authority, _) = match rest.find('/') {
        Some(i) => (&rest[..i], ""),
        None => (rest, ""),
    };
    // userinfo@host:port
    let (userinfo, hostport) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    };
    let (phost, pport) = match hostport.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) => {
            let port = p.parse::<u16>().map_err(|_| format!("proxy url: bad port in {proxy_url}"))?;
            (h.to_string(), port)
        }
        _ => (hostport.to_string(), 80),
    };
    if phost.is_empty() {
        return Err(format!("proxy url: no host in {proxy_url}"));
    }
    let mut stream = tcp_connect(&phost, pport, timeout).map_err(|e| format!("proxy connect: {e}"))?;
    stream.set_read_timeout(Some(timeout)).map_err(|e| format!("proxy: {e}"))?;
    let target = format!("{host}:{port}");
    let mut req = format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n");
    if let Some(user) = userinfo {
        let cred = base64::engine::general_purpose::STANDARD.encode(user);
        req.push_str(&format!("Proxy-Authorization: Basic {cred}\r\n"));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes()).map_err(|e| format!("proxy write: {e}"))?;
    let mut head: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = stream.read(&mut byte).map_err(|e| format!("proxy read: {e}"))?;
        if n == 0 {
            return Err("proxy: connection closed before CONNECT answer".into());
        }
        head.push(byte[0]);
        if head.len() > MAX_HEAD {
            return Err("proxy: CONNECT answer head too large".into());
        }
        if head.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&head);
    let status = text.split("\r\n").next().unwrap_or("");
    // 任何 2xx 都算隧道建立（RFC 7231 CONNECT 的成功类）。
    if !status.contains(" 2") {
        return Err(format!("proxy: CONNECT refused: {}", status.trim()));
    }
    stream.set_read_timeout(None).map_err(|e| format!("proxy: {e}"))?;
    Ok(stream)
}

enum Wire {
    Tls(ureq::native_tls::TlsStream<TcpStream>),
    Plain(TcpStream),
}

impl Read for Wire {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Wire::Tls(s) => s.read(buf),
            Wire::Plain(s) => s.read(buf),
        }
    }
}

impl Write for Wire {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Wire::Tls(s) => s.write(buf),
            Wire::Plain(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        match self {
            Wire::Tls(s) => s.flush(),
            Wire::Plain(s) => s.flush(),
        }
    }
}

impl Wire {
    /// 底层 TCP 的另一个句柄：握手完成后撤读超时用（套接字选项跨句柄共享）。
    fn tcp(&self) -> Option<&TcpStream> {
        match self {
            Wire::Tls(s) => Some(s.get_ref()),
            Wire::Plain(s) => Some(s),
        }
    }
}

/// 连到 `url`（ws:// 或 wss://），带 `headers` 完成 Upgrade 握手。
/// 失败一律返回可读错误字符串——调用方（codex_account）按「应答未交付」
/// 回退 HTTP（Go errCodexSocketFallBack :213 同款）。
pub(crate) fn dial(
    url: &str,
    headers: &[(String, String)],
    connect_timeout: Duration,
    handshake_timeout: Duration,
) -> Result<WsConn, String> {
    let t = parse_ws_url(url)?;
    let host_port = format!("{}:{}", t.host, t.port);
    let stream = match super::outbound_proxy() {
        Some(p) if !p.trim().is_empty() => connect_via_proxy(&p, &t.host, t.port, connect_timeout)?,
        _ => tcp_connect(&t.host, t.port, connect_timeout).map_err(|e| format!("ws connect {host_port}: {e}"))?,
    };
    // 握手阶段限时（Go codexSocketDial :69 的 30s 同义）；完成后撤读超时
    // ——长回合帧间隔可以分钟级（HTTP 路径同款不设限）。
    stream.set_read_timeout(Some(handshake_timeout)).map_err(|e| format!("ws: {e}"))?;
    let wire = if t.secure {
        // ureq 的 native-tls 转出口（mod.rs outbound_agent 2026-11-18 真机
        // 实锤的同款连接器；schannel 走系统根证书）。
        let connector = ureq::native_tls::TlsConnector::new().map_err(|e| format!("ws tls: {e}"))?;
        let tls = connector.connect(&t.host, stream).map_err(|e| format!("ws tls {}: {e}", t.host))?;
        Wire::Tls(tls)
    } else {
        Wire::Plain(stream)
    };
    let mut wire = wire;
    handshake_over(&mut wire, &host_port, &t.path, headers)?;
    if let Some(tcp) = wire.tcp() {
        let _ = tcp.set_read_timeout(None);
        let _ = tcp.set_write_timeout(None);
    }
    Ok(WsConn::over(Box::new(wire)))
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn sha1_public_vectors() {
        // FIPS 180-1 / RFC 3174 附录的三个经典向量。
        assert_eq!(hex(&sha1(b"")), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(hex(&sha1(b"abc")), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            hex(&sha1(b"The quick brown fox jumps over the lazy dog")),
            "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12"
        );
        // >64 字节（两个压缩块）与恰好 55/56/64 字节边界的填充路径。
        assert_eq!(
            hex(&sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        let fifty_five = vec![b'a'; 55];
        let fifty_six = vec![b'a'; 56];
        let sixty_four = vec![b'a'; 64];
        let _ = (sha1(&fifty_five), sha1(&fifty_six), sha1(&sixty_four));
    }

    #[test]
    fn accept_for_rfc6455_sample() {
        // RFC 6455 §1.3 的握手样例：这个 Key 应得这个 Accept。
        assert_eq!(accept_for("dGhlIHNhbXBsZSBub25jZQ=="), "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=");
    }

    /// RFC 6455 §5.7「a single-frame masked text message」向量。
    #[test]
    fn client_text_frame_matches_rfc6455_vector() {
        let frame = encode_client_frame(OP_TEXT, b"Hello", [0x37, 0xfa, 0x21, 0x3d]);
        assert_eq!(
            frame,
            vec![0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58]
        );
    }

    #[test]
    fn frame_length_forms() {
        // 126 两字节扩展长度。
        let payload = vec![0u8; 200];
        let frame = encode_client_frame(OP_TEXT, &payload, [1, 2, 3, 4]);
        assert_eq!(&frame[..4], &[0x81, 0x80 | 126, 0x00, 0xc8][..]);
        assert_eq!(frame.len(), 4 + 4 + 200);
        // 127 八字节扩展长度。
        let payload = vec![0u8; 70_000];
        let frame = encode_client_frame(OP_BINARY, &payload, [9, 9, 9, 9]);
        assert_eq!(frame[0], 0x82);
        assert_eq!(frame[1], 0x80 | 127);
        assert_eq!(&frame[2..10], &70_000u64.to_be_bytes()[..]);
    }

    /// 内存双工：写入累积进共享的 `sent`（测试侧保留句柄检查出站字节）；
    /// 读取按序供给预置/现算的应答。
    struct Duplex {
        sent: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
        served: Vec<u8>,
        cursor: usize,
        /// respond 拿到目前收到的请求后构造应答（握手测试里按 Key 现算
        /// Accept）。+ Send：WsConn 的流框要求 Send。
        respond: Option<Box<dyn Fn(&[u8]) -> Vec<u8> + Send>>,
        response: Option<Vec<u8>>,
    }

    impl Duplex {
        fn with(response: Vec<u8>) -> Duplex {
            Duplex {
                sent: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
                served: response,
                cursor: 0,
                respond: None,
                response: None,
            }
        }

        /// 保留写侧句柄（send_text 的出站字节断言用）。
        fn with_sink(sent: std::sync::Arc<std::sync::Mutex<Vec<u8>>>, response: Vec<u8>) -> Duplex {
            Duplex { sent, served: response, cursor: 0, respond: None, response: None }
        }

        fn sent_string(&self) -> String {
            String::from_utf8_lossy(&self.sent.lock().unwrap()).into_owned()
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.sent.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Read for Duplex {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            if self.response.is_none() {
                if let Some(make) = &self.respond {
                    let req = self.sent.lock().unwrap().clone();
                    self.response = Some(make(&req));
                }
            }
            if let Some(extra) = self.response.take() {
                // 应答是「按已收请求现算」的一次性注入。
                self.served.extend_from_slice(&extra);
            }
            if self.cursor >= self.served.len() {
                // 没有更多预置数据：EOF（调用方各自把 EOF 当断线/应答结束）。
                return Ok(0);
            }
            let n = (self.served.len() - self.cursor).min(buf.len());
            buf[..n].copy_from_slice(&self.served[self.cursor..self.cursor + n]);
            self.cursor += n;
            Ok(n)
        }
    }

    #[test]
    fn handshake_round_trip_and_request_shape() {
        let mut dup = Duplex::with(Vec::new());
        dup.respond = Some(Box::new(|req: &[u8]| {
            // 假服务端：从收到的请求里抠 Key，按 RFC 算 Accept 回 101。
            let text = String::from_utf8_lossy(req);
            let key = text
                .lines()
                .find(|l| l.to_ascii_lowercase().starts_with("sec-websocket-key:"))
                .and_then(|l| l.split_once(':'))
                .map(|(_, v)| v.trim().to_string())
                .unwrap_or_default();
            format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
                accept_for(&key)
            )
            .into_bytes()
        }));
        let headers = vec![
            ("Authorization".to_string(), "Bearer tok".to_string()),
            ("originator".to_string(), "codex_cli_rs".to_string()),
            ("OpenAI-Beta".to_string(), "responses_websockets=2026-02-06".to_string()),
        ];
        handshake_over(&mut dup, "chatgpt.com", "/backend-api/codex/responses", &headers)
            .expect("handshake ok");
        let sent = dup.sent_string();
        // 请求行的形状。
        assert!(sent.starts_with("GET /backend-api/codex/responses HTTP/1.1\r\n"));
        assert!(sent.contains("Host: chatgpt.com\r\n"));
        assert!(sent.contains("Upgrade: websocket\r\n"));
        assert!(sent.contains("Connection: Upgrade\r\n"));
        assert!(sent.contains("Sec-WebSocket-Version: 13\r\n"));
        // Key 是 16 字节 base64（22 字符 + padding）。
        let key = sent
            .lines()
            .find(|l| l.starts_with("Sec-WebSocket-Key:"))
            .unwrap()
            .split_once(':')
            .unwrap()
            .1
            .trim();
        assert_eq!(key.len(), 24);
        assert!(key.ends_with("=="));
        // 鉴权头照抄。
        assert!(sent.contains("Authorization: Bearer tok\r\n"));
        assert!(sent.contains("OpenAI-Beta: responses_websockets=2026-02-06\r\n"));
        assert!(sent.ends_with("\r\n\r\n"));
    }

    #[test]
    fn handshake_rejects_non_101_and_bad_accept() {
        // 非 101（比如 200 的普通 HTTP 应答——现有 Mock 的行为）。
        let mut dup = Duplex::with(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n".to_vec());
        let err = handshake_over(&mut dup, "h", "/x", &[]).unwrap_err();
        assert!(err.contains("200"), "{err}");

        // Accept 不对（不是这个 Key 的 SHA1）。
        let mut dup = Duplex::with(
            b"HTTP/1.1 101 Switching Protocols\r\nSec-WebSocket-Accept: bogusbogusbogus=\r\n\r\n".to_vec(),
        );
        let err = handshake_over(&mut dup, "h", "/x", &[]).unwrap_err();
        assert!(err.contains("Accept mismatch"), "{err}");
    }

    #[test]
    fn reads_unmasked_text_and_binary() {
        // RFC 6455 §5.7「a single-frame unmasked text message」。
        let mut dup = Duplex::with(encode_server_frame(OP_TEXT, b"Hello"));
        let msg = read_message(&mut dup).expect("text");
        assert!(matches!(msg, Incoming::Text(ref t) if t == b"Hello"));
        // 长载荷走 126 长度分支。
        let big = vec![7u8; 300];
        let mut dup = Duplex::with(encode_server_frame(OP_BINARY, &big));
        let msg = read_message(&mut dup).expect("binary");
        assert!(matches!(msg, Incoming::Binary(ref t) if t.len() == 300));
    }

    #[test]
    fn ping_is_answered_and_skipped() {
        // §5.7 unmasked Ping "Hello" 后跟一条 text；pong 应回到写侧。
        let mut served = encode_server_frame(OP_PING, b"Hello");
        served.extend(encode_server_frame(OP_TEXT, b"after-ping"));
        let mut dup = Duplex::with(served);
        let msg = read_message(&mut dup).expect("text after ping");
        assert!(matches!(msg, Incoming::Text(ref t) if t == b"after-ping"));
        // 写侧收到一个 FIN|PONG(0x8A) 且掩码位为 1 的帧。
        let sent = dup.sent.lock().unwrap().clone();
        assert_eq!(sent[0], 0x8A);
        assert_eq!(sent[1] & 0x80, 0x80);
        assert_eq!(sent[1] & 0x7F, 5);
    }

    #[test]
    fn close_frame_reports_code() {
        let mut dup = Duplex::with(encode_server_frame(OP_CLOSE, &1000u16.to_be_bytes()));
        let msg = read_message(&mut dup).expect("close");
        assert!(matches!(msg, Incoming::Close(Some(1000))));
        // 空载荷 close。
        let mut dup = Duplex::with(encode_server_frame(OP_CLOSE, b""));
        let msg = read_message(&mut dup).expect("close");
        assert!(matches!(msg, Incoming::Close(None)));
    }

    #[test]
    fn protocol_violations_are_errors() {
        // 服务端帧不得掩码。
        let masked = encode_client_frame(OP_TEXT, b"x", [1, 2, 3, 4]);
        let mut dup = Duplex::with(masked);
        assert!(read_message(&mut dup).unwrap_err().contains("masked"));
        // RSV 位（扩展协商）置位。
        let mut frame = encode_server_frame(OP_TEXT, b"x");
        frame[0] |= 0x40;
        let mut dup = Duplex::with(frame);
        assert!(read_message(&mut dup).unwrap_err().contains("RSV"));
        // 分片（FIN=0）不支持。
        let mut frame = encode_server_frame(OP_TEXT, b"x");
        frame[0] &= !0x80;
        let mut dup = Duplex::with(frame);
        assert!(read_message(&mut dup).unwrap_err().contains("fragmented"));
    }

    #[test]
    fn send_text_writes_masked_client_frame() {
        let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut conn = WsConn::over(Box::new(Duplex::with_sink(sent.clone(), Vec::new())));
        conn.send_text(b"payload!").expect("send");
        let bytes = sent.lock().unwrap().clone();
        // FIN|TEXT(0x81)、掩码位 + 长度 8。
        assert_eq!(bytes[0], 0x81);
        assert_eq!(bytes[1], 0x80 | 8);
        let mask: [u8; 4] = bytes[2..6].try_into().unwrap();
        let decoded: Vec<u8> = bytes[6..].iter().enumerate().map(|(i, b)| b ^ mask[i & 3]).collect();
        assert_eq!(decoded, b"payload!");
    }

    #[test]
    fn close_normally_writes_1000_close() {
        let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let mut conn = WsConn::over(Box::new(Duplex::with_sink(sent.clone(), Vec::new())));
        conn.close_normally();
        let bytes = sent.lock().unwrap().clone();
        assert_eq!(bytes[0], 0x88); // FIN|CLOSE
        assert_eq!(bytes[1], 0x80 | 2); // masked, len 2
        let mask: [u8; 4] = bytes[2..6].try_into().unwrap();
        let decoded: Vec<u8> = bytes[6..].iter().enumerate().map(|(i, b)| b ^ mask[i & 3]).collect();
        assert_eq!(decoded, 1000u16.to_be_bytes().to_vec());
    }

    #[test]
    fn url_parsing() {
        let t = parse_ws_url("wss://chatgpt.com/backend-api/codex/responses").unwrap();
        assert!(t.secure && t.host == "chatgpt.com" && t.port == 443 && t.path == "/backend-api/codex/responses");
        let t = parse_ws_url("ws://127.0.0.1:9/codex/responses").unwrap();
        assert!(!t.secure && t.host == "127.0.0.1" && t.port == 9 && t.path == "/codex/responses");
        let t = parse_ws_url("wss://example.test").unwrap();
        assert!(t.path == "/");
        let t = parse_ws_url("ws://[::1]:9000/x?y=1").unwrap();
        assert!(t.host == "::1" && t.port == 9000 && t.path == "/x?y=1");
        assert!(parse_ws_url("https://chatgpt.com").is_err());
        assert!(parse_ws_url("wss://chatgpt.com:notaport/x").is_err());
        assert!(parse_ws_url("wss:///x").is_err());
    }

    // ---- keep 形态：本地 TcpListener 假服务端（真握手、真帧、真 TCP）----

    /// 读一段 HTTP 请求头（到 \r\n\r\n）。
    fn read_http_head<S: Read>(stream: &mut S) -> String {
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            match stream.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => head.push(byte[0]),
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&head).into_owned()
    }

    /// 读一条客户端帧（恒掩码）并解掩码；EOF/断线 → None。
    fn read_client_frame<S: Read>(stream: &mut S) -> Option<(u8, Vec<u8>)> {
        let mut h = [0u8; 2];
        stream.read_exact(&mut h).ok()?;
        let opcode = h[0] & 0x0F;
        let masked = h[1] & 0x80 != 0;
        let len = match h[1] & 0x7F {
            126 => {
                let mut b = [0u8; 2];
                stream.read_exact(&mut b).ok()?;
                u16::from_be_bytes(b) as usize
            }
            127 => {
                let mut b = [0u8; 8];
                stream.read_exact(&mut b).ok()?;
                u64::from_be_bytes(b) as usize
            }
            n => n as usize,
        };
        let mut mask = [0u8; 4];
        if masked {
            stream.read_exact(&mut mask).ok()?;
        }
        let mut payload = vec![0u8; len];
        stream.read_exact(&mut payload).ok()?;
        if masked {
            for (i, b) in payload.iter_mut().enumerate() {
                *b ^= mask[i & 3];
            }
        }
        Some((opcode, payload))
    }

    /// 假服务端：回 101 完成握手，返回 (客户端 Key 之外的应答已写毕, 流)。
    fn serve_upgrade<S: Read + Write>(stream: &mut S) {
        let head = read_http_head(stream);
        let key = head
            .lines()
            .find(|l| l.to_ascii_lowercase().starts_with("sec-websocket-key:"))
            .and_then(|l| l.split_once(':'))
            .map(|(_, v)| v.trim().to_string())
            .unwrap_or_default();
        let res = format!(
            "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n",
            accept_for(&key)
        );
        stream.write_all(res.as_bytes()).unwrap();
        stream.flush().unwrap();
    }

    /// keep 形态的主体：同一条连接上先后跑两个回合（握手一次）。读到底
    /// **不**断连是 keep 池的前提——`read_message` 只消费帧、从不关流，
    /// 第二回合在同一 `WsConn` 上继续 send/read（Go codexKept 的复用路径，
    /// codex_socket_keep_test.go TestCodexKeptSocketSendsTheDelta 的传输层
    /// 前提）。
    #[test]
    fn keep_form_two_turns_on_one_connection() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let seen: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let accepted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (seen_c, acc_c) = (seen.clone(), accepted.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                acc_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                serve_upgrade(&mut stream);
                // 每条 text 帧按内容回一条；close/断线即收工。
                while let Some((op, payload)) = read_client_frame(&mut stream) {
                    if op == OP_CLOSE {
                        break;
                    }
                    if op == OP_TEXT {
                        seen_c.lock().unwrap().push(payload.clone());
                        let reply = if payload == b"turn-1" { b"reply-1" } else { b"reply-2" };
                        let _ = stream.write_all(&encode_server_frame(OP_TEXT, reply));
                        let _ = stream.flush();
                    }
                }
            }
        });

        let url = format!("ws://127.0.0.1:{port}/keep");
        let mut conn = dial(&url, &[], Duration::from_secs(5), Duration::from_secs(5)).expect("dial");
        // 第一回合。
        conn.send_text(b"turn-1").expect("send 1");
        assert!(matches!(conn.read_message(), Ok(Incoming::Text(ref t)) if t == b"reply-1"));
        // 第二回合：同一连接、不重新握手（keep 复用的形态）。
        conn.send_text(b"turn-2").expect("send 2");
        assert!(matches!(conn.read_message(), Ok(Incoming::Text(ref t)) if t == b"reply-2"));
        conn.close_normally();

        assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1, "one connection, one handshake");
        assert_eq!(*seen.lock().unwrap(), vec![b"turn-1".to_vec(), b"turn-2".to_vec()]);
    }

    /// 对端 close 后连接必须「可判死」：读到 close 帧（含 1009），再读写都
    /// 失败——keep 池据此回收（复用回合的 Again 路径，codex_account.rs）。
    #[test]
    fn peer_close_ends_the_connection_for_recycling() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                serve_upgrade(&mut stream);
                // 一个回合后以 1009 关闭并断开（后端 message too big 的形状）。
                while let Some((op, _payload)) = read_client_frame(&mut stream) {
                    if op == OP_CLOSE {
                        break;
                    }
                    if op == OP_TEXT {
                        let _ = stream.write_all(&encode_server_frame(OP_TEXT, b"reply"));
                        let _ = stream.flush();
                        break;
                    }
                }
                let _ = stream.write_all(&encode_server_frame(OP_CLOSE, &1009u16.to_be_bytes()));
                let _ = stream.flush();
            }
        });

        let url = format!("ws://127.0.0.1:{port}/keep");
        let mut conn = dial(&url, &[], Duration::from_secs(5), Duration::from_secs(5)).expect("dial");
        conn.send_text(b"turn-1").expect("send");
        assert!(matches!(conn.read_message(), Ok(Incoming::Text(ref t)) if t == b"reply"));
        // close 帧带 1009 上来。
        assert!(matches!(conn.read_message(), Ok(Incoming::Close(Some(1009)))));
        // 此后连接已死：写可能进 OS 缓冲（也可能直接失败），读必然失败——
        // 池的复用方以此判定回收，绝不能把死连接当可用。
        let _ = conn.send_text(b"again");
        assert!(conn.read_message().is_err(), "read on a peer-closed connection must fail");
    }
}
