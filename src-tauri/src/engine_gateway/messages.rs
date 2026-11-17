//! `/v1/messages` 入站（合同 C5 的 anthropic 形态，账本 T3）。
//!
//! Claude 系 agent（dsh 的 anthropic 后端等）打这个端点。两条出站路径，
//! **直通优先**：
//! - **直通**：provider 配了 anthropic URL → 请求体只把 `model` 换成厂商
//!   裸名（Go 网关 `rewriteModel(body, wire)` 同款——id 是 `provider/model`
//!   形态，不剥前缀厂商全 404），其余字节原样转发 `{anthropic}/v1/messages`；
//!   认证 `x-api-key` + `Authorization: Bearer` 双头 + `anthropic-version`
//!   （厂商兼容面广，双头都带）；应答与 SSE 原样透传，不做协议翻译。
//! - **翻译**：anthropic URL 空而 chat URL 有 → `translate::anthropic_to_chat`
//!   转 chat 请求，按 chat.rs 同款出站（剥前缀、Bearer、bodycap 的
//!   prefit / maybe_refit），非流式应答转回 anthropic message，流式应答把
//!   chat SSE 逐事件映射成 anthropic 帧。
//!
//! SSE 编排是**调用方**的职责（translate.rs 模块注释钉的分工）：`message_start`
//! 必须是第一帧——anthropic SDK 的 MessageStream 对它之前的事件直接抛错
//! （Go anthropicEncoder.keepalive 注释实锤）；无状态映射会把 finish 帧与
//! `[DONE]` 各收口一次 `message_stop`，这里在**首个** message_stop 后丢弃
//! 全部尾巴（include_usage 末帧、`[DONE]`），一条流只收口一次。
//!
//! 错误面：厂商错误一律转 anthropic 形状
//! `{"type":"error","error":{"type":"...","message":"..."}}`，状态码透传；
//! 直通路径上本来就是 anthropic 形状的厂商错误原样透传（厂商的话不改写）。

use std::io::{BufRead, BufReader, Read};

use serde_json::{json, Value};

use super::bodycap;
use super::chat;
use super::httpd::Reply;
use super::translate;

/// 直通路径的 anthropic 协议版本头（任务定案值；anthropic 官方与主流
/// 兼容面都认这一个）。
const ANTHROPIC_VERSION: &str = "2023-06-01";

// =====================================================================
// 路由：asked model → 出站目标
// =====================================================================

/// 出站目标（chat.rs resolve 的 anthropic 版）：直通基址与翻译基址分开
/// 存，由 handle 按优先级挑。anthropic/chat 字段是 providers.json 的原语
/// （含 /v1 与否按各家惯例：chat 含 /v1，anthropic 不含——端点路径自拼）。
struct Target {
    provider_id: String,
    key: String,
    headers: Vec<(String, String)>,
    /// 厂商裸模型名（`provider/model` 已剥前缀、已剥 `[1m]`/`[effort]` 标记）。
    bare_model: String,
    /// anthropic 基址：非空 → 直通 `{anthropic}/v1/messages`。
    anthropic: String,
    /// openai chat 基址（含 /v1）：anthropic 为空且它非空 → 翻译出站。
    chat: String,
}

fn resolve(gw: &super::Gateway, model: &str) -> Result<Target, String> {
    let store = gw.store.lock().map_err(|_| "store poisoned".to_string())?;
    // 与 chat.rs 同一条路由：组/别名/直通三形态 + peel 标记。
    let (provider_id, bare) = super::groups::resolve(store.groups(), store.providers(), model)
        .ok_or_else(|| format!("no provider for model: {model}"))?;
    let provider = store
        .find(&provider_id)
        .ok_or_else(|| format!("no provider for model: {model}"))?;
    Ok(Target {
        provider_id: provider.id.clone(),
        key: provider.key.clone(),
        headers: provider.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        bare_model: bare,
        anthropic: provider.anthropic.trim_end_matches('/').to_string(),
        chat: provider.chat.trim_end_matches('/').to_string(),
    })
}

// =====================================================================
// 入口
// =====================================================================

