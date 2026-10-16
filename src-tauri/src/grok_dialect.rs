//! 方言规整器：环回微代理，直通转发 + 按目标 preset 做最小适配。
//!
//! 为什么存在（2026-10-16 实测定案，见 docs/cross-agent-bridge.md 附录）：
//! - grok 重放含 assistant 消息的历史时，会在每条消息对象上标注 `model_id`
//!   （多模型会话追踪）；严格的 OpenAI 兼容端点（实测 api.deepseek.com）以
//!   400 `unsupported_parameter: messages[n].model_id` 拒收。
//! - OpenCode 网关 2026-09 起要求 `x-opencode-session` 系客户端头，缺失即
//!   400（magpie opencode_client.go 实测对照）；claude / grok 都不会自发带。
//! - 个别厂商有同协议内的字段方言（deepseek 无 `max_completion_tokens` 且
//!   静默忽略、kimi k2.5+ 锁定采样参数）。
//! 这些都不是协议差异，不需要翻译网关——「原样转发 + 按目标做已知适配」即可。
//! 这是设计文档「key 型不放网关」原则**记录在案的最小例外**：不翻译协议、
//! 不改写语义、凭证（agent 展开的 Authorization 头）原样透传不落地。
//!
//! 路径约定：`/p/<preset_id>/<rest>`。
//! - rest 以 `v1/messages` / `v1/count_tokens` 开头 → Anthropic 上游（根形态
//!   base，客户端已把 /v1/messages 拼在 rest 里）；
//! - 其余（chat/completions、models 等）→ chat 上游（…/v1 形态 base）。
//! 因此 claude（ANTHROPIC_BASE_URL 指向 `/p/<id>`，自发拼 /v1/messages）与
//! grok（base_url 指向 `/p/<id>`，自发拼 /chat/completions）共用同一个地址。
//!
//! 生命周期与边界：
//! - 惰性启动（首个跨用绑定建注入时），随进程消亡；不主动关停。
//! - 优先绑固定端口 17832，被占则退回临时端口；ensure 的幂等重写会在端口
//!   变化时刷新 agent 侧配置。
//! - 跨用 channel 本就只在 Marionette 会话内有意义（api_key 引用
//!   `$MARIONETTE_PROVIDER_KEY`），「app 不在时代理不可达」不构成额外破坏面。
//!
//! 适配清单按 preset 圈定（magpie LESSONS「Scope a vendor rule to that
//! vendor」）：规则绝不全局套用。当前条目见 [`adapt_body`] 与 [`extra_headers_for`]。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use tiny_http::{Header, Method, Response, Server};

/// 固定首选端口；被占时退回临时端口。
const PREFERRED_PORT: u16 = 17832;

/// 一个 preset 的两个直连上游：chat（…/v1 形态）与 anthropic（根形态）。
#[derive(Clone)]
struct UpstreamPair {
    chat: String,
    anthropic: Option<String>,
}

struct NormalizerState {
    port: u16,
    /// preset_id → 上游对。启动时从 presets 表灌入；测试可覆盖。
    upstreams: Mutex<HashMap<String, UpstreamPair>>,
    /// preset_id → 稳定的 opencode 会话 id（`ses_` 形态，进程内固定）。
    session_ids: Mutex<HashMap<String, String>>,
}

static STATE: OnceLock<Result<NormalizerState, String>> = OnceLock::new();

fn state() -> &'static Result<NormalizerState, String> {
    STATE.get_or_init(start_server)
}

/// 惰性确保代理在跑，返回 agent 侧该用的 base_url（不含尾斜杠）。
/// claude 与 grok 共用：协议差异由路径区分（见模块头）。
pub fn channel_base_url(preset_id: &str) -> Result<String, String> {
    let state = match state() {
        Ok(state) => state,
        Err(error) => return Err(error.clone()),
    };
    Ok(format!("http://127.0.0.1:{}/p/{}", state.port, preset_id))
}

