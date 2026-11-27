//! 模型目录（合同 C2）：厂商 `/v1/models` 抓取 + 元数据转述 + 钉死/live
//! 语义 + 30s 缓存。`context_window / max_output_tokens / modalities.input`
//! 是 Marionette 跨用宣告的唯一事实源——漏转述等于各家 agent 按自己的
//! 默认窗口办事（docs/plans/plan-engine-contract.md C2 红线）。
//! 厂商漏报的窗口字段由 presets（models.dev 目录）逐字段兜——兜底源的新鲜
//! 度由 `vendor_rows` 口上的 `presets::ensure_fresh` 负责（Go KeepFresh 的
//! 最小版；没有它 zhipu 这类不报窗口的厂商整族 ctx=0，T13 真机对账实锤）。
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
    keys.iter()
        .find_map(|k| item.get(*k).and_then(|v| v.as_u64()).filter(|v| *v > 0))
        .unwrap_or(0)
}

fn takes_images(item: &serde_json::Value) -> bool {
    // 两种形状都认（magpie live.go 同款）：modalities.input 数组，
    // 或 input_modalities 数组。
    let lists = [
        item.get("modalities")
            .and_then(|m| m.get("input"))
            .and_then(|v| v.as_array()),
        item.get("input_modalities").and_then(|v| v.as_array()),
    ];
    for list in lists.into_iter().flatten() {
        if list.iter().any(|v| v.as_str() == Some("image")) {
            return true;
        }
    }
    false
}

/// 厂商行有没有自己声明过 input 模态（takes_images 同款两种形状）。
/// 兜底用它区分「厂商说了 text-only」与「厂商什么都没说」——Go Decorate
/// 只在厂商没说（ImageInput == nil）时认目录的图片信息。
fn said_input_modalities(item: &serde_json::Value) -> bool {
    item.get("modalities")
        .and_then(|m| m.get("input"))
        .and_then(|v| v.as_array())
        .is_some()
        || item
            .get("input_modalities")
            .and_then(|v| v.as_array())
            .is_some()
}

fn parse_rows(body: &str) -> Vec<VendorRow> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Vec::new();
    };
    let items = value
        .get("data")
        .and_then(|d| d.as_array())
        .or_else(|| value.get("models").and_then(|m| m.as_array()));
    let Some(items) = items else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for item in items {
        let Some(id) = item.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        let id = id.trim();
        if id.is_empty() {
            continue;
        }
        out.push(VendorRow {
            id: id.to_string(),
            context_window: positive(
                item,
                &["context_window", "context_length", "max_input_tokens"],
            ),
            max_output: positive(item, &["max_output_tokens", "max_tokens"]),
            images: takes_images(item),
            raw: item.clone(),
        });
    }
    out
}

