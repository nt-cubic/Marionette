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
//! codex 特有字段（`include`）在翻译路径下安静丢弃；`instructions` 折进
//! chat 的 system 消息（translate）。`store` / `previous_response_id` 的
//! 服务端存回合语义由 [`super::prev_response`] 的内存表承担（本文件
//! `prev_merge` / `seal_prev`）：store:true 的应答存 (input, output) 返回
//! 网关发的 id；下一轮点名 previous_response_id → 合并整段历史再翻译；
//! miss → 显式 400（`previous_response_not_found`），绝不静默重放空历史。
//! **直通路径不接管 store**——透传原样（真 OpenAI 自家管；Go 对 relay 的
//! 自留回合另有 prevDelta 增量改写，这里不做，差距记录在 prev_response.rs）。
//!
//! 流式 usage：给翻译后的 chat 请求补 `stream_options.include_usage`
//! （Go buildChat 同款，chat.go:636）——否则 chat 厂商不在流尾发 usage 帧，
//! completed 事件的 usage 只能报 0。finish_reason 帧先到时**不**立即
//! completed：chat 流的 usage 帧在 finish 之后（include_usage 时是独立
//! 末帧），先封口 usage 就丢了——等 usage 或 [DONE]。
//!
//! 组路由 + 失败转移（groups.rs）：asked 解析成有序候选，每个成员各自直通或
//! 翻译；只在应答头阶段换成员。直通路径的字节上限 refit 用 responses 形状
//! （`input[].content[].type=input_image`，bodycap::Shape::Responses）。
//! reasoning item：翻译到 chat 时文本暂存进 assistant 消息（translate.rs），
//! 由 chat::fit_for_vendor 按厂商回放/丢弃。
//! 未覆盖（列给账本）：reasoning item 的**应答方向**（chat 流没有对应物，
//! 不编造）。流式 function_call 自 T15 起有完整映射
//! （chat 的 tool_calls 分片经 translate::ChatToolAccum 拼装，一次调用
//! 出一族 `output_item.added → arguments.delta → arguments.done →
//! output_item.done`，与文本 item 按到达序交错）。
//!
//! textcalls（正文里书写的工具调用，`super::textcalls`，Go textcalls.go
//! textCallSee）：**翻译路径的应答文本**过 TextCallScout——请求提供了工具
//! 时，从第一个伪调用标记起的正文被持有，回合结束（finish_reason）可解析
//! 块变成真 function_call（事件族/还原表复用 ChatToolAccum + call_to），
//! 截断流只按原文放出。offered 名单取翻译后 chat 请求的工具名（flat 名，
//! Go 安装点 textCallNames(request.Tools)）。chat 直通与 responses 直通
//! 都不装（Go 的 textCallTidy / textcalls_relay 是独立变体，差距保留）。
//!
//! Codex 的非 function 工具（custom / namespace / tool_search）：翻译请求时按
//! Go readResponses 压平成 chat 函数（flat 名唯一、≤64、`[a-zA-Z0-9_-]`），
//! 还原表 `translate::ToolTable` 随请求走到本文件的应答编码器（流式与非流式），
//! 调用按 Go callTo 还原成 custom_tool_call / 带 namespace 的 function_call /
//! tool_search_call。web_search / image_generation / local_shell 与 Go 一样有意丢弃。
//!
//! 参照（只读研究克隆）：magpie internal/gateway/responses.go 的
//! responsesEncoder（事件序列与帧形状）/ renderResponses（非流式应答）。
use std::io::{BufRead, BufReader, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::bodycap;
use super::groups::Attempt;
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

/// POST /v1/responses 入口。解析 model → 组路由（有序候选，失败转移）→
/// 每个成员直通或翻译。
///
/// usage 记账（capture 层 2026-11-18 接上）：2xx 回合在收口处落一行
/// （agent="responses"）——流式（直通透传与翻译输出）由
/// `usage_ledger::TapReader` 包在 reader 外层，从 response.completed 的
/// usage 提桶（input_tokens 含缓存 → 减两桶得净额，Go rUsage 口径）；
/// 非流式整块扫描。会话种子取 body 侧（路由层不传请求头，
/// `stick_key(&[], …)`——头携带的会话是记录在案的差距）。
pub fn handle(gateway: &super::Gateway, body: &[u8]) -> Reply {
    let began = std::time::Instant::now();
    let model = asked_model(body);
    if model.is_empty() {
        return Reply::error(400, "invalid request: model");
    }
    let cands = super::groups::plan_for(gateway, &model);
    if cands.is_empty() {
        return Reply::error(404, &format!("no provider for model: {model}"));
    }
    let parsed = serde_json::from_slice::<Value>(body).unwrap_or(Value::Null);
    let log = super::usage_ledger::TurnLog::new(
        "responses",
        &model,
        super::affinity::stick_key(&[], &parsed),
        began,
    );
    super::groups::drive(&cands, |cand| {
        let body = super::groups::body_for(gateway, super::effort::Proto::Responses, cand, body);
        attempt(gateway, cand, &body, &log)
    })
}

/// 一个成员的一次尝试：直通优先，responses 端点不存在再退翻译。
fn attempt(
    gateway: &super::Gateway,
    cand: &super::groups::Candidate,
    body: &[u8],
    log: &super::usage_ledger::TurnLog,
) -> Attempt {
    // 锁内只做表查询与字段快照，锁外跑网络（store 锁绝不持着等 HTTP——
    // 2026-10-31 storage 锁事故红线）。
    let (responses_url, key, headers, has_chat) = {
        let store = match gateway.store.lock() {
            Ok(s) => s,
            Err(_) => return Attempt::Done(Reply::error(500, "store poisoned")),
        };
        let Some(provider) = store.find(&cand.provider) else {
            return Attempt::Retry(Reply::error(
                404,
                &format!("no provider for model: {}/{}", cand.provider, cand.model),
            ));
        };
        (
            provider.responses.clone(),
            provider.key.clone(),
            provider
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect::<Vec<_>>(),
            !provider.chat.is_empty(),
        )
    };
    if !responses_url.is_empty() {
        let target = DirectTarget {
            provider_id: &cand.provider,
            base: &responses_url,
            key: &key,
            headers: &headers,
        };
        match passthrough(gateway, &target, body, &cand.model, log) {
            // 预设表给了 responses 端点不等于厂商真有（DeepSeek 一类：用户机上
            // magpie 学到后把它清空了）：端点不存在（404/405/501）且来源有 chat
            // 基址 → 退回翻译路径，别把 codex 晾在一个不存在的端点上。
            // 还有一种：responses 端点存在，但模型不是原生 Responses；
            // codex 会带 `reasoning.context`，厂商回 400 要求 native Responses
            // target。翻译到 chat 时这个字段本来就不带，退回翻译路径。
            Attempt::Retry(reply) if has_chat && passthrough_fallback_to_chat(&reply) => {}
            Attempt::Done(reply) if has_chat && passthrough_fallback_to_chat(&reply) => {}
            Attempt::Retry(reply) => return Attempt::Retry(reply),
            Attempt::Done(reply) => return Attempt::Done(reply),
        }
    }
    if !has_chat {
        return Attempt::Retry(Reply::error(
            404,
            &format!("{} has no endpoint for /v1/responses", cand.provider),
        ));
    }
    translate_via_chat(gateway, cand, body, log)
}

// =====================================================================
// 直通路径
// =====================================================================

/// 直通失败后是否退回 chat 翻译路径。
///
/// 404/405/501 = responses 端点不存在；400 + `reasoning.context` +
/// native Responses target = 端点在、但模型不是原生 Responses，codex 带的
/// `reasoning.context` 不能直通。翻译路径不转发 `reasoning.context`。
fn passthrough_fallback_to_chat(reply: &Reply) -> bool {
    let Reply::Json { status, body } = reply else {
        return false;
    };
    match *status {
        404 | 405 | 501 => true,
        400 => native_responses_reasoning_context_refusal(body),
        _ => false,
    }
}

fn native_responses_reasoning_context_refusal(body: &[u8]) -> bool {
    let lower = String::from_utf8_lossy(body).to_lowercase();
    lower.contains("reasoning.context") && lower.contains("native responses target")
}

struct DirectTarget<'a> {
    provider_id: &'a str,
    base: &'a str,
    key: &'a str,
    headers: &'a [(String, String)],
}