/// 测试入口：注册一个不在 presets 表里的假上游（两个协议同 base）。
#[cfg(test)]
pub(crate) fn register_upstream_for_test(preset_id: &str, base: &str) {
    let state = match state() {
        Ok(state) => state,
        Err(error) => panic!("grok 方言规整器未启动: {error}"),
    };
    state
        .upstreams
        .lock()
        .unwrap()
        .insert(
            preset_id.to_string(),
            UpstreamPair {
                chat: base.to_string(),
                anthropic: Some(base.to_string()),
            },
        );
}

fn start_server() -> Result<NormalizerState, String> {
    let server = Server::http(("127.0.0.1", PREFERRED_PORT))
        .or_else(|_| Server::http(("127.0.0.1", 0)))
        .map_err(|e| format!("方言规整器监听失败: {e}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|addr| addr.port())
        .unwrap_or(PREFERRED_PORT);
    let mut upstreams = HashMap::new();
    for preset in crate::model_bridge::presets() {
        if let Some(chat) = preset.chat_base {
            upstreams.insert(
                preset.id.to_string(),
                UpstreamPair {
                    chat: chat.to_string(),
                    anthropic: preset.anthropic_base.map(str::to_string),
                },
            );
        }
    }
    let upstreams = Mutex::new(upstreams);
    let session_ids = Mutex::new(HashMap::new());
    let server = Arc::new(server);
    let worker = Arc::clone(&server);
    std::thread::Builder::new()
        .name("dialect-normalizer".into())
        .spawn(move || {
            for request in worker.incoming_requests() {
                handle(request);
            }
        })
        .map_err(|e| format!("启动规整器线程失败: {e}"))?;
    Ok(NormalizerState {
        port,
        upstreams,
        session_ids,
    })
}

/// `/p/<preset>/<rest…>` → (preset, rest)。rest 至少为空串（`/p/x/` 的 `/` 已剥）。
fn split_proxy_path(path: &str) -> Option<(String, String)> {
    let rest = path.strip_prefix("/p/")?;
    let (preset, tail) = rest.split_once('/')?;
    Some((preset.to_string(), tail.to_string()))
}

/// 按路径形状选上游：Anthropic Messages 形状走根形态 base，其余走 chat base。
fn upstream_for(pair: &UpstreamPair, rest: &str) -> Option<String> {
    if rest.starts_with("v1/messages") || rest.starts_with("v1/count_tokens") {
        pair.anthropic.clone()
    } else {
        Some(pair.chat.clone())
    }
}

/// 转发一个请求。
fn handle(mut request: tiny_http::Request) {
    let path = request.url().to_string();
    let Some((preset_id, rest)) = split_proxy_path(&path) else {
        let _ = request.respond(Response::from_string("not a proxy path").with_status_code(404));
        return;
    };
    let pair = match state() {
        Ok(state) => state
            .upstreams
            .lock()
            .ok()
            .and_then(|map| map.get(&preset_id).cloned()),
        Err(_) => None,
    };
    let upstream_base = pair.as_ref().and_then(|p| upstream_for(p, &rest));
    let Some(upstream_base) = upstream_base else {
        let _ = request.respond(
            Response::from_string(format!("unknown preset `{preset_id}`")).with_status_code(404),
        );
        return;
    };
    let url = format!("{}/{}", upstream_base.trim_end_matches('/'), rest);

    // 只转发这几个头：凭证原样透传，其余（Host / 长度 / agent 的 CLI 伪装头）
    // 不带去第三方端点；opencode-go 目标再叠加它的会话头（见下）。
    let mut authorization = None;
    let mut content_type = None;
    let mut accept = None;
    for header in request.headers() {
        let name = header.field.as_str().as_str().to_ascii_lowercase();
        let value = header.value.as_str().to_string();
        match name.as_str() {
            "authorization" => authorization = Some(value),
            "content-type" => content_type = Some(value),
            "accept" => accept = Some(value),
            _ => {}
        }
    }

    let method = request.method().clone();
    match method {
        Method::Post => {
            let mut body = Vec::new();
            if let Err(e) = request.as_reader().read_to_end(&mut body) {
                debug_log(&format!("读请求体失败: {e}"));
                return;
            }
            let body = adapt_body(&preset_id, &rest, body);
            let mut call = agent()
                .post(&url)
                .set("Content-Type", content_type.as_deref().unwrap_or("application/json"));
            if let Some(value) = accept.as_deref() {
                call = call.set("Accept", value);
            }
            if let Some(value) = authorization.as_deref() {
                call = call.set("Authorization", value);
            }
            for (name, value) in extra_headers_for(&preset_id) {
                call = call.set(&name, &value);
            }
            forward_response(request, call.send_bytes(&body));
        }
        Method::Get => {
            let mut call = agent().get(&url);
            if let Some(value) = accept.as_deref() {
                call = call.set("Accept", value);
            }
            if let Some(value) = authorization.as_deref() {
                call = call.set("Authorization", value);
            }
            for (name, value) in extra_headers_for(&preset_id) {
                call = call.set(&name, &value);
            }
            forward_response(request, call.call());
        }
        _ => {
            let _ = request.respond(
                Response::from_string("method not forwarded").with_status_code(405),
            );
        }
    }
}

/// 已知 body 方言适配（按 preset 圈定；解析失败或无可改 → 原字节透传）。
///
/// - 全部 chat 路径：剥 `messages[*].model_id`（grok 方言）。
/// - deepseek：`max_completion_tokens` 改名 `max_tokens`（该端点没有新字段名
///   且**静默忽略**——不报错但输出被悄悄限到默认 8K，magpie #2355）。
/// - kimi：剥顶层 `temperature` / `top_p`（k2.5+ 服务端锁死采样参数，客户端
///   自带任何值都 400 "only 1 is allowed"，magpie #1197）。
fn adapt_body(preset_id: &str, rest: &str, body: Vec<u8>) -> Vec<u8> {
    let is_anthropic = rest.starts_with("v1/messages") || rest.starts_with("v1/count_tokens");
    if is_anthropic {
        // Anthropic 形状暂无已知 body 方言；原字节透传。
        return body;
    }
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return body;
    };
    let mut changed = false;
    let object = match value.as_object_mut() {
        Some(object) => object,
        None => return body,
    };
    // grok 的 model_id 标注。
    if let Some(messages) = object.get_mut("messages").and_then(|m| m.as_array_mut()) {
        for message in messages.iter_mut() {
            if let Some(entry) = message.as_object_mut() {
                changed |= entry.remove("model_id").is_some();
            }
        }
    }
    if preset_id == "deepseek" {
        if let Some(max) = object.remove("max_completion_tokens") {
            object.insert("max_tokens".to_string(), max);
            changed = true;
        }
    }
    if preset_id == "kimi" {
        changed |= object.remove("temperature").is_some();
        changed |= object.remove("top_p").is_some();
    }
    if !changed {
        return body;
    }
    serde_json::to_vec(&value).unwrap_or(body)
}

/// 按目标 preset 叠加的请求头。
///
/// OpenCode 网关（zen 与 go 同一路由）2026-09 起要求 OpenCode 客户端会话头，
/// 缺 `x-opencode-session` 直接 400 "cannot be routed efficiently"；Zen 免费
/// 层还拒 UA < 1.18.0、并对带 `x-session-affinity` 的请求 403。会话 id 在
/// 进程内按 preset 固定（`ses_` + 12 hex + 14 base62 的形态是网关校验的，
/// 形状不对免费层 403；magpie opencode_client.go 的实测形状对照）。
fn extra_headers_for(preset_id: &str) -> Vec<(String, String)> {
    if preset_id != "opencode-go" {
        return Vec::new();
    }
    let session_id = match state() {
        Ok(state) => {
            let mut ids = state.session_ids.lock().unwrap();
            let id = ids
                .entry(preset_id.to_string())
                .or_insert_with(|| generate_opencode_session_id());
            id.clone()
        }
        Err(_) => generate_opencode_session_id(),
    };
    vec![
        (
            "User-Agent".to_string(),
            "opencode/1.19.0 ai-sdk/provider-utils/4.0.23 runtime/bun/1.3.14".to_string(),
        ),
        ("x-opencode-session".to_string(), session_id.clone()),
        ("x-opencode-session-id".to_string(), session_id.clone()),
        (
            "x-opencode-request".to_string(),
            format!("msg_{}", &session_id[4..14]),
        ),
        ("x-opencode-client".to_string(), "cli".to_string()),
        ("x-opencode-project".to_string(), "global".to_string()),
    ]
}

/// `ses_` + 12 位小写 hex + 14 位 base62（形态对齐 opencode 客户端；无 rand
/// 依赖，用时钟 + 计数器混合）。
fn generate_opencode_session_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let mixed = (nanos as u64) ^ (count.wrapping_mul(0x9E3779B97F4A7C15));
    let hex: String = (0..6)
        .map(|i| format!("{:02x}", (mixed >> (i * 8)) as u8))
        .collect();
    const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let tail: String = (0..7)
        .map(|i| {
            let byte = (mixed >> (i * 4)) as u8 ^ (nanos >> (i * 11)) as u8;
            BASE62[(byte as usize) % BASE62.len()] as char
        })
        .collect();
    format!("ses_{hex}{tail}{tail}")
}

