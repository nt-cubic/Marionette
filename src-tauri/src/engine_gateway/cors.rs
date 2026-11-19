//! 外站网页防护（Go `cors.go` 的 corsGuard 移植；审计 #5 / #21 的 OPTIONS）。
//!
//! 威胁：loopback 上调用不要钥匙、体的 Content-Type 也不校验——用户仅仅
//! 访问的网页就能发一个「简单」`text/plain` POST，**盲打消耗订阅**。所以：
//!
//! - 没有 `Origin`（所有 agent/CLI 的调用）→ 原样放行；
//! - `Origin` 是 http(s) 页面且既不在本机（localhost / `*.localhost` /
//!   127.0.0.0/8 / `::1`，任意端口）也不是请求自己的 Host → **403**；
//! - `Origin: null`（沙箱 iframe / file:// 页）→ 403。Go 允许带网关钥匙的
//!   放行；Rust 引擎没有网关钥匙概念，所以一律拒；
//! - `localhost` 的子域（Safari 会把名字交给系统 DNS，可能不在本机）→ 403，
//!   Windows 上 `tauri.localhost` / `wails.localhost` 例外（两家 webview 在那里
//!   用这两个名字自托管）；
//! - 非 http(s) 方案（app:// tauri:// file:// vscode-webview:// chrome-extension://）
//!   只有桌面 app / 扩展能发，照旧放行；
//! - `app://obsidian.md`（Obsidian 插件）额外得到 CORS 头，预检直接 204。
//!
//! 用户可配的「Settings › 网页白名单」（Go `settings.CORSOrigins`）在 Rust
//! 引擎里没有对应物：白名单恒空，**没有任何网页能读到应答**（本来就没有）。
use std::net::IpAddr;

/// 判定结果。
#[derive(Debug, PartialEq)]
pub enum Verdict {
    /// 照常路由，不加 CORS 头。
    Pass,
    /// 照常路由，应答带这些头（app 来源）。
    PassWith(Vec<(String, String)>),
    /// 预检：直接 204 + 头，不路由。
    Preflight(Vec<(String, String)>),
    /// 拒绝：403 + 消息。
    Refuse(String),
}

const APP_ORIGINS: &[&str] = &["app://obsidian.md"];

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

/// 入口：method + 小写头名的头表。
pub fn judge(method: &str, headers: &[(String, String)]) -> Verdict {
    let origin = header(headers, "origin").unwrap_or("").trim().to_string();
    if origin.is_empty() {
        return Verdict::Pass;
    }
    if app_origin(&origin) {
        let mut h = vec![
            ("Access-Control-Allow-Origin".to_string(), origin.clone()),
            ("Vary".to_string(), "Origin".to_string()),
        ];
        if is_preflight(method, headers) {
            h.push(("Access-Control-Allow-Methods".into(), "GET, POST, OPTIONS".into()));
            if let Some(asked) = header(headers, "access-control-request-headers") {
                let allowed = cors_headers(asked);
                if !allowed.is_empty() {
                    h.push(("Access-Control-Allow-Headers".into(), allowed));
                }
            }
            if header(headers, "access-control-request-private-network") == Some("true") {
                h.push(("Access-Control-Allow-Private-Network".into(), "true".into()));
            }
            h.push(("Access-Control-Max-Age".into(), "600".into()));
            return Verdict::Preflight(h);
        }
        return Verdict::PassWith(h);
    }
    let host = header(headers, "host").unwrap_or("");
    if foreign_page(host, &origin) {
        return Verdict::Refuse(format!(
            "magpie doesn't answer the web page {origin}: only pages on this computer, \
             desktop apps and agents may call the gateway"
        ));
    }
    if origin == "null" {
        return Verdict::Refuse(
            "magpie doesn't answer a sandboxed frame or a local page (Origin: null)".to_string(),
        );
    }
    if localhost_subdomain(&origin) {
        return Verdict::Refuse(format!(
            "magpie doesn't answer the web page {origin}: Safari and other WebKit browsers ask \
             the network's DNS for a localhost subdomain, so the page may not be on this \
             computer. Open it at localhost or 127.0.0.1"
        ));
    }
    Verdict::Pass
}

