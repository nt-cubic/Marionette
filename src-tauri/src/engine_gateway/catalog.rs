//! 模型目录（合同 C2）：厂商 `/v1/models` 抓取 + 元数据转述 + 钉死/live
//! 语义 + 30s 缓存。`context_window / max_output_tokens / modalities.input`
//! 是 Marionette 跨用宣告的唯一事实源——漏转述等于各家 agent 按自己的
//! 默认窗口办事（docs/plan-engine-contract.md C2 红线）。
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::store::Provider;

const CACHE_TTL: Duration = Duration::from_secs(30);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// 一行厂商目录（已归一化）。
#[derive(Clone)]
pub struct VendorRow {
    pub id: String,
    pub context_window: u64,
    pub max_output: u64,
    pub images: bool,
    /// 厂商原行（display_name / reasoning_levels 等透传给 /v1/models）。
    pub raw: serde_json::Value,
}

/// 目录缓存：provider id → (抓取时刻, 行)。`refresh` 强制重抓。
#[derive(Default)]
pub struct Catalog {
    cache: Mutex<HashMap<String, (Instant, Vec<VendorRow>)>>,
}

fn positive(item: &serde_json::Value, keys: &[&str]) -> u64 {
    keys.iter().find_map(|k| {
        item.get(*k)
            .and_then(|v| v.as_u64())
            .filter(|v| *v > 0)
    })
    .unwrap_or(0)
}

fn takes_images(item: &serde_json::Value) -> bool {
    // 两种形状都认（magpie live.go 同款）：modalities.input 数组，
    // 或 input_modalities 数组。
    let lists = [
        item.get("modalities").and_then(|m| m.get("input")).and_then(|v| v.as_array()),
        item.get("input_modalities").and_then(|v| v.as_array()),
    ];
    for list in lists.into_iter().flatten() {
        if list.iter().any(|v| v.as_str() == Some("image")) {
            return true;
        }
    }
    false
}

fn parse_rows(body: &str) -> Vec<VendorRow> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let items = value
        .get("data")
        .and_then(|d| d.as_array())
        .or_else(|| value.get("models").and_then(|m| m.as_array()));
    let Some(items) = items else { return Vec::new() };
    let mut out = Vec::new();
    for item in items {
        let Some(id) = item.get("id").and_then(|v| v.as_str()) else { continue };
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        out.push(VendorRow {
            id: id.to_string(),
            context_window: positive(item, &["context_window", "context_length", "max_input_tokens"]),
            max_output: positive(item, &["max_output_tokens", "max_tokens"]),
            images: takes_images(item),
            raw: item.clone(),
        });
    }
    out
}

impl Catalog {
    /// 抓（或读缓存）某来源的厂商行。force = CLI refresh 语义。
    pub fn vendor_rows(&self, provider: &Provider, force: bool) -> Result<Vec<VendorRow>, String> {
        let base = provider.chat_base().ok_or_else(|| format!("{} has no endpoint configured", provider.id))?;
        let url = format!("{}/models", base);
        if !force {
            if let Ok(cache) = self.cache.lock() {
                if let Some((at, rows)) = cache.get(&provider.id) {
                    if at.elapsed() < CACHE_TTL {
                        return Ok(rows.clone());
                    }
                }
            }
        }
        let agent = super::outbound_agent();
        let mut request = agent.get(&url).set("Accept", "application/json");
        if !provider.key.is_empty() {
            request = request.set("Authorization", &format!("Bearer {}", provider.key));
        }
        for (k, v) in &provider.headers {
            request = request.set(k, v);
        }
        let response = request
            .timeout(FETCH_TIMEOUT)
            .call()
            .map_err(|e| format!("{}: {}", provider.id, e))?;
        let body = response.into_string().map_err(|e| e.to_string())?;
        let rows = parse_rows(&body);
        if let Ok(mut cache) = self.cache.lock() {
            cache.insert(provider.id.clone(), (Instant::now(), rows.clone()));
        }
        Ok(rows)
    }

    /// C2 的 `/v1/models` 行：厂商行按钉死清单过滤（`models: None` = live
    /// 全量）；厂商抓不到时对钉死模型合成 ctx=0 行（不编数，如实「不知道」）。
    pub fn listed(&self, provider: &Provider) -> Vec<VendorRow> {
        let fetched = self.vendor_rows(provider, false).unwrap_or_default();
        match &provider.models {
            None => fetched,
            Some(pinned) => pinned
                .iter()
                .filter_map(|want| {
                    fetched
                        .iter()
                        .find(|r| &r.id == want)
                        .cloned()
                        .or_else(|| Some(VendorRow {
                            id: want.clone(),
                            context_window: 0,
                            max_output: 0,
                            images: false,
                            raw: serde_json::json!({ "id": want }),
                        }))
                })
                .collect(),
        }
    }
}
