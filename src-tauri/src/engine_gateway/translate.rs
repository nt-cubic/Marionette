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
                        // thinking / redacted_thinking：签名的思考只回 anthropic
                        // 自己（跨协议无对应），本矩阵丢弃
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
        ..Default::default()
    };
    match body.get("input") {
        Some(Value::String(s)) => {
            ir.messages.push(Message { role: "user".into(), parts: vec![Part::text(s.clone())] });
        }
        Some(Value::Array(items)) => {
            let mut sys: Vec<String> = vec![ir.system.clone()];
            for it in items {
                let ty = get_str(it, "type").unwrap_or_default();
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
                        continue;
                    }
                    let role = if role == "assistant" { "assistant" } else { "user" };
                    ir.messages.push(Message { role: role.into(), parts });
                    continue;
                }
                if ty == "function_call" || ty == "custom_tool_call" {
                    let name = get_str(it, "name").unwrap_or_default();
                    let args = if ty == "custom_tool_call" {
                        // 自由文本工具：包成 {"input": …}（Go 同款）
                        json!({"input": get_str(it, "input").unwrap_or_default()})
                    } else {
                        match it.get("arguments") {
                            Some(Value::String(s)) => parse_args(s),
                            Some(v) if v.is_object() => v.clone(),
                            _ => json!({}),
                        }
                    };
                    ir.messages.push(Message {
                        role: "assistant".into(),
                        parts: vec![Part::tool_call(get_str(it, "call_id").unwrap_or_default(), name, args)],
                    });
                    continue;
                }
                if ty == "function_call_output" || ty == "custom_tool_call_output" {
                    let (text, images) = output_parts(it.get("output").unwrap_or(&Value::Null));
                    ir.messages.push(Message {
                        role: "user".into(),
                        parts: vec![Part::tool_result(get_str(it, "call_id").unwrap_or_default(), text, images)],
                    });
                    continue;
                }
                // reasoning（加密思考只回 responses 自己）、web_search_call
                // （服务端搜索回放）等其余 item：不跨协议，丢弃
            }
            ir.system = sys.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("\n\n");
        }
        _ => return Err("invalid request: input".into()),
    }
    ir.messages = merge_turns(std::mem::take(&mut ir.messages));
    if let Some(tools) = body.get("tools").and_then(|t| t.as_array()) {
        for t in tools {
            if get_str(t, "type").as_deref() != Some("function") {
                continue;
            }
            let name = get_str(t, "name").unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            ir.tools.push(ToolDef {
                name,
                description: get_str(t, "description").unwrap_or_default(),
                schema: t.get("parameters").cloned().unwrap_or(Value::Null),
            });
        }
    }
    match body.get("tool_choice") {
        Some(Value::String(s)) => ir.tool_choice = s.clone(),
        Some(choice) => {
            if let Some(name) = get_str(choice, "name") {
                if !name.is_empty() {
                    ir.tool_choice = format!("name:{name}");
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
            let mut am = json!({"role": "assistant"});
            // 有调用且无正文时省掉 content（OpenAI 形态惯用；Go 同款）
            if !text.is_empty() || calls.is_empty() {
                am["content"] = json!(text);
            }
            if !calls.is_empty() {
                am["tool_calls"] = Value::Array(calls);
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
    parse_responses(body).map(|ir| build_chat(&ir))
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

/// chat usage → anthropic usage（只搬本矩阵保真的桶：prompt/completion/
/// cached；四桶拆分的完整口径在 usage 台账侧，别在这里编数）。
fn anthropic_usage(u: Option<&Value>) -> Value {
    match u {
        Some(u) => json!({
            "input_tokens": u.get("prompt_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            "output_tokens": u.get("completion_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
            "cache_read_input_tokens":
                u.pointer("/prompt_tokens_details/cached_tokens").and_then(|v| v.as_u64()).unwrap_or(0),
        }),
        None => json!({"input_tokens": 0, "output_tokens": 0}),
    }
}

/// anthropic usage → chat usage。
fn chat_usage(u: &Value) -> Value {
    let input = u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    let output = u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
    json!({
        "prompt_tokens": input,
        "completion_tokens": output,
        "total_tokens": input + output,
        "prompt_tokens_details": {
            "cached_tokens": u.get("cache_read_input_tokens").and_then(|v| v.as_u64()).unwrap_or(0)
        },
    })
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
    pub fn take(&mut self) -> Vec<ChatToolCall> {
        self.calls.drain(..).map(|(_, c)| c).collect()
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
        assert!(out.contains("\"prompt_tokens\":3"));
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
}
