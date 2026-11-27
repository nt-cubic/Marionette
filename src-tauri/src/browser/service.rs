//! Loopback-only dev-server probing for the workspace watcher.

use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use tauri::Url;

pub fn probe_loopback_url(raw: &str) -> Result<bool, String> {
    let mut url = Url::parse(raw.trim()).map_err(|error| format!("invalid URL: {error}"))?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return Err("only http(s) loopback services can be probed".to_string());
    }
    let host = url.host_str().ok_or("URL has no host")?;
    let literal = host
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(host)
        .to_ascii_lowercase();
    let loopback_name = literal == "localhost" || literal.ends_with(".localhost");
    let loopback_ip = literal
        .parse::<std::net::IpAddr>()
        .ok()
        .is_some_and(|ip| ip.is_loopback());
    let unspecified = literal
        .parse::<std::net::IpAddr>()
        .ok()
        .is_some_and(|ip| ip.is_unspecified());
    if !loopback_name && !loopback_ip && !unspecified {
        return Err("dev-server probe only accepts loopback addresses".to_string());
    }
    if unspecified {
        url.set_host(Some("localhost"))
            .map_err(|_| "could not rewrite wildcard host")?;
    }
    let port = url.port_or_known_default().ok_or("URL has no port")?;
    let authority = format!("{}:{port}", url.host_str().ok_or("URL has no host")?);
    let addresses = authority
        .to_socket_addrs()
        .map_err(|error| format!("resolve {authority}: {error}"))?;
    Ok(addresses
        .into_iter()
        .any(|address| TcpStream::connect_timeout(&address, Duration::from_millis(350)).is_ok()))
}

#[cfg(test)]
mod tests {
    use super::probe_loopback_url;

    #[test]
    fn public_and_non_http_addresses_are_refused() {
        assert!(probe_loopback_url("https://example.com").is_err());
        assert!(probe_loopback_url("file:///tmp/index.html").is_err());
    }

    #[test]
    fn a_closed_loopback_port_is_a_false_probe_not_an_error() {
        assert_eq!(probe_loopback_url("http://127.0.0.1:9/").unwrap(), false);
    }
}
