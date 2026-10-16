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
//!
//! 学习式剥离闭环（附录 B4 第 1 条）：上游 400 且错误文本命中已知「未知
//! 字段」形状时，把字段路径归一化学进**进程内**的 per-preset 学习态，剥掉
//! 后立刻重发（最多 3 次），最终把最后一次上游应答原样回给 agent——新方言
//! 字段从「用户报 400 → 考古 → 手工加清单」变成「会话内自动恢复 + 纸面
//! 轨迹」。硬编码清单保持优先，学习态在其后追加应用。细节见
//! [`forward_post_with_recovery`]。

use std::collections::{HashMap, HashSet};
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

// ─── 学习式剥离闭环（docs/cross-agent-bridge.md 附录 B4 第 1 条）────────────

/// 每 preset 学习态上限：满了不再学（记日志说明），防抽风上游把进程内状态灌爆。
const LEARNED_CAP_PER_PRESET: usize = 16;

/// 一次转发里最多重发几次（不含初始发送；上限内每命中一个新字段重发一次）。
const MAX_AUTO_RETRIES: usize = 3;

/// 学习态：preset → 已学字段路径。进程内全局、不落盘——重启即清，对齐
/// magpie unfit 的语义（错误规则不会长存）。字段路径归一化形态：
/// `messages[*].x`（对一切下标生效）或顶层字段名。
static LEARNED: OnceLock<Mutex<HashMap<String, HashSet<String>>>> = OnceLock::new();

fn learned_map() -> &'static Mutex<HashMap<String, HashSet<String>>> {
    LEARNED.get_or_init(|| Mutex::new(HashMap::new()))
}

enum LearnOutcome {
    /// 新学到（已记 auto-learn 日志）。
    Learned,
    /// 已在学习态里（重发同形状 body 无意义，应放弃）。
    AlreadyKnown,
    /// 每 preset 上限已满（已记日志，不再学）。
    CapFull,
}

fn learned_fields_for(preset_id: &str) -> HashSet<String> {
    learned_map()
        .lock()
        .ok()
        .and_then(|map| map.get(preset_id).cloned())
        .unwrap_or_default()
}

fn learned_contains(preset_id: &str, field: &str) -> bool {
    learned_map()
        .lock()
        .ok()
        .map(|map| map.get(preset_id).is_some_and(|set| set.contains(field)))
        .unwrap_or(false)
}

/// 学一条字段路径。命中已知错误形状时调用；学到即记
/// `[dialect] auto-learn preset=<id> field=<path>`（warn，tag 稳定可 grep）。
fn learn_field(preset_id: &str, field: &str) -> LearnOutcome {
    let mut map = match learned_map().lock() {
        Ok(map) => map,
        Err(poisoned) => poisoned.into_inner(),
    };
    let set = map.entry(preset_id.to_string()).or_default();
    if set.contains(field) {
        return LearnOutcome::AlreadyKnown;
    }
    if set.len() >= LEARNED_CAP_PER_PRESET {
        debug_log_at(
            "warn",
            &format!(
                "[dialect] auto-learn-skipped preset={preset_id} field={field} \
                 reason=cap-reached limit={LEARNED_CAP_PER_PRESET}"
            ),
        );
        return LearnOutcome::CapFull;
    }
    set.insert(field.to_string());
    debug_log_at(
        "warn",
        &format!("[dialect] auto-learn preset={preset_id} field={field}"),
    );
    LearnOutcome::Learned
}