/// POST /v1/messages 入口（routes.rs 接线由主线做）。
pub fn handle(gateway: &super::Gateway, body: &[u8]) -> Reply {
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return error_reply(400, &format!("invalid request body: {e}")),
    };
    let model = parsed.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let target = match resolve(gateway, &model) {
        Ok(t) => t,
        // 与 chat.rs 同口径：解析不到来源就是 404（模型存在性不查，那是
        // 厂商应答该说的话）。
        Err(e) => return error_reply(404, &e),
    };
    let streaming = parsed.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    if !target.anthropic.is_empty() {
        return forward_direct(&target, body, streaming);
    }
    if !target.chat.is_empty() {
        return forward_translated(gateway, &target, &parsed, &model, streaming);
    }
    // 只配了 responses（或什么都没有）：没有诚实的 chat/anthropic 出站点，
    // 别拿 responses 基址拼 /chat/completions 硬试（URL 形状就是错的）；
    // responses 翻译是 T4 的事。
    error_reply(404, &format!("{} has no endpoint for /v1/messages", target.provider_id))
}

// =====================================================================
// 直通路径（anthropic URL 非空）
// =====================================================================

fn forward_direct(target: &Target, body: &[u8], streaming: bool) -> Reply {
    let out_body = rewrite_model(body, &target.bare_model);
    let response = match send_messages(target, &out_body) {
        Ok(r) => r,
        Err(e) => return error_reply(502, &format!("{}: {}", target.provider_id, e)),
    };
    let status = response.status();
    if streaming && status < 400 {
        let content_type = response.content_type().to_string();
        if content_type.contains("event-stream") {
            // SSE 原样透传：字节一个不动，content-type 用厂商自己的。
            return Reply::Stream {
                status,
                content_type,
                reader: Box::new(response.into_reader()),
            };
        }
        // 2xx 但不是事件流（厂商没理会 stream）：整读按普通应答直通。
    }
    let vendor_body = read_all(response.into_reader());
    if status >= 400 {
        return vendor_error_reply(status, &vendor_body);
    }
    Reply::Json { status, body: vendor_body }
}

/// 直通出站：与 chat::send 同骨架（ureq Status(4xx/5xx) 取回不当错误），
/// 认证头换 anthropic 三件套。
fn send_messages(target: &Target, body: &[u8]) -> Result<ureq::Response, String> {
    let agent = super::outbound_agent();
    let url = format!("{}/v1/messages", target.anthropic);
    let mut request = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .set("anthropic-version", ANTHROPIC_VERSION);
    if !target.key.is_empty() {
        // 双头都带：官方认 x-api-key，第三方中转多认 Bearer。
        request = request
            .set("x-api-key", &target.key)
            .set("Authorization", &format!("Bearer {}", target.key));
    }
    for (k, v) in &target.headers {
        request = request.set(k, v);
    }
    match request.send_bytes(body) {
        Ok(response) => Ok(response),
        Err(ureq::Error::Status(_, response)) => Ok(response),
        Err(e) => Err(e.to_string()),
    }
}

/// 把请求体里的 `model` 换成厂商裸名（chat.rs rewrite_model 同款：解析失败
/// 原样返回——让厂商去说这是什么）。
fn rewrite_model(body: &[u8], bare: &str) -> Vec<u8> {
    let mut value = match serde_json::from_slice::<Value>(body) {
        Ok(v) => v,
        Err(_) => return body.to_vec(),
    };
    if let Some(map) = value.as_object_mut() {
        map.insert("model".into(), Value::String(bare.to_string()));
    }
    serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec())
}

// =====================================================================
// 翻译路径（anthropic URL 空、chat URL 有）
// =====================================================================

