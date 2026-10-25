//! ZCode stdio MCP 桥（2026-10-25）。
//!
//! zcode-acp-server 的 `session/new` schema 只收 url 型 MCP——stdio 条目直接
//! -32602（见 AGENTS.md ZCode 节），所以 ACP 借贷永远送不进 zcode 会话。但
//! ZCode 后端自己会加载 `~/.zcode/cli/config.json` 的 `mcp.servers`（stdio:
//! `type/command/args/cwd/env`，zod strict——zcode.cjs 的 `Nso/pJs/Bpt`），
//! 适配器的 `/mcp` 展示读的也是同一个文件。于是走 model_bridge 同款路线：
//! 「能力与上下文」里勾选借贷的**全局 stdio MCP**，除照常经 ACP 借给支持的
//! agent 外，还合并进 zcode 自己的 config.json——桌面版与 Marionette 里的
//! zcode 会话都原生加载，stdio 进程和 env 由 zcode 自己拉起。
//!
//! 归属记账：条目 schema 是 strict 的塞不进标记键，「哪些条目是我们写的」
//! 记在 `~/.marionette/zcode-mcp-bridge.json`。同步只增删改自己名下的条目，
//! 用户自己在 zcode 里配的 server 永不触碰；与 zcode 插件 MCP 同名的条目
//! 整个跳过（插件由后端自己加载，写进 config 只会双开）。借贷选择是
//! per-project 的，zcode config 却是机器级文件——取所有项目勾选的并集：
//! 「所有 agent 有 MCP」语义下，一处勾选、处处可用。

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::context_inventory::{self, McpServerSpec};

/// `~/.zcode/cli/config.json` — zcode 后端的用户配置（MCP 就从这里加载）。
fn zcode_config_path() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(PathBuf::from(home).join(".zcode").join("cli").join("config.json"))
}

/// 归属记账 sidecar（config.json 本体放不下标记键）。
fn sidecar_path() -> Result<PathBuf, String> {
    Ok(crate::app_paths::global_dir()?.join("zcode-mcp-bridge.json"))
}

/// 纯筛选（测试直喂夹具）：勾选集合里、全局来源（非 project 层）、带启动
/// 命令的 stdio 条目。project 层的 server 不进机器级文件——那会把单个项目
/// 的选择泄漏给所有 zcode 会话。
pub fn wanted_from(servers: &[McpServerSpec], enabled: &HashSet<String>) -> Vec<McpServerSpec> {
    servers
        .iter()
        .filter(|spec| enabled.contains(&spec.id))
        .filter(|spec| spec.transport == "stdio" && spec.command.is_some())
        .filter(|spec| spec.sources.iter().any(|source| source != "project"))
        .cloned()
        .collect()
}

/// 所有项目借贷勾选的并集 → 想装进 zcode 的 server 集合。
/// 扫描用第一个项目根（全局源与根无关，project 层结果由 `wanted_from` 过滤）。
fn wanted_servers(project_roots: &[PathBuf]) -> Vec<McpServerSpec> {
    let Some(first) = project_roots.first() else {
        return Vec::new();
    };
    let inventory = context_inventory::scan(first);
    let mut enabled: HashSet<String> = HashSet::new();
    for root in project_roots {
        let selection = context_inventory::load_selection(root);
        for spec in &inventory.mcp_servers {
            if selection.is_enabled("mcp", &spec.id) {
                enabled.insert(spec.id.clone());
            }
        }
    }
    wanted_from(&inventory.mcp_servers, &enabled)
}

/// zcode 插件缓存里所有 `.mcp.json` 声明的 server 名（flat 与 mcpServers 两种
/// 形态都认）。后端对启用的插件自己加载 MCP——同名条目写进 config.json 会
/// 双开，所以整个跳过。保守起见不分插件启用与否，全部算冲突。
fn zcode_plugin_mcp_names(plugin_cache: &Path) -> HashSet<String> {
    let mut names = HashSet::new();
    let mut stack = vec![(plugin_cache.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if depth > 4 {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if path.is_dir() {
                stack.push((path, depth + 1));
            } else if file_name == ".mcp.json" {
                collect_mcp_json_names(&path, &mut names);
            }
        }
    }
    names
}

fn collect_mcp_json_names(path: &Path, names: &mut HashSet<String>) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(value) = serde_json::from_str::<Value>(&raw) else {
        return;
    };
    let Some(map) = value
        .get("mcpServers")
        .and_then(Value::as_object)
        .or_else(|| value.as_object())
    else {
        return;
    };
    for name in map.keys() {
        names.insert(name.clone());
    }
}

