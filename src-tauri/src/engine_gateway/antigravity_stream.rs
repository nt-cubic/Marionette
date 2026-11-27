//! Google Code Assist（Antigravity 账号）的线协议·应答侧。照 magpie
//! `internal/gateway/codeassist.go` 的 codeAssistDecoder（:1627–:1772）逐条移植：
//! Code Assist 的流是 Gemini 分块，每块套在 `{response}` 里（偶尔不套、直接是
//! 分块本身，或是一个 `{error}`）。这里把它解码成标准的 chat 流（OpenAI 分块
//! 形状），三个入站协议的翻译都复用引擎里现成的那一套：
//!
//! - chat 客户端：原样；
//! - responses 客户端：`responses::ResponsesSse`；
//! - anthropic 客户端：`messages::AnthropicSseTranslator`。
//!
//! 解出来的事件（Go 的 Event 种类 → chat 分块）：
//!
//! | Go | chat 分块 |
//! |---|---|
//! | KThink | `delta.reasoning_content` |
//! | KText | `delta.content` |
//! | KToolStart + KToolArgs | 一帧完整的 `delta.tool_calls` |
//! | KStop | `finish_reason` |
//! | KUsage | 末尾独立的 usage 帧 |
//! | KError | `{"error":{…}}` 帧 |
//!
//! ## 与 Go 的差距，如实列出
//!
//! - **图片模型画的图**（KImage）与 **Google 搜索的引用**（KSearch）不发：
//!   chat 流里没有它们的标准位置，客户端也用不上。
//! - **思考签名 / 正文签名**（KSig / KTextSig）不发：请求侧不回传签名。
//! - **开头的 prompt 计数**（KStart 带的 Usage）不发：chat 流没有「开头报
//!   预算」的帧；anthropic 的 message_start 是翻译器出的静态帧，所有 chat 厂商
//!   都一样是 0——引擎既有口径，不在这里单独立例。
//! - **流中途的错误**对 anthropic / chat 客户端是 error 帧；对 responses 客户端
//!   被现成的 `ResponsesSse` 安静跳过（引擎里 chat→responses 翻译路径对所有
//!   chat 厂商都如此，不是这里独有）。

use std::io::{self, BufRead, BufReader, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::antigravity_codeassist::{text_call, tool_of_call, CALL_MARKER, MAX_HELD};

/// 一个回合的 token 账（Go Usage，:1666）：input 不含缓存读的那部分。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Usage {
    pub input: u64,
    pub cache_read: u64,
    pub output: u64,
    pub reasoning: u64,
}

impl Usage {
    /// OpenAI chat 口径：prompt_tokens 含缓存命中，completion 含思考。
    fn chat(self) -> Value {
        json!({
            "prompt_tokens": self.input + self.cache_read,
            "completion_tokens": self.output,
            "total_tokens": self.input + self.cache_read + self.output,
            "prompt_tokens_details": { "cached_tokens": self.cache_read },
            "completion_tokens_details": { "reasoning_tokens": self.reasoning },
        })
    }
}

/// 一路流攒下来的东西（非流式客户端要的整块应答）。
#[derive(Clone, Debug, Default)]
pub(crate) struct Aggregated {
    pub text: String,
    pub think: String,
    /// `{"id","type":"function","function":{"name","arguments"}}`
    pub calls: Vec<Value>,
    /// chat 口径的 finish_reason。
    pub finish: Option<String>,
    pub usage: Option<Usage>,
    pub error: Option<(u16, String)>,
}

/// 读 Code Assist 的流、写 chat 流（`Read` 适配器）。
pub(crate) struct ChatStream {
    inner: BufReader<Box<dyn Read + Send>>,
    decoder: Decoder,
    pos: usize,
}

impl ChatStream {
    pub(crate) fn new(reader: Box<dyn Read + Send>, asked: &str) -> ChatStream {
        ChatStream {
            inner: BufReader::new(reader),
            decoder: Decoder::new(asked),
            pos: 0,
        }
    }

    /// 整路读到底（非流式客户端）：攒出来的整块应答。
    pub(crate) fn into_completion(self) -> Value {
        self.into_result().0
    }

    /// 整路读到底：整块应答，和流中途的 error 事件（状态 + 原话；没有 = None）。
    pub(crate) fn into_result(mut self) -> (Value, Option<(u16, String)>) {
        let mut sink = Vec::new();
        if self.read_to_end(&mut sink).is_err() {
            self.decoder.fail("Antigravity: stream interrupted".into());
        }
        (self.decoder.completion(), self.decoder.total.error.clone())
    }

