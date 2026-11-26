//! Antigravity 的 Gemini 协议面：把 `agy_acp_server` 的 gateway 流量
//! 翻译成引擎的 OpenAI chat-completions。
//!
//! Antigravity 官方支持 `auth.type = "gateway"` + `AGY_LLM_GATEWAY_URL`
//! （ACP server 只支持 `genai` wire protocol）。我们起一个 loopback HTTP
//! 服务，暴露：
//!
//! - `GET  /v1beta/models`（Gemini list 形状，模型来自引擎目录）
//! - `POST /v1beta/models/{model}:generateContent`
//! - `POST /v1beta/models/{model}:streamGenerateContent`
//!
//! 请求体里的 `contents` / `systemInstruction` / `tools` / `toolConfig` /
//! `generationConfig` 转成 OpenAI chat；响应再转回 Gemini `candidates`
//! （含 functionCall）。上游永远是我们自己的引擎 `/v1/chat/completions`，
//! 所以 Antigravity 可以跑 DeepSeek/GPT/Claude/Grok 等任何引擎模型。

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};
use std::sync::OnceLock;
use tiny_http::{Header, Method, Response, Server};

const PREFERRED_PORT: u16 = 3426;
const MAX_BODY: usize = 32 * 1024 * 1024;

static SERVING: OnceLock<Result<u16, String>> = OnceLock::new();

/// 启动（幂等）并返回 `http://127.0.0.1:<port>`。
pub fn ensure_serving() -> Result<String, String> {
    let port = match SERVING.get_or_init(start_server) {
        Ok(port) => *port,
        Err(error) => return Err(error.clone()),
    };
    Ok(format!("http://127.0.0.1:{port}"))
}

fn start_server() -> Result<u16, String> {
    let server = Server::http(("127.0.0.1", PREFERRED_PORT))
        .or_else(|_| Server::http(("127.0.0.1", 0)))
        .map_err(|error| format!("antigravity bridge: bind failed: {error}"))?;
    let port = server
        .server_addr()
        .to_ip()
        .map(|addr| addr.port())
        .unwrap_or(PREFERRED_PORT);
    std::thread::Builder::new()
        .name("antigravity-bridge".into())
        .spawn(move || {
            for request in server.incoming_requests() {
                handle(request);
            }
        })
        .map_err(|error| format!("antigravity bridge: spawn failed: {error}"))?;
    Ok(port)
}

fn engine_base() -> Result<String, String> {
    if let Some(base) = crate::engine::engine_v1_url() {
        return Ok(base);
    }
    crate::engine::wake_if_installed();
    crate::engine::ensure_bridge_ready(std::time::Duration::from_secs(30));
    crate::engine::engine_v1_url()
        .ok_or_else(|| "translation engine is not running".to_string())
}

fn handle(mut request: tiny_http::Request) {
    let url = request.url().to_string();
    let path = url.split('?').next().unwrap_or("").to_string();
    let method = request.method().clone();

    if method == Method::Get && (path == "/v1beta/models" || path == "/v1beta/models/") {
        let body = list_models();
        respond_json(request, 200, body);
        return;
    }

    if method == Method::Post {
        if let Some((model, stream)) = model_from_path(&path) {
            let mut raw = String::new();
            if let Err(error) = request.as_reader().take(MAX_BODY as u64).read_to_string(&mut raw) {
                respond_json(request, 400, gemini_error(400, &format!("read body: {error}")));
                return;
            }
            let body: Value = match serde_json::from_str(&raw) {
                Ok(body) => body,
                Err(error) => {
                    respond_json(request, 400, gemini_error(400, &format!("invalid JSON: {error}")));
                    return;
                }
            };
            handle_generate(request, &model, stream, body);
            return;
        }
    }

    respond_json(request, 404, gemini_error(404, "not found"));
}