fn forward_translated(gw: &super::Gateway, target: &Target, parsed: &Value, asked_model: &str, streaming: bool) -> Reply {
    let chat_body = match translate::anthropic_to_chat(parsed) {
        Ok(v) => v,
        Err(e) => return error_reply(400, &e),
    };
    // model 换厂商裸名；流式再注入 stream_options.include_usage——Go buildChat
    // 同款（chat.go：`if r.Stream { out["stream_options"] = … }`），OpenAI 形态
    // 的厂商只有这个开关才会把最终 usage 帧发出来，anthropic 客户端的
    // message_delta 才有真数可带。
    let chat_body = {
        let mut v = chat_body;
        if let Some(map) = v.as_object_mut() {
            map.insert("model".into(), Value::String(target.bare_model.clone()));
            if streaming {
                map.insert("stream_options".into(), json!({ "include_usage": true }));
            }
        }
        v
    };
    let chat_bytes = match serde_json::to_vec(&chat_body) {
        Ok(b) => b,
        Err(e) => return error_reply(500, &format!("re-serialize failed: {e}")),
    };
    // cap 已知时先合身再发（免吃一次拒答）；4xx 的学 cap 丢图重发挂
    // maybe_refit（见下）。
    let chat_bytes = bodycap::prefit(gw, &target.provider_id, &chat_bytes).unwrap_or(chat_bytes);
    let out = chat::Outbound::new(
        target.provider_id.clone(),
        target.key.clone(),
        target.headers.clone(),
        target.bare_model.clone(),
        format!("{}/chat/completions", target.chat),
    );
    let response = match chat::send(&out, &chat_bytes) {
        Ok(r) => r,
        Err(e) => return error_reply(502, &format!("{}: {}", target.provider_id, e)),
    };
    if streaming && response.status() < 400 && response.content_type().contains("event-stream") {
        // chat SSE → anthropic SSE 逐事件翻译（message_start 编排、单次收口
        // 都在翻译器里）。
        return Reply::Stream {
            status: response.status(),
            content_type: "text/event-stream".into(),
            reader: Box::new(AnthropicSseTranslator::new(response.into_reader(), asked_model)),
        };
    }
    // 非流式（或厂商没给事件流/给了错误状态）：整读再转。
    let status = response.status();
    let vendor_body = read_all(response.into_reader());
    if status >= 400 {
        // C5+C7：字节上限拒答 → 学 cap、丢最老图重发；不是 cap 拒答时
        // maybe_refit 返回 None，落进 anthropic 错误转换。
        if let Some(reply) = bodycap::maybe_refit(gw, &out, &chat_bytes, status, &vendor_body) {
            return refit_reply_to_anthropic(reply, streaming, asked_model);
        }
        return vendor_error_reply(status, &vendor_body);
    }
    match chat_response_to_anthropic(&vendor_body, asked_model) {
        Ok(v) => Reply::json(status, &v),
        // 厂商 2xx 却给了读不懂的体：网关侧问题，按 502 说。
        Err(e) => error_reply(502, &e),
    }
}

/// bodycap 重发回路的应答归一成 anthropic 面：maybe_refit 只认识 chat 世界，
/// 它的 2xx 应答是 chat 形状（流式请求时甚至是一整段 chat SSE 文本）、
/// 413/502 是 openai 形状文本——anthropic 客户端两样都读不懂，这里转掉。
fn refit_reply_to_anthropic(reply: Reply, streaming: bool, asked_model: &str) -> Reply {
    let (status, body) = match reply {
        Reply::Json { status, body } => (status, body),
        other => return other, // maybe_refit 只会回 Json，防御性放行
    };
    if status >= 400 {
        return vendor_error_reply(status, &body);
    }
    if streaming {
        // 重发体带着 stream:true，厂商回的是一整段 chat SSE 文本：映射成
        // 完整 anthropic 流一次性交回（内容完整、帧序正确，只是不再增量）。
        if let Some(text) = sse_text_to_anthropic(&body, asked_model) {
            return Reply::Stream {
                status,
                content_type: "text/event-stream".into(),
                reader: Box::new(std::io::Cursor::new(text)),
            };
        }
    }
    match chat_response_to_anthropic(&body, asked_model) {
        Ok(v) => Reply::json(status, &v),
        Err(_) => vendor_error_reply(status, &body),
    }
}

// =====================================================================
// SSE 翻译器：chat 事件流 → anthropic 事件流
// =====================================================================

/// 包在厂商 SSE reader 外的 Read 适配器：逐行取 `data: …`，经
/// translate::sse_chat_to_anthropic 映射，端点侧负责拼完整帧
/// （`event: <type>\ndata: {...}\n\n`——映射函数给的就是帧体，这里补
/// 帧分隔）与 message_start 编排。无法映射的行（role 首帧、tool_calls
/// 分片、注释、event 行）原样跳过，绝不中断流。
struct AnthropicSseTranslator {
    inner: BufReader<Box<dyn Read + Send>>,
    /// 待写出的 anthropic 字节（pos 之前已消费）。
    out: Vec<u8>,
    pos: usize,
    /// message_start 已发（第一帧必须是它，anthropic SDK 的硬要求）。
    started: bool,
    /// 已见过 message_stop：之后的尾巴（usage 末帧、[DONE]）全部丢弃——
    /// 无状态映射对 finish 与 [DONE] 各收口一次，一条流只能收口一次。
    stopped: bool,
    done: bool,
    model: String,
}

impl AnthropicSseTranslator {
    fn new(reader: Box<dyn Read + Send>, model: &str) -> AnthropicSseTranslator {
        AnthropicSseTranslator {
            inner: BufReader::new(reader),
            out: Vec::new(),
            pos: 0,
            started: false,
            stopped: false,
            done: false,
            model: model.to_string(),
        }
    }
}

