//! 路由（合同 C2/C3/C4/C5 的 HTTP 面）。
use serde_json::Value;

use super::httpd::{Reply, Request};

/// `/v1/models` 行构造（model_bridge.rs parse_models_catalog_rows 读的
/// 字段一个都不能少：id / context_window / max_output_tokens /
/// modalities.input）。
fn model_row(provider_id: &str, row: &super::catalog::VendorRow) -> Value {
    let mut object = serde_json::json!({
        "id": format!("{}/{}", provider_id, row.id),
        "object": "model",
        "type": "model",
        "created": 0,
        "created_at": "2025-01-01T00:00:00Z",
        "owned_by": provider_id,
        "display_name": row.raw.get("display_name").cloned().unwrap_or(Value::String(row.id.clone())),
        "context_window": row.context_window,
        "context_length": row.context_window,
        "max_input_tokens": row.context_window,
        "max_output_tokens": row.max_output,
    });
    // 厂商原行里的 reasoning_levels 等描述字段透传（不在 object 里的才补）。
    if let (Some(dst), Some(src)) = (object.as_object_mut(), row.raw.as_object()) {
        for (k, v) in src {
            if !dst.contains_key(k) && k != "id" {
                dst.insert(k.clone(), v.clone());
            }
        }
    }
    let mut input = vec!["text"];
    if row.images && !input.contains(&"image") {
        input.push("image");
    }
    object
        .as_object_mut()
        .expect("object literal")
        .insert("modalities".into(), serde_json::json!({ "input": input, "output": ["text"] }));
    object
}

// 成员强度后缀（`…:max`）的请求改写不在这里做：失败转移让一个请求可能依次
// 打几个成员，每个成员带自己的固定强度——chat/responses/messages 处理器按候选
// 各调 `groups::body_for`（Go `withEffort` 同语义，模型后缀由 groups 剥掉）。

/// 全部来源的目录行（来源 id, 行）：`/v1/models` 与 `/v1/codex/models` 共用。
/// 先拍一份来源快照再放锁：目录要联网抓厂商，持 store 锁等网络会让所有 chat
/// 请求（resolve 要同一把锁）陪着等（2026-10-31 storage 锁事故同款）。
fn catalog_rows(gateway: &super::Gateway) -> Result<Vec<(String, super::catalog::VendorRow)>, Reply> {
    let providers: Vec<super::store::Provider> = match gateway.store.lock() {
        Ok(s) => s.providers().to_vec(),
        Err(_) => return Err(Reply::error(500, "store poisoned")),
    };
    // Codex 订阅账号的 codex/* 行与其它来源并发抓（自带 10s 超时 +
    // 失败退避，慢/挂都不拖别人）。
    let (mut rows, codex_rows, grok_rows, claude_rows) = std::thread::scope(|scope| {
        let codex = scope.spawn(|| super::codex_account::listed_rows(&providers));
        let grok = scope.spawn(|| super::grok_account::listed_rows(&providers));
        let claude = scope.spawn(|| super::claude_account::listed_rows(&providers));
        gateway.catalog.prewarm(&providers);
        let mut rows = Vec::new();
        for provider in &providers {
            for row in gateway.catalog.listed(provider) {
                rows.push((provider.id.clone(), row));
            }
        }
        (rows, codex.join().unwrap_or_default(), grok.join().unwrap_or_default(), claude.join().unwrap_or_default())
    });
    for row in codex_rows {
        rows.push((super::codex_account::CODEX_ID.to_string(), row));
    }
    for row in grok_rows {
        rows.push((super::grok_account::GROK_ID.to_string(), row));
    }
    for row in claude_rows {
        rows.push((super::claude_account::CLAUDE_ID.to_string(), row));
    }
    Ok(rows)
}