fn list_models() -> Value {
    let (mut ids, _offline) = crate::engine::catalog_direct_or_cached();
    if ids.is_empty() {
        ids = crate::engine::catalog_direct_entries()
            .into_iter()
            .map(|row| row.id)
            .collect();
    }
    let rows: BTreeMap<String, (u64, u64)> = crate::engine::catalog_direct_entries()
        .into_iter()
        .map(|row| (row.id, (row.context_window, row.max_output)))
        .collect();
    let models: Vec<Value> = ids
        .into_iter()
        .map(|id| {
            let (input, output) = rows.get(&id).copied().unwrap_or((0, 0));
            json!({
                "name": format!("models/{id}"),
                "displayName": id,
                "supportedGenerationMethods": ["generateContent", "streamGenerateContent"],
                "inputTokenLimit": if input > 0 { json!(input) } else { Value::Null },
                "outputTokenLimit": if output > 0 { json!(output) } else { Value::Null },
            })
        })
        .collect();
    json!({ "models": models })
}

fn model_from_path(path: &str) -> Option<(String, bool)> {
    let rest = path.strip_prefix("/v1beta/models/")?;
    let (model, tail) = rest.rsplit_once(':')?;
    let stream = tail.starts_with("streamGenerateContent");
    if !tail.starts_with("generateContent") && !stream {
        return None;
    }
    let model = percent_decode(model);
    let model = model.strip_prefix("models/").unwrap_or(&model).to_string();
    if model.is_empty() {
        return None;
    }
    Some((model, stream))
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(hi), Some(lo)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(hi * 16 + lo);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn handle_generate(request: tiny_http::Request, model: &str, stream: bool, body: Value) {
    let base = match engine_base() {
        Ok(base) => base,
        Err(error) => {
            respond_json(request, 503, gemini_error(503, &error));
            return;
        }
    };
    let payload = gemini_to_openai(&body, model, stream);
    let url = format!("{}/chat/completions", base.trim_end_matches('/'));
    let token = crate::engine::gateway_token();
    let response = ureq::post(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(600))
        .send_json(payload);
    let response = match response {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let text = response.into_string().unwrap_or_default();
            respond_json(request, code, gemini_error(code, &text));
            return;
        }
        Err(error) => {
            respond_json(request, 502, gemini_error(502, &format!("upstream: {error}")));
            return;
        }
    };

    if stream {
        let reader = GeminiSseReader::new(response.into_reader());
        let mut forwarded = Response::empty(200).with_data(reader, None);
        if let Ok(header) = Header::from_bytes(&b"Content-Type"[..], b"text/event-stream") {
            forwarded = forwarded.with_header(header);
        }
        let _ = request.respond(forwarded);
        return;
    }

    let body: Result<Value, String> = match response.into_string() {
        Ok(text) => serde_json::from_str::<Value>(&text).map_err(|error| error.to_string()),
        Err(error) => Err(format!("read upstream: {error}")),
    };
    match body {
        Ok(value) => respond_json(request, 200, openai_to_gemini(&value)),
        Err(error) => respond_json(request, 502, gemini_error(502, &format!("bad upstream JSON: {error}"))),
    }
}

fn respond_json(request: tiny_http::Request, status: u16, body: Value) {
    let mut response = Response::from_string(body.to_string()).with_status_code(status);
    if let Ok(header) = Header::from_bytes(&b"Content-Type"[..], b"application/json") {
        response = response.with_header(header);
    }
    let _ = request.respond(response);
}

fn gemini_error(code: u16, message: &str) -> Value {
    json!({
        "error": {
            "code": code,
            "message": message.chars().take(2000).collect::<String>(),
            "status": if code == 400 { "INVALID_ARGUMENT" } else if code == 404 { "NOT_FOUND" } else { "UNAVAILABLE" },
        }
    })
}

// ── Gemini → OpenAI ─────────────────────────────────────────────────────