impl Catalog {
    /// 厂商行的 presets 兜底（C2 红线；Go catalog.Decorate 同语义）：
    /// 厂商没说窗口/输出/图片/显示名的，用 models.dev 目录**逐字段**补，
    /// 厂商显式报的值（含「明确说不」：modalities.input 只列 text）永远
    /// 优先；目录也不知道的保持 0/false——不编数。zhipu coding 端点的
    /// `/v1/models` 只回 `id/object/created/owned_by`（T13 实锤），整族
    /// 行都靠这条补全。
    fn with_preset_fallback(row: VendorRow) -> VendorRow {
        let preset = match super::presets::lookup(&row.id) {
            Some(p) => p,
            None => return row,
        };
        // 图片输入（Go Decorate：m.ImageInput == nil 才认目录——厂商明确
        // 列过 input 模态（哪怕只有 text）就以厂商为准，目录不得翻案）。
        // 要在 raw 被移走前读。
        let vendor_said_input = said_input_modalities(&row.raw);
        let mut raw = row.raw;
        if let Some(map) = raw.as_object_mut() {
            // 目录也不知道（0）就不写——写个 0 进 raw 等于把「不知道」
            // 伪装成「厂商报了 0」。
            if row.context_window == 0 && preset.context_window > 0 {
                map.insert(
                    "context_window".into(),
                    serde_json::json!(preset.context_window),
                );
            }
            if row.max_output == 0 && preset.max_output > 0 {
                map.insert(
                    "max_output_tokens".into(),
                    serde_json::json!(preset.max_output),
                );
            }
            // 显示名（Go Decorate 的 Name 行）：厂商起了名（display_name /
            // name，非空且不等于 id）就用厂商的；没起名才认目录的。
            // display_name 是 routes.rs model_row 读的通道，厂商只给 name
            // 时把它抬进去。
            let own_name = ["display_name", "name"].iter().find_map(|key| {
                map.get(*key)
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|n| !n.is_empty() && *n != row.id)
            });
            match own_name {
                Some(own) => {
                    let has_display = map
                        .get("display_name")
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|n| !n.is_empty())
                        .is_some();
                    if !has_display {
                        map.insert("display_name".into(), serde_json::json!(own));
                    }
                }
                None => {
                    if !preset.display_name.is_empty() {
                        map.insert(
                            "display_name".into(),
                            serde_json::json!(preset.display_name),
                        );
                    }
                }
            }
        }
        VendorRow {
            context_window: if row.context_window == 0 {
                preset.context_window
            } else {
                row.context_window
            },
            max_output: if row.max_output == 0 {
                preset.max_output
            } else {
                row.max_output
            },
            images: if vendor_said_input {
                row.images
            } else {
                row.images || preset.images
            },
            raw,
            ..row
        }
    }

    /// 抓（或读缓存）某来源的厂商行。force = CLI refresh 语义。
    pub fn vendor_rows(&self, provider: &Provider, force: bool) -> Result<Vec<VendorRow>, String> {
        // models.dev 目录保鲜（Go `magpie serve` 起一处 catalog.KeepFresh
        // 同款）：放在最前，缓存命中/厂商退避的早退路径也照过——厂商挂了
        // 也要补目录。纯状态检查 + 单飞后台补抓，绝不阻塞本线程。
        super::presets::ensure_fresh();
        let base = provider
            .chat_base()
            .ok_or_else(|| format!("{} has no endpoint configured", provider.id))?;
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
                if stale.is_empty() {
                    Err(e)
                } else {
                    Ok(stale)
                }
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
        Ok(parse_rows(&body)
            .into_iter()
            .map(Self::with_preset_fallback)
            .collect())
    }

    /// 整串 `provider/model` 是不是该来源**已知**的模型（钉死清单或缓存里
    /// 有）——组成员后缀拆分用（`qwen:7b` 不是 `qwen` 加强度）。纯内存，
    /// 绝不联网。
    pub fn knows(&self, providers: &[Provider], whole: &str) -> bool {
        let Some((pid, model)) = whole.split_once('/') else {
            return false;
        };
        let Some(provider) = providers.iter().find(|p| p.id == pid) else {
            return false;
        };
        if provider
            .models
            .as_ref()
            .is_some_and(|m| m.iter().any(|x| x == model))
        {
            return true;
        }
        self.cache
            .lock()
            .ok()
            .and_then(|c| {
                c.get(pid)
                    .map(|(_, rows)| rows.iter().any(|r| r.id == model))
            })
            .unwrap_or(false)
    }

    /// 缓存行里该模型自报的思考档位（`reasoning_levels` 等）。没有就空——
    /// 强度原样发，不编数。
    pub fn levels_of(&self, provider_id: &str, model: &str) -> Vec<String> {
        let cache = match self.cache.lock() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let Some((_, rows)) = cache.get(provider_id) else {
            return Vec::new();
        };
        let Some(row) = rows.iter().find(|r| r.id == model) else {
            return Vec::new();
        };
        for key in [
            "reasoning_levels",
            "supported_reasoning_levels",
            "reasoning_efforts",
            "efforts",
        ] {
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
                    fetched.iter().find(|r| &r.id == want).cloned().or_else(|| {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static SEQ: AtomicUsize = AtomicUsize::new(0);

    /// 测试沙箱：XDG 重定向 + 模块状态复位 + 跨模块串行锁（presets.rs
    /// 沙箱同款），Drop 收尾（含 panic 展开路径）。绝不碰真实
    /// `~/.config/magpie`。
    struct Sandbox {
        dir: PathBuf,
        prior_xdg: Option<String>,
        lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            crate::engine_gateway::presets::reset_for_tests();
            if let Some(prior) = &self.prior_xdg {
                std::env::set_var("XDG_CONFIG_HOME", prior);
            } else {
                std::env::remove_var("XDG_CONFIG_HOME");
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn sandbox() -> Sandbox {
        let lock = crate::engine_gateway::TEST_SANDBOX_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        crate::engine_gateway::presets::reset_for_tests();
        let dir = std::env::temp_dir().join(format!(
            "marionette-engine-catalog-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp config dir");
        let prior_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        Sandbox {
            dir,
            prior_xdg,
            lock,
        }
    }

    /// models.dev 目录（真实形状的 zhipu 子集，数值照 2026-11-19 api.json
    /// 实抓）：zai 的 glm-4.5（ctx=131072 / out=98304，纯文本）、glm-4.5v
    /// （64000，text+image+video）、coding plan 专属 glm-5.3-highspeed。
    const CATALOG: &str = r#"{
        "zai": {
            "name": "Z.AI",
            "models": {
                "glm-4.5": { "id": "glm-4.5", "name": "GLM-4.5", "limit": { "context": 131072, "output": 98304 }, "modalities": { "input": ["text"] } },
                "glm-4.5v": { "id": "glm-4.5v", "name": "GLM-4.5V", "limit": { "context": 64000, "output": 16384 }, "modalities": { "input": ["text", "image", "video"] } }
            }
        },
        "zhipuai-coding-plan": {
            "name": "Zhipu AI Coding Plan",
            "models": {
                "glm-5.3-highspeed": { "id": "glm-5.3-highspeed", "name": "GLM-5.3 (highspeed)", "limit": { "context": 1000000, "output": 131072 }, "modalities": { "input": ["text"] } }
            }
        }
    }"#;

    /// zhipu coding 端点 `/v1/models` 的真实行形状（T13 实锤）：只有
    /// `id/object/created/owned_by`，没有任何窗口/模态/名字字段。
    fn zhipu_row(id: &str) -> VendorRow {
        VendorRow {
            id: id.to_string(),
            context_window: 0,
            max_output: 0,
            images: false,
            raw: serde_json::json!({ "id": id, "object": "model", "created": 1753632000u64, "owned_by": "z-ai" }),
        }
    }

    /// 厂商零元数据 + 目录有数：逐字段兜上，raw 一并回填（routes.rs 的
    /// model_row 读 raw.display_name / 行数值）。T13 缺口的主用例。
    #[test]
    fn vendor_missing_fields_get_preset_fallback() {
        let _s = sandbox();
        crate::engine_gateway::presets::install_test_catalog(CATALOG).expect("install catalog");
        let filled = Catalog::with_preset_fallback(zhipu_row("glm-4.5"));
        assert_eq!(filled.context_window, 131072);
        assert_eq!(filled.max_output, 98304);
        assert!(!filled.images, "glm-4.5 是纯文本模型");
        assert_eq!(
            filled.raw["context_window"].as_u64(),
            Some(131072),
            "raw 回填"
        );
        assert_eq!(filled.raw["max_output_tokens"].as_u64(), Some(98304));
        assert_eq!(filled.raw["display_name"].as_str(), Some("GLM-4.5"));
        // 视觉变体：厂商没说模态 → 目录说 image 就 image。
        let vision = Catalog::with_preset_fallback(zhipu_row("glm-4.5v"));
        assert!(vision.images);
        assert_eq!(vision.context_window, 64000);
        // coding plan 专属裸 id（provider id 带连字符不影响裸 id 命中）。
        let hs = Catalog::with_preset_fallback(zhipu_row("glm-5.3-highspeed"));
        assert_eq!(hs.context_window, 1000000);
        assert_eq!(hs.max_output, 131072);
    }

    /// 厂商显式报的值永远赢（Go Decorate：m.Context == 0 才认目录、
    /// ImageInput 非空就不动图片信息）；厂商起了名目录不覆盖。
    #[test]
    fn vendor_explicit_values_beat_catalog() {
        let _s = sandbox();
        crate::engine_gateway::presets::install_test_catalog(CATALOG).expect("install catalog");
        // 显式窗口/输出：再小也赢——改写成目录的「更漂亮」的数 = 编数。
        let explicit = Catalog::with_preset_fallback(VendorRow {
            id: "glm-4.5".into(),
            context_window: 4096,
            max_output: 512,
            images: false,
            raw: serde_json::json!({ "id": "glm-4.5", "context_window": 4096, "max_output_tokens": 512, "modalities": { "input": ["text"] } }),
        });
        assert_eq!(explicit.context_window, 4096);
        assert_eq!(explicit.max_output, 512);
        assert!(!explicit.images, "厂商明确 text-only，目录不得翻案");
        assert_eq!(
            explicit.raw["context_window"].as_u64(),
            Some(4096),
            "厂商值原样，不回填"
        );
        // 厂商显式报了 image：目录说纯文本也不动它。
        let said_image = Catalog::with_preset_fallback(VendorRow {
            id: "glm-4.5".into(),
            context_window: 131072,
            max_output: 98304,
            images: true,
            raw: serde_json::json!({ "id": "glm-4.5", "modalities": { "input": ["text", "image"] } }),
        });
        assert!(said_image.images);
        // 厂商起了名（name 形态）：抬进 display_name，不用目录的。
        let named = Catalog::with_preset_fallback(VendorRow {
            id: "glm-4.5".into(),
            context_window: 131072,
            max_output: 98304,
            images: false,
            raw: serde_json::json!({ "id": "glm-4.5", "name": "GLM 4.5 旗舰" }),
        });
        assert_eq!(named.raw["display_name"].as_str(), Some("GLM 4.5 旗舰"));
    }

    /// 两边都不知道：行保持 0/false，raw 不写占位 0——不编数。
    #[test]
    fn unknown_model_row_untouched() {
        let _s = sandbox();
        crate::engine_gateway::presets::install_test_catalog(CATALOG).expect("install catalog");
        let row = Catalog::with_preset_fallback(zhipu_row("mystery-model"));
        assert_eq!(row.context_window, 0);
        assert_eq!(row.max_output, 0);
        assert!(!row.images);
        assert!(row.raw.get("context_window").is_none(), "不知道就不写 0");
        assert!(row.raw.get("display_name").is_none());
    }

    /// 解析 + 兜底全链：zhipu 形状解析出 0 行值 → 兜底把认识的补上、
    /// 目录也没有的（glm-4.6）如实保持 0。
    #[test]
    fn parse_rows_zhipu_shape_then_fallback() {
        let _s = sandbox();
        crate::engine_gateway::presets::install_test_catalog(CATALOG).expect("install catalog");
        let body = r#"{"data":[
            {"id":"glm-4.5","object":"model","created":1753632000,"owned_by":"z-ai"},
            {"id":"glm-4.6","object":"model","created":1753632000,"owned_by":"z-ai"}
        ]}"#;
        let rows = parse_rows(body);
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .all(|r| r.context_window == 0 && r.max_output == 0),
            "zhipu 行解析出来就是零元数据"
        );
        let filled: Vec<VendorRow> = rows
            .into_iter()
            .map(Catalog::with_preset_fallback)
            .collect();
        assert_eq!(filled[0].context_window, 131072, "glm-4.5 由 presets 兜上");
        assert_eq!(filled[0].max_output, 98304);
        assert_eq!(filled[1].context_window, 0, "目录也没有 = 不知道，不编数");
        assert_eq!(filled[1].max_output, 0);
    }
}
