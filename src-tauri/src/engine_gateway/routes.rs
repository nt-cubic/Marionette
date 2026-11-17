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

pub fn route(request: &Request, gateway: &super::Gateway) -> Reply {
    // CLI/外部进程写的 providers.json 立即可见（magpie heldFresh 语义）。
    if let Ok(mut store) = gateway.store.lock() {
        store.reload();
    }
    let path = request.path.split('?').next().unwrap_or("");
    match (request.method.as_str(), path) {
        ("GET", "/v1/models") => {
            let store = match gateway.store.lock() {
                Ok(s) => s,
                Err(_) => return Reply::error(500, "store poisoned"),
            };
            let mut rows = Vec::new();
            for provider in store.providers() {
                for row in gateway.catalog.listed(provider) {
                    rows.push(model_row(&provider.id, &row));
                }
            }
            Reply::json(200, &serde_json::json!({ "object": "list", "data": rows }))
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
            super::chat::forward(gateway, &request.body)
        }
        ("POST", "/v1/responses") | ("POST", "/responses") => {
            // codex 的母语端点（T4）：直通优先，翻译路径复用 chat 全链。
            super::responses::handle(gateway, &request.body)
        }
        ("POST", "/v1/messages") | ("POST", "/messages") => {
            // anthropic Messages 端点（T3）：直通优先 + chat 翻译路径。
            super::messages::handle(gateway, &request.body)
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
            "magpie serves /v1/models, /v1/chat/completions, /v1/responses, /v1/messages and /v1/magpie/quotas",
        ),
    }
}