/// 把 ureq 的应答（含非 2xx）流转回 agent。流式（SSE）靠 chunked 直通。
fn forward_response(request: tiny_http::Request, result: Result<ureq::Response, ureq::Error>) {
    let response = match result {
        Ok(response) => response,
        Err(ureq::Error::Status(_, response)) => response,
        Err(error) => {
            let _ = request.respond(
                Response::from_string(format!("upstream: {error}")).with_status_code(502),
            );
            return;
        }
    };
    let status = response.status();
    let content_type = response
        .header("content-type")
        .unwrap_or("application/octet-stream")
        .to_string();
    let content_length = response
        .header("content-length")
        .and_then(|value| value.parse::<usize>().ok());
    let reader = response.into_reader();
    let mut forwarded = Response::empty(status).with_data(reader, content_length);
    if let Ok(header) = Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()) {
        forwarded = forwarded.with_header(header);
    }
    let _ = request.respond(forwarded);
}

/// 上游客户端：native-tls（与 http_client 同款）+ 尊重用户配置的代理出口。
/// 读超时放宽——SSE 长流期间两次读之间可能安静很久。
fn agent() -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(30))
        .timeout_read(std::time::Duration::from_secs(300));
    if let Ok(connector) = ureq::native_tls::TlsConnector::new() {
        builder = builder.tls_connector(Arc::new(connector));
    }
    if let Some((proxy_url, _no_proxy)) = crate::proxy::proxy_env() {
        if let Ok(proxy) = ureq::Proxy::new(&proxy_url) {
            builder = builder.proxy(proxy);
        }
    }
    builder.build()
}

