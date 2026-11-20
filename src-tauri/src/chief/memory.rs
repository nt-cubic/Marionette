//! 员工分层记忆。
//!
//! 布局：`~/.marionette/chief/memory/<staffId>/`
//!
//! ```text
//! pinned.md             用户确认的长期事实（普通整理不可覆盖）
//! state.md              近期合作上下文/当前关注点
//! history.jsonl         每次写入、删除和失败原因的审计流水
//! journal/YYYY-MM-DD.md 合作日或任务复盘
//! index.json            文件元数据与 tombstone 索引
//! proposals.jsonl       模型候选（可选，永不自动晋升 pinned）
//! ```
//!
//! 本模块不读取 transcript/task ledger，也不把模型输出当事实。正文和
//! index 全部用原子替换；index 损坏时从目录扫描重建，删除记录则从
//! `history.jsonl` 的 forget 行恢复，避免自动整理把已删记忆复活。

use super::types::{now_ms, short_id};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const MEMORY_VERSION: u32 = 1;
pub const MAX_PINNED_BYTES: u64 = 64 * 1024;
pub const MAX_STATE_BYTES: u64 = 32 * 1024;
pub const MAX_JOURNAL_BYTES: u64 = 128 * 1024;
pub const MAX_HISTORY_BYTES: u64 = 1024 * 1024;
pub const MAX_HISTORY_LINES: usize = 2_000;
pub const MAX_INDEX_BYTES: u64 = 256 * 1024;
pub const MAX_PROPOSALS_BYTES: u64 = 128 * 1024;
pub const MAX_SOURCE_REFS: usize = 32;
#[allow(dead_code)]
pub const RECENT_JOURNAL_DAYS: usize = 7;

static MEMORY_LOCK: Mutex<()> = Mutex::new(());
static ROOT_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

#[cfg(test)]
pub fn set_root_override_for_tests(path: Option<PathBuf>) {
    if let Ok(mut slot) = ROOT_OVERRIDE.lock() {
        *slot = path;
    }
}

fn chief_root() -> Result<PathBuf, String> {
    if let Ok(slot) = ROOT_OVERRIDE.lock() {
        if let Some(path) = slot.as_ref() {
            return Ok(path.clone());
        }
    }
    crate::app_paths::global_dir().map(|dir| dir.join("chief"))
}

pub fn memory_root() -> Result<PathBuf, String> {
    Ok(chief_root()?.join("memory"))
}

pub fn staff_dir(staff_id: &str) -> Result<PathBuf, String> {
    validate_staff_id(staff_id)?;
    Ok(memory_root()?.join(staff_id))
}

