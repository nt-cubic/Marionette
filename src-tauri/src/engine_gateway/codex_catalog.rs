//! `GET /v1/codex/models`——Codex 自己形状的模型目录（Go `codex_catalog.go`，
//! 审计 #6）。`/v1/models` 是 OpenAI 的列表，Codex 读不了；Codex 把某个来源
//! 配成 `model_catalog_url` 时读的是这个：`{"models":[ModelInfo…]}`。
//!
//! 与 Go 的差距（有意，见各处注释）：Go 的 `codexcat.Entries` 还会从用户本机
//! Codex 的官方缓存里挑「同名模型的官方 prompt / model_messages / 服务档」、按
//! 设置里的「压缩阈值」调 `auto_compact_token_limit`、对 DeepSeek 模型套
//! `shell_command` 等专属项——这些依赖 magpie 的 settings 与 Codex 缓存，Rust
//! 引擎没有对应物。这里只出 Go「通用条目」那条路径的字段（`Entries` 里没有
//! 官方缓存命中、也没有厂商自带 Codex 元数据的模型走的 `model{…}` 字面量）：
//! Codex 要求的字段全带，prompt 用 vendored `codex_prompt.md`（Codex 对未知
//! 模型的默认 prompt，Go `codexcat.Prompt` 同一份文件）。
//!
//! 体积护栏同 Go：Codex 0.166 之前对 `model_catalog_url` 的应答只读 1MiB，超了
//! 整张表加载失败（选择器里一个 magpie 模型都没有）——`?client_version=` < 0.166
//! 或缺省时从尾部丢条目并在 `X-Magpie-Left-Out` 头里报数；≥0.166 整表返回。
use serde_json::{json, Value};

use super::catalog::VendorRow;

/// Codex 对未知模型的默认 prompt（与 Go `codexcat.Prompt` 同一份 vendored 文件）。
const PROMPT: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../third_party/magpie/internal/codexcat/codex_prompt.md"
));

/// Codex 读 model_catalog_url 的字节上限（codex-rs MAX_MODEL_CATALOG_BYTES）。
const CATALOG_MOST: usize = 1 << 20;

/// Codex 的 ReasoningEffort 词表（其余厂商私有档位词 Codex 反序列化会失败，
/// 整表加载挂掉——不能原样透传）。
const CODEX_EFFORTS: [&str; 6] = ["none", "minimal", "low", "medium", "high", "xhigh"];

/// Go `codexReadsWholeCatalog`：`?client_version=` 为 0.166 及以后 → 整表读。
pub fn reads_whole_catalog(client_version: &str) -> bool {
    let mut parts = client_version.trim().split('.');
    let major = parts.next().and_then(|p| p.parse::<u64>().ok());
    let minor = parts
        .next()
        .and_then(|p| p.split(['-', '+']).next())
        .and_then(|p| p.parse::<u64>().ok());
    match (major, minor) {
        (Some(major), Some(minor)) => major > 0 || minor >= 166,
        _ => false,
    }
}

/// 一个来源模型 → Codex ModelInfo 条目（Go `Entries` 通用分支）。
fn entry(provider_id: &str, row: &VendorRow, priority: usize) -> Value {
    let id = format!("{provider_id}/{}", row.id);
    let name = row
        .raw
        .get("display_name")
        .and_then(|d| d.as_str())
        .filter(|d| !d.trim().is_empty())
        .unwrap_or(&row.id)
        .to_string();
    let mut efforts: Vec<String> = Vec::new();
    for key in [
        "reasoning_levels",
        "supported_reasoning_levels",
        "reasoning_efforts",
        "efforts",
    ] {
        if let Some(list) = row.raw.get(key).and_then(|v| v.as_array()) {
            for item in list {
                let word = item
                    .as_str()
                    .map(String::from)
                    .or_else(|| {
                        item.get("effort")
                            .and_then(|x| x.as_str())
                            .map(String::from)
                    })
                    .or_else(|| item.get("id").and_then(|x| x.as_str()).map(String::from));
                if let Some(w) = word {
                    let w = w.to_ascii_lowercase();
                    if CODEX_EFFORTS.contains(&w.as_str()) && !efforts.contains(&w) {
                        efforts.push(w);
                    }
                }
            }
            if !efforts.is_empty() {
                break;
            }
        }
    }
    let default_effort = if efforts.iter().any(|e| e == "medium") {
        Value::String("medium".into())
    } else {
        efforts
            .first()
            .map(|e| Value::String(e.clone()))
            .unwrap_or(Value::Null)
    };
    let mut modalities = vec!["text"];
    let description = if row.images {
        modalities.push("image");
        format!("{name} via magpie")
    } else {
        // 与 Go 同款：Codex 自己对粘贴的图说「该模型不收图」，说明里指路
        format!("{name} (text only) via magpie")
    };
    let mut e = json!({
        "slug": id,
        "display_name": name,
        "description": description,
        "base_instructions": PROMPT,
        "default_reasoning_level": default_effort,
        "supported_reasoning_levels": efforts
            .iter()
            .map(|e| json!({ "effort": e, "description": "" }))
            .collect::<Vec<_>>(),
        "shell_type": "unified_exec",
        "visibility": "list",
        "supported_in_api": true,
        "priority": priority,
        "support_verbosity": false,
        "default_verbosity": null,
        "apply_patch_tool_type": "freeform",
        "truncation_policy": { "mode": "tokens", "limit": 10000 },
        "experimental_supported_tools": [],
        "input_modalities": modalities,
        "effective_context_window_percent": 95,
        "service_tiers": [],
        "supports_search_tool": true,
        "supports_parallel_tool_calls": true,
        // Codex 0.144 之前的必填项：缺了整张表加载失败（Go 注释 #1450）
        "supports_reasoning_summaries": !efforts.is_empty(),
    });
    if row.context_window > 0 {
        e["context_window"] = json!(row.context_window);
    }
    e
}

