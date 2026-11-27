//! Cheap text generation for auxiliary copy (P1-13).
//!
//! One configured low-cost text model produces semantic session titles, branch
//! names and commit messages. This path is deliberately separate from coding
//! agent turns:
//!
//! - It never goes through an agent process or the engine gateway, so it cannot
//!   delay `session/prompt` and is never counted in an agent's quota totals.
//! - Sources and keys are the existing ones: `providers.json` (the engine's
//!   source store) plus the encrypted `credentials.json` entries (DPAPI on
//!   Windows). No second key store is introduced.
//! - Usage lands in its own append-only file (`~/.marionette/cheap-text/usage.jsonl`)
//!   labelled `source: "cheap-text"`, so the agent usage ledger stays clean.
//! - When the config is off, the source/model is missing, the key cannot be
//!   decrypted, or the network fails, callers get `None` and the existing
//!   first-message title / hand-written message stays. Failures are silent to
//!   the user and logged without prompt or response bodies.
//!
//! The automatic-title path is asynchronous and event driven:
//! [`spawn_session_title`] emits [`CHEAP_TITLE_EVENT`] with
//! `{ sessionId, title }` only on success. Frontend wiring is left to the
//! caller so this stays backend-only.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter};

/// Emitted only when a semantic title was produced. Payload:
/// `{ "sessionId": "…", "title": "…" }`.
pub const CHEAP_TITLE_EVENT: &str = "marionette-cheap-title";

/// Usage rows are labelled with this source and never mixed into the agent ledger.
pub const USAGE_SOURCE: &str = "cheap-text";

const CONFIG_FILE: &str = "cheap-text.json";
const USAGE_DIR: &str = "cheap-text";
const USAGE_FILE: &str = "usage.jsonl";
const USAGE_ROTATED: &str = "usage.jsonl.1";
const USAGE_ROTATE_BYTES: u64 = 2 * 1024 * 1024;

const DEFAULT_MAX_TOKENS: u32 = 96;
const MAX_TOKENS_CAP: u32 = 512;
const DEFAULT_TIMEOUT_MS: u64 = 8_000;
const MIN_TIMEOUT_MS: u64 = 1_500;
const MAX_TIMEOUT_MS: u64 = 30_000;

/// Input sent to the model is bounded per call; cheap models do the rest.
const INPUT_CHAR_CAP: usize = 2_000;
/// Response body cap (a title answer is a few hundred bytes; cap protects memory).
const RESPONSE_BYTE_CAP: u64 = 512 * 1024;
const TITLE_CHAR_CAP: usize = 80;
const BRANCH_CHAR_CAP: usize = 60;
const COMMIT_CHAR_CAP: usize = 200;

/// What kind of auxiliary copy is requested.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheapTextKind {
    Title,
    Branch,
    Commit,
}

impl CheapTextKind {
    fn key(self) -> &'static str {
        match self {
            Self::Title => "title",
            Self::Branch => "branch",
            Self::Commit => "commit",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "title" | "session_title" => Some(Self::Title),
            "branch" | "branch_name" => Some(Self::Branch),
            "commit" | "commit_message" => Some(Self::Commit),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CheapTextConfig {
    /// On by default, but inert until `source` and `model` are set.
    pub enabled: bool,
    /// Provider id in `providers.json`, or a `credentials.json` entry id.
    pub source: String,
    /// Model id as the source spells it.
    pub model: String,
    /// Optional extra instructions for session titles.
    pub title_style: String,
    pub max_tokens: u32,
    pub timeout_ms: u64,
}

impl Default for CheapTextConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            source: String::new(),
            model: String::new(),
            title_style: String::new(),
            max_tokens: DEFAULT_MAX_TOKENS,
            timeout_ms: DEFAULT_TIMEOUT_MS,
        }
    }
}

impl CheapTextConfig {
    fn normalized(mut self) -> Self {
        self.source = self.source.trim().to_string();
        self.model = self.model.trim().to_string();
        self.title_style = self.title_style.trim().chars().take(400).collect();
        if self.max_tokens == 0 {
            self.max_tokens = DEFAULT_MAX_TOKENS;
        }
        self.max_tokens = self.max_tokens.clamp(16, MAX_TOKENS_CAP);
        if self.timeout_ms == 0 {
            self.timeout_ms = DEFAULT_TIMEOUT_MS;
        }
        self.timeout_ms = self.timeout_ms.clamp(MIN_TIMEOUT_MS, MAX_TIMEOUT_MS);
        self
    }

    fn configured(&self) -> bool {
        !self.source.is_empty() && !self.model.is_empty()
    }
}

/// Redacted settings view: never carries a key or a resolved endpoint.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheapTextSettingsView {
    pub enabled: bool,
    pub source: String,
    pub model: String,
    pub title_style: String,
    pub max_tokens: u32,
    pub timeout_ms: u64,
    /// Source and model both present.
    pub configured: bool,
    /// A key/endpoint resolves right now (read-only, no network).
    pub credential_ready: bool,
}

/// One selectable source in the settings dialog.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheapTextSourceView {
    pub id: String,
    pub label: String,
    /// "chat" when an OpenAI-compatible endpoint exists, else "anthropic",
    /// else "none" (not usable for cheap text).
    pub protocol: String,
    /// Pinned list when the source pins models; otherwise catalogue rows.
    pub models: Vec<String>,
    /// A key is present in the source store or the encrypted credential store.
    pub has_key: bool,
}

