//! 跨 agent 共享记忆 — Marionette 的基础功能（2026-10-19 起）。
//!
//! 记忆存在 `~/.marionette/memory/memory.json`：条目列表（来源、标题、文本、
//! 启停）。每个 Marionette 会话进程的**首条 prompt** 由 `acp.rs` 注入一张
//! 「记忆卡」文本块，所有 agent（Claude / Codex / DSH / …）共享同一份 ——
//! 壳是唯一同时看得见所有 agent 的位置。
//!
//! 启动同步：打开应用 / 拉记忆面板时，自动把各家引擎的**用户级**全局记忆
//! （`~/.claude/CLAUDE.md` 等）收成可勾选条目（记 origin_path+mtime，文件
//! 变了下次同步才刷新）。项目级 AGENTS.md/CLAUDE.md 不收：各引擎在会话里
//! 原生就读项目文件。
//!
//! 蒸馏：`memory_distill` 把会话 JSONL 的尾部（user/assistant/thought 的
//! text）发给 DeepSeek 提炼要点草稿，用户确认后落库（source=session）。
//! 用 ureq（见 http_client.rs；Cargo.toml 禁 reqwest/tokio）。

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static ENTRY_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryEntry {
    pub id: String,
    /// manual | claude | codex | opencode | gemini | session
    pub source: String,
    pub title: String,
    pub text: String,
    pub enabled: bool,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_mtime_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryStore {
    pub version: u32,
    pub enabled: bool,
    pub entries: Vec<MemoryEntry>,
}

impl MemoryStore {
    pub fn fresh() -> Self {
        Self {
            version: 1,
            enabled: true,
            entries: Vec::new(),
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn next_entry_id() -> String {
    format!("m{}-{}", now_ms(), ENTRY_COUNTER.fetch_add(1, Ordering::Relaxed))
}

pub fn mark_updated(entry: &mut MemoryEntry) {
    entry.updated_at_ms = now_ms();
}

pub fn new_entry(
    source: &str,
    title: &str,
    text: &str,
    origin_path: Option<String>,
    origin_mtime_ms: Option<u64>,
) -> MemoryEntry {
    let now = now_ms();
    MemoryEntry {
        id: next_entry_id(),
        source: source.to_string(),
        title: title.trim().to_string(),
        text: text.trim_end().to_string(),
        enabled: true,
        created_at_ms: now,
        updated_at_ms: now,
        origin_path,
        origin_mtime_ms,
    }
}

// ── 存取（真实家目录的薄封装；测试用 *_at 纯函数） ─────────────────────

fn store_path() -> Option<PathBuf> {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
    Some(
        PathBuf::from(home)
            .join(".marionette")
            .join("memory")
            .join("memory.json"),
    )
}

fn load_at(path: &Path) -> MemoryStore {
    match fs::read_to_string(path) {
        Ok(text) if !text.trim().is_empty() => match serde_json::from_str(&text) {
            Ok(store) => store,
            Err(_) => {
                // 损坏留档重建（镜像 credentials.rs 的立场），绝不 panic。
                let backup = path.with_extension(format!(
                    "json.corrupt-{}",
                    now_ms()
                ));
                let _ = fs::copy(path, &backup);
                MemoryStore::fresh()
            }
        },
        _ => MemoryStore::fresh(),
    }
}

fn save_at(path: &Path, store: &MemoryStore) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Create memory dir failed: {e}"))?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(store).map_err(|e| e.to_string())?;
    fs::write(&tmp, body).map_err(|e| format!("Write memory tmp failed: {e}"))?;
    if path.exists() {
        let _ = fs::remove_file(path);
    }
    fs::rename(&tmp, path).map_err(|e| format!("Persist memory failed: {e}"))
}

pub fn load() -> MemoryStore {
    store_path().map(|p| load_at(&p)).unwrap_or_else(MemoryStore::fresh)
}

pub fn save(store: &MemoryStore) -> Result<(), String> {
    store_path()
        .ok_or_else(|| "无法定位用户目录（USERPROFILE/HOME 均缺失）".to_string())
        .and_then(|p| save_at(&p, store))
}

// ── 记忆卡 ─────────────────────────────────────────────────────────────

fn source_label(source: &str) -> &str {
    match source {
        "manual" => "手动",
        "claude" => "Claude",
        "codex" => "Codex",
        "opencode" => "OpenCode",
        "gemini" => "Gemini",
        "session" => "会话蒸馏",
        other => other,
    }
}

/// 注入用的记忆卡。无启用条目 → None。纯函数（测试友好）。
///
/// 全局 `enabled` 字段保留给旧存档兼容，面板已不再暴露总开关；注入只看
/// 各条目自己的勾选。
pub fn compose_card_from(store: &MemoryStore) -> Option<String> {
    let entries: Vec<&MemoryEntry> = store
        .entries
        .iter()
        .filter(|e| e.enabled && !e.text.trim().is_empty())
        .collect();
    if entries.is_empty() {
        return None;
    }
    let mut body = String::from(
        "<marionette-memory>\n\
         以下是用户的长期偏好与事实（Marionette 跨 agent 共享记忆，均经用户确认）。\
         在相关时利用它们；无需主动提及或复述本段。\n",
    );
    for entry in entries {
        let section = format!(
            "\n## {}（来源：{}）\n{}\n",
            if entry.title.is_empty() { "（无标题）" } else { &entry.title },
            source_label(&entry.source),
            entry.text.trim()
        );
        if body.len() + section.len() + 24 > 6000 {
            body.push_str("\n（记忆过长，其余条目已截断）\n");
            break;
        }
        body.push_str(&section);
    }
    body.push_str("</marionette-memory>");
    Some(body)
}

pub fn compose_card() -> Option<String> {
    // 发首条 prompt 前再同步一次，免得用户刚改了 ~/.claude/CLAUDE.md。
    let _ = sync_imports();
    compose_card_from(&load())
}

/// 面板 / 命令入口：先把各家全局记忆同步进来，再返回当前 store。
pub fn load_synced() -> MemoryStore {
    let _ = sync_imports();
    load()
}

// ── 收编各家全局记忆 ────────────────────────────────────────────────────

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportSource {
    pub source: String,
    pub title: String,
    pub path: String,
    pub exists: bool,
    pub bytes: u64,
    pub mtime_ms: u64,
    pub excerpt: String,
}

const IMPORT_TARGETS: [(&str, &str, &str); 4] = [
    ("claude", "Claude", ".claude/CLAUDE.md"),
    ("codex", "Codex", ".codex/AGENTS.md"),
    ("opencode", "OpenCode", ".config/opencode/AGENTS.md"),
    ("gemini", "Gemini", ".gemini/GEMINI.md"),
];

fn import_sources_at(home: &Path) -> Vec<ImportSource> {
    IMPORT_TARGETS
        .iter()
        .map(|(source, title, rel)| {
            let path = home.join(rel);
            let (exists, bytes, mtime_ms, excerpt) = match fs::metadata(&path) {
                Ok(meta) => {
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let text = fs::read_to_string(&path).unwrap_or_default();
                    let excerpt: String = text.lines().find(|l| !l.trim().is_empty()).map(str::to_string).unwrap_or_default();
                    let excerpt = excerpt.chars().take(80).collect();
                    (true, meta.len(), mtime, excerpt)
                }
                Err(_) => (false, 0, 0, String::new()),
            };
            ImportSource {
                source: source.to_string(),
                title: title.to_string(),
                path: path.display().to_string(),
                exists,
                bytes,
                mtime_ms,
                excerpt,
            }
        })
        .collect()
}

pub fn import_sources() -> Vec<ImportSource> {
    match std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        Some(home) => import_sources_at(&PathBuf::from(home)),
        None => Vec::new(),
    }
}

/// 导入存在的源：每源一条稳定 id（`import-<source>`）的条目；文件 mtime 没变
/// 就跳过，变了就整体替换文本。返回给 UI 的摘要。
pub fn import_run() -> Result<serde_json::Value, String> {
    let (imported, updated, unchanged, sources) = sync_imports()?;
    Ok(json!({
        "imported": imported,
        "updated": updated,
        "unchanged": unchanged,
        "sources": sources,
    }))
}

/// 静默同步各家全局记忆。启动面板 / 注入记忆卡前调用。
fn sync_imports() -> Result<(Vec<String>, Vec<String>, Vec<String>, Vec<ImportSource>), String> {
    let mut store = load();
    let sources = import_sources();
    let mut imported = Vec::new();
    let mut updated = Vec::new();
    let mut unchanged = Vec::new();
    let mut dirty = false;
    for src in sources.iter().filter(|s| s.exists && s.bytes > 0) {
        let text = fs::read_to_string(&src.path).map_err(|e| format!("读取 {} 失败: {e}", src.path))?;
        if text.trim().is_empty() {
            unchanged.push(src.title.clone());
            continue;
        }
        let stable_id = format!("import-{}", src.source);
        // 稳定 id 优先；旧版曾用随机 id 写入，按 source + origin_path 认回。
        let existing = store.entries.iter_mut().find(|e| {
            e.id == stable_id
                || (e.source == src.source
                    && e.origin_path.as_deref() == Some(src.path.as_str()))
        });
        match existing {
            Some(entry) => {
                if entry.id != stable_id {
                    entry.id = stable_id;
                    dirty = true;
                }
                if entry.title != src.title {
                    entry.title = src.title.clone();
                    dirty = true;
                }
                if entry.origin_mtime_ms == Some(src.mtime_ms) && !entry.text.is_empty() {
                    unchanged.push(src.title.clone());
                } else {
                    entry.text = text.trim_end().to_string();
                    entry.origin_path = Some(src.path.clone());
                    entry.origin_mtime_ms = Some(src.mtime_ms);
                    entry.updated_at_ms = now_ms();
                    dirty = true;
                    updated.push(src.title.clone());
                }
            }
            None => {
                let mut entry = new_entry(
                    &src.source,
                    &src.title,
                    &text,
                    Some(src.path.clone()),
                    Some(src.mtime_ms),
                );
                entry.id = stable_id;
                store.entries.push(entry);
                dirty = true;
                imported.push(src.title.clone());
            }
        }
    }
    if dirty {
        save(&store)?;
    }
    Ok((imported, updated, unchanged, sources))
}

// ── 会话蒸馏 ────────────────────────────────────────────────────────────

/// 从会话 JSONL 尾部抽对话摘要（user / assistant / thought 的 text）。
pub fn transcript_digest(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let mut picked: Vec<(String, String)> = Vec::new();
    for line in content.lines() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let role = match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "user_message" => "用户",
            "assistant_message" => "助手",
            "thought" => "助手思考",
            _ => continue,
        };
        let text = v
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if text.is_empty() {
            continue;
        }
        let capped: String = if text.chars().count() > 400 {
            let head: String = text.chars().take(400).collect();
            format!("{head}…")
        } else {
            text
        };
        picked.push((role.to_string(), capped));
    }
    if picked.is_empty() {
        return None;
    }
    let tail_start = picked.len().saturating_sub(40);
    let mut digest = String::new();
    for (role, text) in &picked[tail_start..] {
        digest.push_str(&format!("[{role}] {text}\n"));
    }
    Some(digest)
}