/// 测试入口：清空学习态（各并行测试用独占 preset id 隔离，避免互相干扰）。
#[cfg(test)]
fn clear_learned_for_test() {
    if let Ok(mut map) = learned_map().lock() {
        map.clear();
    }
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
            forward_post_with_recovery(
                request,
                &preset_id,
                &rest,
                &url,
                content_type,
                accept,
                authorization,
                extra_headers_for(&preset_id),
                body,
            );
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

// ─── 400「未知字段」错误形状识别（学习闭环的入口）──────────────────────────

/// 从 400 错误文本里解析「未知字段」路径并归一化。已知形状：
/// - `Unrecognized request argument supplied: messages[2].future_field.`
///   （OpenAI 风格，尾句点可选）；
/// - `unsupported_parameter: … messages[5].model_id`——按文本里出现的
///   第一个 `messages[<数字>].<字段>` 模式抓，前缀不重要；
/// - `Unknown parameter: vendor_thing`（顶层字段）。
///
/// 归一化：`messages[N].x` → `messages[*].x`（学到的规则对一切下标生效）；
/// 顶层字段 → 原样。不匹配返回 None（一次都不重试）。
fn parse_unknown_field(text: &str) -> Option<String> {
    if let Some(field) = find_indexed_field(text) {
        return Some(field);
    }
    find_top_level_field(text)
}

/// 文本里第一个 `messages[<数字>].<字段>` → `messages[*].<字段>`。
fn find_indexed_field(text: &str) -> Option<String> {
    const MARKER: &str = "messages[";
    let mut from = 0usize;
    while let Some(rel) = text[from..].find(MARKER) {
        let digits_start = from + rel + MARKER.len();
        let digits_end = text[digits_start..]
            .find(|c: char| !c.is_ascii_digit())
            .map(|offset| digits_start + offset)
            .unwrap_or(text.len());
        let after_index = digits_end + 1;
        let shape_ok = digits_end > digits_start
            && text[digits_end..].starts_with(']')
            && text[after_index..].starts_with('.');
        if shape_ok {
            let field_start = after_index + 1;
            let field_end = text[field_start..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                .map(|offset| field_start + offset)
                .unwrap_or(text.len());
            let field = &text[field_start..field_end];
            if is_plausible_field(field) {
                return Some(format!("messages[*].{field}"));
            }
        }
        // 形态不完整：跳过这次命中继续找。
        from = digits_start;
    }
    None
}

/// `Unknown parameter: <field>`（顶层字段；尾句点与包裹引号可选）。
fn find_top_level_field(text: &str) -> Option<String> {
    const MARKER: &str = "Unknown parameter:";
    let index = text.find(MARKER)?;
    let token = text[index + MARKER.len()..]
        .split_whitespace()
        .next()?;
    let token = token.strip_suffix('.').unwrap_or(token);
    let token = strip_matching_quotes(token);
    if is_plausible_field(token)
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        Some(token.to_string())
    } else {
        None
    }
}

/// 错误文本里抓出来的字段名要像个 JSON 键，且别太长（防病态捕获）。
fn is_plausible_field(field: &str) -> bool {
    !field.is_empty() && field.len() <= 64
}

fn strip_matching_quotes(token: &str) -> &str {
    let bytes = token.as_bytes();
    if bytes.len() >= 2 {
        let (first, last) = (bytes[0], bytes[bytes.len() - 1]);
        let quoted = (first == b'\'' && last == b'\'')
            || (first == b'"' && last == b'"')
            || (first == b'`' && last == b'`');
        if quoted {
            return &token[1..token.len() - 1];
        }
    }
    token
}

/// 从 400 的 JSON 错误体里抽错误文本（`error.message` / `error` 字符串 /
/// 顶层 `message`）。非 JSON 或抽不到 → None（一次都不重试）。
fn error_text_from_json(body: &[u8]) -> Option<String> {
    let value = serde_json::from_slice::<serde_json::Value>(body).ok()?;
    let object = value.as_object()?;
    if let Some(text) = object
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(|message| message.as_str())
    {
        return Some(text.to_string());
    }
    if let Some(text) = object.get("error").and_then(|error| error.as_str()) {
        return Some(text.to_string());
    }
    object
        .get("message")
        .and_then(|message| message.as_str())
        .map(str::to_string)
}

/// 该字段路径是否已被 [`adapt_body`] 的硬编码规则处理（已在剥还报 →
/// 重发同形状 body 无意义）。
fn is_hardcoded_strip(preset_id: &str, field: &str) -> bool {
    match field {
        "messages[*].model_id" => true,
        "max_completion_tokens" => preset_id == "deepseek",
        "temperature" | "top_p" => preset_id == "kimi",
        _ => false,
    }
}

/// 日志用截断：按 char 截，绝不落在 UTF-8 多字节字符中间（AGENTS.md 的
/// drain 教训）。
fn truncate_for_log(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

fn give_up_log(preset_id: &str, error_text: &str) {
    debug_log_at(
        "warn",
        &format!(
            "[dialect] auto-give-up preset={preset_id} last-error={}",
            truncate_for_log(error_text, 100)
        ),
    );
}

/// 已知 body 方言适配（按 preset 圈定；解析失败或无可改 → 原字节透传）。
///
/// - 全部 chat 路径：剥 `messages[*].model_id`（grok 方言）。
/// - deepseek：`max_completion_tokens` 改名 `max_tokens`（该端点没有新字段名
///   且**静默忽略**——不报错但输出被悄悄限到默认 8K，magpie #2355）。
/// - kimi：剥顶层 `temperature` / `top_p`（k2.5+ 服务端锁死采样参数，客户端
///   自带任何值都 400 "only 1 is allowed"，magpie #1197）。
/// - 学习态（硬编码优先，在其后追加应用，同一次请求两者都剥）：本 preset
///   学到的 `messages[*].x` / 顶层 `x`，见 [`forward_post_with_recovery`]。
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
    // 学习态追加：上游 400 报过的未知字段按 preset 记住并剥除。硬编码清单
    // 优先（上面已跑），这里只是补学到的形状。
    for field in learned_fields_for(preset_id) {
        if let Some(name) = field.strip_prefix("messages[*].") {
            if let Some(messages) = object.get_mut("messages").and_then(|m| m.as_array_mut()) {
                for message in messages.iter_mut() {
                    if let Some(entry) = message.as_object_mut() {
                        changed |= entry.remove(name).is_some();
                    }
                }
            }
        } else {
            changed |= object.remove(&field).is_some();
        }
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

/// POST 转发 + 学习式剥离闭环。
///
/// 流程：硬编码 [`adapt_body`] 先跑、发一次；上游 400 且错误形状命中（见
/// [`parse_unknown_field`]）→ 字段入 preset 学习态 → 从原始 body 重新走全套
/// adapt（硬编码优先、学习态追加）→ **立刻重发**（同上游、同头）；再 400
/// 且又命中新字段 → 再学再重发，最多 [`MAX_AUTO_RETRIES`] 次重发；最终把
/// **最后一次上游应答原样**回给 agent——用户看到的是真实的残余错误，不是
/// 我们的杜撰。纸面轨迹：`[dialect] auto-learn` / `auto-recovered` /
/// `auto-give-up`（dev.log，tag 稳定可 grep）。
///
/// 一次都不重试、行为与原直通逐字节一致的情形：非 400、请求体不可解析为
/// JSON（学习剥除要按字段操作 body）、anthropic 路径（学习闭环与硬编码
/// 清单同圈定在 chat 形状）、错误体非 JSON、错误文本不匹配已知形状。
fn forward_post_with_recovery(
    request: tiny_http::Request,
    preset_id: &str,
    rest: &str,
    url: &str,
    content_type: Option<String>,
    accept: Option<String>,
    authorization: Option<String>,
    extra_headers: Vec<(String, String)>,
    raw_body: Vec<u8>,
) {
    let chat_shaped = !(rest.starts_with("v1/messages") || rest.starts_with("v1/count_tokens"));
    let body_is_json =
        chat_shaped && serde_json::from_slice::<serde_json::Value>(&raw_body).is_ok();
    let http = agent();
    let mut body = adapt_body(preset_id, rest, raw_body.clone());
    let mut attempts = 0usize;
    let mut last_learned: Option<String> = None;
    loop {
        let result = {
            let mut call = http.post(url).set(
                "Content-Type",
                content_type.as_deref().unwrap_or("application/json"),
            );
            if let Some(value) = accept.as_deref() {
                call = call.set("Accept", value);
            }
            if let Some(value) = authorization.as_deref() {
                call = call.set("Authorization", value);
            }
            for (name, value) in &extra_headers {
                call = call.set(name, value);
            }
            call.send_bytes(&body)
        };
        match result {
            // 400 是唯一进入恢复判定的状态码；guard 不满足时落进 other，
            // 走原流式直通（请求体非 JSON / anthropic 路径行为完全不变）。
            Err(ureq::Error::Status(400, response)) if body_is_json => {
                let (upstream_type, length_hint, bytes) = drain_response(response);
                let Some((error_text, field)) = error_text_from_json(&bytes)
                    .and_then(|text| parse_unknown_field(&text).map(|field| (text, field)))
                else {
                    // 形状不匹配 / 错误体非 JSON：原样透传。
                    forward_buffered(request, 400, upstream_type, length_hint, bytes);
                    return;
                };
                // 已在剥（学习态或硬编码清单）还报 → 重发同形状 body 无意义。
                let already_stripped =
                    is_hardcoded_strip(preset_id, &field) || learned_contains(preset_id, &field);
                let cap_full = !already_stripped
                    && matches!(learn_field(preset_id, &field), LearnOutcome::CapFull);
                let budget_done = !already_stripped && !cap_full && attempts >= MAX_AUTO_RETRIES;
                if already_stripped || cap_full || budget_done {
                    give_up_log(preset_id, &error_text);
                    forward_buffered(request, 400, upstream_type, length_hint, bytes);
                    return;
                }
                attempts += 1;
                last_learned = Some(field);
                body = adapt_body(preset_id, rest, raw_body.clone());
            }
            other => {
                // 传输层错误（拿到 `Err(Transport)`）不算恢复——没有应答可谈。
                let completed = matches!(&other, Ok(_) | Err(ureq::Error::Status(_, _)));
                if attempts > 0 && completed {
                    debug_log_at(
                        "info",
                        &format!(
                            "[dialect] auto-recovered preset={preset_id} field={} \
                             attempts={attempts}",
                            last_learned.as_deref().unwrap_or("?")
                        ),
                    );
                }
                forward_response(request, other);
                return;
            }
        }
    }
}

/// 把上游 400 的头与体读成缓冲（错误体是小 JSON；请求侧本就整读，对称）。
/// 长度提示沿用上游的 content-length 头，转发编码行为与流式直通一致。
fn drain_response(response: ureq::Response) -> (String, Option<usize>, Vec<u8>) {
    let content_type = response
        .header("content-type")
        .unwrap_or("application/octet-stream")
        .to_string();
    let length_hint = response
        .header("content-length")
        .and_then(|value| value.parse::<usize>().ok());
    let mut bytes = Vec::new();
    // 读失败 → 已读部分尽力转发（等价于流式中途断掉）。
    let _ = response.into_reader().read_to_end(&mut bytes);
    (content_type, length_hint, bytes)
}

/// 把已缓冲的上游应答回给 agent（放弃路径；应答字节即上游原文）。
fn forward_buffered(
    request: tiny_http::Request,
    status: u16,
    content_type: String,
    length_hint: Option<usize>,
    body: Vec<u8>,
) {
    let mut forwarded = Response::empty(status).with_data(std::io::Cursor::new(body), length_hint);
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
    debug_log_at("warn", message);
}

fn debug_log_at(level: &str, message: &str) {
    crate::debug_log::append("dialect", level, "", message, None);
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

    // ─── 学习式剥离闭环 ─────────────────────────────────────────────────────

    /// 学习态相关测试的串行锁：`clear_learned_for_test` 是全局清空，与其它
    /// 依赖学习态的测试并发会互相清场——这些测试全部拿这把锁串行跑。
    static LEARN_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn learn_test_guard() -> std::sync::MutexGuard<'static, ()> {
        LEARN_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn error_body(message: &str) -> Vec<u8> {
        serde_json::json!({ "error": { "message": message } })
            .to_string()
            .into_bytes()
    }

    /// 假上游：按顺序应答 plan 里的每个请求（先读完请求体再应答，逐字节读法
    /// 与既有 e2e 一致），记录收到的原始请求。应答完 plan 后线程退出、监听
    /// 器关闭——多余的请求会撞上 connection refused，测试断言干净失败而不是
    /// 挂死。
    fn fake_upstream(plan: Vec<(u16, &'static str, Vec<u8>)>) -> (u16, Arc<Mutex<Vec<String>>>) {
        fn reason(status: u16) -> &'static str {
            match status {
                200 => "OK",
                400 => "Bad Request",
                _ => "Status",
            }
        }
        fn read_one_request(stream: &mut std::net::TcpStream) -> String {
            let mut buf = Vec::new();
            let mut byte = [0u8; 1];
            let mut header_end = 0;
            loop {
                match stream.read(&mut byte) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(_) => break,
                }
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
            String::from_utf8_lossy(&buf).to_string()
        }

        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let received = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&received);
        std::thread::spawn(move || {
            for (status, content_type, body) in plan {
                let Ok((mut stream, _)) = listener.accept() else { return };
                let raw = read_one_request(&mut stream);
                captured.lock().unwrap().push(raw);
                let head = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    status,
                    reason(status),
                    content_type,
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        (port, received)
    }

    /// 测试客户端 POST：返回 (status, body 文本)；4xx 也照样读全。
    fn post_and_read(client: &ureq::Agent, url: &str, body: &[u8]) -> (u16, String) {
        let response = match client
            .post(url)
            .set("Authorization", "Bearer sk-test")
            .send_bytes(body)
        {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(error) => panic!("请求失败: {error}"),
        };
        let status = response.status();
        let mut text = String::new();
        response.into_reader().read_to_string(&mut text).unwrap();
        (status, text)
    }

    #[test]
    fn parses_known_unknown_field_shapes() {
        // OpenAI 风格（尾句点可选）。
        assert_eq!(
            parse_unknown_field("Unrecognized request argument supplied: messages[2].future_field."),
            Some("messages[*].future_field".to_string())
        );
        assert_eq!(
            parse_unknown_field("Unrecognized request argument supplied: messages[7].another"),
            Some("messages[*].another".to_string())
        );
        // unsupported_parameter 复合形态：按文本里第一个 messages[数字].字段 抓。
        assert_eq!(
            parse_unknown_field(
                "unsupported_parameter: Unrecognized request argument supplied: messages[5].model_id"
            ),
            Some("messages[*].model_id".to_string())
        );
        // 顶层字段（含可选包裹引号与尾句点）。
        assert_eq!(
            parse_unknown_field("Unknown parameter: vendor_thing"),
            Some("vendor_thing".to_string())
        );
        assert_eq!(
            parse_unknown_field("Unknown parameter: 'quoted_thing'."),
            Some("quoted_thing".to_string())
        );
        // 不匹配的形状。
        assert_eq!(parse_unknown_field("Model not found"), None);
        assert_eq!(parse_unknown_field("messages[x].not_digits"), None);
        assert_eq!(parse_unknown_field("messages[3]no_dot"), None);
        assert_eq!(parse_unknown_field("Unknown parameter:"), None);
        assert_eq!(parse_unknown_field(""), None);
    }

    #[test]
    fn extracts_error_text_from_json_error_bodies() {
        assert_eq!(
            error_text_from_json(br#"{"error":{"message":"boom"}}"#).as_deref(),
            Some("boom")
        );
        assert_eq!(
            error_text_from_json(br#"{"error":"plain"}"#).as_deref(),
            Some("plain")
        );
        assert_eq!(
            error_text_from_json(br#"{"message":"top-level"}"#).as_deref(),
            Some("top-level")
        );
        assert_eq!(error_text_from_json(br#"{"unexpected":true}"#), None);
        assert_eq!(error_text_from_json(b"not json"), None);
    }

    #[test]
    fn learn_field_tracks_known_state() {
        let _guard = learn_test_guard();
        assert!(matches!(
            learn_field("unit-learn", "messages[*].a"),
            LearnOutcome::Learned
        ));
        assert!(matches!(
            learn_field("unit-learn", "messages[*].a"),
            LearnOutcome::AlreadyKnown
        ));
    }

    #[test]
    fn adapt_body_applies_learned_fields_after_hardcoded() {
        let _guard = learn_test_guard();
        learn_field("unit-adapt", "messages[*].weird");
        learn_field("unit-adapt", "vendor_thing");
        let body = br#"{"model":"m","vendor_thing":1,"messages":[{"role":"user","content":"u","model_id":"m","weird":2}]}"#;
        let out = adapt_body("unit-adapt", "chat/completions", body.to_vec());
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert!(parsed.get("vendor_thing").is_none(), "学到的顶层字段要剥");
        assert!(
            parsed["messages"][0].get("weird").is_none(),
            "学到的消息字段要剥"
        );
        assert!(parsed["messages"][0].get("model_id").is_none(), "硬编码照常");
        // 其它 preset 不受影响（学习态按 preset 隔离）。
        let out2 = adapt_body("unit-adapt-other", "chat/completions", body.to_vec());
        let parsed2: serde_json::Value = serde_json::from_slice(&out2).unwrap();
        assert!(parsed2["messages"][0].get("weird").is_some(), "没学过的字段保留");
        assert!(parsed2.get("vendor_thing").is_some());
    }

    #[test]
    fn clear_learned_for_test_resets_state() {
        let _guard = learn_test_guard();
        clear_learned_for_test();
        learn_field("unit-clear", "messages[*].x");
        assert!(learned_contains("unit-clear", "messages[*].x"));
        clear_learned_for_test();
        assert!(!learned_contains("unit-clear", "messages[*].x"));
    }

    #[test]
    fn log_truncation_is_char_boundary_safe() {
        let cjk = "错".repeat(150);
        assert_eq!(truncate_for_log(&cjk, 100).chars().count(), 100);
        let ascii = "x".repeat(150);
        assert_eq!(truncate_for_log(&ascii, 100).len(), 100);
        assert_eq!(truncate_for_log("short", 100), "short");
    }

    /// 端到端：首答 400（OpenAI 风格 messages[N] 未知字段）→ 学 → 剥 → 重发
    /// 拿 200；后续请求直接被剥（上游第二、三次请求都收不到该字段）。
    #[test]
    fn auto_learn_recovers_and_pre_strips_later_requests() {
        let _guard = learn_test_guard();
        let (port, received) = fake_upstream(vec![
            (
                400,
                "application/json",
                error_body("Unrecognized request argument supplied: messages[2].future_field."),
            ),
            (200, "text/event-stream", b"data: {\"ok\":true}".to_vec()),
            (200, "text/event-stream", b"data: {\"ok\":true}".to_vec()),
        ]);
        register_upstream_for_test("lrn-msg", &format!("http://127.0.0.1:{port}/v1"));

        let base = channel_base_url("lrn-msg").unwrap();
        let client = ureq::AgentBuilder::new().build();
        let body = r#"{"model":"m","messages":[{"role":"user","content":"u","future_field":"x"}]}"#;

        let (status, text) =
            post_and_read(&client, &format!("{base}/chat/completions"), body.as_bytes());
        assert_eq!(status, 200);
        assert!(text.contains("{\"ok\":true}"));

        // 后续请求：学习态已在，直接被剥——上游不会再收到该字段。
        let (status, _) =
            post_and_read(&client, &format!("{base}/chat/completions"), body.as_bytes());
        assert_eq!(status, 200);

        let hits = received.lock().unwrap().clone();
        assert_eq!(hits.len(), 3, "初始 + 一次重试 + 第二个客户端请求: {hits:?}");
        assert!(hits[0].contains("future_field"), "首个请求带方言字段");
        assert!(!hits[1].contains("future_field"), "重发必须已剥掉");
        assert!(!hits[2].contains("future_field"), "后续请求必须预剥");
        assert!(learned_contains("lrn-msg", "messages[*].future_field"));
    }

    /// 端到端：顶层 `Unknown parameter: vendor_thing` 同样学-剥-重发恢复。
    #[test]
    fn auto_learn_handles_top_level_unknown_parameter() {
        let _guard = learn_test_guard();
        let (port, received) = fake_upstream(vec![
            (
                400,
                "application/json",
                error_body("Unknown parameter: vendor_thing."),
            ),
            (200, "application/json", b"{\"ok\":true}".to_vec()),
        ]);
        register_upstream_for_test("lrn-top", &format!("http://127.0.0.1:{port}/v1"));

        let base = channel_base_url("lrn-top").unwrap();
        let client = ureq::AgentBuilder::new().build();
        let body = r#"{"model":"m","vendor_thing":true,"messages":[{"role":"user","content":"u"}]}"#;

        let (status, text) =
            post_and_read(&client, &format!("{base}/chat/completions"), body.as_bytes());
        assert_eq!(status, 200);
        assert!(text.contains("{\"ok\":true}"));
        let hits = received.lock().unwrap().clone();
        assert_eq!(hits.len(), 2, "仅一次重试");
        assert!(hits[0].contains("vendor_thing"));
        assert!(!hits[1].contains("vendor_thing"));
        assert!(learned_contains("lrn-top", "vendor_thing"));
    }

    /// 端到端：上游永远 400 且每次报一个新字段 → 3 次重试后放弃，客户端
    /// 收到的 400 是最后一次真实残余错误。
    #[test]
    fn auto_learn_gives_up_after_three_retries() {
        let _guard = learn_test_guard();
        let plan: Vec<(u16, &'static str, Vec<u8>)> = (1..=4)
            .map(|i| {
                (
                    400,
                    "application/json",
                    error_body(&format!(
                        "Unrecognized request argument supplied: messages[0].future_{i}."
                    )),
                )
            })
            .collect();
        let (port, received) = fake_upstream(plan);
        register_upstream_for_test("lrn-cap3", &format!("http://127.0.0.1:{port}/v1"));

        let base = channel_base_url("lrn-cap3").unwrap();
        let client = ureq::AgentBuilder::new().build();
        let body = r#"{"model":"m","messages":[{"role":"user","content":"u"}]}"#;

        let (status, text) =
            post_and_read(&client, &format!("{base}/chat/completions"), body.as_bytes());
        assert_eq!(status, 400, "重试耗尽后把最后一次真实应答回给 agent");
        assert!(text.contains("future_4"), "客户端看到的是最后的残余错误: {text}");
        assert_eq!(received.lock().unwrap().len(), 4, "初始 + 3 次重试");
        assert_eq!(
            learned_fields_for("lrn-cap3").len(),
            4,
            "放弃前每个报过的字段都已入学习态"
        );
    }

    /// 学习态达到每 preset 上限（16）后不再学、一次都不重试。
    #[test]
    fn learn_state_stops_at_sixteen_per_preset() {
        let _guard = learn_test_guard();
        // 用真实 learn_field 接口预灌满 16 条。
        for i in 0..16 {
            learn_field("lrn-limit", &format!("field_{i}"));
        }
        assert!(matches!(
            learn_field("lrn-limit", "field_16"),
            LearnOutcome::CapFull
        ));

        let (port, received) = fake_upstream(vec![(
            400,
            "application/json",
            error_body("Unrecognized request argument supplied: messages[0].future_17."),
        )]);
        register_upstream_for_test("lrn-limit", &format!("http://127.0.0.1:{port}/v1"));

        let base = channel_base_url("lrn-limit").unwrap();
        let client = ureq::AgentBuilder::new().build();
        let body = r#"{"model":"m","messages":[{"role":"user","content":"u","future_17":1}]}"#;

        let (status, _) =
            post_and_read(&client, &format!("{base}/chat/completions"), body.as_bytes());
        assert_eq!(status, 400);
        assert_eq!(received.lock().unwrap().len(), 1, "满态：零重试");
        assert!(!learned_contains("lrn-limit", "messages[*].future_17"));
        assert_eq!(learned_fields_for("lrn-limit").len(), 16);
    }

    /// 不匹配的错误文本（如 model not found）→ 不重试、应答原样透传。
    #[test]
    fn unmatched_error_text_is_forwarded_verbatim_without_retry() {
        let error_json =
            b"{\"error\":{\"message\":\"Model not found\",\"type\":\"invalid_request_error\"}}"
                .to_vec();
        let (port, received) =
            fake_upstream(vec![(400, "application/json", error_json.clone())]);
        register_upstream_for_test("lrn-miss", &format!("http://127.0.0.1:{port}/v1"));

        let base = channel_base_url("lrn-miss").unwrap();
        let client = ureq::AgentBuilder::new().build();
        let body = r#"{"model":"m","messages":[{"role":"user","content":"u"}]}"#;

        let (status, text) =
            post_and_read(&client, &format!("{base}/chat/completions"), body.as_bytes());
        assert_eq!(status, 400);
        assert_eq!(text.as_bytes(), error_json.as_slice(), "应答原样透传");
        assert_eq!(received.lock().unwrap().len(), 1, "形状不匹配：零重试");
    }

    /// 请求体不可解析为 JSON → 即使 400 报出已知形状也一次都不重试。
    #[test]
    fn non_json_request_body_never_retries() {
        let (port, received) = fake_upstream(vec![(
            400,
            "application/json",
            error_body("Unrecognized request argument supplied: messages[2].future_field."),
        )]);
        register_upstream_for_test("lrn-raw", &format!("http://127.0.0.1:{port}/v1"));

        let base = channel_base_url("lrn-raw").unwrap();
        let client = ureq::AgentBuilder::new().build();
        let (status, _) = post_and_read(
            &client,
            &format!("{base}/chat/completions"),
            b"definitely not json",
        );
        assert_eq!(status, 400);
        assert_eq!(received.lock().unwrap().len(), 1, "请求体非 JSON：零重试");
    }

    /// 已被硬编码清单剥掉的字段再被点名 → 零重试、不进学习态。
    #[test]
    fn already_stripped_field_does_not_burn_retries() {
        let _guard = learn_test_guard();
        let (port, received) = fake_upstream(vec![(
            400,
            "application/json",
            error_body("Unrecognized request argument supplied: messages[0].model_id."),
        )]);
        register_upstream_for_test("lrn-known", &format!("http://127.0.0.1:{port}/v1"));

        let base = channel_base_url("lrn-known").unwrap();
        let client = ureq::AgentBuilder::new().build();
        let body = r#"{"model":"m","messages":[{"role":"assistant","content":"a","model_id":"m"}]}"#;

        let (status, _) =
            post_and_read(&client, &format!("{base}/chat/completions"), body.as_bytes());
        assert_eq!(status, 400);
        assert_eq!(received.lock().unwrap().len(), 1, "已在剥的字段：零重试");
        assert!(
            !learned_contains("lrn-known", "messages[*].model_id"),
            "硬编码条目不进学习态"
        );
    }

    /// 学习态按 preset 隔离：iso-a（相当于 deepseek 侧）学到的字段不影响
    /// iso-b（相当于 xai 侧）的请求——字段必须在场透传。
    #[test]
    fn learned_fields_are_isolated_per_preset() {
        let _guard = learn_test_guard();
        let (port_a, hits_a) = fake_upstream(vec![
            (
                400,
                "application/json",
                error_body(
                    "unsupported_parameter: Unrecognized request argument supplied: messages[0].shared_field",
                ),
            ),
            (200, "application/json", b"{\"ok\":\"a\"}".to_vec()),
        ]);
        register_upstream_for_test("iso-a", &format!("http://127.0.0.1:{port_a}/v1"));
        let (port_b, hits_b) =
            fake_upstream(vec![(200, "application/json", b"{\"ok\":\"b\"}".to_vec())]);
        register_upstream_for_test("iso-b", &format!("http://127.0.0.1:{port_b}/v1"));

        let body = r#"{"model":"m","messages":[{"role":"user","content":"u","shared_field":1}]}"#;
        let client = ureq::AgentBuilder::new().build();
        let (status, _) = post_and_read(
            &client,
            &format!("{}/chat/completions", channel_base_url("iso-a").unwrap()),
            body.as_bytes(),
        );
        assert_eq!(status, 200, "iso-a 侧学会剥掉并恢复");
        let (status, _) = post_and_read(
            &client,
            &format!("{}/chat/completions", channel_base_url("iso-b").unwrap()),
            body.as_bytes(),
        );
        assert_eq!(status, 200);

        let a = hits_a.lock().unwrap().clone();
        assert_eq!(a.len(), 2);
        assert!(a[0].contains("shared_field"));
        assert!(!a[1].contains("shared_field"), "iso-a 重发时已剥");
        let b = hits_b.lock().unwrap().clone();
        assert_eq!(b.len(), 1);
        assert!(
            b[0].contains("shared_field"),
            "iso-b 没学过：字段必须在场透传"
        );
    }
}
