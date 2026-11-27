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
//! SSE 编排在端点的帧编排器（`AnthropicFrames`，translate.rs 模块注释钉的
//! 分工）：`message_start` 必须是第一帧——anthropic SDK 的 MessageStream 对
//! 它之前的事件直接抛错（Go anthropicEncoder.keepalive 注释实锤）；
//! `content_block` 的 index 按块开闭递增（同轮 text 块与 tool_use 块共享
//! 一条 index 序列，anthropic 规矩）；无状态映射会把 finish 帧与
//! `[DONE]` 各收口一次 `message_stop`，编排器在**首个** message_stop 后丢弃
//! 全部尾巴（include_usage 末帧、`[DONE]`），一条流只收口一次。chat 流的
//! `tool_calls` 分片经 `translate::ChatToolAccum` 拼装，每次调用出
//! `content_block_start`（tool_use）→ `content_block_delta`
//! （input_json_delta，拼装结果整段一帧）→ `content_block_stop` 三段式。
//!
//! 错误面：厂商错误一律转 anthropic 形状
//! `{"type":"error","error":{"type":"...","message":"..."}}`，状态码透传；
//! 直通路径上本来就是 anthropic 形状的厂商错误原样透传（厂商的话不改写）。
//!
//! textcalls（正文里书写的工具调用，`super::textcalls`，Go textcalls.go /
//! anthropicTextCalls 的翻译路径形态）：**翻译路径的应答文本**过
//! TextCallScout——请求提供了工具时，从第一个伪调用标记起的正文被持有，
//! 回合结束（finish_reason）可解析块变成真 tool_use 块（三段式复用
//! ChatToolAccum 的拼装与冲刷），截断流只按原文放出。offered 名单取请求
//! 的 anthropic 工具名（translate 不改名）。anthropic 直通不装（Go 的
//! anthropicTextCalls 是直通流的独立变体，差距保留）。

use std::io::{BufRead, BufReader, Read};

use serde_json::{json, Value};

use super::bodycap;
use super::chat;
use super::groups::{self, Attempt, Candidate};
use super::httpd::Reply;
use super::translate;

/// 直通路径的 anthropic 协议版本头（任务定案值；anthropic 官方与主流
/// 兼容面都认这一个）。
const ANTHROPIC_VERSION: &str = "2023-06-01";

// =====================================================================
// 路由：asked model → 出站目标
// =====================================================================

/// 出站目标（chat.rs outbound_for 的 anthropic 版）：直通基址与翻译基址分开
/// 存，由 attempt 按优先级挑。anthropic/chat 字段是 providers.json 的原语
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

/// 候选成员 → 出站目标（锁内只查表克隆）。
fn target_for(gw: &super::Gateway, cand: &Candidate) -> Result<Target, String> {
    let store = gw.store.lock().map_err(|_| "store poisoned".to_string())?;
    let provider = store
        .find(&cand.provider)
        .ok_or_else(|| format!("no provider for model: {}/{}", cand.provider, cand.model))?;
    Ok(Target {
        provider_id: provider.id.clone(),
        key: provider.key.clone(),
        headers: provider
            .headers
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        bare_model: cand.model.clone(),
        anthropic: provider.anthropic.trim_end_matches('/').to_string(),
        chat: provider.chat.trim_end_matches('/').to_string(),
    })
}

// =====================================================================
// 入口
// =====================================================================

/// POST /v1/messages 入口：组路由有序候选，逐成员尝试（只在应答头阶段换）。
///
/// usage 记账（capture 层 2026-11-18 接上）：2xx 回合在收口处落一行
/// （agent="anthropic"）——message_start 的 input/缓存两桶 + message_delta
/// 的 output 桶（Go sniff.go:243-269 aUsage 口径；input_tokens 本就只数
/// 非缓存）。会话种子取 body 侧（路由层不传请求头，`stick_key(&[], …)`
/// ——Claude Code 的 metadata.user_id 在这条上能拿到）。
pub fn handle(gateway: &super::Gateway, body: &[u8]) -> Reply {
    let began = std::time::Instant::now();
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return error_reply(400, &format!("invalid request body: {e}")),
    };
    let model = parsed
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let cands = groups::plan_for(gateway, &model);
    // 与 chat.rs 同口径：解析不到来源就是 404（模型存在性不查，那是
    // 厂商应答该说的话）。
    if cands.is_empty() {
        return error_reply(404, &format!("no provider for model: {model}"));
    }
    let streaming = parsed
        .get("stream")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);
    let log = super::usage_ledger::TurnLog::new(
        "anthropic",
        &model,
        super::affinity::stick_key(&[], &parsed),
        began,
    );
    groups::drive(&cands, |cand| {
        // 成员强度按候选各算各的（失败转移到下一个成员时不沿用首发的）。
        let body = groups::body_for(gateway, super::effort::Proto::Anthropic, cand, body);
        let parsed = serde_json::from_slice::<Value>(&body).unwrap_or_else(|_| parsed.clone());
        attempt(gateway, cand, &body, &parsed, &model, streaming, &log)
    })
}

// =====================================================================
// POST /v1/messages/count_tokens（Go gateway.go countTokens）
// =====================================================================

/// 首选成员有 anthropic 基址 → 转厂商的 count_tokens；厂商不支持计数
/// （404/405/501 或 400 点名 count_tokens 不支持）、没有 anthropic 基址、
/// 解析不到来源 → 本地粗估（字节数 / 4，Go `estimate`）。**不数后备成员**
/// （它可能用另一种分词器，Go 同款）。厂商的其它错误原样转成 anthropic
/// 形状交给客户端（429 带 Retry-After）。
pub fn count_tokens(gateway: &super::Gateway, body: &[u8]) -> Reply {
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => return error_reply(400, &format!("invalid request body: {e}")),
    };
    let model = parsed
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();
    let counted = groups::plan_for(gateway, &model)
        .into_iter()
        .next()
        .and_then(|cand| target_for(gateway, &cand).ok())
        .filter(|t| !t.anthropic.is_empty());
    if let Some(target) = counted {
        let out_body = rewrite_model(body, &target.bare_model);
        match send_anthropic(&target, "/v1/messages/count_tokens", &out_body) {
            Ok(response) => {
                let status = response.status();
                let vendor_body = read_all(response.into_reader());
                if status < 400 {
                    return Reply::Json {
                        status,
                        body: vendor_body,
                    };
                }
                if !unsupported_count(status, &vendor_body) {
                    return vendor_error_reply(status, &vendor_body);
                }
                // 不支持计数：落到下面的本地粗估
            }
            Err(e) => {
                return error_reply(502, &format!("{}: {}", target.provider_id, e));
            }
        }
    }
    Reply::json(200, &json!({ "input_tokens": estimate_tokens(&parsed) }))
}

/// Go `unsupportedCount`：厂商说「我不数」。400 只认点名计数操作/端点本身的话，
/// 不把提到的不支持的图片/模型/工具参数当成「不支持计数」。
fn unsupported_count(status: u16, body: &[u8]) -> bool {
    match status {
        404 | 405 | 501 => true,
        400 => {
            let said = vendor_error_message(status, body).to_lowercase();
            let said = said.trim_matches(|c: char| ": \t\r\n.。".contains(c));
            if matches!(
                said,
                "unsupported" | "unimplemented" | "not supported" | "not implemented"
            ) {
                return true;
            }
            const OPS: [&str; 4] = [
                "count_tokens",
                "counttokens",
                "token counting",
                "counting tokens",
            ];
            OPS.iter().any(|op| {
                [
                    "is not supported",
                    "not supported",
                    "is not implemented",
                    "not implemented",
                    "is unsupported",
                    "is unimplemented",
                ]
                .iter()
                .any(|w| said.contains(&format!("{op} {w}")))
                    || [
                        "unsupported",
                        "unimplemented",
                        "does not support",
                        "doesn't support",
                    ]
                    .iter()
                    .any(|w| said.contains(&format!("{w} {op}")))
            }) || said.contains("不支持") && OPS.iter().any(|op| said.contains(op))
        }
        _ => false,
    }
}