fn send_direct(target: &DirectTarget, body: &[u8]) -> Result<ureq::Response, String> {
    let url = format!("{}/responses", target.base.trim_end_matches('/'));
    let agent = super::outbound_agent();
    let mut request = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json");
    if !target.key.is_empty() {
        request = request.set("Authorization", &format!("Bearer {}", target.key));
    }
    for (k, v) in target.headers {
        request = request.set(k, v);
    }
    match request.send_bytes(body) {
        Ok(r) => {
            super::httpd::note_upstream(&r);
            Ok(r)
        }
        // ureq 2 把非 2xx 当 Err(Status(code, resp))——厂商的 4xx/5xx 是
        // 合同面的一部分，原样取回 response（chat.rs 同款）。
        Err(ureq::Error::Status(_, response)) => {
            super::httpd::note_upstream(&response);
            Ok(response)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// 原样转发 `{responses}/responses`。SSE（stream:true）字节级透传；
/// 错误（4xx/5xx）原样透传——错误面是厂商与 codex 之间的事，重写只会
/// 丢厂商的原话。字节上限拒答走 bodycap 的 responses 形状 refit
/// （`input[].content[].type=input_image` 丢最老的重发——2026-11-12 ollama
/// 32MB 会话死锁在这条路径上同样会发生）。
fn passthrough(
    gateway: &super::Gateway,
    target: &DirectTarget,
    body: &[u8],
    bare: &str,
    log: &super::usage_ledger::TurnLog,
) -> Attempt {
    let out_body = rewrite_model(body, bare);
    // cap 已知时先合身再发（免吃一次拒答）。
    let out_body = bodycap::prefit_shape(
        gateway,
        target.provider_id,
        bodycap::Shape::Responses,
        &out_body,
    )
    .unwrap_or(out_body);
    let streaming = serde_json::from_slice::<Value>(&out_body)
        .ok()
        .and_then(|v| v.get("stream").and_then(|s| s.as_bool()))
        .unwrap_or(false);
    let response = match send_direct(target, &out_body) {
        Ok(r) => r,
        Err(e) => return Attempt::Retry(Reply::error(502, &format!("responses passthrough: {e}"))),
    };
    let status = response.status();
    if status < 400 {
        if streaming {
            // usage 捕获：直通透传流也是 responses 方言（completed 帧带
            // usage），tap 包外层照提（Go sniff.go 的拷贝路径同款）。
            let tap = log.tap(
                gateway.usage.clone(),
                super::usage_ledger::Dialect::Responses,
                target.provider_id,
                status,
            );
            return Attempt::Done(Reply::Stream {
                status,
                content_type: response.content_type().to_string(),
                reader: Box::new(super::usage_ledger::TapReader::new(
                    tap,
                    response.into_reader(),
                )),
            });
        }
        let mut buf = Vec::new();
        let _ = response.into_reader().read_to_end(&mut buf);
        log.whole(
            &gateway.usage,
            super::usage_ledger::Dialect::Responses,
            target.provider_id,
            status,
            &buf,
        );
        return Attempt::Done(Reply::Json { status, body: buf });
    }
    let mut buf = Vec::new();
    let _ = response.into_reader().read_to_end(&mut buf);
    if let Some(reply) = bodycap::maybe_refit_with(
        gateway,
        target.provider_id,
        bodycap::Shape::Responses,
        &out_body,
        status,
        &buf,
        &|b| send_direct(target, b),
    ) {
        return Attempt::Done(tapped(reply, gateway, log, target.provider_id));
    }
    let retry = super::groups::retryable(status, &buf);
    let reply = Reply::Json { status, body: buf };
    if retry {
        Attempt::Retry(reply)
    } else {
        Attempt::Done(reply)
    }
}

/// bodycap refit 重发的终局应答挂 usage 记账（流包 tap、整块扫描）。
fn tapped(
    reply: Reply,
    gateway: &super::Gateway,
    log: &super::usage_ledger::TurnLog,
    provider: &str,
) -> Reply {
    match reply {
        Reply::Stream {
            status,
            content_type,
            reader,
        } => {
            let tap = log.tap(
                gateway.usage.clone(),
                super::usage_ledger::Dialect::Responses,
                provider,
                status,
            );
            Reply::Stream {
                status,
                content_type,
                reader: Box::new(super::usage_ledger::TapReader::new(tap, reader)),
            }
        }
        Reply::Json { status, body } => {
            if status < 400 {
                log.whole(
                    &gateway.usage,
                    super::usage_ledger::Dialect::Responses,
                    provider,
                    status,
                    &body,
                );
            }
            Reply::Json { status, body }
        }
    }
}

// =====================================================================
// 翻译路径
// =====================================================================

/// store:true 回合的落账上下文：应答侧拿它生成 id 并保存（Go
/// previousResponse 的 conversationWriter sink，prev_response.go:395 附近）。
pub(super) struct PrevSave {
    store: std::sync::Arc<super::prev_response::PrevStore>,
    /// 存账的 input（合并后的整段 items；无 previous_response_id 时就是
    /// 本轮自己的 items——Go kept(scope, nil, fresh) 首轮也存）。
    input: Value,
}

/// previous_response_id / store 的请求侧处理（Go previousResponse 的翻译
/// 路径子集，prev_response.go:332）：
/// - 命中 → 合并历史：**存的 input items 在前、存的 output items 作
///   assistant 回合、新 input 在后**（Go `input = append(prior, fresh…)`，
///   prior = 存账的 input+output 连续——prevTurn.items 是一条运行史）；
/// - miss → [`super::prev_response::prev_not_found`] 的 400（**绝不静默
///   重放空历史**——不自存回合的上游会以为对话从零开始）；
/// - input 读不成 items（既非串也非数组，Go prevInput 的 fresh==nil）→
///   整个让路：不合并、不存。
/// store==true → 交回 [`PrevSave`] 给应答侧落账。
fn prev_merge(
    gateway: &super::Gateway,
    mut parsed: Value,
) -> Result<(Value, Option<PrevSave>), Reply> {
    let prev_id = parsed
        .get("previous_response_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let store = parsed
        .get("store")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    // Go prevInput：串 → 一条 user message item；数组 → items 原样；其它不读。
    let fresh_items = |v: &Value| -> Option<Vec<Value>> {
        match v.get("input") {
            Some(Value::String(s)) => Some(vec![
                json!({ "type": "message", "role": "user", "content": s }),
            ]),
            Some(Value::Array(items)) => Some(items.clone()),
            _ => None,
        }
    };
    if !prev_id.is_empty() {
        if let Some(fresh) = fresh_items(&parsed) {
            match gateway.prev.replay(&prev_id) {
                Some(stored) => {
                    let mut items = stored.input.as_array().cloned().unwrap_or_default();
                    items.extend(stored.output.as_array().cloned().unwrap_or_default());
                    items.extend(fresh);
                    if let Some(map) = parsed.as_object_mut() {
                        map.insert("input".into(), Value::Array(items.clone()));
                        map.remove("previous_response_id");
                    }
                    if store {
                        let save = PrevSave {
                            store: gateway.prev.clone(),
                            input: Value::Array(items),
                        };
                        return Ok((parsed, Some(save)));
                    }
                    return Ok((parsed, None));
                }
                None => {
                    let (status, body) = super::prev_response::prev_not_found(&prev_id);
                    return Err(Reply::Json {
                        status,
                        body: serde_json::to_vec(&body).unwrap_or_default(),
                    });
                }
            }
        }
    }
    if store {
        if let Some(items) = fresh_items(&parsed) {
            let save = PrevSave {
                store: gateway.prev.clone(),
                input: Value::Array(items),
            };
            return Ok((parsed, Some(save)));
        }
    }
    Ok((parsed, None))
}

/// 订阅桥（antigravity_account）的 responses 入站也认 `previous_response_id` /
/// store：与翻译路径同一张服务端回合表、同一个 miss 口径。桥拿到的
/// [`PrevSave`] 原样交回 [`reshape`]。
pub(super) fn prev_prepare(
    gateway: &super::Gateway,
    parsed: Value,
) -> Result<(Value, Option<PrevSave>), Reply> {
    prev_merge(gateway, parsed)
}

/// store:true 回合的应答侧落账：生成 fresh_id、存（model=asked、
/// input=请求侧合并的整段、output=应答 output items）、应答 id 换成存账
/// id——客户端下一轮点名的就是它（流式路径在 ResponsesSse 收尾做同一件事）。
fn seal_prev(resp: &mut Value, prev: Option<PrevSave>, model: &str) {
    let Some(p) = prev else { return };
    let id = super::prev_response::fresh_id("resp_");
    p.store.save(
        &id,
        model,
        p.input,
        resp.get("output").cloned().unwrap_or_else(|| json!([])),
    );
    if let Some(map) = resp.as_object_mut() {
        map.insert("id".into(), json!(id));
    }
}

/// 翻译后 chat 请求的工具名（= 厂商/模型看到的 flat 名；Go 安装点
/// textCallNames(request.Tools) 吃的就是翻译后的 IR 工具）。空表 = 没提供
/// 工具 → scout 不装（Go 在安装点整个跳过，gateway.go:9644-9647）。
fn offered_names(chat_body: &Value) -> Vec<String> {
    chat_body
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| t.pointer("/function/name").and_then(|n| n.as_str()))
                .filter(|n| !n.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// responses → chat 出站（`chat::attempt` 全链：剥前缀 / Bearer / 厂商合身 /
/// prefit / bodycap 挂 4xx）→ 应答折回 responses 形状。
fn translate_via_chat(
    gateway: &super::Gateway,
    cand: &super::groups::Candidate,
    body: &[u8],
    log: &super::usage_ledger::TurnLog,
) -> Attempt {
    let parsed: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return Attempt::Done(Reply::error(400, "invalid request body")),
    };
    // asked 模型串回显进应答的 model 字段（codex 拿它对账自己在用的模型）。
    let asked = parsed
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or_default()
        .to_string();
    // previous_response_id / store 的服务端回合；miss 是显式 400——绝不
    // 静默重放空历史（Go prevLost → writePrevNotFound）。
    let (parsed, prev) = match prev_merge(gateway, parsed) {
        Ok(v) => v,
        Err(reply) => return Attempt::Done(reply),
    };
    // table：被压平的 custom / namespace / tool_search 工具的还原表，随这次请求
    // 走到应答编码器（不放全局——并发请求各带各的）。
    let (chat_value, table) = match super::translate::responses_to_chat_with_tools(&parsed) {
        Ok((mut v, table)) => {
            // 流式补 include_usage（Go buildChat 同款）：chat 厂商默认不在
            // 流尾发 usage 帧，completed 事件的 usage 就只能报 0。
            if v.get("stream").and_then(|s| s.as_bool()).unwrap_or(false) {
                if let Some(map) = v.as_object_mut() {
                    map.insert("stream_options".into(), json!({ "include_usage": true }));
                }
            }
            (v, table)
        }
        Err(e) => return Attempt::Done(Reply::error(400, &e)),
    };
    // textcalls：请求提供了工具才装 scout（offered 名单 = 翻译后的 flat 名）。
    let names = offered_names(&chat_value);
    let scout = (!names.is_empty()).then(|| super::textcalls::TextCallScout::new(&names));
    let chat_body = serde_json::to_vec(&chat_value).unwrap_or_default();
    // 入站头尚未穿进 responses 翻译路径（差距清单）：体派生种子（同 messages.rs 注）。
    let oc = super::opencode_client::gateway_ctx(&[], &chat_body);
    match super::chat::attempt(gateway, cand, &chat_body, &oc) {
        Attempt::Done(reply) => Attempt::Done(reshape(
            reply,
            &asked,
            &table,
            scout,
            prev,
            gateway,
            &cand.provider,
            log,
        )),
        Attempt::Retry(reply) => Attempt::Retry(reply),
    }
}

/// `chat::forward` 的应答 → responses 应答。错误（≥400）原样透传：chat
/// 家族的错误面本就是 `{"error":{…}}` 同构形状，重写只会丢厂商的原话，
/// 状态码透传。
///
/// usage 记账：最终面向客户端的是 responses 方言——整块在折好的 response
/// 对象上扫（`usage_to_responses` 补的 0 桶被 tap 的 >0 过滤掉，不编数）；
/// 流式 tap 包在 ResponsesSse 外层，从 completed 帧提桶。
#[allow(clippy::too_many_arguments)]
pub(super) fn reshape(
    reply: Reply,
    asked: &str,
    table: &super::translate::ToolTable,
    scout: Option<super::textcalls::TextCallScout>,
    prev: Option<PrevSave>,
    gateway: &super::Gateway,
    provider: &str,
    log: &super::usage_ledger::TurnLog,
) -> Reply {
    match reply {
        Reply::Json { status, body } => {
            if status < 400 {
                match serde_json::from_slice::<Value>(&body) {
                    Ok(mut v) => {
                        // textcalls：整块正文里的书写调用（Go wholeTextCalls）。
                        if let Some(sc) = scout {
                            super::chat::whole_text_calls(&mut v, sc);
                        }
                        let mut resp = chat_completion_to_response(&v, asked, table);
                        seal_prev(&mut resp, prev, asked);
                        let out = serde_json::to_vec(&resp).unwrap_or(body);
                        log.whole(
                            &gateway.usage,
                            super::usage_ledger::Dialect::Responses,
                            provider,
                            status,
                            &out,
                        );
                        Reply::Json { status, body: out }
                    }
                    // 2xx 但不是 JSON：翻译路径的客户端说的是 responses 话，
                    // 原样的 chat 噪音没有意义——诚实报 502。
                    Err(_) => Reply::error(502, "chat vendor replied a non-JSON body"),
                }
            } else {
                Reply::Json { status, body }
            }
        }
        Reply::Stream {
            status,
            content_type,
            mut reader,
        } => {
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
                    Ok(mut v) => {
                        if let Some(sc) = scout {
                            super::chat::whole_text_calls(&mut v, sc);
                        }
                        let mut resp = chat_completion_to_response(&v, asked, table);
                        seal_prev(&mut resp, prev, asked);
                        let out = serde_json::to_vec(&resp).unwrap_or(buf);
                        log.whole(
                            &gateway.usage,
                            super::usage_ledger::Dialect::Responses,
                            provider,
                            200,
                            &out,
                        );
                        Reply::Json {
                            status: 200,
                            body: out,
                        }
                    }
                    Err(_) => Reply::error(502, "chat vendor replied a non-JSON body"),
                };
            }
            let mut sse = ResponsesSse::new(reader, asked).with_tools(table.clone());
            if let Some(sc) = scout {
                sse = sse.with_scout(sc);
            }
            if let Some(p) = prev {
                sse = sse.with_prev(p);
            }
            let tap = log.tap(
                gateway.usage.clone(),
                super::usage_ledger::Dialect::Responses,
                provider,
                200,
            );
            Reply::Stream {
                status: 200,
                content_type: "text/event-stream".into(),
                reader: Box::new(super::usage_ledger::TapReader::new(tap, sse)),
            }
        }
    }
}

/// chat usage → responses usage。口径：OpenAI 两代的 input_tokens 都含
/// cached（Go `Usage.prompt()` 的注释——"every token the prompt came to,
/// as OpenAI's counts have it"），chat 的 prompt_tokens 同口径，直搬；
/// chat 不报的桶记 0，不编数。`cache_write_tokens` 进 details 与 Go
/// `Usage.responses()`（responses.go:1682）同形——Codex 也读它（#589），
/// usage 捕获层（usage_ledger 的 responses 方言）同字段提桶。
fn usage_to_responses(u: Option<&Value>) -> Value {
    let num = |v: Option<&Value>, k: &str| {
        v.and_then(|x| x.get(k))
            .and_then(|x| x.as_u64())
            .unwrap_or(0)
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
                .unwrap_or(0),
            "cache_write_tokens": u
                .and_then(|x| x.pointer("/prompt_tokens_details/cache_write_tokens"))
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
fn chat_completion_to_response(
    v: &Value,
    asked: &str,
    table: &super::translate::ToolTable,
) -> Value {
    let mut output: Vec<Value> = Vec::new();
    if let Some(msg) = v.pointer("/choices/0/message") {
        // 思考（DeepSeek 等的 reasoning_content）→ reasoning item，排在正文前。
        let think = msg
            .get("reasoning_content")
            .or_else(|| msg.get("reasoning"))
            .and_then(|t| t.as_str())
            .unwrap_or("");
        if !think.trim().is_empty() {
            output.push(json!({
                "id": format!("rs_{}", fresh_suffix()),
                "type": "reasoning", "status": "completed",
                "summary": [{ "type": "summary_text", "text": think }],
            }));
        }
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
                // flat 名按请求的还原表变回 Codex 认的 item（Go callTo）：
                // custom -> custom_tool_call(input) / 命名空间 -> function_call + namespace /
                // tool_search -> tool_search_call；普通工具原样。
                let flat = tc
                    .pointer("/function/name")
                    .and_then(|n| n.as_str())
                    .unwrap_or("")
                    .to_string();
                let mut item = json!({
                    "type": "function_call", "status": "completed",
                    "call_id": tc.get("id").cloned()
                        .unwrap_or_else(|| json!(format!("call_{}", fresh_suffix()))),
                    // 零分数先丢再出门：Codex 的 serde 对浮点整数整单拒绝
                    // （integral.rs / Go grok_integral.go）。
                    "arguments": tc.pointer("/function/arguments")
                        .and_then(|a| a.as_str())
                        .map(|a| json!(super::integral::whole_args(a)))
                        .or_else(|| tc.pointer("/function/arguments").cloned())
                        .unwrap_or_else(|| json!("{}")),
                });
                super::translate::call_to(&mut item, &flat, table);
                item["id"] = json!(format!(
                    "{}{}",
                    super::translate::call_item_prefix(&item),
                    fresh_suffix()
                ));
                output.push(item);
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
/// → 每次拼装完成的工具调用一族：`output_item.added`（function_call）→
///   `function_call_arguments.delta`（拼装结果整段一帧）→
///   `function_call_arguments.done` → `output_item.done`（完整 item：
///   name / arguments / call_id）——与文本 item 按到达序交错（正文先到
///   正文 item 先封口，工具先到事件族先发）
/// → `response.completed` / `response.incomplete`（usage + 完整 output）。
///
/// finish_reason 帧到了**不**立即收尾：chat 流的 usage 帧在 finish 之后
/// （include_usage 时是独立末帧），先封口 usage 就丢了——等 usage 或
/// [DONE]。上游 EOF 也没等到的（厂商半途断流），按已知的收尾（Go
/// readSSE 结束后 finish() 同款）：完成序列照发，usage 没有就 0；拼装到
/// 一半的工具调用按已收到的分片诚实发出（arguments 可能不完整）。
struct ResponsesSse {
    inner: BufReader<Box<dyn Read + Send>>,
    /// 待写出的字节（帧队列的物理形态），pos 是已消费位置。
    out: Vec<u8>,
    pos: usize,
    started: bool,
    item_open: bool,
    /// 思考 item（chat 流的 delta.reasoning_content → responses 的
    /// reasoning summary 事件族）。没有它 codex 看不到 DeepSeek 等模型的思考，
    /// 轨迹面板里也不会有 thought（用户 2026-11-18 实锤）。
    think_open: bool,
    think: String,
    think_id: String,
    think_index: usize,
    text: String,
    item_id: String,
    /// 打开中的 message item 的 output_index（开 item 时 = items.len()）。
    item_index: usize,
    /// 已封口的 items（completed 事件的 output 数组，按封口序）。
    items: Vec<Value>,
    /// chat tool_calls 分片拼装（惰性：收尾或正文交错时冲刷）。
    tools: super::translate::ChatToolAccum,
    /// 请求压平过的工具还原表（custom / 命名空间 / tool_search）。
    named: super::translate::ToolTable,
    /// textcalls 持有者（None = 请求没提供工具，正文惰性直通）。
    scout: Option<super::textcalls::TextCallScout>,
    /// store:true 回合的落账上下文（收尾时存，Go previousResponse 的 sink）。
    prev: Option<PrevSave>,
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
            think_open: false,
            think: String::new(),
            think_id: String::new(),
            think_index: 0,
            text: String::new(),
            item_id: String::new(),
            item_index: 0,
            items: Vec::new(),
            tools: super::translate::ChatToolAccum::new(),
            named: super::translate::ToolTable::new(),
            scout: None,
            prev: None,
            resp_id: format!("resp_{}", fresh_suffix()),
            seq: 0,
            model: model.to_string(),
            finish: None,
            usage: None,
            ended: false,
        }
    }

    /// 带上请求的工具还原表（`translate::responses_to_chat_with_tools`）。
    fn with_tools(mut self, named: super::translate::ToolTable) -> ResponsesSse {
        self.named = named;
        self
    }

    /// 带上 textcalls 持有者（请求提供了工具时才装）。
    fn with_scout(mut self, scout: super::textcalls::TextCallScout) -> ResponsesSse {
        self.scout = Some(scout);
        self
    }

    /// 带上 store:true 的落账上下文：id 直接用存账 id（created 帧与存账
    /// 同一个 id，客户端点名的就是它）。
    fn with_prev(mut self, prev: PrevSave) -> ResponsesSse {
        self.resp_id = super::prev_response::fresh_id("resp_");
        self.prev = Some(prev);
        self
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
    fn response_object(
        &self,
        status: &str,
        output: Value,
        usage: Value,
        incomplete: Value,
    ) -> Value {
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

    /// 思考增量：首条开 reasoning item + summary part，随后逐条 summary delta。
    fn think_delta(&mut self, delta: &str) {
        self.ensure_started();
        self.close_item(); // 正文 item 不跨着思考开（output_index 要保持单调）
        self.flush_tools();
        if !self.think_open {
            self.think_open = true;
            self.think_index = self.items.len();
            self.think_id = format!("rs_{}", fresh_suffix());
            let (id, idx) = (self.think_id.clone(), self.think_index);
            self.frame(
                "response.output_item.added",
                json!({"output_index": idx, "item": {"id": id, "type": "reasoning", "status": "in_progress", "summary": []}}),
            );
            self.frame(
                "response.reasoning_summary_part.added",
                json!({"item_id": &self.think_id, "output_index": idx, "summary_index": 0,
                    "part": {"type": "summary_text", "text": ""}}),
            );
        }
        self.think.push_str(delta);
        let (id, idx) = (self.think_id.clone(), self.think_index);
        self.frame(
            "response.reasoning_summary_text.delta",
            json!({"item_id": id, "output_index": idx, "summary_index": 0, "delta": delta}),
        );
    }

    /// 封口思考 item（正文/工具开始或收尾时调用）。
    fn close_think(&mut self) {
        if !self.think_open {
            return;
        }
        self.think_open = false;
        let (id, idx) = (self.think_id.clone(), self.think_index);
        let text = std::mem::take(&mut self.think);
        let part = json!({"type": "summary_text", "text": &text});
        self.frame(
            "response.reasoning_summary_text.done",
            json!({"item_id": &id, "output_index": idx, "summary_index": 0, "text": &text}),
        );
        self.frame(
            "response.reasoning_summary_part.done",
            json!({"item_id": &id, "output_index": idx, "summary_index": 0, "part": part.clone()}),
        );
        let item = json!({"id": id, "type": "reasoning", "status": "completed", "summary": [part]});
        self.frame(
            "response.output_item.done",
            json!({"output_index": idx, "item": item.clone()}),
        );
        self.items.push(item);
    }

    /// message item 的 added（懒发：第一条正文 delta 到来才开 item）。
    fn ensure_item(&mut self) {
        if self.item_open {
            return;
        }
        self.item_open = true;
        self.item_index = self.items.len();
        self.item_id = format!("msg_{}", fresh_suffix());
        let item_id = self.item_id.clone();
        let output_index = self.item_index;
        self.frame(
            "response.output_item.added",
            json!({
                "output_index": output_index,
                "item": {
                    "id": item_id,
                    "type": "message", "role": "assistant", "status": "in_progress",
                    "content": [],
                },
            }),
        );
    }

    /// 封口打开中的 message item（text.done + item.done，进 items 账）。
    /// 工具分片到达或收尾时调用——正文 item 在工具 item 前面，先封口。
    fn close_item(&mut self) {
        if !self.item_open {
            return;
        }
        self.item_open = false;
        let output_index = self.item_index;
        self.frame(
            "response.output_text.done",
            json!({
                "item_id": &self.item_id, "output_index": output_index, "content_index": 0,
                "text": &self.text,
            }),
        );
        let item = json!({
            "id": &self.item_id,
            "type": "message", "role": "assistant", "status": "completed",
            "content": [{ "type": "output_text", "text": &self.text, "annotations": [] }],
        });
        self.frame(
            "response.output_item.done",
            json!({ "output_index": output_index, "item": item.clone() }),
        );
        self.items.push(item);
    }

    /// 一段（已过 scout 的）正文 delta：按到达序冲刷思考/工具、开 message
    /// item、发 output_text.delta。feed 的正文分支与 textcalls 的交账共用。
    fn emit_text(&mut self, text: &str) {
        self.close_think();
        self.flush_tools();
        self.ensure_started();
        self.ensure_item();
        self.text.push_str(text);
        let item_id = self.item_id.clone();
        let output_index = self.item_index;
        self.frame(
            "response.output_text.delta",
            json!({
                "item_id": item_id, "output_index": output_index, "content_index": 0,
                "delta": text,
            }),
        );
    }

    /// textcalls 交账（Go textCallSee 的 KStop/KError 后置，textcalls.go:779）。
    /// 回合正常结束（finish_reason 到过，Go 的 KStop 在 finish 帧上发）→
    /// finish：读出的调用合成 chat 分片进拼装器（flush_tools 的事件族 +
    /// call_to 还原表白拿），剩余文本按正文发；截断（没有 finish）→ 只
    /// release：持有的按原文放出，不解析（Go readSSE 结束后 held() 同款）。
    fn finish_scout(&mut self) {
        let Some(mut sc) = self.scout.take() else {
            return;
        };
        if self.finish.is_none() {
            let rest = sc.release_rest();
            if !rest.is_empty() {
                self.emit_text(&rest);
            }
            return;
        }
        let fin = sc.finish();
        if !fin.text.is_empty() {
            self.emit_text(&fin.text);
        }
        if !fin.calls.is_empty() {
            let deltas: Vec<Value> = fin
                .calls
                .into_iter()
                .map(|(name, args)| {
                    json!({
                        "index": 0, "id": super::chat::text_call_id(), "type": "function",
                        "function": {"name": name, "arguments": args},
                    })
                })
                .collect();
            self.tools.feed(&deltas);
        }
    }

    /// 冲刷拼装完成的工具调用：每次调用一族
    /// `output_item.added → arguments.delta → arguments.done → output_item.done`
    /// （responsesEncoder 的 KToolStart/KToolArgs/closeItem 同形状；arguments
    /// 拼装结果整段一帧 delta——分片是 chat 线协议的事，responses 客户端
    /// 只要增量语义，不欠它厂商的切片粒度）。空 arguments 的 delta 不发
    /// （Go KToolArgs 同款跳空），done/item 的 arguments 空时记 "{}"。
    fn flush_tools(&mut self) {
        self.ensure_started();
        for call in self.tools.take() {
            let output_index = self.items.len();
            let call_id = if call.id.is_empty() {
                format!("call_{}", fresh_suffix())
            } else {
                call.id.clone()
            };
            let args = call.arguments.trim();
            let args = if args.is_empty() { "{}" } else { args };
            // 厂商给的是 flat 名：按还原表决定这是 function_call / 带 namespace 的
            // function_call / custom_tool_call / tool_search_call（Go callTo）。
            let kind = self.named.get(&call.name).cloned().unwrap_or_default();
            let mut added = json!({
                "type": "function_call", "status": "in_progress",
                "call_id": &call_id, "arguments": "",
            });
            super::translate::call_to(&mut added, &call.name, &self.named);
            let item_id = format!(
                "{}{}",
                super::translate::call_item_prefix(&added),
                fresh_suffix()
            );
            added["id"] = json!(&item_id);
            self.frame(
                "response.output_item.added",
                json!({ "output_index": output_index, "item": added }),
            );
            let mut done_item = json!({
                "id": &item_id,
                "type": "function_call", "status": "completed",
                // custom 的 input 取自原始 arguments：空就是空串，不是 "{}"
                "call_id": &call_id, "arguments": if kind.custom { call.arguments.trim() } else { args },
            });
            super::translate::call_to(&mut done_item, &call.name, &self.named);
            if kind.custom {
                // 自由文本工具：一整段 input（chat 分片已拼完，不欠厂商的切片粒度），
                // 事件族是 custom_tool_call_input.delta/done（OpenAI 同款）
                let input = done_item
                    .get("input")
                    .and_then(|i| i.as_str())
                    .unwrap_or("")
                    .to_string();
                if !input.is_empty() {
                    self.frame(
                        "response.custom_tool_call_input.delta",
                        json!({ "item_id": &item_id, "output_index": output_index, "delta": &input }),
                    );
                }
                self.frame(
                    "response.custom_tool_call_input.done",
                    json!({ "item_id": &item_id, "output_index": output_index, "input": &input }),
                );
            } else {
                if !call.arguments.is_empty() {
                    self.frame(
                        "response.function_call_arguments.delta",
                        json!({
                            "item_id": &item_id, "output_index": output_index,
                            "delta": &call.arguments,
                        }),
                    );
                }
                // tool_search 由 Codex 自己跑，Go 不发 arguments.done
                if !kind.search {
                    let mut done = json!({
                        "item_id": &item_id, "output_index": output_index,
                        "call_id": &call_id, "arguments": args,
                    });
                    super::translate::call_to(&mut done, &call.name, &self.named);
                    self.frame("response.function_call_arguments.done", done);
                }
            }
            self.frame(
                "response.output_item.done",
                json!({ "output_index": output_index, "item": done_item.clone() }),
            );
            self.items.push(done_item);
        }
    }

    /// 终结序列：textcalls 交账 → text.done → item.done →（工具事件族）→
    /// completed/incomplete。store:true 的回合在收尾时落账（Go prevReply：
    /// 流里读得到 completed（= finish 到过）才存；截断流不存）。
    fn finish_stream(&mut self) {
        if self.ended {
            return;
        }
        self.ensure_started(); // 空流也要有 created——completed 不能凭空来
        self.finish_scout();
        let (status, event, incomplete): (&str, &str, Value) = match self.finish.as_deref() {
            Some("length") => (
                "incomplete",
                "response.incomplete",
                json!({ "reason": "max_output_tokens" }),
            ),
            Some("content_filter") => (
                "incomplete",
                "response.incomplete",
                json!({ "reason": "content_filter" }),
            ),
            _ => ("completed", "response.completed", Value::Null),
        };
        let usage = usage_to_responses(self.usage.as_ref());
        self.close_think();
        self.close_item();
        self.flush_tools();
        // 空回合（无正文无工具）的 output 就是空数组——诚实汇报。
        let output = Value::Array(std::mem::take(&mut self.items));
        if self.finish.is_some() {
            if let Some(p) = self.prev.take() {
                p.store
                    .save(&self.resp_id, &self.model, p.input, output.clone());
            }
        }
        let response = self.response_object(status, output, usage, incomplete);
        self.frame(event, json!({ "response": response }));
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
        let think = choice
            .and_then(|c| {
                c.pointer("/delta/reasoning_content")
                    .or_else(|| c.pointer("/delta/reasoning"))
            })
            .and_then(|d| d.as_str())
            .unwrap_or("");
        if !think.is_empty() {
            self.think_delta(think);
        }
        let text = choice
            .and_then(|c| c.pointer("/delta/content"))
            .and_then(|d| d.as_str())
            .unwrap_or("");
        if !text.is_empty() {
            // textcalls：正文先过 scout——书写调用的标记出现后这一段被持有
            // （不发 delta），放出的照发（Go textCallSee 的 KText）。
            let released = match self.scout.as_mut() {
                Some(sc) => sc.feed_text(text),
                None => text.to_string(),
            };
            if !released.is_empty() {
                // 正文在工具分片之后到达：先按到达序冲刷工具事件族，再开新 item。
                self.emit_text(&released);
            }
        }
        if let Some(calls) = choice
            .and_then(|c| c.pointer("/delta/tool_calls"))
            .and_then(|t| t.as_array())
        {
            if !calls.is_empty() {
                // 工具分片到达：正文 item 到此为止（先封口），分片进拼装器，
                // 事件族等收尾或下次正文交错再发（chat 流没有「调用完了」信号）。
                // textcalls：回复自己发起了调用——settle（首个原生调用前
                // tagsOnly 仍生效），放出的文本按正文发（Go KToolStart）。
                if let Some(sc) = self.scout.as_mut() {
                    let out = sc.note_native_call();
                    if !out.is_empty() {
                        self.emit_text(&out);
                    }
                }
                self.close_think();
                self.close_item();
                self.tools.feed(calls);
            }
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
        let lock = super::super::TEST_SANDBOX_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
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
        (
            gateway,
            Sandbox {
                dir,
                prior_xdg,
                lock,
            },
        )
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

    fn fake_vendor(
        status: u16,
        content_type: &'static str,
        reply_body: &'static str,
    ) -> FakeVendor {
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
                let path = request_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or("")
                    .to_string();
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
                    .push(Seen {
                        path,
                        authorization,
                        body,
                    });
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
        // codex 形态的请求：instructions + 文本 input + 特有字段（store 现在
        // 有服务端存回合语义——这里只验证不进 chat 请求；previous_response_id
        // 的存取/合并/miss 由 prev_response_* 系列测试覆盖）。
        let body = br#"{"model":"p/m","instructions":"be nice","input":"hi",
            "store":true,"include":["usage"]}"#;
        let reply = handle(&gateway, body);

        // 打到 chat 厂商：Bearer、路径、翻译后的形状。
        assert_eq!(vendor.hits.load(Ordering::SeqCst), 1);
        let seen = vendor.seen.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(seen[0].path, "/v1/chat/completions");
        assert_eq!(seen[0].authorization, "Bearer sk-test");
        let sent: Value = serde_json::from_slice(&seen[0].body).unwrap();
        assert_eq!(sent["model"], "m", "provider/ 前缀已剥");
        assert_eq!(
            sent["messages"][0],
            json!({"role": "system", "content": "be nice"})
        );
        assert_eq!(
            sent["messages"][1],
            json!({"role": "user", "content": "hi"})
        );
        assert!(sent.get("store").is_none(), "codex 特有字段不进 chat 请求");
        assert!(
            sent.get("previous_response_id").is_none() && sent.get("include").is_none(),
            "codex 特有字段不进 chat 请求"
        );

        // 应答折成 responses 形状。
        let Reply::Json { status, body } = reply else {
            panic!("json reply")
        };
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
            r#"data: {"id":"chatcmpl-9","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"chatcmpl-9","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"content":"你好"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"chatcmpl-9","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}}"#,
            "\n\n",
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
        let Reply::Stream {
            status,
            content_type,
            mut reader,
        } = reply
        else {
            panic!("stream reply")
        };
        assert_eq!(status, 200);
        assert_eq!(content_type, "text/event-stream");
        let mut out = String::new();
        reader.read_to_string(&mut out).expect("read sse");
        assert!(
            out.starts_with("event: response.created\ndata: "),
            "帧格式：event 行在前"
        );
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
    fn translated_streaming_tool_call_event_family() {
        // T15 的验收形状：text delta + tool_calls 三分片（arguments 跨帧）+
        // finish(tool_calls) + usage + [DONE]。
        let sse = concat!(
            r#"data: {"id":"c2","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c2","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"content":"checking…"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c2","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_77","type":"function","function":{"name":"get_weather","arguments":"{\"ci"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c2","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ty\":\"Berlin\""}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c2","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"}"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c2","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":11,"completion_tokens":6,"total_tokens":17}}"#,
            "\n\n",
            "data: [DONE]\n\n"
        );
        let vendor = fake_vendor(200, "text/event-stream", sse);
        let (gateway, _sandbox) = sandbox(&format!(
            r#"{{"providers":[{{"id":"p","name":"P","key":"sk-test","chat":"{}/v1"}}]}}"#,
            vendor.base
        ));
        let reply = handle(
            &gateway,
            br#"{"model":"p/m","input":"weather?","stream":true,
                "tools":[{"type":"function","name":"get_weather","description":"weather",
                    "parameters":{"type":"object","properties":{"city":{"type":"string"}}}}]}"#,
        );

        // 流式请求同样带 tools 定义（翻译是体级的，与 stream 无关）。
        let seen = vendor.seen.lock().unwrap_or_else(|p| p.into_inner());
        let sent: Value = serde_json::from_slice(&seen[0].body).unwrap();
        assert_eq!(sent["tools"][0]["function"]["name"], "get_weather");
        drop(seen);

        let Reply::Stream {
            status,
            content_type,
            mut reader,
        } = reply
        else {
            panic!("stream reply")
        };
        assert_eq!(status, 200);
        assert_eq!(content_type, "text/event-stream");
        let mut out = String::new();
        reader.read_to_string(&mut out).expect("read sse");
        let events = parse_frames(&out);
        let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "response.created",
                "response.output_item.added", // message item
                "response.output_text.delta",
                "response.output_text.done", // 工具分片到达，正文先封口
                "response.output_item.done",
                "response.output_item.added", // function_call item
                "response.function_call_arguments.delta",
                "response.function_call_arguments.done",
                "response.output_item.done",
                "response.completed",
            ],
            "文本 item 先封口，再是完整 function_call 事件族"
        );
        // added：function_call 占位（arguments 空、status in_progress）。
        let added = &events[5].1["item"];
        assert_eq!(added["type"], "function_call");
        assert_eq!(added["status"], "in_progress");
        assert_eq!(added["call_id"], "call_77");
        assert_eq!(added["name"], "get_weather");
        assert_eq!(added["arguments"], "");
        // delta：拼装结果整段一帧。
        assert_eq!(events[6].1["delta"], r#"{"city":"Berlin"}"#);
        // done：完整 arguments。
        assert_eq!(events[7].1["arguments"], r#"{"city":"Berlin"}"#);
        assert_eq!(events[7].1["call_id"], "call_77");
        // item.done：完整 function_call item。
        let item = &events[8].1["item"];
        assert_eq!(item["type"], "function_call");
        assert_eq!(item["status"], "completed");
        assert_eq!(item["call_id"], "call_77");
        assert_eq!(item["name"], "get_weather");
        assert_eq!(item["arguments"], r#"{"city":"Berlin"}"#);
        // completed：output = [message, function_call]，usage 真数。
        let completed = &events[9].1["response"];
        assert_eq!(completed["status"], "completed");
        assert_eq!(completed["output"].as_array().unwrap().len(), 2);
        assert_eq!(completed["output"][0]["type"], "message");
        assert_eq!(completed["output"][1]["type"], "function_call");
        assert_eq!(completed["output"][1]["call_id"], "call_77");
        assert_eq!(completed["usage"]["input_tokens"], 11);
        assert_eq!(completed["usage"]["total_tokens"], 17);
    }

    #[test]
    fn translated_streaming_truncated_flushes_partial_call() {
        // 厂商半途断流：工具分片到了一半连接就没了（无 finish 无 [DONE]）。
        // 按已知的收尾（Go readSSE 结束后 finish() 同款）：created + 已拼到
        // 的 arguments（可能不完整）+ completed，usage 0。
        let sse = concat!(
            r#"data: {"id":"c3","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read","arguments":"{\"pa"}}]},"finish_reason":null}]}"#,
            "\n\n" // 没有 finish、没有 [DONE]——EOF 即收尾
        );
        let vendor = fake_vendor(200, "text/event-stream", sse);
        let (gateway, _sandbox) = sandbox(&format!(
            r#"{{"providers":[{{"id":"p","name":"P","key":"sk-test","chat":"{}/v1"}}]}}"#,
            vendor.base
        ));
        let reply = handle(&gateway, br#"{"model":"p/m","input":"go","stream":true}"#);
        let Reply::Stream { mut reader, .. } = reply else {
            panic!("stream reply")
        };
        let mut out = String::new();
        reader.read_to_string(&mut out).expect("read sse");
        let events = parse_frames(&out);
        let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "response.created",
                "response.output_item.added",
                "response.function_call_arguments.delta",
                "response.function_call_arguments.done",
                "response.output_item.done",
                "response.completed",
            ]
        );
        // arguments 按已收到的分片诚实发出（截断的 JSON 不修饰）。
        assert_eq!(events[2].1["delta"], r#"{"pa"#);
        assert_eq!(events[3].1["arguments"], r#"{"pa"#);
        assert_eq!(events[5].1["response"]["usage"]["input_tokens"], 0);
        assert_eq!(
            events[5].1["response"]["output"][0]["type"],
            "function_call"
        );
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
        assert_eq!(
            chat_vendor.hits.load(Ordering::SeqCst),
            0,
            "直通优先，chat 家不该被碰到"
        );
        let seen = responses_vendor
            .seen
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        assert_eq!(seen[0].path, "/v1/responses");
        assert_eq!(seen[0].authorization, "Bearer sk-resp");
        let sent: Value = serde_json::from_slice(&seen[0].body).unwrap();
        assert_eq!(sent["model"], "m2", "直通也剥 provider/ 前缀");

        // 应答原样（不折形状、不翻译）。
        let Reply::Json { status, body } = reply else {
            panic!("json reply")
        };
        assert_eq!(status, 200);
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["id"], "resp_vendor");
    }

    #[test]
    fn passthrough_native_responses_refusal_falls_back_to_chat() {
        // opencode-go 一类：responses 端点在、但模型不是原生 Responses；
        // codex 带 `reasoning.context` → 400，退回 chat 翻译。翻译路径不转发
        // `reasoning.context`，只保留可选 reasoning_effort。
        let responses_vendor = fake_vendor(
            400,
            "application/json",
            r#"{"type":"error","error":{"type":"invalid_request_error","message":"reasoning.context: this reasoning option requires a native Responses target"}}"#,
        );
        let chat_vendor = fake_vendor(
            200,
            "application/json",
            r#"{"id":"chatcmpl_ok","object":"chat.completion","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#,
        );
        let (gateway, _sandbox) = sandbox(&format!(
            r#"{{"providers":[{{"id":"o","name":"O","key":"k",
                "chat":"{0}/v1","responses":"{1}/v1"}}]}}"#,
            chat_vendor.base, responses_vendor.base
        ));
        let reply = handle(
            &gateway,
            br#"{"model":"o/m","input":"hi","reasoning":{"effort":"max","context":"all_turns"}}"#,
        );

        assert_eq!(responses_vendor.hits.load(Ordering::SeqCst), 1);
        assert_eq!(
            chat_vendor.hits.load(Ordering::SeqCst),
            1,
            "native Responses 拒绝后退回 chat"
        );
        let seen_chat = chat_vendor.seen.lock().unwrap_or_else(|p| p.into_inner());
        let chat_body = String::from_utf8_lossy(&seen_chat[0].body);
        assert!(
            !chat_body.contains("reasoning.context"),
            "翻译到 chat 不转发 reasoning.context: {chat_body}"
        );
        let Reply::Json { status, .. } = reply else {
            panic!("json reply")
        };
        assert_eq!(status, 200);
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
        let Reply::Stream {
            status,
            content_type,
            mut reader,
        } = reply
        else {
            panic!("stream reply")
        };
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
        let Reply::Json { status, body } = reply else {
            panic!("json reply")
        };
        assert_eq!(status, 404);
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert!(v["error"]["message"].as_str().unwrap().contains("nope/x"));
    }

    #[test]
    fn alias_group_member_effort_reaches_vendor_clean() {
        // 用户机上的真实形态：codex 别名组成员 `…:1017:max`。厂商必须收到裸
        // 模型 `m:1017`（tag 保留）与 reasoning_effort=max——最初 Rust 引擎把
        // `m:1017:max` 原样发出去，厂商回 model not found。
        let chat_vendor = fake_vendor(
            200,
            "application/json",
            r#"{"id":"c1","object":"chat.completion","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#,
        );
        let (gateway, _sandbox) = sandbox(&format!(
            r#"{{"providers":[{{"id":"o","name":"O","key":"k","chat":"{}/v1"}}],
                "groups":[{{"id":"gpt-6.1-terra","name":"gpt-6.1-terra","routing":"manual",
                  "members":["o/m:1017:max"],"pick":"o/m:1017:max"}}]}}"#,
            chat_vendor.base
        ));
        let request = super::super::httpd::Request {
            method: "POST".into(),
            path: "/v1/responses".into(),
            headers: vec![],
            body: br#"{"model":"gpt-6.1-terra","input":"hi","reasoning":{"effort":"low"}}"#
                .to_vec(),
        };
        let reply = super::super::routes::route(&request, &gateway);
        let Reply::Json { status, .. } = reply else {
            panic!("json reply")
        };
        assert_eq!(status, 200);
        let seen = chat_vendor.seen.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(seen.len(), 1);
        let sent: Value = serde_json::from_slice(&seen[0].body).unwrap();
        assert_eq!(sent["model"], "m:1017", "强度后缀不能进厂商模型名");
        assert_eq!(
            sent["reasoning_effort"], "max",
            "成员固定强度要覆盖 agent 的 low"
        );
    }

    #[test]
    fn missing_responses_endpoint_falls_back_to_chat_translation() {
        // 预设给了 responses 端点、厂商其实没有（404）：codex 不能被晾住，
        // 退回 chat 翻译路径。
        let dead_responses = fake_vendor(404, "application/json", r#"{"error":"not found"}"#);
        let chat_vendor = fake_vendor(
            200,
            "application/json",
            r#"{"id":"c1","object":"chat.completion","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#,
        );
        let (gateway, _sandbox) = sandbox(&format!(
            r#"{{"providers":[{{"id":"o","name":"O","key":"k","chat":"{}/v1","responses":"{}/v1"}}]}}"#,
            chat_vendor.base, dead_responses.base
        ));
        let reply = handle(&gateway, br#"{"model":"o/m","input":"hi"}"#);
        let Reply::Json { status, body } = reply else {
            panic!("json reply")
        };
        assert_eq!(status, 200);
        assert_eq!(dead_responses.hits.load(Ordering::SeqCst), 1);
        assert_eq!(chat_vendor.hits.load(Ordering::SeqCst), 1);
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["object"], "response");
    }

    #[test]
    fn chat_reasoning_content_becomes_reasoning_summary_events() {
        // DeepSeek 的 delta.reasoning_content 必须变成 responses 的 reasoning
        // summary 事件族，codex 才有思考块/轨迹；之前被整段丢掉。
        let sse = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think A\"}}]}

",
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\" B\"}}]}

",
            "data: {\"choices\":[{\"delta\":{\"content\":\"answer\"}}]}

",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}

",
            "data: [DONE]

"
        );
        let mut r = ResponsesSse::new(Box::new(std::io::Cursor::new(sse.as_bytes().to_vec())), "m");
        let mut out = String::new();
        r.read_to_string(&mut out).unwrap();
        assert!(
            out.contains("response.reasoning_summary_text.delta"),
            "{out}"
        );
        assert!(out.contains("\"delta\":\"think A\""));
        let done = out
            .find("response.reasoning_summary_text.done")
            .expect("done");
        let text = out.find("response.output_text.delta").expect("text");
        assert!(done < text, "思考先封口再出正文");
        assert!(out.contains("\"text\":\"think A B\""));
        // completed 的 output：reasoning item 在 message item 前
        let completed = out.find("event: response.completed").unwrap();
        let tail = &out[completed..];
        assert!(
            tail.find("\"type\":\"reasoning\"").unwrap()
                < tail.find("\"type\":\"message\"").unwrap()
        );
    }

    #[test]
    fn nonstream_reasoning_content_becomes_reasoning_item() {
        let v: Value = serde_json::from_str(r#"{"choices":[{"message":{"role":"assistant","reasoning_content":"why","content":"ok"},"finish_reason":"stop"}]}"#).unwrap();
        let r = chat_completion_to_response(&v, "m", &super::super::translate::ToolTable::new());
        assert_eq!(r["output"][0]["type"], "reasoning");
        assert_eq!(r["output"][0]["summary"][0]["text"], "why");
        assert_eq!(r["output"][1]["type"], "message");
    }

    // ---- Codex 非 function 工具的应答还原（端到端：routes::route + 假 chat 厂商）----

    const CODEX_TOOLS: &str = r#"[
        {"type":"function","name":"shell","parameters":{"type":"object"}},
        {"type":"custom","name":"apply_patch","description":"edit"},
        {"type":"namespace","name":"mcp__srv__","tools":[
            {"type":"function","name":"do_it","parameters":{"type":"object"}}]},
        {"type":"tool_search","execution":"client","description":"s","parameters":{"type":"object"}}]"#;

    fn route_responses(gateway: &Gateway, body: String) -> Reply {
        let request = super::super::httpd::Request {
            method: "POST".into(),
            path: "/v1/responses".into(),
            headers: vec![],
            body: body.into_bytes(),
        };
        super::super::routes::route(&request, gateway)
    }

    fn one_chat_vendor(vendor: &FakeVendor) -> String {
        format!(
            r#"{{"providers":[{{"id":"p","name":"P","key":"k","chat":"{}/v1"}}]}}"#,
            vendor.base
        )
    }

    #[test]
    fn nonstream_tool_calls_are_restored_to_codex_item_kinds() {
        // 厂商（chat-only）按 flat 名调了：custom / 命名空间 / 普通 / tool_search 各一
        let vendor = fake_vendor(
            200,
            "application/json",
            r#"{"id":"c1","object":"chat.completion","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":null,"tool_calls":[
                {"id":"call_a","type":"function","function":{"name":"apply_patch","arguments":"{\"input\":\"*** Begin Patch\\n*** End Patch\"}"}},
                {"id":"call_b","type":"function","function":{"name":"mcp__srv____do_it","arguments":"{\"x\":1}"}},
                {"id":"call_c","type":"function","function":{"name":"shell","arguments":"{\"cmd\":\"ls\"}"}},
                {"id":"call_d","type":"function","function":{"name":"tool_search","arguments":"{\"query\":\"cal\"}"}}]},
                "finish_reason":"tool_calls"}],
                "usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}"#,
        );
        let (gateway, _sandbox) = sandbox(&one_chat_vendor(&vendor));
        let reply = route_responses(
            &gateway,
            format!(r#"{{"model":"p/m","input":"do things","tools":{CODEX_TOOLS}}}"#),
        );

        // 厂商看到的是压平后的函数集（custom 带 input 串 schema）
        let seen = vendor.seen.lock().unwrap_or_else(|p| p.into_inner());
        let sent: Value = serde_json::from_slice(&seen[0].body).unwrap();
        let names: Vec<&str> = sent["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(
            names,
            vec!["shell", "apply_patch", "mcp__srv____do_it", "tool_search"]
        );
        assert_eq!(
            sent["tools"][1]["function"]["parameters"]["required"],
            json!(["input"])
        );
        drop(seen);

        let Reply::Json { status, body } = reply else {
            panic!("json reply")
        };
        assert_eq!(status, 200);
        let v: Value = serde_json::from_slice(&body).unwrap();
        let out = v["output"].as_array().unwrap();
        assert_eq!(out.len(), 4);
        // custom -> custom_tool_call + input（无 arguments），id 前缀 ctc_
        assert_eq!(out[0]["type"], "custom_tool_call");
        assert_eq!(out[0]["name"], "apply_patch");
        assert_eq!(out[0]["input"], "*** Begin Patch\n*** End Patch");
        assert_eq!(out[0]["call_id"], "call_a");
        assert!(out[0].get("arguments").is_none());
        assert!(out[0]["id"].as_str().unwrap().starts_with("ctc_"));
        // 命名空间 -> function_call + 原名 + namespace
        assert_eq!(out[1]["type"], "function_call");
        assert_eq!(out[1]["name"], "do_it");
        assert_eq!(out[1]["namespace"], "mcp__srv__");
        assert_eq!(out[1]["arguments"], "{\"x\":1}");
        // 普通 -> 原样
        assert_eq!(out[2]["type"], "function_call");
        assert_eq!(out[2]["name"], "shell");
        assert!(out[2].get("namespace").is_none());
        assert!(out[2]["id"].as_str().unwrap().starts_with("fc_"));
        // tool_search -> tool_search_call（client），arguments 对象，tsc_
        assert_eq!(out[3]["type"], "tool_search_call");
        assert_eq!(out[3]["execution"], "client");
        assert_eq!(out[3]["arguments"], json!({"query": "cal"}));
        assert!(out[3]["id"].as_str().unwrap().starts_with("tsc_"));
    }

    #[test]
    fn streaming_tool_calls_are_restored_to_codex_item_kinds() {
        // custom 的 arguments 跨帧；随后一个命名空间调用
        let sse = concat!(
            r#"data: {"id":"s1","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"apply_patch","arguments":"{\"input\":\"*** Be"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"s1","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"gin\\n\"}"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"s1","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"tool_calls":[{"index":1,"id":"call_b","type":"function","function":{"name":"mcp__srv____do_it","arguments":"{\"x\":1}"}}]},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"s1","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":4,"completion_tokens":3,"total_tokens":7}}"#,
            "\n\n",
            "data: [DONE]\n\n"
        );
        let vendor = fake_vendor(200, "text/event-stream", sse);
        let (gateway, _sandbox) = sandbox(&one_chat_vendor(&vendor));
        let reply = route_responses(
            &gateway,
            format!(r#"{{"model":"p/m","input":"x","stream":true,"tools":{CODEX_TOOLS}}}"#),
        );
        let Reply::Stream { mut reader, .. } = reply else {
            panic!("stream reply")
        };
        let mut out = String::new();
        reader.read_to_string(&mut out).expect("read sse");
        let events = parse_frames(&out);
        let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "response.created",
                // custom：added -> input.delta -> input.done -> item.done
                "response.output_item.added",
                "response.custom_tool_call_input.delta",
                "response.custom_tool_call_input.done",
                "response.output_item.done",
                // 命名空间：普通 function_call 事件族
                "response.output_item.added",
                "response.function_call_arguments.delta",
                "response.function_call_arguments.done",
                "response.output_item.done",
                "response.completed",
            ]
        );
        let added = &events[1].1["item"];
        assert_eq!(added["type"], "custom_tool_call");
        assert_eq!(added["name"], "apply_patch");
        assert_eq!(added["input"], "");
        assert_eq!(added["status"], "in_progress");
        assert!(added["id"].as_str().unwrap().starts_with("ctc_"));
        assert_eq!(events[2].1["delta"], "*** Begin\n");
        assert_eq!(events[3].1["input"], "*** Begin\n");
        assert_eq!(events[2].1["item_id"], added["id"]);
        let done = &events[4].1["item"];
        assert_eq!(done["type"], "custom_tool_call");
        assert_eq!(done["input"], "*** Begin\n");
        assert_eq!(done["call_id"], "call_a");
        assert!(done.get("arguments").is_none());
        // 命名空间调用
        let ns_added = &events[5].1["item"];
        assert_eq!(ns_added["type"], "function_call");
        assert_eq!(ns_added["name"], "do_it");
        assert_eq!(ns_added["namespace"], "mcp__srv__");
        assert_eq!(events[7].1["arguments"], "{\"x\":1}");
        assert_eq!(events[7].1["name"], "do_it");
        let ns_done = &events[8].1["item"];
        assert_eq!(
            (
                ns_done["name"].as_str().unwrap(),
                ns_done["namespace"].as_str().unwrap()
            ),
            ("do_it", "mcp__srv__")
        );
        // completed 的 output 与逐项 done 一致
        let completed = &events[9].1["response"];
        assert_eq!(completed["output"][0]["type"], "custom_tool_call");
        assert_eq!(completed["output"][1]["namespace"], "mcp__srv__");
        assert_eq!(completed["usage"]["input_tokens"], 4);
    }

    #[test]
    fn plain_function_tool_roundtrip_is_unchanged() {
        // 回归：只有普通 function 工具的请求，请求与应答形状与改动前一致
        let vendor = fake_vendor(
            200,
            "application/json",
            r#"{"id":"c1","object":"chat.completion","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"","tool_calls":[
                {"id":"call_1","type":"function","function":{"name":"get_weather","arguments":"{\"city\":\"Berlin\"}"}}]},
                "finish_reason":"tool_calls"}]}"#,
        );
        let (gateway, _sandbox) = sandbox(&one_chat_vendor(&vendor));
        let reply = route_responses(
            &gateway,
            r#"{"model":"p/m","input":"w?","tools":[{"type":"function","name":"get_weather","description":"w","parameters":{"type":"object"}}]}"#.into(),
        );
        let seen = vendor.seen.lock().unwrap_or_else(|p| p.into_inner());
        let sent: Value = serde_json::from_slice(&seen[0].body).unwrap();
        assert_eq!(sent["tools"][0]["function"]["name"], "get_weather");
        assert!(sent.get("parallel_tool_calls").is_none());
        drop(seen);
        let Reply::Json { body, .. } = reply else {
            panic!("json reply")
        };
        let v: Value = serde_json::from_slice(&body).unwrap();
        let item = &v["output"][0];
        assert_eq!(item["type"], "function_call");
        assert_eq!(item["name"], "get_weather");
        assert_eq!(item["arguments"], "{\"city\":\"Berlin\"}");
        assert_eq!(item["call_id"], "call_1");
        assert_eq!(item["status"], "completed");
        assert!(item.get("namespace").is_none() && item.get("input").is_none());
        assert!(item["id"].as_str().unwrap().starts_with("fc_"));
    }

    #[test]
    fn history_with_namespaced_and_custom_calls_reaches_vendor_with_flat_names() {
        let vendor = fake_vendor(
            200,
            "application/json",
            r#"{"id":"c1","object":"chat.completion","model":"m","choices":[{"index":0,"message":{"role":"assistant","content":"done"},"finish_reason":"stop"}]}"#,
        );
        let (gateway, _sandbox) = sandbox(&one_chat_vendor(&vendor));
        let body = format!(
            r#"{{"model":"p/m","tools":{CODEX_TOOLS},"input":[
                {{"type":"message","role":"user","content":"go"}},
                {{"type":"function_call","call_id":"c1","namespace":"mcp__srv__","name":"do_it","arguments":"{{}}"}},
                {{"type":"function_call_output","call_id":"c1","output":"ok"}},
                {{"type":"custom_tool_call","call_id":"c2","name":"apply_patch","input":"patch"}},
                {{"type":"custom_tool_call_output","call_id":"c2","output":"applied"}}]}}"#
        );
        let reply = route_responses(&gateway, body);
        let Reply::Json { status, .. } = reply else {
            panic!("json reply")
        };
        assert_eq!(status, 200);
        let seen = vendor.seen.lock().unwrap_or_else(|p| p.into_inner());
        let sent: Value = serde_json::from_slice(&seen[0].body).unwrap();
        let s = sent["messages"].to_string();
        assert!(s.contains("mcp__srv____do_it"), "{s}");
        assert!(
            s.contains(r#"\"input\":\"patch\""#) || s.contains("input"),
            "{s}"
        );
        // 每个 tool_call 都有对应的 tool 消息
        let msgs = sent["messages"].as_array().unwrap();
        assert_eq!(msgs.iter().filter(|m| m["role"] == "tool").count(), 2);
    }

    // -----------------------------------------------------------------
    // previous_response_id（prev_response.rs 的翻译路径接线）
    // -----------------------------------------------------------------

    const PREV_CHAT_REPLY: &str = r#"{"id":"chatcmpl-1","object":"chat.completion","model":"m",
        "choices":[{"index":0,"message":{"role":"assistant","content":"hello there"},"finish_reason":"stop"}],
        "usage":{"prompt_tokens":7,"completion_tokens":3,"total_tokens":10}}"#;

    #[test]
    fn prev_response_store_replay_and_miss() {
        let vendor = fake_vendor(200, "application/json", PREV_CHAT_REPLY);
        let (gateway, _sandbox) = sandbox(&format!(
            r#"{{"providers":[{{"id":"p","name":"P","key":"k","chat":"{}/v1"}}]}}"#,
            vendor.base
        ));

        // 第一轮：store:true、无 previous_response_id——存（input=本轮、
        // output=应答 output），应答 id 是网关生成的存账 id。
        let reply = handle(&gateway, br#"{"model":"p/m","input":"first","store":true}"#);
        let Reply::Json { status, body } = reply else {
            panic!("json reply")
        };
        assert_eq!(status, 200);
        let v: Value = serde_json::from_slice(&body).unwrap();
        let id1 = v["id"].as_str().unwrap().to_string();
        assert!(id1.starts_with("resp_"), "存账 id：{id1}");

        // 第二轮：只发新 input + previous_response_id——厂商必须收到整段
        // 历史（存的 input 在前、存的 output 作 assistant 回合、新 input 在后）。
        let body = format!(
            r#"{{"model":"p/m","input":"second","store":true,"previous_response_id":"{id1}"}}"#
        );
        let reply = handle(&gateway, body.as_bytes());
        let Reply::Json { status, body } = reply else {
            panic!("json reply")
        };
        assert_eq!(status, 200);
        let v: Value = serde_json::from_slice(&body).unwrap();
        let id2 = v["id"].as_str().unwrap().to_string();
        assert!(id2.starts_with("resp_"));
        assert_ne!(id1, id2, "每轮各有各的存账 id");
        let seen = vendor.seen.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(seen.len(), 2);
        let sent: Value = serde_json::from_slice(&seen[1].body).unwrap();
        assert!(
            sent.get("previous_response_id").is_none(),
            "合并后的请求不再点名 previous_response_id"
        );
        let msgs = sent["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3, "合并出的整段历史：{msgs:?}");
        assert_eq!(msgs[0], json!({"role": "user", "content": "first"}));
        assert_eq!(
            msgs[1],
            json!({"role": "assistant", "content": "hello there"})
        );
        assert_eq!(msgs[2], json!({"role": "user", "content": "second"}));
        drop(seen);

        // miss：网关不持有的 id → 显式 400（OpenAI 形状 code=
        // previous_response_not_found），绝不静默重放空历史——厂商收不到
        // 第三个请求。
        let reply = handle(
            &gateway,
            br#"{"model":"p/m","input":"third","previous_response_id":"resp_never"}"#,
        );
        let Reply::Json { status, body } = reply else {
            panic!("json reply")
        };
        assert_eq!(status, 400, "Go prevLost 是 400，不是 404");
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "previous_response_not_found");
        assert_eq!(v["error"]["param"], "previous_response_id");
        assert_eq!(vendor.hits.load(Ordering::SeqCst), 2, "miss 不该打到厂商");

        // store:false 的回合不存（Go previousResponse 的 early return）：
        // 它照常应答，但不产生可点名的存账。
        let reply = handle(
            &gateway,
            br#"{"model":"p/m","input":"no keep","store":false}"#,
        );
        let Reply::Json { status, .. } = reply else {
            panic!("json reply")
        };
        assert_eq!(status, 200);
        // 点名一个网关从没存过的 id（包括 store:false 轮的占位 id）都 miss。
        let reply = handle(
            &gateway,
            br#"{"model":"p/m","input":"again","previous_response_id":"resp_nokeep"}"#,
        );
        let Reply::Json { status, .. } = reply else {
            panic!("json reply")
        };
        assert_eq!(status, 400);
    }

    #[test]
    fn prev_response_stream_store_roundtrip() {
        // 流式存回合：created/completed 帧里的 id 就是存账 id；下一轮点名它，
        // 厂商收到整段历史（流式轮的 output 已并进 assistant 回合）。
        let sse = concat!(
            r#"data: {"id":"c1","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"content":"你好"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c1","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":2,"total_tokens":7}}"#,
            "\n\n",
            "data: [DONE]\n\n"
        );
        let vendor = fake_vendor(200, "text/event-stream", sse);
        let (gateway, _sandbox) = sandbox(&format!(
            r#"{{"providers":[{{"id":"p","name":"P","key":"k","chat":"{}/v1"}}]}}"#,
            vendor.base
        ));
        let reply = handle(
            &gateway,
            br#"{"model":"p/m","input":"first","store":true,"stream":true}"#,
        );
        let Reply::Stream { mut reader, .. } = reply else {
            panic!("stream reply")
        };
        let mut out = String::new();
        reader.read_to_string(&mut out).expect("read sse");
        let events = parse_frames(&out);
        let created_id = events
            .iter()
            .find(|(n, _)| n == "response.created")
            .map(|(_, d)| d["response"]["id"].as_str().unwrap().to_string())
            .expect("created frame");
        let completed_id = events
            .iter()
            .find(|(n, _)| n == "response.completed")
            .map(|(_, d)| d["response"]["id"].as_str().unwrap().to_string())
            .expect("completed frame");
        assert_eq!(
            created_id, completed_id,
            "流式存账：帧里的 id 与存账 id 是同一个"
        );
        assert!(created_id.starts_with("resp_"));

        // 下一轮点名它——存账表在 Gateway 里，同一网关才命中；vendor 回的
        // 固定 SSE 与请求是否流式无关，第二轮同样走流式。
        let body = format!(
            r#"{{"model":"p/m","input":"second","stream":true,"previous_response_id":"{created_id}"}}"#
        );
        let reply = handle(&gateway, body.as_bytes());
        let Reply::Stream {
            status, mut reader, ..
        } = reply
        else {
            panic!("stream reply")
        };
        assert_eq!(status, 200);
        let mut out2 = String::new();
        reader.read_to_string(&mut out2).expect("read sse");
        assert!(out2.contains("event: response.completed"));
        let seen = vendor.seen.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(seen.len(), 2);
        let sent: Value = serde_json::from_slice(&seen[1].body).unwrap();
        let msgs = sent["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 3, "流式轮的历史并进来了：{msgs:?}");
        assert_eq!(msgs[0], json!({"role": "user", "content": "first"}));
        assert_eq!(msgs[1], json!({"role": "assistant", "content": "你好"}));
        assert_eq!(msgs[2], json!({"role": "user", "content": "second"}));
    }

    #[test]
    fn prev_response_passthrough_untouched() {
        // 直通路径不接管 store：请求（含 previous_response_id）原样透传，
        // 应答原样（id 是厂商的），也不进网关存账表。
        let vendor = fake_vendor(
            200,
            "application/json",
            r#"{"id":"resp_vendor","object":"response","status":"completed","output":[]}"#,
        );
        let (gateway, _sandbox) = sandbox(&format!(
            r#"{{"providers":[{{"id":"o","name":"O","key":"k","responses":"{}/v1","chat":"{}/v1"}}]}}"#,
            vendor.base, vendor.base
        ));
        let reply = handle(
            &gateway,
            br#"{"model":"o/m","input":"hi","store":true,"previous_response_id":"resp_anything"}"#,
        );
        let Reply::Json { status, body } = reply else {
            panic!("json reply")
        };
        assert_eq!(status, 200);
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["id"], "resp_vendor", "厂商的 id 原样，不换存账 id");
        let seen = vendor.seen.lock().unwrap_or_else(|p| p.into_inner());
        let sent: Value = serde_json::from_slice(&seen[0].body).unwrap();
        assert_eq!(
            sent["previous_response_id"], "resp_anything",
            "直通透传原样"
        );
        assert_eq!(sent["store"], true);
        // 网关没存：点名厂商那个 id（网关表里没有）——翻译路径才查表，
        // 这里没有翻译路径可走，只验证存账表没吞直通应答。
    }

    // -----------------------------------------------------------------
    // textcalls（正文里书写的工具调用，翻译路径应答侧）
    // -----------------------------------------------------------------

    const TEXTCALL_TOOLS: &str = r#"[{"type":"function","name":"get_weather","description":"weather",
        "parameters":{"type":"object","properties":{"city":{"type":"string"}},"required":["city"]}}]"#;

    /// DeepSeek/Hermes 形：正文里写 `<tool_call>{"name":…,"arguments":…}</tool_call>`。
    const TEXTCALL_BODY: &str = concat!(
        "查一下天气。",
        "<tool_call>{\"name\":\"get_weather\",\"arguments\":{\"city\":\"Berlin\"}}</tool_call>"
    );

    #[test]
    fn textcalls_nonstream_written_call_becomes_function_call() {
        let reply_body = format!(
            r#"{{"id":"c1","object":"chat.completion","model":"m","choices":[{{"index":0,"message":{{"role":"assistant","content":{}}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}}}"#,
            serde_json::to_string(&TEXTCALL_BODY).unwrap()
        );
        // fake_vendor 要 &'static str——leak 一份（测试进程生命周期）。
        let reply_body: &'static str = Box::leak(reply_body.into_boxed_str());
        let vendor = fake_vendor(200, "application/json", reply_body);
        let (gateway, _sandbox) = sandbox(&one_chat_vendor(&vendor));
        let reply = route_responses(
            &gateway,
            format!(r#"{{"model":"p/m","input":"weather?","tools":{TEXTCALL_TOOLS}}}"#),
        );
        let Reply::Json { status, body } = reply else {
            panic!("json reply")
        };
        assert_eq!(status, 200);
        let v: Value = serde_json::from_slice(&body).unwrap();
        let out = v["output"].as_array().unwrap();
        assert_eq!(out.len(), 2, "正文 item + function_call item：{out:?}");
        // 标记前的正文原样放出，标记本身不再出现在文本里。
        assert_eq!(out[0]["type"], "message");
        assert_eq!(out[0]["content"][0]["text"], "查一下天气。");
        // 书写调用 → 真 function_call。
        assert_eq!(out[1]["type"], "function_call");
        assert_eq!(out[1]["name"], "get_weather");
        assert_eq!(out[1]["arguments"], r#"{"city":"Berlin"}"#);
        assert!(
            out[1]["call_id"]
                .as_str()
                .unwrap()
                .starts_with("call_text_"),
            "Go textCallID 形状：{}",
            out[1]["call_id"]
        );
        assert_eq!(v["status"], "completed");
    }

    #[test]
    fn textcalls_stream_written_call_becomes_function_call_family() {
        // 标记跨 chunk：正文分三片到（前半 / 标记开 / 标记闭），持有不发。
        let sse = concat!(
            r#"data: {"id":"c9","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"content":"查一下"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c9","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"content":"天气。<tool_call>{\"name\":\"get_weather\",\"argu"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c9","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"content":"ments\":{\"city\":\"Berlin\"}}</tool_call>"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c9","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":4,"total_tokens":9}}"#,
            "\n\n",
            "data: [DONE]\n\n"
        );
        let vendor = fake_vendor(200, "text/event-stream", sse);
        let (gateway, _sandbox) = sandbox(&one_chat_vendor(&vendor));
        let reply = route_responses(
            &gateway,
            format!(
                r#"{{"model":"p/m","input":"weather?","stream":true,"tools":{TEXTCALL_TOOLS}}}"#
            ),
        );
        let Reply::Stream { mut reader, .. } = reply else {
            panic!("stream reply")
        };
        let mut out = String::new();
        reader.read_to_string(&mut out).expect("read sse");
        let events = parse_frames(&out);
        let names: Vec<&str> = events.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "response.created",
                "response.output_item.added", // message item
                "response.output_text.delta", // "查一下"
                "response.output_text.delta", // "天气。"
                "response.output_text.done",  // 全文（不含标记）
                "response.output_item.done",
                "response.output_item.added", // function_call item
                "response.function_call_arguments.delta",
                "response.function_call_arguments.done",
                "response.output_item.done",
                "response.completed",
            ],
            "标记前正文照发、标记起的持有变 function_call 事件族"
        );
        assert_eq!(events[2].1["delta"], "查一下");
        assert_eq!(events[3].1["delta"], "天气。");
        assert_eq!(events[4].1["text"], "查一下天气。");
        let added = &events[6].1["item"];
        assert_eq!(added["type"], "function_call");
        assert_eq!(added["name"], "get_weather");
        assert_eq!(events[7].1["delta"], r#"{"city":"Berlin"}"#);
        let done_item = &events[9].1["item"];
        assert_eq!(done_item["arguments"], r#"{"city":"Berlin"}"#);
        assert!(done_item["call_id"]
            .as_str()
            .unwrap()
            .starts_with("call_text_"));
        // held 的标记正文绝不当 delta 发出去。
        assert!(!out.contains("tool_call>"), "持有文本不进 delta：{out}");
        let completed = &events[10].1["response"];
        assert_eq!(completed["output"].as_array().unwrap().len(), 2);
        assert_eq!(completed["output"][1]["type"], "function_call");
    }

    #[test]
    fn textcalls_without_offered_tools_stays_text() {
        // 请求没提供工具：scout 不装（Go 在安装点整个跳过），正文原样。
        let reply_body = format!(
            r#"{{"id":"c1","object":"chat.completion","model":"m","choices":[{{"index":0,"message":{{"role":"assistant","content":{}}},"finish_reason":"stop"}}]}}"#,
            serde_json::to_string(&TEXTCALL_BODY).unwrap()
        );
        let reply_body: &'static str = Box::leak(reply_body.into_boxed_str());
        let vendor = fake_vendor(200, "application/json", reply_body);
        let (gateway, _sandbox) = sandbox(&one_chat_vendor(&vendor));
        let reply = route_responses(&gateway, r#"{"model":"p/m","input":"weather?"}"#.into());
        let Reply::Json { status, body } = reply else {
            panic!("json reply")
        };
        assert_eq!(status, 200);
        let v: Value = serde_json::from_slice(&body).unwrap();
        let out = v["output"].as_array().unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["type"], "message");
        assert_eq!(
            out[0]["content"][0]["text"], TEXTCALL_BODY,
            "正文一个字不动"
        );
    }

    #[test]
    fn textcalls_unoffered_name_stays_text() {
        // 标记点了没提供的工具名：整段按原文放出（不收成截短的调用）。
        let text = "看这个。<tool_call>{\"name\":\"rm_rf\",\"arguments\":{}}</tool_call>";
        let reply_body = format!(
            r#"{{"id":"c1","object":"chat.completion","model":"m","choices":[{{"index":0,"message":{{"role":"assistant","content":{}}},"finish_reason":"stop"}}]}}"#,
            serde_json::to_string(text).unwrap()
        );
        let reply_body: &'static str = Box::leak(reply_body.into_boxed_str());
        let vendor = fake_vendor(200, "application/json", reply_body);
        let (gateway, _sandbox) = sandbox(&one_chat_vendor(&vendor));
        let reply = route_responses(
            &gateway,
            format!(r#"{{"model":"p/m","input":"x","tools":{TEXTCALL_TOOLS}}}"#),
        );
        let Reply::Json { status, body } = reply else {
            panic!("json reply")
        };
        assert_eq!(status, 200);
        let v: Value = serde_json::from_slice(&body).unwrap();
        let out = v["output"].as_array().unwrap();
        assert_eq!(out.len(), 1, "没读出调用：{out:?}");
        assert_eq!(out[0]["content"][0]["text"], text);
    }

    // -----------------------------------------------------------------
    // usage 记账（capture 层，Go sniff.go 的 responses 方言口径）
    // -----------------------------------------------------------------

    /// 翻译路径流式回合落账：completed 帧的 usage 四桶——responses 的
    /// input_tokens 含缓存 → 减两桶得净额；prompt_cache_key 种子随行。
    #[test]
    fn translated_stream_turn_records_usage_buckets() {
        let sse = concat!(
            r#"data: {"id":"c1","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{"content":"你好"},"finish_reason":null}]}"#,
            "\n\n",
            r#"data: {"id":"c1","object":"chat.completion.chunk","model":"m","choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1000,"completion_tokens":40,"total_tokens":1040,"prompt_tokens_details":{"cached_tokens":300,"cache_write_tokens":100}}}"#,
            "\n\n",
            "data: [DONE]\n\n"
        );
        let vendor = fake_vendor(200, "text/event-stream", sse);
        let (gateway, _sandbox) = sandbox(&format!(
            r#"{{"providers":[{{"id":"p","name":"P","key":"k","chat":"{}/v1"}}]}}"#,
            vendor.base
        ));
        let reply = handle(
            &gateway,
            br#"{"model":"p/m","input":"hi","stream":true,"prompt_cache_key":"sess-r"}"#,
        );
        let Reply::Stream { mut reader, .. } = reply else {
            panic!("stream reply")
        };
        let mut out = String::new();
        reader.read_to_string(&mut out).expect("read sse");
        assert!(out.contains("event: response.completed"));
        let rows = gateway.usage.records_since(0);
        assert_eq!(rows.len(), 1, "一条回合一行");
        let r = &rows[0];
        assert_eq!(r.agent, "responses");
        assert_eq!(r.provider, "p");
        assert_eq!(r.model, "p/m");
        assert_eq!(
            r.session.as_deref(),
            Some("sess-r"),
            "body 侧种子（prompt_cache_key）"
        );
        assert_eq!(
            r.in_tokens,
            Some(600),
            "completed 的 input_tokens 含缓存：1000 − 300 − 100"
        );
        assert_eq!(r.out_tokens, Some(40));
        assert_eq!(r.cache_read, Some(300));
        assert_eq!(r.cache_write, Some(100));
        assert!(r.millis > 0);
    }

    /// 翻译路径非流式：折好的 response 对象整块扫描；chat 厂商没说的桶
    /// （usage_to_responses 补的 0）不进账。
    #[test]
    fn translated_non_stream_turn_records_usage() {
        let vendor = fake_vendor(
            200,
            "application/json",
            r#"{"id":"c1","object":"chat.completion","model":"m",
                "choices":[{"index":0,"message":{"role":"assistant","content":"ok"},"finish_reason":"stop"}],
                "usage":{"prompt_tokens":700,"completion_tokens":12,"total_tokens":712,
                    "prompt_tokens_details":{"cached_tokens":250}}}"#,
        );
        let (gateway, _sandbox) = sandbox(&one_chat_vendor(&vendor));
        let reply = handle(&gateway, br#"{"model":"p/m","input":"hi"}"#);
        assert!(matches!(reply, Reply::Json { status: 200, .. }));
        let rows = gateway.usage.records_since(0);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.agent, "responses");
        assert_eq!(r.in_tokens, Some(450), "700 − 250 缓存读");
        assert_eq!(r.out_tokens, Some(12));
        assert_eq!(r.cache_read, Some(250));
        assert!(r.cache_write.is_none(), "厂商没说缓存写：补的 0 不编成实测");
        assert!(r.session.is_none(), "请求没带种子");
    }
}