pub fn route(request: &Request, gateway: &super::Gateway) -> Reply {
    // CLI/外部进程写的 providers.json 立即可见（magpie heldFresh 语义）。
    if let Ok(mut store) = gateway.store.lock() {
        store.reload();
    }
    let path = request.path.split('?').next().unwrap_or("");
    match (request.method.as_str(), path) {
        ("GET", "/v1/models") => {
            let rows = match catalog_rows(gateway) {
                Ok(r) => r,
                Err(reply) => return reply,
            };
            let data: Vec<Value> = rows.iter().map(|(pid, row)| model_row(pid, row)).collect();
            Reply::json(200, &serde_json::json!({ "object": "list", "data": data }))
        }
        ("GET", "/v1/codex/models") => {
            // Codex 自己形状的目录（model_catalog_url 读它；/v1/models 对 Codex 不可读）。
            let rows = match catalog_rows(gateway) {
                Ok(r) => r,
                Err(reply) => return reply,
            };
            let version = request
                .path
                .split_once('?')
                .and_then(|(_, q)| q.split('&').find_map(|kv| kv.strip_prefix("client_version=")))
                .unwrap_or("");
            let (body, left_out) = super::codex_catalog::build(&rows, version);
            if left_out > 0 {
                super::httpd::note_header("X-Magpie-Left-Out", &left_out.to_string());
            }
            Reply::Json { status: 200, body }
        }
        ("GET", "/v1/magpie/quotas") => {
            // C3：无参 = force（现场问供应商）；?kept=1 = 读 60s 缓存。
            let kept = request
                .path
                .split_once('?')
                .map(|(_, query)| query.split('&').any(|pair| pair == "kept=1"))
                .unwrap_or(false);
            Reply::json(
                200,
                &serde_json::json!({
                    "object": "list",
                    "data": super::quotas::quota_entries(gateway, !kept),
                }),
            )
        }
        ("POST", "/v1/chat/completions") | ("POST", "/chat/completions") => {
            // 入站体预处理（bodyprep.rs，Go base64_input.go + body_text.go）：
            // Go 里 textBody 在读体出口（request_bounds.go:284）、stdBase64Body
            // 在 serve() 最前（gateway.go:2544），都在任何路由/翻译/bodycap
            // 之前。Rust 的读体在 httpd.rs `handle`、路由入口在这里——所以
            // 接在 route() 拿到体之后、进任何处理器（含 codex/grok 拦截：
            // 它们也要解析并转发这个体）之前。体没变时零拷贝借回原请求。
            // Err（redact 开态扫不了的体）= fails-closed 拒发 503（Go
            // maskStatus 非拦截分支，internal/gateway/redact.go:229）。
            let req = match super::bodyprep::try_prepared_request(request, super::effort::Proto::Chat) {
                Ok(req) => req,
                Err(e) => return Reply::error(503, &e),
            };
            let request = &*req;
            if let Some(reply) = super::codex_account::intercept(gateway, request, super::effort::Proto::Chat) {
                return reply;
            }
            if let Some(reply) = super::grok_account::intercept(gateway, request, super::effort::Proto::Chat) {
                return reply;
            }
            let body = &request.body;
            super::chat::forward(gateway, &request.headers, &body)
        }
        ("POST", "/v1/responses") | ("POST", "/responses") => {
            // codex 的母语端点（T4）：直通优先，翻译路径复用 chat 全链。
            // Codex 订阅账号（合成来源 codex）接不住真实来源的请求先在这里处理。
            let req = match super::bodyprep::try_prepared_request(request, super::effort::Proto::Responses) {
                Ok(req) => req,
                Err(e) => return Reply::error(503, &e),
            };
            let request = &*req;
            if let Some(reply) = super::codex_account::intercept(gateway, request, super::effort::Proto::Responses) {
                return reply;
            }
            if let Some(reply) = super::grok_account::intercept(gateway, request, super::effort::Proto::Responses) {
                return reply;
            }
            let body = &request.body;
            super::responses::handle(gateway, &body)
        }
        ("POST", "/v1/messages/count_tokens") | ("POST", "/messages/count_tokens") => {
            // Go countTokens：厂商有 anthropic 基址就转它的计数，不支持/没有就本地粗估。
            // 计数前同样跑 bodyprep（Go 在这里显式调 stdBase64Body，gateway.go:2112，
            // 注释：按将要发送的形态计数）——转发给厂商计数的体必须与真发的同形态。
            let req = match super::bodyprep::try_prepared_request(request, super::effort::Proto::Anthropic) {
                Ok(req) => req,
                Err(e) => return Reply::error(503, &e),
            };
            let request = &*req;
            super::messages::count_tokens(gateway, &request.body)
        }
        ("POST", "/v1/messages") | ("POST", "/messages") => {
            // anthropic Messages 端点（T3）：直通优先 + chat 翻译路径。
            let req = match super::bodyprep::try_prepared_request(request, super::effort::Proto::Anthropic) {
                Ok(req) => req,
                Err(e) => return Reply::error(503, &e),
            };
            let request = &*req;
            if let Some(reply) = super::codex_account::intercept(gateway, request, super::effort::Proto::Anthropic) {
                return reply;
            }
            if let Some(reply) = super::grok_account::intercept(gateway, request, super::effort::Proto::Anthropic) {
                return reply;
            }
            // Claude 订阅桥（anthropic 原生入站——chat/responses 入站是桥的
            // 差距项，见 claude_account.rs 头注；不在这两个臂挂）。
            if let Some(reply) = super::claude_account::intercept(gateway, request, super::effort::Proto::Anthropic) {
                return reply;
            }
            let body = &request.body;
            super::messages::handle(gateway, &body)
        }
        (method, path) if method == "GET" && path.starts_with("/v1/models/group/") => {
            // C4 组宣告探针：组在 → 200（列成员行），不在 → 404。
            let id = path.trim_start_matches("/v1/models/group/");
            let store = match gateway.store.lock() {
                Ok(s) => s,
                Err(_) => return Reply::error(500, "store poisoned"),
            };
            match store.group(id) {
                Some(group) => {
                    let rows: Vec<Value> = group
                        .members
                        .iter()
                        .map(|m| serde_json::json!({ "id": m, "object": "model", "type": "model" }))
                        .collect();
                    Reply::json(200, &serde_json::json!({ "object": "list", "data": rows }))
                }
                None => Reply::error(404, &format!("no group: {id}")),
            }
        }
        _ => Reply::error(
            404,
            "magpie serves /v1/models, /v1/codex/models, /v1/chat/completions, /v1/responses, /v1/messages (+ /count_tokens) and /v1/magpie/quotas",
        ),
    }
}