/// zcode 条目（schema strict，只写 schema 认识的键；env 缺省不写空表）。
fn zcode_server_entry(spec: &McpServerSpec, env: &BTreeMap<String, String>) -> Value {
    let mut entry = Map::new();
    entry.insert("type".into(), json!("stdio"));
    entry.insert(
        "command".into(),
        json!(spec.command.clone().unwrap_or_default()),
    );
    entry.insert("args".into(), json!(spec.args));
    if !env.is_empty() {
        let env_map: Map<String, Value> = env
            .iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect();
        entry.insert("env".into(), Value::Object(env_map));
    }
    Value::Object(entry)
}

fn sidecar_record(spec: &McpServerSpec) -> Value {
    json!({
        "command": spec.command,
        "args": spec.args,
        "sources": spec.sources,
        "sourcePaths": spec.source_paths,
    })
}

/// sidecar 里记的归属名。损坏的 sidecar 当空集处理（自愈），但绝不能把
/// 「无归属」误当「有归属」去删用户条目——空集语义下只装不删，是安全的。
fn owned_names_from_sidecar(raw: Option<&str>) -> BTreeMap<String, Value> {
    let Some(raw) = raw else {
        return BTreeMap::new();
    };
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return BTreeMap::new();
    };
    value
        .get("servers")
        .and_then(Value::as_object)
        .map(|servers| servers.clone().into_iter().collect())
        .unwrap_or_default()
}

fn sidecar_text(owned: &BTreeMap<String, Value>) -> String {
    let body = serde_json::to_string_pretty(&json!({
        "version": 1,
        "servers": owned,
    }))
    .unwrap_or_else(|_| "{\"version\":1,\"servers\":{}}".to_string());
    format!("{body}\n")
}

/// 纯合并核心（测试直喂夹具）。只动 `mcp.servers` 里我们名下的条目：
/// 想装的装上/换血，不再想要的拆走；用户自己的条目原样保留，同名且非我们
/// 名下的跳过（`skipped`）。原文存在但不是合法 JSON object 时报错——宁可不
/// 同步也不整体覆盖可能救得回来的用户配置。返回 (新文本, 新归属, 跳过, 是否变更)。
fn merge_zcode_config(
    existing: Option<&str>,
    wanted: &[McpServerSpec],
    envs: &BTreeMap<String, BTreeMap<String, String>>,
    owned: &BTreeMap<String, Value>,
) -> Result<(String, BTreeMap<String, Value>, Vec<String>, bool), String> {
    let mut root: Map<String, Value> = match existing {
        Some(text) => serde_json::from_str::<Value>(text)
            .map_err(|error| format!("config.json 不是合法 JSON（跳过同步）: {error}"))?
            .as_object()
            .cloned()
            .ok_or_else(|| "config.json 顶层不是 JSON object（跳过同步）".to_string())?,
        None => Map::new(),
    };
    let before = serde_json::to_value(&root).unwrap_or(Value::Null);

    // mcp 键缺失 ≠ 损坏：正常补一个空层。只有「存在但不是 object」才报错。
    let mcp_missing = !root.contains_key("mcp");
    let mut mcp = if mcp_missing {
        Map::new()
    } else {
        root.remove("mcp")
            .and_then(|value| value.as_object().cloned())
            .ok_or_else(|| "config.json 的 mcp 键存在但不是 object（跳过同步）".to_string())?
    };
    let mut servers: Map<String, Value> = mcp
        .remove("servers")
        .map(|value| {
            value
                .as_object()
                .cloned()
                .ok_or_else(|| "config.json 的 mcp.servers 不是 object（跳过同步）".to_string())
        })
        .transpose()?
        .unwrap_or_default();

    let mut new_owned: BTreeMap<String, Value> = BTreeMap::new();
    let mut skipped: Vec<String> = Vec::new();
    for spec in wanted {
        if !owned.contains_key(&spec.name) && servers.contains_key(&spec.name) {
            skipped.push(spec.name.clone());
            continue;
        }
        let env = envs.get(&spec.name).cloned().unwrap_or_default();
        servers.insert(spec.name.clone(), zcode_server_entry(spec, &env));
        new_owned.insert(spec.name.clone(), sidecar_record(spec));
    }
    // 拆掉我们装过、但不再被任何项目勾选的条目。
    for name in owned.keys() {
        if !new_owned.contains_key(name) {
            servers.remove(name);
        }
    }

    mcp.insert("servers".into(), Value::Object(servers));
    root.insert("mcp".into(), Value::Object(mcp));
    let changed = serde_json::to_value(&root).unwrap_or(Value::Null) != before;
    let text = serde_json::to_string_pretty(&root)
        .map_err(|error| format!("Serialize zcode config failed: {error}"))?;
    Ok((format!("{text}\n"), new_owned, skipped, changed))
}

