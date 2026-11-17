//! `/v1/responses` 入站端点（账本 T4，合同 C5）——OpenAI Responses 线协议，
//! codex 的母语；这个端点让 codex 直连 Rust 引擎。
//!
//! 两条路，**直通优先**：
//! - **直通**：provider 的 responses URL 非空 → 请求原样转发
//!   `{responses}/responses`（model 剥 `provider/` 前缀，Bearer 认证 +
//!   来源 headers；`stream:true` 时 SSE 字节级透传——厂商自己说的是
//!   responses 话，翻译是画蛇添足）。
//! - **翻译**：responses URL 空、chat URL 有 →
//!   [`translate::responses_to_chat`] 折成 chat/completions 交给
//!   `chat::forward` 出站（剥前缀 / Bearer / bodycap 挂 4xx / prefit 全在
//!   那条链上——`chat::Outbound` 的字段是模块私有的，翻译路径复用
//!   `chat::forward` 是唯一也是正确路径），再把 chat 应答折回 responses
//!   形状：非流式 JSON 折成 response 对象，流式 SSE 折成事件序列
//!   `response.created → response.output_item.added → output_text.delta…
//!   → output_text.done → output_item.done → response.completed`。
//!
//! codex 特有字段（`store` / `include` / `previous_response_id`）在翻译
//! 路径下**安静丢弃**——引擎没有会话存储（无状态转发），这些字段的服务端
//! 存回合语义没有诚实对应；`instructions` 例外：它就是 system 提示，
//! translate 已折进 chat 的 system 消息。直通路径下这些字段原样透传
//! （厂商自己认）。
//!
//! 流式 usage：给翻译后的 chat 请求补 `stream_options.include_usage`
//! （Go buildChat 同款，chat.go:636）——否则 chat 厂商不在流尾发 usage 帧，
//! completed 事件的 usage 只能报 0。finish_reason 帧先到时**不**立即
//! completed：chat 流的 usage 帧在 finish 之后（include_usage 时是独立
//! 末帧），先封口 usage 就丢了——等 usage 或 [DONE]。
//!
//! 未覆盖（列给账本）：reasoning item（chat 流没有对应物，不编造）、
//! function call 的流式事件（chat 的 tool_calls 分片需要参数拼装状态机，
//! 非流式路径有完整映射）、直通路径的 bodycap refit（丢图逻辑只认 chat
//! 形状的 messages/image_url，对 responses 形状的 input/input_image 找
//! 不到可丢的图——挂上反而把「可救」说成「救过了」）。
//!
//! 参照（只读研究克隆）：magpie internal/gateway/responses.go 的
//! responsesEncoder（事件序列与帧形状）/ renderResponses（非流式应答）。
use std::io::{BufRead, BufReader, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::httpd::Reply;

// =====================================================================
// 小工具
// =====================================================================

/// 请求体里的 model 字段（解析失败/缺失返回空串——400/404 面由调用方出）。
fn asked_model(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(String::from))
        .unwrap_or_default()
}

/// unix 秒（response 对象的 created_at）。
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 自造 id 的后缀：纳秒 + 进程内计数。不引 rand——同一流内只要求不重复，
/// 跨流靠纳秒时钟分开，足够（id 是占位标识，不是安全敏感值）。
fn fresh_suffix() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{:x}-{:x}", nanos, n)
}

/// 把请求体里的 `model` 换成厂商裸名。chat.rs 的 rewrite_model 是模块私有
/// 的——直通路径同样要做这件事（组路由 resolve 出的 bare 才是厂商认的名字），
/// 这里复刻一份同语义实现。
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
// 入口
// =====================================================================