/// 整张目录应答体 + 被丢的条目数。`rows` = (来源 id, 目录行)，按给定顺序出。
pub fn build(rows: &[(String, VendorRow)], client_version: &str) -> (Vec<u8>, usize) {
    let most = if reads_whole_catalog(client_version) {
        usize::MAX
    } else {
        CATALOG_MOST
    };
    let head = b"{\"models\":[";
    let tail = b"]}\n";
    let mut body: Vec<u8> = head.to_vec();
    let (mut kept, mut left_out) = (0usize, 0usize);
    for (i, (provider_id, row)) in rows.iter().enumerate() {
        let raw = serde_json::to_vec(&entry(provider_id, row, i + 1)).unwrap_or_default();
        if raw.is_empty() || body.len().saturating_add(1 + raw.len() + tail.len()) > most {
            left_out += 1;
            continue;
        }
        if kept > 0 {
            body.push(b',');
        }
        body.extend_from_slice(&raw);
        kept += 1;
    }
    body.extend_from_slice(tail);
    (body, left_out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: &str, window: u64, images: bool, raw: Value) -> VendorRow {
        VendorRow {
            id: id.into(),
            context_window: window,
            max_output: 0,
            images,
            raw,
        }
    }

    #[test]
    fn client_version_gate() {
        assert!(reads_whole_catalog("0.166.0"));
        assert!(reads_whole_catalog("0.170.2"));
        assert!(reads_whole_catalog("1.0.0"));
        assert!(!reads_whole_catalog("0.165.9"));
        assert!(!reads_whole_catalog(""));
        assert!(!reads_whole_catalog("garbage"));
    }

    #[test]
    fn entry_has_every_field_codex_requires() {
        let r = row(
            "deepseek-v4",
            1_000_000,
            false,
            json!({ "display_name": "DeepSeek V4", "reasoning_levels": ["low", "high", "max"] }),
        );
        let e = entry("ds", &r, 3);
        assert_eq!(e["slug"], "ds/deepseek-v4");
        assert_eq!(e["display_name"], "DeepSeek V4");
        assert_eq!(e["context_window"], 1_000_000);
        assert_eq!(e["priority"], 3);
        // 私有档位词 max 被丢（Codex 不认），low/high 保留；没有 medium 取首档
        assert_eq!(e["supported_reasoning_levels"].as_array().unwrap().len(), 2);
        assert_eq!(e["default_reasoning_level"], "low");
        assert_eq!(e["supports_reasoning_summaries"], true);
        assert_eq!(e["input_modalities"], json!(["text"]));
        assert!(e["description"].as_str().unwrap().contains("text only"));
        assert!(e["base_instructions"].as_str().unwrap().len() > 1000);
        for key in [
            "shell_type",
            "visibility",
            "supported_in_api",
            "apply_patch_tool_type",
            "truncation_policy",
            "experimental_supported_tools",
            "service_tiers",
            "supports_search_tool",
            "supports_parallel_tool_calls",
            "support_verbosity",
            "effective_context_window_percent",
        ] {
            assert!(e.get(key).is_some(), "missing {key}");
        }
        // 能看图 + 无档位
        let r = row("vision", 0, true, json!({}));
        let e = entry("p", &r, 1);
        assert_eq!(e["input_modalities"], json!(["text", "image"]));
        assert!(e.get("context_window").is_none());
        assert_eq!(e["default_reasoning_level"], Value::Null);
        assert_eq!(e["supports_reasoning_summaries"], false);
    }

    #[test]
    fn old_codex_gets_a_catalog_under_1mib_and_new_codex_gets_all() {
        let rows: Vec<(String, VendorRow)> = (0..80)
            .map(|i| {
                (
                    "p".to_string(),
                    row(&format!("m{i}"), 128_000, true, json!({})),
                )
            })
            .collect();
        // 每条带 ~21KB prompt：80 条 > 1MiB
        let (old, left) = build(&rows, "0.150.0");
        assert!(old.len() <= CATALOG_MOST, "{}", old.len());
        assert!(left > 0);
        let v: Value = serde_json::from_slice(&old).expect("valid json");
        assert_eq!(v["models"].as_array().unwrap().len() + left, 80);
        let (new, left) = build(&rows, "0.166.0");
        assert_eq!(left, 0);
        assert!(new.len() > CATALOG_MOST);
        let v: Value = serde_json::from_slice(&new).unwrap();
        assert_eq!(v["models"].as_array().unwrap().len(), 80);
        // 缺省 client_version 按老 Codex 对待
        assert!(build(&rows, "").1 > 0);
    }

    #[test]
    fn empty_catalog_is_an_empty_list() {
        let (body, left) = build(&[], "");
        assert_eq!(body, b"{\"models\":[]}\n".to_vec());
        assert_eq!(left, 0);
    }
}
