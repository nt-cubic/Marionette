//! 字节上限自适配（合同 C5+C7；marionette_body_cap.go 的 Rust 对应）。
//!
//! 厂商拒收超限体时（明说超限，或 ollama.com 那句不读体的
//! "failed to read request body"——2026-11-12 会话死锁事故）：学到 cap
//! （厂商说了数就用它的，否则按本次的 3/4），把**最老**的图换成占位文本
//! 重发，直到压到 cap 的 3/4 以下（给对话留增长余量，别每轮都破坏缓存）；
//! 最新一条用户消息的图保留。无图可丢 → 413（带尺寸语义，codex 能读懂
//! 方向而不是当传输错误原样重发）。后续请求先按已学 cap 预剪裁。
use serde_json::Value;

use super::httpd::Reply;

/// ollama 那句拒答只在大体上才可信（marionette 补丁同款 8MiB 门槛）。
const UNREAD_FLOOR: usize = 8 << 20;

/// cap 拒答词表（对齐 magpie bodyCapWords 的常用子集；精确正则见账本任务）。
const CAP_WORDS: [&str; 6] = [
    "too large",
    "over the",
    "request_too_large",
    "payload too large",
    "exceeds the",
    "limit for one request",
];

fn is_cap_refusal(status: u16, err_body: &[u8], sent: usize) -> bool {
    if status == 401 || status == 403 || status == 429 {
        return false;
    }
    if status == 413 {
        return true;
    }
    if status < 400 {
        return false;
    }
    let text = String::from_utf8_lossy(err_body).to_ascii_lowercase();
    if sent >= UNREAD_FLOOR && text.contains("failed to read request body") {
        return true;
    }
    CAP_WORDS.iter().any(|w| text.contains(w))
}

/// 从拒答文本里挖厂商明说的 cap（`(33554432 bytes)` 优先，退 `N MB`）。
fn stated_cap(err_body: &[u8]) -> Option<usize> {
    let text = String::from_utf8_lossy(err_body);
    let parens: Vec<&str> = text
        .split(|c| c == '(' || c == ')')
        .filter(|s| s.contains("bytes") || s.contains("byte)"))
        .collect();
    for chunk in parens {
        let digits: String = chunk.chars().filter(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            if let Ok(n) = digits.parse::<usize>() {
                if n > 0 {
                    return Some(n);
                }
            }
        }
    }
    // "over the 32 MB limit"
    let lower = text.to_ascii_lowercase();
    if let Some(pos) = lower.find("over the") {
        let tail = &lower[pos + 8..];
        for unit in ["gb", "mb", "kb"] {
            if let Some(u) = tail.find(unit) {
                let digits: String = tail[..u].chars().filter(|c| c.is_ascii_digit()).collect();
                if let Ok(n) = digits.parse::<usize>() {
                    let mult = match unit {
                        "gb" => 1 << 30,
                        "mb" => 1 << 20,
                        _ => 1 << 10,
                    };
                    return Some(n * mult);
                }
            }
        }
    }
    None
}

/// 序列化后的字节数。
fn size_of(value: &Value) -> usize {
    serde_json::to_vec(value).map(|b| b.len()).unwrap_or(usize::MAX)
}

