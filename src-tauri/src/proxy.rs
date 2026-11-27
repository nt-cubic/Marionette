//! Agent proxy configuration.
//!
//! Marionette is an agent harness: it starts agent CLIs (codex, opencode, …)
//! as child processes. When the user lives behind a censored or metered
//! network, those agents need an HTTP(S) proxy exit. We store a *single*
//! proxy address and inject it into every spawned agent via the standard
//! `HTTPS_PROXY` / `HTTP_PROXY` / `NO_PROXY` environment variables.
//!
//! Deliberate scope:
//! - Only one address. Rule-vs-global routing belongs to the local proxy
//!   client (Clash, v2ray, …); we never try to control it.
//! - `NO_PROXY` is fixed to localhost/loopback so local MCP servers spawned
//!   by agents are never proxied. Not exposed in the UI.
//! - No per-agent overrides. One exit for everything.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Loopback is always kept direct — local MCP servers (Unity, drawio,
/// playwright, node_repl, …) must never round-trip through the proxy.
pub const NO_PROXY_DEFAULT: &str = "localhost,127.0.0.1,::1";

/// Endpoint used by `test_proxy`. Reaching it (even with HTTP 401) proves
/// the full path — DNS, CONNECT tunnel and TLS — works through the proxy.
const TEST_TARGET: &str = "https://api.openai.com/v1/models";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyConfig {
    /// Whether spawned agents should route through `url`.
    #[serde(default)]
    pub enabled: bool,
    /// Full proxy URL, e.g. `http://127.0.0.1:7890`. May carry `user:pass@`.
    #[serde(default)]
    pub url: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            url: String::new(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyTestResult {
    pub ok: bool,
    /// Human-readable outcome (latency or failure reason).
    pub message: String,
    /// Round-trip latency in ms when the proxy path responded.
    pub latency_ms: Option<u64>,
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn proxy_config_path() -> Option<PathBuf> {
    Some(home_dir()?.join(".marionette").join("proxy.json"))
}

pub fn load_proxy_config() -> ProxyConfig {
    let Some(path) = proxy_config_path() else {
        return ProxyConfig::default();
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return ProxyConfig::default();
    };
    serde_json::from_str(&raw).unwrap_or_default()
}

/// Persist the config. An enabled config must be a usable proxy URL.
pub fn save_proxy_config(config: &ProxyConfig) -> Result<(), String> {
    if config.enabled {
        validate_proxy_url(&config.url)?;
    }
    let Some(path) = proxy_config_path() else {
        return Err("cannot locate home directory".to_string());
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create ~/.marionette: {e}"))?;
    }
    let raw =
        serde_json::to_string_pretty(config).map_err(|e| format!("serialize proxy config: {e}"))?;
    std::fs::write(&path, raw).map_err(|e| format!("write proxy config: {e}"))?;
    Ok(())
}

/// Accepts `http://host:port`, `socks5://host:port` (also `socks5h`, `socks`,
/// `socks4`, `socks4a`) or bare `host:port` (= http). An `https://` proxy is
/// rejected: ureq 2 cannot speak TLS to a proxy, and pretending otherwise would
/// route the app's traffic somewhere it does not go.
pub fn validate_proxy_url(url: &str) -> Result<(), String> {
    let url = url.trim();
    if url.is_empty() {
        return Err(crate::ts!(
            "代理地址不能为空",
            "Proxy address cannot be empty"
        ));
    }
    if !url.starts_with("http://") && !url.starts_with("https://") {
        // Allow bare host:port (ureq treats it as http), but still require a port.
        if !url.contains(':') {
            return Err(crate::ts!(
                "代理地址缺少端口，例如 http://127.0.0.1:7890",
                "Proxy address is missing a port, e.g. http://127.0.0.1:7890"
            ));
        }
    }
    ureq_proxy(url)
        .map(|_| ())
        .map_err(|e| crate::ts!("代理地址无效: {e}", "Invalid proxy address: {e}"))
}

/// The app's one way to turn a user-typed proxy address into a `ureq::Proxy`
/// (test / source probing / engine-less callers). Callers must not call
/// `ureq::Proxy::new` directly:
/// - `socks5h://` is the curl-style spelling (the proxy resolves names); ureq 2
///   only knows `socks5://`, whose SOCKS5 path already hands the hostname to
///   the proxy — so the two are synonyms and the rewrite is lossless.
/// - The scheme is case-insensitive (ureq's own match is case-sensitive, so
///   `SOCKS5://` would be rejected as invalid).
pub fn ureq_proxy(url: &str) -> Result<ureq::Proxy, ureq::Error> {
    let url = url.trim();
    let normalized = match url.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("socks5h") => {
            format!("socks5://{rest}")
        }
        Some((scheme, rest)) => format!("{}://{rest}", scheme.to_ascii_lowercase()),
        None => url.to_string(),
    };
    ureq::Proxy::new(normalized)
}

/// Environment to inject into agent processes when a proxy is enabled.
/// Returns `(HTTPS_PROXY/HTTP_PROXY value, NO_PROXY value)`.
pub fn proxy_env() -> Option<(String, String)> {
    let config = load_proxy_config();
    if !config.enabled {
        return None;
    }
    let url = config.url.trim();
    if url.is_empty() {
        return None;
    }
    Some((url.to_string(), NO_PROXY_DEFAULT.to_string()))
}

/// Round-trip through `url` to OpenAI's API endpoint. HTTP 401 still counts
/// as success: it means the proxy path itself is alive (auth is a later step).
pub fn test_proxy(url: &str) -> Result<ProxyTestResult, String> {
    test_proxy_against(url, TEST_TARGET)
}

/// `test_proxy` with a caller-chosen target (tests use a loopback server; the
/// UI always goes through [`test_proxy`]).
fn test_proxy_against(url: &str, target: &str) -> Result<ProxyTestResult, String> {
    let proxy = ureq_proxy(url).map_err(|e| format!("代理地址无效: {e}"))?;
    let connector = ureq::native_tls::TlsConnector::new()
        .map_err(|e| format!("TLS init failed (schannel): {e}"))?;
    let agent = ureq::AgentBuilder::new()
        .proxy(proxy)
        .tls_connector(Arc::new(connector))
        .timeout(Duration::from_secs(12))
        .build();

    let started = Instant::now();
    match agent.get(target).call() {
        Ok(_) => {
            let ms = started.elapsed().as_millis() as u64;
            Ok(ProxyTestResult {
                ok: true,
                message: crate::ts!("连接成功 · {ms}ms", "Connected · {ms}ms"),
                latency_ms: Some(ms),
            })
        }
        // Any HTTP status (notably 401 without a key) proves the tunnel works.
        Err(ureq::Error::Status(code, _)) => {
            let ms = started.elapsed().as_millis() as u64;
            Ok(ProxyTestResult {
                ok: true,
                message: crate::ts!(
                    "代理连通（HTTP {code}）· {ms}ms",
                    "Proxy reachable (HTTP {code}) · {ms}ms"
                ),
                latency_ms: Some(ms),
            })
        }
        Err(error) => Ok(ProxyTestResult {
            ok: false,
            message: failure_message(&error),
            latency_ms: None,
        }),
    }
}

/// "连接失败: <ureq 的原话> · <一句能照着做的原因>". The raw chain stays (it
/// names the target and the OS error); the hint is appended only when the
/// chain carries an io error this function recognises — never invented.
fn failure_message(error: &ureq::Error) -> String {
    let raw = crate::ts!("连接失败: {error}", "Connection failed: {error}");
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(err) = source {
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            let hint = match io.kind() {
                std::io::ErrorKind::ConnectionRefused => Some(crate::ts!(
                    "代理端口没有响应，检查代理客户端是否在运行、端口是否正确",
                    "the proxy port did not answer — check that your proxy client is running and the port is right"
                )),
                std::io::ErrorKind::TimedOut => Some(crate::ts!(
                    "连代理超时，检查地址和端口",
                    "timed out reaching the proxy — check the address and port"
                )),
                _ => None,
            };
            if let Some(hint) = hint {
                return format!("{raw} · {hint}");
            }
        }
        source = err.source();
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;

    /// 目标站点经代理回给客户端的应答（OpenAI 形状的空目录）。
    const OK_RESPONSE: &str = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"data\":[]}";
    /// 一定是 http 的测试目标：`.invalid` 永远解析不出来——请求能成功，就证明
    /// 域名交给了代理解析，客户端没有自己在 DNS 上查它。
    const TARGET: &str = "http://example.invalid/v1/models";

    /// 一个没人监听的回环端口（bind 后立刻放掉）。
    fn closed_port() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    }

    fn read_exact(stream: &mut TcpStream, n: usize) -> Vec<u8> {
        let mut buf = vec![0u8; n];
        stream.read_exact(&mut buf).unwrap();
        buf
    }

    /// 读完一个 HTTP 请求头（到空行为止）。
    fn read_http_head(stream: &mut TcpStream) -> String {
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            assert_eq!(
                stream.read(&mut byte).unwrap(),
                1,
                "request head ended early"
            );
            head.push(byte[0]);
            assert!(head.len() < 64 * 1024, "request head too large");
        }
        String::from_utf8_lossy(&head).into_owned()
    }

    /// 最小 SOCKS5 假服务端（不转发）：协商 →（`require_password` 时）用户名
    /// 密码 → CONNECT → 按 `reply` 回应；`reply == 0` 时在同一连接上充当目标
    /// 站点。把 CONNECT 的目标 `host:port` 与凭据子协商交给调用方判定。
    fn mock_socks5(reply: u8, require_password: bool) -> (u16, mpsc::Receiver<(String, Vec<u8>)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let methods = read_exact(&mut stream, 2);
                let methods = read_exact(&mut stream, methods[1] as usize);
                let mut creds = Vec::new();
                if require_password {
                    assert!(
                        methods.contains(&0x02),
                        "client must offer username/password"
                    );
                    stream.write_all(&[0x05, 0x02]).unwrap();
                    let head = read_exact(&mut stream, 2);
                    let mut rest = read_exact(&mut stream, head[1] as usize);
                    let plen = read_exact(&mut stream, 1);
                    rest.extend(&plen);
                    rest.extend(read_exact(&mut stream, plen[0] as usize));
                    creds = head;
                    creds.extend(rest);
                    stream.write_all(&[0x01, 0x00]).unwrap();
                } else {
                    assert!(methods.contains(&0x00), "client must offer no-auth");
                    stream.write_all(&[0x05, 0x00]).unwrap();
                }
                let head = read_exact(&mut stream, 4);
                let target = match head[3] {
                    0x01 => read_exact(&mut stream, 4)
                        .iter()
                        .map(|b| b.to_string())
                        .collect::<Vec<_>>()
                        .join("."),
                    0x03 => {
                        let len = read_exact(&mut stream, 1)[0] as usize;
                        String::from_utf8(read_exact(&mut stream, len)).unwrap()
                    }
                    other => panic!("unexpected ATYP {other}"),
                };
                let port = u16::from_be_bytes([
                    read_exact(&mut stream, 1)[0],
                    read_exact(&mut stream, 1)[0],
                ]);
                stream
                    .write_all(&[0x05, reply, 0x00, 0x01, 127, 0, 0, 1, 0, 80])
                    .unwrap();
                if reply == 0 {
                    read_http_head(&mut stream);
                    stream.write_all(OK_RESPONSE.as_bytes()).unwrap();
                }
                tx.send((format!("{target}:{port}"), creds)).unwrap();
            }
        });
        (port, rx)
    }

    /// 最小 HTTP 代理假服务端：读到请求头后原样回 `response`。返回请求行。
    fn mock_http_proxy(response: &'static str) -> (u16, mpsc::Receiver<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let head = read_http_head(&mut stream);
                stream.write_all(response.as_bytes()).unwrap();
                tx.send(head.lines().next().unwrap_or("").to_string())
                    .unwrap();
            }
        });
        (port, rx)
    }

    /// 回归（2026-11-26 用户实锤）：`socks5://` 以前在「测试连接 / 来源探测 /
    /// 引擎出站」全部报 "SOCKS feature disabled"。三种写法都要能走通 SOCKS5。
    #[test]
    fn socks5_proxy_passes_the_request_and_hands_the_domain_over() {
        for scheme in ["socks5", "socks5h", "SOCKS5"] {
            let (port, seen) = mock_socks5(0x00, false);
            let result = test_proxy_against(&format!("{scheme}://127.0.0.1:{port}"), TARGET)
                .unwrap_or_else(|e| panic!("{scheme}: {e}"));
            assert!(result.ok, "{scheme}: {}", result.message);
            assert!(result.latency_ms.is_some(), "{scheme}");
            assert_eq!(
                seen.recv().unwrap(),
                ("example.invalid:80".to_string(), Vec::new()),
                "{scheme}"
            );
        }
    }

    #[test]
    fn socks5_proxy_with_credentials_negotiates_username_password() {
        let (port, seen) = mock_socks5(0x00, true);
        let result =
            test_proxy_against(&format!("socks5://john:1234@127.0.0.1:{port}"), TARGET).unwrap();
        assert!(result.ok, "{}", result.message);
        let (target, creds) = seen.recv().unwrap();
        assert_eq!(target, "example.invalid:80");
        let mut expected = vec![0x01, 4];
        expected.extend_from_slice(b"john");
        expected.push(4);
        expected.extend_from_slice(b"1234");
        assert_eq!(creds, expected, "RFC 1929 sub-negotiation bytes");
    }

    /// 代理拒绝（REP=5 connection refused）必须是 ok=false：前端按 ok 画红/绿。
    #[test]
    fn socks5_refusal_is_a_failure_not_a_success() {
        let (port, _seen) = mock_socks5(0x05, false);
        let result = test_proxy_against(&format!("socks5://127.0.0.1:{port}"), TARGET).unwrap();
        assert!(!result.ok, "{}", result.message);
        assert!(result.latency_ms.is_none());
        assert!(!result.message.is_empty());
    }

    /// HTTP 代理路径不许被 socks 改动波及：请求到达代理；代理回的任何 HTTP
    /// 状态（这里是 407）都算「代理连通」。
    #[test]
    fn http_proxy_path_still_reaches_the_proxy() {
        let (port, seen) = mock_http_proxy(OK_RESPONSE);
        let result = test_proxy_against(&format!("http://127.0.0.1:{port}"), TARGET).unwrap();
        assert!(
            result.ok && result.latency_ms.is_some(),
            "{}",
            result.message
        );
        assert!(
            seen.recv().unwrap().starts_with("GET "),
            "absolute or origin form both fine"
        );

        let (port, _seen) = mock_http_proxy(
            "HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n",
        );
        let result = test_proxy_against(&format!("http://127.0.0.1:{port}"), TARGET).unwrap();
        assert!(result.ok, "{}", result.message);
        assert!(result.message.contains("407"), "{}", result.message);
    }

    /// 代理端口没人听 = 最常见的一种失败：ok=false，且原话之外补一句能照着做的原因。
    #[test]
    fn unreachable_proxy_port_is_a_failure_with_a_hint() {
        let port = closed_port();
        let result = test_proxy_against(&format!("socks5://127.0.0.1:{port}"), TARGET).unwrap();
        assert!(!result.ok, "{}", result.message);
        let zh = "代理端口没有响应";
        let en = "proxy port did not answer";
        assert!(
            result.message.contains(zh) || result.message.contains(en),
            "{}",
            result.message
        );
    }

    #[test]
    fn validate_accepts_socks_and_rejects_https_proxies() {
        for url in [
            "socks5://127.0.0.1:7891",
            "socks5h://127.0.0.1:7891",
            "SOCKS5://127.0.0.1:7891",
            "socks://127.0.0.1:7891",
            "socks4://127.0.0.1:1080",
            "http://127.0.0.1:7890",
            "127.0.0.1:7890",
        ] {
            validate_proxy_url(url).unwrap_or_else(|e| panic!("{url}: {e}"));
        }
        // ureq 2 不会跟代理说 TLS：如实拒绝，不让「保存成功」掩盖一条走不通的配置。
        assert!(validate_proxy_url("https://127.0.0.1:8080").is_err());
        assert!(validate_proxy_url("").is_err());
        assert!(
            validate_proxy_url("127.0.0.1").is_err(),
            "bare host without a port"
        );
        assert!(validate_proxy_url("ftp://127.0.0.1:21").is_err());
    }
}
