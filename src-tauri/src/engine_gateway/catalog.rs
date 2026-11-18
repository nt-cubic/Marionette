//! 模型目录（合同 C2）：厂商 `/v1/models` 抓取 + 元数据转述 + 钉死/live
//! 语义 + 30s 缓存。`context_window / max_output_tokens / modalities.input`
//! 是 Marionette 跨用宣告的唯一事实源——漏转述等于各家 agent 按自己的
//! 默认窗口办事（docs/plans/plan-engine-contract.md C2 红线）。
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
    /// 抓取失败的来源 → 下次允许重试的时刻。没有它，一个挂掉的厂商会让
    /// 每次 `/v1/models` 都白等 10s（目录是串行抓的，菜单 5s 就超时）。
    failed: Mutex<HashMap<String, Instant>>,
}

/// 失败退避：厂商抓不到时，这段时间内直接用旧缓存/空表，不再打它。
const FAIL_BACKOFF: Duration = Duration::from_secs(60);

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
    /// 厂商行的 presets 兜底（C2 红线）：厂商没说窗口/输出/图片能力的，
    /// 用 models.dev 目录补（lookup 纯内存，无缓存时 None——不编数）。
    /// 返回可能被补全过的行。
    fn with_preset_fallback(row: VendorRow) -> VendorRow {
        let preset = match super::presets::lookup(&row.id) {
            Some(p) => p,
            None => return row,
        };
        let mut raw = row.raw;
        if let Some(map) = raw.as_object_mut() {
            if row.context_window == 0 {
                map.insert("context_window".into(), serde_json::json!(preset.context_window));
            }
            if row.max_output == 0 {
                map.insert("max_output_tokens".into(), serde_json::json!(preset.max_output));
            }
        }
        VendorRow {
            context_window: if row.context_window == 0 { preset.context_window } else { row.context_window },
            max_output: if row.max_output == 0 { preset.max_output } else { row.max_output },
            images: row.images || preset.images,
            raw,
            ..row
        }
    }

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
        // 退避窗口内（且不是强制刷新）：不再打这个厂商，旧缓存兜底。
        if !force {
            let backing_off = self
                .failed
                .lock()
                .ok()
                .and_then(|m| m.get(&provider.id).copied())
                .is_some_and(|until| Instant::now() < until);
            if backing_off {
                return Ok(self.stale_rows(&provider.id));
            }
        }
        match self.fetch_vendor_rows(provider, &url) {
            Ok(rows) => {
                if let Ok(mut failed) = self.failed.lock() {
                    failed.remove(&provider.id);
                }
                if let Ok(mut cache) = self.cache.lock() {
                    cache.insert(provider.id.clone(), (Instant::now(), rows.clone()));
                }
                Ok(rows)
            }
            Err(e) => {
                if let Ok(mut failed) = self.failed.lock() {
                    failed.insert(provider.id.clone(), Instant::now() + FAIL_BACKOFF);
                }
                // 厂商一时抓不到：有旧清单就继续用旧的（magpie 同款——
                // 跨用菜单不该因为一次网络抖动整家消失），没有才报错。
                let stale = self.stale_rows(&provider.id);
                if stale.is_empty() { Err(e) } else { Ok(stale) }
            }
        }
    }

    fn stale_rows(&self, provider_id: &str) -> Vec<VendorRow> {
        self.cache
            .lock()
            .ok()
            .and_then(|c| c.get(provider_id).map(|(_, rows)| rows.clone()))
            .unwrap_or_default()
    }

    fn fetch_vendor_rows(&self, provider: &Provider, url: &str) -> Result<Vec<VendorRow>, String> {
        let agent = super::outbound_agent();
        let mut request = agent.get(url).set("Accept", "application/json");
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
        Ok(parse_rows(&body).into_iter().map(Self::with_preset_fallback).collect())
    }

    /// 整串 `provider/model` 是不是该来源**已知**的模型（钉死清单或缓存里
    /// 有）——组成员后缀拆分用（`qwen:7b` 不是 `qwen` 加强度）。纯内存，
    /// 绝不联网。
    pub fn knows(&self, providers: &[Provider], whole: &str) -> bool {
        let Some((pid, model)) = whole.split_once('/') else { return false };
        let Some(provider) = providers.iter().find(|p| p.id == pid) else { return false };
        if provider.models.as_ref().is_some_and(|m| m.iter().any(|x| x == model)) {
            return true;
        }
        self.cache
            .lock()
            .ok()
            .and_then(|c| c.get(pid).map(|(_, rows)| rows.iter().any(|r| r.id == model)))
            .unwrap_or(false)
    }

    /// 缓存行里该模型自报的思考档位（`reasoning_levels` 等）。没有就空——
    /// 强度原样发，不编数。
    pub fn levels_of(&self, provider_id: &str, model: &str) -> Vec<String> {
        let cache = match self.cache.lock() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let Some((_, rows)) = cache.get(provider_id) else { return Vec::new() };
        let Some(row) = rows.iter().find(|r| r.id == model) else { return Vec::new() };
        for key in ["reasoning_levels", "supported_reasoning_levels", "reasoning_efforts", "efforts"] {
            if let Some(list) = row.raw.get(key).and_then(|v| v.as_array()) {
                let words: Vec<String> = list
                    .iter()
                    .filter_map(|v| {
                        v.as_str()
                            .map(String::from)
                            .or_else(|| v.get("id").and_then(|x| x.as_str()).map(String::from))
                    })
                    .collect();
                if !words.is_empty() {
                    return words;
                }
            }
        }
        Vec::new()
    }

    /// 并发预热所有来源的目录（`/v1/models` 入口调）：串行抓取时 N 个慢
    /// 厂商 = N×10s，菜单 5s 超时直接整组消失。线程各抓各的，总耗时 =
    /// 最慢那一家。结果落缓存，随后 `listed` 读缓存。
    pub fn prewarm(&self, providers: &[Provider]) {
        let due: Vec<&Provider> = providers
            .iter()
            .filter(|p| p.chat_base().is_some())
            .filter(|p| {
                let fresh = self
                    .cache
                    .lock()
                    .ok()
                    .and_then(|c| c.get(&p.id).map(|(at, _)| at.elapsed() < CACHE_TTL))
                    .unwrap_or(false);
                let backing_off = self
                    .failed
                    .lock()
                    .ok()
                    .and_then(|m| m.get(&p.id).copied())
                    .is_some_and(|until| Instant::now() < until);
                !fresh && !backing_off
            })
            .collect();
        if due.len() < 2 {
            return; // 0/1 家：listed 自己抓就是
        }
        std::thread::scope(|scope| {
            for provider in due {
                scope.spawn(move || {
                    let _ = self.vendor_rows(provider, false);
                });
            }
        });
    }

    /// C2 的 `/v1/models` 行：厂商行按钉死清单过滤（`models: None` = live
    /// 全量）；厂商抓不到时对钉死模型合成行（窗口来自 presets 兜底，
    /// 仍不知道就是 0——不编数）。
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
                        .or_else(|| {
                            let mut row = VendorRow {
                                id: want.clone(),
                                context_window: 0,
                                max_output: 0,
                                images: false,
                                raw: serde_json::json!({ "id": want }),
                            };
                            if let Some(p) = super::presets::lookup(want) {
                                row.context_window = p.context_window;
                                row.max_output = p.max_output;
                                row.images = p.images;
                            }
                            Some(row)
                        })
                })
                .collect(),
        }
    }
}