fn warn(message: &str, detail: Option<&str>) {
    crate::debug_log::append("chief-memory", "warn", "", message, detail);
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct MemorySourceRef {
    pub kind: String,
    pub session_id: Option<String>,
    pub task_id: Option<String>,
    pub event_seq: Option<u64>,
    pub transcript_from: Option<u64>,
    pub transcript_to: Option<u64>,
}

impl MemorySourceRef {
    #[allow(dead_code)]
    pub fn session(session_id: impl Into<String>) -> Self {
        Self {
            kind: "session".to_string(),
            session_id: Some(session_id.into()),
            ..Default::default()
        }
    }

    pub fn task(task_id: impl Into<String>) -> Self {
        Self {
            kind: "task".to_string(),
            task_id: Some(task_id.into()),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct MemoryFileMeta {
    pub path: String,
    pub kind: String,
    pub bytes: u64,
    pub updated_at: u64,
    pub source_refs: Vec<MemorySourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct MemoryTombstone {
    pub target: String,
    pub deleted_at: u64,
    pub reason: String,
    pub source_refs: Vec<MemorySourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct MemoryIndex {
    pub version: u32,
    pub staff_id: String,
    pub updated_at: u64,
    pub files: Vec<MemoryFileMeta>,
    pub tombstones: Vec<MemoryTombstone>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct MemoryHistoryEntry {
    pub record_id: String,
    pub staff_id: String,
    /// write | forget | proposal | repair
    pub action: String,
    pub target: String,
    pub at: u64,
    pub operator: String,
    pub summary: String,
    pub source_refs: Vec<MemorySourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct MemoryProposal {
    pub proposal_id: String,
    pub staff_id: String,
    pub kind: String,
    pub text: String,
    pub source_refs: Vec<MemorySourceRef>,
    pub created_at: u64,
    /// pending | accepted | rejected
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct JournalEntry {
    pub date: String,
    pub body: String,
    pub meta: MemoryFileMeta,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct MemorySnapshot {
    pub staff_id: String,
    pub pinned: String,
    pub state: String,
    pub recent_journals: Vec<JournalEntry>,
    pub proposals: Vec<MemoryProposal>,
    pub index: MemoryIndex,
}

#[derive(Debug, Clone)]
pub struct MemoryPaths {
    pub root: PathBuf,
    #[allow(dead_code)]
    pub pinned: PathBuf,
    #[allow(dead_code)]
    pub state: PathBuf,
    pub history: PathBuf,
    pub journal: PathBuf,
    pub index: PathBuf,
    pub proposals: PathBuf,
}

fn validate_staff_id(staff_id: &str) -> Result<(), String> {
    if staff_id.is_empty()
        || staff_id.len() > 160
        || !staff_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        return Err(format!("非法 staffId: {staff_id:?}"));
    }
    Ok(())
}

fn normalize_source_ref(mut source: MemorySourceRef) -> Option<MemorySourceRef> {
    source.kind = source.kind.trim().to_string();
    source.session_id = source
        .session_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    source.task_id = source
        .task_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if source.kind.is_empty()
        && source.session_id.is_none()
        && source.task_id.is_none()
        && source.event_seq.is_none()
    {
        return None;
    }
    if source.kind.len() > 128
        || source
            .session_id
            .as_ref()
            .is_some_and(|value| value.len() > 512)
        || source
            .task_id
            .as_ref()
            .is_some_and(|value| value.len() > 512)
    {
        return None;
    }
    Some(source)
}

fn normalize_source_refs(source_refs: Vec<MemorySourceRef>) -> Vec<MemorySourceRef> {
    source_refs
        .into_iter()
        .filter_map(normalize_source_ref)
        .take(MAX_SOURCE_REFS)
        .collect()
}

fn paths(staff_id: &str) -> Result<MemoryPaths, String> {
    let root = staff_dir(staff_id)?;
    Ok(MemoryPaths {
        pinned: root.join("pinned.md"),
        state: root.join("state.md"),
        history: root.join("history.jsonl"),
        journal: root.join("journal"),
        index: root.join("index.json"),
        proposals: root.join("proposals.jsonl"),
        root,
    })
}

fn relative_target(path: &str) -> Result<(), String> {
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!("非法 memory target: {path}"));
    }
    Ok(())
}

fn read_bounded(path: &Path, max: u64) -> Result<Option<String>, String> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("读取 {} 失败: {error}", path.display())),
    };
    if metadata.len() > max {
        return Err(format!(
            "{} 超过大小上限（{} > {} bytes）",
            path.display(),
            metadata.len(),
            max
        ));
    }
    fs::read_to_string(path)
        .map(Some)
        .map_err(|error| format!("读取 {} 失败: {error}", path.display()))
}

fn read_text(path: &Path, max: u64) -> Result<String, String> {
    Ok(read_bounded(path, max)?.unwrap_or_default())
}

fn read_history_unlocked(staff_id: &str) -> Vec<MemoryHistoryEntry> {
    let path = match paths(staff_id) {
        Ok(paths) => paths.history,
        Err(_) => return Vec::new(),
    };
    let raw = match read_bounded(&path, MAX_HISTORY_BYTES) {
        Ok(Some(raw)) => raw,
        Ok(None) => return Vec::new(),
        Err(error) => {
            warn("history.jsonl unreadable", Some(&error));
            return Vec::new();
        }
    };
    raw.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<MemoryHistoryEntry>(line).ok())
        .collect()
}

fn history_tombstones(history: &[MemoryHistoryEntry]) -> Vec<MemoryTombstone> {
    history
        .iter()
        .filter(|entry| entry.action == "forget")
        .map(|entry| MemoryTombstone {
            target: entry.target.clone(),
            deleted_at: entry.at,
            reason: entry.summary.clone(),
            source_refs: entry.source_refs.clone(),
        })
        .collect()
}

fn scan_file_meta(root: &Path, relative: &str) -> Option<MemoryFileMeta> {
    let path = root.join(relative);
    let metadata = fs::metadata(&path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let kind = if relative == "pinned.md" {
        "pinned"
    } else if relative == "state.md" {
        "state"
    } else if relative.starts_with("journal/") {
        "journal"
    } else if relative == "proposals.jsonl" {
        "proposal"
    } else {
        "other"
    };
    Some(MemoryFileMeta {
        path: relative.to_string(),
        kind: kind.to_string(),
        bytes: metadata.len(),
        updated_at: metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0),
        source_refs: Vec::new(),
    })
}

fn scan_files_unlocked(root: &Path) -> Vec<MemoryFileMeta> {
    let mut files = Vec::new();
    for relative in ["pinned.md", "state.md", "proposals.jsonl"] {
        if let Some(meta) = scan_file_meta(root, relative) {
            files.push(meta);
        }
    }
    if let Ok(entries) = fs::read_dir(root.join("journal")) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if valid_journal_date(name.trim_end_matches(".md")) && name.ends_with(".md") {
                if let Some(meta) = scan_file_meta(root, &format!("journal/{name}")) {
                    files.push(meta);
                }
            }
        }
    }
    files.sort_by(|left, right| left.path.cmp(&right.path));
    files
}

fn merge_tombstones(index: &mut MemoryIndex, extra: Vec<MemoryTombstone>) {
    for tombstone in extra {
        if !index
            .tombstones
            .iter()
            .any(|existing| existing.target == tombstone.target)
        {
            index.tombstones.push(tombstone);
        }
    }
    index
        .tombstones
        .sort_by(|left, right| left.target.cmp(&right.target));
}

fn read_index_unlocked(staff_id: &str) -> MemoryIndex {
    let paths = match paths(staff_id) {
        Ok(paths) => paths,
        Err(_) => {
            return MemoryIndex {
                staff_id: staff_id.to_string(),
                ..Default::default()
            }
        }
    };
    let parsed = read_bounded(&paths.index, MAX_INDEX_BYTES)
        .ok()
        .flatten()
        .and_then(|raw| serde_json::from_str::<MemoryIndex>(&raw).ok());
    let mut index = parsed.unwrap_or_else(|| MemoryIndex {
        version: MEMORY_VERSION,
        staff_id: staff_id.to_string(),
        updated_at: 0,
        files: scan_files_unlocked(&paths.root),
        tombstones: Vec::new(),
    });
    index.version = MEMORY_VERSION;
    index.staff_id = staff_id.to_string();
    let history = read_history_unlocked(staff_id);
    merge_tombstones(&mut index, history_tombstones(&history));
    if index.files.is_empty() {
        index.files = scan_files_unlocked(&paths.root);
    }
    index
}

fn save_index_unlocked(staff_id: &str, index: &mut MemoryIndex) -> Result<(), String> {
    let paths = paths(staff_id)?;
    fs::create_dir_all(&paths.root).map_err(|error| format!("创建 memory 目录失败: {error}"))?;
    index.version = MEMORY_VERSION;
    index.staff_id = staff_id.to_string();
    index.updated_at = now_ms();
    let body = serde_json::to_string_pretty(index).map_err(|error| error.to_string())?;
    crate::storage::write_atomic(&paths.index, &body)
}

fn is_tombstoned(index: &MemoryIndex, target: &str) -> bool {
    index
        .tombstones
        .iter()
        .any(|tombstone| tombstone.target == target)
}

fn set_file_meta(index: &mut MemoryIndex, meta: MemoryFileMeta) {
    if let Some(existing) = index.files.iter_mut().find(|file| file.path == meta.path) {
        *existing = meta;
    } else {
        index.files.push(meta);
    }
    index
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
}

fn remove_file_meta(index: &mut MemoryIndex, target: &str) {
    index.files.retain(|file| file.path != target);
}

fn encode_history(entries: &[MemoryHistoryEntry]) -> Result<String, String> {
    let mut out = String::new();
    for entry in entries {
        out.push_str(&serde_json::to_string(entry).map_err(|error| error.to_string())?);
        out.push('\n');
    }
    Ok(out)
}

fn trim_history(mut entries: Vec<MemoryHistoryEntry>) -> Vec<MemoryHistoryEntry> {
    // Keep forget records even when the audit log is trimmed: they are the
    // source used to prevent a deleted target from being recreated.
    let mut forgotten: Vec<_> = entries
        .iter()
        .filter(|entry| entry.action == "forget")
        .cloned()
        .collect();
    if entries.len() > MAX_HISTORY_LINES {
        let recent = entries.split_off(entries.len() - MAX_HISTORY_LINES);
        entries = recent;
    }
    // A forget line is authoritative even if it fell outside the recent
    // window. Re-add missing tombstones before applying the byte budget.
    forgotten.retain(|entry| {
        !entries
            .iter()
            .any(|existing| existing.record_id == entry.record_id)
    });
    entries.extend(forgotten);
    loop {
        let size = entries
            .iter()
            .map(|entry| {
                serde_json::to_vec(entry)
                    .map(|bytes| bytes.len() + 1)
                    .unwrap_or(usize::MAX)
            })
            .sum::<usize>();
        if size as u64 <= MAX_HISTORY_BYTES {
            break;
        }
        let Some(index) = entries.iter().position(|entry| entry.action != "forget") else {
            // Tombstones are authoritative for forget semantics. If they
            // alone exceed the audit byte budget, keep them rather than
            // silently allowing a deleted target to be recreated later.
            break;
        };
        entries.remove(index);
    }
    entries.sort_by_key(|entry| entry.at);
    entries
}

fn append_history_unlocked(
    staff_id: &str,
    action: &str,
    target: &str,
    summary: &str,
    source_refs: Vec<MemorySourceRef>,
) -> Result<MemoryHistoryEntry, String> {
    validate_staff_id(staff_id)?;
    relative_target(target)?;
    let mut entries = read_history_unlocked(staff_id);
    let entry = MemoryHistoryEntry {
        record_id: short_id("memhist"),
        staff_id: staff_id.to_string(),
        action: action.to_string(),
        target: target.to_string(),
        at: now_ms(),
        operator: "host".to_string(),
        summary: summary.trim().chars().take(8_192).collect(),
        source_refs: normalize_source_refs(source_refs),
    };
    entries.push(entry.clone());
    entries = trim_history(entries);
    let body = encode_history(&entries)?;
    let path = paths(staff_id)?.history;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("创建 memory 目录失败: {error}"))?;
    }
    crate::storage::write_atomic(&path, &body)?;
    Ok(entry)
}