/// POST /v1/responses 入口。解析 model → 组路由 → 直通或翻译。
pub fn handle(gateway: &super::Gateway, body: &[u8]) -> Reply {
    let model = asked_model(body);
    if model.is_empty() {
        return Reply::error(400, "invalid request: model");
    }
    // 锁内只做表查询与字段快照，锁外跑网络（store 锁绝不持着等 HTTP——
    // 2026-10-31 storage 锁事故红线）。
    let (responses_url, key, headers, bare) = {
        let store = match gateway.store.lock() {
            Ok(s) => s,
            Err(_) => return Reply::error(500, "store poisoned"),
        };
        let Some((provider_id, bare)) =
            super::groups::resolve(store.groups(), store.providers(), &model)
        else {
            return Reply::error(404, &format!("no provider for model: {model}"));
        };
        let Some(provider) = store.find(&provider_id) else {
            return Reply::error(404, &format!("no provider for model: {model}"));
        };
        (
            provider.responses.clone(),
            provider.key.clone(),
            provider
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>(),
            bare,
        )
    };
    if !responses_url.is_empty() {
        let streaming = serde_json::from_slice::<Value>(body)
            .ok()
            .and_then(|v| v.get("stream").and_then(|s| s.as_bool()))
            .unwrap_or(false);
        return passthrough(&responses_url, &key, &headers, body, &bare, streaming);
    }
    translate_via_chat(gateway, body)
}

// =====================================================================
// 直通路径
// =====================================================================

/// 原样转发 `{responses}/responses`。SSE（stream:true）字节级透传；
/// 错误（4xx/5xx）原样透传——错误面是厂商与 codex 之间的事，重写只会
/// 丢厂商的原话（bodycap 见模块注释：responses 形状不挂）。
fn passthrough(
    base: &str,
    key: &str,
    headers: &[(String, String)],
    body: &[u8],
    bare: &str,
    streaming: bool,
) -> Reply {
    let url = format!("{}/responses", base.trim_end_matches('/'));
    let out_body = rewrite_model(body, bare);
    let agent = super::outbound_agent();
    let mut request = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json");
    if !key.is_empty() {
        request = request.set("Authorization", &format!("Bearer {}", key));
    }
    for (k, v) in headers {
        request = request.set(k, v);
    }
    let response = match request.send_bytes(&out_body) {
        Ok(r) => r,
        // ureq 2 把非 2xx 当 Err(Status(code, resp))——厂商的 4xx/5xx 是
        // 合同面的一部分，原样取回 response（chat.rs 同款）。
        Err(ureq::Error::Status(_, response)) => response,
        Err(e) => return Reply::error(502, &format!("responses passthrough: {e}")),
    };
    if streaming {
        return Reply::Stream {
            status: response.status(),
            content_type: response.content_type().to_string(),
            reader: Box::new(response.into_reader()),
        };
    }
    let status = response.status();
    let mut buf = Vec::new();
    let _ = response.into_reader().read_to_end(&mut buf);
    Reply::Json { status, body: buf }
}

// =====================================================================
// 翻译路径
// =====================================================================

/// responses → chat 出站（`chat::forward` 全链：resolve / 剥前缀 / Bearer /
/// prefit / bodycap 挂 4xx）→ 应答折回 responses 形状。
fn translate_via_chat(gateway: &super::Gateway, body: &[u8]) -> Reply {
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return Reply::error(400, "invalid request body"),
    };
    // asked 模型串回显进应答的 model 字段（codex 拿它对账自己在用的模型）。
    let asked = parsed
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or_default()
        .to_string();
    let chat_body = match super::translate::responses_to_chat(&parsed) {
        Ok(mut v) => {
            // 流式补 include_usage（Go buildChat 同款）：chat 厂商默认不在
            // 流尾发 usage 帧，completed 事件的 usage 就只能报 0。
            if v.get("stream").and_then(|s| s.as_bool()).unwrap_or(false) {
                if let Some(map) = v.as_object_mut() {
                    map.insert("stream_options".into(), json!({ "include_usage": true }));
                }
            }
            serde_json::to_vec(&v).unwrap_or_default()
        }
        Err(e) => return Reply::error(400, &e),
    };
    reshape(super::chat::forward(gateway, &chat_body), &asked)
}

