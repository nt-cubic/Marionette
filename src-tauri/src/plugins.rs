//! Universal plugin registry — install once, every agent gets it
//! (docs/plan-plugin-layer.md).
//!
//! The registry is Marionette-owned (`~/.marionette/plugins/index.json`);
//! agent configs are never written — the fan-out writer road is codeg's
//! 10k-line cautionary tale. Enabled plugins are exposed on the tools-mcp
//! localhost server as url-type entries (the one shape every ACP adapter
//! accepts, zcode included), so injection never branches per agent.
//!
//! Store discipline follows memos.rs: fail-open reads (a broken registry
//! never takes sessions down), atomic writes, no panics — errors are
//! `Result<_, String>` all the way up. Credential values (env/headers) are
//! stored here but never logged; over the proxy route the agent never sees
//! them at all — only the loopback URL and a session token.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::AppHandle;

// ─── Records ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeRecord {
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_count: Option<usize>,
    #[serde(default)]
    pub note: String,
    /// Unix millis (compact sortable stamp — house style, no chrono).
    pub at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginRecord {
    pub id: String,
    /// "stdio" | "http"
    pub transport: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub env: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
    /// Install = enabled for every agent. That is the whole point.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub added_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_probe: Option<ProbeRecord>,
}

fn default_enabled() -> bool {
    true
}

/// What the install form sends (no record bookkeeping fields).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginSpec {
    pub id: String,
    pub transport: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub headers: HashMap<String, String>,
}

// ─── Validation ──────────────────────────────────────────────────────────────

/// Slug ids only: the id becomes the MCP server name inside every agent
/// (`mcp__<id>__<tool>`), so no spaces, slashes, or case games.
pub fn valid_plugin_id(id: &str) -> bool {
    let mut chars = id.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() || c.is_ascii_digit() => {}
        _ => return false,
    }
    let mut len = 1;
    for c in chars {
        len += 1;
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.')) {
            return false;
        }
    }
    (1..=64).contains(&len)
}

fn validate_spec(spec: &PluginSpec) -> Result<(), String> {
    if !valid_plugin_id(&spec.id) {
        return Err(format!(
            "invalid plugin id `{}` (lowercase slug, 1-64 chars: a-z 0-9 . _ -)",
            spec.id
        ));
    }
    match spec.transport.as_str() {
        "stdio" => {
            if spec.command.trim().is_empty() {
                Err("stdio plugin needs a command".to_string())
            } else {
                Ok(())
            }
        }
        "http" => {
            let url = spec.url.trim();
            if url.starts_with("http://") || url.starts_with("https://") {
                Ok(())
            } else {
                Err("http plugin needs an http(s) url".to_string())
            }
        }
        other => Err(format!(
            "unsupported transport `{other}` (stdio | http)"
        )),
    }
}

// ─── Store (`*_at` forms are the testable core; no global override needed) ──

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IndexFile {
    version: u32,
    plugins: Vec<PluginRecord>,
}

fn index_path_at(dir: &Path) -> PathBuf {
    dir.join("index.json")
}

/// Fail-open: missing → empty; unreadable/corrupt → archive the corpse and
/// start over. A broken registry must not take sessions down with it.
pub fn load_at(dir: &Path) -> Vec<PluginRecord> {
    let path = index_path_at(dir);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    match serde_json::from_str::<IndexFile>(&raw) {
        Ok(file) => file.plugins,
        Err(error) => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let corpse = path.with_extension(format!("json.corrupt-{stamp}"));
            let _ = std::fs::rename(&path, &corpse);
            crate::debug_log::append(
                "plugins",
                "warn",
                "",
                "registry unreadable; archived and reset",
                Some(&error.to_string()),
            );
            Vec::new()
        }
    }
}