fn write_file_unlocked(
    staff_id: &str,
    target: &str,
    kind: &str,
    body: &str,
    max_bytes: u64,
    source_refs: Vec<MemorySourceRef>,
) -> Result<MemoryFileMeta, String> {
    validate_staff_id(staff_id)?;
    relative_target(target)?;
    if body.len() as u64 > max_bytes {
        return Err(format!("{target} 超过大小上限 {max_bytes} bytes"));
    }
    let paths = paths(staff_id)?;
    let mut index = read_index_unlocked(staff_id);
    if is_tombstoned(&index, target) {
        return Err(format!("memory target 已被忘记，不能静默重写: {target}"));
    }
    let path = paths.root.join(target);
    crate::storage::write_atomic(&path, body)?;
    let meta = MemoryFileMeta {
        path: target.to_string(),
        kind: kind.to_string(),
        bytes: body.len() as u64,
        updated_at: now_ms(),
        source_refs: normalize_source_refs(source_refs.clone()),
    };
    append_history_unlocked(staff_id, "write", target, "memory updated", source_refs)?;
    set_file_meta(&mut index, meta.clone());
    save_index_unlocked(staff_id, &mut index)?;
    Ok(meta)
}

fn read_file_unlocked(staff_id: &str, target: &str, max_bytes: u64) -> Result<String, String> {
    let index = read_index_unlocked(staff_id);
    if is_tombstoned(&index, target) {
        return Ok(String::new());
    }
    read_text(&paths(staff_id)?.root.join(target), max_bytes)
}