/// `chat::forward` 的应答 → responses 应答。错误（≥400）原样透传：chat
/// 家族的错误面本就是 `{"error":{…}}` 同构形状，重写只会丢厂商的原话，
/// 状态码透传。
fn reshape(reply: Reply, asked: &str) -> Reply {
    match reply {
        Reply::Json { status, body } => {
            if status < 400 {
                match serde_json::from_slice::<Value>(&body) {
                    Ok(v) => Reply::Json {
                        status,
                        body: serde_json::to_vec(&chat_completion_to_response(&v, asked))
                            .unwrap_or(body),
                    },
                    // 2xx 但不是 JSON：翻译路径的客户端说的是 responses 话，
                    // 原样的 chat 噪音没有意义——诚实报 502。
                    Err(_) => Reply::error(502, "chat vendor replied a non-JSON body"),
                }
            } else {
                Reply::Json { status, body }
            }
        }
        Reply::Stream { status, content_type, mut reader } => {
            if status != 200 {
                // 流式请求吃到错误：chat.rs 把 4xx 也包成 Stream 透传，但
                // 这里的客户端期待 responses SSE——读全量按错误面透传，
                // 别把 chat 的 JSON 错误体伪装成 SSE 流。
                let mut buf = Vec::new();
                let _ = reader.read_to_end(&mut buf);
                return Reply::Json { status, body: buf };
            }
            if !content_type.contains("text/event-stream") {
                // 厂商无视 stream:true 回了整块 JSON：按非流式折（chat.rs
                // 的 Stream 分支只看请求的 stream 字段，这种应答它原样
                // 透传，这里没有诚实透传可言——折成客户要的形状）。
                let mut buf = Vec::new();
                let _ = reader.read_to_end(&mut buf);
                return match serde_json::from_slice::<Value>(&buf) {
                    Ok(v) => Reply::Json {
                        status: 200,
                        body: serde_json::to_vec(&chat_completion_to_response(&v, asked))
                            .unwrap_or(buf),
                    },
                    Err(_) => Reply::error(502, "chat vendor replied a non-JSON body"),
                };
            }
            Reply::Stream {
                status: 200,
                content_type: "text/event-stream".into(),
                reader: Box::new(ResponsesSse::new(reader, asked)),
            }
        }
    }
}

/// chat usage → responses usage。口径：OpenAI 两代的 input_tokens 都含
/// cached（Go `Usage.prompt()` 的注释——"every token the prompt came to,
/// as OpenAI's counts have it"），chat 的 prompt_tokens 同口径，直搬；
/// chat 不报的桶记 0，不编数。
fn usage_to_responses(u: Option<&Value>) -> Value {
    let num = |v: Option<&Value>, k: &str| {
        v.and_then(|x| x.get(k)).and_then(|x| x.as_u64()).unwrap_or(0)
    };
    let input = num(u, "prompt_tokens");
    let output = num(u, "completion_tokens");
    let total = u
        .and_then(|x| x.get("total_tokens"))
        .and_then(|x| x.as_u64())
        .unwrap_or(input + output);
    json!({
        "input_tokens": input,
        "output_tokens": output,
        "total_tokens": total,
        "input_tokens_details": {
            "cached_tokens": u
                .and_then(|x| x.pointer("/prompt_tokens_details/cached_tokens"))
                .and_then(|x| x.as_u64())
                .unwrap_or(0)
        },
        "output_tokens_details": {
            "reasoning_tokens": u
                .and_then(|x| x.pointer("/completion_tokens_details/reasoning_tokens"))
                .and_then(|x| x.as_u64())
                .unwrap_or(0)
        },
    })
}

/// chat message.content（string 或 text 块数组）→ 纯文本。
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => {
            let mut b = String::new();
            for it in items {
                if it.get("type").and_then(|t| t.as_str()) == Some("text") {
                    b.push_str(it.get("text").and_then(|t| t.as_str()).unwrap_or(""));
                }
            }
            b
        }
        _ => String::new(),
    }
}