    /// 流内错误出口前先过这道改写（账号侧把被拒的 429 说成 400 + 提示）。
    pub(crate) fn with_error_hook(
        mut self,
        hook: Box<dyn FnMut(u16, String) -> (u16, String) + Send>,
    ) -> ChatStream {
        self.decoder.hook = Some(hook);
        self
    }
}

impl Read for ChatStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.pos < self.decoder.out.len() {
                let n = buf.len().min(self.decoder.out.len() - self.pos);
                buf[..n].copy_from_slice(&self.decoder.out.as_bytes()[self.pos..self.pos + n]);
                self.pos += n;
                if self.pos == self.decoder.out.len() {
                    self.decoder.out.clear();
                    self.pos = 0;
                }
                return Ok(n);
            }
            if self.decoder.done {
                return Ok(0);
            }
            let mut line = String::new();
            match self.inner.read_line(&mut line) {
                Ok(0) => self.decoder.eof(),
                Ok(_) => {
                    if let Some(data) = line.strip_prefix("data:") {
                        self.decoder.decode(data.trim());
                    }
                }
                Err(e) => self
                    .decoder
                    .fail(format!("Antigravity: stream interrupted: {e}")),
            }
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn fresh_id(prefix: &str) -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mixed = (nanos as u64)
        ^ (SEQ
            .fetch_add(1, Ordering::Relaxed)
            .wrapping_mul(0x9e37_79b9_7f4a_7c15))
        ^ ((std::process::id() as u64) << 32);
    format!("{prefix}{mixed:x}")
}

/// Gemini 的 finishReason → Go 的收尾词（stopFromGemini，:2061）。
fn stop_from_gemini(s: &str) -> &'static str {
    match s {
        "MAX_TOKENS" => "length",
        "STOP"
        | "FINISH_REASON_UNSPECIFIED"
        | "OTHER"
        | "MALFORMED_FUNCTION_CALL"
        | "UNEXPECTED_TOOL_CALL" => "stop",
        // SAFETY、RECITATION、BLOCKLIST、PROHIBITED_CONTENT、SPII……
        _ => "filter",
    }
}

/// Go 的收尾词 → chat 的 finish_reason（有工具调用的回合是 tool_calls）。
fn chat_finish(stop: &str, tools: bool) -> &'static str {
    match stop {
        "length" => "length",
        "tool" => "tool_calls",
        "filter" => "content_filter",
        _ if tools => "tool_calls",
        _ => "stop",
    }
}

/// 流里一个 error 对象自己报的状态（namedStatusOf 的子集，fallback.go:4853）：
/// `status` 优先、再 `code`，400–599 才算。
fn named_status(err: &Value) -> Option<u16> {
    let num = |v: Option<&Value>| {
        v.and_then(Value::as_i64)
            .filter(|n| (400..=599).contains(n))
            .map(|n| n as u16)
    };
    num(err.get("status")).or_else(|| num(err.get("code")))
}