fn app_origin(origin: &str) -> bool {
    let o = origin.trim_end_matches('/').to_ascii_lowercase();
    APP_ORIGINS.contains(&o.as_str())
}

/// 浏览器的 CORS 预检：带 Origin 的 OPTIONS 且声明了将要发的方法。
fn is_preflight(method: &str, headers: &[(String, String)]) -> bool {
    method == "OPTIONS"
        && header(headers, "origin").is_some_and(|o| !o.is_empty())
        && header(headers, "access-control-request-method").is_some_and(|m| !m.is_empty())
}

/// 预检的 Access-Control-Request-Headers：只放行名字合法的那些。
fn cors_headers(asked: &str) -> String {
    asked
        .split(',')
        .map(str::trim)
        .filter(|f| {
            !f.is_empty()
                && f.chars().all(|c| c == '-' || c == '_' || c.is_ascii_alphanumeric())
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// origin 拆成（小写 scheme，authority 去掉 userinfo）。没有 `://` → scheme 空。
fn split_origin(origin: &str) -> (String, String) {
    match origin.split_once("://") {
        Some((scheme, rest)) => {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
            let authority = authority.rsplit('@').next().unwrap_or(authority);
            (scheme.to_ascii_lowercase(), authority.to_ascii_lowercase())
        }
        None => (String::new(), String::new()),
    }
}

/// authority → (主机名无括号, 显式端口)。
fn split_host_port(hp: &str) -> (String, Option<String>) {
    if let Some(rest) = hp.strip_prefix('[') {
        if let Some((host, tail)) = rest.split_once(']') {
            let port = tail.strip_prefix(':').filter(|p| !p.is_empty()).map(String::from);
            return (host.to_string(), port);
        }
        return (rest.to_string(), None);
    }
    // 多个冒号且没括号 = 裸 IPv6，不是 host:port
    if hp.matches(':').count() > 1 {
        return (hp.to_string(), None);
    }
    match hp.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), Some(p.to_string()).filter(|p| !p.is_empty())),
        None => (hp.to_string(), None),
    }
}

/// host[:port] 小写，缺端口按 scheme 补默认（https://a.example 与 Host
/// a.example:443 相等）。
fn host_port(hp: &str, scheme: &str) -> String {
    let hp = hp.to_ascii_lowercase();
    if hp.is_empty() {
        return String::new();
    }
    let (host, port) = split_host_port(&hp);
    let port = port.unwrap_or_else(|| if scheme == "https" { "443".into() } else { "80".into() });
    format!("{host}:{port}")
}

fn is_loopback_host(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") {
        return true;
    }
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => v4.is_loopback(),
        Ok(IpAddr::V6(v6)) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
        Err(_) => false,
    }
}

/// Go foreignPage：origin 是 http(s) 页面，既不在本机也不是请求自己的源。
fn foreign_page(request_host: &str, origin: &str) -> bool {
    if origin.is_empty() {
        return false;
    }
    let (scheme, authority) = split_origin(origin);
    if scheme.is_empty() {
        // 没有 :// 的东西：只有以 http 开头的才算「读不懂的网页」
        return origin.to_ascii_lowercase().starts_with("http");
    }
    if scheme != "http" && scheme != "https" {
        return false;
    }
    let (host, _) = split_host_port(&authority);
    if is_loopback_host(&host) {
        return false;
    }
    host_port(&authority, &scheme) != host_port(request_host, &scheme)
}

