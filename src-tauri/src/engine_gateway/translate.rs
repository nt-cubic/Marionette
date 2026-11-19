//! 协议翻译（合同 C5 的 chat ↔ responses ↔ anthropic 矩阵）。
//!
//! 学 magpie 的架构：先把三种线协议各自解析进**一张中间形态**
//! （ir.go 的 Request/Part/Message 裁剪版），再从中间形态建目标协议——
//! 每个方向的语义只定义一次，往返（chat→anthropic→chat 等）天然一致。
//! 覆盖子集：text / image_url(image) / tools 定义与 tool_calls（tool_use/
//! tool_result、function_call/function_call_output）/ system 提取。
//! thinking 块与视频/文件附件不进本矩阵：跨协议没有诚实对应（签名的思考
//! 只回原协议，文件各协议拼写差异大），透传层另有通路。
//! SSE 映射是无状态的单事件翻译（text delta / finish / usage / [DONE]），
//! content_block 序号编排与 message_start 编排由调用方负责；chat 流的
//! tool_calls 分片拼装走 [`ChatToolAccum`]（有状态，两侧端点共享），
//! 无状态映射对 tool_calls 分片恒返回 None——分片不是完整事件。
//! 语义参照：D:\Project\magpie-upstream\internal\gateway\（研究克隆，只读）。

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

// =====================================================================
// 中间形态（ir.go 的 Request/Part/Message 裁剪版）
// =====================================================================

/// 消息块的种类。
#[derive(Clone, Copy, PartialEq)]
enum PartKind {
    Text,
    Image,
    ToolCall,
    ToolResult,
    /// 上一回合的思考文本（responses 的 `reasoning` item；text 字段存正文）。
    /// 只有 build_chat 会用它（暂存给 chat.rs 按厂商回放/丢弃），其余协议
    /// 不跨协议携带——加密思考只回 responses 自己。
    Thinking,
}

#[derive(Clone)]
struct Part {
    kind: PartKind,
    /// text 正文 / tool_result 的输出文本
    text: String,
    /// image：data: URL 拆出的内联形态（media_type + base64 data）
    media_type: String,
    data: String,
    /// image：只有 URL、没有内联数据时保留
    url: String,
    /// tool_call
    id: String,
    name: String,
    /// tool_call 参数：JSON 对象（Null 视为 {}，见 args_of——anthropic 的
    /// input 恒为对象，chat/responses 的 arguments 恒为它的序列化串）
    args: Value,
    /// tool_result
    call_id: String,
    is_error: bool,
    /// tool_result 附带的图（anthropic tool_result 的 content 块 /
    /// responses function_call_output 的数组 output；chat 的 tool 消息
    /// 内容数组）
    images: Vec<Part>,
}

impl Part {
    fn text(s: impl Into<String>) -> Part {
        Part {
            kind: PartKind::Text,
            text: s.into(),
            media_type: String::new(),
            data: String::new(),
            url: String::new(),
            id: String::new(),
            name: String::new(),
            args: Value::Null,
            call_id: String::new(),
            is_error: false,
            images: Vec::new(),
        }
    }

    fn image(media_type: impl Into<String>, data: impl Into<String>, url: impl Into<String>) -> Part {
        Part {
            kind: PartKind::Image,
            text: String::new(),
            media_type: media_type.into(),
            data: data.into(),
            url: url.into(),
            id: String::new(),
            name: String::new(),
            args: Value::Null,
            call_id: String::new(),
            is_error: false,
            images: Vec::new(),
        }
    }

    fn thinking(s: impl Into<String>) -> Part {
        Part { kind: PartKind::Thinking, text: s.into(), ..Part::text("") }
    }

    fn tool_call(id: impl Into<String>, name: impl Into<String>, args: Value) -> Part {
        Part {
            kind: PartKind::ToolCall,
            id: id.into(),
            name: name.into(),
            args,
            ..Part::text("")
        }
    }

    fn tool_result(call_id: impl Into<String>, text: impl Into<String>, images: Vec<Part>) -> Part {
        Part {
            kind: PartKind::ToolResult,
            call_id: call_id.into(),
            text: text.into(),
            images,
            ..Part::text("")
        }
    }
}

/// 一个可被模型调用的函数定义（三种协议共通的最小面：名/描述/schema；
/// strict、deferred 等开关不跨协议保真，重建时省略）。
#[derive(Clone)]
struct ToolDef {
    name: String,
    description: String,
    /// JSON Schema；Null = 请求没带
    schema: Value,
}

#[derive(Clone)]
struct Message {
    role: String,
    parts: Vec<Part>,
}

/// 三种协议共通的请求面。role 只有 user/assistant——system 一律提取到
/// `system` 字段（chat 的 system/developer 消息、anthropic 的顶层 system、
/// responses 的 instructions + 前置 system/developer item 都归这里）。
#[derive(Clone, Default)]
struct Ir {
    model: String,
    system: String,
    messages: Vec<Message>,
    tools: Vec<ToolDef>,
    /// "" | auto | none | required | name:<tool>（magpie 的内部拼写，
    /// 各协议的 tool_choice 形态在 build 侧展开）
    tool_choice: String,
    max_tokens: u64,
    temperature: Option<f64>,
    top_p: Option<f64>,
    stop: Vec<String>,
    stream: bool,
    /// 请求要的思考强度（"" = 没要）。三协议各有各的字段，翻译时必须带过去——
    /// 不带 = codex/Claude Code 选的强度被静默吞掉，厂商按默认档思考。
    effort: String,
    /// responses 的 `parallel_tool_calls`（Go Request.Parallel）；只有 build_chat
    /// 在带工具时转发。
    parallel: Option<bool>,
    /// 结构化输出要求，已归一成 chat 的 `response_format` 形态（Go `Request.Format`：
    /// responses 的 `text.format`、anthropic 的 `output_config.format` 读进来，
    /// build_chat 以 `response_format` 写出）。只有 build_chat 带它。
    format: Option<Value>,
    /// 请求自己的 `prompt_cache_key`（Go `Request.CacheKey`；codex 的线程 id）。
    cache_key: String,
    /// responses 请求里被「压平」交给厂商的工具（flat 名 → 原貌）。只有
    /// responses→chat 的应答编码器用它把调用还原成 Codex 认的 item。
    named: ToolTable,
}

/// anthropic /v1/messages 的 max_tokens 是必填项；请求没带时兜底
/// （C5 合同钉 8192；Go 侧 defaultMaxTokens 是 16384，接线对账时
/// 如需对齐只改这一个常量）。
const ANTHROPIC_MAX_TOKENS_FLOOR: u64 = 8192;

// =====================================================================
// 小工具（名字与语义对照 magpie：parseArgs / argsOf / stringOrText /
// imagePart / dataURL / mergeTurns）
// =====================================================================

fn get_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().map(str::to_string)
}

/// magpie parseArgs：chat/responses 的 arguments 是字符串；合法 JSON 对象
/// 原样进 IR，其余（含空）包成 {"input": s}——不丢模型给的任何字。
fn parse_args(s: &str) -> Value {
    let s = s.trim();
    if s.is_empty() {
        return json!({});
    }
    if s.starts_with('{') {
        if let Ok(v) = serde_json::from_str::<Value>(s) {
            return v;
        }
    }
    json!({"input": s})
}

/// magpie argsOf：参数恒为对象，空/Null 补 {}（anthropic 的 input 拒绝空）。
fn args_of(p: &Part) -> Value {
    if p.args.is_null() { json!({}) } else { p.args.clone() }
}

fn args_string(p: &Part) -> String {
    serde_json::to_string(&args_of(p)).unwrap_or_else(|_| "{}".into())
}

/// magpie stringOrText：字符串原样；块数组的 text 块拼接（anthropic 的
/// system / tool_result content、chat 的 system 消息内容都可能是两种形态）。
fn string_or_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            let mut b = String::new();
            for it in items {
                match get_str(it, "type").unwrap_or_default().as_str() {
                    "text" | "input_text" | "output_text" => {
                        b.push_str(&get_str(it, "text").unwrap_or_default())
                    }
                    _ => {}
                }
            }
            b
        }
        _ => String::new(),
    }
}

/// magpie imagePart：data: URL 拆成内联 (media_type, base64)，其余按 URL 保留。
fn image_part(url: &str) -> Part {
    if let Some(rest) = url.strip_prefix("data:") {
        if let Some((meta, data)) = rest.split_once(',') {
            let media_type = meta.strip_suffix(";base64").unwrap_or(meta);
            return Part::image(media_type, data, "");
        }
    }
    Part::image("", "", url)
}

/// magpie dataURL：imagePart 的逆——内联形态拼回 data: URL，URL 形态直通。
fn data_url(p: &Part) -> String {
    if !p.url.is_empty() && p.data.is_empty() {
        return p.url.clone();
    }
    format!("data:{};base64,{}", p.media_type, p.data)
}

/// 工具输出内容（chat tool 消息的 content / anthropic tool_result 的
/// content / responses function_call_output 的 output 三种同构形状）：
/// 文本拼接 + 附图收集。
fn output_parts(content: &Value) -> (String, Vec<Part>) {
    match content {
        Value::String(s) => (s.clone(), Vec::new()),
        Value::Array(items) => {
            let mut text = String::new();
            let mut images = Vec::new();
            for it in items {
                match get_str(it, "type").unwrap_or_default().as_str() {
                    "text" | "input_text" | "output_text" => {
                        text.push_str(&get_str(it, "text").unwrap_or_default())
                    }
                    // chat 的嵌套 image_url 块
                    "image_url" => {
                        if let Some(u) = it.pointer("/image_url/url").and_then(|u| u.as_str()) {
                            images.push(image_part(u));
                        }
                    }
                    // responses 的扁平 image_url 字段
                    "input_image" => {
                        if let Some(u) = get_str(it, "image_url") {
                            images.push(image_part(&u));
                        }
                    }
                    _ => {}
                }
            }
            (text, images)
        }
        _ => (String::new(), Vec::new()),
    }
}

/// magpie mergeTurns：相邻同角色（user/assistant）合并。responses 的
/// function_call 与 message 是平铺 item，解析后会拆成相邻同角色消息，
/// 合并回来才能与 chat/anthropic 的「一条消息多块」对齐。
fn merge_turns(msgs: Vec<Message>) -> Vec<Message> {
    let mut out: Vec<Message> = Vec::with_capacity(msgs.len());
    for m in msgs {
        if let Some(last) = out.last_mut() {
            if last.role == m.role && (m.role == "user" || m.role == "assistant") {
                last.parts.extend(m.parts);
                continue;
            }
        }
        out.push(m);
    }
    out
}

fn err_not_object(what: &str) -> String {
    format!("invalid {what}: not an object")
}

// =====================================================================
// 解析：线协议 → IR
// =====================================================================