impl Read for AnthropicSseTranslator {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.pos < self.out.len() {
                let n = std::cmp::min(buf.len(), self.out.len() - self.pos);
                buf[..n].copy_from_slice(&self.out[self.pos..self.pos + n]);
                self.pos += n;
                if self.pos == self.out.len() {
                    self.out.clear();
                    self.pos = 0;
                }
                return Ok(n);
            }
            // 收口之后不再读上游：尾巴全丢，也免厂商挂着连接不关。
            if self.done || self.stopped {
                return Ok(0);
            }
            let mut line = String::new();
            match self.inner.read_line(&mut line) {
                Ok(0) => {
                    self.done = true;
                    return Ok(0);
                }
                Ok(_) => {}
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    self.done = true;
                    return Err(e);
                }
            }
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            let Some(data) = trimmed.strip_prefix("data:") else { continue };
            let Some(frames) = translate::sse_chat_to_anthropic(data.trim()) else { continue };
            if !self.started {
                self.out.extend_from_slice(message_start_frame(&self.model).as_bytes());
                self.started = true;
            }
            if frames.contains("event: message_stop") {
                self.stopped = true;
            }
            self.out.extend_from_slice(frames.as_bytes());
            self.out.extend_from_slice(b"\n\n");
        }
    }
}

/// message_start 帧（Go anthropicEncoder.start 同形状：空 content、null
/// stop、零 usage——首帧时还没有任何真数，不编）。id 用进程内计数器，
/// 客户端只要一个非空串做链路标识。
fn message_start_frame(model: &str) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let data = json!({
        "type": "message_start",
        "message": {
            "id": format!("msg_{id}"),
            "type": "message",
            "role": "assistant",
            "model": model,
            "content": [],
            "stop_reason": null,
            "stop_sequence": null,
            "usage": { "input_tokens": 0, "output_tokens": 0 },
        }
    });
    format!(
        "event: message_start\ndata: {}\n\n",
        serde_json::to_string(&data).unwrap_or_default()
    )
}

/// 一整段 chat SSE 文本 → 完整 anthropic SSE 文本（refit 回路的整读形态）。
/// 一条 data 行都映射不上（不是 SSE）返回 None。
fn sse_text_to_anthropic(body: &[u8], model: &str) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?;
    let mut out = String::new();
    let mut mapped = false;
    let mut stopped = false;
    for line in text.lines() {
        if stopped {
            break;
        }
        let Some(data) = line.trim_end().strip_prefix("data:") else { continue };
        if let Some(frames) = translate::sse_chat_to_anthropic(data.trim()) {
            mapped = true;
            if frames.contains("event: message_stop") {
                stopped = true;
            }
            out.push_str(&frames);
            out.push_str("\n\n");
        }
    }
    if !mapped {
        return None;
    }
    Some(message_start_frame(model) + &out)
}

// =====================================================================
// 非流式应答转换：chat completion → anthropic message
// =====================================================================

/// chat completion 应答 → anthropic message 应答。translate.rs 刻意不含
/// 应答面的转换（T1 的稳定 API 只有请求互转与 SSE 事件映射），这块留在
/// 端点里；stop/usage 的映射语义与 translate.rs 的私有函数同款（那边不
/// 导出，这里镜像，改语义时两处一起看）。
fn chat_response_to_anthropic(body: &[u8], asked_model: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("bad vendor reply: {e}"))?;
    let choice = v
        .pointer("/choices/0")
        .ok_or_else(|| "vendor reply has no choices".to_string())?;
    let message = choice.get("message");
    let mut blocks: Vec<Value> = Vec::new();
    match message.and_then(|m| m.get("content")) {
        // content 通常是字符串；parts 数组形态（少数厂商）取 text 块。
        Some(Value::String(s)) => {
            if !s.is_empty() {
                blocks.push(json!({ "type": "text", "text": s }));
            }
        }
        Some(Value::Array(parts)) => {
            for p in parts {
                if p.get("type").and_then(|t| t.as_str()) == Some("text") {
                    if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                        if !t.is_empty() {
                            blocks.push(json!({ "type": "text", "text": t }));
                        }
                    }
                }
            }
        }
        _ => {}
    }
    if let Some(calls) = message.and_then(|m| m.get("tool_calls")).and_then(|c| c.as_array()) {
        for tc in calls {
            let name = tc.pointer("/function/name").and_then(|n| n.as_str()).unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let args = tc.pointer("/function/arguments").and_then(|a| a.as_str()).unwrap_or_default();
            blocks.push(json!({
                "type": "tool_use",
                // chat 的 call_* id 原样透传：往返（下一条请求的 tool_result
                // 回来）只要求同一根串，不要求 anthropic 的 toolu_ 前缀。
                "id": tc.get("id").and_then(|i| i.as_str()).unwrap_or_default(),
                "name": name,
                "input": parse_tool_args(args),
            }));
        }
    }
    let finish = choice.get("finish_reason").and_then(|f| f.as_str()).unwrap_or("stop");
    Ok(json!({
        "id": format!("msg_{}", v.get("id").and_then(|i| i.as_str()).unwrap_or("translated")),
        "type": "message",
        "role": "assistant",
        // 回程报客户端问的那个模型名（provider/model 形态），不是厂商裸名。
        "model": asked_model,
        "content": blocks,
        "stop_reason": chat_stop_to_anthropic(finish),
        "stop_sequence": null,
        "usage": anthropic_usage_of(v.get("usage")),
    }))
}