fn ensure_paths_and_files_unlocked(staff_id: &str) -> Result<MemoryPaths, String> {
    let paths = paths(staff_id)?;
    fs::create_dir_all(&paths.journal)
        .map_err(|error| format!("创建 journal 目录失败: {error}"))?;
    let mut index = read_index_unlocked(staff_id);
    for (target, max_bytes, kind) in [
        ("pinned.md", MAX_PINNED_BYTES, "pinned"),
        ("state.md", MAX_STATE_BYTES, "state"),
    ] {
        if !paths.root.join(target).exists() && !is_tombstoned(&index, target) {
            crate::storage::write_atomic(&paths.root.join(target), "")?;
            set_file_meta(
                &mut index,
                MemoryFileMeta {
                    path: target.to_string(),
                    kind: kind.to_string(),
                    bytes: 0,
                    updated_at: now_ms(),
                    source_refs: Vec::new(),
                },
            );
            let _ = max_bytes;
        }
    }
    if !paths.history.exists() {
        crate::storage::write_atomic(&paths.history, "")?;
    }
    save_index_unlocked(staff_id, &mut index)?;
    Ok(paths)
}

/// 创建目录与默认文件，但不会复活 tombstone 目标。
pub fn ensure_staff_memory(staff_id: &str) -> Result<MemoryPaths, String> {
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_paths_and_files_unlocked(staff_id)
}

#[cfg(test)]
pub fn read_index(staff_id: &str) -> MemoryIndex {
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    read_index_unlocked(staff_id)
}

#[cfg(test)]
pub fn read_pinned(staff_id: &str) -> Result<String, String> {
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    read_file_unlocked(staff_id, "pinned.md", MAX_PINNED_BYTES)
}

#[allow(dead_code)]
pub fn read_state(staff_id: &str) -> Result<String, String> {
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    read_file_unlocked(staff_id, "state.md", MAX_STATE_BYTES)
}

pub fn write_pinned(staff_id: &str, body: &str) -> Result<MemoryFileMeta, String> {
    write_pinned_with_source_refs(staff_id, body, Vec::new())
}

pub fn write_pinned_with_source_refs(
    staff_id: &str,
    body: &str,
    source_refs: Vec<MemorySourceRef>,
) -> Result<MemoryFileMeta, String> {
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_paths_and_files_unlocked(staff_id)?;
    write_file_unlocked(
        staff_id,
        "pinned.md",
        "pinned",
        body,
        MAX_PINNED_BYTES,
        source_refs,
    )
}