// ─── paths ───────────────────────────────────────────────────────────────

#[cfg(test)]
static ROOT_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

#[cfg(test)]
fn set_root_override_for_tests(path: Option<PathBuf>) {
    *ROOT_OVERRIDE.lock().unwrap_or_else(|p| p.into_inner()) = path;
}

#[cfg(test)]
fn test_root_override() -> Option<PathBuf> {
    ROOT_OVERRIDE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
}

#[cfg(not(test))]
fn test_root_override() -> Option<PathBuf> {
    None
}

fn root() -> Result<PathBuf, String> {
    if let Some(path) = test_root_override() {
        return Ok(path);
    }
    if cfg!(test) {
        // Tests must never read or write the real ~/.marionette.
        return Err("cheap_text root not set for tests".to_string());
    }
    crate::app_paths::global_dir()
}

fn config_path(root: &Path) -> PathBuf {
    root.join(CONFIG_FILE)
}

fn providers_path(root: &Path) -> PathBuf {
    root.join("engine").join("data").join("providers.json")
}

/// Old magpie layout (`~/.config/magpie`) is still read as a fallback; the
/// engine's own copy under `~/.marionette/engine/data` always wins.
fn legacy_providers_path() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)?;
    Some(home.join(".config").join("magpie").join("providers.json"))
}

fn credentials_path(root: &Path) -> PathBuf {
    root.join("credentials.json")
}

fn usage_path(root: &Path) -> PathBuf {
    root.join(USAGE_DIR).join(USAGE_FILE)
}

// ─── config ──────────────────────────────────────────────────────────────

fn read_config_at(root: &Path) -> CheapTextConfig {
    let Ok(text) = fs::read_to_string(config_path(root)) else {
        return CheapTextConfig::default().normalized();
    };
    serde_json::from_str::<CheapTextConfig>(&text)
        .unwrap_or_default()
        .normalized()
}

fn write_config_at(root: &Path, config: &CheapTextConfig) -> Result<(), String> {
    let body = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    if let Err(error) = fs::create_dir_all(root) {
        return Err(format!("create config dir failed: {error}"));
    }
    let path = config_path(root);
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&temp, format!("{body}\n")).map_err(|e| format!("write temp config failed: {e}"))?;
    if let Err(error) = fs::rename(&temp, &path) {
        let _ = fs::remove_file(&temp);
        return Err(format!("replace config failed: {error}"));
    }
    Ok(())
}

// ─── source / key resolution (existing stores only) ──────────────────────

#[derive(Clone, Debug, PartialEq, Eq)]
enum Protocol {
    Chat,
    Anthropic,
}

impl Protocol {
    fn key(&self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::Anthropic => "anthropic",
        }
    }
}

#[derive(Clone, Debug)]
struct ResolvedSource {
    id: String,
    protocol: Protocol,
    endpoint: String,
    key: String,
    headers: Vec<(String, String)>,
}

#[derive(Debug, Default, Deserialize)]
struct ProviderRow {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    chat: String,
    #[serde(default)]
    anthropic: String,
    #[serde(default)]
    preset: String,
    #[serde(default)]
    models: Option<Vec<String>>,
    #[serde(default)]
    headers: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
struct ProvidersFile {
    #[serde(default)]
    providers: Vec<ProviderRow>,
}

fn read_providers(path: &Path) -> Vec<ProviderRow> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str::<ProvidersFile>(&text)
        .map(|file| file.providers)
        .unwrap_or_default()
}

fn credential_key(root: &Path, id: &str, preset: &str) -> Option<String> {
    let path = credentials_path(root);
    let store = crate::credentials::load_store_at(&path).ok()?;
    let entry = store.entries.iter().find(|entry| {
        entry.id.eq_ignore_ascii_case(id)
            || (!preset.is_empty()
                && entry
                    .preset
                    .as_deref()
                    .is_some_and(|value| value.eq_ignore_ascii_case(preset)))
    })?;
    if entry
        .secret
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .is_empty()
    {
        return None;
    }
    crate::credentials::peek_secret_at(&path, &entry.id)
        .ok()
        .flatten()
        .filter(|key| !key.trim().is_empty())
}

fn protocol_of(chat: &str, anthropic: &str) -> Option<(Protocol, String)> {
    if !chat.trim().is_empty() {
        return Some((Protocol::Chat, chat.trim().to_string()));
    }
    if !anthropic.trim().is_empty() {
        return Some((Protocol::Anthropic, anthropic.trim().to_string()));
    }
    None
}

