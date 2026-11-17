//! `/v1/chat/completions` 转发（合同 C5 的 openai→openai 直通形态）。
//! 模型 id 剥 `provider/` 前缀后按来源出站；SSE 流式透传（边读边 flush）。
//! 字节上限 refit（丢最老图重发）在 bodycap.rs，由这里在 4xx 应答后调用。
use std::io::Read;

use serde_json::Value;

use super::bodycap;
use super::httpd::Reply;

/// 出站请求的目标（bodycap 重发复用）。
pub(crate) struct Outbound {
    pub(crate) provider_id: String,
    key: String,
    headers: Vec<(String, String)>,
    bare_model: String,
    url: String,
}

fn resolve(gw: &super::Gateway, model: &str) -> Result<Outbound, String> {
    // 组别名与 model[effort] 剥离在 translate.rs 成熟前先支持 provider/model
    // 直通形态（跨用桥写入引擎表的就是这个形态）。
    let (provider_id, bare) = match model.split_once('/') {
        Some((p, m)) if !p.is_empty() && !m.is_empty() => (p.to_string(), m.to_string()),
        _ => return Err(format!("unknown model: {model}")),
    };
    let store = gw.store.lock().map_err(|_| "store poisoned".to_string())?;
    let provider = store
        .find(&provider_id)
        .ok_or_else(|| format!("no provider for model: {model}"))?;
    let base = provider
        .chat_base()
        .ok_or_else(|| format!("{} has no endpoint configured", provider_id))?
        .to_string();
    Ok(Outbound {
        provider_id: provider.id.clone(),
        key: provider.key.clone(),
        headers: provider.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        bare_model: bare,
        url: format!("{}/chat/completions", base),
    })
}

/// 把请求体里的 `model` 换成厂商裸名。
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

pub(crate) fn send(out: &Outbound, body: &[u8]) -> Result<ureq::Response, String> {
    let agent = super::outbound_agent();
    let mut request = agent
        .post(&out.url)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json");
    if !out.key.is_empty() {
        request = request.set("Authorization", &format!("Bearer {}", out.key));
    }
    for (k, v) in &out.headers {
        request = request.set(k, v);
    }
    match request.send_bytes(body) {
        Ok(response) => Ok(response),
        // ureq 2 把非 2xx 当 Err(Status(code, resp))——厂商的 4xx/5xx 是
        // 合同面的一部分（body-cap refit 靠它），原样取回 response。
        Err(ureq::Error::Status(_, response)) => Ok(response),
        Err(e) => Err(e.to_string()),
    }
}

fn read_all(mut reader: impl Read) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = reader.read_to_end(&mut buf);
    buf
}

/// POST /v1/chat/completions 入口。
pub fn forward(gw: &super::Gateway, body: &[u8]) -> Reply {
    let model = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(String::from))
        .unwrap_or_default();
    let out = match resolve(gw, &model) {
        Ok(o) => o,
        Err(e) => return Reply::error(404, &e),
    };
    let out_body = rewrite_model(body, &out.bare_model);
    // cap 已知时先合身再发（免吃一次拒答）。
    let out_body = bodycap::prefit(gw, &out.provider_id, &out_body).unwrap_or(out_body);
    let streaming = serde_json::from_slice::<Value>(&out_body)
        .ok()
        .and_then(|v| v.get("stream").and_then(|s| s.as_bool()))
        .unwrap_or(false);

    let response = match send(&out, &out_body) {
        Ok(r) => r,
        Err(e) => return Reply::error(502, &format!("{}: {}", out.provider_id, e)),
    };

    if streaming {
        let content_type = response.content_type().to_string();
        return Reply::Stream {
            status: response.status(),
            content_type,
            reader: Box::new(response.into_reader()),
        };
    }

    let status = response.status();
    let vendor_body = read_all(response.into_reader());

    // C5+C7：字节上限拒答（ollama「failed to read request body」/ 明说超限）→
    // 学 cap、丢最老图重发；bodycap 没命中时原样透传错误。
    if status >= 400 {
        if let Some(reply) = bodycap::maybe_refit(gw, &out, &out_body, status, &vendor_body) {
            return reply;
        }
    }
    Reply::Json { status, body: vendor_body }
}