pub fn write_state(staff_id: &str, body: &str) -> Result<MemoryFileMeta, String> {
    write_state_with_source_refs(staff_id, body, Vec::new())
}

pub fn write_state_with_source_refs(
    staff_id: &str,
    body: &str,
    source_refs: Vec<MemorySourceRef>,
) -> Result<MemoryFileMeta, String> {
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_paths_and_files_unlocked(staff_id)?;
    write_file_unlocked(
        staff_id,
        "state.md",
        "state",
        body,
        MAX_STATE_BYTES,
        source_refs,
    )
}

pub fn journal_target(date: &str) -> Result<String, String> {
    if !valid_journal_date(date) {
        return Err(format!("非法 journal 日期: {date}"));
    }
    Ok(format!("journal/{date}.md"))
}

#[cfg(test)]
pub fn write_journal(staff_id: &str, date: &str, body: &str) -> Result<MemoryFileMeta, String> {
    write_journal_with_source_refs(staff_id, date, body, Vec::new())
}

pub fn write_journal_with_source_refs(
    staff_id: &str,
    date: &str,
    body: &str,
    source_refs: Vec<MemorySourceRef>,
) -> Result<MemoryFileMeta, String> {
    let target = journal_target(date)?;
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_paths_and_files_unlocked(staff_id)?;
    write_file_unlocked(
        staff_id,
        &target,
        "journal",
        body,
        MAX_JOURNAL_BYTES,
        source_refs,
    )
}

/// Append a Host-authored factual entry to today's journal without replacing
/// earlier entries. Memory tombstones and size limits still apply.
pub fn append_journal_with_source_refs(
    staff_id: &str,
    date: &str,
    entry: &str,
    source_refs: Vec<MemorySourceRef>,
) -> Result<MemoryFileMeta, String> {
    let existing = read_journal(staff_id, date).unwrap_or_default();
    let body = if existing.trim().is_empty() {
        entry.trim().to_string()
    } else {
        format!("{}\n\n{}", existing.trim_end(), entry.trim())
    };
    write_journal_with_source_refs(staff_id, date, &body, source_refs)
}

pub fn read_journal(staff_id: &str, date: &str) -> Result<String, String> {
    let target = journal_target(date)?;
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    read_file_unlocked(staff_id, &target, MAX_JOURNAL_BYTES)
}

pub fn list_journal_dates(staff_id: &str) -> Result<Vec<String>, String> {
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let paths = ensure_paths_and_files_unlocked(staff_id)?;
    let mut dates = Vec::new();
    if let Ok(entries) = fs::read_dir(&paths.journal) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(date) = name.strip_suffix(".md") {
                if valid_journal_date(date) {
                    dates.push(date.to_string());
                }
            }
        }
    }
    dates.sort();
    dates.reverse();
    Ok(dates)
}

pub fn recall(staff_id: &str, recent_days: usize) -> Result<MemorySnapshot, String> {
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_paths_and_files_unlocked(staff_id)?;
    let index = read_index_unlocked(staff_id);
    let pinned = read_file_unlocked(staff_id, "pinned.md", MAX_PINNED_BYTES)?;
    let state = read_file_unlocked(staff_id, "state.md", MAX_STATE_BYTES)?;
    let mut recent_journals = Vec::new();
    for date in list_journal_dates_unlocked(staff_id)?
        .into_iter()
        .take(recent_days.clamp(0, 60))
    {
        if is_tombstoned(&index, &journal_target(&date)?) {
            continue;
        }
        let body = read_file_unlocked(staff_id, &journal_target(&date)?, MAX_JOURNAL_BYTES)?;
        let meta = index
            .files
            .iter()
            .find(|file| file.path == journal_target(&date).unwrap_or_default())
            .cloned()
            .unwrap_or_default();
        recent_journals.push(JournalEntry { date, body, meta });
    }
    let proposals = read_proposals_unlocked(staff_id, &index)?;
    Ok(MemorySnapshot {
        staff_id: staff_id.to_string(),
        pinned,
        state,
        recent_journals,
        proposals,
        index,
    })
}

fn list_journal_dates_unlocked(staff_id: &str) -> Result<Vec<String>, String> {
    let paths = paths(staff_id)?;
    let mut dates = Vec::new();
    if let Ok(entries) = fs::read_dir(&paths.journal) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(date) = name.strip_suffix(".md") {
                if valid_journal_date(date) {
                    dates.push(date.to_string());
                }
            }
        }
    }
    dates.sort();
    dates.reverse();
    Ok(dates)
}