fn resolve_source(root: &Path, config: &CheapTextConfig) -> Option<ResolvedSource> {
    let wanted = config.source.trim();
    if wanted.is_empty() {
        return None;
    }

    let mut rows = read_providers(&providers_path(root));
    if rows.is_empty() {
        if let Some(legacy) = legacy_providers_path() {
            rows = read_providers(&legacy);
        }
    }
    if let Some(row) = rows
        .iter()
        .find(|row| row.id.eq_ignore_ascii_case(wanted) || row.name.eq_ignore_ascii_case(wanted))
    {
        let (protocol, endpoint) = protocol_of(&row.chat, &row.anthropic)?;
        let key = if !row.key.trim().is_empty() {
            row.key.trim().to_string()
        } else {
            credential_key(root, &row.id, &row.preset)?
        };
        return Some(ResolvedSource {
            id: row.id.clone(),
            protocol,
            endpoint,
            key,
            headers: row
                .headers
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        });
    }

    // The engine may never have run: a manual custom source still lives in the
    // encrypted credential store with its endpoint and protocol.
    let path = credentials_path(root);
    let store = crate::credentials::load_store_at(&path).ok()?;
    let entry = store
        .entries
        .iter()
        .find(|entry| entry.id.eq_ignore_ascii_case(wanted))?;
    let custom = entry.custom_provider.as_ref()?;
    let protocol = match custom.protocol.as_str() {
        "chat" => Protocol::Chat,
        "anthropic" => Protocol::Anthropic,
        _ => return None,
    };
    let key = crate::credentials::peek_secret_at(&path, &entry.id)
        .ok()
        .flatten()
        .filter(|key| !key.trim().is_empty())?;
    Some(ResolvedSource {
        id: entry.id.clone(),
        protocol,
        endpoint: custom.endpoint.trim().to_string(),
        key,
        headers: Vec::new(),
    })
}

fn chat_url(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with("/chat/completions") {
        base.to_string()
    } else {
        format!("{base}/chat/completions")
    }
}

fn anthropic_url(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with("/messages") {
        base.to_string()
    } else if base.ends_with("/v1") {
        format!("{base}/messages")
    } else {
        format!("{base}/v1/messages")
    }
}

// ─── prompts ─────────────────────────────────────────────────────────────

fn clip_input(input: &str) -> String {
    let trimmed = input.trim();
    let mut out = String::new();
    for (count, ch) in trimmed.chars().enumerate() {
        if count >= INPUT_CHAR_CAP {
            break;
        }
        out.push(ch);
    }
    out
}

fn system_prompt(kind: CheapTextKind, title_style: &str) -> String {
    match kind {
        CheapTextKind::Title => {
            let mut prompt = String::from(
                "你是会话标题助手。根据用户的第一条消息，生成一个语义化会话标题。\n\
                 要求：\n\
                 - 只输出标题本身，不要引号、不要句号、不要“标题：”之类的前缀；\n\
                 - 只写一行，最多 24 个汉字或 60 个字符；\n\
                 - 语言与用户消息的主要语言一致；\n\
                 - 抓住主题（做什么、在哪个项目、想达成什么），不要复述整句。",
            );
            if !title_style.trim().is_empty() {
                prompt.push_str("\n用户偏好：");
                prompt.push_str(title_style.trim());
            }
            prompt
        }
        CheapTextKind::Branch => String::from(
            "你是 Git 分支名助手。根据改动描述生成一个分支名。\n\
             要求：\n\
             - 只输出分支名本身，一行，不要解释；\n\
             - 小写 ASCII，必要时用 `-` 连接，最多 48 个字符；\n\
             - 用 `feat/`、`fix/`、`chore/`、`docs/`、`refactor/`、`test/` 之一做前缀。",
        ),
        CheapTextKind::Commit => String::from(
            "你是 Git 提交信息助手。根据改动内容写一条提交摘要。\n\
             要求：\n\
             - 只输出摘要本身，一行，不要代码围栏、不要引号、不要句号；\n\
             - 使用祈使句，最多 72 个字符；\n\
             - 语言与输入的主要语言一致。",
        ),
    }
}

fn user_prompt(kind: CheapTextKind, input: &str) -> String {
    let input = clip_input(input);
    match kind {
        CheapTextKind::Title => format!("用户的第一条消息：\n{input}"),
        CheapTextKind::Branch => format!("这次改动：\n{input}"),
        CheapTextKind::Commit => format!("这次改动：\n{input}"),
    }
}

// ─── output cleanup ──────────────────────────────────────────────────────