/// chat completion JSON → response 对象（renderResponses 的最小诚实子集）。
/// output 只含真实出现的东西：正文 → message item；tool_calls →
/// function_call item（codex 工具回路的非流式形态）。finish_reason 的
/// length / content_filter → status incomplete（OpenAI 的语义），其余
/// completed。empty 回合的 output 是空数组——不是 null。
fn chat_completion_to_response(v: &Value, asked: &str) -> Value {
    let mut output: Vec<Value> = Vec::new();
    if let Some(msg) = v.pointer("/choices/0/message") {
        let text = content_text(msg.get("content"));
        if !text.is_empty() {
            output.push(json!({
                "id": format!("msg_{}", fresh_suffix()),
                "type": "message", "role": "assistant", "status": "completed",
                "content": [{ "type": "output_text", "text": text, "annotations": [] }],
            }));
        }
        if let Some(calls) = msg.get("tool_calls").and_then(|c| c.as_array()) {
            for tc in calls {
                output.push(json!({
                    "id": format!("fc_{}", fresh_suffix()),
                    "type": "function_call", "status": "completed",
                    "call_id": tc.get("id").cloned()
                        .unwrap_or_else(|| json!(format!("call_{}", fresh_suffix()))),
                    "name": tc.pointer("/function/name").cloned()
                        .unwrap_or_else(|| json!(String::new())),
                    "arguments": tc.pointer("/function/arguments").cloned()
                        .unwrap_or_else(|| json!("{}")),
                }));
            }
        }
    }
    let finish = v
        .pointer("/choices/0/finish_reason")
        .and_then(|f| f.as_str())
        .unwrap_or("");
    let (status, incomplete): (&str, Value) = match finish {
        "length" => ("incomplete", json!({ "reason": "max_output_tokens" })),
        "content_filter" => ("incomplete", json!({ "reason": "content_filter" })),
        _ => ("completed", Value::Null),
    };
    let model = v
        .get("model")
        .and_then(|m| m.as_str())
        .filter(|m| !m.is_empty())
        .unwrap_or(asked);
    json!({
        "id": format!("resp_{}", fresh_suffix()),
        "object": "response",
        "created_at": now_unix(),
        "status": status,
        "model": model,
        "output": output,
        "usage": usage_to_responses(v.get("usage")),
        "incomplete_details": incomplete,
        "parallel_tool_calls": true,
        "tool_choice": "auto",
        "tools": [],
        "error": null,
    })
}

// =====================================================================
// 流式翻译：chat SSE → responses SSE
// =====================================================================

/// chat SSE → responses SSE 的流式翻译器（实现 Read，直接塞 Reply::Stream）。
///
/// 事件序列（responsesEncoder 的最小子集，帧 `event: <类型>\ndata: {…}\n\n`）：
/// `response.created`（首帧；response.output 是 `[]` 占位——**绝不能是
/// null**，Muse Code 拒收 output 为 null 的流，Go 注释实锤）
/// → `response.output_item.added`（assistant message item）
/// → 每条 chat text delta 一帧 `response.output_text.delta`
/// → `response.output_text.done`（全文）
/// → `response.output_item.done`（完整 item）
/// → `response.completed` / `response.incomplete`（usage + 完整 output）。
///
/// finish_reason 帧到了**不**立即收尾：chat 流的 usage 帧在 finish 之后
/// （include_usage 时是独立末帧），先封口 usage 就丢了——等 usage 或
/// [DONE]。上游 EOF 也没等到的（厂商半途断流），按已知的收尾（Go
/// readSSE 结束后 finish() 同款）：完成序列照发，usage 没有就 0。
struct ResponsesSse {
    inner: BufReader<Box<dyn Read + Send>>,
    /// 待写出的字节（帧队列的物理形态），pos 是已消费位置。
    out: Vec<u8>,
    pos: usize,
    started: bool,
    item_open: bool,
    text: String,
    item_id: String,
    resp_id: String,
    seq: u64,
    model: String,
    finish: Option<String>,
    usage: Option<Value>,
    ended: bool,
}

impl ResponsesSse {
    fn new(reader: Box<dyn Read + Send>, model: &str) -> ResponsesSse {
        ResponsesSse {
            inner: BufReader::new(reader),
            out: Vec::new(),
            pos: 0,
            started: false,
            item_open: false,
            text: String::new(),
            item_id: String::new(),
            resp_id: format!("resp_{}", fresh_suffix()),
            seq: 0,
            model: model.to_string(),
            finish: None,
            usage: None,
            ended: false,
        }
    }

    /// 一帧：`event: <类型>\ndata: {…}\n\n`（sseWriter.event 同款格式），
    /// type 与 sequence_number 写进 data（Go write() 同款）。
    fn frame(&mut self, ty: &str, mut fields: Value) {
        if let Some(map) = fields.as_object_mut() {
            map.insert("type".into(), json!(ty));
            map.insert("sequence_number".into(), json!(self.seq));
        }
        self.seq += 1;
        let data = serde_json::to_string(&fields).unwrap_or_default();
        self.out
            .extend_from_slice(format!("event: {}\ndata: {}\n\n", ty, data).as_bytes());
    }

    /// response 对象（created 的占位 / completed 的完整形态）。
    fn response_object(&self, status: &str, output: Value, usage: Value, incomplete: Value) -> Value {
        json!({
            "id": self.resp_id,
            "object": "response",
            "created_at": now_unix(),
            "status": status,
            "model": self.model,
            "output": output,
            "usage": usage,
            "incomplete_details": incomplete,
            "parallel_tool_calls": true,
            "tool_choice": "auto",
            "tools": [],
        })
    }

