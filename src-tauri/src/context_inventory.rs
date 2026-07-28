//! What each agent already brings (MCP servers + skills), and what Marionette
//! lends to the agents that are missing it.
//!
//! Two layers, deliberately:
//!   * **global** — where users actually keep this stuff (`~/.config/opencode`,
//!     `~/.codex/config.toml`, `~/.claude`, `~/.grok/skills`)
//!   * **project** — `.mcp.json`, `.claude/skills`, `.marionette/skills`
//!
//! The decision of what to lend is per project (`.marionette/context.json`) and
//! stores **references only**. Secrets stay in the agent's own config file and
//! are read at injection time, never copied, never logged.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const SELECTION_VERSION: u32 = 1;

// ─── Model ──────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerSpec {
    /// Stable across scans: agents share one entry per server name.
    pub id: String,
    pub name: String,
    /// `stdio` | `http` | `sse`
    pub transport: String,
    pub command: Option<String>,
    pub args: Vec<String>,
    /// Names only — values live in the agent's config and are read on demand.
    pub env_keys: Vec<String>,
    /// HTTP header *names* only (e.g. `Authorization`). Values are re-read at
    /// inject time — Unity MCP and similar servers 401 without them.
    #[serde(default)]
    pub header_keys: Vec<String>,
    pub url: Option<String>,
    /// Where it was found (agent id or `project`).
    pub sources: Vec<String>,
    pub source_paths: Vec<String>,
    /// Agents that already load it themselves — never inject into these.
    pub agents: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSpec {
    pub id: String,
    pub name: String,
    pub description: String,
    pub dir: String,
    pub file: String,
    pub sources: Vec<String>,
    pub agents: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextInventory {
    pub mcp_servers: Vec<McpServerSpec>,
    pub skills: Vec<SkillSpec>,
    /// Anything unreadable / skipped — surfaced in the UI instead of swallowed.
    pub notes: Vec<String>,
    pub scanned_at: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextSelection {
    #[serde(default)]
    pub version: u32,
    /// Explicit user decisions. Absent = per-kind default (see `is_enabled`).
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, bool>,
    #[serde(default)]
    pub skills: BTreeMap<String, bool>,
    /// Folders outside the project the user granted this project's agents.
    ///
    /// Sent as ACP `session/new.additionalDirectories`, which "expands the
    /// session's filesystem scope without changing cwd". Granting up front is
    /// the only way to unblock a *subagent*: its approval prompt never reaches
    /// us, so it can only be answered before the turn starts.
    #[serde(default)]
    pub workspace_roots: Vec<String>,
    #[serde(default)]
    pub updated_at: String,
}

impl ContextSelection {
    /// Skills are prompt text (free); MCP servers spawn processes, so they wait
    /// for an explicit yes.
    pub fn is_enabled(&self, kind: &str, id: &str) -> bool {
        match kind {
            "skill" => self.skills.get(id).copied().unwrap_or(true),
            _ => self.mcp_servers.get(id).copied().unwrap_or(false),
        }
    }
}

// ─── Scanning ───────────────────────────────────────────────────────────────

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn now_millis() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_default()
}

/// Strip `//` and `/* */` comments so `serde_json` can read a `.jsonc`.
/// String-aware: a `//` inside a quoted value is content, not a comment.
pub fn strip_jsonc_comments(input: &str) -> String {
    let bytes: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut index = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    while index < bytes.len() {
        let ch = bytes[index];
        if in_string {
            out.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            index += 1;
            continue;
        }
        if ch == '"' {
            in_string = true;
            out.push(ch);
            index += 1;
            continue;
        }
        if ch == '/' && index + 1 < bytes.len() {
            match bytes[index + 1] {
                '/' => {
                    while index < bytes.len() && bytes[index] != '\n' {
                        index += 1;
                    }
                    continue;
                }
                '*' => {
                    index += 2;
                    while index + 1 < bytes.len() && !(bytes[index] == '*' && bytes[index + 1] == '/') {
                        index += 1;
                    }
                    index = (index + 2).min(bytes.len());
                    continue;
                }
                _ => {}
            }
        }
        out.push(ch);
        index += 1;
    }
    out
}

fn read_jsonc(path: &Path) -> Option<Value> {
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&strip_jsonc_comments(&raw)).ok()
}

struct Collector {
    servers: Vec<McpServerSpec>,
    skills: Vec<SkillSpec>,
    notes: Vec<String>,
}

impl Collector {
    fn add_server(&mut self, mut spec: McpServerSpec) {
        if let Some(existing) = self.servers.iter_mut().find(|s| s.id == spec.id) {
            for source in spec.sources.drain(..) {
                if !existing.sources.contains(&source) {
                    existing.sources.push(source);
                }
            }
            for path in spec.source_paths.drain(..) {
                if !existing.source_paths.contains(&path) {
                    existing.source_paths.push(path);
                }
            }
            for agent in spec.agents.drain(..) {
                if !existing.agents.contains(&agent) {
                    existing.agents.push(agent);
                }
            }
            // Prefer a definition that actually says how to launch it.
            if existing.command.is_none() && existing.url.is_none() {
                existing.command = spec.command;
                existing.args = spec.args;
                existing.url = spec.url;
                existing.transport = spec.transport;
                existing.env_keys = spec.env_keys;
                existing.header_keys = spec.header_keys;
            } else if existing.header_keys.is_empty() && !spec.header_keys.is_empty() {
                // Same url from another source that actually carries auth.
                // (source_paths already merged above so inject can re-read values.)
                existing.header_keys = spec.header_keys;
            }
            return;
        }
        self.servers.push(spec);
    }

    fn add_skill(&mut self, mut spec: SkillSpec) {
        if let Some(existing) = self.skills.iter_mut().find(|s| s.id == spec.id) {
            for source in spec.sources.drain(..) {
                if !existing.sources.contains(&source) {
                    existing.sources.push(source);
                }
            }
            for agent in spec.agents.drain(..) {
                if !existing.agents.contains(&agent) {
                    existing.agents.push(agent);
                }
            }
            if existing.description.is_empty() {
                existing.description = spec.description;
            }
            return;
        }
        self.skills.push(spec);
    }
}