fn clean_line(raw: &str) -> String {
    let line = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let mut text = line
        .trim_matches(|c: char| matches!(c, '`' | '"' | '\'' | '“' | '”' | '‘' | '’'))
        .trim();
    for prefix in [
        "标题：",
        "标题:",
        "会话标题：",
        "会话标题:",
        "title:",
        "Title:",
        "分支名：",
        "分支名:",
        "branch:",
        "Branch:",
        "提交信息：",
        "提交信息:",
        "commit:",
        "Commit:",
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.trim();
            break;
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn cap_chars(text: &str, cap: usize) -> String {
    text.chars().take(cap).collect()
}

fn trim_trailing_punctuation(text: &str) -> &str {
    text.trim_end_matches(|c: char| {
        matches!(
            c,
            '。' | '.' | '!' | '！' | '?' | '？' | ';' | '；' | ',' | '，' | ':' | '：'
        )
    })
}

fn sanitize_title(raw: &str) -> Option<String> {
    let cleaned = clean_line(raw);
    let text = trim_trailing_punctuation(&cleaned);
    let text = cap_chars(text, TITLE_CHAR_CAP);
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn sanitize_commit(raw: &str) -> Option<String> {
    let cleaned = clean_line(raw);
    let text = trim_trailing_punctuation(&cleaned);
    let text = cap_chars(text, COMMIT_CHAR_CAP);
    let text = text.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

fn sanitize_branch(raw: &str) -> Option<String> {
    let line = clean_line(raw).to_ascii_lowercase();
    let mut out = String::new();
    let mut last_separator = false;
    for ch in line.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '/' | '-' | '_' | '.') {
            out.push(ch);
            last_separator = matches!(ch, '/' | '-' | '_' | '.');
        } else if (ch.is_whitespace() || ch == ':') && !last_separator {
            out.push('-');
            last_separator = true;
        }
    }
    let out = out.trim_matches(|c: char| matches!(c, '-' | '/' | '.' | '_'));
    if out.is_empty() {
        return None;
    }
    let out = cap_chars(out, BRANCH_CHAR_CAP);
    let out = out.trim_end_matches(|c: char| matches!(c, '-' | '/' | '.'));
    if out.is_empty() || !out.chars().next()?.is_ascii_alphanumeric() {
        return None;
    }
    Some(out.to_string())
}

fn sanitize(kind: CheapTextKind, raw: &str) -> Option<String> {
    match kind {
        CheapTextKind::Title => sanitize_title(raw),
        CheapTextKind::Branch => sanitize_branch(raw),
        CheapTextKind::Commit => sanitize_commit(raw),
    }
}

// ─── outbound call ───────────────────────────────────────────────────────

#[derive(Debug)]
struct GenFailure {
    stage: &'static str,
    status: Option<u16>,
    reason: String,
}

impl GenFailure {
    fn new(stage: &'static str, status: Option<u16>, reason: impl Into<String>) -> Self {
        Self {
            stage,
            status,
            reason: reason.into(),
        }
    }
}

struct GenOutput {
    text: String,
    tokens_in: Option<u64>,
    tokens_out: Option<u64>,
}

fn build_body(
    protocol: &Protocol,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
) -> Value {
    match protocol {
        Protocol::Chat => json!({
            "model": model,
            "messages": [
                { "role": "system", "content": system },
                { "role": "user", "content": user }
            ],
            "max_tokens": max_tokens,
            "stream": false
        }),
        Protocol::Anthropic => json!({
            "model": model,
            "max_tokens": max_tokens,
            "system": system,
            "messages": [ { "role": "user", "content": user } ]
        }),
    }
}

fn url_for(protocol: &Protocol, endpoint: &str) -> String {
    match protocol {
        Protocol::Chat => chat_url(endpoint),
        Protocol::Anthropic => anthropic_url(endpoint),
    }
}

fn call_model(
    source: &ResolvedSource,
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
    timeout_ms: u64,
) -> Result<GenOutput, GenFailure> {
    let url = url_for(&source.protocol, &source.endpoint);
    let body = build_body(&source.protocol, model, system, user, max_tokens);
    let body =
        serde_json::to_string(&body).map_err(|e| GenFailure::new("encode", None, e.to_string()))?;

    let agent = crate::http_client::agent_with_timeout(Duration::from_millis(timeout_ms), true)
        .map_err(|e| GenFailure::new("client", None, e))?;

    let mut request = agent
        .post(&url)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json")
        .timeout(Duration::from_millis(timeout_ms));
    match source.protocol {
        Protocol::Chat => {
            request = request.set("Authorization", &format!("Bearer {}", source.key));
        }
        Protocol::Anthropic => {
            request = request
                .set("x-api-key", &source.key)
                .set("anthropic-version", "2023-06-01");
        }
    }
    for (name, value) in &source.headers {
        if name.trim().is_empty() || value.trim().is_empty() {
            continue;
        }
        request = request.set(name, value);
    }

    let response = match request.send_string(&body) {
        Ok(response) => response,
        Err(ureq::Error::Status(status, _)) => {
            return Err(GenFailure::new("status", Some(status), "vendor status"));
        }
        Err(ureq::Error::Transport(_)) => {
            // Never log vendor text here: the category is enough to triage.
            return Err(GenFailure::new("network", None, "transport"));
        }
    };

    let status = response.status();
    if !(200..300).contains(&status) {
        return Err(GenFailure::new("status", Some(status), "vendor status"));
    }
    let mut reader = response.into_reader().take(RESPONSE_BYTE_CAP + 1);
    let mut buf = Vec::new();
    reader
        .read_to_end(&mut buf)
        .map_err(|e| GenFailure::new("read", Some(status), e.to_string()))?;
    if buf.len() as u64 > RESPONSE_BYTE_CAP {
        return Err(GenFailure::new("too_large", Some(status), "response cap"));
    }
    let text = String::from_utf8_lossy(&buf);
    let value: Value = serde_json::from_str(&text)
        .map_err(|_| GenFailure::new("parse", Some(status), "invalid json"))?;
    let parsed = parse_response(&source.protocol, &value)
        .ok_or_else(|| GenFailure::new("empty", Some(status), "no content"))?;
    Ok(parsed)
}

fn parse_response(protocol: &Protocol, value: &Value) -> Option<GenOutput> {
    match protocol {
        Protocol::Chat => {
            let content = value.pointer("/choices/0/message/content")?;
            let text = content
                .as_str()
                .map(str::to_string)
                .or_else(|| {
                    content.as_array().map(|parts| {
                        parts
                            .iter()
                            .filter_map(|part| part.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("")
                    })
                })
                .unwrap_or_default();
            Some(GenOutput {
                text,
                tokens_in: u64_of(value, &["/usage/prompt_tokens", "/usage/input_tokens"]),
                tokens_out: u64_of(value, &["/usage/completion_tokens", "/usage/output_tokens"]),
            })
        }
        Protocol::Anthropic => {
            let text = value
                .get("content")
                .and_then(Value::as_array)
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            Some(GenOutput {
                text,
                tokens_in: u64_of(value, &["/usage/input_tokens"]),
                tokens_out: u64_of(value, &["/usage/output_tokens"]),
            })
        }
    }
}

fn u64_of(value: &Value, pointers: &[&str]) -> Option<u64> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(*pointer).and_then(Value::as_u64))
}

// ─── usage label (separate from agent quota totals) ─────────────────────

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CheapTextUsageRow {
    ts: u64,
    source: &'static str,
    kind: &'static str,
    provider: String,
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens_in: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens_out: Option<u64>,
    millis: u64,
    status: u16,
}

static USAGE_LOCK: Mutex<()> = Mutex::new(());

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn append_usage(root: &Path, row: &CheapTextUsageRow) {
    let path = usage_path(root);
    let Some(dir) = path.parent() else {
        return;
    };
    if fs::create_dir_all(dir).is_err() {
        return;
    }
    let Ok(_guard) = USAGE_LOCK.lock() else {
        return;
    };
    if fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > USAGE_ROTATE_BYTES {
        let rotated = dir.join(USAGE_ROTATED);
        let _ = fs::remove_file(&rotated);
        let _ = fs::rename(&path, &rotated);
    }
    let Ok(line) = serde_json::to_string(row) else {
        return;
    };
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{line}");
    }
}