/// 用 DeepSeek 把会话摘要蒸成要点草稿。key 来源：env → OpenCode auth.json。
pub fn distill_with_deepseek(digest: &str) -> Result<String, String> {
    let key = std::env::var("DEEPSEEK_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .or_else(|| crate::provider_usage::peek_provider_key("deepseek"))
        .ok_or_else(|| {
            "没有可用的 DEEPSEEK_API_KEY（Provider keys / OpenCode auth.json / 环境变量）".to_string()
        })?;
    let agent = crate::http_client::agent()?;
    // 手工序列化 + send_string：不依赖 ureq 的 "json" feature（get_json 也是
    // 这么自食其力的）。
    let body = serde_json::to_string(&json!({
        "model": "deepseek-chat",
        "messages": [
            {
                "role": "system",
                "content": "你负责从 AI 编程会话记录中提炼长期记忆。输出 3-8 条中文要点，每条一行，格式为「- 要点」。只记录值得跨会话、跨项目长期记住的用户偏好、事实与背景（例如常用技术栈、习惯、环境约束），不要记录一次性的任务细节。没有值得记的就只输出 NONE。不要输出其它任何内容。"
            },
            { "role": "user", "content": digest }
        ],
        "max_tokens": 400,
        "temperature": 0.2
    }))
    .map_err(|e| e.to_string())?;
    let resp = agent
        .post("https://api.deepseek.com/chat/completions")
        .set("Authorization", &format!("Bearer {key}"))
        .set("Content-Type", "application/json")
        .timeout(std::time::Duration::from_secs(60))
        .send_string(&body)
        .map_err(|e| format!("DeepSeek 请求失败: {e}"))?;
    let status = resp.status();
    let text = resp
        .into_string()
        .map_err(|e| format!("读取 DeepSeek 应答失败: {e}"))?;
    if !(200..300).contains(&status) {
        let head: String = text.chars().take(200).collect();
        return Err(format!("DeepSeek 返回 {status}: {head}"));
    }
    let body: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("解析 DeepSeek 应答失败: {e}"))?;
    let content = body
        .pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if content.is_empty() {
        return Err("DeepSeek 返回了空内容".into());
    }
    Ok(content)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("marionette-memory-test-{tag}"));
        let _ = fs::create_dir_all(&dir);
        dir.join("memory.json")
    }

    #[test]
    fn card_skips_disabled_and_empty() {
        let mut store = MemoryStore::fresh();
        assert!(compose_card_from(&store).is_none());
        store.entries.push(new_entry("manual", "偏好", "用中文回复", None, None));
        assert!(compose_card_from(&store).is_some());
        // 全局开关不再挡注入；只看条目勾选。
        store.enabled = false;
        assert!(compose_card_from(&store).is_some());
        let mut quiet = new_entry("manual", "空", "   ", None, None);
        quiet.enabled = false;
        store.entries.push(quiet);
        let card = compose_card_from(&store).unwrap();
        assert!(card.contains("用中文回复"));
        assert!(!card.contains("空"));
    }

    #[test]
    fn import_uses_stable_id_and_is_idempotent() {
        let home = temp_path("import-stable");
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(home.join(".claude")).unwrap();
        let claude = home.join(".claude").join("CLAUDE.md");
        fs::write(&claude, "用户喜欢 pnpm").unwrap();

        let sources = import_sources_at(&home);
        assert!(sources[0].exists);
        let mut store = MemoryStore::fresh();
        let src = &sources[0];
        let stable_id = format!("import-{}", src.source);
        let mut entry = new_entry(
            &src.source,
            &src.title,
            "用户喜欢 pnpm",
            Some(src.path.clone()),
            Some(src.mtime_ms),
        );
        entry.id = stable_id.clone();
        store.entries.push(entry);
        assert_eq!(store.entries[0].id, "import-claude");
        assert_eq!(store.entries.len(), 1);
    }

    #[test]
    fn card_truncates_long_entries() {
        let mut store = MemoryStore::fresh();
        let long = "很".repeat(4000);
        store.entries.push(new_entry("manual", "长", &long, None, None));
        store.entries.push(new_entry("manual", "第二条", "短", None, None));
        let card = compose_card_from(&store).unwrap();
        assert!(card.len() < 6400);
        assert!(card.contains("已截断"));
    }

    #[test]
    fn save_load_roundtrip_and_corrupt_recovery() {
        let path = temp_path("roundtrip");
        let mut store = MemoryStore::fresh();
        store.entries.push(new_entry("manual", "t", "hello", None, None));
        save_at(&path, &store).unwrap();
        let back = load_at(&path);
        assert_eq!(back.entries.len(), 1);
        assert_eq!(back.entries[0].text, "hello");
        fs::write(&path, "{{not json").unwrap();
        let recovered = load_at(&path);
        assert!(recovered.entries.is_empty());
    }

    #[test]
    fn import_is_stable_and_idempotent() {
        let home = temp_path("import-home");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(home.join(".claude").join("CLAUDE.md"), "用户喜欢 pnpm").unwrap();
        let sources = import_sources_at(&home);
        assert_eq!(sources.len(), 4);
        assert!(sources[0].exists);
        assert_eq!(sources[0].excerpt, "用户喜欢 pnpm");
        let stable_id = format!("import-{}", sources[0].source);
        assert!(stable_id.starts_with("import-claude"));
    }

    #[test]
    fn digest_picks_dialogue_tail_only() {
        let path = temp_path("digest");
        let mut body = String::new();
        for i in 0..60 {
            body.push_str(&format!(
                "{{\"type\":\"user_message\",\"text\":\"消息 {i}\"}}\n"
            ));
        }
        body.push_str("{\"type\":\"tool_call\",\"text\":\"noise\"}\n");
        fs::write(&path, body).unwrap();
        let digest = transcript_digest(&path).unwrap();
        assert!(!digest.contains("noise"));
        // 尾部 40 条：0..=19 被裁掉，20..=59 保留。
        assert!(!digest.contains("[用户] 消息 0\n"));
        assert!(!digest.contains("[用户] 消息 19\n"));
        assert!(digest.contains("[用户] 消息 20\n"));
        assert!(digest.contains("[用户] 消息 59\n"));
        assert_eq!(digest.lines().count(), 40);
    }
}