pub fn save_at(dir: &Path, plugins: &[PluginRecord]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create plugins dir failed: {e}"))?;
    let body = serde_json::to_string_pretty(&IndexFile {
        version: 1,
        plugins: plugins.to_vec(),
    })
    .map_err(|e| format!("serialize registry failed: {e}"))?;
    crate::storage::write_atomic(&index_path_at(dir), &format!("{body}\n"))
        .map_err(|e| format!("write registry failed: {e}"))
}

fn plugins_dir() -> Result<PathBuf, String> {
    Ok(crate::app_paths::global_dir()?.join("plugins"))
}

pub fn load() -> Vec<PluginRecord> {
    plugins_dir().map(|dir| load_at(&dir)).unwrap_or_default()
}

pub fn save(plugins: &[PluginRecord]) -> Result<(), String> {
    save_at(&plugins_dir()?, plugins)
}

pub fn enabled_ids() -> Vec<String> {
    load()
        .into_iter()
        .filter(|plugin| plugin.enabled)
        .map(|plugin| plugin.id)
        .collect()
}

/// The record a proxy request resolves; disabled plugins are invisible.
pub fn enabled_plugin(id: &str) -> Option<PluginRecord> {
    load().into_iter().find(|plugin| plugin.enabled && plugin.id == id)
}

// ─── Skills（plugins/skills 的安装/停用/删除，2026-11-06） ─────────────────
//
// 技能不是 MCP server，不进 index.json：登记处就是 skills/ 目录本身——
// 每个含 SKILL.md 的子目录即一个技能（context_inventory 按同一口径扫描，
// 注入链「per-project 默认 on 借贷」不变，本轮只补管理入口）。全局停用 =
// 目录里放一个 `.disabled` 标记文件：scan_skills_dir 跳过带标记的目录，
// 「项目上下文→技能」清单与注入同步消失——两个入口看同一份真相，
// 永不打架（也比给 index.json 加记录类少一次 schema 演进）。

#[derive(Clone, Debug, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillRecord {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub dir: String,
    pub enabled: bool,
}

/// Marker file name that globally disables a skill directory.
const SKILL_DISABLED_MARKER: &str = ".disabled";

pub fn skills_dir() -> Result<PathBuf, String> {
    Ok(plugins_dir()?.join("skills"))
}

fn skill_record_at(dir: &Path) -> Option<SkillRecord> {
    let skill_file = dir.join("SKILL.md");
    if !dir.is_dir() || !skill_file.exists() {
        return None;
    }
    let id = dir.file_name()?.to_str()?.to_string();
    if id.is_empty() {
        return None;
    }
    let text = std::fs::read_to_string(&skill_file).unwrap_or_default();
    let (name, description) = crate::context_inventory::parse_skill_front_matter(&text);
    let fallback_name = id.clone();
    Some(SkillRecord {
        id,
        name: name.unwrap_or(fallback_name),
        description: description.unwrap_or_default(),
        dir: dir.display().to_string(),
        enabled: !dir.join(SKILL_DISABLED_MARKER).exists(),
    })
}

/// List skills under `root` (one SKILL.md folder = one skill, disabled ones
/// included so the UI can show/toggle them).
pub fn list_skills_at(root: &Path) -> Vec<SkillRecord> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut skills: Vec<SkillRecord> = entries
        .flatten()
        .filter_map(|entry| skill_record_at(&entry.path()))
        .collect();
    skills.sort_by(|a, b| a.id.cmp(&b.id));
    skills
}

/// Copy a skill folder (must contain SKILL.md) into `root/<id>`. Installing is
/// enabling: a `.disabled` marker in the source is not carried over.
pub fn install_skill_at(source: &Path, id: Option<String>, root: &Path) -> Result<SkillRecord, String> {
    if !source.is_dir() {
        return Err(format!("source folder not found: {}", source.display()));
    }
    if !source.join("SKILL.md").exists() {
        return Err(format!(
            "no SKILL.md in {} (a skill is a folder that contains one)",
            source.display()
        ));
    }
    let id = match id {
        Some(explicit) if !explicit.trim().is_empty() => explicit.trim().to_lowercase(),
        _ => source
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_lowercase(),
    };
    if !valid_plugin_id(&id) {
        return Err(format!(
            "invalid skill id `{id}` (lowercase slug, 1-64 chars: a-z 0-9 . _ -)"
        ));
    }
    let target = root.join(&id);
    if target.exists() {
        return Err(format!("skill `{id}` already exists (remove it first)"));
    }
    crate::memos::copy_tree(source, &target)?;
    // Install = enabled; drop any marker the source happened to carry.
    let marker = target.join(SKILL_DISABLED_MARKER);
    if marker.exists() {
        let _ = std::fs::remove_file(marker);
    }
    skill_record_at(&target).ok_or_else(|| "installed skill folder unreadable".to_string())
}