fn gemini_to_openai(body: &Value, model: &str, stream: bool) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(text) = parts_text(body.get("systemInstruction")) {
        if !text.trim().is_empty() {
            messages.push(json!({ "role": "system", "content": text }));
        }
    }
    let mut call_ids: BTreeMap<String, String> = BTreeMap::new();
    let mut counter = 0usize;
    if let Some(contents) = body.get("contents").and_then(Value::as_array) {
        for content in contents {
            let role = content
                .get("role")
                .and_then(Value::as_str)
                .unwrap_or("user");
            let parts = content
                .get("parts")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if role == "model" {
                let mut text = String::new();
                let mut tool_calls = Vec::new();
                for part in &parts {
                    if let Some(value) = part.get("text").and_then(Value::as_str) {
                        text.push_str(value);
                    }
                    if let Some(call) = part.get("functionCall") {
                        let name = call.get("name").and_then(Value::as_str).unwrap_or("tool");
                        let id = format!("call_{name}_{counter}");
                        counter += 1;
                        call_ids.insert(name.to_string(), id.clone());
                        tool_calls.push(json!({
                            "id": id,
                            "type": "function",
                            "function": {
                                "name": name,
                                "arguments": call.get("args").cloned().unwrap_or_else(|| json!({})).to_string(),
                            }
                        }));
                    }
                }
                if tool_calls.is_empty() {
                    if !text.is_empty() {
                        messages.push(json!({ "role": "assistant", "content": text }));
                    }
                } else {
                    messages.push(json!({
                        "role": "assistant",
                        "content": if text.is_empty() { Value::Null } else { json!(text) },
                        "tool_calls": tool_calls,
                    }));
                }
            } else {
                let mut text = String::new();
                let mut images: Vec<Value> = Vec::new();
                for part in &parts {
                    if let Some(value) = part.get("text").and_then(Value::as_str) {
                        text.push_str(value);
                    }
                    if let Some(inline) = part.get("inlineData") {
                        let mime = inline
                            .get("mimeType")
                            .and_then(Value::as_str)
                            .unwrap_or("image/png");
                        let data = inline.get("data").and_then(Value::as_str).unwrap_or("");
                        if !data.is_empty() {
                            images.push(json!({
                                "type": "image_url",
                                "image_url": { "url": format!("data:{mime};base64,{data}") }
                            }));
                        }
                    }
                    if let Some(response) = part.get("functionResponse") {
                        let name = response
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("tool");
                        let id = call_ids
                            .get(name)
                            .cloned()
                            .unwrap_or_else(|| format!("call_{name}_0"));
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": id,
                            "content": response.get("response").cloned().unwrap_or_else(|| json!({})).to_string(),
                        }));
                    }
                }
                if !text.is_empty() || !images.is_empty() {
                    let mut content = Vec::new();
                    if !text.is_empty() {
                        content.push(json!({ "type": "text", "text": text }));
                    }
                    content.extend(images);
                    messages.push(json!({ "role": "user", "content": content }));
                }
            }
        }
    }
    if messages.is_empty() {
        messages.push(json!({ "role": "user", "content": "" }));
    }

    let mut payload = Map::new();
    payload.insert("model".into(), json!(model));
    payload.insert("messages".into(), json!(messages));
    payload.insert("stream".into(), json!(stream));

    if let Some(config) = body.get("generationConfig") {
        for (gemini, openai) in [
            ("temperature", "temperature"),
            ("topP", "top_p"),
            ("maxOutputTokens", "max_tokens"),
        ] {
            if let Some(value) = config.get(gemini) {
                payload.insert(openai.into(), value.clone());
            }
        }
        if let Some(stops) = config.get("stopSequences") {
            payload.insert("stop".into(), stops.clone());
        }
    }

    let mut tools = Vec::new();
    if let Some(items) = body.get("tools").and_then(Value::as_array) {
        for item in items {
            if let Some(declarations) = item.get("functionDeclarations").and_then(Value::as_array) {
                for declaration in declarations {
                    tools.push(json!({
                        "type": "function",
                        "function": {
                            "name": declaration.get("name").cloned().unwrap_or_else(|| json!("tool")),
                            "description": declaration.get("description").cloned().unwrap_or_else(|| json!("")),
                            "parameters": declaration.get("parameters").cloned().unwrap_or_else(|| json!({"type":"object","properties":{}})),
                        }
                    }));
                }
            }
        }
    }
    if !tools.is_empty() {
        payload.insert("tools".into(), json!(tools));
        if let Some(mode) = body
            .get("toolConfig")
            .and_then(|config| config.get("functionCallingConfig"))
            .and_then(|config| config.get("mode"))
            .and_then(Value::as_str)
        {
            payload.insert(
                "tool_choice".into(),
                json!(match mode {
                    "ANY" => "required",
                    "NONE" => "none",
                    _ => "auto",
                }),
            );
        }
    }

    Value::Object(payload)
}