    /// 首帧（懒发：第一条有效载荷到来才发——空流也由 finish_stream 兜出
    /// created，保证任何流都有 created + 终结帧这对括号）。
    fn ensure_started(&mut self) {
        if self.started {
            return;
        }
        self.started = true;
        let response = self.response_object("in_progress", json!([]), Value::Null, Value::Null);
        self.frame("response.created", json!({ "response": response }));
    }

    /// message item 的 added（懒发：第一条正文 delta 到来才开 item）。
    fn ensure_item(&mut self) {
        if self.item_open {
            return;
        }
        self.item_open = true;
        self.item_id = format!("msg_{}", fresh_suffix());
        let item_id = self.item_id.clone();
        self.frame(
            "response.output_item.added",
            json!({
                "output_index": 0,
                "item": {
                    "id": item_id,
                    "type": "message", "role": "assistant", "status": "in_progress",
                    "content": [],
                },
            }),
        );
    }

    /// 终结序列：text.done → item.done → completed/incomplete。
    fn finish_stream(&mut self) {
        if self.ended {
            return;
        }
        self.ensure_started(); // 空流也要有 created——completed 不能凭空来
        let (status, event, incomplete): (&str, &str, Value) = match self.finish.as_deref() {
            Some("length") => {
                ("incomplete", "response.incomplete", json!({ "reason": "max_output_tokens" }))
            }
            Some("content_filter") => {
                ("incomplete", "response.incomplete", json!({ "reason": "content_filter" }))
            }
            _ => ("completed", "response.completed", Value::Null),
        };
        let usage = usage_to_responses(self.usage.as_ref());
        let item = json!({
            "id": self.item_id,
            "type": "message", "role": "assistant", "status": "completed",
            "content": [{ "type": "output_text", "text": self.text, "annotations": [] }],
        });
        if self.item_open {
            let item_id = self.item_id.clone();
            self.frame(
                "response.output_text.done",
                json!({
                    "item_id": item_id, "output_index": 0, "content_index": 0,
                    "text": self.text,
                }),
            );
            self.frame("response.output_item.done", json!({ "output_index": 0, "item": item.clone() }));
            let response = self.response_object(status, json!([item]), usage, incomplete);
            self.frame(event, json!({ "response": response }));
            self.item_open = false;
        } else {
            // 没有任何正文（空回合 / 纯工具轮的流式路径——本映射不带工具
            // 事件）：output 空数组，诚实汇报。
            let response = self.response_object(status, json!([]), usage, incomplete);
            self.frame(event, json!({ "response": response }));
        }
        self.ended = true;
    }

    /// 一条 data 行（JSON chunk 或 [DONE]）。
    fn feed(&mut self, data: &str) {
        if data == "[DONE]" {
            self.finish_stream();
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return; // 流里的杂行不是我们的菜，安静跳过（不 forward 错误面）
        };
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            self.usage = Some(u.clone());
        }
        let choice = v.pointer("/choices/0");
        let text = choice
            .and_then(|c| c.pointer("/delta/content"))
            .and_then(|d| d.as_str())
            .unwrap_or("");
        if !text.is_empty() {
            self.ensure_started();
            self.ensure_item();
            self.text.push_str(text);
            let item_id = self.item_id.clone();
            self.frame(
                "response.output_text.delta",
                json!({
                    "item_id": item_id, "output_index": 0, "content_index": 0,
                    "delta": text,
                }),
            );
        }
        if let Some(finish) = choice
            .and_then(|c| c.get("finish_reason"))
            .and_then(|f| f.as_str())
            .filter(|f| !f.is_empty())
        {
            // 只记住，不收尾：usage 帧还在后面（见结构体注释）。
            self.finish = Some(finish.to_string());
        }
    }
}