/// Go localhostSubdomain。
fn localhost_subdomain(origin: &str) -> bool {
    let (scheme, authority) = split_origin(origin);
    if scheme != "http" && scheme != "https" {
        return false;
    }
    let (host, _) = split_host_port(&authority);
    let host = host.trim_end_matches('.');
    if !host.ends_with(".localhost") {
        return false;
    }
    !(cfg!(windows) && (host == "tauri.localhost" || host == "wails.localhost"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn no_origin_passes() {
        assert_eq!(judge("POST", &hs(&[("host", "127.0.0.1:9999")])), Verdict::Pass);
    }

    #[test]
    fn foreign_web_page_is_refused_on_every_method() {
        for method in ["GET", "POST", "OPTIONS"] {
            let v = judge(method, &hs(&[("host", "127.0.0.1:9999"), ("origin", "https://evil.example")]));
            assert!(matches!(v, Verdict::Refuse(_)), "{method}: {v:?}");
        }
        // 预检也一样（带 ACRM 的 OPTIONS）
        let v = judge(
            "OPTIONS",
            &hs(&[
                ("host", "127.0.0.1:9999"),
                ("origin", "http://evil.example"),
                ("access-control-request-method", "POST"),
            ]),
        );
        assert!(matches!(v, Verdict::Refuse(_)));
    }

    #[test]
    fn local_pages_pass_any_port() {
        for o in [
            "http://localhost:5173",
            "http://127.0.0.1:3000",
            "http://127.1.2.3",
            "http://[::1]:8080",
            "http://app.localhost:1420",
        ] {
            let v = judge("POST", &hs(&[("host", "127.0.0.1:9999"), ("origin", o)]));
            // *.localhost 不是 foreign，但要过子域规则（Windows 上只放 tauri/wails）
            if o.contains("app.localhost") {
                assert!(matches!(v, Verdict::Refuse(_)), "{o}: {v:?}");
            } else {
                assert_eq!(v, Verdict::Pass, "{o}");
            }
        }
    }

    #[test]
    fn same_origin_as_the_request_host_passes() {
        let v = judge("POST", &hs(&[("host", "192.168.1.5:9999"), ("origin", "http://192.168.1.5:9999")]));
        assert_eq!(v, Verdict::Pass);
        // 端口不同 = 另一个站
        let v = judge("POST", &hs(&[("host", "192.168.1.5:9999"), ("origin", "http://192.168.1.5:8000")]));
        assert!(matches!(v, Verdict::Refuse(_)));
        // 默认端口补齐
        let v = judge("POST", &hs(&[("host", "gw.lan:443"), ("origin", "https://gw.lan")]));
        assert_eq!(v, Verdict::Pass);
    }

    #[test]
    fn null_origin_is_refused() {
        let v = judge("POST", &hs(&[("host", "127.0.0.1:1"), ("origin", "null")]));
        assert!(matches!(v, Verdict::Refuse(_)));
    }

    #[test]
    fn app_schemes_pass_untouched() {
        for o in ["tauri://localhost", "file://", "vscode-webview://abc", "chrome-extension://abc"] {
            let v = judge("POST", &hs(&[("host", "127.0.0.1:1"), ("origin", o)]));
            assert_eq!(v, Verdict::Pass, "{o}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn tauri_localhost_is_exempt_on_windows() {
        let v = judge("POST", &hs(&[("host", "127.0.0.1:1"), ("origin", "http://tauri.localhost")]));
        assert_eq!(v, Verdict::Pass);
    }

    #[test]
    fn obsidian_gets_cors_headers_and_204_preflight() {
        let v = judge("POST", &hs(&[("host", "127.0.0.1:1"), ("origin", "app://obsidian.md")]));
        match v {
            Verdict::PassWith(h) => {
                assert!(h.iter().any(|(k, v)| k == "Access-Control-Allow-Origin" && v == "app://obsidian.md"));
            }
            other => panic!("{other:?}"),
        }
        let v = judge(
            "OPTIONS",
            &hs(&[
                ("host", "127.0.0.1:1"),
                ("origin", "app://obsidian.md"),
                ("access-control-request-method", "POST"),
                ("access-control-request-headers", "content-type, x-bad header, authorization"),
                ("access-control-request-private-network", "true"),
            ]),
        );
        match v {
            Verdict::Preflight(h) => {
                let get = |name: &str| h.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str());
                assert_eq!(get("Access-Control-Allow-Methods"), Some("GET, POST, OPTIONS"));
                // 名字里带空格的整条丢弃，其余保留
                assert_eq!(get("Access-Control-Allow-Headers"), Some("content-type, authorization"));
                assert_eq!(get("Access-Control-Allow-Private-Network"), Some("true"));
                assert_eq!(get("Access-Control-Max-Age"), Some("600"));
            }
            other => panic!("{other:?}"),
        }
    }
}