// ─── public service API ──────────────────────────────────────────────────

/// Generate auxiliary text synchronously. `style` overrides the configured
/// title style for `Title` requests only.
///
/// Returns `None` on any missing prerequisite or failure; failures are logged
/// without prompt/response bodies.
pub fn generate(kind: CheapTextKind, input: &str, style: Option<&str>) -> Option<String> {
    let root = match root() {
        Ok(root) => root,
        Err(error) => {
            log_failure(kind, "root", None, None, None, &error);
            return None;
        }
    };
    let config = read_config_at(&root);
    if !config.enabled || !config.configured() {
        // Inert by default; the caller keeps the existing first-message title.
        return None;
    }
    if clip_input(input).is_empty() {
        return None;
    }
    let Some(source) = resolve_source(&root, &config) else {
        log_failure(kind, "source", None, None, None, "no usable source or key");
        return None;
    };

    let title_style = if kind == CheapTextKind::Title {
        style.unwrap_or(&config.title_style)
    } else {
        ""
    };
    let system = system_prompt(kind, title_style);
    let user = user_prompt(kind, input);
    let started = Instant::now();
    let outcome = call_model(
        &source,
        &config.model,
        &system,
        &user,
        config.max_tokens,
        config.timeout_ms,
    );
    let millis = started.elapsed().as_millis() as u64;
    match outcome {
        Ok(output) => {
            let Some(text) = sanitize(kind, &output.text) else {
                log_failure(
                    kind,
                    "empty",
                    Some(200),
                    Some(source.id.as_str()),
                    Some(config.model.as_str()),
                    "empty after cleanup",
                );
                return None;
            };
            append_usage(
                &root,
                &CheapTextUsageRow {
                    ts: now_millis(),
                    source: USAGE_SOURCE,
                    kind: kind.key(),
                    provider: source.id.clone(),
                    model: config.model.clone(),
                    tokens_in: output.tokens_in,
                    tokens_out: output.tokens_out,
                    millis,
                    status: 200,
                },
            );
            crate::debug_log::append(
                USAGE_SOURCE,
                "info",
                "-",
                "cheap text generated",
                Some(&format!(
                    "kind={} provider={} model={} in={} out={} millis={}",
                    kind.key(),
                    source.id,
                    config.model,
                    output.tokens_in.unwrap_or(0),
                    output.tokens_out.unwrap_or(0),
                    millis
                )),
            );
            Some(text)
        }
        Err(failure) => {
            log_failure(
                kind,
                failure.stage,
                failure.status,
                Some(source.id.as_str()),
                Some(config.model.as_str()),
                &failure.reason,
            );
            None
        }
    }
}

fn log_failure(
    kind: CheapTextKind,
    stage: &str,
    status: Option<u16>,
    provider: Option<&str>,
    model: Option<&str>,
    reason: &str,
) {
    crate::debug_log::append(
        USAGE_SOURCE,
        "warn",
        "-",
        "cheap text generation skipped",
        Some(&format!(
            "kind={} stage={} status={} provider={} model={} reason={}",
            kind.key(),
            stage,
            status
                .map(|code| code.to_string())
                .unwrap_or_else(|| "-".to_string()),
            provider.unwrap_or("-"),
            model.unwrap_or("-"),
            reason
        )),
    );
}

/// Fire-and-forget semantic session title. Never blocks sending; emits
/// [`CHEAP_TITLE_EVENT`] only when a title was produced. The caller keeps the
/// existing first-message title on `None`.
pub fn spawn_session_title(
    app: AppHandle,
    session_id: String,
    text: String,
    style: Option<String>,
) {
    if session_id.trim().is_empty() || text.trim().is_empty() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("cheap-title".into())
        .spawn(move || {
            if let Some(title) = generate(CheapTextKind::Title, &text, style.as_deref()) {
                let _ = app.emit(
                    CHEAP_TITLE_EVENT,
                    json!({ "sessionId": session_id, "title": title }),
                );
            }
        });
}