/// 工具参数串 → JSON 对象（translate.rs parse_args 同语义：合法 JSON 对象
/// 原样，其余包成 {"input": s}——不丢模型给的任何字；空 → {}）。
fn parse_tool_args(s: &str) -> Value {
    let s = s.trim();
    if s.is_empty() {
        return json!({});
    }
    if let Ok(v) = serde_json::from_str::<Value>(s) {
        if v.is_object() {
            return v;
        }
    }
    json!({ "input": s })
}

/// chat stop_reason → anthropic 的（translate.rs chat_stop_to_anthropic 镜像）。
fn chat_stop_to_anthropic(stop: &str) -> &'static str {
    match stop {
        "length" => "max_tokens",
        "tool_calls" | "function_call" => "tool_use",
        "content_filter" => "refusal",
        _ => "end_turn",
    }
}

/// chat usage → anthropic usage（translate.rs anthropic_usage 镜像：只搬
/// 本矩阵保真的桶，agent 不报的就是 0，不编数）。
fn anthropic_usage_of(u: Option<&Value>) -> Value {
    match u {
        Some(u) => json!({
            "input_tokens": u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            "output_tokens": u.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            "cache_read_input_tokens":
                u.pointer("/prompt_tokens_details/cached_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        }),
        None => json!({ "input_tokens": 0, "output_tokens": 0 }),
    }
}

// =====================================================================
// 错误面
// =====================================================================

/// anthropic 形状的错误应答（Go writeErrorAs 的 anthropic 分支）。
fn error_reply(status: u16, message: &str) -> Reply {
    Reply::json(status, &json!({ "type": "error", "error": { "type": error_type(status), "message": message } }))
}

/// 状态码 → anthropic 错误类型（Go errorTypeFor 同表）。
fn error_type(status: u16) -> &'static str {
    match status {
        400 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        413 => "request_too_large",
        429 => "rate_limit_error",
        529 => "overloaded_error",
        _ => "api_error",
    }
}

/// 厂商错误 → anthropic 错误面，状态码透传。直通路径上厂商自己的
/// anthropic 形状错误（`type:"error"` + `error.type`）原样透传——厂商的
/// 话（错误类型串、message 原文）一个字不改写。
fn vendor_error_reply(status: u16, body: &[u8]) -> Reply {
    if anthropic_error_shaped(body) {
        return Reply::Json { status, body: body.to_vec() };
    }
    error_reply(status, &vendor_error_message(status, body))
}

fn anthropic_error_shaped(body: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    v.get("type").and_then(|t| t.as_str()) == Some("error")
        && v.pointer("/error/type").is_some_and(|t| t.as_str().is_some())
}

/// 从厂商错误体里捞人话：openai 形状的 error.message / 裸 error 串 /
/// 顶层 message / detail；JSON 读不出就用截断原文（空体给占位——状态码
/// 本身已经是信息，别回一个空 message）。
fn vendor_error_message(status: u16, body: &[u8]) -> String {
    if let Ok(v) = serde_json::from_slice::<Value>(body) {
        for pointer in ["/error/message", "/error", "/message", "/detail"] {
            if let Some(s) = v.pointer(pointer).and_then(|x| x.as_str()) {
                if !s.trim().is_empty() {
                    return s.to_string();
                }
            }
        }
    }
    let text = String::from_utf8_lossy(body);
    let text = text.trim();
    if text.is_empty() {
        return format!("vendor answered {status} with an empty body");
    }
    // 整页 HTML/超长文本别全回给 agent：按字符截 400。
    text.chars().take(400).collect()
}

fn read_all(mut reader: impl Read) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = reader.read_to_end(&mut buf);
    buf
}

// =====================================================================
// 测试（本地假厂商：std TcpListener 手写 HTTP，quotas.rs 同款写法）
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// 测试串行锁：配置目录靠 XDG_CONFIG_HOME（进程级 env），跨模块的沙箱
    /// 测试统一取 mod.rs 的共享锁（quotas / presets 同款），并行测试不互踩 env。
    static DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

    struct Sandbox {
        dir: std::path::PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn sandbox(providers_json: &str) -> (super::super::Gateway, Sandbox) {
        let lock = super::super::TEST_SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "marionette-engine-messages-{}-{}",
            std::process::id(),
            DIR_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        // config_dir() = XDG_CONFIG_HOME/magpie；绝不碰真实 ~/.config/magpie。
        std::fs::create_dir_all(dir.join("magpie")).expect("temp config dir");
        std::fs::write(dir.join("magpie").join("providers.json"), providers_json)
            .expect("providers.json");
        let prior_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let gateway = super::super::Gateway::open().expect("gateway");
        // 立刻恢复 env：Gateway::open 之后 store 持有的是绝对路径，后续
        // reload 也只读它——比整场持有 env 的窗口小，跨模块并行测试更不易互踩。
        match &prior_xdg {
            Some(p) => std::env::set_var("XDG_CONFIG_HOME", p),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        (gateway, Sandbox { dir, _lock: lock })
    }

    /// 假厂商收到的一个请求。
    #[derive(Clone)]
    struct Seen {
        method: String,
        path: String,
        headers: Vec<(String, String)>,
        body: String,
    }

    impl Seen {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
        }
    }

    struct FakeVendor {
        /// 基址（不带路径），providers.json 里填它。
        base: String,
        requests: Arc<Mutex<Vec<Seen>>>,
    }

    impl FakeVendor {
        fn last(&self) -> Seen {
            self.requests
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .last()
                .expect("vendor got no request")
                .clone()
        }
    }

    fn status_text(status: u16) -> &'static str {
        match status {
            200 => "OK",
            401 => "Unauthorized",
            404 => "Not Found",
            413 => "Payload Too Large",
            _ => "Response",
        }
    }

    /// 本地假厂商：读一个完整请求（请求行 + 头 + Content-Length 体）记档，
    /// 回固定应答（status + content_type + body）。Connection: close + 精确
    /// Content-Length：ureq 不复用连接，一请求一连接。
    fn fake_vendor(status: u16, content_type: &'static str, body: &'static str) -> FakeVendor {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake vendor");
        let port = listener.local_addr().expect("port").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let thread_seen = requests.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let mut reader = BufReader::new(match stream.try_clone() {
                    Ok(s) => s,
                    Err(_) => continue,
                });
                let mut start = String::new();
                if reader.read_line(&mut start).unwrap_or(0) == 0 {
                    continue;
                }
                let mut parts = start.split_whitespace();
                let method = parts.next().unwrap_or_default().to_string();
                let path = parts.next().unwrap_or_default().to_string();
                let mut headers = Vec::new();
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        break;
                    }
                    let trimmed = line.trim_end();
                    if trimmed.is_empty() {
                        break;
                    }
                    if let Some((k, v)) = trimmed.split_once(':') {
                        let k = k.trim().to_ascii_lowercase();
                        let v = v.trim().to_string();
                        if k == "content-length" {
                            length = v.parse().unwrap_or(0);
                        }
                        headers.push((k, v));
                    }
                }
                let mut body_bytes = vec![0u8; length];
                if length > 0 {
                    let _ = reader.read_exact(&mut body_bytes);
                }
                thread_seen.lock().unwrap_or_else(|p| p.into_inner()).push(Seen {
                    method,
                    path,
                    headers,
                    body: String::from_utf8_lossy(&body_bytes).into_owned(),
                });
                let mut stream = stream;
                let head = format!(
                    "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    status,
                    status_text(status),
                    content_type,
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body.as_bytes());
                let _ = stream.flush();
            }
        });
        FakeVendor { base: format!("http://127.0.0.1:{port}"), requests }
    }

    // -----------------------------------------------------------------
    // 翻译路径非流式
    // -----------------------------------------------------------------

    #[test]
    fn translate_non_streaming() {
        let chat_reply = r#"{"id":"chatcmpl-1","object":"chat.completion","model":"m",
            "choices":[{"index":0,"message":{"role":"assistant","content":"hello there"},"finish_reason":"stop"}],
            "usage":{"prompt_tokens":3,"completion_tokens":2,"prompt_tokens_details":{"cached_tokens":1}}}"#;
        let vendor = fake_vendor(200, "application/json", chat_reply);
        let providers = format!(
            r#"{{"providers":[{{"id":"fake","name":"Fake","key":"sk-test","chat":"{}/v1"}}]}}"#,
            vendor.base
        );
        let (gateway, _sandbox) = sandbox(&providers);
        let request = json!({
            "model": "fake/m", "max_tokens": 64, "system": "be nice",
            "messages": [{ "role": "user", "content": [{ "type": "text", "text": "hi" }] }]
        });
        let reply = handle(&gateway, serde_json::to_vec(&request).unwrap().as_slice());
        let Reply::Json { status, body } = reply else {
            panic!("expected json reply, got stream");
        };
        assert_eq!(status, 200);
        let v: Value = serde_json::from_slice(&body).expect("anthropic json");
        assert_eq!(v["type"], "message");
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["model"], "fake/m", "回程报客户端问的模型名");
        assert_eq!(v["content"][0]["type"], "text");
        assert_eq!(v["content"][0]["text"], "hello there");
        assert_eq!(v["stop_reason"], "end_turn");
        assert_eq!(v["usage"]["input_tokens"], 3);
        assert_eq!(v["usage"]["output_tokens"], 2);
        assert_eq!(v["usage"]["cache_read_input_tokens"], 1);

        // 假厂商收到的是 chat 形状：剥前缀的裸模型名、system 折叠、Bearer。
        let seen = vendor.last();
        assert_eq!(seen.method, "POST");
        assert_eq!(seen.path, "/v1/chat/completions");
        assert_eq!(seen.header("authorization"), Some("Bearer sk-test"));
        let sent: Value = serde_json::from_str(&seen.body).expect("sent chat json");
        assert_eq!(sent["model"], "m");
        assert_eq!(sent["messages"][0], json!({"role": "system", "content": "be nice"}));
        assert_eq!(sent["messages"][1], json!({"role": "user", "content": "hi"}));
        assert_eq!(sent["max_tokens"], 64);
    }

    // -----------------------------------------------------------------
    // 直通路径
    // -----------------------------------------------------------------

    #[test]
    fn direct_passthrough_sends_anthropic_auth() {
        let anthropic_reply = r#"{"id":"msg_9","type":"message","role":"assistant","model":"claude-x",
            "content":[{"type":"text","text":"raw"}],"stop_reason":"end_turn",
            "usage":{"input_tokens":1,"output_tokens":1}}"#;
        let vendor = fake_vendor(200, "application/json", anthropic_reply);
        let providers = format!(
            r#"{{"providers":[{{"id":"anth","name":"Anth","key":"sk-ant","anthropic":"{}"}}]}}"#,
            vendor.base
        );
        let (gateway, _sandbox) = sandbox(&providers);
        let request = json!({
            "model": "anth/claude-x", "max_tokens": 32, "system": "be nice",
            "messages": [{ "role": "user", "content": "hi" }]
        });
        let reply = handle(&gateway, serde_json::to_vec(&request).unwrap().as_slice());
        let Reply::Json { status, body } = reply else {
            panic!("expected json reply, got stream");
        };
        assert_eq!(status, 200);
        // 应答原样：一个字节不动。
        assert_eq!(body, anthropic_reply.as_bytes());

        let seen = vendor.last();
        assert_eq!(seen.path, "/v1/messages", "直通打到 anthropic 基址下的 /v1/messages");
        assert_eq!(seen.header("x-api-key"), Some("sk-ant"));
        assert_eq!(seen.header("authorization"), Some("Bearer sk-ant"));
        assert_eq!(seen.header("anthropic-version"), Some("2023-06-01"));
        // model 剥前缀，其余字段原样（system 还在）。
        let sent: Value = serde_json::from_str(&seen.body).expect("sent anthropic json");
        assert_eq!(sent["model"], "claude-x");
        assert_eq!(sent["system"], "be nice");
        assert_eq!(sent["messages"][0]["content"], "hi");
    }

    // -----------------------------------------------------------------
    // 流式映射
    // -----------------------------------------------------------------

    #[test]
    fn stream_translation_maps_chat_sse_to_anthropic_frames() {
        // role 首帧（映射不上，该跳过）→ 文本 delta → finish（带 usage）→
        // include_usage 末帧 → [DONE]。无状态映射对 finish 与 [DONE] 各
        // 收口一次 message_stop——编排层只留一次。
        let sse = concat!(
            r#"data: {"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"你好"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":7,"completion_tokens":4}}"#,
            "\n\n",
            r#"data: {"id":"c1","object":"chat.completion.chunk","choices":[],"usage":{"prompt_tokens":7,"completion_tokens":4}}"#,
            "\n\n",
            "data: [DONE]\n\n",
        );
        let vendor = fake_vendor(200, "text/event-stream", sse);
        let providers = format!(
            r#"{{"providers":[{{"id":"fake","name":"Fake","key":"sk-test","chat":"{}/v1"}}]}}"#,
            vendor.base
        );
        let (gateway, _sandbox) = sandbox(&providers);
        let request = json!({
            "model": "fake/m", "max_tokens": 64, "stream": true,
            "messages": [{ "role": "user", "content": "hi" }]
        });
        let reply = handle(&gateway, serde_json::to_vec(&request).unwrap().as_slice());
        let Reply::Stream { status, content_type, mut reader } = reply else {
            panic!("expected stream reply, got json");
        };
        assert_eq!(status, 200);
        assert!(content_type.contains("text/event-stream"));
        let mut out = String::new();
        reader.read_to_string(&mut out).expect("read translated stream");

        // message_start 必须是第一帧（anthropic SDK 对它之前的事件直接抛错）。
        assert!(out.starts_with("event: message_start\n"), "got: {out}");
        assert!(out.contains("event: content_block_delta\n"));
        assert!(out.contains(r#""text":"你好""#));
        // finish 帧的 usage 进 message_delta。
        assert!(out.contains("event: message_delta\n"));
        assert!(out.contains(r#""input_tokens":7"#));
        // 一条流只收口一次：finish 与 [DONE] 的重复 message_stop、以及收口后
        // 的 include_usage 末帧都被编排层丢弃。
        assert_eq!(out.matches("event: message_stop").count(), 1);
        assert_eq!(out.matches("event: message_delta").count(), 1);
        // 出站的 chat 体带了 stream_options.include_usage（Go buildChat 同款）。
        let sent: Value = serde_json::from_str(&vendor.last().body).expect("sent chat json");
        assert_eq!(sent["stream_options"], json!({"include_usage": true}));
    }

    // -----------------------------------------------------------------
    // 未知模型
    // -----------------------------------------------------------------

    #[test]
    fn unknown_model_is_anthropic_404() {
        let (gateway, _sandbox) = sandbox(r#"{"providers":[]}"#);
        let request = json!({
            "model": "nope/m", "max_tokens": 8,
            "messages": [{ "role": "user", "content": "hi" }]
        });
        let reply = handle(&gateway, serde_json::to_vec(&request).unwrap().as_slice());
        let Reply::Json { status, body } = reply else {
            panic!("expected json reply, got stream");
        };
        assert_eq!(status, 404);
        let v: Value = serde_json::from_slice(&body).expect("error json");
        assert_eq!(v["type"], "error");
        assert_eq!(v["error"]["type"], "not_found_error");
        assert!(v["error"]["message"].as_str().unwrap().contains("nope/m"));
    }

    // -----------------------------------------------------------------
    // 厂商错误 → anthropic 错误面
    // -----------------------------------------------------------------

    #[test]
    fn vendor_error_becomes_anthropic_error_shape() {
        let vendor = fake_vendor(401, "application/json", r#"{"error":{"message":"bad key"}}"#);
        let providers = format!(
            r#"{{"providers":[{{"id":"fake","name":"Fake","key":"sk-test","chat":"{}/v1"}}]}}"#,
            vendor.base
        );
        let (gateway, _sandbox) = sandbox(&providers);
        let request = json!({
            "model": "fake/m", "max_tokens": 8,
            "messages": [{ "role": "user", "content": "hi" }]
        });
        let reply = handle(&gateway, serde_json::to_vec(&request).unwrap().as_slice());
        let Reply::Json { status, body } = reply else {
            panic!("expected json reply, got stream");
        };
        assert_eq!(status, 401, "状态码透传");
        let v: Value = serde_json::from_slice(&body).expect("error json");
        assert_eq!(v["type"], "error");
        assert_eq!(v["error"]["type"], "authentication_error");
        assert_eq!(v["error"]["message"], "bad key", "厂商的话保留");
    }
}