fn parts_text(container: Option<&Value>) -> Option<String> {
    let parts = container?.get("parts")?.as_array()?;
    let mut out = String::new();
    for part in parts {
        if let Some(text) = part.get("text").and_then(Value::as_str) {
            out.push_str(text);
        }
    }
    Some(out)
}

fn openai_to_gemini(value: &Value) -> Value {
    let choice = value.get("choices").and_then(|choices| choices.get(0));
    let message = choice.and_then(|choice| choice.get("message"));
    let mut parts = Vec::new();
    if let Some(text) = message.and_then(|message| message.get("content")).and_then(Value::as_str) {
        if !text.is_empty() {
            parts.push(json!({ "text": text }));
        }
    }
    if let Some(calls) = message
        .and_then(|message| message.get("tool_calls"))
        .and_then(Value::as_array)
    {
        for call in calls {
            let function = call.get("function").cloned().unwrap_or_else(|| json!({}));
            let arguments = function
                .get("arguments")
                .and_then(Value::as_str)
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                .unwrap_or_else(|| json!({}));
            parts.push(json!({
                "functionCall": {
                    "name": function.get("name").cloned().unwrap_or_else(|| json!("tool")),
                    "args": arguments,
                }
            }));
        }
    }
    if parts.is_empty() {
        parts.push(json!({ "text": "" }));
    }
    let finish = match choice
        .and_then(|choice| choice.get("finish_reason"))
        .and_then(Value::as_str)
    {
        Some("length") => "MAX_TOKENS",
        Some("content_filter") => "SAFETY",
        _ => "STOP",
    };
    let usage = value.get("usage").cloned().unwrap_or_else(|| json!({}));
    json!({
        "candidates": [{
            "content": { "role": "model", "parts": parts },
            "finishReason": finish,
            "index": 0,
        }],
        "usageMetadata": {
            "promptTokenCount": usage.get("prompt_tokens").cloned().unwrap_or(json!(0)),
            "candidatesTokenCount": usage.get("completion_tokens").cloned().unwrap_or(json!(0)),
            "totalTokenCount": usage.get("total_tokens").cloned().unwrap_or(json!(0)),
        }
    })
}

// ── Gemini SSE 转换 ─────────────────────────────────────────────────────

#[derive(Default)]
struct ToolAccum {
    name: String,
    args: String,
}

struct GeminiSseReader {
    upstream: BufReader<Box<dyn Read + Send + Sync>>,
    pending: Vec<u8>,
    pos: usize,
    done: bool,
    finish: Option<String>,
    usage: Value,
    tools: BTreeMap<u64, ToolAccum>,
}

impl GeminiSseReader {
    fn new(upstream: Box<dyn Read + Send + Sync>) -> Self {
        Self {
            upstream: BufReader::new(upstream),
            pending: Vec::new(),
            pos: 0,
            done: false,
            finish: None,
            usage: json!({}),
            tools: BTreeMap::new(),
        }
    }