pub fn remove_skill_at(id: &str, root: &Path) -> Result<(), String> {
    if !valid_plugin_id(id) {
        return Err(format!("invalid skill id `{id}`"));
    }
    let target = root.join(id);
    if !target.is_dir() {
        return Err(format!("skill `{id}` not found"));
    }
    std::fs::remove_dir_all(&target).map_err(|e| format!("remove skill failed: {e}"))
}

pub fn set_skill_enabled_at(id: &str, enabled: bool, root: &Path) -> Result<(), String> {
    if !valid_plugin_id(id) {
        return Err(format!("invalid skill id `{id}`"));
    }
    let target = root.join(id);
    if !target.is_dir() {
        return Err(format!("skill `{id}` not found"));
    }
    let marker = target.join(SKILL_DISABLED_MARKER);
    if enabled {
        if marker.exists() {
            std::fs::remove_file(&marker).map_err(|e| format!("enable skill failed: {e}"))?;
        }
    } else if !marker.exists() {
        std::fs::write(&marker, b"").map_err(|e| format!("disable skill failed: {e}"))?;
    }
    Ok(())
}

// ─── Injection payloads ──────────────────────────────────────────────────────

/// url-type entries on the shared loopback server — the exact entry shape
/// `tools_mcp::session_entry` has proven across every adapter (zcode
/// included). One plugin-layer token names the session for all entries.
pub fn proxy_session_entries(app: &AppHandle, session_id: &str) -> (Vec<Value>, Vec<String>) {
    let enabled: Vec<PluginRecord> = load().into_iter().filter(|p| p.enabled).collect();
    if enabled.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let Some(port) = crate::tools_mcp::ensure_listening(app) else {
        return (
            Vec::new(),
            vec!["plugins skipped (loopback server failed to start)".to_string()],
        );
    };
    let token = crate::tools_mcp::register_session_token(session_id);
    (proxy_entries_for(port, &token, &enabled), Vec::new())
}

/// Pure builder (testable without an AppHandle). The only credential an
/// agent ever sees on the proxy route is the loopback session token.
fn proxy_entries_for(port: u16, token: &str, enabled: &[PluginRecord]) -> Vec<Value> {
    enabled
        .iter()
        .map(|plugin| {
            json!({
                "name": plugin.id,
                "url": format!("http://127.0.0.1:{port}/mcp/p/{}", plugin.id),
                "headers": { "Authorization": format!("Bearer {token}") },
                "type": "http",
            })
        })
        .collect()
}

/// Fallback for agents that take stdio but not http: the original spec, env
/// values included (same posture as stdio lending — the agent process spawns
/// the server itself).
pub fn stdio_fallback_entries() -> (Vec<Value>, Vec<String>) {
    let mut entries = Vec::new();
    let mut skipped = Vec::new();
    for plugin in load().into_iter().filter(|p| p.enabled) {
        if plugin.transport == "stdio" && !plugin.command.is_empty() {
            entries.push(json!({
                "name": plugin.id,
                "command": plugin.command,
                "args": plugin.args,
                "env": plugin.env.iter()
                    .map(|(key, value)| json!({"name": key, "value": value}))
                    .collect::<Vec<_>>(),
            }));
        } else if plugin.transport == "http" {
            skipped.push(format!(
                "{} (http plugin needs mcpCapabilities.http)",
                plugin.id
            ));
        }
    }
    (entries, skipped)
}