/// OpenCode: `"mcp": { name: { type, command: [prog, ...args], env, enabled } }`
fn scan_opencode_config(path: &Path, source: &str, agent: Option<&str>, out: &mut Collector) {
    let Some(value) = read_jsonc(path) else {
        if path.exists() {
            out.notes.push(format!("Could not parse {}", path.display()));
        }
        return;
    };
    let Some(mcp) = value.get("mcp").and_then(Value::as_object) else {
        return;
    };
    for (name, entry) in mcp {
        let obj = match entry.as_object() {
            Some(obj) => obj,
            None => continue,
        };
        if obj.get("enabled").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        let command_list: Vec<String> = obj
            .get("command")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let url = obj.get("url").and_then(Value::as_str).map(str::to_string);
        let env_keys = obj
            .get("env")
            .and_then(Value::as_object)
            .map(|env| env.keys().cloned().collect())
            .unwrap_or_default();
        let header_keys = header_keys_from_json(obj.get("headers"));

        out.add_server(McpServerSpec {
            id: name.to_lowercase(),
            name: name.clone(),
            transport: match obj.get("type").and_then(Value::as_str) {
                Some("sse") => "sse".into(),
                Some("remote") | Some("http") if url.is_some() => "http".into(),
                _ if url.is_some() => "http".into(),
                _ => "stdio".into(),
            },
            command: command_list.first().cloned(),
            args: command_list.iter().skip(1).cloned().collect(),
            env_keys,
            header_keys,
            url,
            sources: vec![source.to_string()],
            source_paths: vec![path.display().to_string()],
            agents: agent.map(|a| vec![a.to_string()]).unwrap_or_default(),
        });
    }
}