/// chat 消息 content（string 或 parts 数组）→ 块（chatParts 同款子集）。
fn chat_parts(content: &Value) -> Vec<Part> {
    match content {
        Value::String(s) if !s.is_empty() => vec![Part::text(s.clone())],
        Value::Array(items) => {
            let mut out = Vec::new();
            for it in items {
                match get_str(it, "type").unwrap_or_default().as_str() {
                    "text" => out.push(Part::text(get_str(it, "text").unwrap_or_default())),
                    "image_url" => {
                        if let Some(u) = it.pointer("/image_url/url").and_then(|u| u.as_str()) {
                            out.push(image_part(u));
                        }
                    }
                    _ => {}
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

/// openai chat/completions 请求体 → IR（readChat 同款）。
fn parse_chat(body: &Value) -> Result<Ir, String> {
    if !body.is_object() {
        return Err(err_not_object("request"));
    }
    let mut ir = Ir {
        model: get_str(body, "model").unwrap_or_default(),
        // OpenAI 新字段优先，老字段兜底（Go：MaxCompletionTokens，为 0 再取 MaxTokens）
        max_tokens: body
            .get("max_completion_tokens")
            .and_then(|v| v.as_u64())
            .filter(|n| *n > 0)
            .or_else(|| body.get("max_tokens").and_then(|v| v.as_u64()))
            .unwrap_or(0),
        temperature: body.get("temperature").and_then(|v| v.as_f64()),
        top_p: body.get("top_p").and_then(|v| v.as_f64()),
        stream: body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false),
        effort: super::effort::requested(super::effort::Proto::Chat, body).unwrap_or_default(),
        ..Default::default()
    };
    match body.get("stop") {
        Some(Value::String(s)) => ir.stop = vec![s.clone()],
        Some(Value::Array(a)) => {
            ir.stop = a.iter().filter_map(|v| v.as_str().map(String::from)).collect()
        }
        _ => {}
    }
    let messages = body
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or("invalid request: messages")?;
    let mut sys: Vec<String> = Vec::new();
    for m in messages {
        match get_str(m, "role").unwrap_or_default().as_str() {
            // system 与 developer 同待遇：都折叠进顶层 system（Go 同款）
            "system" | "developer" => {
                let t = string_or_text(m.get("content").unwrap_or(&Value::Null));
                if !t.is_empty() {
                    sys.push(t);
                }
            }
            "user" => ir.messages.push(Message {
                role: "user".into(),
                parts: chat_parts(m.get("content").unwrap_or(&Value::Null)),
            }),
            "assistant" => {
                let mut parts = chat_parts(m.get("content").unwrap_or(&Value::Null));
                if let Some(calls) = m.get("tool_calls").and_then(|c| c.as_array()) {
                    for tc in calls {
                        let name = tc
                            .pointer("/function/name")
                            .and_then(|n| n.as_str())
                            .unwrap_or_default();
                        let args = tc
                            .pointer("/function/arguments")
                            .and_then(|a| a.as_str())
                            .unwrap_or_default();
                        parts.push(Part::tool_call(get_str(tc, "id").unwrap_or_default(), name, parse_args(&args)));
                    }
                }
                ir.messages.push(Message { role: "assistant".into(), parts });
            }
            "tool" => {
                // chat 的 tool 消息在 IR 里是一条 user 消息的 tool_result 块
                // （anthropic/responses 同款语义：结果由 user 侧交回）
                let (text, images) = output_parts(m.get("content").unwrap_or(&Value::Null));
                ir.messages.push(Message {
                    role: "user".into(),
                    parts: vec![Part::tool_result(
                        get_str(m, "tool_call_id").unwrap_or_default(),
                        text,
                        images,
                    )],
                });
            }
            _ => {} // 未知角色：与 Go 一致，不认
        }
    }
    ir.system = sys.join("\n\n");
    if let Some(tools) = body.get("tools").and_then(|t| t.as_array()) {
        for t in tools {
            let ty = get_str(t, "type").unwrap_or_default();
            if !ty.is_empty() && ty != "function" {
                continue; // web_search 等服务端工具不跨协议
            }
            let name = t
                .pointer("/function/name")
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            ir.tools.push(ToolDef {
                name: name.into(),
                description: t
                    .pointer("/function/description")
                    .and_then(|d| d.as_str())
                    .unwrap_or_default()
                    .into(),
                schema: t.pointer("/function/parameters").cloned().unwrap_or(Value::Null),
            });
        }
    }
    match body.get("tool_choice") {
        Some(Value::String(s)) => ir.tool_choice = s.clone(),
        Some(choice) => {
            if let Some(name) = choice.pointer("/function/name").and_then(|n| n.as_str()) {
                ir.tool_choice = format!("name:{name}");
            }
        }
        None => {}
    }
    Ok(ir)
}

/// Go `openAIFormat`：OpenAI 两代的结构化输出声明 → chat `response_format` 形态。
/// responses 的 `text.format`（`{type, name, schema, strict, description}`）与 chat
/// 自己的 `response_format`（`{type, json_schema:{…}}`）都认；`{type:"text"}`
/// 与其它一律 None；没带 schema 的 json_schema 当 json_object（Go 同款）。
pub(crate) fn openai_format(raw: &Value) -> Option<Value> {
    let ty = raw.get("type").and_then(|t| t.as_str())?;
    match ty {
        "json_object" => Some(json!({"type": "json_object"})),
        "json_schema" => {
            // chat 形态把字段包在 json_schema 里；responses 形态平铺
            let inner = raw.get("json_schema").filter(|j| j.is_object()).unwrap_or(raw);
            let schema = inner.get("schema").filter(|s| !s.is_null());
            let Some(schema) = schema else {
                return Some(json!({"type": "json_object"}));
            };
            let name = inner
                .get("name")
                .and_then(|n| n.as_str())
                .filter(|n| !n.is_empty())
                .unwrap_or("response");
            let mut js = json!({"name": name, "schema": schema});
            if let Some(d) = inner.get("description").and_then(|d| d.as_str()).filter(|d| !d.is_empty()) {
                js["description"] = json!(d);
            }
            if let Some(strict) = inner.get("strict").and_then(|s| s.as_bool()) {
                js["strict"] = json!(strict);
            }
            Some(json!({"type": "json_schema", "json_schema": js}))
        }
        _ => None,
    }
}

/// anthropic /v1/messages 请求体 → IR（readAnthropic 同款子集）。
fn parse_anthropic(body: &Value) -> Result<Ir, String> {
    if !body.is_object() {
        return Err(err_not_object("request"));
    }
    let mut ir = Ir {
        model: get_str(body, "model").unwrap_or_default(),
        max_tokens: body.get("max_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        temperature: body.get("temperature").and_then(|v| v.as_f64()),
        top_p: body.get("top_p").and_then(|v| v.as_f64()),
        stream: body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false),
        system: string_or_text(body.get("system").unwrap_or(&Value::Null)),
        effort: super::effort::requested(super::effort::Proto::Anthropic, body).unwrap_or_default(),
        // anthropic 的结构化输出只有 json_schema 一种（Go bodyFormat 同款）
        format: body.pointer("/output_config/format").and_then(|f| {
            let schema = f.get("schema").filter(|s| s.is_object())?;
            (get_str(f, "type").as_deref() == Some("json_schema")).then(|| {
                json!({"type": "json_schema", "json_schema": {"name": "response", "schema": schema}})
            })
        }),
        ..Default::default()
    };
    if let Some(stops) = body.get("stop_sequences").and_then(|s| s.as_array()) {
        ir.stop = stops.iter().filter_map(|v| v.as_str().map(String::from)).collect();
    }
    let messages = body
        .get("messages")
        .and_then(|v| v.as_array())
        .ok_or("invalid request: messages")?;
    for m in messages {
        // anthropic 只认 user/assistant；其余（system 消息等罕见形态）按 user 收
        let role = if get_str(m, "role").as_deref() == Some("assistant") { "assistant" } else { "user" };
        let mut parts = Vec::new();
        match m.get("content") {
            Some(Value::String(s)) => {
                if !s.is_empty() {
                    parts.push(Part::text(s.clone()));
                }
            }
            Some(Value::Array(blocks)) => {
                for b in blocks {
                    match get_str(b, "type").unwrap_or_default().as_str() {
                        "text" => parts.push(Part::text(get_str(b, "text").unwrap_or_default())),
                        "image" => {
                            if let Some(src) = b.get("source") {
                                if get_str(src, "type").as_deref() == Some("url") {
                                    parts.push(Part::image("", "", get_str(src, "url").unwrap_or_default()));
                                } else {
                                    parts.push(Part::image(
                                        get_str(src, "media_type").unwrap_or_default(),
                                        get_str(src, "data").unwrap_or_default(),
                                        "",
                                    ));
                                }
                            }
                        }
                        "tool_use" => parts.push(Part::tool_call(
                            get_str(b, "id").unwrap_or_default(),
                            get_str(b, "name").unwrap_or_default(),
                            // input 恒为对象；缺/坏时补 {}（argsOf 兜底）
                            b.get("input").cloned().filter(|v| v.is_object()).unwrap_or_else(|| json!({})),
                        )),
                        "tool_result" => {
                            let (text, images) =
                                output_parts(b.get("content").unwrap_or(&Value::Null));
                            let mut p = Part::tool_result(
                                get_str(b, "tool_use_id").unwrap_or_default(),
                                text,
                                images,
                            );
                            p.is_error = b.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
                            parts.push(p);
                        }
                        // thinking 文本留成 Thinking 块（Go readAnthropic 保留
                        // Thinking+Signature）：翻到 chat 时 build_chat 暂存、
                        // chat::fit_for_vendor 按厂商回放成 `reasoning_content`
                        // （DeepSeek 系要求第二轮工具调用带回思考，否则 400）或丢弃；
                        // 翻到 anthropic/responses 的 build_* 照旧不带它（签名/加密体
                        // 不跨协议）。空白占位不留。
                        "thinking" => {
                            let t = get_str(b, "thinking").unwrap_or_default();
                            if !t.trim().is_empty() {
                                parts.push(Part::thinking(t));
                            }
                        }
                        // redacted_thinking：密封块只有 Anthropic 读得懂，丢弃
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        ir.messages.push(Message { role: role.into(), parts });
    }
    if let Some(tools) = body.get("tools").and_then(|t| t.as_array()) {
        for t in tools {
            let name = get_str(t, "name").unwrap_or_default();
            if name.is_empty() {
                continue; // 服务端工具（web_search_* 等带 type 无 name）不跨协议
            }
            ir.tools.push(ToolDef {
                name,
                description: get_str(t, "description").unwrap_or_default(),
                schema: t.get("input_schema").cloned().unwrap_or(Value::Null),
            });
        }
    }
    if let Some(tc) = body.get("tool_choice") {
        ir.tool_choice = match get_str(tc, "type").unwrap_or_default().as_str() {
            "auto" => "auto".into(),
            "none" => "none".into(),
            // anthropic 的 any = 「必须调某个工具」= openai 的 required
            "any" => "required".into(),
            "tool" => format!("name:{}", get_str(tc, "name").unwrap_or_default()),
            _ => String::new(),
        };
    }
    Ok(ir)
}

/// responses 消息 content（string 或 parts 数组）→ 块（responsesParts 同款）。
fn responses_parts(content: &Value) -> Vec<Part> {
    match content {
        Value::String(s) if !s.is_empty() => vec![Part::text(s.clone())],
        Value::Array(items) => {
            let mut out = Vec::new();
            for it in items {
                match get_str(it, "type").unwrap_or_default().as_str() {
                    "input_text" | "output_text" | "text" => {
                        out.push(Part::text(get_str(it, "text").unwrap_or_default()))
                    }
                    "input_image" => {
                        if let Some(u) = get_str(it, "image_url") {
                            out.push(image_part(&u));
                        }
                    }
                    _ => {}
                }
            }
            out
        }
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------------
// Codex 的非 function 工具：压平给 chat 厂商，应答再还原
// （Go responses.go 的 nsTool / flatName / customSchema / callTo）
// ---------------------------------------------------------------------

/// 一个被压平的工具的原貌（Go nsTool）。`search` = Codex 自己的 tool_search
/// （client 执行）；`custom` = 自由文本工具（apply_patch / exec，调用要还原成
/// `custom_tool_call`）；`namespace` 非空 = 命名空间里的工具（还原成带
/// `namespace` 的 `function_call`）。普通 function 工具不进表。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NsTool {
    pub namespace: String,
    pub name: String,
    pub search: bool,
    pub custom: bool,
}

/// 单个请求的「厂商看到的 flat 名 → 原貌」表。随请求走到应答编码器，
/// 不放任何全局（并发请求各带各的）。
pub type ToolTable = HashMap<String, NsTool>;

const TOOL_SEARCH: &str = "tool_search";
/// Codex Responses Lite 把自家工具归在这个命名空间里（Go liteNamespace）。
const LITE_NAMESPACE: &str = "functions";
const UNAVAILABLE_RESULT: &str = "[The result of this tool call is unavailable: the turn was interrupted.]";

fn tool_char_ok(c: u8) -> bool {
    c == b'_' || c == b'-' || c.is_ascii_alphanumeric()
}

fn short_hash(s: &str) -> String {
    Sha256::digest(s.as_bytes())[..4].iter().map(|b| format!("{b:02x}")).collect()
}

/// Go provider.FlatName：`namespace__name`，≤64 字符；超长截 55 + `_` + sha256
/// 前 4 字节。比 Go 多一步：chat 厂商只收 `[a-zA-Z0-9_-]`，非法字节换 `_` 并
/// 同样补哈希（同名恒同结果、不同名不撞）。
pub(crate) fn flat_name(namespace: &str, name: &str) -> String {
    let raw = format!("{namespace}__{name}");
    if raw.len() <= 64 && raw.bytes().all(tool_char_ok) {
        return raw;
    }
    let clean: String =
        raw.bytes().take(55).map(|c| if tool_char_ok(c) { c as char } else { '_' }).collect();
    format!("{clean}_{}", short_hash(&format!("{namespace}\0{name}")))
}

fn custom_schema() -> Value {
    json!({"type": "object",
        "properties": {"input": {"type": "string",
            "description": "The tool's free-form input, the whole of it, exactly as it is to be run."}},
        "required": ["input"], "additionalProperties": false})
}

/// Go customDescription：自带描述 + 输入说明 + 语法（grammar）。
fn custom_description(t: &Value) -> String {
    let mut d = get_str(t, "description").unwrap_or_default().trim().to_string();
    if !d.is_empty() {
        d.push_str("\n\n");
    }
    d.push_str("Put the tool's whole free-form input, as it is to be run, in the `input` string; it is passed on as written, so it is not itself JSON.");
    if let Some(f) = t.get("format") {
        let def = get_str(f, "definition").unwrap_or_default();
        if get_str(f, "type").as_deref() == Some("grammar") && !def.trim().is_empty() {
            let syntax = get_str(f, "syntax").filter(|s| !s.is_empty()).unwrap_or_else(|| "the".into());
            d.push_str(&format!("\n\nThe input follows this {syntax} grammar:\n{}", def.trim()));
        }
    }
    d
}

/// Go customInput：自由文本调用的输入 = arguments 里的 `input` 串，否则
/// arguments 本身是 JSON 串就取它，否则原文。
pub(crate) fn custom_input(args: &str) -> String {
    match serde_json::from_str::<Value>(args) {
        Ok(Value::Object(m)) => {
            if let Some(Value::String(s)) = m.get("input") {
                return s.clone();
            }
        }
        Ok(Value::String(s)) => return s,
        _ => {}
    }
    args.to_string()
}

/// Go unsealed：去掉 schema 里 Codex 标的 `"encrypted": <bool>`（属性名叫
/// encrypted 的子 schema 是对象，不动）。
fn unsealed(schema: &Value) -> Value {
    fn strip(v: &mut Value) {
        match v {
            Value::Object(m) => {
                if matches!(m.get("encrypted"), Some(Value::Bool(_))) {
                    m.remove("encrypted");
                }
                for c in m.values_mut() {
                    strip(c);
                }
            }
            Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut v = schema.clone();
    strip(&mut v);
    v
}

/// Go searchFound：tool_search 的结果在模型眼里是「现在可以调这些名字」。
fn search_found(tools: &[Value], flat_of: &dyn Fn(&str, &str) -> String) -> String {
    let mut names: Vec<String> = Vec::new();
    for t in tools {
        match get_str(t, "type").as_deref() {
            Some("function") => names.push(get_str(t, "name").unwrap_or_default()),
            Some("namespace") => {
                let ns = get_str(t, "name").unwrap_or_default();
                for nt in t.get("tools").and_then(|x| x.as_array()).into_iter().flatten() {
                    if get_str(nt, "type").as_deref() == Some("function") {
                        names.push(flat_of(&ns, &get_str(nt, "name").unwrap_or_default()));
                    }
                }
            }
            _ => {}
        }
    }
    if names.is_empty() {
        "No tools matched the search.".into()
    } else {
        format!("These tools are now available to call: {}", names.join(", "))
    }
}

/// 请求的工具清单构建器：唯一名、原貌表、(ns,name)→flat 名（历史 item 回放用）。
#[derive(Default)]
struct ToolSet {
    tools: Vec<ToolDef>,
    named: ToolTable,
    flat: HashMap<(String, String), String>,
    used: HashSet<String>,
}

impl ToolSet {
    /// `ns` = None 是普通 function（名字原样、不改名、不进表）。压平/自由文本/
    /// search 的名字若与已提供的撞了，补哈希后缀保唯一。`found` = 工具搜索
    /// 找到的（Go：已提供过同名就不再提供）。
    fn offer(&mut self, mut def: ToolDef, ns: Option<NsTool>, found: bool) {
        if found && self.used.contains(&def.name) {
            return;
        }
        let key = ns.as_ref().map(|n| (n.namespace.clone(), n.name.clone()));
        if ns.is_some() && self.used.contains(&def.name) {
            let seed = key.as_ref().map(|(a, b)| format!("{a}\0{b}")).unwrap_or_default();
            let base: String = def.name.chars().take(55).collect();
            let mut cand = format!("{base}_{}", short_hash(&seed));
            let mut n = 2;
            while self.used.contains(&cand) {
                cand = format!("{base}_{}{n}", short_hash(&seed));
                n += 1;
            }
            def.name = cand;
        }
        self.used.insert(def.name.clone());
        if let Some(k) = key {
            self.flat.entry(k).or_insert_with(|| def.name.clone());
        }
        if let Some(n) = ns {
            self.named.insert(def.name.clone(), n);
        }
        self.tools.push(def);
    }

    /// 历史 item / tool_choice 里的 (namespace, name) → 厂商看到的名字。
    fn flat_of(&self, ns: &str, name: &str) -> String {
        if ns.is_empty() {
            return name.to_string();
        }
        self.flat
            .get(&(ns.to_string(), name.to_string()))
            .cloned()
            .unwrap_or_else(|| flat_name(ns, name))
    }

    /// Go readResponses 的 offer 分派：function / custom / namespace / tool_search
    /// （client）。web_search / image_generation / local_shell 等服务端或
    /// 本机内建工具 Go 同样不提供，这里有意丢弃。
    fn add(&mut self, t: &Value, found: bool) {
        let name = get_str(t, "name").unwrap_or_default();
        match get_str(t, "type").as_deref().unwrap_or("") {
            "function" => {
                if name.is_empty() {
                    return;
                }
                self.offer(
                    ToolDef {
                        name,
                        description: get_str(t, "description").unwrap_or_default(),
                        schema: t.get("parameters").map(unsealed).unwrap_or(Value::Null),
                    },
                    None,
                    found,
                );
            }
            "custom" => {
                if name.is_empty() {
                    return;
                }
                self.offer(
                    ToolDef { name: name.clone(), description: custom_description(t), schema: custom_schema() },
                    Some(NsTool { name, custom: true, ..Default::default() }),
                    found,
                );
            }
            "namespace" => {
                for nt in t.get("tools").and_then(|x| x.as_array()).into_iter().flatten() {
                    let nname = get_str(nt, "name").unwrap_or_default();
                    if nname.is_empty() {
                        continue;
                    }
                    let flat = flat_name(&name, &nname);
                    match get_str(nt, "type").as_deref() {
                        Some("function") => self.offer(
                            ToolDef {
                                name: flat,
                                description: get_str(nt, "description").unwrap_or_default(),
                                schema: nt.get("parameters").map(unsealed).unwrap_or(Value::Null),
                            },
                            Some(NsTool { namespace: name.clone(), name: nname, ..Default::default() }),
                            found,
                        ),
                        Some("custom") => self.offer(
                            ToolDef { name: flat, description: custom_description(nt), schema: custom_schema() },
                            Some(NsTool { namespace: name.clone(), name: nname, custom: true, ..Default::default() }),
                            found,
                        ),
                        _ => {}
                    }
                }
            }
            "tool_search" => {
                if get_str(t, "execution").as_deref() == Some("client") {
                    self.offer(
                        ToolDef {
                            name: TOOL_SEARCH.into(),
                            description: get_str(t, "description").unwrap_or_default(),
                            schema: t.get("parameters").cloned().unwrap_or(Value::Null),
                        },
                        Some(NsTool { search: true, ..Default::default() }),
                        found,
                    );
                }
            }
            _ => {}
        }
    }
}

/// Go callTo：把厂商按 flat 名给的 function_call item 还原成 Codex 认的形态
/// （原地改 `item`，它带字符串 `arguments`）。
///   search    -> `tool_search_call`（client 执行，arguments 是对象，无 name）
///   custom    -> `custom_tool_call`（`input` 取代 arguments；有 namespace 带上）
///   namespace -> `function_call` + 原名 + `namespace`
///   其余      -> `function_call` + 原名
pub(crate) fn call_to(item: &mut Value, flat: &str, table: &ToolTable) {
    let args = item.get("arguments").and_then(|a| a.as_str()).unwrap_or("").to_string();
    let Some(map) = item.as_object_mut() else { return };
    match table.get(flat) {
        Some(q) if q.search => {
            map.remove("name");
            map.insert("type".into(), json!("tool_search_call"));
            map.insert("execution".into(), json!("client"));
            map.insert("arguments".into(), parse_args(&args));
        }
        Some(q) if q.custom => {
            map.remove("arguments");
            map.insert("type".into(), json!("custom_tool_call"));
            map.insert("name".into(), json!(q.name));
            map.insert("input".into(), json!(if args.is_empty() { String::new() } else { custom_input(&args) }));
            if !q.namespace.is_empty() {
                map.insert("namespace".into(), json!(q.namespace));
            }
        }
        Some(q) => {
            map.insert("name".into(), json!(q.name));
            map.insert("namespace".into(), json!(q.namespace));
            // Go：Codex 把没有这个列表的命名空间调用读成「已加密」
            map.insert("encrypted_function_args".into(), json!([]));
        }
        None => {
            map.insert("name".into(), json!(flat));
        }
    }
}

/// Go itemPrefix：OpenAI 拒收 id 前缀与类型不符的 tool_search_call / custom_tool_call
/// （Codex 会把 item 原样带回），按类型选前缀。
pub(crate) fn call_item_prefix(item: &Value) -> &'static str {
    match item.get("type").and_then(|t| t.as_str()) {
        Some("tool_search_call") => "tsc_",
        Some("custom_tool_call") => "ctc_",
        _ => "fc_",
    }
}

fn is_call_item(t: &str) -> bool {
    matches!(t, "function_call" | "custom_tool_call" | "tool_search_call")
}

fn is_result_item(t: &str) -> bool {
    matches!(t, "function_call_output" | "custom_tool_call_output" | "tool_search_output")
}

fn call_args_of(it: &Value) -> Value {
    match it.get("arguments") {
        Some(Value::String(s)) => parse_args(s),
        Some(v) if v.is_object() => v.clone(),
        _ => json!({}),
    }
}

/// openai responses 请求体 → IR（readResponses 同款子集）。
fn parse_responses(body: &Value) -> Result<Ir, String> {
    if !body.is_object() {
        return Err(err_not_object("request"));
    }
    let mut ir = Ir {
        model: get_str(body, "model").unwrap_or_default(),
        system: get_str(body, "instructions").unwrap_or_default(),
        max_tokens: body.get("max_output_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        temperature: body.get("temperature").and_then(|v| v.as_f64()),
        top_p: body.get("top_p").and_then(|v| v.as_f64()),
        stream: body.get("stream").and_then(|v| v.as_bool()).unwrap_or(false),
        effort: super::effort::requested(super::effort::Proto::Responses, body).unwrap_or_default(),
        parallel: body.get("parallel_tool_calls").and_then(|v| v.as_bool()),
        format: body.pointer("/text/format").and_then(openai_format),
        cache_key: get_str(body, "prompt_cache_key").unwrap_or_default(),
        ..Default::default()
    };
    let items: Option<&Vec<Value>> = body.get("input").and_then(|i| i.as_array());

    // ---- 工具清单（先于历史：历史 item 要用同一张 flat 名表）----
    // 请求自带的 tools，加 Responses Lite 的 additional_tools（Go
    // liftAdditionalTools：提到 tools 里，`functions` 命名空间的拆成普通工具），
    // 再加 tool_search_output 找到的（Codex 不会再发一遍，排在后面）。
    let mut sent: Vec<Value> = body.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default();
    let mut found: Vec<Value> = Vec::new();
    let mut lite = false;
    for it in items.into_iter().flatten() {
        match get_str(it, "type").as_deref() {
            Some("additional_tools") => {
                lite = true;
                for t in it.get("tools").and_then(|x| x.as_array()).into_iter().flatten() {
                    let lite_ns = get_str(t, "type").as_deref() == Some("namespace")
                        && get_str(t, "name").as_deref() == Some(LITE_NAMESPACE);
                    let more: Vec<Value> = if lite_ns {
                        t.get("tools").and_then(|x| x.as_array()).cloned().unwrap_or_default()
                    } else {
                        vec![t.clone()]
                    };
                    for m in more {
                        let n = get_str(&m, "name").unwrap_or_default();
                        let dup = !n.is_empty()
                            && sent.iter().any(|s| get_str(s, "name").as_deref() == Some(n.as_str()));
                        if !dup {
                            sent.push(m);
                        }
                    }
                }
            }
            Some("tool_search_output") => {
                found.extend(it.get("tools").and_then(|x| x.as_array()).cloned().unwrap_or_default());
            }
            _ => {}
        }
    }
    let mut ts = ToolSet::default();
    for t in &sent {
        ts.add(t, false);
    }
    for t in &found {
        ts.add(t, true);
    }

    match body.get("input") {
        Some(Value::String(s)) => {
            ir.messages.push(Message { role: "user".into(), parts: vec![Part::text(s.clone())] });
        }
        Some(Value::Array(items)) => {
            let mut sys: Vec<String> = vec![ir.system.clone()];
            let orphan = |it: &Value| {
                matches!(get_str(it, "type").as_deref(), Some("function_call_output" | "custom_tool_call_output"))
                    && get_str(it, "call_id").unwrap_or_default().is_empty()
            };
            // 在场的结果：没有结果的调用补一条合成结果（Go pairToolItems）
            let mut answered: HashSet<String> = items
                .iter()
                .filter(|it| {
                    matches!(get_str(it, "type").as_deref(), Some("function_call_output" | "custom_tool_call_output"))
                })
                .filter_map(|it| get_str(it, "call_id"))
                .filter(|c| !c.is_empty())
                .collect();
            let mut synthetic: Vec<Message> = Vec::new();
            // 没有 call_id 的结果（Go orphanedToolOutputs）：变成 user 文本；
            // 夹在调用与结果之间的先攒着，等这一串调用/结果结束再放
            let mut pending: Vec<Message> = Vec::new();
            let (mut in_calls, mut had_results) = (false, false);
            for (i, it) in items.iter().enumerate() {
                let ty = get_str(it, "type").unwrap_or_default();
                let is_orphan = orphan(it);
                if !is_orphan {
                    if is_call_item(&ty) {
                        if had_results {
                            ir.messages.append(&mut pending);
                        }
                        in_calls = true;
                        had_results = false;
                    } else if is_result_item(&ty) {
                        had_results = true;
                    } else {
                        ir.messages.append(&mut pending);
                        in_calls = false;
                    }
                }
                'item: {
                    if ty == "message" || (ty.is_empty() && get_str(it, "role").is_some()) {
                        let role = get_str(it, "role").unwrap_or_default();
                        let parts = responses_parts(it.get("content").unwrap_or(&Value::Null));
                        if role == "system" || role == "developer" {
                            // instructions item / 消息形的 system：一律折叠进顶层
                            // system（Go 对「回合已开始后」的 developer item 另有
                            // system-reminder 保位处理，本矩阵简化为全折叠）
                            let t = string_or_text(it.get("content").unwrap_or(&Value::Null));
                            if !t.is_empty() {
                                sys.push(t);
                            }
                            break 'item;
                        }
                        let role = if role == "assistant" { "assistant" } else { "user" };
                        ir.messages.push(Message { role: role.into(), parts });
                        break 'item;
                    }
                    if is_call_item(&ty) {
                        let call_id = get_str(it, "call_id").unwrap_or_default();
                        let (name, args) = if ty == "tool_search_call" {
                            (TOOL_SEARCH.to_string(), call_args_of(it))
                        } else {
                            let mut ns = get_str(it, "namespace").unwrap_or_default();
                            if lite && ns == LITE_NAMESPACE {
                                ns.clear(); // Lite 的自家工具就是普通调用
                            }
                            let name = ts.flat_of(&ns, &get_str(it, "name").unwrap_or_default());
                            let args = if ty == "custom_tool_call" {
                                // 自由文本工具：包成 {"input": …}（Go 同款）
                                json!({"input": get_str(it, "input").unwrap_or_default()})
                            } else {
                                call_args_of(it)
                            };
                            (name, args)
                        };
                        ir.messages.push(Message {
                            role: "assistant".into(),
                            parts: vec![Part::tool_call(call_id.clone(), name, args)],
                        });
                        if (ty == "function_call" || ty == "custom_tool_call")
                            && !call_id.is_empty()
                            && !answered.contains(&call_id)
                        {
                            answered.insert(call_id.clone());
                            synthetic.push(Message {
                                role: "user".into(),
                                parts: vec![Part::tool_result(call_id, UNAVAILABLE_RESULT, Vec::new())],
                            });
                        }
                        break 'item;
                    }
                    if ty == "tool_search_output" {
                        let tools: Vec<Value> =
                            it.get("tools").and_then(|x| x.as_array()).cloned().unwrap_or_default();
                        let text = search_found(&tools, &|ns, n| ts.flat_of(ns, n));
                        ir.messages.push(Message {
                            role: "user".into(),
                            parts: vec![Part::tool_result(
                                get_str(it, "call_id").unwrap_or_default(),
                                text,
                                Vec::new(),
                            )],
                        });
                        break 'item;
                    }
                    if ty == "function_call_output" || ty == "custom_tool_call_output" {
                        let (text, images) = output_parts(it.get("output").unwrap_or(&Value::Null));
                        if is_orphan {
                            // 跨线程通知之类：要的是文字，不是假 call_id
                            let mut parts: Vec<Part> = Vec::new();
                            if !text.is_empty() {
                                parts.push(Part::text(text));
                            }
                            parts.extend(images);
                            if parts.is_empty() {
                                parts.push(Part::text("Tool result received."));
                            }
                            let m = Message { role: "user".into(), parts };
                            if in_calls {
                                pending.push(m); // chat 不许 tool_calls 与结果之间夹 user 消息
                            } else {
                                ir.messages.push(m);
                            }
                        } else {
                            ir.messages.push(Message {
                                role: "user".into(),
                                parts: vec![Part::tool_result(
                                    get_str(it, "call_id").unwrap_or_default(),
                                    text,
                                    images,
                                )],
                            });
                        }
                        break 'item;
                    }
                    if ty == "reasoning" {
                        // Go parseResponses 同款：item 带原文（content[].reasoning_text）
                        // 就用原文，否则退它的 summary 文本（magpie 给翻译应答的客户端
                        // 回的就是 summary）；加密内容读不了，丢。文本进 assistant 的
                        // Thinking 块，build_chat 暂存、chat.rs 按厂商决定回放与否。
                        let mut said = String::new();
                        if let Some(parts) = it.get("content").and_then(|c| c.as_array()) {
                            for c in parts {
                                if get_str(c, "type").as_deref() == Some("reasoning_text") {
                                    said.push_str(&get_str(c, "text").unwrap_or_default());
                                }
                            }
                        }
                        if said.is_empty() {
                            if let Some(parts) = it.get("summary").and_then(|c| c.as_array()) {
                                for c in parts {
                                    said.push_str(&get_str(c, "text").unwrap_or_default());
                                }
                            }
                        }
                        if !said.trim().is_empty() {
                            ir.messages.push(Message {
                                role: "assistant".into(),
                                parts: vec![Part::thinking(said)],
                            });
                        }
                        break 'item;
                    }
                    // web_search_call / local_shell_call / image_generation_call 等其余
                    // item：Go 同样不跨协议，丢弃
                }
                // 一串调用/结果的末尾：补合成结果（orphan 在 Go 里已是消息，不算串内）
                let exch = |x: &Value| {
                    let t = get_str(x, "type").unwrap_or_default();
                    (is_call_item(&t) || is_result_item(&t)) && !orphan(x)
                };
                if !synthetic.is_empty()
                    && exch(it)
                    && !items[i + 1..].iter().find(|n| !orphan(n)).map_or(false, |n| exch(n))
                {
                    ir.messages.append(&mut synthetic);
                }
            }
            ir.messages.append(&mut pending);
            ir.messages.append(&mut synthetic);
            ir.system = sys.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n");
        }
        _ => return Err("invalid request: input".into()),
    }
    ir.messages = merge_turns(std::mem::take(&mut ir.messages));
    ir.tools = std::mem::take(&mut ts.tools);
    ir.named = std::mem::take(&mut ts.named);
    match body.get("tool_choice") {
        Some(Value::String(s)) => ir.tool_choice = s.clone(),
        Some(choice) => {
            let pick = |c: &Value| {
                let ns = get_str(c, "namespace").unwrap_or_default();
                ts.flat_of(&ns, &get_str(c, "name").unwrap_or_default())
            };
            match get_str(choice, "type").as_deref() {
                Some("allowed_tools") => {
                    // Go：mode 变 tool_choice，工具清单收窄到允许的那几个
                    let mode = get_str(choice, "mode").unwrap_or_default();
                    if mode != "auto" && mode != "required" {
                        return Err(format!("invalid allowed_tools mode {mode:?}"));
                    }
                    ir.tool_choice = mode;
                    let allowed: HashSet<String> = choice
                        .get("tools")
                        .and_then(|t| t.as_array())
                        .into_iter()
                        .flatten()
                        .filter(|t| {
                            matches!(get_str(t, "type").as_deref(), Some("function" | "custom"))
                                && !get_str(t, "name").unwrap_or_default().is_empty()
                        })
                        .map(|t| pick(t))
                        .collect();
                    ir.tools.retain(|t| allowed.contains(&t.name));
                    ir.named.retain(|k, _| allowed.contains(k));
                }
                // 带 name 的点名（function / custom；无 type 的旧形态同样）
                _ => {
                    if !get_str(choice, "name").unwrap_or_default().is_empty() {
                        ir.tool_choice = format!("name:{}", pick(choice));
                    }
                }
            }
        }
        None => {}
    }
    Ok(ir)
}

// =====================================================================
// 构建：IR → 线协议
// =====================================================================

/// anthropic 的 image 块（imageBlock 同款：URL 直通，否则内联 base64）。
fn image_block(p: &Part) -> Value {
    if !p.url.is_empty() && p.data.is_empty() {
        json!({"type": "image", "source": {"type": "url", "url": p.url}})
    } else {
        json!({"type": "image", "source": {"type": "base64", "media_type": p.media_type, "data": p.data}})
    }
}

/// tool_choice 的三协议展开（buildChat/buildAnthropic/buildResponses 共用）。
fn tool_choice_chat(ir: &Ir) -> Value {
    match ir.tool_choice.as_str() {
        "auto" | "none" | "required" => json!(ir.tool_choice),
        s if s.starts_with("name:") => json!({"type": "function", "function": {"name": s[5..]}}),
        _ => Value::Null,
    }
}

fn tool_choice_anthropic(ir: &Ir) -> Value {
    match ir.tool_choice.as_str() {
        "auto" | "none" => json!({"type": ir.tool_choice}),
        "required" => json!({"type": "any"}),
        s if s.starts_with("name:") => json!({"type": "tool", "name": s[5..]}),
        _ => Value::Null,
    }
}

fn tool_choice_responses(ir: &Ir) -> Value {
    match ir.tool_choice.as_str() {
        "auto" | "none" | "required" => json!(ir.tool_choice),
        s if s.starts_with("name:") => json!({"type": "function", "name": s[5..]}),
        _ => Value::Null,
    }
}

/// IR → openai chat/completions 请求体（buildChat 同款子集）。
fn build_chat(ir: &Ir) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if !ir.system.is_empty() {
        messages.push(json!({"role": "system", "content": ir.system}));
    }
    for m in &ir.messages {
        if m.role == "assistant" {
            let text: String = m
                .parts
                .iter()
                .filter(|p| p.kind == PartKind::Text)
                .map(|p| p.text.as_str())
                .collect();
            let calls: Vec<Value> = m
                .parts
                .iter()
                .filter(|p| p.kind == PartKind::ToolCall)
                .map(|p| {
                    json!({"id": p.id, "type": "function",
                        "function": {"name": p.name, "arguments": args_string(p)}})
                })
                .collect();
            let think: String = m
                .parts
                .iter()
                .filter(|p| p.kind == PartKind::Thinking)
                .map(|p| p.text.as_str())
                .collect();
            // 只有思考、没有正文也没有调用的回合（尾部孤零零的 reasoning item）
            // 不成一条空 assistant 消息——多家厂商拒收空回合（Go silentAssistantsOut）。
            if text.is_empty() && calls.is_empty() && !think.is_empty() {
                continue;
            }
            let mut am = json!({"role": "assistant"});
            // 有调用且无正文时省掉 content（OpenAI 形态惯用；Go 同款）
            if !text.is_empty() || calls.is_empty() {
                am["content"] = json!(text);
            }
            if !calls.is_empty() {
                am["tool_calls"] = Value::Array(calls);
            }
            if !think.trim().is_empty() {
                // 暂存键：chat::fit_for_vendor 出站前一定摘掉（DeepSeek 系回放成
                // reasoning_content，其余丢弃）。
                am[super::chat::THINK_KEY] = json!(think);
            }
            messages.push(am);
            continue;
        }
        // user 侧：tool 消息先出（严格上游要求 tool 消息紧跟 tool_calls 的
        // assistant，正文消息垫后），正文再出
        let mut tool_msgs: Vec<Value> = Vec::new();
        for p in m.parts.iter().filter(|p| p.kind == PartKind::ToolResult) {
            // 注：与 Go 有意不同——Go 把工具附图挪到下一条 user 消息并加注
            // （见 buildChat 的 seeLater）；这里保留在 tool 消息的 content
            // 数组里（现代 chat completions 接受 parts 数组），省一条合成消息
            let content = if p.images.is_empty() {
                json!(p.text)
            } else {
                let mut items = vec![json!({"type": "text", "text": p.text})];
                for im in &p.images {
                    items.push(json!({"type": "image_url", "image_url": {"url": data_url(im)}}));
                }
                Value::Array(items)
            };
            tool_msgs.push(json!({"role": "tool", "tool_call_id": p.call_id, "content": content}));
        }
        messages.append(&mut tool_msgs);
        let content_parts: Vec<&Part> =
            m.parts.iter().filter(|p| p.kind != PartKind::ToolResult).collect();
        if !content_parts.is_empty() {
            if content_parts.iter().all(|p| p.kind == PartKind::Text) {
                // 纯文本：content 收成字符串（与 OpenAI 客户端的常见形态一致）
                let text: String =
                    content_parts.iter().map(|p| p.text.as_str()).collect();
                messages.push(json!({"role": "user", "content": text}));
            } else {
                let content: Vec<Value> = content_parts
                    .iter()
                    .map(|p| match p.kind {
                        PartKind::Image => {
                            json!({"type": "image_url", "image_url": {"url": data_url(p)}})
                        }
                        _ => json!({"type": "text", "text": p.text}),
                    })
                    .collect();
                messages.push(json!({"role": "user", "content": content}));
            }
        }
    }
    let mut out = json!({"model": ir.model, "messages": messages, "stream": ir.stream});
    if !ir.cache_key.is_empty() {
        // Go buildChat 同款：请求自己的 prompt_cache_key 原样带给 chat 厂商
        // （厂商不认会 400 点名它，chat::attempt 摘掉重发并记住）
        out["prompt_cache_key"] = json!(ir.cache_key);
    }
    if let Some(format) = &ir.format {
        // 结构化输出（Go buildChat：`out["response_format"] = r.Format.chat()`）；
        // 厂商不认会 400 点名它，chat::attempt 改成 json_object / 写进 system 重发。
        out["response_format"] = format.clone();
    }
    if ir.max_tokens > 0 {
        out["max_tokens"] = json!(ir.max_tokens);
    }
    if let Some(t) = ir.temperature {
        out["temperature"] = json!(t);
    }
    if let Some(p) = ir.top_p {
        out["top_p"] = json!(p);
    }
    if !ir.stop.is_empty() {
        out["stop"] = json!(ir.stop);
    }
    if !ir.effort.is_empty() {
        out["reasoning_effort"] = json!(ir.effort);
    }
    if !ir.tools.is_empty() {
        let tools: Vec<Value> = ir
            .tools
            .iter()
            .map(|t| {
                let mut f = json!({"name": t.name, "description": t.description});
                if !t.schema.is_null() {
                    f["parameters"] = t.schema.clone();
                }
                json!({"type": "function", "function": f})
            })
            .collect();
        out["tools"] = Value::Array(tools);
        let tc = tool_choice_chat(ir);
        if !tc.is_null() {
            out["tool_choice"] = tc;
        }
        // Go buildChat 同款：只在带工具时转发 parallel_tool_calls
        if let Some(p) = ir.parallel {
            out["parallel_tool_calls"] = json!(p);
        }
    }
    out
}

/// IR → anthropic /v1/messages 请求体（anthropicBody 同款子集）。
fn build_anthropic(ir: &Ir) -> Value {
    // (role, blocks)：push 语义——相邻同角色合并成一条消息（chat 的连续
    // tool 消息必须并成一条 user 的多个 tool_result 块，anthropic 拒绝
    // 未配对的 tool_use）
    let mut msgs: Vec<(String, Vec<Value>)> = Vec::new();
    for m in &ir.messages {
        let role = if m.role == "assistant" { "assistant" } else { "user" };
        let mut results: Vec<Value> = Vec::new();
        let mut rest: Vec<Value> = Vec::new();
        for p in &m.parts {
            match p.kind {
                PartKind::Text => {
                    // 纯空白文本块没有意义，anthropic 也拒收空 text
                    if !p.text.trim().is_empty() {
                        rest.push(json!({"type": "text", "text": p.text}));
                    }
                }
                PartKind::Image => rest.push(image_block(p)),
                PartKind::ToolCall => {
                    rest.push(json!({"type": "tool_use", "id": p.id, "name": p.name, "input": args_of(p)}));
                }
                // 思考不跨协议（anthropic 的 thinking 块要签名，伪造会被拒）
                PartKind::Thinking => {}
                PartKind::ToolResult => {
                    let content = if p.images.is_empty() {
                        json!(p.text)
                    } else {
                        let mut blocks = Vec::new();
                        if !p.text.trim().is_empty() {
                            blocks.push(json!({"type": "text", "text": p.text}));
                        }
                        for im in &p.images {
                            blocks.push(image_block(im));
                        }
                        Value::Array(blocks)
                    };
                    let mut b =
                        json!({"type": "tool_result", "tool_use_id": p.call_id, "content": content});
                    if p.is_error {
                        b["is_error"] = json!(true);
                    }
                    results.push(b);
                }
            }
        }
        // tool_result 块必须排在 user 消息的其它块前面（anthropic 的顺序要求）
        results.extend(rest);
        if results.is_empty() {
            continue;
        }
        if let Some(last) = msgs.last_mut() {
            if last.0 == role {
                last.1.extend(results);
                continue;
            }
        }
        msgs.push((role.to_string(), results));
    }
    let messages: Vec<Value> = msgs
        .into_iter()
        .map(|(role, blocks)| json!({"role": role, "content": blocks}))
        .collect();
    let mut out = json!({"model": ir.model, "messages": messages, "stream": ir.stream});
    if !ir.system.is_empty() {
        // 单 text 块（Go 同款形态；cache_control 由 anthropic→anthropic 透传层负责）
        out["system"] = json!([{"type": "text", "text": ir.system}]);
    }
    // max_tokens 必填：没带就兜底
    out["max_tokens"] = json!(if ir.max_tokens > 0 { ir.max_tokens } else { ANTHROPIC_MAX_TOKENS_FLOOR });
    if let Some(t) = ir.temperature {
        out["temperature"] = json!(t);
    }
    if let Some(p) = ir.top_p {
        out["top_p"] = json!(p);
    }
    if !ir.stop.is_empty() {
        out["stop_sequences"] = json!(ir.stop);
    }
    if !ir.effort.is_empty() {
        // 思考预算必须给回答留余地（Go withEffort 同款）：放不下就不开。
        let max = out["max_tokens"].as_u64().unwrap_or(0);
        let budget = super::effort::budget_of(&ir.effort).min(max.saturating_sub(1));
        if budget >= 1024 {
            out["thinking"] = json!({"type": "enabled", "budget_tokens": budget});
        }
    }
    if !ir.tools.is_empty() {
        let tools: Vec<Value> = ir
            .tools
            .iter()
            .map(|t| {
                json!({"name": t.name, "description": t.description,
                    "input_schema": if t.schema.is_null() {
                        // anthropic 拒绝缺 schema 的工具定义（Go 补空对象同款）
                        json!({"type": "object", "properties": {}})
                    } else {
                        t.schema.clone()
                    }})
            })
            .collect();
        out["tools"] = Value::Array(tools);
        let tc = tool_choice_anthropic(ir);
        if !tc.is_null() {
            out["tool_choice"] = tc;
        }
    }
    out
}

/// IR → openai responses 请求体（buildResponses 同款子集）。
fn build_responses(ir: &Ir) -> Value {
    let mut input: Vec<Value> = Vec::new();
    for m in &ir.messages {
        let role = if m.role == "assistant" { "assistant" } else { "user" };
        // 正文块攒进本条消息的 message item；工具项是独立 item，出现时
        // 先冲刷正文（responses 的 item 是平铺的，一条消息一项）
        fn flush(input: &mut Vec<Value>, role: &str, content: &mut Vec<Value>) {
            if !content.is_empty() {
                input.push(json!({"type": "message", "role": role, "content": std::mem::take(content)}));
            }
        }
        let mut content: Vec<Value> = Vec::new();
        for p in &m.parts {
            match p.kind {
                PartKind::Text => {
                    let t = if role == "assistant" { "output_text" } else { "input_text" };
                    content.push(json!({"type": t, "text": p.text}));
                }
                PartKind::Image => {
                    // assistant 侧不带图（responses 输出消息无图位，Go 同款丢弃）
                    if role != "assistant" {
                        content.push(json!({"type": "input_image", "image_url": data_url(p), "detail": "auto"}));
                    }
                }
                PartKind::ToolCall => {
                    flush(&mut input, role, &mut content);
                    input.push(json!({"type": "function_call", "call_id": p.id, "name": p.name,
                        "arguments": args_string(p)}));
                }
                // 其它协议的思考文本不塞进 responses（reasoning item 要它自己的 id/加密体）
                PartKind::Thinking => {}
                PartKind::ToolResult => {
                    flush(&mut input, role, &mut content);
                    let output = if p.images.is_empty() {
                        json!(p.text)
                    } else {
                        let mut items = Vec::new();
                        if !p.text.trim().is_empty() {
                            items.push(json!({"type": "input_text", "text": p.text}));
                        }
                        for im in &p.images {
                            items.push(json!({"type": "input_image", "image_url": data_url(im), "detail": "auto"}));
                        }
                        Value::Array(items)
                    };
                    input.push(json!({"type": "function_call_output", "call_id": p.call_id, "output": output}));
                }
            }
        }
        flush(&mut input, role, &mut content);
    }
    let mut out = json!({"model": ir.model, "input": input, "stream": ir.stream, "store": false});
    if !ir.system.is_empty() {
        out["instructions"] = json!(ir.system);
    }
    if ir.max_tokens > 0 {
        out["max_output_tokens"] = json!(ir.max_tokens);
    }
    if let Some(t) = ir.temperature {
        out["temperature"] = json!(t);
    }
    if let Some(p) = ir.top_p {
        out["top_p"] = json!(p);
    }
    if !ir.effort.is_empty() {
        out["reasoning"] = json!({"effort": ir.effort});
    }
    if !ir.tools.is_empty() {
        let tools: Vec<Value> = ir
            .tools
            .iter()
            .map(|t| {
                let mut tool =
                    json!({"type": "function", "name": t.name, "description": t.description, "strict": false});
                if !t.schema.is_null() {
                    tool["parameters"] = t.schema.clone();
                }
                tool
            })
            .collect();
        out["tools"] = Value::Array(tools);
        let tc = tool_choice_responses(ir);
        if !tc.is_null() {
            out["tool_choice"] = tc;
        }
    }
    out
}

// =====================================================================
// 公开入口：请求互转
// =====================================================================

/// openai chat/completions 请求体 → anthropic /v1/messages 请求体。
/// system 提取为顶层 system；image_url → image source（data: URL 拆内联
/// base64，其余按 URL）；tools 的 function 定义映射；max_tokens 必填兜底。
pub fn chat_to_anthropic(body: &Value) -> Result<Value, String> {
    parse_chat(body).map(|ir| build_anthropic(&ir))
}

/// anthropic /v1/messages 请求体 → openai chat/completions 请求体（反向）。
pub fn anthropic_to_chat(body: &Value) -> Result<Value, String> {
    parse_anthropic(body).map(|ir| build_chat(&ir))
}

/// openai chat/completions 请求体 → openai responses 请求体
/// （messages → input items、system → instructions、tools 扁平化）。
pub fn chat_to_responses(body: &Value) -> Result<Value, String> {
    parse_chat(body).map(|ir| build_responses(&ir))
}

/// openai responses 请求体 → openai chat/completions 请求体（反向）：
/// input 的 message / function_call / function_call_output 映射回
/// messages + tool_calls / tool 角色。
pub fn responses_to_chat(body: &Value) -> Result<Value, String> {
    responses_to_chat_with_tools(body).map(|(chat, _)| chat)
}

/// [`responses_to_chat`] + 压平工具的还原表：Codex 的 custom / namespace /
/// tool_search 工具以 flat 函数名交给 chat 厂商，应答编码器拿这张表把
/// 厂商的调用还原成 Codex 认的 item（`call_to`）。表随请求走，不放全局。
pub fn responses_to_chat_with_tools(body: &Value) -> Result<(Value, ToolTable), String> {
    let ir = parse_responses(body)?;
    let chat = build_chat(&ir);
    Ok((chat, ir.named))
}

// =====================================================================
// SSE 事件映射（无状态单事件翻译）
// =====================================================================

/// 一条 anthropic SSE 事件的完整帧（event: 行 + data: 行）。
fn sse_block(event: &str, data: &Value) -> String {
    format!("event: {}\ndata: {}", event, serde_json::to_string(data).unwrap_or_default())
}

/// chat 的 stop_reason → anthropic 的（stopToAnthropic 同款）。
fn chat_stop_to_anthropic(stop: &str) -> &'static str {
    match stop {
        "length" => "max_tokens",
        "tool_calls" | "function_call" => "tool_use",
        "content_filter" => "refusal",
        _ => "end_turn",
    }
}

/// anthropic 的 stop_reason → chat 的（stopFromAnthropic 的线值版）。
fn anthropic_stop_to_chat(stop: &str) -> &'static str {
    match stop {
        "max_tokens" | "model_context_window_exceeded" => "length",
        "tool_use" | "tool_calls" | "function_call" => "tool_calls",
        "refusal" | "content_filter" => "content_filter",
        _ => "stop",
    }
}

fn num(v: Option<&Value>) -> u64 {
    v.and_then(|x| x.as_u64()).unwrap_or(0)
}

/// chat usage → anthropic usage，口径同 Go（chat.go `cUsage.usage()` →
/// anthropic.go `Usage.anthropic()`）：
/// - 缓存读 = `prompt_tokens_details.cached_tokens`，缺了依次退
///   `prompt_cache_hit_tokens`（DeepSeek）/`cached_tokens`（Moonshot）/
///   `num_cached_tokens`（Mistral）；缓存写 = `prompt_tokens_details.cache_write_tokens`
///   （OpenRouter）；两者都没有时认 Anthropic 自己的名字
///   `cache_read_input_tokens` / `cache_creation_input_tokens`（前面有 Claude 的中转）；
/// - OpenAI 的 `prompt_tokens` **含**缓存，Anthropic 的 `input_tokens` **不含**——
///   输出 input = prompt − 读 − 写（下限 0）；缓存数大于 prompt 说明上游本来就按
///   Anthropic 口径报（prompt 不含缓存），那就原样当 input。Claude Code 的上下文
///   占用 = input + cache_read + cache_creation，双计会让它高估、过早压缩；
/// - 思考 token 有就带 `output_tokens_details.thinking_tokens`。
/// 上游没给 usage 就全 0（不编数）。
pub(crate) fn anthropic_usage(u: Option<&Value>) -> Value {
    let Some(u) = u.filter(|u| u.is_object()) else {
        return json!({
            "input_tokens": 0, "output_tokens": 0,
            "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0,
        });
    };
    let prompt = num(u.get("prompt_tokens"));
    let mut read = num(u.pointer("/prompt_tokens_details/cached_tokens"));
    let mut write = num(u.pointer("/prompt_tokens_details/cache_write_tokens"));
    if read == 0 {
        for key in ["prompt_cache_hit_tokens", "cached_tokens", "num_cached_tokens"] {
            if read == 0 {
                read = num(u.get(key));
            }
        }
    }
    let mut whole = true; // prompt_tokens 含缓存（OpenAI 口径）
    if read == 0 && write == 0 {
        read = num(u.get("cache_read_input_tokens"));
        write = num(u.get("cache_creation_input_tokens"));
        whole = prompt >= read + write;
    }
    let input = if whole { prompt.saturating_sub(read + write) } else { prompt };
    let mut out = json!({
        "input_tokens": input,
        "output_tokens": num(u.get("completion_tokens")),
        "cache_read_input_tokens": read,
        "cache_creation_input_tokens": write,
    });
    let thinking = num(u.pointer("/completion_tokens_details/reasoning_tokens"));
    if thinking > 0 {
        out["output_tokens_details"] = json!({ "thinking_tokens": thinking });
    }
    out
}

/// anthropic usage → chat usage（Go `aUsage.usage()` → `Usage.chat()`）：
/// prompt_tokens = input + 缓存读 + 缓存写（OpenAI 口径含缓存）；缓存写取
/// `cache_creation_input_tokens` 与 `cache_creation` 分档合计的较大者。
fn chat_usage(u: &Value) -> Value {
    let input = num(u.get("input_tokens"));
    let output = num(u.get("output_tokens"));
    let read = num(u.get("cache_read_input_tokens"));
    let split = num(u.pointer("/cache_creation/ephemeral_5m_input_tokens"))
        + num(u.pointer("/cache_creation/ephemeral_1h_input_tokens"));
    let write = num(u.get("cache_creation_input_tokens")).max(split);
    let prompt = input + read + write;
    let mut out = json!({
        "prompt_tokens": prompt,
        "completion_tokens": output,
        "total_tokens": prompt + output,
        "prompt_tokens_details": { "cached_tokens": read, "cache_write_tokens": write },
    });
    let thinking = num(u.pointer("/output_tokens_details/thinking_tokens"));
    if thinking > 0 {
        out["completion_tokens_details"] = json!({ "reasoning_tokens": thinking });
    }
    out
}

/// 一条 chat.completion.chunk 的 data JSON → 对应的 anthropic 事件帧：
/// 文本 delta → content_block_delta；finish_reason（可能带 usage）→
/// message_delta + message_stop；纯 usage 帧 → message_delta；`[DONE]` →
/// message_stop。无对应（role 首帧、tool_calls 分片…）返回 None。
/// 返回值可能含多个帧（\n\n 连接），调用方写流时自行补帧分隔。
pub fn sse_chat_to_anthropic(event_data: &str) -> Option<String> {
    let data = event_data.trim();
    if data == "[DONE]" {
        return Some(sse_block("message_stop", &json!({"type": "message_stop"})));
    }
    let v: Value = serde_json::from_str(data).ok()?;
    let choice = v.pointer("/choices/0");
    let text = choice
        .and_then(|c| c.pointer("/delta/content"))
        .and_then(|d| d.as_str())
        .unwrap_or("");
    if !text.is_empty() {
        return Some(sse_block(
            "content_block_delta",
            &json!({"type": "content_block_delta", "index": 0,
                "delta": {"type": "text_delta", "text": text}}),
        ));
    }
    let finish = choice
        .and_then(|c| c.get("finish_reason"))
        .and_then(|f| f.as_str())
        .filter(|s| !s.is_empty());
    let usage = v.get("usage").filter(|u| u.is_object());
    if let Some(stop) = finish {
        // anthropic 的收尾是一条 message_delta（stop + usage 同帧）接 message_stop
        let mut blocks = vec![sse_block(
            "message_delta",
            &json!({"type": "message_delta",
                "delta": {"stop_reason": chat_stop_to_anthropic(stop), "stop_sequence": null},
                "usage": anthropic_usage(usage)}),
        )];
        blocks.push(sse_block("message_stop", &json!({"type": "message_stop"})));
        return Some(blocks.join("\n\n"));
    }
    if usage.is_some() {
        // stream_options.include_usage 的末帧（choices 为空）：只有 usage
        return Some(sse_block(
            "message_delta",
            &json!({"type": "message_delta",
                "delta": {"stop_reason": null, "stop_sequence": null},
                "usage": anthropic_usage(usage)}),
        ));
    }
    None
}

/// 一条 anthropic 事件的 data JSON → 对应的 chat.completion.chunk 帧：
/// text_delta → content delta；message_delta（stop/usage）→ finish 帧
/// （+ 独立 usage 帧）；message_start → role 首帧；message_stop →
/// `data: [DONE]`。返回 `data: …` 行（可能多帧 \n\n 连接）。
/// id/created/model 是占位（纯函数无会话状态），需要时调用方重写。
pub fn sse_anthropic_to_chat(event_data: &str) -> Option<String> {
    let v: Value = serde_json::from_str(event_data.trim()).ok()?;
    let ty = v.get("type").and_then(|t| t.as_str())?;
    let chunk = |delta: Value, finish: Option<&str>, usage: Option<Value>| -> String {
        format!(
            "data: {}",
            serde_json::to_string(&json!({
                "id": "chatcmpl-translated",
                "object": "chat.completion.chunk",
                "created": 0,
                "model": "",
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
                "usage": usage,
            }))
            .unwrap_or_default()
        )
    };
    match ty {
        "message_start" => Some(chunk(json!({"role": "assistant", "content": ""}), None, None)),
        "content_block_delta" => {
            let is_text = v.pointer("/delta/type").and_then(|t| t.as_str()) == Some("text_delta");
            let text = v.pointer("/delta/text").and_then(|t| t.as_str()).unwrap_or("");
            if is_text && !text.is_empty() {
                Some(chunk(json!({"content": text}), None, None))
            } else {
                None // input_json_delta / thinking_delta 等：工具与思考分片不在本映射
            }
        }
        "message_delta" => {
            let stop = v
                .pointer("/delta/stop_reason")
                .and_then(|s| s.as_str())
                .filter(|s| !s.is_empty() && *s != "null");
            let usage = v.get("usage").filter(|u| u.is_object());
            match (stop, usage) {
                (Some(s), u) => {
                    // Go chatEncoder 的收尾形态：finish 帧与 usage 帧分开
                    let mut blocks = vec![chunk(json!({}), Some(anthropic_stop_to_chat(s)), None)];
                    if let Some(u) = u {
                        blocks.push(chunk(json!({}), None, Some(chat_usage(u))));
                    }
                    Some(blocks.join("\n\n"))
                }
                (None, Some(u)) => Some(chunk(json!({}), None, Some(chat_usage(u)))),
                (None, None) => None,
            }
        }
        "message_stop" => Some("data: [DONE]".into()),
        _ => None,
    }
}

// =====================================================================
// chat SSE 的 tool_calls 分片拼装（Go chatDecoder.toolCall 的子集）
// =====================================================================

/// 拼装完成的一次工具调用（chat 流侧的最终形态）。
#[derive(Clone, PartialEq, Debug)]
pub struct ChatToolCall {
    /// chat 的 call id（可空：个别厂商不给——端点侧兜底合成）
    pub id: String,
    pub name: String,
    /// arguments 的跨帧拼接结果（JSON 文本，原样不修饰）
    pub arguments: String,
}

/// chat SSE `delta.tool_calls[]` 分片拼装器（responses.rs / messages.rs
/// 两个流式翻译端点共享）。识别「同一次调用」的口径照 Go chatDecoder：
/// 分片的 index 相同且 id 相容（缺 id = 续片——有些中转厂每片都带 id，
/// 规范只在第一片带）→ arguments 追加；否则开下一条调用。
/// 拼装是**惰性**的：分片只进账，事件族由端点在收尾（finish/[DONE]/EOF）
/// 或交错（工具分片之后又来正文）时按首片到达序冲刷——chat 协议没有
/// 「这一次调用完了」的显式信号，完成点只能由流的边界推。
/// 未覆盖：乱序 parallel 分片的**事件序**（0/1 交替来时拼装正确、
/// 按首片序发族，但不再反映厂商自己的交错讲述序）。
pub struct ChatToolAccum {
    /// 拼装中的调用（首片到达序）；index 同时记账用于续片匹配。
    calls: Vec<(u64, ChatToolCall)>,
}

impl ChatToolAccum {
    pub fn new() -> ChatToolAccum {
        ChatToolAccum { calls: Vec::new() }
    }

    pub fn is_empty(&self) -> bool {
        self.calls.is_empty()
    }

    /// 一条 chunk 的 `delta.tool_calls` 数组喂进来。分片没带 index 时按
    /// 数组位次当 index（Go chatDecoder 同款兜底）。
    pub fn feed(&mut self, tool_calls: &[Value]) {
        for (pos, tc) in tool_calls.iter().enumerate() {
            let index = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(pos as u64);
            let id = tc.get("id").and_then(|i| i.as_str()).unwrap_or("");
            let name = tc.pointer("/function/name").and_then(|n| n.as_str()).unwrap_or("");
            let args = tc
                .pointer("/function/arguments")
                .and_then(|a| a.as_str())
                .unwrap_or("");
            // 倒着找同 index 的调用（最新的优先）：id 相容即续片。
            if let Some((_, call)) = self
                .calls
                .iter_mut()
                .rev()
                .find(|(i, c)| *i == index && (id.is_empty() || id == c.id))
            {
                call.arguments.push_str(args);
                if call.name.is_empty() && !name.is_empty() {
                    call.name = name.to_string();
                }
            } else {
                self.calls.push((
                    index,
                    ChatToolCall {
                        id: id.to_string(),
                        name: name.to_string(),
                        arguments: args.to_string(),
                    },
                ));
            }
        }
    }

    /// 全部取走（首片到达序），拼装器清空。端点在收尾/交错冲刷时调用。
    /// 拼好的 arguments 过 [`super::integral::whole_args`] 丢零分数——
    /// 模型写 `2500.0` 这类整值分数会被 Codex 的 serde 整单拒绝
    /// （Go grok_integral.go #1431/#3152）；整块改写与分片流改写等价。
    pub fn take(&mut self) -> Vec<ChatToolCall> {
        self.calls
            .drain(..)
            .map(|(_, mut c)| {
                c.arguments = super::integral::whole_args(&c.arguments);
                c
            })
            .collect()
    }
}

// =====================================================================
// 模型 id 剥离（marionette_peel.go 的对应）
// =====================================================================

/// Codex [effort] 后缀的合法集。照抄 Go 侧 provider.MemberEfforts 全表
/// （none/minimal 也在表里——任务简报只列了五档，但 Go 原文件就是全表，
/// 裁掉会让 model[none] 这类别名剥不掉而后 404）。
const MEMBER_EFFORTS: [&str; 7] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"];

/// 剥 Claude Code 的 `[1m]` 窗口标记和 Codex ACP 的 `[effort]` 后缀
/// （gpt-5.6-luna[xhigh] → gpt-5.6-luna），让组/来源查找仍按裸名匹配。
/// effort 大小写不敏感；`[1m]` 大小写敏感（Go TrimSuffix 同款）；
/// 非 effort 的方括号后缀原样保留。
pub fn peel_agent_model_mark(id: &str) -> String {
    let mut id = id.trim();
    if let Some(stripped) = id.strip_suffix("[1m]") {
        id = stripped;
    }
    if let Some(i) = id.rfind('[') {
        // i > 0：整个 id 就是一个 [..] 时不是后缀（Go 同款判断）
        if i > 0 && id.ends_with(']') {
            let inner = id[i + 1..id.len() - 1].to_ascii_lowercase();
            if MEMBER_EFFORTS.contains(&inner.as_str()) {
                return id[..i].to_string();
            }
        }
    }
    id.to_string()
}

// =====================================================================
// Codex 订阅入站所需的 responses 应答面翻译（新增，不改上面任何行为）
//   anthropic 请求 → responses 请求；responses SSE / 聚合对象 → chat
//   （anthropic 出站再接 messages.rs 既有的 chat → anthropic 翻译器）
// =====================================================================

/// anthropic /v1/messages 请求体 → openai responses 请求体。
pub fn anthropic_to_responses(body: &Value) -> Result<Value, String> {
    parse_anthropic(body).map(|ir| build_responses(&ir))
}

/// responses 的 usage → chat usage（cached / reasoning 两个明细桶带上；
/// 上游不报的桶就是缺省，不编数）。
fn responses_usage_to_chat(u: &Value) -> Value {
    let input = u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let output = u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let total = u.get("total_tokens").and_then(|v| v.as_u64()).unwrap_or(input + output);
    let mut out = json!({
        "prompt_tokens": input,
        "completion_tokens": output,
        "total_tokens": total,
        "prompt_tokens_details": {
            "cached_tokens": u.pointer("/input_tokens_details/cached_tokens").and_then(|v| v.as_u64()).unwrap_or(0)
        },
    });
    if let Some(r) = u.pointer("/output_tokens_details/reasoning_tokens").and_then(|v| v.as_u64()) {
        out["completion_tokens_details"] = json!({ "reasoning_tokens": r });
    }
    out
}

/// responses 终态 → chat finish_reason。
fn responses_finish_reason(resp: &Value, saw_tool: bool) -> &'static str {
    if resp.get("status").and_then(|s| s.as_str()) == Some("incomplete") {
        match resp.pointer("/incomplete_details/reason").and_then(|r| r.as_str()) {
            Some("max_output_tokens") => return "length",
            Some("content_filter") => return "content_filter",
            _ => {}
        }
    }
    if saw_tool {
        "tool_calls"
    } else {
        "stop"
    }
}

/// 厂商（改名后的）工具名 → agent 声明的名（Go codexNamesBack）。
pub type NamesBack = std::collections::HashMap<String, String>;

fn name_back(back: &NamesBack, name: &str) -> String {
    back.get(name).cloned().unwrap_or_else(|| name.to_string())
}

/// responses SSE → chat SSE 的有状态翻译器（无 IO：吃一条 `data:` JSON，
/// 吐 0..n 条完整的 chat 帧文本 `data: …\n\n`）。流式 Read 适配器与测试共用。
pub struct ResponsesToChat {
    model: String,
    id: String,
    include_usage: bool,
    back: NamesBack,
    started: bool,
    finished: bool,
    /// output_index → chat tool_calls 的 index
    tools: std::collections::HashMap<u64, usize>,
    /// 已发过 arguments 的 output_index（done 事件只补没发过的）
    args_sent: std::collections::HashSet<u64>,
    next_tool: usize,
    saw_tool: bool,
    last_tool_out: Option<u64>,
}

impl ResponsesToChat {
    pub fn new(model: &str, include_usage: bool, back: NamesBack) -> ResponsesToChat {
        ResponsesToChat {
            model: model.to_string(),
            id: "chatcmpl-codex".to_string(),
            include_usage,
            back,
            started: false,
            finished: false,
            tools: Default::default(),
            args_sent: Default::default(),
            next_tool: 0,
            saw_tool: false,
            last_tool_out: None,
        }
    }

    /// 已收口（completed/failed/error 之后的一切尾巴丢弃）。
    pub fn finished(&self) -> bool {
        self.finished
    }

    fn frame(&self, delta: Value, finish: Option<&str>) -> String {
        let v = json!({
            "id": self.id,
            "object": "chat.completion.chunk",
            "created": std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            "model": self.model,
            "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
        });
        format!("data: {}\n\n", serde_json::to_string(&v).unwrap_or_default())
    }

    fn start(&mut self, out: &mut Vec<String>) {
        if !self.started {
            self.started = true;
            out.push(self.frame(json!({"role": "assistant", "content": ""}), None));
        }
    }

    fn error_frames(&mut self, message: &str, code: &str) -> Vec<String> {
        self.finished = true;
        let v = json!({"error": {"message": message, "type": "upstream_error", "code": code}});
        vec![format!("data: {}\n\n", serde_json::to_string(&v).unwrap_or_default()), "data: [DONE]\n\n".into()]
    }

    fn tool_start(&mut self, out_index: u64, item: &Value, out: &mut Vec<String>) {
        if self.tools.contains_key(&out_index) {
            return;
        }
        let idx = self.next_tool;
        self.next_tool += 1;
        self.tools.insert(out_index, idx);
        self.saw_tool = true;
        self.last_tool_out = Some(out_index);
        let id = item
            .get("call_id")
            .and_then(|c| c.as_str())
            .filter(|c| !c.is_empty())
            .or_else(|| item.get("id").and_then(|c| c.as_str()))
            .unwrap_or("");
        let name = name_back(&self.back, item.get("name").and_then(|n| n.as_str()).unwrap_or(""));
        self.start(out);
        out.push(self.frame(
            json!({"tool_calls": [{"index": idx, "id": id, "type": "function",
                                    "function": {"name": name, "arguments": ""}}]}),
            None,
        ));
    }

    fn tool_args(&mut self, out_index: u64, args: &str, out: &mut Vec<String>) {
        let Some(&idx) = self.tools.get(&out_index) else { return };
        if args.is_empty() {
            return;
        }
        self.args_sent.insert(out_index);
        out.push(self.frame(json!({"tool_calls": [{"index": idx, "function": {"arguments": args}}]}), None));
    }

    pub fn feed(&mut self, data: &str) -> Vec<String> {
        let mut out = Vec::new();
        if self.finished {
            return out;
        }
        let Ok(ev) = serde_json::from_str::<Value>(data.trim()) else { return out };
        let ty = ev.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let out_index = ev.get("output_index").and_then(|i| i.as_u64());
        match ty {
            "response.created" | "response.in_progress" => {
                if let Some(id) = ev.pointer("/response/id").and_then(|i| i.as_str()) {
                    self.id = format!("chatcmpl-{id}");
                }
                self.start(&mut out);
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                let d = ev.get("delta").and_then(|d| d.as_str()).unwrap_or("");
                if !d.is_empty() {
                    self.start(&mut out);
                    out.push(self.frame(json!({"content": d}), None));
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let d = ev.get("delta").and_then(|d| d.as_str()).unwrap_or("");
                if !d.is_empty() {
                    self.start(&mut out);
                    out.push(self.frame(json!({"reasoning_content": d}), None));
                }
            }
            "response.output_item.added" => {
                if let Some(item) = ev.get("item").filter(|i| i.get("type").and_then(|t| t.as_str()) == Some("function_call")) {
                    let oi = out_index.unwrap_or(self.next_tool as u64);
                    self.tool_start(oi, item, &mut out);
                }
            }
            "response.function_call_arguments.delta" => {
                let d = ev.get("delta").and_then(|d| d.as_str()).unwrap_or("");
                if let Some(oi) = out_index.or(self.last_tool_out) {
                    self.tool_args(oi, d, &mut out);
                }
            }
            "response.output_item.done" => {
                if let Some(item) = ev.get("item").filter(|i| i.get("type").and_then(|t| t.as_str()) == Some("function_call")) {
                    let oi = out_index.unwrap_or(self.next_tool as u64);
                    // 没见过 added（直接给整项）：补开头
                    self.tool_start(oi, item, &mut out);
                    if !self.args_sent.contains(&oi) {
                        let a = item.get("arguments").and_then(|a| a.as_str()).unwrap_or("");
                        self.tool_args(oi, a, &mut out);
                    }
                }
            }
            "response.completed" | "response.incomplete" => {
                let resp = ev.get("response").cloned().unwrap_or(Value::Null);
                self.start(&mut out);
                let finish = responses_finish_reason(&resp, self.saw_tool);
                out.push(self.frame(json!({}), Some(finish)));
                if self.include_usage {
                    if let Some(u) = resp.get("usage").filter(|u| u.is_object()) {
                        let v = json!({
                            "id": self.id, "object": "chat.completion.chunk", "created": 0,
                            "model": self.model, "choices": [], "usage": responses_usage_to_chat(u),
                        });
                        out.push(format!("data: {}\n\n", serde_json::to_string(&v).unwrap_or_default()));
                    }
                }
                out.push("data: [DONE]\n\n".into());
                self.finished = true;
            }
            "response.failed" => {
                let msg = ev
                    .pointer("/response/error/message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("Codex response failed")
                    .to_string();
                let code = ev.pointer("/response/error/code").and_then(|m| m.as_str()).unwrap_or("response_failed").to_string();
                return self.error_frames(&msg, &code);
            }
            "error" => {
                let msg = ev
                    .get("message")
                    .or_else(|| ev.pointer("/error/message"))
                    .and_then(|m| m.as_str())
                    .unwrap_or("Codex stream error")
                    .to_string();
                let code = ev.get("code").or_else(|| ev.pointer("/error/code")).and_then(|m| m.as_str()).unwrap_or("stream_error").to_string();
                return self.error_frames(&msg, &code);
            }
            _ => {}
        }
        out
    }

    /// 上游在 completed 之前就 EOF：如实报错（不编 finish_reason）。
    pub fn truncated(&mut self) -> Vec<String> {
        if self.finished {
            return Vec::new();
        }
        self.error_frames("Codex stream ended before response.completed", "stream_truncated")
    }
}

/// 包在上游 responses SSE reader 外的 Read 适配器，吐 chat SSE 字节。
pub struct ResponsesToChatReader {
    inner: std::io::BufReader<Box<dyn std::io::Read + Send>>,
    conv: ResponsesToChat,
    out: Vec<u8>,
    pos: usize,
    done: bool,
}

impl ResponsesToChatReader {
    pub fn new(reader: Box<dyn std::io::Read + Send>, conv: ResponsesToChat) -> ResponsesToChatReader {
        ResponsesToChatReader { inner: std::io::BufReader::new(reader), conv, out: Vec::new(), pos: 0, done: false }
    }
}

impl std::io::Read for ResponsesToChatReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use std::io::BufRead;
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
            if self.done || self.conv.finished() {
                self.done = true;
                return Ok(0);
            }
            let mut line = String::new();
            match self.inner.read_line(&mut line) {
                Ok(0) => {
                    for f in self.conv.truncated() {
                        self.out.extend_from_slice(f.as_bytes());
                    }
                    self.done = true;
                    if self.out.is_empty() {
                        return Ok(0);
                    }
                    continue;
                }
                Ok(_) => {}
                Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    // 读断：同截断处理（错误帧让客户端知道不是正常收口）
                    for f in self.conv.truncated() {
                        self.out.extend_from_slice(f.as_bytes());
                    }
                    self.done = true;
                    if self.out.is_empty() {
                        return Ok(0);
                    }
                    continue;
                }
            }
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            let Some(data) = trimmed.strip_prefix("data:") else { continue };
            for f in self.conv.feed(data.trim()) {
                self.out.extend_from_slice(f.as_bytes());
            }
        }
    }
}

/// 聚合后的 responses 对象 → chat.completion 应答（非流式）。
pub fn responses_object_to_chat(resp: &Value, model: &str, back: &NamesBack) -> Value {
    let mut text = String::new();
    let mut reasoning = String::new();
    let mut calls: Vec<Value> = Vec::new();
    for item in resp.get("output").and_then(|o| o.as_array()).into_iter().flatten() {
        match item.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "message" => match item.get("content") {
                Some(Value::String(s)) => text.push_str(s),
                Some(Value::Array(parts)) => {
                    for p in parts {
                        match p.get("type").and_then(|t| t.as_str()) {
                            Some("output_text") | Some("refusal") | Some("text") => {
                                if let Some(t) = p.get("text").or_else(|| p.get("refusal")).and_then(|t| t.as_str()) {
                                    text.push_str(t);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            },
            "reasoning" => {
                for p in item.get("summary").and_then(|s| s.as_array()).into_iter().flatten() {
                    if let Some(t) = p.get("text").and_then(|t| t.as_str()) {
                        if !reasoning.is_empty() {
                            reasoning.push_str("\n\n");
                        }
                        reasoning.push_str(t);
                    }
                }
            }
            "function_call" => {
                let id = item
                    .get("call_id")
                    .and_then(|c| c.as_str())
                    .filter(|c| !c.is_empty())
                    .or_else(|| item.get("id").and_then(|c| c.as_str()))
                    .unwrap_or("");
                let name = name_back(back, item.get("name").and_then(|n| n.as_str()).unwrap_or(""));
                let args = item.get("arguments").and_then(|a| a.as_str()).unwrap_or("");
                calls.push(json!({"id": id, "type": "function",
                                  "function": {"name": name, "arguments": args}}));
            }
            _ => {}
        }
    }
    let finish = responses_finish_reason(resp, !calls.is_empty());
    let mut message = json!({"role": "assistant", "content": if text.is_empty() && !calls.is_empty() { Value::Null } else { json!(text) }});
    if !reasoning.is_empty() {
        message["reasoning_content"] = json!(reasoning);
    }
    if !calls.is_empty() {
        message["tool_calls"] = Value::Array(calls);
    }
    let mut out = json!({
        "id": format!("chatcmpl-{}", resp.get("id").and_then(|i| i.as_str()).unwrap_or("codex")),
        "object": "chat.completion",
        "created": resp.get("created_at").and_then(|c| c.as_u64()).unwrap_or(0),
        "model": model,
        "choices": [{"index": 0, "message": message, "finish_reason": finish}],
    });
    if let Some(u) = resp.get("usage").filter(|u| u.is_object()) {
        out["usage"] = responses_usage_to_chat(u);
    }
    out
}

// =====================================================================
// 测试（验收口径见模块注释：文本轮 / 带图轮 / 工具轮 / system / peel / SSE）
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn v(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn text_round_trip_chat_anthropic() {
        let chat = v(
            r#"{"model":"prov/m","max_tokens":123,"temperature":0.3,"top_p":0.9,
                "stop":["X"],
                "messages":[
                    {"role":"system","content":"be nice"},
                    {"role":"user","content":"hi"},
                    {"role":"assistant","content":"hello"}
                ]}"#,
        );
        let anth = chat_to_anthropic(&chat).unwrap();
        // system 提取为顶层 system（单 text 块）
        assert_eq!(anth["system"][0]["type"], "text");
        assert_eq!(anth["system"][0]["text"], "be nice");
        assert_eq!(anth["max_tokens"], json!(123));
        assert_eq!(anth["temperature"], json!(0.3));
        assert_eq!(anth["top_p"], json!(0.9));
        assert_eq!(anth["stop_sequences"], json!(["X"]));
        assert_eq!(anth["messages"][0]["role"], "user");
        assert_eq!(anth["messages"][0]["content"][0]["text"], "hi");
        assert_eq!(anth["messages"][1]["role"], "assistant");
        assert_eq!(anth["messages"][1]["content"][0]["text"], "hello");

        let back = anthropic_to_chat(&anth).unwrap();
        assert_eq!(back["model"], json!("prov/m"));
        assert_eq!(back["messages"][0], json!({"role": "system", "content": "be nice"}));
        assert_eq!(back["messages"][1], json!({"role": "user", "content": "hi"}));
        assert_eq!(back["messages"][2], json!({"role": "assistant", "content": "hello"}));
        assert_eq!(back["max_tokens"], json!(123));
        assert_eq!(back["temperature"], json!(0.3));
        assert_eq!(back["stop"], json!(["X"]));
    }

    #[test]
    fn anthropic_max_tokens_floor() {
        // chat 不带 max_tokens：anthropic 侧必填，兜 8192
        let anth = chat_to_anthropic(&v(r#"{"model":"m","messages":[{"role":"user","content":"x"}]}"#)).unwrap();
        assert_eq!(anth["max_tokens"], json!(8192));
        // 带了就原样
        let anth = chat_to_anthropic(&v(r#"{"model":"m","max_tokens":7,"messages":[{"role":"user","content":"x"}]}"#)).unwrap();
        assert_eq!(anth["max_tokens"], json!(7));
    }

    #[test]
    fn image_round_trip_chat_anthropic() {
        let chat = v(
            r#"{"model":"m","max_tokens":64,"messages":[{"role":"user","content":[
                {"type":"text","text":"look"},
                {"type":"image_url","image_url":{"url":"data:image/png;base64,aGk="}},
                {"type":"image_url","image_url":{"url":"https://example.com/i.png"}}
            ]}]}"#,
        );
        let anth = chat_to_anthropic(&chat).unwrap();
        let content = &anth["messages"][0]["content"];
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["type"], "base64");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
        assert_eq!(content[1]["source"]["data"], "aGk=");
        assert_eq!(content[2]["source"]["type"], "url");
        assert_eq!(content[2]["source"]["url"], "https://example.com/i.png");

        let back = anthropic_to_chat(&anth).unwrap();
        let c = &back["messages"][0]["content"];
        assert_eq!(c[0]["type"], "text");
        assert_eq!(c[1]["image_url"]["url"], "data:image/png;base64,aGk=");
        assert_eq!(c[2]["image_url"]["url"], "https://example.com/i.png");
    }

    #[test]
    fn tools_and_calls_round_trip_chat_anthropic() {
        let chat = v(
            r#"{"model":"prov/m","max_tokens":512,"temperature":0.5,
                "messages":[
                    {"role":"system","content":"sys"},
                    {"role":"user","content":"weather?"},
                    {"role":"assistant","content":null,"tool_calls":[
                        {"id":"call_1","type":"function",
                         "function":{"name":"get_weather","arguments":"{\"city\":\"Berlin\"}"}}
                    ]},
                    {"role":"tool","tool_call_id":"call_1","content":"sunny"},
                    {"role":"user","content":"thanks"}
                ],
                "tools":[{"type":"function","function":{
                    "name":"get_weather","description":"weather",
                    "parameters":{"type":"object","properties":{"city":{"type":"string"}}}}}],
                "tool_choice":"auto"}"#,
        );
        let anth = chat_to_anthropic(&chat).unwrap();
        // tools 定义映射
        assert_eq!(anth["tools"][0]["name"], "get_weather");
        assert_eq!(anth["tools"][0]["input_schema"]["properties"]["city"]["type"], "string");
        assert_eq!(anth["tool_choice"], json!({"type": "auto"}));
        let msgs = anth["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1]["role"], "assistant");
        assert_eq!(msgs[1]["content"][0]["type"], "tool_use");
        assert_eq!(msgs[1]["content"][0]["id"], "call_1");
        assert_eq!(msgs[1]["content"][0]["input"], json!({"city": "Berlin"}));
        // tool 消息与后续 user 正文并成一条 user（tool_result 在前）
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[2]["content"][0]["tool_use_id"], "call_1");
        assert_eq!(msgs[2]["content"][0]["content"], "sunny");
        assert_eq!(msgs[2]["content"][1]["text"], "thanks");

        let back = anthropic_to_chat(&anth).unwrap();
        assert_eq!(back["tools"][0]["function"]["name"], "get_weather");
        assert_eq!(back["tool_choice"], "auto");
        let bm = back["messages"].as_array().unwrap();
        assert_eq!(bm[0], json!({"role": "system", "content": "sys"}));
        assert_eq!(bm[1], json!({"role": "user", "content": "weather?"}));
        assert_eq!(bm[2]["role"], "assistant");
        assert_eq!(bm[2]["tool_calls"][0]["id"], "call_1");
        assert_eq!(bm[2]["tool_calls"][0]["function"]["name"], "get_weather");
        assert_eq!(
            serde_json::from_str::<Value>(bm[2]["tool_calls"][0]["function"]["arguments"].as_str().unwrap()).unwrap(),
            json!({"city": "Berlin"})
        );
        assert_eq!(bm[3], json!({"role": "tool", "tool_call_id": "call_1", "content": "sunny"}));
        assert_eq!(bm[4], json!({"role": "user", "content": "thanks"}));
    }

    #[test]
    fn anthropic_required_tool_choice_becomes_any_and_back() {
        let anth = v(
            r#"{"model":"m","max_tokens":10,"messages":[{"role":"user","content":[
                {"type":"text","text":"go"}]}],
                "tools":[{"name":"t","description":"d","input_schema":{"type":"object"}}],
                "tool_choice":{"type":"any"}}"#,
        );
        let chat = anthropic_to_chat(&anth).unwrap();
        assert_eq!(chat["tool_choice"], "required");
        let back = chat_to_anthropic(&chat).unwrap();
        assert_eq!(back["tool_choice"], json!({"type": "any"}));
    }

    #[test]
    fn text_round_trip_chat_responses() {
        let chat = v(
            r#"{"model":"m","max_tokens":99,
                "messages":[
                    {"role":"system","content":"sys"},
                    {"role":"user","content":"hi"},
                    {"role":"assistant","content":"hello"}
                ]}"#,
        );
        let resp = chat_to_responses(&chat).unwrap();
        assert_eq!(resp["instructions"], "sys");
        assert_eq!(resp["max_output_tokens"], json!(99));
        assert_eq!(resp["store"], false);
        let items = resp["input"].as_array().unwrap();
        assert_eq!(items[0]["type"], "message");
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[0]["content"][0]["type"], "input_text");
        assert_eq!(items[0]["content"][0]["text"], "hi");
        assert_eq!(items[1]["role"], "assistant");
        assert_eq!(items[1]["content"][0]["type"], "output_text");

        let back = responses_to_chat(&resp).unwrap();
        assert_eq!(back["messages"][0], json!({"role": "system", "content": "sys"}));
        assert_eq!(back["messages"][1], json!({"role": "user", "content": "hi"}));
        assert_eq!(back["messages"][2], json!({"role": "assistant", "content": "hello"}));
    }

    #[test]
    fn function_call_round_trip_chat_responses() {
        let chat = v(
            r#"{"model":"m","max_tokens":50,
                "messages":[
                    {"role":"user","content":"run it"},
                    {"role":"assistant","content":null,"tool_calls":[
                        {"id":"call_9","type":"function",
                         "function":{"name":"exec","arguments":"{\"cmd\":\"dir\"}"}}]},
                    {"role":"tool","tool_call_id":"call_9","content":"done"}
                ],
                "tools":[{"type":"function","function":{
                    "name":"exec","description":"e","parameters":{"type":"object"}}}],
                "tool_choice":{"type":"function","function":{"name":"exec"}}}"#,
        );
        let resp = chat_to_responses(&chat).unwrap();
        let items = resp["input"].as_array().unwrap();
        assert_eq!(items[0]["type"], "message");
        assert_eq!(items[1]["type"], "function_call");
        assert_eq!(items[1]["call_id"], "call_9");
        assert_eq!(items[1]["name"], "exec");
        assert_eq!(items[1]["arguments"], "{\"cmd\":\"dir\"}");
        assert_eq!(items[2]["type"], "function_call_output");
        assert_eq!(items[2]["call_id"], "call_9");
        assert_eq!(items[2]["output"], "done");
        // tools + 具名 tool_choice
        assert_eq!(resp["tools"][0]["type"], "function");
        assert_eq!(resp["tools"][0]["name"], "exec");
        assert_eq!(resp["tool_choice"], json!({"type": "function", "name": "exec"}));

        let back = responses_to_chat(&resp).unwrap();
        let bm = back["messages"].as_array().unwrap();
        assert_eq!(bm[0], json!({"role": "user", "content": "run it"}));
        assert_eq!(bm[1]["role"], "assistant");
        assert_eq!(bm[1]["tool_calls"][0]["function"]["name"], "exec");
        assert_eq!(bm[1]["tool_calls"][0]["function"]["arguments"], "{\"cmd\":\"dir\"}");
        assert_eq!(bm[2], json!({"role": "tool", "tool_call_id": "call_9", "content": "done"}));
        assert_eq!(back["tools"][0]["function"]["name"], "exec");
    }

    #[test]
    fn responses_input_string_and_image() {
        let resp = v(
            r#"{"model":"m","instructions":"sys","input":[
                {"type":"message","role":"user","content":[
                    {"type":"input_text","text":"see"},
                    {"type":"input_image","image_url":"data:image/jpeg;base64,eA=="}
                ]}
            ]}"#,
        );
        let chat = responses_to_chat(&resp).unwrap();
        assert_eq!(chat["messages"][0], json!({"role": "system", "content": "sys"}));
        let c = &chat["messages"][1]["content"];
        assert_eq!(c[0]["type"], "text");
        assert_eq!(c[1]["image_url"]["url"], "data:image/jpeg;base64,eA==");
        // 纯字符串 input
        let resp = v(r#"{"model":"m","input":"plain"}"#);
        let chat = responses_to_chat(&resp).unwrap();
        assert_eq!(chat["messages"][0], json!({"role": "user", "content": "plain"}));
    }

    #[test]
    fn bad_bodies_error() {
        assert!(chat_to_anthropic(&json!("nope")).is_err());
        assert!(chat_to_anthropic(&json!({"model": "m"})).is_err()); // 缺 messages
        assert!(anthropic_to_chat(&json!({"model": "m"})).is_err()); // 缺 messages
        assert!(responses_to_chat(&json!({"model": "m"})).is_err()); // 缺 input
        assert!(chat_to_responses(&json!([])).is_err());
    }

    #[test]
    fn peel_marks() {
        assert_eq!(peel_agent_model_mark("gpt-5.6-luna[1m]"), "gpt-5.6-luna");
        assert_eq!(peel_agent_model_mark("gpt-5.6-luna[xhigh]"), "gpt-5.6-luna");
        // effort 大小写不敏感
        assert_eq!(peel_agent_model_mark("gpt-5.6-luna[XHigh]"), "gpt-5.6-luna");
        assert_eq!(peel_agent_model_mark("gpt-5.6-luna"), "gpt-5.6-luna");
        // 首尾空白先裁（Go TrimSpace 同款）
        assert_eq!(peel_agent_model_mark("  gpt-5.6-luna[max] "), "gpt-5.6-luna");
        // MemberEfforts 全表（Go 原表含 none/minimal）
        assert_eq!(peel_agent_model_mark("m[minimal]"), "m");
        assert_eq!(peel_agent_model_mark("m[none]"), "m");
        assert_eq!(peel_agent_model_mark("m[LOW]"), "m");
        // 非 effort 的方括号后缀原样保留
        assert_eq!(peel_agent_model_mark("gpt[foo]"), "gpt[foo]");
        // 整串就是一个 [..]：不是后缀，不剥（Go i>0 同款）
        assert_eq!(peel_agent_model_mark("[xhigh]"), "[xhigh]");
    }

    #[test]
    fn sse_chat_to_anthropic_text_delta() {
        let out = sse_chat_to_anthropic(
            r#"{"id":"x","object":"chat.completion.chunk",
                "choices":[{"index":0,"delta":{"content":"你好"},"finish_reason":null}]}"#,
        )
        .unwrap();
        assert!(out.starts_with("event: content_block_delta\ndata: "));
        let data = out.splitn(2, '\n').nth(1).unwrap().strip_prefix("data: ").unwrap();
        let ev: Value = serde_json::from_str(data).unwrap();
        assert_eq!(ev["type"], "content_block_delta");
        assert_eq!(ev["index"], json!(0));
        assert_eq!(ev["delta"]["type"], "text_delta");
        assert_eq!(ev["delta"]["text"], "你好");
    }

    #[test]
    fn sse_chat_to_anthropic_finish_and_done() {
        let out = sse_chat_to_anthropic(
            r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
        )
        .unwrap();
        assert!(out.contains("event: message_delta"));
        assert!(out.contains("\"stop_reason\":\"end_turn\""));
        assert!(out.contains("\"input_tokens\":10"));
        assert!(out.contains("event: message_stop"));
        // [DONE] → message_stop
        let out = sse_chat_to_anthropic("[DONE]").unwrap();
        assert!(out.starts_with("event: message_stop"));
        // 纯 usage 帧（include_usage 的空 choices 末帧）
        let out = sse_chat_to_anthropic(r#"{"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":1}}"#).unwrap();
        assert!(out.starts_with("event: message_delta"));
        // 无对应事件
        assert!(sse_chat_to_anthropic(r#"{"choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}"#).is_none());
    }

    #[test]
    fn sse_anthropic_to_chat_text_and_stop() {
        let out = sse_anthropic_to_chat(
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hey"}}"#,
        )
        .unwrap();
        assert!(out.starts_with("data: "));
        let chunk: Value = serde_json::from_str(&out["data: ".len()..]).unwrap();
        assert_eq!(chunk["object"], "chat.completion.chunk");
        assert_eq!(chunk["choices"][0]["delta"]["content"], "hey");
        assert!(chunk["choices"][0]["finish_reason"].is_null());

        // stop + usage：finish 帧与 usage 帧分开（Go chatEncoder 形态）
        let out = sse_anthropic_to_chat(
            r#"{"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},
                "usage":{"input_tokens":3,"output_tokens":4,"cache_read_input_tokens":2}}"#,
        )
        .unwrap();
        assert!(out.contains("\"finish_reason\":\"tool_calls\""));
        // OpenAI 口径：prompt_tokens 含缓存读（3 + 2）
        assert!(out.contains("\"prompt_tokens\":5"), "{out}");
        assert!(out.contains("\"cached_tokens\":2"));

        assert_eq!(sse_anthropic_to_chat(r#"{"type":"message_stop"}"#).unwrap(), "data: [DONE]");
        // 非文本 delta（input_json_delta）没有对应
        assert!(sse_anthropic_to_chat(
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"x"}}"#
        )
        .is_none());
    }

    // -----------------------------------------------------------------
    // tool_calls 分片拼装（ChatToolAccum）
    // -----------------------------------------------------------------

    fn frag(index: u64, id: &str, name: &str, args: &str) -> Value {
        json!({"index": index, "id": id, "type": "function",
            "function": {"name": name, "arguments": args}})
    }

    #[test]
    fn tool_accum_three_fragments_one_call() {
        // 规范形态：首片带 id+name，arguments 跨三帧拼接；后续片可省 id。
        let mut acc = ChatToolAccum::new();
        acc.feed(&[frag(0, "call_1", "get_weather", r#"{"ci"#)]);
        acc.feed(&[frag(0, "", "", r#"ty":"Berlin""#)]);
        acc.feed(&[frag(0, "", "", r#"}"#)]);
        let calls = acc.take();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "call_1");
        assert_eq!(calls[0].name, "get_weather");
        assert_eq!(calls[0].arguments, r#"{"city":"Berlin"}"#);
        assert!(acc.is_empty());
        // take 清空：再 take 不重复
        assert!(acc.take().is_empty());
    }

    #[test]
    fn tool_accum_repeated_id_and_index_fallback() {
        // 中转厂每片都重复 id（Go 注释实锤的形态）＋ 分片没带 index 时
        // 按数组位次兜底。
        let mut acc = ChatToolAccum::new();
        acc.feed(&[json!({"id":"call_9","type":"function",
            "function":{"name":"exec","arguments":"{\"a\""}})]);
        acc.feed(&[json!({"id":"call_9","type":"function",
            "function":{"arguments":":1}"}})]);
        let calls = acc.take();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].arguments, r#"{"a":1}"#);
        assert_eq!(calls[0].name, "exec");
    }

    #[test]
    fn tool_accum_two_sequential_calls() {
        // 两次调用顺序到（index 0 → 1）：各自拼装、首片序输出。
        let mut acc = ChatToolAccum::new();
        acc.feed(&[frag(0, "call_a", "read", "{}")]);
        acc.feed(&[frag(1, "call_b", "write", r#"{"p":"x"#)]);
        acc.feed(&[frag(1, "", "", r#""}"#)]);
        let calls = acc.take();
        assert_eq!(calls.len(), 2);
        assert_eq!((calls[0].id.as_str(), calls[0].name.as_str()), ("call_a", "read"));
        assert_eq!((calls[1].id.as_str(), calls[1].name.as_str()), ("call_b", "write"));
        assert_eq!(calls[1].arguments, r#"{"p":"x"}"#);
    }

    #[test]
    fn tool_accum_new_id_same_index_starts_new_call() {
        // 同 index 换 id：新的一次调用（Go newID 同款判定）。
        let mut acc = ChatToolAccum::new();
        acc.feed(&[frag(0, "call_1", "a", "1")]);
        acc.feed(&[frag(0, "call_2", "b", "2")]);
        let calls = acc.take();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments, "1");
        assert_eq!(calls[1].arguments, "2");
    }

    #[test]
    fn tool_accum_interleaved_parallel_assembles_by_index() {
        // 乱序 parallel 分片：按 index 拼装正确（0/1 交替来各归各）；
        // 输出按首片到达序——事件序不再反映厂商的交错讲述（已知边界）。
        let mut acc = ChatToolAccum::new();
        acc.feed(&[frag(0, "call_a", "read", "A1")]);
        acc.feed(&[frag(1, "call_b", "write", "B1")]);
        acc.feed(&[frag(0, "", "", "A2")]);
        acc.feed(&[frag(1, "", "", "B2")]);
        let calls = acc.take();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].arguments, "A1A2");
        assert_eq!(calls[1].arguments, "B1B2");
    }

    #[test]
    fn tool_accum_single_frame_call() {
        // 单帧完整调用（无分片）：一次进一次出。
        let mut acc = ChatToolAccum::new();
        acc.feed(&[frag(0, "call_x", "ping", r#"{"ok":true}"#)]);
        assert_eq!(
            acc.take(),
            vec![ChatToolCall {
                id: "call_x".into(),
                name: "ping".into(),
                arguments: r#"{"ok":true}"#.into(),
            }]
        );
    }

    // ---- Codex 的非 function 工具（custom / namespace / tool_search）----

    /// Codex 真实形态的 tools：普通 function、custom（apply_patch，带 grammar）、
    /// namespace（MCP）、client 的 tool_search，外加服务端的 web_search /
    /// image_generation / local_shell（应被有意丢弃）。
    fn codex_tools() -> Value {
        json!([
            {"type": "function", "name": "shell", "description": "run", "parameters": {"type": "object", "properties": {"cmd": {"type": "string"}}}},
            {"type": "custom", "name": "apply_patch", "description": "edit files",
             "format": {"type": "grammar", "syntax": "lark", "definition": "start: /.+/"}},
            {"type": "namespace", "name": "mcp__marionette__", "description": "ns", "tools": [
                {"type": "function", "name": "browser_open", "description": "open", "parameters": {"type": "object", "properties": {"url": {"type": "string"}}}},
                {"type": "custom", "name": "freeform", "description": "ff"}
            ]},
            {"type": "tool_search", "execution": "client", "description": "search tools", "parameters": {"type": "object", "properties": {"query": {"type": "string"}}}},
            {"type": "web_search", "external_web_access": true},
            {"type": "image_generation"},
            {"type": "local_shell"}
        ])
    }

    fn chat_tool_names(chat: &Value) -> Vec<String> {
        chat["tools"].as_array().unwrap().iter()
            .map(|t| t["function"]["name"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn flat_name_rules() {
        assert_eq!(flat_name("mcp__srv__", "do"), "mcp__srv____do");
        // 超长：截 55 + _ + 8 位哈希，恒 <= 64，同输入同输出
        let long_ns = "n".repeat(70);
        let a = flat_name(&long_ns, "x");
        assert!(a.len() <= 64, "{a}");
        assert_eq!(a, flat_name(&long_ns, "x"));
        assert_ne!(a, flat_name(&long_ns, "y"));
        // 非法字符换 _，仍合法；不同原名不撞
        let b = flat_name("my.ns", "do it");
        assert!(b.bytes().all(|c| c == b'_' || c == b'-' || c.is_ascii_alphanumeric()), "{b}");
        assert_ne!(b, flat_name("my_ns", "do_it"));
    }

    #[test]
    fn codex_tool_kinds_flatten_for_chat_vendor() {
        let body = json!({"model": "m", "input": "hi", "tools": codex_tools(), "parallel_tool_calls": true});
        let (chat, table) = responses_to_chat_with_tools(&body).unwrap();
        // web_search / image_generation / local_shell 有意丢弃；其余各一条函数
        assert_eq!(
            chat_tool_names(&chat),
            vec!["shell", "apply_patch", "mcp__marionette____browser_open", "mcp__marionette____freeform", "tool_search"]
        );
        for n in chat_tool_names(&chat) {
            assert!(n.len() <= 64 && n.bytes().all(|c| c == b'_' || c == b'-' || c.is_ascii_alphanumeric()), "{n}");
        }
        // custom：一个字符串 input 的函数 + grammar 写进描述
        let patch = &chat["tools"][1]["function"];
        assert_eq!(patch["parameters"]["required"], json!(["input"]));
        let d = patch["description"].as_str().unwrap();
        assert!(d.starts_with("edit files") && d.contains("lark grammar") && d.contains("start: /.+/"), "{d}");
        // 普通 function 工具原样
        assert_eq!(chat["tools"][0]["function"]["name"], "shell");
        assert_eq!(chat["tools"][0]["function"]["parameters"]["properties"]["cmd"]["type"], "string");
        // 还原表：普通工具不进表
        assert!(!table.contains_key("shell"));
        assert_eq!(table["apply_patch"], NsTool { name: "apply_patch".into(), custom: true, ..Default::default() });
        assert_eq!(
            table["mcp__marionette____browser_open"],
            NsTool { namespace: "mcp__marionette__".into(), name: "browser_open".into(), ..Default::default() }
        );
        assert!(table["mcp__marionette____freeform"].custom);
        assert!(table["tool_search"].search);
        assert_eq!(chat["parallel_tool_calls"], true);
    }

    #[test]
    fn tool_search_is_dropped_unless_client_executed() {
        let body = json!({"model": "m", "input": "hi", "tools": [
            {"type": "tool_search", "execution": "server", "description": "x"},
            {"type": "function", "name": "a", "parameters": {"type": "object"}}]});
        let (chat, table) = responses_to_chat_with_tools(&body).unwrap();
        assert_eq!(chat_tool_names(&chat), vec!["a"]);
        assert!(table.is_empty());
    }

    #[test]
    fn plain_function_tools_unchanged_and_not_tabled() {
        let body = json!({"model": "m", "input": "hi",
            "tools": [{"type": "function", "name": "get_weather", "description": "w",
                       "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}}],
            "tool_choice": "auto"});
        let (chat, table) = responses_to_chat_with_tools(&body).unwrap();
        assert!(table.is_empty());
        assert_eq!(chat["tools"][0], json!({"type": "function", "function": {"name": "get_weather",
            "description": "w", "parameters": {"type": "object", "properties": {"city": {"type": "string"}}}}}));
        assert_eq!(chat["tool_choice"], "auto");
        assert!(chat.get("parallel_tool_calls").is_none());
    }

    #[test]
    fn colliding_flat_names_are_made_unique() {
        // 命名空间 a + 工具 b__c 与 命名空间 a__b + 工具 c 压平后同名
        let body = json!({"model": "m", "input": "hi", "tools": [
            {"type": "namespace", "name": "a", "tools": [{"type": "function", "name": "b__c", "parameters": {"type": "object"}}]},
            {"type": "namespace", "name": "a__b", "tools": [{"type": "function", "name": "c", "parameters": {"type": "object"}}]}]});
        let (chat, table) = responses_to_chat_with_tools(&body).unwrap();
        let names = chat_tool_names(&chat);
        assert_eq!(names.len(), 2);
        assert_ne!(names[0], names[1]);
        assert_eq!(table[&names[0]].namespace, "a");
        assert_eq!(table[&names[1]].namespace, "a__b");
    }

    #[test]
    fn history_items_use_the_same_flat_names() {
        let body = json!({"model": "m", "tools": codex_tools(), "input": [
            {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "go"}]},
            {"type": "function_call", "call_id": "c1", "namespace": "mcp__marionette__", "name": "browser_open", "arguments": "{\"url\":\"http://x\"}"},
            {"type": "function_call_output", "call_id": "c1", "output": "opened"},
            {"type": "custom_tool_call", "call_id": "c2", "name": "apply_patch", "input": "*** Begin Patch"},
            {"type": "custom_tool_call_output", "call_id": "c2", "output": "Done"},
            {"type": "tool_search_call", "call_id": "c3", "execution": "client", "arguments": {"query": "calendar"}},
            {"type": "tool_search_output", "call_id": "c3", "execution": "client", "tools": [
                {"type": "namespace", "name": "mcp__cal__", "tools": [{"type": "function", "name": "list_events", "parameters": {"type": "object"}}]}]}
        ]});
        let (chat, table) = responses_to_chat_with_tools(&body).unwrap();
        let msgs = chat["messages"].as_array().unwrap();
        // user, assistant(3 个调用并成一条 tool_calls 的前两条之间有 tool 消息 -> 分开)
        let calls: Vec<&Value> = msgs.iter().filter_map(|m| m.get("tool_calls")).flat_map(|c| c.as_array().unwrap()).collect();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0]["function"]["name"], "mcp__marionette____browser_open");
        assert_eq!(calls[0]["function"]["arguments"], "{\"url\":\"http://x\"}");
        assert_eq!(calls[1]["function"]["name"], "apply_patch");
        assert_eq!(calls[1]["function"]["arguments"], "{\"input\":\"*** Begin Patch\"}");
        assert_eq!(calls[2]["function"]["name"], "tool_search");
        assert_eq!(calls[2]["function"]["arguments"], "{\"query\":\"calendar\"}");
        // 三个结果都按 call id 配成 tool 消息；tool_search 的结果讲「现在可调的名字」
        let tool_msgs: Vec<&Value> = msgs.iter().filter(|m| m["role"] == "tool").collect();
        assert_eq!(tool_msgs.len(), 3);
        assert_eq!(tool_msgs[2]["tool_call_id"], "c3");
        assert_eq!(tool_msgs[2]["content"], "These tools are now available to call: mcp__cal____list_events");
        // 搜到的工具排在请求自带工具之后被提供，并进还原表
        let names = chat_tool_names(&chat);
        assert_eq!(names.last().unwrap(), "mcp__cal____list_events");
        assert_eq!(table["mcp__cal____list_events"].name, "list_events");
    }

    #[test]
    fn orphan_output_becomes_user_text_deferred_past_the_run() {
        let body = json!({"model": "m", "input": [
            {"type": "message", "role": "user", "content": "go"},
            {"type": "function_call", "call_id": "a", "name": "f", "arguments": "{}"},
            {"type": "function_call_output", "output": "notification from another thread"},
            {"type": "function_call_output", "call_id": "a", "output": "ok"},
            {"type": "function_call_output", "output": ""}
        ]});
        let chat = responses_to_chat(&body).unwrap();
        let msgs = chat["messages"].as_array().unwrap();
        let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
        // user, assistant(tool_calls), tool(a) 紧跟其后，孤儿文字在串之后
        assert_eq!(roles[..3], ["user", "assistant", "tool"]);
        assert_eq!(msgs[2]["tool_call_id"], "a");
        let tail: String = msgs[3..].iter().map(|m| m["content"].to_string()).collect();
        assert!(tail.contains("notification from another thread"), "{tail}");
        assert!(tail.contains("Tool result received."), "{tail}");
        assert!(msgs.iter().all(|m| m["role"] != "tool" || m["tool_call_id"] == "a"), "孤儿不得带假 call_id");
    }

    #[test]
    fn unanswered_calls_get_a_synthetic_result_at_the_end_of_the_run() {
        let body = json!({"model": "m", "input": [
            {"type": "message", "role": "user", "content": "go"},
            {"type": "function_call", "call_id": "a", "name": "f", "arguments": "{}"},
            {"type": "custom_tool_call", "call_id": "b", "name": "g", "input": "x"},
            {"type": "message", "role": "user", "content": "continue"}
        ]});
        let chat = responses_to_chat(&body).unwrap();
        let msgs = chat["messages"].as_array().unwrap();
        let roles: Vec<&str> = msgs.iter().map(|m| m["role"].as_str().unwrap()).collect();
        assert_eq!(roles, ["user", "assistant", "tool", "tool", "user"]);
        assert_eq!(msgs[2]["tool_call_id"], "a");
        assert_eq!(msgs[3]["tool_call_id"], "b");
        assert!(msgs[2]["content"].as_str().unwrap().contains("unavailable"));
    }

    #[test]
    fn tool_choice_forms() {
        let tools = codex_tools();
        // 点名命名空间里的函数 -> flat 名
        let c = responses_to_chat(&json!({"model": "m", "input": "x", "tools": tools,
            "tool_choice": {"type": "function", "namespace": "mcp__marionette__", "name": "browser_open"}})).unwrap();
        assert_eq!(c["tool_choice"]["function"]["name"], "mcp__marionette____browser_open");
        // allowed_tools：mode 成为 tool_choice，清单收窄
        let c = responses_to_chat(&json!({"model": "m", "input": "x", "tools": tools,
            "tool_choice": {"type": "allowed_tools", "mode": "required", "tools": [
                {"type": "function", "name": "shell"}, {"type": "custom", "name": "apply_patch"}]}})).unwrap();
        assert_eq!(c["tool_choice"], "required");
        assert_eq!(chat_tool_names(&c), vec!["shell", "apply_patch"]);
        // 非法 mode 报错
        assert!(responses_to_chat(&json!({"model": "m", "input": "x", "tools": tools,
            "tool_choice": {"type": "allowed_tools", "mode": "bogus", "tools": []}})).is_err());
    }

    #[test]
    fn lite_additional_tools_are_lifted_and_functions_namespace_stripped() {
        let body = json!({"model": "m", "input": [
            {"type": "additional_tools", "tools": [
                {"type": "namespace", "name": "functions", "tools": [
                    {"type": "function", "name": "exec_command", "parameters": {"type": "object"}}]}]},
            {"type": "message", "role": "user", "content": "go"},
            {"type": "function_call", "call_id": "a", "namespace": "functions", "name": "exec_command", "arguments": "{}"},
            {"type": "function_call_output", "call_id": "a", "output": "ok"}
        ]});
        let (chat, table) = responses_to_chat_with_tools(&body).unwrap();
        assert_eq!(chat_tool_names(&chat), vec!["exec_command"]);
        assert!(table.is_empty());
        assert_eq!(chat["messages"][1]["tool_calls"][0]["function"]["name"], "exec_command");
    }

    #[test]
    fn call_to_round_trips_each_kind() {
        let body = json!({"model": "m", "input": "hi", "tools": codex_tools()});
        let (_, table) = responses_to_chat_with_tools(&body).unwrap();
        let mk = |args: &str| json!({"type": "function_call", "status": "completed", "call_id": "c", "arguments": args});
        // custom
        let mut it = mk("{\"input\":\"*** patch\\n\"}");
        call_to(&mut it, "apply_patch", &table);
        assert_eq!(it["type"], "custom_tool_call");
        assert_eq!(it["name"], "apply_patch");
        assert_eq!(it["input"], "*** patch\n");
        assert!(it.get("arguments").is_none() && it.get("namespace").is_none());
        assert_eq!(call_item_prefix(&it), "ctc_");
        // custom：arguments 不是 {input} 对象时取原文（Go customInput）
        let mut it = mk("raw text");
        call_to(&mut it, "apply_patch", &table);
        assert_eq!(it["input"], "raw text");
        // 命名空间 custom
        let mut it = mk("{\"input\":\"z\"}");
        call_to(&mut it, "mcp__marionette____freeform", &table);
        assert_eq!((it["type"].as_str().unwrap(), it["namespace"].as_str().unwrap(), it["name"].as_str().unwrap()),
            ("custom_tool_call", "mcp__marionette__", "freeform"));
        // 命名空间 function：原名 + namespace
        let mut it = mk("{\"url\":\"u\"}");
        call_to(&mut it, "mcp__marionette____browser_open", &table);
        assert_eq!(it["type"], "function_call");
        assert_eq!(it["name"], "browser_open");
        assert_eq!(it["namespace"], "mcp__marionette__");
        assert_eq!(it["arguments"], "{\"url\":\"u\"}");
        assert_eq!(call_item_prefix(&it), "fc_");
        // tool_search：client 执行，arguments 是对象，没有 name
        let mut it = mk("{\"query\":\"q\"}");
        call_to(&mut it, "tool_search", &table);
        assert_eq!(it["type"], "tool_search_call");
        assert_eq!(it["execution"], "client");
        assert_eq!(it["arguments"], json!({"query": "q"}));
        assert!(it.get("name").is_none());
        assert_eq!(call_item_prefix(&it), "tsc_");
        // 普通工具：只补原名
        let mut it = mk("{}");
        call_to(&mut it, "shell", &table);
        assert_eq!((it["type"].as_str().unwrap(), it["name"].as_str().unwrap()), ("function_call", "shell"));
        assert!(it.get("namespace").is_none());
    }
}