/// Google 的 codeExecution 跑了什么、给了什么，当正文发（ranCode，:1777）：没有
/// 别的 API 有「Google 自己跑的代码」这种部件，不发答案里就像没有过。
fn ran_code(p: &Value) -> String {
    if let Some(c) = p.get("executableCode") {
        let language = c
            .get("language")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase();
        let code = c
            .get("code")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim_end_matches('\n');
        return format!("\n```{language}\n{code}\n```\n");
    }
    let r = p.get("codeExecutionResult");
    let mut out = String::new();
    let outcome = r
        .and_then(|r| r.get("outcome"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if !outcome.is_empty() && outcome != "OUTCOME_OK" {
        out.push_str(&format!("\n{outcome}:"));
    }
    let printed = r
        .and_then(|r| r.get("output"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim_end_matches('\n');
    if !printed.is_empty() {
        out.push_str(&format!("\n```\n{printed}\n```\n"));
    }
    out
}

/// 一路 Code Assist 流的解码器：喂它 `data:` 行后面的 JSON，它往 `out` 里写
/// chat 流，同时攒一份 `total`。
pub(crate) struct Decoder {
    asked: String,
    id: String,
    created: u64,
    started: bool,
    tools: bool,
    stopped: bool,
    /// 这一回合的账：stop 之后才发。
    usage: Option<Usage>,
    /// 一段可能是调用开头的正文，压到确认是或不是为止（见 `text`）。
    held: String,
    tool_index: usize,
    /// 待写出的 chat 流字节。
    out: String,
    done: bool,
    total: Aggregated,
    /// 流内错误出口前的改写（账号侧：被拒的 429 → 400 + 提示）。
    hook: Option<Box<dyn FnMut(u16, String) -> (u16, String) + Send>>,
}

impl Decoder {
    fn new(asked: &str) -> Decoder {
        Decoder {
            asked: asked.to_string(),
            id: String::new(),
            created: now_unix(),
            started: false,
            tools: false,
            stopped: false,
            usage: None,
            held: String::new(),
            tool_index: 0,
            out: String::new(),
            done: false,
            total: Aggregated::default(),
            hook: None,
        }
    }

    /// 攒出来的整块 chat completion（非流式客户端）。
    pub(crate) fn completion(&self) -> Value {
        let mut message = json!({ "role": "assistant", "content": self.total.text });
        if !self.total.think.is_empty() {
            message["reasoning_content"] = json!(self.total.think);
        }
        if !self.total.calls.is_empty() {
            message["tool_calls"] = Value::Array(self.total.calls.clone());
        }
        let finish = if self.total.error.is_some() && self.total.finish.is_none() {
            "stop"
        } else {
            self.total.finish.as_deref().unwrap_or("stop")
        };
        json!({
            "id": self.chunk_id(),
            "object": "chat.completion",
            "created": self.created,
            "model": self.asked,
            "choices": [{ "index": 0, "message": message, "finish_reason": finish }],
            "usage": self.total.usage.unwrap_or_default().chat(),
        })
    }

    fn chunk_id(&self) -> String {
        if self.id.is_empty() {
            "chatcmpl-antigravity".to_string()
        } else {
            self.id.clone()
        }
    }

    /// 一条 chat 分块（`delta` 给了就带 choices；`finish` 给了就填 finish_reason）。
    fn chunk(&mut self, delta: Value, finish: Option<&str>) {
        let data = json!({
            "id": self.chunk_id(),
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.asked,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
        });
        self.raw(&format!("data: {data}\n\n"));
    }

    fn raw(&mut self, s: &str) {
        self.out.push_str(s);
    }

    fn delta(&mut self, key: &str, text: &str) {
        self.chunk(json!({ key: text }), None);
    }

    /// 压着的正文按原文放出去：接下来的不是它的延续（flush，:1875）。
    fn flush(&mut self) {
        if !self.held.is_empty() {
            let held = std::mem::take(&mut self.held);
            self.total.text.push_str(&held);
            self.delta("content", &held);
        }
    }

    /// 一个调用（KToolStart + KToolArgs）：一帧完整的 tool_calls。
    fn tool(&mut self, id: String, name: String, args: String) {
        self.tools = true;
        let index = self.tool_index;
        self.tool_index += 1;
        self.total.calls.push(json!({ "id": id, "type": "function", "function": { "name": name, "arguments": args } }));
        self.chunk(json!({ "tool_calls": [{ "index": index, "id": id, "type": "function", "function": { "name": name, "arguments": args } }] }), None);
    }

    /// 回合收尾（KStop）：finish_reason 帧。
    fn stop(&mut self, reason: &str) {
        self.stopped = true;
        let finish = chat_finish(reason, self.tools);
        self.total.finish = Some(finish.to_string());
        self.chunk(json!({}), Some(finish));
    }

    fn usage_frame(&mut self, usage: &Usage) {
        self.total.usage = Some(*usage);
        let data = json!({
            "id": self.chunk_id(),
            "object": "chat.completion.chunk",
            "created": self.created,
            "model": self.asked,
            "choices": [],
            "usage": usage.chat(),
        });
        self.raw(&format!("data: {data}\n\n"));
    }

    /// 回合出错（KError）：error 帧 + `[DONE]`，流到此为止。Antigravity 把某些
    /// 系统提示词一律回 429，那是「被拒」不是额度（账号侧据此附提示）。
    pub(crate) fn fail(&mut self, message: String) {
        self.fail_with(0, message);
    }

    fn fail_with(&mut self, status: u16, message: String) {
        if self.done {
            return;
        }
        let (status, message) = match self.hook.as_mut() {
            Some(hook) => hook(status, message),
            None => (status, message),
        };
        if !self.started {
            self.begin("");
        }
        self.total.error = Some((status, message.clone()));
        let mut error = json!({ "message": message, "type": "api_error", "param": null });
        if status != 0 {
            error["code"] = json!(status);
        }
        self.raw(&format!("data: {}\n\n", json!({ "error": error })));
        self.raw("data: [DONE]\n\n");
        self.done = true;
    }

    /// 第一块到的时候（KStart）：记下 id、发 role 帧。
    fn begin(&mut self, id: &str) {
        self.started = true;
        self.id = if id.is_empty() {
            format!("chatcmpl-{}", fresh_id(""))
        } else {
            id.to_string()
        };
        self.chunk(json!({ "role": "assistant" }), None);
    }

    /// 一行 `data:` 后面的内容（decode，:1640）。
    pub(crate) fn decode(&mut self, data: &str) {
        if self.done {
            return;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            // 读不懂的行安静跳过，绝不中断流
            return;
        };
        if let Some(err) = v.get("error").filter(|e| e.is_object()) {
            let message = err
                .get("message")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty())
                .unwrap_or("Antigravity stream error")
                .to_string();
            self.fail_with(named_status(err).unwrap_or(0), message);
            return;
        }
        let c = v.get("response").filter(|r| r.is_object()).unwrap_or(&v);
        if let Some(u) = c.get("usageMetadata").and_then(Value::as_object) {
            let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
            let (prompt, candidates, thoughts, cached) = (
                n("promptTokenCount"),
                n("candidatesTokenCount"),
                n("thoughtsTokenCount"),
                n("cachedContentTokenCount"),
            );
            self.usage = Some(Usage {
                input: prompt.saturating_sub(cached),
                cache_read: cached,
                output: candidates + thoughts,
                reasoning: thoughts,
            });
        }
        if !self.started {
            self.begin(c.get("responseId").and_then(Value::as_str).unwrap_or(""));
        }
        if let Some(candidates) = c.get("candidates").and_then(Value::as_array) {
            for cand in candidates {
                self.candidate(cand);
            }
        }
        self.after();
    }

    fn candidate(&mut self, cand: &Value) {
        for p in cand
            .pointer("/content/parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(fc) = p.get("functionCall").filter(|f| f.is_object()) {
                self.flush();
                // 模型没给 id 的调用自己起一个
                let id = fc
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .unwrap_or_else(|| format!("call_{}", fresh_id("")));
                let name = tool_of_call(fc.get("name").and_then(Value::as_str).unwrap_or(""));
                // 没有参数、或就是 null，按 `{}` 发
                let args = fc
                    .get("args")
                    .filter(|a| !a.is_null())
                    .map(Value::to_string)
                    .unwrap_or_else(|| "{}".into());
                self.tool(id, name, args);
            } else if p.get("thought").and_then(Value::as_bool) == Some(true) {
                self.flush();
                if let Some(t) = p
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|t| !t.is_empty())
                {
                    self.total.think.push_str(t);
                    self.delta("reasoning_content", t);
                }
            } else if p
                .get("inlineData")
                .and_then(|d| d.get("data"))
                .and_then(Value::as_str)
                .is_some_and(|d| !d.is_empty())
            {
                // 画图模型画的图：chat 流里没有它的位置（见头注差距）
                self.flush();
            } else if p.get("executableCode").is_some() || p.get("codeExecutionResult").is_some() {
                let text = ran_code(p);
                if !text.is_empty() {
                    self.text(&text);
                }
            } else if let Some(text) = p
                .get("text")
                .and_then(Value::as_str)
                .filter(|t| !t.is_empty())
            {
                self.text(text);
            }
        }
        let reason = cand
            .get("finishReason")
            .and_then(Value::as_str)
            .unwrap_or("");
        if reason.is_empty() || self.stopped {
            return;
        }
        self.flush();
        // 模型发了调用、Google 不认（「Malformed function call:
        // call:default_api:Name{…}」）：调用整个在那段话里，客户端能跑。不管它，
        // 回复读起来是空的，可 token 已经花了（fwxkogaia on X）。
        if !self.tools && matches!(reason, "MALFORMED_FUNCTION_CALL" | "UNEXPECTED_TOOL_CALL") {
            let message = cand
                .get("finishMessage")
                .and_then(Value::as_str)
                .unwrap_or("");
            if let Some(i) = message.find(CALL_MARKER) {
                let (name, args, n) = text_call(&message[i..]);
                if n > 0 {
                    let id = format!("call_{}", fresh_id(""));
                    self.tool(id, name, args.unwrap_or_else(|| "{}".into()));
                }
            }
        }
        let mut stop = stop_from_gemini(reason).to_string();
        if self.tools && stop == "stop" {
            stop = "tool".into();
        }
        self.stop(&stop);
    }

    /// 收尾之后的账（:1767）：stop 了、账有了，就发 usage 帧和 `[DONE]`。
    fn after(&mut self) {
        if self.stopped && !self.done {
            if let Some(usage) = self.usage.take() {
                self.usage_frame(&usage);
                self.raw("data: [DONE]\n\n");
                self.done = true;
            }
        }
    }

    /// 上游的流到头了：压着的按原文放；没收尾的按已知的收（Go readSSE 结束后的
    /// finish() 同款）。
    pub(crate) fn eof(&mut self) {
        if self.done {
            return;
        }
        if !self.started {
            self.begin("");
        }
        self.flush();
        if !self.stopped {
            self.stop("stop");
        }
        // usage 没等到就是没有：照实发一帧 0，客户端的账不缺帧。
        let usage = self.usage.take().unwrap_or_default();
        self.usage_frame(&usage);
        self.raw("data: [DONE]\n\n");
        self.done = true;
    }

    /// 正文往后传，里面写出来的调用（`call:default_api:tool{…}`）变成真调用
    /// （text，:1835）。开头看着像、还没到齐的先压着。
    fn text(&mut self, s: &str) {
        let mut s = format!("{}{}", self.held, s);
        self.held.clear();
        while !s.is_empty() {
            let Some(i) = s.find(CALL_MARKER) else {
                // 结尾可能是标记的开头：留给下一段
                let mut k = CALL_MARKER.len() - 1;
                while k > 0 && !s.ends_with(&CALL_MARKER[..k]) {
                    k -= 1;
                }
                let cut = s.len() - k;
                if cut > 0 {
                    let head = s[..cut].to_string();
                    self.total.text.push_str(&head);
                    self.delta("content", &head);
                }
                self.held = s[cut..].to_string();
                return;
            };
            if i > 0 {
                let head = s[..i].to_string();
                self.total.text.push_str(&head);
                self.delta("content", &head);
            }
            s = s[i..].to_string();
            let (name, args, n) = text_call(&s);
            if n == 0 && s.len() < MAX_HELD {
                // 还没到齐
                self.held = s;
                return;
            } else if n <= 0 {
                // 不是调用
                self.total.text.push_str(CALL_MARKER);
                self.delta("content", CALL_MARKER);
                s = s[CALL_MARKER.len()..].to_string();
            } else {
                let id = format!("call_{}", fresh_id(""));
                self.tool(id, name, args.unwrap_or_else(|| "{}".into()));
                s = s[n as usize..].to_string();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 把一路 SSE 文本喂给解码器，收集写出的 chat 流的 data 行。
    fn run(asked: &str, sse: &str) -> (Vec<String>, ChatStream) {
        let mut stream = ChatStream::new(Box::new(io::Cursor::new(sse.to_string())), asked);
        let mut out = Vec::new();
        stream.read_to_end(&mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        let lines = text
            .lines()
            .filter_map(|l| l.strip_prefix("data: "))
            .map(String::from)
            .collect();
        (lines, stream)
    }

    /// 一段 Gemini 分块套进 Code Assist 的流（`data: {"response": …}}`）。
    fn wrapped(body: &str) -> String {
        format!("data: {{\"response\": {body}}}\n\n")
    }

    fn json_lines(lines: &[String]) -> Vec<Value> {
        lines
            .iter()
            .filter(|l| *l != "[DONE]")
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn thought_text_stop_and_usage_become_chat_chunks() {
        let sse = format!(
            "{}{}",
            wrapped(
                r#"{"responseId":"resp_1","candidates":[{"content":{"parts":[{"text":"thinking…","thought":true}]}}],"usageMetadata":{"promptTokenCount":11,"thoughtsTokenCount":3,"cachedContentTokenCount":4}}"#
            ),
            wrapped(
                r#"{"candidates":[{"content":{"parts":[{"text":"Hello"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":11,"candidatesTokenCount":2,"thoughtsTokenCount":3,"cachedContentTokenCount":4}}"#
            ),
        );
        let (lines, stream) = run("antigravity/gemini-3.7-flash", &sse);
        let data = json_lines(&lines);
        assert_eq!(lines.last().unwrap(), "[DONE]");
        assert_eq!(data.len(), 5, "{lines:#?}");
        assert_eq!(data[0]["id"], "resp_1");
        assert_eq!(data[0]["object"], "chat.completion.chunk");
        assert_eq!(data[0]["model"], "antigravity/gemini-3.7-flash");
        assert_eq!(data[0]["choices"][0]["delta"]["role"], "assistant");
        // 思考进 reasoning_content，正文进 content
        assert_eq!(
            data[1]["choices"][0]["delta"]["reasoning_content"],
            "thinking…"
        );
        assert_eq!(data[2]["choices"][0]["delta"]["content"], "Hello");
        // finish → stop（没有工具调用）
        assert_eq!(data[3]["choices"][0]["finish_reason"], "stop");
        // usage：prompt_tokens 含缓存读；completion 含思考
        assert_eq!(data[4]["usage"]["prompt_tokens"], 11);
        assert_eq!(
            data[4]["usage"]["prompt_tokens_details"]["cached_tokens"],
            4
        );
        assert_eq!(data[4]["usage"]["completion_tokens"], 5);
        assert_eq!(
            data[4]["usage"]["completion_tokens_details"]["reasoning_tokens"],
            3
        );
        assert_eq!(stream.decoder.total.text, "Hello");
        assert_eq!(stream.decoder.total.think, "thinking…");
    }

    #[test]
    fn function_call_parts_become_tool_calls_and_finish_is_tool_calls() {
        let sse = format!(
            "{}{}",
            wrapped(
                r#"{"candidates":[{"content":{"parts":[{"text":"Let me look."},{"functionCall":{"id":"call_a","name":"read_file","args":{"path":"src/main.rs"}}}]}}]}"#
            ),
            wrapped(
                r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"default_api:bash","args":{}}}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":9,"candidatesTokenCount":4}}"#
            ),
        );
        let (lines, stream) = run("antigravity/gemini-3.7-flash", &sse);
        let data = json_lines(&lines);
        let tool_frames: Vec<&Value> = data
            .iter()
            .filter(|d| d["choices"][0]["delta"].get("tool_calls").is_some())
            .collect();
        assert_eq!(tool_frames.len(), 2, "{data:#?}");
        assert_eq!(
            tool_frames[0]["choices"][0]["delta"]["tool_calls"][0]["index"],
            0
        );
        assert_eq!(
            tool_frames[0]["choices"][0]["delta"]["tool_calls"][0]["id"],
            "call_a"
        );
        assert_eq!(
            tool_frames[0]["choices"][0]["delta"]["tool_calls"][0]["function"]["name"],
            "read_file"
        );
        assert_eq!(
            tool_frames[0]["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            r#"{"path":"src/main.rs"}"#
        );
        // Gemini 的命名空间从调用名上摘掉；没有 id 的调用自己起一个
        let second = &tool_frames[1]["choices"][0]["delta"]["tool_calls"][0];
        assert_eq!(second["index"], 1);
        assert_eq!(second["function"]["name"], "bash");
        assert_eq!(second["function"]["arguments"], "{}");
        assert!(second["id"].as_str().unwrap().starts_with("call_"));
        let finish: Vec<&Value> = data
            .iter()
            .filter(|d| !d["choices"][0]["finish_reason"].is_null())
            .collect();
        assert_eq!(finish.len(), 1);
        assert_eq!(finish[0]["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(stream.decoder.total.calls.len(), 2);
        assert_eq!(stream.decoder.total.text, "Let me look.");
    }

    /// 模型把调用写进正文（#636）：「Let's look.call:default_api:bash{command:ls}」
    /// 没有任何 functionCall——解码器把它变回它想发的调用。
    #[test]
    fn a_call_written_out_in_text_becomes_a_call() {
        let sse = format!(
            "{}{}",
            wrapped(
                r#"{"candidates":[{"content":{"parts":[{"text":"Let me look.call:default_api:bash{command:ls -"}]}}]}"#
            ),
            wrapped(
                r#"{"candidates":[{"content":{"parts":[{"text":"la}"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":5,"candidatesTokenCount":6}}"#
            ),
        );
        let (lines, stream) = run("m", &sse);
        let data = json_lines(&lines);
        // 第一段的尾巴「call:default_api:bash{command:ls -」被压着，到齐了才认
        let texts: Vec<String> = data
            .iter()
            .filter_map(|d| d["choices"][0]["delta"]["content"].as_str())
            .map(String::from)
            .collect();
        assert_eq!(texts, vec!["Let me look."], "{texts:#?}");
        let calls: Vec<&Value> = data
            .iter()
            .filter_map(|d| d["choices"][0]["delta"]["tool_calls"].as_array())
            .flat_map(|a| a.iter())
            .collect();
        assert_eq!(calls.len(), 1, "{data:#?}");
        assert_eq!(calls[0]["function"]["name"], "bash");
        assert_eq!(calls[0]["function"]["arguments"], r#"{"command":"ls -la"}"#);
        assert_eq!(stream.decoder.total.finish.as_deref(), Some("tool_calls"));
    }

    /// 看着像调用开头、其实不是的正文按原文放，一个字不丢。
    #[test]
    fn text_that_only_looks_like_a_call_is_passed_on_whole() {
        let sse = wrapped(
            r#"{"candidates":[{"content":{"parts":[{"text":"See call:default_api: not a call{} and call:default_api:x{broken"}]},"finishReason":"STOP"}]}"#,
        );
        let (lines, stream) = run("m", &sse);
        let texts: String = json_lines(&lines)
            .iter()
            .filter_map(|d| d["choices"][0]["delta"]["content"].as_str())
            .collect();
        assert_eq!(
            texts,
            "See call:default_api: not a call{} and call:default_api:x{broken"
        );
        assert_eq!(stream.decoder.total.calls.len(), 0);
        assert_eq!(stream.decoder.total.finish.as_deref(), Some("stop"));
    }

    #[test]
    fn finish_reasons_map_to_chat_and_max_tokens_is_length() {
        let finish_of = |sse: &str| -> String {
            let (lines, _) = run("m", sse);
            let data = json_lines(&lines);
            let frame = data
                .iter()
                .find(|d| !d["choices"][0]["finish_reason"].is_null())
                .expect("a finish frame");
            // usage 是收尾之后独立的一帧
            assert!(
                data.last().unwrap()["choices"]
                    .as_array()
                    .unwrap()
                    .is_empty(),
                "{data:#?}"
            );
            frame["choices"][0]["finish_reason"]
                .as_str()
                .unwrap()
                .to_string()
        };
        let sse = wrapped(
            r#"{"candidates":[{"content":{"parts":[{"text":"cut"}]},"finishReason":"MAX_TOKENS"}],"usageMetadata":{"promptTokenCount":1,"candidatesTokenCount":2}}"#,
        );
        let (_, stream) = run("m", &sse);
        assert_eq!(finish_of(&sse), "length");
        assert_eq!(stream.decoder.total.finish.as_deref(), Some("length"));
        // SAFETY 之类 → content_filter；空正文的部件不发帧
        assert_eq!(
            finish_of(&wrapped(
                r#"{"candidates":[{"content":{"parts":[{"text":""}]},"finishReason":"SAFETY"}]}"#
            )),
            "content_filter"
        );
        // Go 的收尾词 → chat：STOP 在有工具调用时是 tool_calls
        assert_eq!(chat_finish("stop", false), "stop");
        assert_eq!(chat_finish("tool", true), "tool_calls");
        assert_eq!(stop_from_gemini("UNEXPECTED_TOOL_CALL"), "stop");
    }

    /// Google 不认的调用（「Malformed function call」）：调用整个在 finishMessage
    /// 里，照样交成调用（fwxkogaia on X）。
    #[test]
    fn a_malformed_call_in_the_finish_message_is_still_a_call() {
        let sse = wrapped(
            r#"{"candidates":[{"content":{"role":"model"},"finishReason":"MALFORMED_FUNCTION_CALL","finishMessage":"Malformed function call: call:default_api:search{query: rust}"}]}"#,
        );
        let (lines, stream) = run("m", &sse);
        let data = json_lines(&lines);
        let calls: Vec<&Value> = data
            .iter()
            .filter_map(|d| d["choices"][0]["delta"]["tool_calls"].as_array())
            .flat_map(|a| a.iter())
            .collect();
        assert_eq!(calls.len(), 1, "{data:#?}");
        assert_eq!(calls[0]["function"]["name"], "search");
        assert_eq!(stream.decoder.total.finish.as_deref(), Some("tool_calls"));
    }

    #[test]
    fn an_error_event_ends_the_stream_with_an_error_frame() {
        let sse = "data: {\"error\":{\"code\":429,\"message\":\"Resource has been exhausted (e.g. check quota).\",\"status\":\"RESOURCE_EXHAUSTED\"}}\n\n".to_string();
        let (lines, stream) = run("m", &sse);
        let data = json_lines(&lines);
        assert_eq!(lines.last().unwrap(), "[DONE]");
        assert_eq!(
            data.last().unwrap()["error"]["message"],
            "Resource has been exhausted (e.g. check quota)."
        );
        assert_eq!(data.last().unwrap()["error"]["code"], 429);
        assert_eq!(
            stream.decoder.total.error.as_ref().map(|(s, _)| *s),
            Some(429)
        );
        assert!(stream.decoder.done);
    }

    /// 流断了、没等到收尾：按已知的收（finish stop、usage 0），不编收口之外的东西。
    #[test]
    fn a_truncated_stream_still_closes_with_zero_usage() {
        let (lines, stream) = run(
            "m",
            &wrapped(r#"{"candidates":[{"content":{"parts":[{"text":"partial"}]}}]}"#),
        );
        let data = json_lines(&lines);
        assert_eq!(data.last().unwrap()["usage"]["prompt_tokens"], 0);
        assert_eq!(lines.last().unwrap(), "[DONE]");
        assert_eq!(stream.decoder.total.text, "partial");
        assert_eq!(stream.decoder.total.finish.as_deref(), Some("stop"));
    }

    #[test]
    fn the_whole_object_for_non_streaming_clients() {
        let sse = format!(
            "{}{}",
            wrapped(
                r#"{"responseId":"resp_9","candidates":[{"content":{"parts":[{"text":"Hi","thought":true},{"functionCall":{"id":"c1","name":"t","args":{"a":1}}}]}}]}"#
            ),
            wrapped(
                r#"{"candidates":[{"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":7,"candidatesTokenCount":3,"thoughtsTokenCount":1}}"#
            ),
        );
        let stream = ChatStream::new(
            Box::new(io::Cursor::new(sse)),
            "antigravity/gemini-3.7-flash",
        );
        let completion = stream.into_completion();
        assert_eq!(completion["id"], "resp_9");
        assert_eq!(completion["object"], "chat.completion");
        assert_eq!(completion["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(
            completion["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
            "t"
        );
        assert_eq!(completion["usage"]["completion_tokens"], 4);
    }

    #[test]
    fn written_out_call_arguments_in_both_shapes() {
        // JSON 自己的成员
        let (_, args, n) =
            text_call(r#"call:default_api:bash{command:"ls -la",cwd:"/tmp"} trailing"#);
        assert_eq!(
            n,
            "call:default_api:bash{command:\"ls -la\",cwd:\"/tmp\"}".len() as isize
        );
        assert_eq!(args.unwrap(), r#"{"command":"ls -la","cwd":"/tmp"}"#);
        // key:value 对：值用 <ctrl46> 引着，或裸文字
        let (_, args, n) =
            text_call("call:default_api:edit{file:<ctrl46>a b<ctrl46>,text:hello world,n:3}");
        assert!(n > 0);
        assert_eq!(
            args.unwrap(),
            r#"{"file":"a b","text":"hello world","n":3}"#
        );
        // 括号里的逗号不算下一个键；引号里的花括号不计数
        let (_, args, _) = text_call(r#"call:default_api:t{q:{a:[1,2]},z:<ctrl46>}<ctrl46>}"#);
        assert_eq!(args.unwrap(), r#"{"q":"{a:[1,2]}","z":"}"}"#);
        let (_, args, _) = text_call("call:default_api:t{s:<ctrl46>a {b} c<ctrl46>,n:1}");
        assert_eq!(args.unwrap(), r#"{"s":"a {b} c","n":1}"#);
        let (_, args, _) = text_call(r#"call:default_api:t{list:[1, 2], done:true}"#);
        // 值本身是合法 JSON（数字、true、数组）就原样放
        assert_eq!(args.unwrap(), r#"{"list":[1, 2],"done":true}"#);
        // 不是调用
        assert_eq!(text_call("call:default_api: nope").2, -1);
        // 参数读不懂的调用
        assert_eq!(text_call("call:default_api:t{a b c}").2, -1);
        // 括号还没闭上：可能下一段才到齐
        assert_eq!(text_call("call:default_api:t{not json at all").2, 0);
        // 还没到齐
        assert_eq!(text_call("call:default_api:t{a:1").2, 0);
        assert_eq!(text_call("call:default_api:t").2, 0);
    }
}