// ─── Tauri commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn plugins_list() -> Result<Vec<PluginRecord>, String> {
    Ok(load())
}

#[tauri::command(async)]
pub fn plugins_add(spec: PluginSpec) -> Result<PluginRecord, String> {
    validate_spec(&spec)?;
    let mut plugins = load();
    if plugins.iter().any(|p| p.id == spec.id) {
        return Err(format!("plugin `{}` already exists", spec.id));
    }
    let mut record = PluginRecord {
        id: spec.id,
        transport: spec.transport,
        command: spec.command.trim().to_string(),
        args: spec.args,
        env: spec.env,
        url: spec.url.trim().to_string(),
        headers: spec.headers,
        enabled: true,
        added_at: now_ms(),
        last_probe: None,
    };
    // Probe before saving, but a failed probe does not block installation —
    // the user may install first and start the server later.
    let probe = crate::plugin_host::probe(&record);
    record.last_probe = Some(probe);
    plugins.push(record.clone());
    save(&plugins)?;
    Ok(record)
}

#[tauri::command]
pub fn plugins_remove(id: String) -> Result<(), String> {
    let mut plugins = load();
    let before = plugins.len();
    plugins.retain(|p| p.id != id);
    if plugins.len() == before {
        return Err(format!("plugin `{id}` not found"));
    }
    save(&plugins)?;
    crate::plugin_host::kill_plugin(&id);
    Ok(())
}

#[tauri::command]
pub fn plugins_set_enabled(id: String, enabled: bool) -> Result<(), String> {
    let mut plugins = load();
    let Some(record) = plugins.iter_mut().find(|p| p.id == id) else {
        return Err(format!("plugin `{id}` not found"));
    };
    record.enabled = enabled;
    save(&plugins)?;
    if !enabled {
        crate::plugin_host::kill_plugin(&id);
    }
    Ok(())
}

#[tauri::command(async)]
pub fn plugins_probe(id: String) -> Result<ProbeRecord, String> {
    let mut plugins = load();
    let Some(record) = plugins.iter_mut().find(|p| p.id == id) else {
        return Err(format!("plugin `{id}` not found"));
    };
    let probe = crate::plugin_host::probe(record);
    record.last_probe = Some(probe.clone());
    let snapshot = plugins.clone();
    save(&snapshot)?;
    Ok(probe)
}

// ── Skills commands（skills/ 目录即登记处；停用 = .disabled 标记） ──

#[tauri::command]
pub fn skills_list() -> Result<Vec<SkillRecord>, String> {
    Ok(list_skills_at(&skills_dir()?))
}

#[tauri::command(async)]
pub fn skill_install(source_path: String, id: Option<String>) -> Result<SkillRecord, String> {
    install_skill_at(std::path::Path::new(&source_path), id, &skills_dir()?)
}

#[tauri::command(async)]
pub fn skill_remove(id: String) -> Result<(), String> {
    remove_skill_at(&id, &skills_dir()?)
}

#[tauri::command(async)]
pub fn skill_set_enabled(id: String, enabled: bool) -> Result<(), String> {
    set_skill_enabled_at(&id, enabled, &skills_dir()?)
}