pub struct SyncReport {
    pub installed: Vec<String>,
    pub removed: Vec<String>,
    pub skipped: Vec<String>,
}

impl SyncReport {
    fn is_quiet(&self) -> bool {
        self.installed.is_empty() && self.removed.is_empty() && self.skipped.is_empty()
    }
}

/// 主入口：把所有项目勾选借贷的全局 stdio MCP 同步进 `~/.zcode/cli/config.json`。
/// 幂等；无变更不落盘（zcode 不必重读配置）。记 debug log 供排查。
pub fn sync(project_roots: &[PathBuf]) -> Result<SyncReport, String> {
    let Some(config_path) = zcode_config_path() else {
        return Err("无法定位用户目录".to_string());
    };
    let sidecar = sidecar_path()?;

    let mut wanted = wanted_servers(project_roots);
    // 与 zcode 插件 MCP 同名 → 整个跳过（后端对插件自己加载，写 config 双开）。
    let plugin_names = zcode_config_path()
        .and_then(|path| path.parent().map(|p| p.to_path_buf()))
        .map(|cli_dir| zcode_plugin_mcp_names(&cli_dir.join("plugins").join("cache")))
        .unwrap_or_default();
    let mut skipped: Vec<String> = Vec::new();
    wanted.retain(|spec| {
        if plugin_names.contains(&spec.name) {
            skipped.push(spec.name.clone());
            false
        } else {
            true
        }
    });

    // env 值在注入时现读源配置（同 mcp_payload_for_agent 的口径）。
    let mut envs: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for spec in &wanted {
        let env: BTreeMap<String, String> = context_inventory::env_values_for(spec)
            .into_iter()
            .collect();
        envs.insert(spec.name.clone(), env);
    }

    let existing = std::fs::read_to_string(&config_path).ok();
    let sidecar_raw = std::fs::read_to_string(&sidecar).ok();
    let owned = owned_names_from_sidecar(sidecar_raw.as_deref());
    let (text, new_owned, mut skipped_native, changed) =
        merge_zcode_config(existing.as_deref(), &wanted, &envs, &owned)?;
    skipped.append(&mut skipped_native);

    let old_names: HashSet<&String> = owned.keys().collect();
    let new_names: HashSet<&String> = new_owned.keys().collect();
    let report = SyncReport {
        installed: new_names
            .iter()
            .filter(|name| !old_names.contains(*name))
            .map(|name| (*name).clone())
            .collect(),
        removed: old_names
            .iter()
            .filter(|name| !new_names.contains(*name))
            .map(|name| (*name).clone())
            .collect(),
        skipped: skipped.clone(),
    };

    if changed || new_owned != owned {
        crate::storage::write_atomic(&config_path, &text)?;
        crate::storage::write_atomic(&sidecar, &sidecar_text(&new_owned))?;
        if !report.is_quiet() {
            crate::debug_log::append(
                "bridge",
                "info",
                "",
                "zcode stdio mcp bridge synced",
                Some(&format!(
                    "installed={:?} removed={:?} skipped={:?}",
                    report.installed, report.removed, report.skipped
                )),
            );
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, command: &str) -> McpServerSpec {
        McpServerSpec {
            id: name.to_lowercase(),
            name: name.to_string(),
            transport: "stdio".into(),
            command: Some(command.to_string()),
            args: vec!["-y".to_string(), format!("{name}@latest")],
            env_keys: vec!["TOKEN".to_string()],
            header_keys: Vec::new(),
            url: None,
            sources: vec!["grok-build".to_string()],
            source_paths: vec![r"C:\fake\.grok\config.toml".to_string()],
            agents: vec!["grok-build".to_string()],
        }
    }

    fn owned_map(names: &[&str]) -> BTreeMap<String, Value> {
        names
            .iter()
            .map(|name| (name.to_string(), json!({ "command": "x" })))
            .collect()
    }

    #[test]
    fn wanted_filters_to_enabled_global_stdio() {
        let mut http = spec("remote", "cmd");
        http.transport = "http".into();
        http.url = Some("https://x.dev/mcp".into());
        http.command = None;
        let mut project_only = spec("local", "cmd");
        project_only.sources = vec!["project".to_string()];
        let mut no_command = spec("bare", "cmd");
        no_command.command = None;

        let servers = vec![spec("notion", "cmd"), http, project_only, no_command];
        let enabled: HashSet<String> = ["notion".into(), "remote".into(), "local".into(), "bare".into()]
            .into_iter()
            .collect();

        let wanted = wanted_from(&servers, &enabled);
        let names: Vec<&str> = wanted.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["notion"]);
    }

    #[test]
    fn merge_installs_stdio_entry_with_env() {
        let envs: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::from([(
            "notion".to_string(),
            BTreeMap::from([("NOTION_TOKEN".to_string(), "ntn_x".to_string())]),
        )]);
        let (text, owned, skipped, changed) =
            merge_zcode_config(None, &[spec("notion", "cmd.exe")], &envs, &BTreeMap::new())
                .unwrap();
        assert!(changed);
        assert!(skipped.is_empty());
        assert!(owned.contains_key("notion"));
        let value: Value = serde_json::from_str(&text).unwrap();
        let entry = &value["mcp"]["servers"]["notion"];
        assert_eq!(entry["type"], "stdio");
        assert_eq!(entry["command"], "cmd.exe");
        assert_eq!(entry["args"], json!(["-y", "notion@latest"]));
        assert_eq!(entry["env"]["NOTION_TOKEN"], "ntn_x");
    }

    #[test]
    fn merge_preserves_user_entries_and_removes_stale_owned() {
        let existing = r#"{
          "plugins": { "enabledPlugins": { "p@market": true } },
          "mcp": {
            "servers": {
              "my-own": { "type": "stdio", "command": "mine.exe" },
              "rea": { "type": "stdio", "command": "stale.exe" }
            }
          }
        }"#;
        let (text, owned, _, changed) = merge_zcode_config(
            Some(existing),
            &[spec("notion", "cmd.exe")],
            &BTreeMap::new(),
            &owned_map(&["rea"]),
        )
        .unwrap();
        assert!(changed);
        let value: Value = serde_json::from_str(&text).unwrap();
        // 用户条目原样保留，插件层不动，过期的托管条目拆走，新条目装上。
        assert_eq!(value["mcp"]["servers"]["my-own"]["command"], "mine.exe");
        assert!(value["mcp"]["servers"]["rea"].is_null());
        assert!(value["mcp"]["servers"]["notion"].is_object());
        assert_eq!(value["plugins"]["enabledPlugins"]["p@market"], true);
        assert_eq!(owned.len(), 1);
    }

    #[test]
    fn merge_skips_unowned_name_collision() {
        let existing = r#"{ "mcp": { "servers": { "notion": { "type": "stdio", "command": "user.exe" } } } }"#;
        let (text, owned, skipped, changed) = merge_zcode_config(
            Some(existing),
            &[spec("notion", "cmd.exe")],
            &BTreeMap::new(),
            &BTreeMap::new(),
        )
        .unwrap();
        assert!(!changed);
        assert_eq!(skipped, vec!["notion".to_string()]);
        assert!(owned.is_empty());
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["mcp"]["servers"]["notion"]["command"], "user.exe");
    }

    #[test]
    fn merge_is_idempotent() {
        let envs: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::from([(
            "notion".to_string(),
            BTreeMap::from([("NOTION_TOKEN".to_string(), "ntn_x".to_string())]),
        )]);
        let wanted = [spec("notion", "cmd.exe")];
        let (first, owned, _, _) =
            merge_zcode_config(None, &wanted, &envs, &BTreeMap::new()).unwrap();
        let (second, owned_again, _, changed) =
            merge_zcode_config(Some(&first), &wanted, &envs, &owned).unwrap();
        assert!(!changed);
        assert_eq!(first, second);
        assert_eq!(owned, owned_again);
    }

    #[test]
    fn merge_bails_on_invalid_json() {
        let result = merge_zcode_config(
            Some("{ not json"),
            &[spec("notion", "cmd.exe")],
            &BTreeMap::new(),
            &BTreeMap::new(),
        );
        assert!(result.is_err());
    }

    #[test]
    fn plugin_names_scan_reads_both_shapes() {
        let root = std::env::temp_dir().join(format!(
            "marionette-zcode-mcp-plugins-{}",
            std::process::id()
        ));
        let flat = root.join("market/p/1.0.0");
        let nested = root.join("market/q/2.0.0");
        std::fs::create_dir_all(&flat).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(flat.join(".mcp.json"), r#"{ "tools": { "command": "x" } }"#).unwrap();
        std::fs::write(
            nested.join(".mcp.json"),
            r#"{ "mcpServers": { "deep": { "command": "y" } } }"#,
        )
        .unwrap();

        let names = zcode_plugin_mcp_names(&root);
        assert!(names.contains("tools"));
        assert!(names.contains("deep"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn sidecar_roundtrip_and_corrupt_treated_as_empty() {
        let owned = owned_map(&["notion", "rea"]);
        let text = sidecar_text(&owned);
        assert_eq!(owned_names_from_sidecar(Some(&text)), owned);
        assert!(owned_names_from_sidecar(Some("{ broken")).is_empty());
        assert!(owned_names_from_sidecar(None).is_empty());
    }
}