/// 把最老的图换成占位文本直到压到 `target` 以下；返回 (新体, 丢了几张)。
/// 最新一条用户消息的图不动；没有可丢的图/压不进去返回 None。
fn without_oldest_images(body: &[u8], target: usize) -> Option<(Vec<u8>, usize)> {
    let mut value = serde_json::from_slice::<Value>(body).ok()?;
    let count = value
        .get("messages")?
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    // 最新一条用户消息的下标（它的图保留——magpie 同款规则）。
    let last_user = value
        .get("messages")
        .and_then(|m| m.as_array())
        .and_then(|a| a.iter().rposition(|m| m.get("role").and_then(|r| r.as_str()) == Some("user")));

    let mut shed = 0usize;
    loop {
        if size_of(&value) <= target {
            break;
        }
        // 每轮从最老的消息重新扫起，丢**一张**图（天然最老优先）。
        let mut replaced = false;
        'messages: for index in 0..count {
            if Some(index) == last_user {
                continue;
            }
            let Some(parts) = value
                .get_mut("messages")
                .and_then(|m| m.as_array_mut())
                .and_then(|a| a.get_mut(index))
                .and_then(|m| m.get_mut("content"))
                .and_then(|c| c.as_array_mut())
            else {
                continue;
            };
            for part in parts.iter_mut() {
                if part.get("type").and_then(|t| t.as_str()) == Some("image_url") {
                    *part = Value::String(
                        "[image left out: the request was over the provider's byte cap]".into(),
                    );
                    shed += 1;
                    replaced = true;
                    break 'messages;
                }
            }
        }
        if !replaced {
            return None;
        }
    }
    if shed == 0 {
        return None;
    }
    serde_json::to_vec(&value).ok().map(|b| (b, shed))
}

fn over_cap_reply(sent: usize, cap: usize, shed_attempted: bool) -> Reply {
    let mb = |n: usize| format!("{:.1} MB", n as f64 / 1_048_576.0);
    Reply::error(
        413,
        &format!(
            "the request is {}, over the {} it takes in one request{}",
            mb(sent),
            mb(cap),
            if shed_attempted {
                "; leaving out its earlier images didn't bring it under"
            } else {
                "; it holds no image to leave out"
            }
        ),
    )
}

/// chat.rs 在 4xx 应答后调用：命中 cap 拒答 → 学 cap、丢最老图重发（至多
/// 3 轮）；不是 cap 拒答返回 None（原样透传）。
pub fn maybe_refit(
    gw: &super::Gateway,
    out: &super::chat::Outbound,
    sent: &[u8],
    status: u16,
    err_body: &[u8],
) -> Option<Reply> {
    if !is_cap_refusal(status, err_body, sent.len()) {
        return None;
    }
    let mut cap = stated_cap(err_body).unwrap_or(sent.len() / 4 * 3).max(1);
    {
        let mut caps = gw.caps.lock().ok()?;
        caps.insert(out.provider_id.clone(), cap);
    }
    let mut current = sent.to_vec();
    for _ in 0..3 {
        let target = cap / 4 * 3;
        let Some((fitted, shed)) = without_oldest_images(&current, target) else {
            return Some(over_cap_reply(sent.len(), cap, true));
        };
        if fitted.len() >= current.len() {
            return Some(over_cap_reply(sent.len(), cap, true));
        }
        match super::chat::send(out, &fitted) {
            Err(e) => return Some(Reply::error(502, &format!("{}: {}", out.provider_id, e))),
            Ok(response) => {
                let restatus = response.status();
                let mut reader = response.into_reader();
                let mut vendor_body = Vec::new();
                use std::io::Read;
                let _ = reader.read_to_end(&mut vendor_body);
                if restatus >= 400 && is_cap_refusal(restatus, &vendor_body, fitted.len()) {
                    if let Some(stated) = stated_cap(&vendor_body) {
                        cap = stated;
                    } else {
                        cap = fitted.len() / 4 * 3;
                    }
                    current = fitted;
                    let _ = shed;
                    continue;
                }
                return Some(Reply::Json { status: restatus, body: vendor_body });
            }
        }
    }
    Some(over_cap_reply(sent.len(), cap, true))
}

/// 出站前的预剪裁（cap 已知且体超 3/4）：后续请求直接以合身形态发出，
/// 不用先吃一次拒答。返回 None = 不需要剪。
pub fn prefit(gw: &super::Gateway, provider_id: &str, body: &[u8]) -> Option<Vec<u8>> {
    let cap = *gw.caps.lock().ok()?.get(provider_id)?;
    if body.len() <= cap / 4 * 3 {
        return None;
    }
    without_oldest_images(body, cap / 4 * 3).map(|(fitted, _)| fitted)
}