fn settings_view(root: &Path, config: &CheapTextConfig) -> CheapTextSettingsView {
    CheapTextSettingsView {
        enabled: config.enabled,
        source: config.source.clone(),
        model: config.model.clone(),
        title_style: config.title_style.clone(),
        max_tokens: config.max_tokens,
        timeout_ms: config.timeout_ms,
        configured: config.configured(),
        credential_ready: config.configured() && resolve_source(root, config).is_some(),
    }
}

fn catalog(root: &Path) -> Vec<CheapTextSourceView> {
    let mut out: Vec<CheapTextSourceView> = Vec::new();

    // Catalogue rows come from the engine's offline cache; this must not start
    // or wake the engine.
    let catalog_rows = crate::engine::cached_catalog_entries();
    let mut rows = read_providers(&providers_path(root));
    if rows.is_empty() {
        if let Some(legacy) = legacy_providers_path() {
            rows = read_providers(&legacy);
        }
    }
    for row in rows {
        if row.id.trim().is_empty() || out.iter().any(|source| source.id == row.id) {
            continue;
        }
        let protocol = match protocol_of(&row.chat, &row.anthropic) {
            Some((protocol, _)) => protocol.key(),
            None => "none",
        };
        let mut models: Vec<String> = row
            .models
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|model| model.trim().to_string())
            .filter(|model| !model.is_empty())
            .collect();
        if models.is_empty() {
            let prefix = format!("{}/", row.id);
            models = catalog_rows
                .iter()
                .filter_map(|entry| entry.id.strip_prefix(prefix.as_str()))
                .map(str::to_string)
                .collect();
        }
        models.sort();
        models.dedup();
        let has_key =
            !row.key.trim().is_empty() || credential_key(root, &row.id, &row.preset).is_some();
        out.push(CheapTextSourceView {
            id: row.id.clone(),
            label: if row.name.trim().is_empty() {
                row.id
            } else {
                row.name
            },
            protocol: protocol.to_string(),
            models,
            has_key,
        });
    }

    // Manual custom sources that only live in the encrypted store.
    if let Ok(store) = crate::credentials::load_store_at(&credentials_path(root)) {
        for entry in &store.entries {
            let Some(custom) = entry.custom_provider.as_ref() else {
                continue;
            };
            if out.iter().any(|source| source.id == entry.id) {
                continue;
            }
            let protocol = match custom.protocol.as_str() {
                "chat" => "chat",
                "anthropic" => "anthropic",
                _ => "none",
            };
            let mut models: Vec<String> = custom
                .models
                .iter()
                .map(|model| model.trim().to_string())
                .filter(|model| !model.is_empty())
                .collect();
            models.sort();
            models.dedup();
            out.push(CheapTextSourceView {
                id: entry.id.clone(),
                label: if entry.label.trim().is_empty() {
                    entry.id.clone()
                } else {
                    entry.label.clone()
                },
                protocol: protocol.to_string(),
                models,
                has_key: entry
                    .secret
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|value| !value.is_empty()),
            });
        }
    }

    out.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    out
}

// ─── Tauri commands ──────────────────────────────────────────────────────

#[tauri::command]
pub async fn cheap_text_settings_get() -> Result<CheapTextSettingsView, String> {
    tauri::async_runtime::spawn_blocking(|| -> Result<CheapTextSettingsView, String> {
        let root = root()?;
        let config = read_config_at(&root);
        Ok(settings_view(&root, &config))
    })
    .await
    .map_err(|e| format!("cheap text settings task failed: {e}"))?
}

#[tauri::command]
pub async fn cheap_text_settings_set(
    settings: CheapTextConfig,
) -> Result<CheapTextSettingsView, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<CheapTextSettingsView, String> {
        let root = root()?;
        let config = settings.normalized();
        write_config_at(&root, &config)?;
        Ok(settings_view(&root, &config))
    })
    .await
    .map_err(|e| format!("cheap text settings task failed: {e}"))?
}

#[tauri::command]
pub async fn cheap_text_catalog() -> Result<Vec<CheapTextSourceView>, String> {
    tauri::async_runtime::spawn_blocking(|| -> Result<Vec<CheapTextSourceView>, String> {
        let root = root()?;
        Ok(catalog(&root))
    })
    .await
    .map_err(|e| format!("cheap text catalog task failed: {e}"))?
}

/// On-demand branch name / commit message / title. Returns `None` instead of an
/// error so the caller can keep its own draft.
#[tauri::command]
pub async fn cheap_text_generate(
    kind: String,
    text: String,
    style: Option<String>,
) -> Option<String> {
    let parsed = CheapTextKind::parse(&kind)?;
    tauri::async_runtime::spawn_blocking(move || generate(parsed, &text, style.as_deref()))
        .await
        .ok()
        .flatten()
}