#[allow(dead_code)]
pub fn read_proposals(staff_id: &str) -> Result<Vec<MemoryProposal>, String> {
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let index = read_index_unlocked(staff_id);
    read_proposals_unlocked(staff_id, &index)
}

fn read_proposals_unlocked(
    staff_id: &str,
    index: &MemoryIndex,
) -> Result<Vec<MemoryProposal>, String> {
    let target = "proposals.jsonl";
    if is_tombstoned(index, target) {
        return Ok(Vec::new());
    }
    let raw = read_text(&paths(staff_id)?.proposals, MAX_PROPOSALS_BYTES)?;
    Ok(raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<MemoryProposal>(line).ok())
        .collect())
}

/// 模型只能写候选区；此函数不修改 pinned/state 的权威内容。
pub fn append_proposal(
    staff_id: &str,
    kind: &str,
    text: &str,
    source_refs: Vec<MemorySourceRef>,
) -> Result<MemoryProposal, String> {
    if text.trim().is_empty() {
        return Err("proposal text 不能为空".to_string());
    }
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_paths_and_files_unlocked(staff_id)?;
    let mut index = read_index_unlocked(staff_id);
    if is_tombstoned(&index, "proposals.jsonl") {
        return Err("proposal 区已被忘记，不能静默重写".to_string());
    }
    let mut proposals = read_proposals_unlocked(staff_id, &index)?;
    let proposal = MemoryProposal {
        proposal_id: short_id("memp"),
        staff_id: staff_id.to_string(),
        kind: kind.trim().to_string(),
        text: text.trim().chars().take(16_384).collect(),
        source_refs: normalize_source_refs(source_refs),
        created_at: now_ms(),
        status: "pending".to_string(),
    };
    proposals.push(proposal.clone());
    let body = proposals
        .iter()
        .map(|entry| serde_json::to_string(entry).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?
        .join("\n")
        + "\n";
    if body.len() as u64 > MAX_PROPOSALS_BYTES {
        return Err("proposal 区超过大小上限".to_string());
    }
    crate::storage::write_atomic(&paths(staff_id)?.proposals, &body)?;
    append_history_unlocked(
        staff_id,
        "proposal",
        "proposals.jsonl",
        "memory proposal appended",
        proposal.source_refs.clone(),
    )?;
    set_file_meta(
        &mut index,
        MemoryFileMeta {
            path: "proposals.jsonl".to_string(),
            kind: "proposal".to_string(),
            bytes: body.len() as u64,
            updated_at: now_ms(),
            source_refs: proposal.source_refs.clone(),
        },
    );
    save_index_unlocked(staff_id, &mut index)?;
    Ok(proposal)
}

/// User-confirmed proposal promotion. Accepting appends to pinned/state and
/// marks the candidate accepted; rejecting only changes its audit status.
pub fn resolve_proposal(
    staff_id: &str,
    proposal_id: &str,
    accept: bool,
    layer: Option<&str>,
) -> Result<MemoryProposal, String> {
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_paths_and_files_unlocked(staff_id)?;
    let mut index = read_index_unlocked(staff_id);
    let mut proposals = read_proposals_unlocked(staff_id, &index)?;
    let resolved = {
        let proposal = proposals
            .iter_mut()
            .find(|proposal| proposal.proposal_id == proposal_id)
            .ok_or_else(|| format!("记忆候选不存在: {proposal_id}"))?;
        if proposal.status != "pending" {
            return Err(format!("记忆候选已处理（{}）", proposal.status));
        }
        if accept {
            let (target, kind, max_bytes) = match layer.unwrap_or("state") {
                "pinned" => ("pinned.md", "pinned", MAX_PINNED_BYTES),
                "state" => ("state.md", "state", MAX_STATE_BYTES),
                other => return Err(format!("未知记忆层: {other}")),
            };
            let existing = read_file_unlocked(staff_id, target, max_bytes)?;
            let line = format!("- [{}] {}", proposal.kind, proposal.text.trim());
            let body = if existing.trim().is_empty() {
                line
            } else {
                format!("{}\n{line}", existing.trim_end())
            };
            write_file_unlocked(
                staff_id,
                target,
                kind,
                &body,
                max_bytes,
                proposal.source_refs.clone(),
            )?;
            proposal.status = "accepted".to_string();
        } else {
            proposal.status = "rejected".to_string();
        }
        proposal.clone()
    };
    let body = proposals
        .iter()
        .map(|entry| serde_json::to_string(entry).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()?
        .join("\n")
        + "\n";
    if body.len() as u64 > MAX_PROPOSALS_BYTES {
        return Err("proposal 区超过大小上限".to_string());
    }
    crate::storage::write_atomic(&paths(staff_id)?.proposals, &body)?;
    append_history_unlocked(
        staff_id,
        if accept { "proposal_accept" } else { "proposal_reject" },
        "proposals.jsonl",
        if accept { "memory proposal accepted" } else { "memory proposal rejected" },
        resolved.source_refs.clone(),
    )?;
    index = read_index_unlocked(staff_id);
    set_file_meta(
        &mut index,
        MemoryFileMeta {
            path: "proposals.jsonl".to_string(),
            kind: "proposal".to_string(),
            bytes: body.len() as u64,
            updated_at: now_ms(),
            source_refs: resolved.source_refs.clone(),
        },
    );
    save_index_unlocked(staff_id, &mut index)?;
    Ok(resolved)
}

#[cfg(test)]
pub fn read_history(staff_id: &str) -> Vec<MemoryHistoryEntry> {
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    read_history_unlocked(staff_id)
}

#[allow(dead_code)]
pub fn append_history(
    staff_id: &str,
    action: &str,
    target: &str,
    summary: &str,
    source_refs: Vec<MemorySourceRef>,
) -> Result<MemoryHistoryEntry, String> {
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_paths_and_files_unlocked(staff_id)?;
    append_history_unlocked(staff_id, action, target, summary, source_refs)
}

#[cfg(test)]
pub fn is_tombstoned_target(staff_id: &str, target: &str) -> bool {
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    is_tombstoned(&read_index_unlocked(staff_id), target)
}

/// 删除一个正文/候选目标并留下 tombstone。先写审计行，再移除正文，
/// 这样即使进程在两者之间崩溃，重连也不会把它当成“新文件”复活。
pub fn forget(
    staff_id: &str,
    target: &str,
    reason: &str,
    source_refs: Vec<MemorySourceRef>,
) -> Result<(), String> {
    relative_target(target)?;
    if target != "pinned.md"
        && target != "state.md"
        && target != "proposals.jsonl"
        && !target.starts_with("journal/")
    {
        return Err(format!("不允许忘记未知 memory target: {target}"));
    }
    if target.starts_with("journal/") {
        let date = target
            .strip_prefix("journal/")
            .and_then(|value| value.strip_suffix(".md"))
            .unwrap_or("");
        if !valid_journal_date(date) {
            return Err(format!("非法 journal target: {target}"));
        }
    }
    let _guard = MEMORY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    ensure_paths_and_files_unlocked(staff_id)?;
    let mut index = read_index_unlocked(staff_id);
    if is_tombstoned(&index, target) {
        return Ok(());
    }
    let at = now_ms();
    let source_refs = normalize_source_refs(source_refs);
    append_history_unlocked(
        staff_id,
        "forget",
        target,
        reason
            .trim()
            .chars()
            .take(8_192)
            .collect::<String>()
            .as_str(),
        source_refs.clone(),
    )?;
    let path = paths(staff_id)?.root.join(target);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("删除 {target} 失败: {error}")),
    }
    remove_file_meta(&mut index, target);
    merge_tombstones(
        &mut index,
        vec![MemoryTombstone {
            target: target.to_string(),
            deleted_at: at,
            reason: reason.trim().chars().take(8_192).collect(),
            source_refs,
        }],
    );
    save_index_unlocked(staff_id, &mut index)
}