/// Header *names* from a JSON object/array — never the secret values.
fn header_keys_from_json(headers: Option<&Value>) -> Vec<String> {
    match headers {
        Some(Value::Object(map)) => map.keys().cloned().collect(),
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(|item| {
                item.get("name")
                    .or_else(|| item.get("key"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Codex / Grok: `[mcp_servers.name]` with `command` / `args` / `[.env]` or `url`.
fn scan_codex_config(path: &Path, out: &mut Collector) {
    let Ok(raw) = fs::read_to_string(path) else {
        return;
    };
    let parsed: toml::Value = match toml::from_str(&raw) {
        Ok(value) => value,
        Err(error) => {
            out.notes.push(format!("Could not parse {}: {error}", path.display()));
            return;
        }
    };
    let Some(servers) = parsed.get("mcp_servers").and_then(|v| v.as_table()) else {
        return;
    };
    for (name, entry) in servers {
        let Some(table) = entry.as_table() else { continue };
        if table.get("enabled").and_then(toml::Value::as_bool) == Some(false) {
            continue;
        }
        let url = table
            .get("url")
            .and_then(toml::Value::as_str)
            .map(str::to_string);
        let command = table
            .get("command")
            .and_then(toml::Value::as_str)
            .map(str::to_string);
        let args = table
            .get("args")
            .and_then(toml::Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let env_keys = table
            .get("env")
            .and_then(toml::Value::as_table)
            .map(|env| env.keys().cloned().collect())
            .unwrap_or_default();

        let header_keys = table
            .get("headers")
            .and_then(toml::Value::as_table)
            .map(|h| h.keys().cloned().collect())
            .unwrap_or_default();

        out.add_server(McpServerSpec {
            id: name.to_lowercase(),
            name: name.clone(),
            transport: if url.is_some() { "http".into() } else { "stdio".into() },
            command,
            args,
            env_keys,
            header_keys,
            url,
            sources: vec!["codex".to_string()],
            source_paths: vec![path.display().to_string()],
            agents: vec!["codex".to_string()],
        });
    }
}

/// Grok: same `[mcp_servers.*]` table shape as Codex, but own agent id.
fn scan_grok_config(path: &Path, out: &mut Collector) {
    let Ok(raw) = fs::read_to_string(path) else {
        return;
    };
    let parsed: toml::Value = match toml::from_str(&raw) {
        Ok(value) => value,
        Err(error) => {
            out.notes.push(format!("Could not parse {}: {error}", path.display()));
            return;
        }
    };
    let Some(servers) = parsed.get("mcp_servers").and_then(|v| v.as_table()) else {
        return;
    };
    for (name, entry) in servers {
        let Some(table) = entry.as_table() else { continue };
        if table.get("enabled").and_then(toml::Value::as_bool) == Some(false) {
            continue;
        }
        let url = table
            .get("url")
            .and_then(toml::Value::as_str)
            .map(str::to_string);
        let command = table
            .get("command")
            .and_then(toml::Value::as_str)
            .map(str::to_string);
        let args = table
            .get("args")
            .and_then(toml::Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let env_keys = table
            .get("env")
            .and_then(toml::Value::as_table)
            .map(|env| env.keys().cloned().collect())
            .unwrap_or_default();
        let header_keys = table
            .get("headers")
            .and_then(toml::Value::as_table)
            .map(|h| h.keys().cloned().collect())
            .unwrap_or_default();

        out.add_server(McpServerSpec {
            id: name.to_lowercase(),
            name: name.clone(),
            transport: if url.is_some() { "http".into() } else { "stdio".into() },
            command,
            args,
            env_keys,
            header_keys,
            url,
            sources: vec!["grok-build".to_string()],
            source_paths: vec![path.display().to_string()],
            agents: vec!["grok-build".to_string()],
        });
    }
}

/// Claude / generic: `{ "mcpServers": { name: { command, args, env } | { url } } }`
fn scan_mcp_json(path: &Path, source: &str, agent: Option<&str>, out: &mut Collector) {
    let Some(value) = read_jsonc(path) else { return };
    let Some(servers) = value.get("mcpServers").and_then(Value::as_object) else {
        return;
    };
    for (name, entry) in servers {
        let Some(obj) = entry.as_object() else { continue };
        let url = obj.get("url").and_then(Value::as_str).map(str::to_string);
        out.add_server(McpServerSpec {
            id: name.to_lowercase(),
            name: name.clone(),
            transport: match obj.get("type").and_then(Value::as_str) {
                Some("sse") => "sse".into(),
                _ if url.is_some() => "http".into(),
                _ => "stdio".into(),
            },
            command: obj.get("command").and_then(Value::as_str).map(str::to_string),
            args: obj
                .get("args")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            env_keys: obj
                .get("env")
                .and_then(Value::as_object)
                .map(|env| env.keys().cloned().collect())
                .unwrap_or_default(),
            header_keys: header_keys_from_json(obj.get("headers")),
            url,
            sources: vec![source.to_string()],
            source_paths: vec![path.display().to_string()],
            agents: agent.map(|a| vec![a.to_string()]).unwrap_or_default(),
        });
    }
}

/// `name:` / `description:` out of a SKILL.md frontmatter block.
fn parse_skill_front_matter(text: &str) -> (Option<String>, Option<String>) {
    let mut name = None;
    let mut description = None;
    let mut in_front = false;
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if index == 0 && trimmed == "---" {
            in_front = true;
            continue;
        }
        if in_front && trimmed == "---" {
            break;
        }
        if !in_front && index > 6 {
            break;
        }
        if let Some(rest) = trimmed.strip_prefix("name:") {
            name = Some(rest.trim().trim_matches('"').trim_matches('\'').to_string());
        } else if let Some(rest) = trimmed.strip_prefix("description:") {
            description = Some(rest.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    (name, description)
}

fn scan_skills_dir(dir: &Path, source: &str, agent: Option<&str>, out: &mut Collector) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let skill_file = path.join("SKILL.md");
        if !skill_file.exists() {
            continue;
        }
        let id = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if id.is_empty() {
            continue;
        }
        let text = fs::read_to_string(&skill_file).unwrap_or_default();
        let (name, description) = parse_skill_front_matter(&text);
        out.add_skill(SkillSpec {
            id: id.to_lowercase(),
            name: name.unwrap_or_else(|| id.clone()),
            description: description.unwrap_or_default(),
            dir: path.display().to_string(),
            file: skill_file.display().to_string(),
            sources: vec![source.to_string()],
            agents: agent.map(|a| vec![a.to_string()]).unwrap_or_default(),
        });
    }
}

/// Everything this machine + project can offer, deduped by name/id.
pub fn scan(project_root: &Path) -> ContextInventory {
    let mut out = Collector {
        servers: Vec::new(),
        skills: Vec::new(),
        notes: Vec::new(),
    };

    if let Some(home) = home_dir() {
        // Global: where users actually keep MCP servers and skills.
        scan_opencode_config(
            &home.join(".config/opencode/opencode.jsonc"),
            "opencode",
            Some("opencode"),
            &mut out,
        );
        scan_opencode_config(
            &home.join(".config/opencode/opencode.json"),
            "opencode",
            Some("opencode"),
            &mut out,
        );
        scan_codex_config(&home.join(".codex/config.toml"), &mut out);
        // Grok uses the same `[mcp_servers.*]` table as Codex. Without this scan
        // we re-inject servers Grok already owns — and used to drop their auth
        // headers, so Unity MCP came up as 401 and the agent only saw `tasks`.
        scan_grok_config(&home.join(".grok/config.toml"), &mut out);
        scan_mcp_json(&home.join(".claude.json"), "claude-code", Some("claude-code"), &mut out);

        scan_skills_dir(&home.join(".config/opencode/skills"), "opencode", Some("opencode"), &mut out);
        scan_skills_dir(&home.join(".claude/skills"), "claude-code", Some("claude-code"), &mut out);
        scan_skills_dir(&home.join(".grok/skills"), "grok-build", Some("grok-build"), &mut out);
        scan_skills_dir(&home.join(".codex/skills"), "codex", Some("codex"), &mut out);
    }

    // Project: these travel with the repo, so they win on relevance.
    scan_mcp_json(&project_root.join(".mcp.json"), "project", None, &mut out);
    scan_opencode_config(&project_root.join("opencode.jsonc"), "project", None, &mut out);
    scan_opencode_config(&project_root.join("opencode.json"), "project", None, &mut out);
    scan_skills_dir(&project_root.join(".claude/skills"), "project", Some("claude-code"), &mut out);
    scan_skills_dir(&project_root.join(".agents/skills"), "project", None, &mut out);
    scan_skills_dir(
        &crate::app_paths::project_dir(project_root).join("skills"),
        "project",
        None,
        &mut out,
    );

    out.servers.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    out.skills.sort_by(|a, b| a.id.cmp(&b.id));

    ContextInventory {
        mcp_servers: out.servers,
        skills: out.skills,
        notes: out.notes,
        scanned_at: now_millis(),
    }
}

// ─── Per-project selection ──────────────────────────────────────────────────

fn selection_path(project_root: &Path) -> PathBuf {
    crate::app_paths::project_dir(project_root).join("context.json")
}

pub fn load_selection(project_root: &Path) -> ContextSelection {
    let path = selection_path(project_root);
    let Ok(raw) = fs::read_to_string(&path) else {
        return ContextSelection {
            version: SELECTION_VERSION,
            ..Default::default()
        };
    };
    serde_json::from_str(&raw).unwrap_or(ContextSelection {
        version: SELECTION_VERSION,
        ..Default::default()
    })
}

pub fn save_selection(project_root: &Path, selection: &ContextSelection) -> Result<(), String> {
    let path = selection_path(project_root);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Create .marionette failed: {error}"))?;
    }
    let body = serde_json::to_string_pretty(selection)
        .map_err(|error| format!("Serialize context selection failed: {error}"))?;
    fs::write(&path, format!("{body}\n"))
        .map_err(|error| format!("Write context selection failed: {error}"))
}

pub fn set_enabled(
    project_root: &Path,
    kind: &str,
    id: &str,
    enabled: bool,
) -> Result<ContextSelection, String> {
    let mut selection = load_selection(project_root);
    selection.version = SELECTION_VERSION;
    selection.updated_at = now_millis();
    if kind == "skill" {
        selection.skills.insert(id.to_string(), enabled);
    } else {
        selection.mcp_servers.insert(id.to_string(), enabled);
    }
    save_selection(project_root, &selection)?;
    Ok(selection)
}

// ─── Workspace roots (paths outside the project) ────────────────────────────

/// Case-insensitive on Windows, and `\\?\` prefixes must not defeat matching.
fn normalize_path(path: &str) -> String {
    path.trim()
        .trim_matches('"')
        .strip_prefix(r"\\?\")
        .unwrap_or_else(|| path.trim().trim_matches('"'))
        .replace('\\', "/")
        .trim_end_matches('/')
        .to_lowercase()
}

pub fn is_inside(parent: &str, candidate: &str) -> bool {
    let parent = normalize_path(parent);
    let candidate = normalize_path(candidate);
    if parent.is_empty() {
        return false;
    }
    candidate == parent || candidate.starts_with(&format!("{parent}/"))
}

/// Folder to grant for a path: the file's parent, or the folder itself.
pub fn grant_dir_for(path: &Path) -> PathBuf {
    if path.is_dir() {
        path.to_path_buf()
    } else {
        path.parent().map(Path::to_path_buf).unwrap_or_else(|| path.to_path_buf())
    }
}

pub fn workspace_roots(project_root: &Path) -> Vec<String> {
    load_selection(project_root).workspace_roots
}

/// Remember a folder outside the project. Idempotent; keeps the list minimal by
/// dropping entries the new root already covers.
pub fn grant_workspace_root(project_root: &Path, dir: &str) -> Result<Vec<String>, String> {
    let dir = dir.trim().trim_matches('"');
    if dir.is_empty() {
        return Err("Empty path".to_string());
    }
    let path = PathBuf::from(strip_extended(dir));
    if !path.is_absolute() {
        return Err(format!("Workspace root must be absolute: {dir}"));
    }
    let display = path.to_string_lossy().to_string();

    let mut selection = load_selection(project_root);
    selection.version = SELECTION_VERSION;
    selection.updated_at = now_millis();
    if selection
        .workspace_roots
        .iter()
        .any(|existing| is_inside(existing, &display))
    {
        return Ok(selection.workspace_roots); // already covered
    }
    // A broader grant replaces the narrower ones it contains.
    selection
        .workspace_roots
        .retain(|existing| !is_inside(&display, existing));
    selection.workspace_roots.push(display);
    save_selection(project_root, &selection)?;
    Ok(selection.workspace_roots)
}

pub fn revoke_workspace_root(project_root: &Path, dir: &str) -> Result<Vec<String>, String> {
    let mut selection = load_selection(project_root);
    selection.workspace_roots.retain(|root| normalize_path(root) != normalize_path(dir));
    selection.updated_at = now_millis();
    save_selection(project_root, &selection)?;
    Ok(selection.workspace_roots)
}

fn strip_extended(path: &str) -> String {
    path.strip_prefix(r"\\?\").unwrap_or(path).to_string()
}

/// `OPENCODE_PERMISSION` value granting the project's workspace roots.
///
/// ACP's `additionalDirectories` is accepted by opencode but does **not** widen
/// its permission scope (measured: same prompt asked with and without it). Its
/// real gate is `Tool.assertExternalDirectory`, which asks under the
/// `external_directory` permission with a `<dir>/*` pattern — and that config is
/// merged from this env var. Since Marionette spawns the process, we can set the
/// grant per session without touching any of the user's config files.
///
/// Existing values are merged, not replaced: the user may already run with their
/// own `OPENCODE_PERMISSION`.
pub fn opencode_permission_env(project_root: &Path, existing: Option<&str>) -> Option<String> {
    let roots = workspace_roots(project_root);
    if roots.is_empty() {
        return None;
    }

    let mut config: Map<String, Value> = existing
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();

    // Keep whatever the user already allowed under this key.
    let mut external = config
        .get("external_directory")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    for root in roots {
        // opencode normalizes patterns to forward slashes and appends `/*`.
        let pattern = format!("{}/*", root.replace('\\', "/").trim_end_matches('/'));
        external.insert(pattern, json!("allow"));
    }

    config.insert("external_directory".to_string(), Value::Object(external));
    serde_json::to_string(&Value::Object(config)).ok()
}

/// Which of these paths sit outside the project and are not granted yet.
pub fn outside_project_paths(project_root: &Path, paths: &[String]) -> Vec<Value> {
    let root = project_root.to_string_lossy().to_string();
    let granted = workspace_roots(project_root);
    let mut seen: Vec<String> = Vec::new();
    let mut out = Vec::new();

    for raw in paths {
        let path = PathBuf::from(strip_extended(raw.trim().trim_matches('"')));
        if !path.is_absolute() || !path.exists() {
            continue; // relative paths resolve inside the project; missing ones are not ours to grant
        }
        let display = path.to_string_lossy().to_string();
        if is_inside(&root, &display) {
            continue;
        }
        let dir = grant_dir_for(&path).to_string_lossy().to_string();
        if granted.iter().any(|root| is_inside(root, &display)) {
            continue;
        }
        if seen.iter().any(|other| normalize_path(other) == normalize_path(&dir)) {
            continue;
        }
        seen.push(dir.clone());
        out.push(json!({
            "path": display,
            "dir": dir,
            "isDirectory": path.is_dir(),
        }));
    }
    out
}

// ─── Grok folder trust ──────────────────────────────────────────────────────

/// Record `project_root` as trusted in `~/.grok/trusted_folders.toml`.
///
/// Grok refuses to start **project-scoped** MCP (`.grok/config.toml`, `mcps/`)
/// until the folder is trusted. Marionette is opening the project on the user's
/// behalf, so we grant trust the same way `/hooks-trust` / `grok --trust` do.
/// Idempotent: already-trusted paths are left alone.
pub fn ensure_grok_folder_trust(project_root: &Path) {
    let Some(home) = home_dir() else { return };
    let path = home.join(".grok").join("trusted_folders.toml");
    let key = normalize_trust_path(project_root);
    if key.is_empty() {
        return;
    }

    let mut raw = fs::read_to_string(&path).unwrap_or_default();
    // Cheap membership check — section header uses the absolute path in quotes.
    let section = format!("[folders.'{key}']");
    if raw.contains(&section) && raw.contains("trusted = true") {
        // May still be untrusted for this path if another folder block matches
        // first; re-write this section explicitly below when needed.
        if let Ok(parsed) = toml::from_str::<toml::Value>(&raw) {
            if parsed
                .get("folders")
                .and_then(|f| f.get(&key))
                .and_then(|e| e.get("trusted"))
                .and_then(|v| v.as_bool())
                == Some(true)
            {
                return;
            }
        }
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let block = format!(
        "\n[folders.'{key}']\ntrusted = true\ndecided_at = {now}\n"
    );
    if let Ok(mut parsed) = toml::from_str::<toml::Value>(&raw) {
        let folders = parsed
            .as_table_mut()
            .map(|t| {
                t.entry("folders")
                    .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
            })
            .and_then(|v| v.as_table_mut());
        if let Some(folders) = folders {
            let mut entry = toml::map::Map::new();
            entry.insert("trusted".into(), toml::Value::Boolean(true));
            entry.insert("decided_at".into(), toml::Value::Integer(now as i64));
            folders.insert(key.clone(), toml::Value::Table(entry));
            if let Ok(serialized) = toml::to_string_pretty(&parsed) {
                raw = serialized;
            } else {
                raw.push_str(&block);
            }
        } else {
            raw.push_str(&block);
        }
    } else if raw.trim().is_empty() {
        raw = format!("[folders.'{key}']\ntrusted = true\ndecided_at = {now}\n");
    } else {
        raw.push_str(&block);
    }

    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(&path, raw);
}

fn normalize_trust_path(project_root: &Path) -> String {
    // Match Grok's store keys: absolute, as displayed on this OS.
    fs::canonicalize(project_root)
        .unwrap_or_else(|_| project_root.to_path_buf())
        .to_string_lossy()
        // Windows canonicalize adds `\\?\` — Grok's store uses plain paths.
        .trim_start_matches(r"\\?\")
        .to_string()
}

// ─── Injection ──────────────────────────────────────────────────────────────

/// Re-read a server's env from its own config. Values never leave this call.
fn env_values_for(spec: &McpServerSpec) -> Vec<(String, String)> {
    let mut values: Vec<(String, String)> = Vec::new();
    for path in &spec.source_paths {
        let path = Path::new(path);
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            if let Ok(raw) = fs::read_to_string(path) {
                if let Ok(parsed) = toml::from_str::<toml::Value>(&raw) {
                    if let Some(env) = parsed
                        .get("mcp_servers")
                        .and_then(|v| v.get(&spec.name))
                        .and_then(|v| v.get("env"))
                        .and_then(|v| v.as_table())
                    {
                        for (key, value) in env {
                            if let Some(text) = value.as_str() {
                                values.push((key.clone(), text.to_string()));
                            }
                        }
                    }
                }
            }
            continue;
        }
        if let Some(value) = read_jsonc(path) {
            let entry = value
                .get("mcp")
                .and_then(|v| v.get(&spec.name))
                .or_else(|| value.get("mcpServers").and_then(|v| v.get(&spec.name)));
            if let Some(env) = entry.and_then(|v| v.get("env")).and_then(Value::as_object) {
                for (key, value) in env {
                    if let Some(text) = value.as_str() {
                        values.push((key.clone(), text.to_string()));
                    }
                }
            }
        }
    }
    values
}

/// Re-read HTTP headers (e.g. `Authorization: Bearer …`) at inject time.
///
/// Without these, remote Unity MCP answers 401 and the agent only sees whatever
/// other servers still connected — often just a generic `tasks` toolset.
fn header_values_for(spec: &McpServerSpec) -> Vec<(String, String)> {
    let mut values: Vec<(String, String)> = Vec::new();
    // Project sources are usually last in `source_paths` — prefer them so a
    // repo-local Bearer token wins over a stale global one.
    for path in spec.source_paths.iter().rev() {
        let path = Path::new(path);
        if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            if let Ok(raw) = fs::read_to_string(path) {
                if let Ok(parsed) = toml::from_str::<toml::Value>(&raw) {
                    if let Some(headers) = parsed
                        .get("mcp_servers")
                        .and_then(|v| v.get(&spec.name))
                        .and_then(|v| v.get("headers"))
                        .and_then(|v| v.as_table())
                    {
                        for (key, value) in headers {
                            if let Some(text) = value.as_str() {
                                values.push((key.clone(), text.to_string()));
                            }
                        }
                    }
                }
            }
            continue;
        }
        if let Some(value) = read_jsonc(path) {
            let entry = value
                .get("mcp")
                .and_then(|v| v.get(&spec.name))
                .or_else(|| value.get("mcpServers").and_then(|v| v.get(&spec.name)));
            if let Some(headers) = entry.and_then(|v| v.get("headers")) {
                match headers {
                    Value::Object(map) => {
                        for (key, value) in map {
                            if let Some(text) = value.as_str() {
                                values.push((key.clone(), text.to_string()));
                            }
                        }
                    }
                    Value::Array(list) => {
                        for item in list {
                            let name = item
                                .get("name")
                                .or_else(|| item.get("key"))
                                .and_then(Value::as_str);
                            let value = item.get("value").and_then(Value::as_str);
                            if let (Some(name), Some(value)) = (name, value) {
                                values.push((name.to_string(), value.to_string()));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    // First source wins per header name (project opencode.json usually first).
    let mut seen = std::collections::BTreeSet::new();
    values.retain(|(k, _)| seen.insert(k.clone()));
    values
}

/// ACP `session/new.mcpServers` payload for one agent.
///
/// Skips stdio servers the agent already owns (a second `blender` would give
/// two conflicting tool namespaces). **HTTP/SSE are never skipped for
/// "already native"**: Grok lists servers in `config.toml` that do not actually
/// attach in ACP (`_x.ai/mcp/servers_updated` only had `ue58_official` while
/// `ai-game-developer` was marked native and left uninjected).
pub fn mcp_payload_for_agent(
    project_root: &Path,
    agent_id: &str,
    supports_http: bool,
    supports_sse: bool,
) -> (Vec<Value>, Vec<String>) {
    let inventory = scan(project_root);
    let selection = load_selection(project_root);
    let mut payload = Vec::new();
    let mut skipped = Vec::new();

    for spec in inventory.mcp_servers {
        if !selection.is_enabled("mcp", &spec.id) {
            continue;
        }
        let claimed_native = spec.agents.iter().any(|a| a == agent_id);
        match spec.transport.as_str() {
            "http" | "sse" => {
                // Config-file ownership ≠ ACP session attachment for remote MCP.
                // Always inject when the user enabled lending + agent can do HTTP.
                let ok = if spec.transport == "sse" {
                    supports_sse
                } else {
                    supports_http
                };
                let Some(url) = spec.url.clone() else { continue };
                if !ok {
                    skipped.push(format!(
                        "{} ({} unsupported by agent)",
                        spec.name, spec.transport
                    ));
                    continue;
                }
                // ACP `HttpHeader { name, value }` — never ship an empty list when
                // the source config has Authorization (Unity MCP 401 otherwise).
                let headers: Vec<Value> = header_values_for(&spec)
                    .into_iter()
                    .map(|(name, value)| json!({ "name": name, "value": value }))
                    .collect();
                if headers.is_empty() && !spec.header_keys.is_empty() {
                    skipped.push(format!(
                        "{} (headers {:?} configured but unreadable)",
                        spec.name, spec.header_keys
                    ));
                    continue;
                }
                if claimed_native {
                    // Still inject; note so logs explain why we ignore "native".
                    skipped.push(format!(
                        "{} (config native — still injecting HTTP for ACP)",
                        spec.name
                    ));
                }
                let mut entry = Map::new();
                entry.insert("name".into(), json!(spec.name));
                entry.insert("url".into(), json!(url));
                entry.insert("headers".into(), json!(headers));
                entry.insert("type".into(), json!(spec.transport));
                payload.push(Value::Object(entry));
            }
            _ => {
                // Stdio: a second process with the same name is a real conflict.
                if claimed_native {
                    skipped.push(format!("{} (already native)", spec.name));
                    continue;
                }
                let Some(command) = spec.command.clone() else {
                    skipped.push(format!("{} (no command)", spec.name));
                    continue;
                };
                let env: Vec<Value> = env_values_for(&spec)
                    .into_iter()
                    .map(|(name, value)| json!({ "name": name, "value": value }))
                    .collect();
                payload.push(json!({
                    "name": spec.name,
                    "command": command,
                    "args": spec.args,
                    "env": env,
                }));
            }
        }
    }

    (payload, skipped)
}

/// Prompt preamble telling an agent about skills it does not ship with.
///
/// Cheap fallback for agents with no skill system of their own: the agent can
/// read `SKILL.md` itself — we only point at it.
pub fn skills_prompt_for_agent(project_root: &Path, agent_id: &str) -> Option<String> {
    let inventory = scan(project_root);
    let selection = load_selection(project_root);

    let lent: Vec<&SkillSpec> = inventory
        .skills
        .iter()
        .filter(|skill| selection.is_enabled("skill", &skill.id))
        .filter(|skill| !skill.agents.iter().any(|a| a == agent_id))
        .collect();
    if lent.is_empty() {
        return None;
    }

    let mut out = String::from(
        "[Marionette — skills available in this project]\nThese are instruction files on this machine. \
         If one matches the task, read its SKILL.md first and follow it. Ignore the rest.\n\n",
    );
    for skill in lent {
        out.push_str(&format!("- **{}**", skill.name));
        if !skill.description.is_empty() {
            out.push_str(&format!(" — {}", skill.description));
        }
        out.push_str(&format!("\n  `{}`\n", skill.file));
    }
    out.push_str("\n[End of skills list]\n\n");
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_comments_but_not_urls_inside_strings() {
        let input = r#"{
          // leading comment
          "url": "https://example.com/mcp", /* trailing */
          "path": "C:\\a\\b" // note
        }"#;
        let cleaned = strip_jsonc_comments(input);
        let value: Value = serde_json::from_str(&cleaned).unwrap();
        assert_eq!(value["url"], "https://example.com/mcp");
        assert_eq!(value["path"], r"C:\a\b");
    }

    #[test]
    fn reads_opencode_and_codex_shapes() {
        let root = std::env::temp_dir().join(format!("Marionette-ctx-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();

        let oc = root.join("opencode.jsonc");
        fs::write(
            &oc,
            r#"{
              // user config
              "mcp": {
                "blender": { "type": "local", "command": ["cmd", "/c", "uvx", "blender-mcp"], "enabled": true },
                "off": { "type": "local", "command": ["x"], "enabled": false },
                "ai-game-developer": {
                  "type": "remote",
                  "url": "http://localhost:23915",
                  "headers": { "Authorization": "Bearer secret-token" },
                  "enabled": true
                }
              }
            }"#,
        )
        .unwrap();

        let codex = root.join("config.toml");
        fs::write(
            &codex,
            "[mcp_servers.node_repl]\ncommand = 'node.exe'\nargs = []\n[mcp_servers.node_repl.env]\nTOKEN = \"secret\"\n\n[mcp_servers.remote]\nurl = \"https://ai-game.dev/mcp\"\n",
        )
        .unwrap();

        let mut out = Collector { servers: vec![], skills: vec![], notes: vec![] };
        scan_opencode_config(&oc, "opencode", Some("opencode"), &mut out);
        scan_codex_config(&codex, &mut out);

        let names: Vec<&str> = out.servers.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"blender"), "got {names:?}");
        assert!(!names.contains(&"off"), "disabled servers must be skipped");
        assert!(names.contains(&"node_repl"));

        let blender = out.servers.iter().find(|s| s.name == "blender").unwrap();
        assert_eq!(blender.command.as_deref(), Some("cmd"));
        assert_eq!(blender.args, vec!["/c", "uvx", "blender-mcp"]);
        assert_eq!(blender.agents, vec!["opencode"]);

        let repl = out.servers.iter().find(|s| s.name == "node_repl").unwrap();
        // Only key names are inventoried — the value never enters the model.
        assert_eq!(repl.env_keys, vec!["TOKEN"]);
        let serialized = serde_json::to_string(&repl).unwrap();
        assert!(!serialized.contains("secret"));

        let remote = out.servers.iter().find(|s| s.name == "remote").unwrap();
        assert_eq!(remote.transport, "http");

        let unity = out
            .servers
            .iter()
            .find(|s| s.name == "ai-game-developer")
            .unwrap();
        assert_eq!(unity.transport, "http");
        assert_eq!(unity.header_keys, vec!["Authorization".to_string()]);
        let inv_json = serde_json::to_string(&unity).unwrap();
        assert!(
            !inv_json.contains("secret-token"),
            "header values must not be inventoried"
        );

        fs::remove_dir_all(root).unwrap();
    }

    /// Unity MCP rejects unauthenticated HTTP; inject must carry Authorization.
    #[test]
    fn http_mcp_payload_includes_authorization_header() {
        let root = std::env::temp_dir().join(format!("Marionette-hdr-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let oc = root.join("opencode.json");
        fs::write(
            &oc,
            r#"{
              "mcp": {
                "ai-game-developer": {
                  "type": "remote",
                  "url": "http://localhost:23915",
                  "headers": { "Authorization": "Bearer secret-token" },
                  "enabled": true
                }
              }
            }"#,
        )
        .unwrap();
        // Enable lending for this project.
        fs::create_dir_all(root.join(".marionette")).unwrap();
        fs::write(
            root.join(".marionette/context.json"),
            r#"{"version":1,"mcpServers":{"ai-game-developer":true},"skills":{}}"#,
        )
        .unwrap();

        // Use an agent that does not natively own this server on the machine
        // (grok-build / opencode may already list it in ~/.grok or ~/.config).
        let (payload, skipped) = mcp_payload_for_agent(&root, "claude-code", true, true);
        assert!(
            !skipped.iter().any(|s| s.contains("already native")),
            "claude-code should receive a lend, not a native skip: {skipped:?}"
        );
        assert_eq!(payload.len(), 1, "expected inject for claude: {payload:?} skipped={skipped:?}");
        let headers = payload[0]
            .get("headers")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(!headers.is_empty(), "headers={headers:?}");
        assert_eq!(headers[0]["name"], "Authorization");
        let value = headers[0]["value"].as_str().unwrap_or("");
        assert!(
            value.starts_with("Bearer "),
            "expected Bearer token, got {value:?}"
        );
        // Inventory model stores key names only — never the secret.
        let inv = scan(&root);
        let unity = inv
            .mcp_servers
            .iter()
            .find(|s| s.name == "ai-game-developer")
            .unwrap();
        let inv_json = serde_json::to_string(unity).unwrap();
        assert!(
            !inv_json.contains("Bearer "),
            "header values must not appear in inventory JSON"
        );

        fs::remove_dir_all(root).unwrap();
    }

    /// Diagnostic against the real machine (`cargo test -- --ignored --nocapture`).
    /// Prints names only — an MCP env can hold API tokens.
    #[test]
    #[ignore]
    fn dump_local_inventory() {
        let inventory = scan(Path::new("."));
        println!("-- mcp servers --");
        for server in &inventory.mcp_servers {
            println!(
                "  {:<22} {:<6} sources={:<22} native={:?} envKeys={:?}",
                server.name,
                server.transport,
                server.sources.join("+"),
                server.agents,
                server.env_keys
            );
        }
        println!("-- skills ({}) --", inventory.skills.len());
        for skill in &inventory.skills {
            println!(
                "  {:<20} {:<24} native={:?}",
                skill.id,
                skill.sources.join("+"),
                skill.agents
            );
        }
        println!("-- notes -- {:?}", inventory.notes);
    }

    #[test]
    fn selection_defaults_lend_skills_but_not_servers() {
        let selection = ContextSelection::default();
        assert!(selection.is_enabled("skill", "unity"));
        assert!(!selection.is_enabled("mcp", "blender"));
    }

    #[test]
    fn ensure_grok_folder_trust_writes_trusted_entry() {
        let home = std::env::temp_dir().join(format!("Marionette-trust-home-{}", std::process::id()));
        let project = std::env::temp_dir().join(format!("Marionette-trust-proj-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let _ = fs::remove_dir_all(&project);
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&project).unwrap();
        // Point USERPROFILE at temp so we don't touch the real store.
        let prev = std::env::var_os("USERPROFILE");
        std::env::set_var("USERPROFILE", &home);
        #[cfg(not(windows))]
        {
            let prev_home = std::env::var_os("HOME");
            std::env::set_var("HOME", &home);
            ensure_grok_folder_trust(&project);
            if let Some(v) = prev_home {
                std::env::set_var("HOME", v);
            } else {
                std::env::remove_var("HOME");
            }
        }
        #[cfg(windows)]
        {
            ensure_grok_folder_trust(&project);
        }
        if let Some(v) = prev {
            std::env::set_var("USERPROFILE", v);
        } else {
            std::env::remove_var("USERPROFILE");
        }

        let raw = fs::read_to_string(home.join(".grok/trusted_folders.toml")).unwrap();
        assert!(raw.contains("trusted = true"), "{raw}");
        assert!(
            raw.contains("[folders.") || raw.contains("folders."),
            "expected a folders table in:\n{raw}"
        );
        // Path may be canonicalized / slash-normalized — just require the unique leaf.
        let leaf = project
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("Marionette-trust-proj");
        assert!(
            raw.contains(leaf),
            "expected path leaf {leaf} in:\n{raw}"
        );
        let _ = fs::remove_dir_all(&home);
        let _ = fs::remove_dir_all(&project);
    }

    #[test]
    fn selection_round_trips_and_keeps_defaults_for_untouched_items() {
        let root = std::env::temp_dir().join(format!("Marionette-sel-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();

        set_enabled(&root, "mcp", "blender", true).unwrap();
        set_enabled(&root, "skill", "docx", false).unwrap();

        let reloaded = load_selection(&root);
        assert!(reloaded.is_enabled("mcp", "blender"), "explicit yes must persist");
        assert!(!reloaded.is_enabled("skill", "docx"), "explicit no must persist");
        // Untouched items keep the per-kind default.
        assert!(!reloaded.is_enabled("mcp", "notion"));
        assert!(reloaded.is_enabled("skill", "unity-tools"));

        // Reference only — a selection file must never carry credentials.
        let raw = fs::read_to_string(root.join(".marionette/context.json")).unwrap();
        assert!(raw.contains("blender"));
        assert!(!raw.to_lowercase().contains("token"));

        fs::remove_dir_all(root).unwrap();
    }

    /// Screenshots and art folders are where spaces live, and the composer's
    /// path regex stops at whitespace — so the frontend now forwards the exact
    /// path the OS reported on drop instead of re-parsing it out of the text.
    /// This pins the half of that contract that lives here.
    #[test]
    fn a_dropped_image_in_a_folder_with_spaces_still_needs_a_grant() {
        let root = std::env::temp_dir().join(format!("Marionette-spaces-{}", std::process::id()));
        let project = root.join("project");
        let outside = root.join("My Pictures").join("角色 立绘");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let image = outside.join("Screen Shot 2026.png");
        fs::write(&image, "x").unwrap();

        let found = outside_project_paths(&project, &[image.to_string_lossy().to_string()]);
        assert_eq!(
            found.len(),
            1,
            "a path with spaces must still be flagged for a grant: {found:?}"
        );
        assert_eq!(
            found[0]["dir"].as_str().unwrap().to_lowercase(),
            outside.to_string_lossy().to_lowercase(),
            "a file grants its folder, spaces and all"
        );

        // Best-effort: a temp-dir cleanup failing must not fail a grant test.
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_roots_grant_dedupe_and_detect_outside_paths() {
        let root = std::env::temp_dir().join(format!("Marionette-roots-{}", std::process::id()));
        let project = root.join("project");
        let outside = root.join("pictures").join("screenshots");
        fs::create_dir_all(project.join("src")).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let image = outside.join("shot.png");
        fs::write(&image, "x").unwrap();
        fs::write(project.join("src/main.rs"), "x").unwrap();

        let inside = project.join("src/main.rs").to_string_lossy().to_string();
        let image_path = image.to_string_lossy().to_string();

        // Before granting: the outside file is flagged, the project file is not.
        let found = outside_project_paths(&project, &[inside.clone(), image_path.clone()]);
        assert_eq!(found.len(), 1, "only the outside path needs a grant: {found:?}");
        assert_eq!(
            found[0]["dir"].as_str().unwrap().to_lowercase(),
            outside.to_string_lossy().to_lowercase(),
            "a file grants its folder, not itself"
        );

        // After granting the folder, nothing is left to ask about.
        grant_workspace_root(&project, &outside.to_string_lossy()).unwrap();
        assert!(outside_project_paths(&project, &[image_path.clone()]).is_empty());
        assert_eq!(workspace_roots(&project).len(), 1);

        // Granting a parent replaces the narrower grant instead of stacking.
        let parent = root.join("pictures");
        grant_workspace_root(&project, &parent.to_string_lossy()).unwrap();
        let roots = workspace_roots(&project);
        assert_eq!(roots.len(), 1, "narrower root should be folded in: {roots:?}");
        // Re-granting a child of an existing root is a no-op.
        grant_workspace_root(&project, &outside.to_string_lossy()).unwrap();
        assert_eq!(workspace_roots(&project).len(), 1);

        revoke_workspace_root(&project, &parent.to_string_lossy()).unwrap();
        assert!(workspace_roots(&project).is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn opencode_permission_env_merges_and_uses_measured_pattern() {
        let root = std::env::temp_dir().join(format!("Marionette-perm-{}", std::process::id()));
        let project = root.join("project");
        let granted = root.join("shots");
        fs::create_dir_all(&project).unwrap();
        fs::create_dir_all(&granted).unwrap();

        assert!(opencode_permission_env(&project, None).is_none(), "nothing granted → no env");

        grant_workspace_root(&project, &granted.to_string_lossy()).unwrap();
        let value = opencode_permission_env(&project, None).unwrap();
        let parsed: Value = serde_json::from_str(&value).unwrap();
        let external = parsed["external_directory"].as_object().unwrap();
        let expected = format!("{}/*", granted.to_string_lossy().replace('\\', "/"));
        assert_eq!(external[&expected], "allow", "got {value}");

        // A user's own OPENCODE_PERMISSION must survive.
        let merged = opencode_permission_env(
            &project,
            Some(r#"{"edit":"deny","external_directory":{"D:/keep/*":"allow"}}"#),
        )
        .unwrap();
        let parsed: Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(parsed["edit"], "deny");
        assert_eq!(parsed["external_directory"]["D:/keep/*"], "allow");
        assert_eq!(parsed["external_directory"][&expected], "allow");

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inside_check_survives_separators_and_case() {
        assert!(is_inside(r"D:\Myself\AgentsShell", r"D:\Myself\AgentsShell\src\main.rs"));
        // Extended prefix + forward slashes + different case still match.
        assert!(is_inside(r"D:\Myself\AgentsShell", r"\\?\d:/myself/agentsshell/src"));
        assert!(!is_inside(r"D:\Myself\AgentsShell", r"D:\Myself\AgentsShellOther\x"));
        assert!(!is_inside(r"D:\Myself\AgentsShell", r"C:\Users\me\pic.png"));
    }

    #[test]
    fn skills_prompt_skips_agents_that_already_have_them() {
        let root = std::env::temp_dir().join(format!("Marionette-skills-{}", std::process::id()));
        let dir = root.join(".marionette/skills/unity-tools");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            "---\nname: unity-tools\ndescription: Unity project helpers\n---\n\nbody\n",
        )
        .unwrap();

        let prompt = skills_prompt_for_agent(&root, "grok-build").unwrap();
        assert!(prompt.contains("unity-tools"));
        assert!(prompt.contains("Unity project helpers"));

        // Same skill, but sourced from the agent's own directory → not lent back.
        let mut out = Collector { servers: vec![], skills: vec![], notes: vec![] };
        scan_skills_dir(
            &root.join(".marionette/skills"),
            "project",
            Some("grok-build"),
            &mut out,
        );
        assert_eq!(out.skills.len(), 1);
        assert_eq!(out.skills[0].agents, vec!["grok-build"]);

        fs::remove_dir_all(root).unwrap();
    }
}