/// Non-blocking automatic title. Returns immediately; listen for
/// [`CHEAP_TITLE_EVENT`].
#[tauri::command]
pub fn cheap_text_generate_title(
    app: AppHandle,
    session_id: String,
    text: String,
    style: Option<String>,
) {
    spawn_session_title(app, session_id, text, style);
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marionette-cheap-text-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        set_root_override_for_tests(Some(dir.clone()));
        dir
    }

    fn write_providers(root: &Path, providers: &str) {
        let path = providers_path(root);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, providers).unwrap();
    }

    #[test]
    fn config_defaults_and_normalization() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("config");
        let config = read_config_at(&root);
        assert!(config.enabled);
        assert!(!config.configured());
        assert_eq!(config.max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(config.timeout_ms, DEFAULT_TIMEOUT_MS);

        let normalized = CheapTextConfig {
            enabled: false,
            source: "  deepseek ".into(),
            model: " deepseek-chat ".into(),
            title_style: "  简短 ".into(),
            max_tokens: 0,
            timeout_ms: 1,
        }
        .normalized();
        assert_eq!(normalized.source, "deepseek");
        assert_eq!(normalized.model, "deepseek-chat");
        assert_eq!(normalized.title_style, "简短");
        assert_eq!(normalized.max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(normalized.timeout_ms, MIN_TIMEOUT_MS);
        assert!(normalized.configured());
    }

    #[test]
    fn config_round_trips_through_disk() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("config-roundtrip");
        let config = CheapTextConfig {
            enabled: true,
            source: "deepseek".into(),
            model: "deepseek-chat".into(),
            title_style: "中文".into(),
            max_tokens: 64,
            timeout_ms: 5_000,
        };
        write_config_at(&root, &config).unwrap();
        assert_eq!(read_config_at(&root), config);
    }

    #[test]
    fn title_cleanup_strips_wrappers_and_punctuation() {
        assert_eq!(
            sanitize_title(
                "\u{6807}\u{9898}\u{FF1A} \u{767B}\u{5F55}\u{95EE}\u{9898}\u{6392}\u{67E5}\u{3002}"
            ),
            Some("登录问题排查".to_string())
        );
        assert_eq!(
            sanitize_title("`fix login flow`"),
            Some("fix login flow".to_string())
        );
        assert_eq!(
            sanitize_title("Title: Refactor storage layer."),
            Some("Refactor storage layer".to_string())
        );
        assert_eq!(sanitize_title("第一行\n第二行"), Some("第一行".to_string()));
        assert_eq!(sanitize_title("   \n  "), None);
    }

    #[test]
    fn commit_and_branch_cleanup() {
        assert_eq!(
            sanitize_commit("Add retry to the gateway."),
            Some("Add retry to the gateway".to_string())
        );
        assert_eq!(
            sanitize_branch("feat: Add user login!"),
            Some("feat-add-user-login".to_string())
        );
        assert_eq!(
            sanitize_branch("`fix/storage-race`"),
            Some("fix/storage-race".to_string())
        );
        assert_eq!(sanitize_branch("修复登录问题"), None);
    }

    #[test]
    fn output_is_capped_by_chars_not_bytes() {
        let long = "题".repeat(TITLE_CHAR_CAP + 40);
        let title = sanitize_title(&long).unwrap();
        assert_eq!(title.chars().count(), TITLE_CHAR_CAP);
        let input = "字".repeat(INPUT_CHAR_CAP + 100);
        assert_eq!(clip_input(&input).chars().count(), INPUT_CHAR_CAP);
    }

    #[test]
    fn prompts_carry_style_and_bounded_input() {
        let system = system_prompt(CheapTextKind::Title, "尽量短");
        assert!(system.contains("尽量短"));
        let user = user_prompt(CheapTextKind::Title, &"a".repeat(INPUT_CHAR_CAP + 10));
        assert!(user.chars().count() < INPUT_CHAR_CAP + 40);
        assert!(user_prompt(CheapTextKind::Branch, "x").contains("这次改动"));
    }

    #[test]
    fn provider_row_resolves_from_existing_source_store() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("provider");
        write_providers(
            &root,
            r#"{
                "providers": [
                    {
                        "id": "deepseek",
                        "name": "DeepSeek",
                        "key": "sk-row",
                        "chat": "https://api.deepseek.com/v1",
                        "anthropic": "https://api.deepseek.com/anthropic"
                    }
                ]
            }"#,
        );
        let config = CheapTextConfig {
            source: "DeepSeek".into(),
            model: "deepseek-chat".into(),
            ..Default::default()
        }
        .normalized();
        let resolved = resolve_source(&root, &config).expect("resolves by name");
        assert_eq!(resolved.id, "deepseek");
        assert_eq!(resolved.key, "sk-row");
        assert_eq!(resolved.protocol, Protocol::Chat);
        assert_eq!(resolved.endpoint, "https://api.deepseek.com/v1");
    }

    #[test]
    fn provider_without_key_falls_back_to_encrypted_store() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("provider-cred");
        write_providers(
            &root,
            r#"{ "providers": [ { "id": "glm", "name": "GLM", "chat": "https://glm.example/v1" } ] }"#,
        );
        crate::credentials::add_key_plan_at(&credentials_path(&root), "glm", "GLM", "sk-encrypted")
            .unwrap();
        let config = CheapTextConfig {
            source: "glm".into(),
            model: "glm-4-flash".into(),
            ..Default::default()
        }
        .normalized();
        let resolved = resolve_source(&root, &config).expect("credential key is reused");
        assert_eq!(resolved.key, "sk-encrypted");
    }

    #[test]
    fn manual_custom_source_resolves_from_encrypted_entry_alone() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("custom-only");
        let id = crate::credentials::add_custom_provider_at(
            &credentials_path(&root),
            "Relay",
            "sk-relay",
            crate::credentials::CustomProviderConfig {
                endpoint: "https://relay.example/v1".into(),
                protocol: "chat".into(),
                models: vec!["cheap-1".into()],
            },
        )
        .unwrap();
        let config = CheapTextConfig {
            source: id.clone(),
            model: "cheap-1".into(),
            ..Default::default()
        }
        .normalized();
        let resolved = resolve_source(&root, &config).expect("no providers.json needed");
        assert_eq!(resolved.id, id);
        assert_eq!(resolved.endpoint, "https://relay.example/v1");
        assert_eq!(resolved.key, "sk-relay");
    }

    #[test]
    fn missing_key_generates_nothing() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("no-key");
        write_providers(
            &root,
            r#"{ "providers": [ { "id": "empty", "chat": "https://empty.example/v1" } ] }"#,
        );
        write_config_at(
            &root,
            &CheapTextConfig {
                source: "empty".into(),
                model: "m".into(),
                ..Default::default()
            }
            .normalized(),
        )
        .unwrap();
        assert_eq!(generate(CheapTextKind::Title, "hello", None), None);
    }

    #[test]
    fn disabled_or_unconfigured_generates_nothing() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("inert");
        write_config_at(
            &root,
            &CheapTextConfig {
                source: "deepseek".into(),
                model: "deepseek-chat".into(),
                enabled: false,
                ..Default::default()
            }
            .normalized(),
        )
        .unwrap();
        assert_eq!(generate(CheapTextKind::Title, "hello", None), None);

        write_config_at(&root, &CheapTextConfig::default()).unwrap();
        assert_eq!(generate(CheapTextKind::Commit, "hello", None), None);
    }

    #[test]
    fn usage_rows_are_labelled_and_carry_no_bodies() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("usage");
        append_usage(
            &root,
            &CheapTextUsageRow {
                ts: 1_700_000_000_000,
                source: USAGE_SOURCE,
                kind: "title",
                provider: "deepseek".into(),
                model: "deepseek-chat".into(),
                tokens_in: Some(120),
                tokens_out: Some(12),
                millis: 800,
                status: 200,
            },
        );
        let text = fs::read_to_string(usage_path(&root)).unwrap();
        let row: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert_eq!(row["source"], "cheap-text");
        assert_eq!(row["kind"], "title");
        assert_eq!(row["tokensIn"], 120);
        // No prompt/response text field exists on the row at all.
        assert!(row.get("text").is_none());
        assert!(row.get("prompt").is_none());
        assert!(row.get("response").is_none());
    }

    #[test]
    fn catalog_merges_pinned_models_and_encrypted_sources() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let root = sandbox("catalog");
        write_providers(
            &root,
            r#"{
                "providers": [
                    {
                        "id": "pinned",
                        "name": "Pinned",
                        "key": "sk",
                        "chat": "https://p.example/v1",
                        "models": ["b-model", "a-model", "a-model"]
                    },
                    { "id": "nokey", "name": "No Key", "anthropic": "https://n.example" }
                ]
            }"#,
        );
        let id = crate::credentials::add_custom_provider_at(
            &credentials_path(&root),
            "Manual",
            "sk-manual",
            crate::credentials::CustomProviderConfig {
                endpoint: "https://m.example/v1".into(),
                protocol: "chat".into(),
                models: vec!["m1".into()],
            },
        )
        .unwrap();
        let rows = catalog(&root);
        let pinned = rows.iter().find(|row| row.id == "pinned").unwrap();
        assert_eq!(
            pinned.models,
            vec!["a-model".to_string(), "b-model".to_string()]
        );
        assert!(pinned.has_key);
        let nokey = rows.iter().find(|row| row.id == "nokey").unwrap();
        assert_eq!(nokey.protocol, "anthropic");
        assert!(!nokey.has_key);
        let manual = rows.iter().find(|row| row.id == id).unwrap();
        assert_eq!(manual.models, vec!["m1".to_string()]);
        assert!(manual.has_key);
    }

    #[test]
    fn absolute_urls_are_built_without_double_v1() {
        assert_eq!(
            chat_url("https://api.deepseek.com/v1/"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        assert_eq!(
            chat_url("https://api.deepseek.com"),
            "https://api.deepseek.com/chat/completions"
        );
        assert_eq!(
            anthropic_url("https://api.deepseek.com/anthropic"),
            "https://api.deepseek.com/anthropic/v1/messages"
        );
        assert_eq!(
            anthropic_url("https://relay.example/v1"),
            "https://relay.example/v1/messages"
        );
    }

    #[test]
    fn response_parsing_extracts_text_and_tokens() {
        let chat: Value = serde_json::from_str(
            r#"{"choices":[{"message":{"content":"  Hello  "}}],"usage":{"prompt_tokens":7,"completion_tokens":3}}"#,
        )
        .unwrap();
        let parsed = parse_response(&Protocol::Chat, &chat).unwrap();
        assert_eq!(parsed.text, "  Hello  ");
        assert_eq!(parsed.tokens_in, Some(7));
        assert_eq!(parsed.tokens_out, Some(3));

        let parts: Value = serde_json::from_str(
            r#"{"choices":[{"message":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}}]}"#,
        )
        .unwrap();
        assert_eq!(parse_response(&Protocol::Chat, &parts).unwrap().text, "ab");

        let anthropic: Value = serde_json::from_str(
            r#"{"content":[{"type":"text","text":" Title "}],"usage":{"input_tokens":5,"output_tokens":2}}"#,
        )
        .unwrap();
        let parsed = parse_response(&Protocol::Anthropic, &anthropic).unwrap();
        assert_eq!(parsed.text, " Title ");
        assert_eq!(parsed.tokens_in, Some(5));
        assert_eq!(parsed.tokens_out, Some(2));
    }
}