impl Read for ResponsesSse {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.pos < self.out.len() {
                let n = (self.out.len() - self.pos).min(buf.len());
                buf[..n].copy_from_slice(&self.out[self.pos..self.pos + n]);
                self.pos += n;
                return Ok(n);
            }
            if self.ended {
                return Ok(0);
            }
            self.out.clear();
            self.pos = 0;
            let mut line = String::new();
            let n = self.inner.read_line(&mut line)?;
            if n == 0 {
                // 上游 EOF：没等到 [DONE] 也把已知的收尾（厂商半途断流，
                // Go readSSE 结束后 finish() 同款）。
                self.finish_stream();
                continue;
            }
            let line = line.trim_end();
            let Some(data) = line.strip_prefix("data:") else {
                continue; // 空行 / event: 行 / 注释——chat 流只有 data: 有载荷
            };
            self.feed(data.trim());
        }
    }
}

// =====================================================================
// 测试（quotas.rs 的假厂商写法：std TcpListener 手写 HTTP，零新依赖）
// =====================================================================

#[cfg(test)]
mod tests {
    use super::super::Gateway;
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};

    /// 测试串行锁：配置目录靠 XDG_CONFIG_HOME（进程级 env），并行测试会
    /// 互踩——全局锁串行（quotas.rs / council store 沙箱同款做法）。
    // 统一走 mod.rs 的跨模块共享锁（XDG_CONFIG_HOME 是进程级 env）。
    static DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

    struct Sandbox {
        dir: std::path::PathBuf,
        prior_xdg: Option<String>,
        #[allow(dead_code)]
        lock: MutexGuard<'static, ()>,
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            if let Some(prior) = &self.prior_xdg {
                std::env::set_var("XDG_CONFIG_HOME", prior);
            } else {
                std::env::remove_var("XDG_CONFIG_HOME");
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// 一次测试的世界：%TEMP% 下唯一配置目录 + env 恢复 + 串行锁。
    /// 绝不碰真实 `~/.config/magpie`。
    fn sandbox(providers_json: &str) -> (Gateway, Sandbox) {
        let lock = super::super::TEST_SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "marionette-engine-responses-{}-{}",
            std::process::id(),
            DIR_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("magpie")).expect("temp config dir");
        std::fs::write(dir.join("magpie").join("providers.json"), providers_json)
            .expect("providers.json");
        let prior_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let gateway = Gateway::open().expect("gateway");
        (gateway, Sandbox { dir, prior_xdg, lock })
    }

    /// 假厂商收到的一次请求。
    struct Seen {
        path: String,
        authorization: String,
        body: Vec<u8>,
    }

    /// POST 型假厂商：记 path / Authorization / 请求体，回固定应答
    /// （Connection: close + 精确 Content-Length：ureq 一请求一连接，
    /// 计数器才准——quotas.rs 同款）。
    struct FakeVendor {
        base: String,
        hits: Arc<AtomicUsize>,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    fn status_line(status: u16) -> String {
        format!("{} {}", status, if status == 200 { "OK" } else { "Err" })
    }

    fn fake_vendor(status: u16, content_type: &'static str, reply_body: &'static str) -> FakeVendor {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake vendor");
        let port = listener.local_addr().expect("port").port();
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let thread_hits = hits.clone();
        let thread_seen = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                thread_hits.fetch_add(1, Ordering::SeqCst);
                let mut reader = BufReader::new(match stream.try_clone() {
                    Ok(s) => s,
                    Err(_) => continue,
                });
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
                    continue;
                }
                let path = request_line.split_whitespace().nth(1).unwrap_or("").to_string();
                let mut authorization = String::new();
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
                    if let Some(v) = trimmed.strip_prefix("Authorization:") {
                        authorization = v.trim().to_string();
                    }
                    if let Some(v) = trimmed.strip_prefix("Content-Length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                if length > 0 {
                    let _ = reader.read_exact(&mut body);
                }
                thread_seen
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(Seen { path, authorization, body });
                let mut stream = stream;
                let reply = format!(
                    "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    status_line(status),
                    content_type,
                    reply_body.len(),
                    reply_body
                );
                let _ = stream.write_all(reply.as_bytes());
                let _ = stream.flush();
            }
        });
        FakeVendor {
            base: format!("http://127.0.0.1:{port}"),
            hits,
            seen,
        }
    }

    /// 解析我们产出的 SSE：每帧 (event 名, data JSON)。没有 event: 行的
    /// 帧被丢弃——对 event 行的断言天然检查帧格式。
    fn parse_frames(sse: &str) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        for frame in sse.split("\n\n") {
            let mut name = String::new();
            let mut data = String::new();
            for line in frame.lines() {
                if let Some(v) = line.strip_prefix("event: ") {
                    name = v.to_string();
                }
                if let Some(v) = line.strip_prefix("data: ") {
                    data = v.to_string();
                }
            }
            if !name.is_empty() && !data.is_empty() {
                out.push((name, serde_json::from_str(&data).expect("frame data json")));
            }
        }
        out
    }

    #[test]
    fn translated_non_streaming() {
        // 假 chat 厂商回一条固定 completion（含 cached_tokens 的 usage）。
        let vendor = fake_vendor(
            200,
            "application/json",
            r#"{"id":"chatcmpl-1","object":"chat.completion","model":"m",
                "choices":[{"index":0,"message":{"role":"assistant","content":"hello there"},
                "finish_reason":"stop"}],
                "usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10,
                    "prompt_tokens_details":{"cached_tokens":2}}}"#,
        );
        let (gateway, _sandbox) = sandbox(&format!(
            r#"{{"providers":[{{"id":"p","name":"P","key":"sk-test","chat":"{}/v1"}}]}}"#,
            vendor.base
        ));
        // codex 形态的请求：instructions + 文本 input + 特有字段（store/
        // include/previous_response_id 翻译路径下安静丢弃）。
        let body = br#"{"model":"p/m","instructions":"be nice","input":"hi",
            "store":true,"include":["usage"],"previous_response_id":"resp_old"}"#;
        let reply = handle(&gateway, body);

        // 打到 chat 厂商：Bearer、路径、翻译后的形状。
        assert_eq!(vendor.hits.load(Ordering::SeqCst), 1);
        let seen = vendor.seen.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(seen[0].path, "/v1/chat/completions");
        assert_eq!(seen[0].authorization, "Bearer sk-test");
        let sent: Value = serde_json::from_slice(&seen[0].body).unwrap();
        assert_eq!(sent["model"], "m", "provider/ 前缀已剥");
        assert_eq!(sent["messages"][0], json!({"role": "system", "content": "be nice"}));
        assert_eq!(sent["messages"][1], json!({"role": "user", "content": "hi"}));
        assert!(sent.get("store").is_none(), "codex 特有字段不进 chat 请求");

        // 应答折成 responses 形状。
        let Reply::Json { status, body } = reply else { panic!("json reply") };
        assert_eq!(status, 200);
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["object"], "response");
        assert_eq!(v["status"], "completed");
        assert!(v["id"].as_str().unwrap().starts_with("resp_"));
        assert_eq!(v["output"][0]["type"], "message");
        assert_eq!(v["output"][0]["role"], "assistant");
        assert_eq!(v["output"][0]["content"][0]["type"], "output_text");
        assert_eq!(v["output"][0]["content"][0]["text"], "hello there");
        assert_eq!(v["usage"]["input_tokens"], 7);
        assert_eq!(v["usage"]["input_tokens_details"]["cached_tokens"], 2);
        assert_eq!(v["usage"]["total_tokens"], 10);
    }

    #[test]
    fn translated_streaming_event_sequence() {
        // 两条 SSE：text delta → finish+usage → [DONE]（include_usage 语义）。
        let sse = concat!(
            r#"data: {"id":"chatcmpl-9","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#, "\n\n",
            r#"data: {"id":"chatcmpl-9","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"content":"你好"},"finish_reason":null}]}"#, "\n\n",
            r#"data: {"id":"chatcmpl-9","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}}"#, "\n\n",
            "data: [DONE]\n\n"
        );
        let vendor = fake_vendor(200, "text/event-stream", sse);
        let (gateway, _sandbox) = sandbox(&format!(
            r#"{{"providers":[{{"id":"p","name":"P","key":"sk-test","chat":"{}/v1"}}]}}"#,
            vendor.base
        ));
        let reply = handle(&gateway, br#"{"model":"p/m","input":"hi","stream":true}"#);

        // 发给厂商的 chat 请求补了 include_usage（Go buildChat 同款）。
        let seen = vendor.seen.lock().unwrap_or_else(|p| p.into_inner());
        let sent: Value = serde_json::from_slice(&seen[0].body).unwrap();
        assert_eq!(sent["stream_options"]["include_usage"], true);
        assert_eq!(sent["model"], "m");

        // 应答：responses 事件流，帧格式带 event: 行。
        let Reply::Stream { status, content_type, mut reader } = reply else { panic!("stream reply") };
        assert_eq!(status, 200);
        assert_eq!(content_type, "text/event-stream");
        let mut out = String::new();
        reader.read_to_string(&mut out).expect("read sse");
        assert!(out.starts_with("event: response.created\ndata: "), "帧格式：event 行在前");
        let events = parse_frames(&out);
        let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "response.created",
                "response.output_item.added",
                "response.output_text.delta",
                "response.output_text.done",
                "response.output_item.done",
                "response.completed",
            ],
            "最小事件序列（created → added → delta → done×2 → completed）"
        );
        // created：占位 output 是数组不是 null（Muse Code 拒 null）。
        assert_eq!(events[0].1["response"]["status"], "in_progress");
        assert_eq!(events[0].1["response"]["output"], json!([]));
        // delta 增量。
        assert_eq!(events[2].1["delta"], "你好");
        // text.done 带全文。
        assert_eq!(events[3].1["text"], "你好");
        // completed：完整 output 与 usage（finish 帧先到，等到了 usage 才封口）。
        let completed = &events[5].1["response"];
        assert_eq!(completed["status"], "completed");
        assert_eq!(completed["output"][0]["content"][0]["text"], "你好");
        assert_eq!(completed["usage"]["input_tokens"], 5);
        assert_eq!(completed["usage"]["output_tokens"], 2);
        assert_eq!(completed["usage"]["total_tokens"], 7);
    }

    #[test]
    fn passthrough_prefers_responses_url_with_bearer() {
        // responses URL 与 chat URL 都配了：直通优先，打到 responses 那家。
        let responses_vendor = fake_vendor(
            200,
            "application/json",
            r#"{"id":"resp_vendor","object":"response","status":"completed"}"#,
        );
        let chat_vendor = fake_vendor(
            200,
            "application/json",
            r#"{"id":"chatcmpl_trap","object":"chat.completion"}"#,
        );
        let (gateway, _sandbox) = sandbox(&format!(
            r#"{{"providers":[{{"id":"o","name":"O","key":"sk-resp",
                "chat":"{0}/v1","responses":"{1}/v1"}}]}}"#,
            chat_vendor.base, responses_vendor.base
        ));
        let reply = handle(&gateway, br#"{"model":"o/m2","input":"hi"}"#);

        assert_eq!(responses_vendor.hits.load(Ordering::SeqCst), 1);
        assert_eq!(chat_vendor.hits.load(Ordering::SeqCst), 0, "直通优先，chat 家不该被碰到");
        let seen = responses_vendor.seen.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(seen[0].path, "/v1/responses");
        assert_eq!(seen[0].authorization, "Bearer sk-resp");
        let sent: Value = serde_json::from_slice(&seen[0].body).unwrap();
        assert_eq!(sent["model"], "m2", "直通也剥 provider/ 前缀");

        // 应答原样（不折形状、不翻译）。
        let Reply::Json { status, body } = reply else { panic!("json reply") };
        assert_eq!(status, 200);
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["id"], "resp_vendor");
    }

    #[test]
    fn passthrough_stream_sse_unchanged() {
        let sse = "event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_x\"}}\n\ndata: [DONE]\n\n";
        let vendor = fake_vendor(200, "text/event-stream", sse);
        let (gateway, _sandbox) = sandbox(&format!(
            r#"{{"providers":[{{"id":"o","name":"O","key":"k","responses":"{}/v1"}}]}}"#,
            vendor.base
        ));
        let reply = handle(&gateway, br#"{"model":"o/m","input":"hi","stream":true}"#);
        let Reply::Stream { status, content_type, mut reader } = reply else { panic!("stream reply") };
        assert_eq!(status, 200);
        assert!(content_type.contains("text/event-stream"));
        let mut out = String::new();
        reader.read_to_string(&mut out).expect("read sse");
        assert_eq!(out, sse, "SSE 字节级透传：一个字节都不动");
    }

    #[test]
    fn unknown_model_404() {
        let (gateway, _sandbox) = sandbox(r#"{"providers":[]}"#);
        let reply = handle(&gateway, br#"{"model":"nope/x","input":"hi"}"#);
        let Reply::Json { status, body } = reply else { panic!("json reply") };
        assert_eq!(status, 404);
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert!(v["error"]["message"].as_str().unwrap().contains("nope/x"));
    }
}