fn debug_log(message: &str) {
    crate::debug_log::append("dialect", "warn", "", message, None);
}

// ─── 测试 ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    #[test]
    fn strips_model_id_from_assistant_messages_only() {
        let body = br#"{"model":"m","messages":[{"role":"system","content":"s"},{"role":"assistant","content":"a","model_id":"m"},{"role":"user","content":"u"}],"stream":true}"#;
        let out = adapt_body("deepseek", "chat/completions", body.to_vec());
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert!(parsed["messages"][1].get("model_id").is_none());
        assert_eq!(parsed["messages"][1]["content"], "a");
    }

    #[test]
    fn deepseek_renames_max_completion_tokens() {
        let body = br#"{"model":"deepseek-v4-pro","max_completion_tokens":4096,"messages":[{"role":"user","content":"u"}]}"#;
        let out = adapt_body("deepseek", "chat/completions", body.to_vec());
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(parsed["max_tokens"], 4096);
        assert!(parsed.get("max_completion_tokens").is_none());
        // 其他 preset 不改名。
        let out2 = adapt_body("xai", "chat/completions", body.to_vec());
        assert_eq!(out2, body.to_vec());
    }

    #[test]
    fn kimi_strips_locked_sampling_params() {
        let body = br#"{"model":"kimi-k3","temperature":0.1,"top_p":0.9,"messages":[{"role":"user","content":"u"}]}"#;
        let out = adapt_body("kimi", "chat/completions", body.to_vec());
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert!(parsed.get("temperature").is_none());
        assert!(parsed.get("top_p").is_none());
        // 其他 preset 原样。
        let out2 = adapt_body("deepseek", "chat/completions", body.to_vec());
        assert_eq!(out2, body.to_vec());
    }

    #[test]
    fn anthropic_path_passes_body_through() {
        let body = br#"{"model":"m","system":"s","messages":[{"role":"user","content":"u"}]}"#;
        let out = adapt_body("opencode-go", "v1/messages", body.to_vec());
        assert_eq!(out, body.to_vec());
    }

    #[test]
    fn passthrough_when_nothing_to_strip_or_not_json() {
        let body = br#"{"model":"m","messages":[{"role":"user","content":"u"}]}"#;
        let out = adapt_body("deepseek", "chat/completions", body.to_vec());
        assert_eq!(out, body.to_vec());
        let raw = b"not json at all".to_vec();
        assert_eq!(adapt_body("deepseek", "chat/completions", raw.clone()), raw);
    }

    #[test]
    fn proxy_path_splits_preset_and_rest() {
        assert_eq!(
            split_proxy_path("/p/deepseek/chat/completions"),
            Some(("deepseek".to_string(), "chat/completions".to_string()))
        );
        assert_eq!(split_proxy_path("/v1/chat/completions"), None);
        assert_eq!(split_proxy_path("/p/only"), None);
    }

    #[test]
    fn upstream_chooses_base_by_protocol_shape() {
        let pair = UpstreamPair {
            chat: "https://x.example/v1".to_string(),
            anthropic: Some("https://x.example".to_string()),
        };
        assert_eq!(
            upstream_for(&pair, "v1/messages"),
            Some("https://x.example".to_string())
        );
        assert_eq!(
            upstream_for(&pair, "v1/count_tokens"),
            Some("https://x.example".to_string())
        );
        assert_eq!(
            upstream_for(&pair, "chat/completions"),
            Some("https://x.example/v1".to_string())
        );
        let chat_only = UpstreamPair {
            chat: "https://y.example/v1".to_string(),
            anthropic: None,
        };
        assert_eq!(upstream_for(&chat_only, "v1/messages"), None);
    }

    #[test]
    fn opencode_session_id_has_valid_shape() {
        let id = generate_opencode_session_id();
        assert!(id.starts_with("ses_"), "id = {id}");
        assert_eq!(id.len(), 4 + 12 + 14, "id = {id}");
        assert!(id[4..16].chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn extra_headers_only_for_opencode_go() {
        assert!(extra_headers_for("deepseek").is_empty());
        let headers = extra_headers_for("opencode-go");
        assert!(headers
            .iter()
            .any(|(name, _)| name == "x-opencode-session"));
    }

    /// 端到端：假上游断言收到的 body 无 model_id、头带会话信息；应答字节直通。
    #[test]
    fn end_to_end_forwards_strips_and_adds_headers() {
        let upstream = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let upstream_port = upstream.local_addr().unwrap().port();
        register_upstream_for_test("e2e-preset", &format!("http://127.0.0.1:{upstream_port}/v1"));

        let received = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&received);
        std::thread::spawn(move || {
            let (mut stream, _) = upstream.accept().unwrap();
            let mut buf = Vec::new();
            let mut byte = [0u8; 1];
            let mut header_end = 0;
            loop {
                stream.read(&mut byte).unwrap();
                buf.push(byte[0]);
                if buf.ends_with(b"\r\n\r\n") && header_end == 0 {
                    header_end = buf.len();
                }
                if header_end > 0 {
                    let headers = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
                    if let Some(len) = headers
                        .split("\r\n")
                        .find_map(|l| l.strip_prefix("content-length: "))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                    {
                        if buf.len() >= header_end + len {
                            break;
                        }
                    }
                }
            }
            *captured.lock().unwrap() = buf;
            let body = b"data: {\"ok\":true}";
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(body).unwrap();
        });

        let base = channel_base_url("e2e-preset").unwrap();
        let client = ureq::AgentBuilder::new().build();
        let body = r#"{"model":"m","messages":[{"role":"assistant","content":"a","model_id":"m"}]}"#;
        let response = client
            .post(&format!("{base}/chat/completions"))
            .set("Authorization", "Bearer sk-marionette-spike")
            .send_bytes(body.as_bytes())
            .unwrap();
        assert_eq!(response.status(), 200);
        let mut text = String::new();
        response.into_reader().read_to_string(&mut text).unwrap();
        assert!(text.contains("{\"ok\":true}"));

        let raw = String::from_utf8(received.lock().unwrap().clone()).unwrap();
        assert!(raw.contains("POST /v1/chat/completions"), "上游应收到拼好的路径: {raw}");
        assert!(raw.contains("authorization: Bearer sk-marionette-spike") || raw.contains("Authorization: Bearer sk-marionette-spike"));
        assert!(!raw.contains("model_id"), "方言字段必须被剥掉: {raw}");
    }

    /// 端到端（anthropic 形状路径）：v1/messages 应路由到 anthropic 上游。
    #[test]
    fn end_to_end_anthropic_path_routes_to_root_base() {
        let upstream = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let upstream_port = upstream.local_addr().unwrap().port();
        // 注册成「chat 在 /v1、anthropic 在根」的两形态，验证路由选择。
        let state = match state() {
            Ok(state) => state,
            Err(error) => panic!("规整器未启动: {error}"),
        };
        state
            .upstreams
            .lock()
            .unwrap()
            .insert(
                "e2e-anth".to_string(),
                UpstreamPair {
                    chat: format!("http://127.0.0.1:{upstream_port}/v1"),
                    anthropic: Some(format!("http://127.0.0.1:{upstream_port}/anthroot")),
                },
            );
        let received = Arc::new(Mutex::new(String::new()));
        let captured = Arc::clone(&received);
        std::thread::spawn(move || {
            let (mut stream, _) = upstream.accept().unwrap();
            // 先把请求读完再应答，否则 ureq 写体时撞上过早关闭的连接。
            let mut buf = Vec::new();
            let mut byte = [0u8; 1];
            let mut header_end = 0;
            loop {
                stream.read(&mut byte).unwrap();
                buf.push(byte[0]);
                if buf.ends_with(b"\r\n\r\n") && header_end == 0 {
                    header_end = buf.len();
                }
                if header_end > 0 {
                    let headers = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
                    if let Some(len) = headers
                        .split("\r\n")
                        .find_map(|l| l.strip_prefix("content-length: "))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                    {
                        if buf.len() >= header_end + len {
                            break;
                        }
                    }
                }
            }
            *captured.lock().unwrap() = String::from_utf8_lossy(&buf).to_string();
            let body = "{}";
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            stream.write_all(head.as_bytes()).unwrap();
            stream.write_all(body.as_bytes()).unwrap();
        });

        let base = channel_base_url("e2e-anth").unwrap();
        let client = ureq::AgentBuilder::new().build();
        let response = client
            .post(&format!("{base}/v1/messages"))
            .set("Authorization", "Bearer sk-marionette-spike")
            .send_bytes(br#"{"model":"m","messages":[]}"#)
            .unwrap();
        assert_eq!(response.status(), 200);
        let raw = received.lock().unwrap().clone();
        assert!(raw.contains("/anthroot/v1/messages"), "应路由到 anthropic 根形态: {raw}");
    }
}