/// Go `estimate`：系统提示 + 各块文本/工具参数/名字 + 工具定义（名+描述+
/// schema）的字节数 / 4。图片不计（Go 同款）。给「发之前先问问」的客户端用。
fn estimate_tokens(req: &Value) -> usize {
    fn text_of(v: &Value) -> usize {
        match v {
            Value::String(s) => s.len(),
            Value::Array(items) => items
                .iter()
                .map(|b| {
                    b.get("text")
                        .and_then(|t| t.as_str())
                        .map(str::len)
                        .unwrap_or(0)
                })
                .sum(),
            _ => 0,
        }
    }
    let mut n = req.get("system").map(text_of).unwrap_or(0);
    if let Some(msgs) = req.get("messages").and_then(|m| m.as_array()) {
        for m in msgs {
            match m.get("content") {
                Some(Value::String(s)) => n += s.len(),
                Some(Value::Array(blocks)) => {
                    for b in blocks {
                        match b.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                            "text" => n += text_of(&Value::Array(vec![b.clone()])),
                            "tool_use" => {
                                n += b
                                    .get("name")
                                    .and_then(|x| x.as_str())
                                    .map(str::len)
                                    .unwrap_or(0);
                                n += b.get("input").map(|i| i.to_string().len()).unwrap_or(0);
                            }
                            "tool_result" => n += b.get("content").map(text_of).unwrap_or(0),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(tools) = req.get("tools").and_then(|t| t.as_array()) {
        for t in tools {
            n += t
                .get("name")
                .and_then(|x| x.as_str())
                .map(str::len)
                .unwrap_or(0);
            n += t
                .get("description")
                .and_then(|x| x.as_str())
                .map(str::len)
                .unwrap_or(0);
            n += t
                .get("input_schema")
                .map(|s| s.to_string().len())
                .unwrap_or(0);
        }
    }
    n / 4
}

fn attempt(
    gw: &super::Gateway,
    cand: &Candidate,
    body: &[u8],
    parsed: &Value,
    asked_model: &str,
    streaming: bool,
    log: &super::usage_ledger::TurnLog,
) -> Attempt {
    let target = match target_for(gw, cand) {
        Ok(t) => t,
        Err(e) => return Attempt::Retry(error_reply(404, &e)),
    };
    if !target.anthropic.is_empty() {
        return forward_direct(gw, &target, body, streaming, log);
    }
    if !target.chat.is_empty() {
        return forward_translated(gw, cand, parsed, asked_model, streaming, log);
    }
    // 只配了 responses（或什么都没有）：没有诚实的 chat/anthropic 出站点，
    // 别拿 responses 基址拼 /chat/completions 硬试（URL 形状就是错的）；
    // responses 翻译是 T4 的事。
    Attempt::Retry(error_reply(
        404,
        &format!("{} has no endpoint for /v1/messages", target.provider_id),
    ))
}

// =====================================================================
// 直通路径（anthropic URL 非空）
// =====================================================================

/// 直通一个成员。字节上限拒答走 bodycap 的 anthropic 形状 refit
/// （`messages[].content[].type=image` 丢最老的重发）。
fn forward_direct(
    gw: &super::Gateway,
    target: &Target,
    body: &[u8],
    streaming: bool,
    log: &super::usage_ledger::TurnLog,
) -> Attempt {
    let asked = body_model(body);
    let out_body = rewrite_model(body, &target.bare_model);
    // DeepSeek 系 anthropic 端点的 thinking 回放修补（Go gateway.go
    // `anthropicReplaysThinking(host, model) || (host, upstream)` → withThinkingOnToolTurns）：
    // 路由组里 Claude 成员掉到 DeepSeek 时 asked 是 claude-*、upstream 才是 deepseek，两个名字都要看。
    let host = chat::host_of(&target.anthropic);
    let out_body = if anthropic_replays_thinking(&host, &asked)
        || anthropic_replays_thinking(&host, &target.bare_model)
    {
        with_thinking_on_tool_turns(&out_body)
    } else {
        out_body
    };
    // cap 已知时先合身再发（免吃一次拒答）。
    let out_body = bodycap::prefit_shape(
        gw,
        &target.provider_id,
        bodycap::Shape::Anthropic,
        &out_body,
    )
    .unwrap_or(out_body);
    let response = match send_messages(target, &out_body) {
        Ok(r) => r,
        Err(e) => {
            return Attempt::Retry(error_reply(502, &format!("{}: {}", target.provider_id, e)))
        }
    };
    let status = response.status();
    if streaming && status < 400 {
        let content_type = response.content_type().to_string();
        if content_type.contains("event-stream") {
            // SSE 原样透传：字节一个不动，content-type 用厂商自己的。usage
            // 捕获（anthropic 方言）包外层——message_start/message_delta 提桶。
            let tap = log.tap(
                gw.usage.clone(),
                super::usage_ledger::Dialect::Anthropic,
                &target.provider_id,
                status,
            );
            return Attempt::Done(Reply::Stream {
                status,
                content_type,
                reader: Box::new(super::usage_ledger::TapReader::new(
                    tap,
                    response.into_reader(),
                )),
            });
        }
        // 2xx 但不是事件流（厂商没理会 stream）：整读按普通应答直通。
    }
    let vendor_body = read_all(response.into_reader());
    if status < 400 {
        log.whole(
            &gw.usage,
            super::usage_ledger::Dialect::Anthropic,
            &target.provider_id,
            status,
            &vendor_body,
        );
        return Attempt::Done(Reply::Json {
            status,
            body: vendor_body,
        });
    }
    if let Some(reply) = bodycap::maybe_refit_with(
        gw,
        &target.provider_id,
        bodycap::Shape::Anthropic,
        &out_body,
        status,
        &vendor_body,
        &|b| send_messages(target, b),
    ) {
        // 重发应答本来就是 anthropic 话：成功原样，失败（含 413 语义）转 anthropic 错误形状。
        return Attempt::Done(match reply {
            Reply::Json { status, body } if status >= 400 => vendor_error_reply(status, &body),
            Reply::Json { status, body } => {
                if status < 400 {
                    log.whole(
                        &gw.usage,
                        super::usage_ledger::Dialect::Anthropic,
                        &target.provider_id,
                        status,
                        &body,
                    );
                }
                Reply::Json { status, body }
            }
            Reply::Stream {
                status,
                content_type,
                reader,
            } => {
                let tap = log.tap(
                    gw.usage.clone(),
                    super::usage_ledger::Dialect::Anthropic,
                    &target.provider_id,
                    status,
                );
                Reply::Stream {
                    status,
                    content_type,
                    reader: Box::new(super::usage_ledger::TapReader::new(tap, reader)),
                }
            }
        });
    }
    let retry = groups::retryable(status, &vendor_body);
    let reply = vendor_error_reply(status, &vendor_body);
    if retry {
        Attempt::Retry(reply)
    } else {
        Attempt::Done(reply)
    }
}

/// 直通出站：与 chat::send 同骨架（ureq Status(4xx/5xx) 取回不当错误），
/// 认证头换 anthropic 三件套。
fn send_messages(target: &Target, body: &[u8]) -> Result<ureq::Response, String> {
    send_anthropic(target, "/v1/messages", body)
}

/// 直通出站（路径可换：/v1/messages 与 /v1/messages/count_tokens 共用）。
fn send_anthropic(target: &Target, path: &str, body: &[u8]) -> Result<ureq::Response, String> {
    let agent = super::outbound_agent();
    let url = format!("{}{}", target.anthropic, path);
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
        Ok(response) => {
            super::httpd::note_upstream(&response);
            Ok(response)
        }
        Err(ureq::Error::Status(_, response)) => {
            super::httpd::note_upstream(&response);
            Ok(response)
        }
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

fn body_model(body: &[u8]) -> String {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(String::from))
        .unwrap_or_default()
}

/// Go `anthropicModel`：`(?i)(?:^|[/.:-])claude-`——Anthropic 自家模型（含
/// 中转写法 `anthropic/claude-…`、`bedrock.claude-…`）。
fn is_claude_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.match_indices("claude-")
        .any(|(i, _)| i == 0 || matches!(m.as_bytes()[i - 1], b'/' | b'.' | b':' | b'-'))
}

/// Go `anthropicReplaysThinking`：该主机的 Messages API 是否要求每个调用工具
/// 的 assistant 回合带 thinking 块——DeepSeek 的 Anthropic 端点（thinking 是
/// 默认，也是被请求时）对没有它的 tool_use 回合 400「The `content[].thinking`
/// in the thinking mode must be passed back to the API」，并把 redacted_thinking
/// 当未知块 422（2026-10-11 对 api.deepseek.com/anthropic 实测，Go #1611）。
/// 它收 Claude 自己的签名 thinking、自己的、以及无签名带空格的那种。
fn anthropic_replays_thinking(host: &str, model: &str) -> bool {
    if host == "api.anthropic.com" || is_claude_model(model) {
        return false;
    }
    host.contains("deepseek") || model.to_ascii_lowercase().contains("deepseek")
}

/// Go `withThinkingOnToolTurns`：请求里每条带 tool_use 的 assistant 消息，或
/// 只回答 tool_result 的 user 之后的 assistant 消息（DeepSeek 也拒，2026-10-12，
/// Go #1822），若没有 thinking 块就在最前补一个 `" "` 占位 thinking（Go
/// `noReasoning`）；redacted_thinking（Claude 的密封块，DeepSeek 读不了）换成
/// 同一个占位，该消息已有 thinking 时直接丢掉它。其余消息与无需改动的体
/// 逐字节原样返回。
fn with_thinking_on_tool_turns(body: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(body);
    if !text.contains("\"tool_use\"") && !text.contains("\"redacted_thinking\"") {
        return body.to_vec();
    }
    let Ok(mut root) = serde_json::from_slice::<Value>(body) else {
        return body.to_vec();
    };
    let Some(msgs) = root.get_mut("messages").and_then(|m| m.as_array_mut()) else {
        return body.to_vec();
    };
    let space = json!({ "type": "thinking", "thinking": chat::NO_REASONING });
    let block_type = |b: &Value| {
        b.get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string()
    };
    let mut changed = false;
    // 前一条是「只有 tool_result」的 user 消息
    let mut after_tool = false;
    for msg in msgs.iter_mut() {
        let Some(role) = msg.get("role").and_then(|r| r.as_str()).map(String::from) else {
            after_tool = false;
            continue;
        };
        if role != "assistant" {
            after_tool = msg
                .get("content")
                .and_then(|c| c.as_array())
                .is_some_and(|blocks| {
                    !blocks.is_empty() && blocks.iter().all(|b| block_type(b) == "tool_result")
                });
            continue;
        }
        let after = after_tool;
        after_tool = false;
        let Some(blocks) = msg.get("content").and_then(|c| c.as_array()).cloned() else {
            continue;
        };
        let mut out: Vec<Value> = Vec::new();
        let (mut thought, mut called, mut edited) = (false, false, false);
        for b in blocks {
            match block_type(&b).as_str() {
                "thinking" => thought = true,
                "tool_use" => called = true,
                "redacted_thinking" => {
                    edited = true;
                    if thought {
                        continue;
                    }
                    out.push(space.clone());
                    thought = true;
                    continue;
                }
                _ => {}
            }
            out.push(b);
        }
        if (called || after) && !thought {
            out.insert(0, space.clone());
            edited = true;
        }
        if edited {
            msg["content"] = Value::Array(out);
            changed = true;
        }
    }
    if !changed {
        return body.to_vec();
    }
    serde_json::to_vec(&root).unwrap_or_else(|_| body.to_vec())
}

// =====================================================================
// 翻译路径（anthropic URL 空、chat URL 有）
// =====================================================================

fn forward_translated(
    gw: &super::Gateway,
    cand: &Candidate,
    parsed: &Value,
    asked_model: &str,
    streaming: bool,
    log: &super::usage_ledger::TurnLog,
) -> Attempt {
    let chat_body = match translate::anthropic_to_chat(parsed) {
        Ok(v) => v,
        Err(e) => return Attempt::Done(error_reply(400, &e)),
    };
    // 流式注入 stream_options.include_usage——Go buildChat 同款（chat.go：
    // `if r.Stream { out["stream_options"] = … }`），OpenAI 形态的厂商只有
    // 这个开关才会把最终 usage 帧发出来，anthropic 客户端的 message_delta
    // 才有真数可带。model 的裸名替换、prefit、字节上限 refit、厂商合身都在
    // chat::attempt 里。
    let chat_body = {
        let mut v = chat_body;
        if streaming {
            if let Some(map) = v.as_object_mut() {
                map.insert("stream_options".into(), json!({ "include_usage": true }));
            }
        }
        v
    };
    // textcalls：请求提供了工具才装 scout（anthropic 工具名 = 翻译后 chat
    // 的函数名，translate 不改名；Go 安装点 textCallNames(request.Tools)）。
    let scout = offered_scout(parsed);
    let chat_bytes = match serde_json::to_vec(&chat_body) {
        Ok(b) => b,
        Err(e) => return Attempt::Done(error_reply(500, &format!("re-serialize failed: {e}"))),
    };
    // 入站头尚未穿进 messages 翻译路径（差距清单）：体派生种子——codex 的
    // prompt_cache_key 在体里全保真；agent 头名族在此路不生效，穿线另开小批。
    let oc = super::opencode_client::gateway_ctx(&[], &chat_bytes);
    match chat::attempt(gw, cand, &chat_bytes, &oc) {
        Attempt::Retry(Reply::Json { status, body }) => {
            Attempt::Retry(vendor_error_reply(status, &body))
        }
        Attempt::Retry(other) => Attempt::Retry(other),
        Attempt::Done(reply) => Attempt::Done(chat_reply_to_anthropic(
            reply,
            streaming,
            asked_model,
            scout,
            gw,
            &cand.provider,
            log,
        )),
    }
}

/// 请求提供了 anthropic 工具 → textcalls 的 scout（None = 没提供，惰性
/// 直通——Go 在安装点整个跳过，gateway.go:9644-9647）。
fn offered_scout(parsed: &Value) -> Option<super::textcalls::TextCallScout> {
    let names: Vec<String> = parsed
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
                .filter(|n| !n.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    (!names.is_empty()).then(|| super::textcalls::TextCallScout::new(&names))
}

/// chat 世界的终局应答 → anthropic 面：事件流逐事件翻译；整块 JSON 转
/// anthropic message；错误转 anthropic 错误形状。
///
/// usage 记账：最终面向客户端的是 anthropic 方言——流式 tap 包在翻译器
/// 外层（message_start/message_delta 帧），整块在折好的 message 对象上扫
/// （`anthropic_usage_of` 补的 0 桶被 tap 的 >0 过滤掉，不编数）。
#[allow(clippy::too_many_arguments)]
pub(super) fn chat_reply_to_anthropic(
    reply: Reply,
    streaming: bool,
    asked_model: &str,
    scout: Option<super::textcalls::TextCallScout>,
    gw: &super::Gateway,
    provider: &str,
    log: &super::usage_ledger::TurnLog,
) -> Reply {
    match reply {
        Reply::Stream {
            status,
            content_type,
            reader,
        } => {
            if status < 400 && streaming && content_type.contains("event-stream") {
                // chat SSE → anthropic SSE 逐事件翻译（message_start 编排、单次收口
                // 都在翻译器里）。textcalls 的 scout 挂在帧编排器上。
                let mut translator = AnthropicSseTranslator::new(reader, asked_model);
                translator.frames.scout = scout;
                let tap = log.tap(
                    gw.usage.clone(),
                    super::usage_ledger::Dialect::Anthropic,
                    provider,
                    status,
                );
                return Reply::Stream {
                    status,
                    content_type: "text/event-stream".into(),
                    reader: Box::new(super::usage_ledger::TapReader::new(tap, translator)),
                };
            }
            // 厂商没给事件流（无视了 stream）：整读再按非流式转。
            let body = read_all(reader);
            chat_json_to_anthropic(status, body, asked_model, scout, gw, provider, log)
        }
        Reply::Json { status, body } => {
            chat_json_to_anthropic(status, body, asked_model, scout, gw, provider, log)
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn chat_json_to_anthropic(
    status: u16,
    body: Vec<u8>,
    asked_model: &str,
    scout: Option<super::textcalls::TextCallScout>,
    gw: &super::Gateway,
    provider: &str,
    log: &super::usage_ledger::TurnLog,
) -> Reply {
    if status >= 400 {
        return vendor_error_reply(status, &body);
    }
    // textcalls：整块正文里的书写调用变 tool_calls（Go wholeTextCalls），
    // 下面的转换器把它们映射成 tool_use 块。
    let body = match scout {
        Some(sc) => match serde_json::from_slice::<Value>(&body) {
            Ok(mut v) => {
                chat::whole_text_calls(&mut v, sc);
                serde_json::to_vec(&v).unwrap_or(body)
            }
            // 解析失败的错误面交给下面的原路径统一出。
            Err(_) => body,
        },
        None => body,
    };
    match chat_response_to_anthropic(&body, asked_model) {
        Ok(v) => {
            let out = serde_json::to_vec(&v).unwrap_or(body);
            log.whole(
                &gw.usage,
                super::usage_ledger::Dialect::Anthropic,
                provider,
                status,
                &out,
            );
            Reply::Json { status, body: out }
        }
        // 厂商 2xx 却给了读不懂的体：网关侧问题，按 502 说。
        Err(e) => error_reply(502, &e),
    }
}

// =====================================================================
// SSE 翻译器：chat 事件流 → anthropic 事件流
// =====================================================================

/// chat → anthropic 的帧编排器（无 IO：吃一条 chat data 行，吐 anthropic
/// 帧文本）。职责：message_start 首帧编排、content_block index 递增
/// （text 块与 tool_use 块共享一条序列，anthropic 规矩）、tool_calls
/// 分片拼装（translate::ChatToolAccum，惰性冲刷）、单次 message_stop
/// 收口。流式 Read 适配器与 refit 整读路径共用这一个状态机。
struct AnthropicFrames {
    /// message_start 已发（第一帧必须是它，anthropic SDK 的硬要求）。
    started: bool,
    /// 已收口（message_stop 之后的一切尾巴全部丢弃，一条流只收口一次）。
    stopped: bool,
    /// 下一个 content_block 的 index（块开一次递增一次）。
    next_index: usize,
    /// 开着的 text 块 index（Some = 文本块开着；文本块与 pending 工具
    /// 互斥——工具分片到达即封文本块，正文到达即冲刷工具族）。
    text_open: Option<usize>,
    /// 开着的 thinking 块 index（chat 的 delta.reasoning_content → anthropic
    /// thinking_delta；没有它 Claude Code 看不到共享模型的思考）。
    think_open: Option<usize>,
    /// chat tool_calls 分片拼装（Go chatDecoder 子集，两侧端点共享）。
    tools: translate::ChatToolAccum,
    /// textcalls 持有者（None = 请求没提供工具，正文惰性直通）。
    scout: Option<super::textcalls::TextCallScout>,
    /// finish_reason 已到（**只记账不收口**：chat 流的 usage 帧在 finish
    /// 之后——include_usage 时是独立末帧，responses.rs 的同款口径——
    /// message_delta/message_stop 推迟到 [DONE]/EOF 才发，usage 才真）。
    finish: Option<String>,
    /// 见过的最后一份 usage（finish 帧 或 include_usage 末帧）。
    usage: Option<Value>,
}

impl AnthropicFrames {
    fn new() -> AnthropicFrames {
        AnthropicFrames {
            started: false,
            stopped: false,
            next_index: 0,
            text_open: None,
            think_open: None,
            tools: translate::ChatToolAccum::new(),
            scout: None,
            finish: None,
            usage: None,
        }
    }

    /// 一条 chat data 行（JSON chunk 或 `[DONE]`）→ anthropic 帧文本
    /// （多帧以 `\n\n` 连接；空串 = 本行无输出）。首条有输出的行自带
    /// message_start 前缀（model 参数给 message_start 回显用）。
    fn feed(&mut self, data: &str, model: &str) -> String {
        if self.stopped {
            return String::new();
        }
        let mut out = String::new();
        if data == "[DONE]" {
            // textcalls：finish 没来过的怪流，持有的按原文放（Go readSSE
            // 结束后 held() 的 release；finish 来过时 scout 已在 finish 帧交账）。
            out.push_str(&self.release_scout());
            out.push_str(&self.close_think());
            out.push_str(&self.close_text());
            out.push_str(&self.flush_tools());
            match self.finish.take() {
                // finish 来过：补收尾两帧（带上等到的 usage）。
                Some(stop) => out.push_str(&self.message_end(&stop, self.usage.as_ref())),
                // finish 没来过的怪流：Go 网关仍发 message_delta（合成
                // end_turn + 等到的 usage），Claude Code 靠它结算上下文；
                // 只发裸 message_stop 会让客户端丢了收尾帧（2026-11-18
                // 合同 D1c 对齐）。
                None => out.push_str(&self.message_end("stop", self.usage.as_ref())),
            }
            self.stopped = true;
            return self.with_start(out, model);
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            return String::new(); // 杂行安静跳过，绝不中断流
        };
        // 上游中途失败（codex 翻译器等给出的 `{"error":{…}}` 帧）：封开着的块、
        // 发 anthropic 的 error 事件并收口——不静默吞成「正常结束」。
        if let Some(err) = v.get("error").filter(|e| e.is_object()) {
            // textcalls：流错误 → 持有的按原文放（Go KError → release）。
            let mut out = self.release_scout();
            out.push_str(&self.close_think());
            out.push_str(&self.close_text());
            out.push_str(&self.flush_tools());
            let message = err
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or("upstream stream error");
            out.push_str(&sse_frame(
                "error",
                &json!({"type": "error", "error": {"type": "api_error", "message": message}}),
            ));
            self.stopped = true;
            return self.with_start(out, model);
        }
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            self.usage = Some(u.clone());
        }
        let choice = v.pointer("/choices/0");
        // 同一条 chunk 里 content 与 tool_calls 并存时正文在前（Go
        // chatDecoder 同款顺序：先 text 后 toolCall）。
        let think = choice
            .and_then(|c| {
                c.pointer("/delta/reasoning_content")
                    .or_else(|| c.pointer("/delta/reasoning"))
            })
            .and_then(|d| d.as_str())
            .unwrap_or("");
        if !think.is_empty() {
            out.push_str(&self.think_delta(think));
        }
        let text = choice
            .and_then(|c| c.pointer("/delta/content"))
            .and_then(|d| d.as_str())
            .unwrap_or("");
        if !text.is_empty() {
            // textcalls：正文先过 scout——标记出现后这一段被持有（不发
            // text_delta），放出的照发（Go textCallSee 的 KText）。
            let released = match self.scout.as_mut() {
                Some(sc) => sc.feed_text(text),
                None => text.to_string(),
            };
            if !released.is_empty() {
                out.push_str(&self.close_think());
                out.push_str(&self.text_delta(&released));
            }
        }
        if let Some(calls) = choice
            .and_then(|c| c.pointer("/delta/tool_calls"))
            .and_then(|t| t.as_array())
        {
            if !calls.is_empty() {
                // 工具分片到达：文本块到此为止，分片进拼装器（事件族惰性）。
                // textcalls：回复自己发起了调用——settle（首个原生调用前
                // tagsOnly 仍生效），放出的文本按正文发（Go KToolStart）。
                if let Some(sc) = self.scout.as_mut() {
                    let rest = sc.note_native_call();
                    if !rest.is_empty() {
                        out.push_str(&self.text_delta(&rest));
                    }
                }
                out.push_str(&self.close_think());
                out.push_str(&self.close_text());
                self.tools.feed(calls);
            }
        }
        if let Some(stop) = choice
            .and_then(|c| c.get("finish_reason"))
            .and_then(|f| f.as_str())
            .filter(|f| !f.is_empty())
        {
            self.finish = Some(stop.to_string());
            // textcalls：回合结束（Go 的 KStop 在 finish 帧上发）——读出的
            // 调用进拼装器、正文调用改的收尾按 tool_use；正文剩余按文本块发。
            out.push_str(&self.finish_scout());
            // 封块现在发（anthropic 规矩：所有 content_block_stop 在
            // message_delta 前），收尾两帧等流尾。
            out.push_str(&self.close_think());
            out.push_str(&self.close_text());
            out.push_str(&self.flush_tools());
        }
        self.with_start(out, model)
    }

    /// EOF 兜底（没等到 [DONE]）：冲刷 pending 的工具与开着的文本块；
    /// finish 来过就补收尾两帧（回合是真完了，缺的只是 [DONE]），
    /// 没来过就到此为止——流是被截断的，不编收口。
    fn flush_truncated(&mut self, model: &str) -> String {
        // textcalls：finish 来过时 scout 已在 finish 帧交账（take 后为
        // None）；截断流持有的按原文放（Go held() 的 release）。
        let mut out = self.release_scout();
        out.push_str(&self.close_think());
        out.push_str(&self.close_text());
        out.push_str(&self.flush_tools());
        if !self.stopped {
            if let Some(stop) = self.finish.take() {
                out.push_str(&self.message_end(&stop, self.usage.as_ref()));
                self.stopped = true;
            }
        }
        self.with_start(out, model)
    }

    /// textcalls 交账（Go textCallSee 的 KStop，textcalls.go:779）：读出的
    /// 调用合成 chat 分片进拼装器（flush_tools 的三段式白拿）；正文调用
    /// 改成回合收尾时把 stop_reason 置 tool_calls（anthropic 的 tool_use，
    /// Go `ev.Stop = "tool"`）；剩余文本按文本块发。
    fn finish_scout(&mut self) -> String {
        let Some(sc) = self.scout.take() else {
            return String::new();
        };
        let fin = sc.finish();
        let mut out = String::new();
        if !fin.text.is_empty() {
            out.push_str(&self.text_delta(&fin.text));
        }
        if !fin.calls.is_empty() {
            let deltas: Vec<Value> = fin
                .calls
                .into_iter()
                .map(|(name, args)| {
                    json!({
                        "index": 0, "id": chat::text_call_id(), "type": "function",
                        "function": {"name": name, "arguments": args},
                    })
                })
                .collect();
            self.tools.feed(&deltas);
            self.finish = Some("tool_calls".into());
        }
        out
    }

    /// textcalls 的错误/截断路径（Go KError 与 readSSE 结束后的 held()）：
    /// 持有的按原文放出，不解析。
    fn release_scout(&mut self) -> String {
        let Some(mut sc) = self.scout.take() else {
            return String::new();
        };
        let rest = sc.release_rest();
        if rest.is_empty() {
            String::new()
        } else {
            self.text_delta(&rest)
        }
    }

    /// 首条输出前插 message_start（started 只翻一次）。
    fn with_start(&mut self, mut out: String, model: &str) -> String {
        if !self.started && !out.is_empty() {
            self.started = true;
            out.insert_str(0, &message_start_frame(model));
        }
        out
    }

    /// 一条正文 delta：必要时先冲刷 pending 工具族（到达序），文本块
    /// 没开就开（content_block_start type=text）。
    fn text_delta(&mut self, text: &str) -> String {
        let mut out = self.flush_tools();
        let index = match self.text_open {
            Some(i) => i,
            None => {
                let i = self.next_index;
                self.next_index += 1;
                self.text_open = Some(i);
                out.push_str(&sse_frame(
                    "content_block_start",
                    &json!({"type": "content_block_start", "index": i,
                        "content_block": {"type": "text", "text": ""}}),
                ));
                i
            }
        };
        out.push_str(&sse_frame(
            "content_block_delta",
            &json!({"type": "content_block_delta", "index": index,
                "delta": {"type": "text_delta", "text": text}}),
        ));
        out
    }

    /// 一条思考 delta：thinking 块没开就开（content_block_start type=thinking）。
    fn think_delta(&mut self, text: &str) -> String {
        let mut out = String::new();
        let index = match self.think_open {
            Some(i) => i,
            None => {
                out.push_str(&self.close_text());
                out.push_str(&self.flush_tools());
                let i = self.next_index;
                self.next_index += 1;
                self.think_open = Some(i);
                out.push_str(&sse_frame(
                    "content_block_start",
                    &json!({"type": "content_block_start", "index": i,
                        "content_block": {"type": "thinking", "thinking": ""}}),
                ));
                i
            }
        };
        out.push_str(&sse_frame(
            "content_block_delta",
            &json!({"type": "content_block_delta", "index": index,
                "delta": {"type": "thinking_delta", "thinking": text}}),
        ));
        out
    }

    /// 封口开着的 thinking 块（幂等）。
    fn close_think(&mut self) -> String {
        match self.think_open.take() {
            Some(i) => sse_frame(
                "content_block_stop",
                &json!({"type": "content_block_stop", "index": i}),
            ),
            None => String::new(),
        }
    }

    /// 封口开着的文本块（幂等）。
    fn close_text(&mut self) -> String {
        match self.text_open.take() {
            Some(i) => sse_frame(
                "content_block_stop",
                &json!({"type": "content_block_stop", "index": i}),
            ),
            None => String::new(),
        }
    }

    /// 冲刷拼装完成的工具调用：三段式（start：tool_use 空 input →
    /// delta：input_json_delta（拼装结果整段一帧）→ stop）。空参数合成
    /// `"{}"`（Go anthropicEncoder.close 同款）——客户端把 partial_json
    /// 串起来解析，串不出对象整个调用就废了。
    fn flush_tools(&mut self) -> String {
        let mut out = String::new();
        for call in self.tools.take() {
            let index = self.next_index;
            self.next_index += 1;
            // 空 id 兜底合成（Go anthropicEncoder 同款）：客户端要拿 id
            // 回传 tool_result，空串接不上。chat 的 call_* id 原样透传——
            // 往返只要求同一根串，不要求 anthropic 的 toolu_ 前缀。
            let id = if call.id.is_empty() {
                fresh_toolu_id()
            } else {
                call.id.clone()
            };
            out.push_str(&sse_frame(
                "content_block_start",
                &json!({"type": "content_block_start", "index": index,
                    "content_block": {"type": "tool_use", "id": id, "name": call.name, "input": {}}}),
            ));
            let partial = if call.arguments.is_empty() {
                "{}".to_string()
            } else {
                call.arguments
            };
            out.push_str(&sse_frame(
                "content_block_delta",
                &json!({"type": "content_block_delta", "index": index,
                    "delta": {"type": "input_json_delta", "partial_json": partial}}),
            ));
            out.push_str(&sse_frame(
                "content_block_stop",
                &json!({"type": "content_block_stop", "index": index}),
            ));
        }
        out
    }

    /// 回合收尾两帧：message_delta（stop + usage 同帧）+ message_stop。
    /// 与 translate::sse_chat_to_anthropic 的 finish 分支同形状（那边对
    /// 带正文的帧会被 text 分支抢先，这里按帧面自建；stop/usage 的映射
    /// 用本模块非流式路径已有的镜像函数，改语义两处一起看）。
    fn message_end(&self, stop: &str, usage: Option<&Value>) -> String {
        let mut out = sse_frame(
            "message_delta",
            &json!({"type": "message_delta",
                "delta": {"stop_reason": chat_stop_to_anthropic(stop), "stop_sequence": null},
                "usage": anthropic_usage_of(usage)}),
        );
        out.push_str(&sse_frame("message_stop", &json!({"type": "message_stop"})));
        out
    }
}

/// 包在厂商 SSE reader 外的 Read 适配器：逐行取 `data: …` 喂
/// [`AnthropicFrames`]，帧文本进缓冲再按 read 的粒度吐出。
pub(super) struct AnthropicSseTranslator {
    inner: BufReader<Box<dyn Read + Send>>,
    /// 待写出的 anthropic 字节（pos 之前已消费）。
    out: Vec<u8>,
    pos: usize,
    frames: AnthropicFrames,
    done: bool,
    model: String,
}

impl AnthropicSseTranslator {
    pub(super) fn new(reader: Box<dyn Read + Send>, model: &str) -> AnthropicSseTranslator {
        AnthropicSseTranslator {
            inner: BufReader::new(reader),
            out: Vec::new(),
            pos: 0,
            frames: AnthropicFrames::new(),
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
            if self.done || self.frames.stopped {
                self.done = true;
                return Ok(0);
            }
            let mut line = String::new();
            match self.inner.read_line(&mut line) {
                Ok(0) => {
                    // 上游 EOF：截断流兜底——pending 工具与开着的文本块
                    // 冲出去；finish 来过才补收尾两帧（缺的只是 [DONE]）。
                    let tail = self.frames.flush_truncated(&self.model);
                    if !tail.is_empty() {
                        self.out.extend_from_slice(tail.as_bytes());
                        continue;
                    }
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
            let Some(data) = trimmed.strip_prefix("data:") else {
                continue;
            };
            let frames = self.frames.feed(data.trim(), &self.model);
            if !frames.is_empty() {
                self.out.extend_from_slice(frames.as_bytes());
            }
        }
    }
}

/// 一条 anthropic SSE 事件的完整帧（`event: <type>\ndata: {…}\n\n`，
/// translate.rs sse_block 的端点侧镜像——那边是私有的）。
fn sse_frame(event: &str, data: &Value) -> String {
    format!(
        "event: {}\ndata: {}\n\n",
        event,
        serde_json::to_string(data).unwrap_or_default()
    )
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
    sse_frame("message_start", &data)
}

/// 流式 tool_use 的空 id 兜底（进程内计数 + 纳秒时钟，同 message_start
/// 的 id 口径——只要不重复，不是安全敏感值）。
fn fresh_toolu_id() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("toolu_{:x}-{:x}", nanos, n)
}

/// 一整段 chat SSE 文本 → 完整 anthropic SSE 文本（refit 回路的整读形态）。
/// 一条 data 行都映射不上（不是 SSE）返回 None。与流式路径共用
/// [`AnthropicFrames`]，工具分片在此同样拼装。
fn sse_text_to_anthropic(body: &[u8], model: &str) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?;
    let mut frames = AnthropicFrames::new();
    let mut out = String::new();
    for line in text.lines() {
        if frames.stopped {
            break;
        }
        let Some(data) = line.trim_end().strip_prefix("data:") else {
            continue;
        };
        out.push_str(&frames.feed(data.trim(), model));
    }
    if out.is_empty() {
        return None;
    }
    Some(out)
}

// =====================================================================
// 非流式应答转换：chat completion → anthropic message
// =====================================================================

/// chat completion 应答 → anthropic message 应答。translate.rs 刻意不含
/// 应答面的转换（T1 的稳定 API 只有请求互转与 SSE 事件映射），这块留在
/// 端点里；stop/usage 的映射语义与 translate.rs 的私有函数同款（那边不
/// 导出，这里镜像，改语义时两处一起看）。
pub(super) fn chat_response_to_anthropic(body: &[u8], asked_model: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("bad vendor reply: {e}"))?;
    let choice = v
        .pointer("/choices/0")
        .ok_or_else(|| "vendor reply has no choices".to_string())?;
    let message = choice.get("message");
    let mut blocks: Vec<Value> = Vec::new();
    if let Some(think) = message
        .and_then(|m| m.get("reasoning_content").or_else(|| m.get("reasoning")))
        .and_then(|t| t.as_str())
        .filter(|t| !t.trim().is_empty())
    {
        blocks.push(json!({ "type": "thinking", "thinking": think }));
    }
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
    if let Some(calls) = message
        .and_then(|m| m.get("tool_calls"))
        .and_then(|c| c.as_array())
    {
        for tc in calls {
            let name = tc
                .pointer("/function/name")
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            // 零分数在解析前丢（serde 的 f64 会把 2500.0 原样序列化回来，
            // Claude Code 系客户端对浮点整数同样严格；integral.rs）。
            let args = tc
                .pointer("/function/arguments")
                .and_then(|a| a.as_str())
                .unwrap_or_default();
            let args = super::integral::whole_args(args);
            blocks.push(json!({
                "type": "tool_use",
                // chat 的 call_* id 原样透传：往返（下一条请求的 tool_result
                // 回来）只要求同一根串，不要求 anthropic 的 toolu_ 前缀。
                "id": tc.get("id").and_then(|i| i.as_str()).unwrap_or_default(),
                "name": name,
                "input": parse_tool_args(&args),
            }));
        }
    }
    let finish = choice
        .get("finish_reason")
        .and_then(|f| f.as_str())
        .unwrap_or("stop");
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

/// chat usage → anthropic usage：与翻译矩阵共用 translate::anthropic_usage
/// （Go 口径：input_tokens 不含缓存，缓存读/写单列）。
fn anthropic_usage_of(u: Option<&Value>) -> Value {
    translate::anthropic_usage(u)
}

// =====================================================================
// 错误面
// =====================================================================

/// anthropic 形状的错误应答（Go writeErrorAs 的 anthropic 分支）。
fn error_reply(status: u16, message: &str) -> Reply {
    Reply::json(
        status,
        &json!({ "type": "error", "error": { "type": error_type(status), "message": message } }),
    )
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
        return Reply::Json {
            status,
            body: body.to_vec(),
        };
    }
    error_reply(status, &vendor_error_message(status, body))
}

fn anthropic_error_shaped(body: &[u8]) -> bool {
    let Ok(v) = serde_json::from_slice::<Value>(body) else {
        return false;
    };
    v.get("type").and_then(|t| t.as_str()) == Some("error")
        && v.pointer("/error/type")
            .is_some_and(|t| t.as_str().is_some())
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
        let lock = super::super::TEST_SANDBOX_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
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
            self.headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
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
                thread_seen
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(Seen {
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
        FakeVendor {
            base: format!("http://127.0.0.1:{port}"),
            requests,
        }
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
        // 净额口径（Go chat.go Usage：prompt 含缓存，input 减去 cache 桶）
        assert_eq!(v["usage"]["input_tokens"], 2);
        assert_eq!(v["usage"]["output_tokens"], 2);
        assert_eq!(v["usage"]["cache_read_input_tokens"], 1);

        // 假厂商收到的是 chat 形状：剥前缀的裸模型名、system 折叠、Bearer。
        let seen = vendor.last();
        assert_eq!(seen.method, "POST");
        assert_eq!(seen.path, "/v1/chat/completions");
        assert_eq!(seen.header("authorization"), Some("Bearer sk-test"));
        let sent: Value = serde_json::from_str(&seen.body).expect("sent chat json");
        assert_eq!(sent["model"], "m");
        assert_eq!(
            sent["messages"][0],
            json!({"role": "system", "content": "be nice"})
        );
        assert_eq!(
            sent["messages"][1],
            json!({"role": "user", "content": "hi"})
        );
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
        assert_eq!(
            seen.path, "/v1/messages",
            "直通打到 anthropic 基址下的 /v1/messages"
        );
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
        let Reply::Stream {
            status,
            content_type,
            mut reader,
        } = reply
        else {
            panic!("expected stream reply, got json");
        };
        assert_eq!(status, 200);
        assert!(content_type.contains("text/event-stream"));
        let mut out = String::new();
        reader
            .read_to_string(&mut out)
            .expect("read translated stream");

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

    /// 解析产出的 anthropic SSE：每帧 (event 名, data JSON)。
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

    // -----------------------------------------------------------------
    // 流式工具调用（T15）
    // -----------------------------------------------------------------

    #[test]
    fn stream_translation_maps_tool_calls_to_tool_use_blocks() {
        // text delta + tool_calls 三分片（arguments 跨帧）+ finish(tool_calls)
        // + usage + [DONE] —— T15 的验收形状。
        let sse = concat!(
            r#"data: {"id":"c2","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c2","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"looking up"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c2","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_5","type":"function","function":{"name":"get_weather","arguments":"{\"ci"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c2","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ty\":\"Tokyo\""}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c2","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"}"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c2","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":9,"completion_tokens":5}}"#,
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
            "messages": [{ "role": "user", "content": "weather?" }],
            "tools": [{ "name": "get_weather", "description": "weather",
                "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}} }],
        });
        let reply = handle(&gateway, serde_json::to_vec(&request).unwrap().as_slice());
        // 流式请求同样带 tools 定义（翻译是体级的，与 stream 无关）。
        let sent: Value = serde_json::from_str(&vendor.last().body).expect("sent chat json");
        assert_eq!(sent["tools"][0]["function"]["name"], "get_weather");
        assert_eq!(sent["stream_options"], json!({"include_usage": true}));

        let Reply::Stream {
            status,
            content_type,
            mut reader,
        } = reply
        else {
            panic!("expected stream reply, got json");
        };
        assert_eq!(status, 200);
        assert!(content_type.contains("text/event-stream"));
        let mut out = String::new();
        reader
            .read_to_string(&mut out)
            .expect("read translated stream");
        let events = parse_frames(&out);
        let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "message_start",
                "content_block_start", // text 块 index 0
                "content_block_delta", // text_delta
                "content_block_stop",  // 工具分片到达，文本块封口
                "content_block_start", // tool_use 块 index 1（三段式起）
                "content_block_delta", // input_json_delta（拼装结果整段）
                "content_block_stop",
                "message_delta",
                "message_stop",
            ],
            "tool_use 三段式与 text 块按到达序、index 递增"
        );
        // text 块 index 0。
        assert_eq!(events[1].1["index"], json!(0));
        assert_eq!(events[1].1["content_block"]["type"], "text");
        assert_eq!(events[2].1["delta"]["text"], "looking up");
        assert_eq!(events[3].1["index"], json!(0));
        // tool_use 块 index 1：id / name / 空 input。
        assert_eq!(events[4].1["index"], json!(1));
        let block = &events[4].1["content_block"];
        assert_eq!(block["type"], "tool_use");
        assert_eq!(block["id"], "call_5");
        assert_eq!(block["name"], "get_weather");
        assert_eq!(block["input"], json!({}));
        // input_json_delta：拼装结果整段一帧。
        assert_eq!(events[5].1["index"], json!(1));
        assert_eq!(events[5].1["delta"]["type"], "input_json_delta");
        assert_eq!(events[5].1["delta"]["partial_json"], r#"{"city":"Tokyo"}"#);
        assert_eq!(events[6].1["index"], json!(1));
        // 收尾：chat 的 tool_calls → anthropic 的 tool_use。
        assert_eq!(events[7].1["delta"]["stop_reason"], "tool_use");
        assert_eq!(events[7].1["usage"]["input_tokens"], 9);
        // 一条流只收口一次。
        assert_eq!(out.matches("event: message_stop").count(), 1);
    }

    #[test]
    fn stream_translation_interleaves_text_and_tools_with_indices() {
        // 正文 → 工具分片 → 又一段正文：三块 index 0/1/2 递增，工具族在
        // 第二段正文之前冲刷（到达序）。
        let sse = concat!(
            r#"data: {"choices":[{"index":0,"delta":{"content":"one"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"t","arguments":"{}"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"index":0,"delta":{"content":"two"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
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
            "messages": [{ "role": "user", "content": "hi" }],
        });
        let reply = handle(&gateway, serde_json::to_vec(&request).unwrap().as_slice());
        let Reply::Stream { mut reader, .. } = reply else {
            panic!("stream reply")
        };
        let mut out = String::new();
        reader
            .read_to_string(&mut out)
            .expect("read translated stream");
        let events = parse_frames(&out);
        let shape: Vec<(String, usize)> = events
            .iter()
            .filter(|(n, _)| n.starts_with("content_block"))
            .map(|(n, d)| (n.clone(), d["index"].as_u64().unwrap_or(999) as usize))
            .collect();
        assert_eq!(
            shape,
            vec![
                ("content_block_start".into(), 0),
                ("content_block_delta".into(), 0),
                ("content_block_stop".into(), 0),
                ("content_block_start".into(), 1), // tool_use
                ("content_block_delta".into(), 1),
                ("content_block_stop".into(), 1),
                ("content_block_start".into(), 2), // 第二段正文新开文本块
                ("content_block_delta".into(), 2),
                ("content_block_stop".into(), 2),
            ],
            "同轮多块 index 递增，工具族穿插在两段正文之间"
        );
        assert_eq!(events[0].0, "message_start");
    }

    #[test]
    fn stream_translation_truncated_flushes_tool_without_stop() {
        // 厂商半途断流（无 finish 无 [DONE]）：tool_use 三段式照发（工具
        // 调用是真的），但没有 message_stop——流是被截断的，不编收口。
        let sse = concat!(
            r#"data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"c1","type":"function","function":{"name":"read","arguments":"{\"p\":1}"}}]},"finish_reason":null}]}"#,
            "\n\n"
        );
        let vendor = fake_vendor(200, "text/event-stream", sse);
        let providers = format!(
            r#"{{"providers":[{{"id":"fake","name":"Fake","key":"sk-test","chat":"{}/v1"}}]}}"#,
            vendor.base
        );
        let (gateway, _sandbox) = sandbox(&providers);
        let request = json!({
            "model": "fake/m", "max_tokens": 64, "stream": true,
            "messages": [{ "role": "user", "content": "go" }],
        });
        let reply = handle(&gateway, serde_json::to_vec(&request).unwrap().as_slice());
        let Reply::Stream { mut reader, .. } = reply else {
            panic!("stream reply")
        };
        let mut out = String::new();
        reader
            .read_to_string(&mut out)
            .expect("read translated stream");
        let events = parse_frames(&out);
        let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
            ]
        );
        assert_eq!(events[1].1["content_block"]["type"], "tool_use");
        assert_eq!(events[2].1["delta"]["partial_json"], r#"{"p":1}"#);
        assert!(!out.contains("event: message_stop"), "截断流不编收口");
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
        let vendor = fake_vendor(
            401,
            "application/json",
            r#"{"error":{"message":"bad key"}}"#,
        );
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

    // -----------------------------------------------------------------
    // textcalls（正文里书写的工具调用，翻译路径应答侧）
    // -----------------------------------------------------------------

    /// DeepSeek 自家形：正文里写 `<tool_call>` 包 `<｜DSML｜invoke …>` 参数。
    const DSML_TEXT: &str = concat!(
        "看一下。",
        "<tool_call>\n<｜DSML｜invoke name=\"get_weather\">\n",
        "<｜DSML｜parameter name=\"city\" string=\"true\">Berlin</｜DSML｜parameter>\n",
        "</｜DSML｜invoke>\n</tool_call>"
    );

    fn textcall_tools() -> Value {
        json!([{ "name": "get_weather", "description": "weather",
            "input_schema": {"type": "object", "properties": {"city": {"type": "string"}}} }])
    }

    #[test]
    fn textcalls_nonstream_written_call_becomes_tool_use_block() {
        let reply_body = format!(
            r#"{{"id":"c1","object":"chat.completion","model":"m","choices":[{{"index":0,"message":{{"role":"assistant","content":{}}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":3,"completion_tokens":2}}}}"#,
            serde_json::to_string(DSML_TEXT).unwrap()
        );
        let reply_body: &'static str = Box::leak(reply_body.into_boxed_str());
        let vendor = fake_vendor(200, "application/json", reply_body);
        let providers = format!(
            r#"{{"providers":[{{"id":"fake","name":"Fake","key":"sk-test","chat":"{}/v1"}}]}}"#,
            vendor.base
        );
        let (gateway, _sandbox) = sandbox(&providers);
        let request = json!({
            "model": "fake/m", "max_tokens": 64,
            "messages": [{ "role": "user", "content": "weather?" }],
            "tools": textcall_tools(),
        });
        let reply = handle(&gateway, serde_json::to_vec(&request).unwrap().as_slice());
        let Reply::Json { status, body } = reply else {
            panic!("expected json reply, got stream");
        };
        assert_eq!(status, 200);
        let v: Value = serde_json::from_slice(&body).expect("anthropic json");
        let blocks = v["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2, "文本块 + tool_use 块：{blocks:?}");
        // 标记前正文原样，DSML 标记不再出现在文本里。
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], "看一下。");
        // 书写调用 → 真 tool_use 块。
        assert_eq!(blocks[1]["type"], "tool_use");
        assert_eq!(blocks[1]["name"], "get_weather");
        assert_eq!(blocks[1]["input"], json!({"city": "Berlin"}));
        assert!(
            blocks[1]["id"].as_str().unwrap().starts_with("call_text_"),
            "Go textCallID 形状：{}",
            blocks[1]["id"]
        );
        // 回合作为调过工具的回合结束（Go `ev.Stop = "tool"`）。
        assert_eq!(v["stop_reason"], "tool_use");
    }

    #[test]
    fn textcalls_stream_written_call_becomes_tool_use_block() {
        // 正文分两片：标记前的说明先放，DSML 块整片被持有；finish 到了才交账。
        let sse = concat!(
            "data: {\"id\":\"c2\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"看一下。\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"c2\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"<tool_call>\\n<｜DSML｜invoke name=\\\"get_weather\\\">\\n<｜DSML｜parameter name=\\\"city\\\" string=\\\"true\\\">Berlin</｜DSML｜parameter>\\n</｜DSML｜invoke>\\n</tool_call>\"},\"finish_reason\":null}]}\n\n",
            "data: {\"id\":\"c2\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":5}}\n\n",
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
            "messages": [{ "role": "user", "content": "weather?" }],
            "tools": textcall_tools(),
        });
        let reply = handle(&gateway, serde_json::to_vec(&request).unwrap().as_slice());
        let Reply::Stream {
            status,
            content_type,
            mut reader,
        } = reply
        else {
            panic!("expected stream reply, got json");
        };
        assert_eq!(status, 200);
        assert!(content_type.contains("text/event-stream"));
        let mut out = String::new();
        reader
            .read_to_string(&mut out)
            .expect("read translated stream");
        let events = parse_frames(&out);
        let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "message_start",
                "content_block_start", // text 块 index 0
                "content_block_delta", // text_delta "看一下。"
                "content_block_stop",  // finish 到达，文本块封口
                "content_block_start", // tool_use 块 index 1（三段式）
                "content_block_delta", // input_json_delta
                "content_block_stop",
                "message_delta",
                "message_stop",
            ],
            "书写调用在 finish 帧交账，tool_use 三段式在文本块后"
        );
        assert_eq!(events[1].1["index"], json!(0));
        assert_eq!(events[2].1["delta"]["text"], "看一下。");
        let block = &events[4].1["content_block"];
        assert_eq!(events[4].1["index"], json!(1));
        assert_eq!(block["type"], "tool_use");
        assert_eq!(block["name"], "get_weather");
        assert!(block["id"].as_str().unwrap().starts_with("call_text_"));
        assert_eq!(events[5].1["delta"]["partial_json"], r#"{"city":"Berlin"}"#);
        // 回合收尾按 tool_use（Go `ev.Stop = "tool"`）。
        assert_eq!(events[7].1["delta"]["stop_reason"], "tool_use");
        // 持有的 DSML 标记绝不当 text_delta 发出去。
        assert!(!out.contains("DSML"), "持有文本不进 text_delta：{out}");
    }

    #[test]
    fn textcalls_without_tools_stays_text() {
        // 请求没提供工具：scout 不装（Go 在安装点整个跳过），正文原样。
        let reply_body = format!(
            r#"{{"id":"c1","object":"chat.completion","model":"m","choices":[{{"index":0,"message":{{"role":"assistant","content":{}}},"finish_reason":"stop"}}]}}"#,
            serde_json::to_string(DSML_TEXT).unwrap()
        );
        let reply_body: &'static str = Box::leak(reply_body.into_boxed_str());
        let vendor = fake_vendor(200, "application/json", reply_body);
        let providers = format!(
            r#"{{"providers":[{{"id":"fake","name":"Fake","key":"sk-test","chat":"{}/v1"}}]}}"#,
            vendor.base
        );
        let (gateway, _sandbox) = sandbox(&providers);
        let request = json!({
            "model": "fake/m", "max_tokens": 64,
            "messages": [{ "role": "user", "content": "weather?" }],
        });
        let reply = handle(&gateway, serde_json::to_vec(&request).unwrap().as_slice());
        let Reply::Json { status, body } = reply else {
            panic!("expected json reply, got stream");
        };
        assert_eq!(status, 200);
        let v: Value = serde_json::from_slice(&body).expect("anthropic json");
        let blocks = v["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "text");
        assert_eq!(blocks[0]["text"], DSML_TEXT, "正文一个字不动");
        assert_eq!(v["stop_reason"], "end_turn");
    }

    // -----------------------------------------------------------------
    // usage 记账（capture 层，Go sniff.go 的 anthropic 方言口径）
    // -----------------------------------------------------------------

    /// 翻译路径流式回合落账：chat usage 折成 anthropic 帧（净额 input +
    /// 缓存单列）→ tap 从 message_delta 提桶；metadata.user_id 做种子。
    #[test]
    fn translated_stream_turn_records_usage_buckets() {
        let sse = concat!(
            r#"data: {"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":900,"completion_tokens":30,"total_tokens":930,"prompt_tokens_details":{"cached_tokens":400,"cache_write_tokens":120}}}"#,
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
            "metadata": { "user_id": "user-77" },
            "messages": [{ "role": "user", "content": "hi" }]
        });
        let reply = handle(&gateway, serde_json::to_vec(&request).unwrap().as_slice());
        let Reply::Stream { mut reader, .. } = reply else {
            panic!("expected stream reply, got json");
        };
        let mut out = String::new();
        reader
            .read_to_string(&mut out)
            .expect("read translated stream");
        assert!(out.contains("event: message_delta"));
        let rows = gateway.usage.records_since(0);
        assert_eq!(rows.len(), 1, "一条回合一行");
        let r = &rows[0];
        assert_eq!(r.agent, "anthropic");
        assert_eq!(r.provider, "fake");
        assert_eq!(r.model, "fake/m");
        assert_eq!(
            r.session.as_deref(),
            Some("user-77"),
            "body 侧种子（metadata.user_id）"
        );
        // anthropic 口径：input 只数非缓存（chat 的 900 含缓存 → 900−400−120）。
        assert_eq!(r.in_tokens, Some(380));
        assert_eq!(r.out_tokens, Some(30));
        assert_eq!(r.cache_read, Some(400));
        assert_eq!(r.cache_write, Some(120));
        assert!(r.millis > 0);
    }

    /// 直通透传流也是 anthropic 方言：message_start 的 input+缓存两桶 +
    /// message_delta 的 output 桶，两处分头到、零桶不编。
    #[test]
    fn direct_stream_turn_records_anthropic_usage() {
        let sse = concat!(
            "event: message_start\n",
            r#"data: {"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":650,"output_tokens":1,"cache_read_input_tokens":500,"cache_creation_input_tokens":80}}}"#,
            "\n\n",
            "event: content_block_delta\n",
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}}"#,
            "\n\n",
            "event: message_delta\n",
            r#"data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":27}}"#,
            "\n\n",
            "event: message_stop\n",
            r#"data: {"type":"message_stop"}"#,
            "\n\n",
        );
        let vendor = fake_vendor(200, "text/event-stream", sse);
        let providers = format!(
            r#"{{"providers":[{{"id":"anth","name":"Anth","key":"sk-ant","anthropic":"{}"}}]}}"#,
            vendor.base
        );
        let (gateway, _sandbox) = sandbox(&providers);
        let request = json!({
            "model": "anth/claude-x", "max_tokens": 32, "stream": true,
            "messages": [{ "role": "user", "content": "hi" }]
        });
        let reply = handle(&gateway, serde_json::to_vec(&request).unwrap().as_slice());
        let Reply::Stream { mut reader, .. } = reply else {
            panic!("expected stream reply, got json");
        };
        let mut out = String::new();
        reader
            .read_to_string(&mut out)
            .expect("read passthrough stream");
        assert!(out.contains("event: message_stop"), "字节照转: {out}");
        let rows = gateway.usage.records_since(0);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.agent, "anthropic");
        assert_eq!(r.provider, "anth");
        // message_start 的 input/缓存 + message_delta 的 output（后到覆盖占位 1）。
        assert_eq!(
            r.in_tokens,
            Some(650),
            "anthropic 的 input_tokens 本就只数非缓存"
        );
        assert_eq!(r.out_tokens, Some(27));
        assert_eq!(r.cache_read, Some(500));
        assert_eq!(r.cache_write, Some(80));
    }
}