    fn next_chunk(&mut self) -> Option<Vec<u8>> {
        if self.done {
            return None;
        }
        loop {
            let mut line = String::new();
            match self.upstream.read_line(&mut line) {
                Ok(0) => {
                    self.done = true;
                    return Some(self.final_event());
                }
                Ok(_) => {}
                Err(_) => {
                    self.done = true;
                    return Some(self.final_event());
                }
            }
            let line = line.trim_end_matches(['\r', '\n']);
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                self.done = true;
                return Some(self.final_event());
            }
            let Ok(value) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            if let Some(usage) = value.get("usage") {
                self.usage = usage.clone();
            }
            let Some(choice) = value.get("choices").and_then(|choices| choices.get(0)) else {
                continue;
            };
            if let Some(delta) = choice.get("delta") {
                if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for call in calls {
                        let index = call.get("index").and_then(Value::as_u64).unwrap_or(0);
                        let entry = self.tools.entry(index).or_default();
                        if let Some(name) = call
                            .get("function")
                            .and_then(|function| function.get("name"))
                            .and_then(Value::as_str)
                        {
                            if entry.name.is_empty() {
                                entry.name = name.to_string();
                            }
                        }
                        if let Some(args) = call
                            .get("function")
                            .and_then(|function| function.get("arguments"))
                            .and_then(Value::as_str)
                        {
                            entry.args.push_str(args);
                        }
                    }
                }
                if let Some(text) = delta.get("content").and_then(Value::as_str) {
                    if !text.is_empty() {
                        return Some(sse(json!({
                            "candidates": [{
                                "content": { "role": "model", "parts": [{ "text": text }] },
                                "index": 0,
                            }]
                        })));
                    }
                }
            }
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                self.finish = Some(reason.to_string());
            }
        }
    }

    fn final_event(&mut self) -> Vec<u8> {
        let mut parts = Vec::new();
        for tool in self.tools.values() {
            if tool.name.is_empty() {
                continue;
            }
            let args = serde_json::from_str::<Value>(&tool.args).unwrap_or_else(|_| json!({}));
            parts.push(json!({
                "functionCall": { "name": tool.name, "args": args }
            }));
        }
        let finish = match self.finish.as_deref() {
            Some("length") => "MAX_TOKENS",
            Some("content_filter") => "SAFETY",
            _ => "STOP",
        };
        let mut candidate = json!({
            "content": { "role": "model", "parts": if parts.is_empty() { json!([{ "text": "" }]) } else { json!(parts) } },
            "finishReason": finish,
            "index": 0,
        });
        if let Some(object) = candidate.as_object_mut() {
            let _ = object;
        }
        sse(json!({
            "candidates": [candidate],
            "usageMetadata": {
                "promptTokenCount": self.usage.get("prompt_tokens").cloned().unwrap_or(json!(0)),
                "candidatesTokenCount": self.usage.get("completion_tokens").cloned().unwrap_or(json!(0)),
                "totalTokenCount": self.usage.get("total_tokens").cloned().unwrap_or(json!(0)),
            }
        }))
    }
}

fn sse(value: Value) -> Vec<u8> {
    format!("data: {}\n\n", value).into_bytes()
}

impl Read for GeminiSseReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.pos < self.pending.len() {
                let available = &self.pending[self.pos..];
                let take = available.len().min(buf.len());
                buf[..take].copy_from_slice(&available[..take]);
                self.pos += take;
                return Ok(take);
            }
            self.pending.clear();
            self.pos = 0;
            match self.next_chunk() {
                Some(chunk) => self.pending = chunk,
                None => return Ok(0),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_text_and_tool_calls() {
        let body = json!({
            "contents": [{ "role": "user", "parts": [{ "text": "hi" }] }],
            "systemInstruction": { "parts": [{ "text": "be nice" }] },
            "tools": [{ "functionDeclarations": [{ "name": "read", "description": "read a file", "parameters": { "type": "object", "properties": { "path": { "type": "string" } } } }] }],
            "toolConfig": { "functionCallingConfig": { "mode": "ANY" } }
        });
        let payload = gemini_to_openai(&body, "opencode-go/deepseek-v4.1-flash", false);
        assert_eq!(payload["model"], "opencode-go/deepseek-v4.1-flash");
        assert_eq!(payload["messages"][0]["role"], "system");
        assert_eq!(payload["messages"][1]["content"][0]["text"], "hi");
        assert_eq!(payload["tools"][0]["function"]["name"], "read");
        assert_eq!(payload["tool_choice"], "required");
    }

    #[test]
    fn converts_response_and_decodes_model_path() {
        let response = json!({
            "choices": [{ "message": { "content": "done", "tool_calls": [{ "function": { "name": "read", "arguments": "{\"path\":\"C:\\\\x\"}" } }] }, "finish_reason": "tool_calls" }],
            "usage": { "prompt_tokens": 3, "completion_tokens": 4, "total_tokens": 7 }
        });
        let gemini = openai_to_gemini(&response);
        assert_eq!(gemini["candidates"][0]["content"]["parts"][1]["functionCall"]["name"], "read");
        assert_eq!(gemini["usageMetadata"]["totalTokenCount"], 7);

        let path = "/v1beta/models/opencode-go%2Fdeepseek-v4.1-flash:generateContent";
        let (model, stream) = model_from_path(path).unwrap();
        assert_eq!(model, "opencode-go/deepseek-v4.1-flash");
        assert!(!stream);
    }
}