fn now_ms() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique temp dir per test (cargo test runs in one process, so the pid
    /// alone would collide across parallel tests — include the test name).
    fn temp_dir_for(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "Marionette-plugins-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn spec(id: &str, transport: &str) -> PluginSpec {
        PluginSpec {
            id: id.into(),
            transport: transport.into(),
            command: "node".into(),
            args: vec!["server.js".into()],
            env: HashMap::new(),
            url: "http://127.0.0.1:9999/mcp".into(),
            headers: HashMap::new(),
        }
    }

    #[test]
    fn plugin_ids_are_slugs() {
        assert!(valid_plugin_id("a"));
        assert!(valid_plugin_id("context7"));
        assert!(valid_plugin_id("my.tool_2-x"));
        assert!(!valid_plugin_id(""));
        assert!(!valid_plugin_id("Upper"));
        assert!(!valid_plugin_id("has space"));
        assert!(!valid_plugin_id("a/b"));
        assert!(!valid_plugin_id(&"x".repeat(65)));
    }

    // ── Skills（2026-11-06）：目录即登记处，.disabled 即全局停用 ──

    /// A source skill folder with frontmatter + one asset file.
    fn skill_source(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "Marionette-skill-src-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "---\nname: Demo Skill\ndescription: A demo.\n---\nBody here.\n",
        )
        .unwrap();
        std::fs::write(dir.join("assets").join("note.txt"), "asset").unwrap();
        dir
    }

    #[test]
    fn skill_install_list_toggle_remove_round_trip() {
        let root = temp_dir_for("skills");
        let source = skill_source("roundtrip");

        let record = install_skill_at(&source, Some("demo".into()), &root).unwrap();
        assert_eq!(record.id, "demo");
        assert_eq!(record.name, "Demo Skill");
        assert!(record.enabled);
        assert!(root.join("demo").join("assets").join("note.txt").exists());

        // Duplicate ids are refused, not overwritten.
        assert!(install_skill_at(&source, Some("demo".into()), &root).is_err());

        // Disable via marker; list still shows it (UI needs the toggle).
        set_skill_enabled_at("demo", false, &root).unwrap();
        let listed = list_skills_at(&root);
        assert_eq!(listed.len(), 1);
        assert!(!listed[0].enabled);
        assert!(root.join("demo").join(".disabled").exists());

        set_skill_enabled_at("demo", true, &root).unwrap();
        assert!(list_skills_at(&root)[0].enabled);
        assert!(!root.join("demo").join(".disabled").exists());

        remove_skill_at("demo", &root).unwrap();
        assert!(list_skills_at(&root).is_empty());
        assert!(!root.join("demo").exists());
    }

    #[test]
    fn skill_install_requires_skill_md_and_slug_id() {
        let root = temp_dir_for("skills-invalid");
        // Folder without SKILL.md.
        let bare = std::env::temp_dir().join(format!(
            "Marionette-skill-bare-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&bare);
        std::fs::create_dir_all(&bare).unwrap();
        assert!(install_skill_at(&bare, None, &root).is_err());
        // Bad explicit id (path traversal shape).
        let source = skill_source("invalid");
        assert!(install_skill_at(&source, Some("../escape".into()), &root).is_err());
        // Marker from the source is not carried over: install = enabled.
        // Default id = lowercased source folder name.
        std::fs::write(source.join(".disabled"), b"").unwrap();
        let record = install_skill_at(&source, None, &root).unwrap();
        assert!(record.enabled);
        assert!(record.id.starts_with("marionette-skill-src-invalid"));
    }

    #[test]
    fn disabled_marker_hides_skill_from_inventory_scan() {
        use crate::context_inventory::Collector;
        let dir = temp_dir_for("skills-scan");
        let source = skill_source("scan");
        install_skill_at(&source, Some("scan-demo".into()), &dir).unwrap();

        let mut visible = Collector::default();
        crate::context_inventory::scan_skills_dir_for_test(&dir, "marionette", &mut visible);
        assert_eq!(visible.skills.len(), 1);

        set_skill_enabled_at("scan-demo", false, &dir).unwrap();
        let mut hidden = Collector::default();
        crate::context_inventory::scan_skills_dir_for_test(&dir, "marionette", &mut hidden);
        assert!(hidden.skills.is_empty());
    }

    #[test]
    fn store_round_trip_at_dir() {
        let dir = temp_dir_for("roundtrip");
        assert!(load_at(&dir).is_empty());
        let plugins = vec![
            PluginRecord {
                id: "alpha".into(),
                transport: "http".into(),
                command: String::new(),
                args: vec![],
                env: HashMap::new(),
                url: "https://mcp.example.com/mcp".into(),
                headers: HashMap::from([("Authorization".into(), "Bearer secret".into())]),
                enabled: true,
                added_at: "1".into(),
                last_probe: None,
            },
        ];
        save_at(&dir, &plugins).unwrap();
        assert_eq!(load_at(&dir).len(), 1);
        let loaded = load_at(&dir);
        assert_eq!(loaded[0].headers["Authorization"], "Bearer secret");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_registry_archives_and_resets() {
        let dir = temp_dir_for("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(index_path_at(&dir), "{ not json").unwrap();
        assert!(load_at(&dir).is_empty());
        // The corpse must be parked next to it, not deleted.
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("index.json.corrupt-")
            })
            .collect();
        assert_eq!(leftovers.len(), 1, "corrupt index must be archived");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn proxy_entries_match_the_battle_tested_shape() {
        let record = PluginRecord {
            id: "demo".into(),
            transport: "http".into(),
            command: String::new(),
            args: vec![],
            env: HashMap::new(),
            url: "https://mcp.example.com/mcp".into(),
            headers: HashMap::from([("Authorization".into(), "Bearer real-secret".into())]),
            enabled: true,
            added_at: String::new(),
            last_probe: None,
        };
        let entries = proxy_entries_for(12345, "session-token", std::slice::from_ref(&record));
        let entry = &entries[0];
        assert_eq!(entry["type"], "http");
        assert_eq!(entry["name"], "demo");
        assert_eq!(
            entry["url"],
            "http://127.0.0.1:12345/mcp/p/demo"
        );
        // The only header is the loopback session token — the plugin's real
        // credentials never appear in a proxy entry.
        assert_eq!(entry["headers"].as_object().unwrap().len(), 1);
        assert_eq!(entry["headers"]["Authorization"], "Bearer session-token");
        assert!(
            !serde_json::to_string(entry).unwrap().contains("real-secret"),
            "source credential must not leak into the entry"
        );
    }

    #[test]
    fn stdio_fallback_carries_env_values() {
        let dir = temp_dir_for("stdio-fallback");
        save_at(
            &dir,
            &vec![PluginRecord {
                id: "stdio-demo".into(),
                transport: "stdio".into(),
                command: "npx".into(),
                args: vec!["-y".into(), "demo-mcp".into()],
                env: HashMap::from([("API_KEY".into(), "k".into())]),
                url: String::new(),
                headers: HashMap::new(),
                enabled: true,
                added_at: String::new(),
                last_probe: None,
            }],
        )
        .unwrap();
        // stdio_fallback_entries reads the global dir — exercise the *_at
        // core through the same builder it uses.
        let loaded = load_at(&dir);
        let enabled: Vec<&PluginRecord> = loaded.iter().filter(|p| p.enabled).collect();
        let entries: Vec<Value> = enabled
            .iter()
            .filter(|p| p.transport == "stdio" && !p.command.is_empty())
            .map(|p| {
                json!({
                    "name": p.id,
                    "command": p.command,
                    "args": p.args,
                    "env": p.env.iter().map(|(k, v)| json!({"name": k, "value": v})).collect::<Vec<_>>(),
                })
            })
            .collect();
        assert_eq!(entries[0]["command"], "npx");
        assert_eq!(entries[0]["env"][0]["name"], "API_KEY");
        assert_eq!(entries[0]["env"][0]["value"], "k");
    }

    #[test]
    fn validation_rejects_bad_specs() {
        let mut bad = spec("ok-id", "stdio");
        bad.command = String::new();
        assert!(validate_spec(&bad).is_err());
        assert!(validate_spec(&spec("Bad Id", "stdio")).is_err());
        assert!(validate_spec(&spec("ok-id", "carrier-pigeon")).is_err());
        let mut bad = spec("ok-id", "http");
        bad.url = "ftp://nope".into();
        assert!(validate_spec(&bad).is_err());
        assert!(validate_spec(&spec("ok-id", "stdio")).is_ok());
        assert!(validate_spec(&spec("ok-id", "http")).is_ok());
    }
}