#[cfg(test)]
pub fn forget_pinned(staff_id: &str, reason: &str) -> Result<(), String> {
    forget(staff_id, "pinned.md", reason, Vec::new())
}

#[cfg(test)]
pub fn forget_state(staff_id: &str, reason: &str) -> Result<(), String> {
    forget(staff_id, "state.md", reason, Vec::new())
}

/// Unix 毫秒对应的 UTC `YYYY-MM-DD`（用于 journal 文件名，稳定且无时区歧义）。
pub fn journal_date_for_ms(timestamp_ms: u64) -> String {
    let days = (timestamp_ms / 86_400_000) as i64;
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02}")
}

fn civil_from_days(days_since_epoch: i64) -> (i64, u32, u32) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year, month as u32, day as u32)
}

fn valid_journal_date(value: &str) -> bool {
    if value.len() != 10
        || value.as_bytes().get(4) != Some(&b'-')
        || value.as_bytes().get(7) != Some(&b'-')
    {
        return false;
    }
    let parts: Vec<&str> = value.split('-').collect();
    if parts.len() != 3
        || parts
            .iter()
            .any(|part| part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()))
    {
        return false;
    }
    let Ok(year) = parts[0].parse::<u32>() else {
        return false;
    };
    let Ok(month) = parts[1].parse::<u32>() else {
        return false;
    };
    let Ok(day) = parts[2].parse::<u32>() else {
        return false;
    };
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        _ => return false,
    };
    year >= 1970 && day >= 1 && day <= max_day
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chief::store::SANDBOX_LOCK;

    fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("marionette-chief-memory-{name}-{}", now_ms()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        SANDBOX_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[test]
    fn layout_roundtrip_and_source_metadata() {
        let _guard = lock();
        let root = sandbox("roundtrip");
        set_root_override_for_tests(Some(root.clone()));
        let paths = ensure_staff_memory("staff_a").unwrap();
        assert!(paths.pinned.exists());
        assert!(paths.state.exists());
        assert!(paths.history.exists());
        assert!(paths.index.exists());
        let source = MemorySourceRef::task("task_1");
        write_pinned_with_source_refs("staff_a", "用户偏好：简体中文", vec![source.clone()])
            .unwrap();
        write_journal("staff_a", "2026-11-17", "完成了构建").unwrap();
        assert_eq!(read_pinned("staff_a").unwrap(), "用户偏好：简体中文");
        assert_eq!(read_journal("staff_a", "2026-11-17").unwrap(), "完成了构建");
        let index = read_index("staff_a");
        let pinned = index
            .files
            .iter()
            .find(|file| file.path == "pinned.md")
            .unwrap();
        assert_eq!(pinned.source_refs[0].task_id.as_deref(), Some("task_1"));
        assert!(read_history("staff_a")
            .iter()
            .any(|entry| entry.action == "write"));
        set_root_override_for_tests(None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn append_journal_preserves_earlier_entries_and_source_refs() {
        let _guard = lock();
        let root = sandbox("journal-append");
        set_root_override_for_tests(Some(root.clone()));
        ensure_staff_memory("staff_a").unwrap();
        write_journal("staff_a", "2026-11-17", "first").unwrap();
        append_journal_with_source_refs(
            "staff_a",
            "2026-11-17",
            "second",
            vec![MemorySourceRef::task("task_1")],
        )
        .unwrap();
        let body = read_journal("staff_a", "2026-11-17").unwrap();
        assert!(body.contains("first"));
        assert!(body.contains("second"));
        let snapshot = recall("staff_a", 7).unwrap();
        let journal = snapshot
            .recent_journals
            .iter()
            .find(|entry| entry.date == "2026-11-17")
            .unwrap();
        assert_eq!(
            journal.meta.source_refs[0].task_id.as_deref(),
            Some("task_1")
        );
        set_root_override_for_tests(None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn proposal_resolution_requires_explicit_layer_and_is_one_shot() {
        let _guard = lock();
        let root = sandbox("proposal-resolve");
        set_root_override_for_tests(Some(root.clone()));
        ensure_staff_memory("staff_a").unwrap();
        let proposal = append_proposal(
            "staff_a",
            "preference",
            "Use short final reports.",
            vec![MemorySourceRef::task("task_1")],
        )
        .unwrap();
        let accepted = resolve_proposal(
            "staff_a",
            &proposal.proposal_id,
            true,
            Some("pinned"),
        )
        .unwrap();
        assert_eq!(accepted.status, "accepted");
        assert!(read_pinned("staff_a").unwrap().contains("Use short final reports."));
        assert!(resolve_proposal(
            "staff_a",
            &proposal.proposal_id,
            false,
            None
        )
        .is_err());
        set_root_override_for_tests(None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn forget_writes_tombstone_and_blocks_revival() {
        let _guard = lock();
        let root = sandbox("forget");
        set_root_override_for_tests(Some(root.clone()));
        write_pinned("staff_a", "remember me").unwrap();
        forget_pinned("staff_a", "user requested deletion").unwrap();
        assert_eq!(read_pinned("staff_a").unwrap(), "");
        assert!(is_tombstoned_target("staff_a", "pinned.md"));
        assert!(write_pinned("staff_a", "revived").is_err());
        // ensure 不应把目标重新写出来。
        let paths = ensure_staff_memory("staff_a").unwrap();
        assert!(!paths.pinned.exists());
        assert!(read_history("staff_a")
            .iter()
            .any(|entry| entry.action == "forget"));
        set_root_override_for_tests(None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn corrupt_index_reconstructs_and_history_preserves_tombstone() {
        let _guard = lock();
        let root = sandbox("reconstruct");
        set_root_override_for_tests(Some(root.clone()));
        write_state("staff_a", "current context").unwrap();
        forget_state("staff_a", "forget it").unwrap();
        fs::write(staff_dir("staff_a").unwrap().join("index.json"), "{bad").unwrap();
        assert!(is_tombstoned_target("staff_a", "state.md"));
        assert!(write_state("staff_a", "must not return").is_err());
        set_root_override_for_tests(None);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn journal_date_conversion_and_validation() {
        assert_eq!(journal_date_for_ms(0), "1970-01-01");
        assert!(valid_journal_date("2024-02-29"));
        assert!(!valid_journal_date("2023-02-29"));
        assert!(!valid_journal_date("2026-13-01"));
        assert!(journal_target("2026-11-17").is_ok());
    }
}
