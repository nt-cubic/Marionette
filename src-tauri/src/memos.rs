//! Memo inbox (edge dock stage 3, docs/plans/plan-edge-dock.md §4).
//!
//! Deterministic local storage under `~/.marionette/memos/`:
//!   index.json  — the entry list (atomic tmp+rename, same discipline as the
//!                 sessions index in storage.rs)
//!   files/      — dragged-in files COPIED in (content-hash names; only
//!                 storing references would break on move/rename)
//!
//! Fail-open reads (missing/corrupt index → empty list): a broken memo store
//! must never take sessions or the UI down. No panics/unwraps — errors are
//! `Result<_, String>` all the way up (plan §7 red line).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

static MEMO_STORE_LOCK: Mutex<()> = Mutex::new(());

fn lock_store() -> Result<MutexGuard<'static, ()>, String> {
    MEMO_STORE_LOCK
        .lock()
        .map_err(|_| "memo store lock poisoned".to_string())
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoEntry {
    pub id: String,
    /// file | text | image | link
    pub kind: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Unix milliseconds since the epoch, as a string.
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// User pin (2026-11-18): display-level float-to-top; never reordered in
    /// the index itself — upsert ordering stays newest-first.
    #[serde(default)]
    pub pinned: bool,
    /// User tags (2026-11-18); sanitized on write, see `sanitize_tags`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

/// Field patch for `memo_update`. `None` = leave unchanged (no nested
/// Option games — clearing is not a supported operation for any field).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoPatch {
    pub title: Option<String>,
    pub content: Option<String>,
    pub tags: Option<Vec<String>>,
    pub pinned: Option<bool>,
}

/// Trim, drop empties, char-truncate to 24, exact-dedup, cap at 8.
fn sanitize_tags(tags: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tag in tags {
        let trimmed: String = tag.trim().chars().take(24).collect();
        if trimmed.is_empty() || out.contains(&trimmed) {
            continue;
        }
        out.push(trimmed);
        if out.len() == 8 {
            break;
        }
    }
    out
}

fn memos_dir() -> Result<PathBuf, String> {
    crate::app_paths::global_dir().map(|dir| dir.join("memos"))
}

fn files_dir() -> Result<PathBuf, String> {
    Ok(memos_dir()?.join("files"))
}

fn read_index(dir: &Path) -> Vec<MemoEntry> {
    let path = dir.join("index.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            crate::debug_log::append(
                "memos",
                "warn",
                "",
                "memo index unreadable — starting empty",
                Some(&err.to_string()),
            );
            return Vec::new();
        }
    };
    match serde_json::from_str::<Vec<MemoEntry>>(&raw) {
        Ok(entries) => entries,
        Err(err) => {
            crate::debug_log::append(
                "memos",
                "warn",
                "",
                "memo index corrupt — starting empty",
                Some(&err.to_string()),
            );
            Vec::new()
        }
    }
}

fn write_index(dir: &Path, entries: &[MemoEntry]) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create memos dir failed: {e}"))?;
    let tmp = dir.join("index.json.tmp");
    let json = serde_json::to_string_pretty(entries)
        .map_err(|e| format!("serialize memo index failed: {e}"))?;
    std::fs::write(&tmp, json).map_err(|e| format!("write memo index failed: {e}"))?;
    std::fs::rename(&tmp, dir.join("index.json"))
        .map_err(|e| format!("swap memo index failed: {e}"))
}

fn now_ms() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "avif",
];

fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn stored_file_for_hash(root: &Path, hash: &str) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .find(|path| {
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .is_some_and(|stem| stem == hash)
        })
}

/// Copy one dragged-in FILE into files/<hash>.<ext> without loading it into RAM.
/// Content hash is both the dedup key and the stable stored name.
fn save_one_file(source: &Path, source_label: &str, created_at: &str) -> Result<MemoEntry, String> {
    let root = files_dir()?;
    std::fs::create_dir_all(&root).map_err(|e| format!("create files dir failed: {e}"))?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temp = root.join(format!(".incoming-{}-{nonce}.tmp", std::process::id()));

    let copied = (|| -> Result<(String, u64, PathBuf), String> {
        let mut input =
            std::fs::File::open(source).map_err(|e| format!("read {source_label} failed: {e}"))?;
        let mut output = std::fs::File::create(&temp)
            .map_err(|e| format!("create memo temp file failed: {e}"))?;
        let mut hasher = Sha256::new();
        let mut size = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = input
                .read(&mut buffer)
                .map_err(|e| format!("read {source_label} failed: {e}"))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            output
                .write_all(&buffer[..read])
                .map_err(|e| format!("write memo temp file failed: {e}"))?;
            size += read as u64;
        }
        output
            .flush()
            .map_err(|e| format!("flush memo temp file failed: {e}"))?;
        drop(output);
        let mut hash = String::with_capacity(64);
        for byte in hasher.finalize() {
            hash.push_str(&format!("{byte:02x}"));
        }
        let target = if let Some(existing) = stored_file_for_hash(&root, &hash) {
            std::fs::remove_file(&temp)
                .map_err(|e| format!("remove duplicate memo temp failed: {e}"))?;
            existing
        } else {
            let ext = source
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| format!(".{e}"))
                .unwrap_or_default();
            let target = root.join(format!("{hash}{ext}"));
            std::fs::rename(&temp, &target).map_err(|e| format!("store memo file failed: {e}"))?;
            target
        };
        Ok((hash, size, target))
    })();

    if copied.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    let (hash, size, target) = copied?;
    let title = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(source_label)
        .to_string();
    Ok(MemoEntry {
        id: hash,
        kind: if is_image_path(source) {
            "image"
        } else {
            "file"
        }
        .to_string(),
        title,
        path: Some(target.to_string_lossy().to_string()),
        content: None,
        size: Some(size),
        created_at: created_at.to_string(),
        source: Some(source_label.to_string()),
        pinned: false,
        tags: Vec::new(),
    })
}

/// Copy a dragged-in DIRECTORY as files/<id>/<name>/… . No content dedup for
/// trees (hashing every byte of a big folder on drop is a stall); the id is
/// unique per drop.
fn save_one_dir(source: &Path, source_label: &str, created_at: &str) -> Result<MemoEntry, String> {
    let seed = format!("{:?}|{}|{}", source, walk_stat(source), created_at);
    let id = format!("d{}", &sha256_hex(seed.as_bytes())[..16]);
    let base_name = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("folder")
        .to_string();
    let target_root = files_dir()?.join(&id);
    if !target_root.exists() {
        copy_tree(source, &target_root)?;
    }
    let size = dir_size(&target_root).unwrap_or(0);
    Ok(MemoEntry {
        id,
        kind: "file".to_string(),
        title: base_name,
        path: Some(target_root.to_string_lossy().to_string()),
        content: None,
        size: Some(size),
        created_at: created_at.to_string(),
        source: Some(source_label.to_string()),
        pinned: false,
        tags: Vec::new(),
    })
}

/// Rough fingerprint for the folder id: walk depth-limited so a huge tree
/// cannot stall the drop.
fn walk_stat(root: &Path) -> String {
    let mut acc = String::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        visited += 1;
        if visited > 200 || depth > 6 {
            break;
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                acc.push_str(&name);
                if let Ok(meta) = entry.metadata() {
                    if meta.is_dir() {
                        stack.push((entry.path(), depth + 1));
                    } else {
                        acc.push_str(&meta.len().to_string());
                    }
                }
            }
        }
    }
    acc
}

/// pub(crate)：插件层装技能复用同一棵复制树（2026-11-06）。
pub(crate) fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target).map_err(|e| format!("create folder failed: {e}"))?;
    let entries = std::fs::read_dir(source).map_err(|e| format!("read folder failed: {e}"))?;
    for entry in entries.flatten() {
        let to = target.join(entry.file_name());
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            copy_tree(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)
                .map_err(|e| format!("copy into folder failed: {e}"))?;
        }
    }
    Ok(())
}

fn dir_size(root: &Path) -> Option<u64> {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    let mut visited = 0usize;
    while let Some(dir) = stack.pop() {
        visited += 1;
        if visited > 500 {
            return Some(total); // good enough for a size hint
        }
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_dir() {
                    stack.push(entry.path());
                } else {
                    total += meta.len();
                }
            }
        }
    }
    Some(total)
}

/// URL shape for the link kind (plain http/https — mailto etc. stay text).
fn looks_like_url(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.contains(char::is_whitespace) {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

fn upsert(dir: &Path, entry: MemoEntry) -> Result<Vec<MemoEntry>, String> {
    let mut entries = read_index(dir);
    // Re-dropping the same bytes keeps the original row (and its pin/tags) in
    // place. The file copy itself already reuses files/<hash>.<ext>.
    if entries.iter().any(|existing| existing.id == entry.id) {
        return Ok(entries);
    }
    entries.insert(0, entry);
    // Newest-first, cap at 500: the box is a tray, not an archive backend.
    entries.truncate(500);
    write_index(dir, &entries)?;
    Ok(entries)
}

#[tauri::command]
pub fn memo_list() -> Result<Vec<MemoEntry>, String> {
    let _guard = lock_store()?;
    Ok(read_index(&memos_dir()?))
}

/// Explorer drag-in: copy each path (file or folder) into the box.
/// Returns the full list so callers refresh count + rows in one round trip.
#[tauri::command]
pub fn memo_save_paths(
    paths: Vec<String>,
    source: Option<String>,
) -> Result<Vec<MemoEntry>, String> {
    let _guard = lock_store()?;
    let dir = memos_dir()?;
    let created_at = now_ms();
    let mut latest_err: Option<String> = None;
    for raw in &paths {
        let path = PathBuf::from(raw);
        let label = source
            .clone()
            .unwrap_or_else(|| crate::ts!("拖入", "Dropped in"));
        let result = if path.is_dir() {
            save_one_dir(&path, &label, &created_at)
        } else {
            save_one_file(&path, &label, &created_at)
        };
        match result {
            Ok(entry) => {
                if let Err(err) = upsert(&dir, entry) {
                    latest_err = Some(err);
                }
            }
            Err(err) => {
                crate::debug_log::append(
                    "memos",
                    "warn",
                    "",
                    "memo save path failed",
                    Some(&format!("{raw}: {err}")),
                );
                latest_err = Some(err);
            }
        }
    }
    let entries = read_index(&dir);
    if entries.is_empty() {
        if let Some(err) = latest_err {
            return Err(err);
        }
    }
    Ok(entries)
}

/// Ctrl+V channel: text or a bare URL (→ link kind).
#[tauri::command]
pub fn memo_save_text(
    text: String,
    title: Option<String>,
    source: Option<String>,
) -> Result<MemoEntry, String> {
    let _guard = lock_store()?;
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        return Err("nothing to save".to_string());
    }
    let dir = memos_dir()?;
    let created_at = now_ms();
    let seed = format!("{}|{}", created_at, trimmed);
    let id = format!("t{}", &sha256_hex(seed.as_bytes())[..16]);
    let is_link = looks_like_url(&trimmed);
    let entry = MemoEntry {
        id,
        kind: if is_link { "link" } else { "text" }.to_string(),
        title: title
            .filter(|t| !t.trim().is_empty())
            .map(|t| t.trim().chars().take(120).collect())
            .unwrap_or_else(|| trimmed.chars().take(80).collect()),
        path: None,
        content: Some(trimmed),
        size: None,
        created_at,
        source,
        pinned: false,
        tags: Vec::new(),
    };
    upsert(&dir, entry.clone())?;
    Ok(entry)
}

/// In-place field patch (2026-11-18): editing must NOT reorder the feed, so
/// this replaces the entry at its index instead of going through `upsert`
/// (which front-inserts). The id is an identity, not a content hash — it
/// stays stable across content edits.
fn update_entry(dir: &Path, id: &str, patch: &MemoPatch) -> Result<Vec<MemoEntry>, String> {
    let mut entries = read_index(dir);
    let at = entries
        .iter()
        .position(|e| e.id == id)
        .ok_or_else(|| format!("memo {id} not found"))?;
    let entry = &mut entries[at];
    if let Some(title) = patch.title.as_deref() {
        let trimmed = title.trim();
        if !trimmed.is_empty() {
            entry.title = trimmed.chars().take(120).collect();
        }
    }
    if let Some(content) = patch.content.as_deref() {
        if entry.kind != "text" && entry.kind != "link" {
            return Err("content can only be edited on text/link memos".to_string());
        }
        let trimmed = content.trim();
        if trimmed.is_empty() {
            return Err("memo content cannot be empty".to_string());
        }
        // Keep the kind honest with the new content: a link edited away from
        // a bare URL becomes a text memo and vice versa.
        entry.kind = if looks_like_url(trimmed) {
            "link"
        } else {
            "text"
        }
        .to_string();
        entry.content = Some(trimmed.to_string());
    }
    if let Some(tags) = patch.tags.clone() {
        entry.tags = sanitize_tags(tags);
    }
    if let Some(pinned) = patch.pinned {
        entry.pinned = pinned;
    }
    write_index(dir, &entries)?;
    Ok(entries)
}

/// Edit / pin / tag one entry in place; returns the full refreshed list
/// (same round-trip shape as `memo_delete`).
#[tauri::command]
pub fn memo_update(id: String, patch: MemoPatch) -> Result<Vec<MemoEntry>, String> {
    let _guard = lock_store()?;
    update_entry(&memos_dir()?, &id, &patch)
}

/// File or directory — folder memos store a tree path.
fn remove_any(path: &Path) -> std::io::Result<()> {
    let meta = std::fs::metadata(path)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

fn owned_stored_path(files_root: &Path, raw: &str) -> Option<PathBuf> {
    let root = std::fs::canonicalize(files_root).ok()?;
    let target = std::fs::canonicalize(raw).ok()?;
    (target != root && target.starts_with(&root)).then_some(target)
}

/// Remove one entry; a stored file goes only when it is still inside files/.
fn delete_entry(dir: &Path, id: &str) -> Result<Vec<MemoEntry>, String> {
    let mut entries = read_index(dir);
    let removed = entries.iter().position(|e| e.id == id);
    let Some(at) = removed else {
        return Ok(entries);
    };
    let entry = entries.remove(at);
    if let Some(path) = entry.path.as_deref() {
        let still_used = entries.iter().any(|e| e.path.as_deref() == Some(path));
        if !still_used {
            if let Some(target) = owned_stored_path(&dir.join("files"), path) {
                // Best effort: a locked file must not block the list removal.
                if let Err(err) = remove_any(&target) {
                    crate::debug_log::append(
                        "memos",
                        "warn",
                        "",
                        "memo backing file delete failed",
                        Some(&format!("{path}: {err}")),
                    );
                }
            } else {
                crate::debug_log::append(
                    "memos",
                    "warn",
                    "",
                    "memo backing path escaped store",
                    Some(path),
                );
            }
        }
    }
    write_index(dir, &entries)?;
    Ok(entries)
}

#[tauri::command]
pub fn memo_delete(id: String) -> Result<Vec<MemoEntry>, String> {
    let _guard = lock_store()?;
    delete_entry(&memos_dir()?, &id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marionette-memos-test-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("create tempdir");
        dir
    }

    fn text_entry(id: &str) -> MemoEntry {
        MemoEntry {
            id: id.into(),
            kind: "text".into(),
            title: format!("title {id}"),
            path: None,
            content: Some("hello world".into()),
            size: None,
            created_at: "1".into(),
            source: None,
            pinned: false,
            tags: Vec::new(),
        }
    }

    #[test]
    fn index_round_trip_and_fail_open() {
        let dir = tempdir("roundtrip");
        assert!(read_index(&dir).is_empty());
        upsert(&dir, text_entry("t1")).expect("upsert");
        let read = read_index(&dir);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].id, "t1");
        // Corrupt index must fail open to an empty list, not error out.
        std::fs::write(dir.join("index.json"), "{not json").expect("corrupt");
        assert!(read_index(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_dedups_without_reordering_and_caps() {
        let dir = tempdir("dedup");
        for i in 0..505 {
            let entry = MemoEntry {
                id: format!("e{i}"),
                kind: "text".into(),
                title: format!("t{i}"),
                path: None,
                content: Some("c".into()),
                size: None,
                created_at: format!("{i}"),
                source: None,
                pinned: false,
                tags: Vec::new(),
            };
            upsert(&dir, entry).expect("upsert");
        }
        let entries = read_index(&dir);
        assert_eq!(entries.len(), 500);
        assert_eq!(entries[0].id, "e504");
        let mut pinned = entries[17].clone();
        pinned.pinned = true;
        pinned.tags = vec!["keep".into()];
        let mut rewritten = entries.clone();
        rewritten[17] = pinned.clone();
        write_index(&dir, &rewritten).expect("seed metadata");
        let before_ids: Vec<String> = rewritten.iter().map(|entry| entry.id.clone()).collect();

        // Re-dropping the same bytes must keep the existing row, metadata and order.
        let mut again = pinned.clone();
        again.title = "again".into();
        again.created_at = "999".into();
        again.pinned = false;
        again.tags.clear();
        upsert(&dir, again).expect("upsert again");
        let entries = read_index(&dir);
        assert_eq!(entries.len(), 500);
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>(),
            before_ids
        );
        let kept = entries.iter().find(|entry| entry.id == pinned.id).unwrap();
        assert_eq!(kept.title, pinned.title);
        assert_eq!(kept.created_at, pinned.created_at);
        assert!(kept.pinned);
        assert_eq!(kept.tags, vec!["keep".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_never_follows_an_index_path_outside_files() {
        let dir = tempdir("safe-delete");
        let files = dir.join("files");
        std::fs::create_dir_all(&files).expect("files dir");
        let outside = dir.join("outside.txt");
        let inside = files.join("inside.txt");
        std::fs::write(&outside, "outside").expect("outside file");
        std::fs::write(&inside, "inside").expect("inside file");

        let mut bad = text_entry("bad");
        bad.kind = "file".into();
        bad.path = Some(outside.to_string_lossy().to_string());
        upsert(&dir, bad).expect("bad entry");
        delete_entry(&dir, "bad").expect("delete bad row");
        assert!(outside.exists(), "outside path must never be deleted");

        let mut good = text_entry("good");
        good.kind = "file".into();
        good.path = Some(inside.to_string_lossy().to_string());
        upsert(&dir, good).expect("good entry");
        delete_entry(&dir, "good").expect("delete good row");
        assert!(!inside.exists(), "owned stored file should be removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn url_detection() {
        assert!(looks_like_url("https://example.com/x?y=1"));
        assert!(looks_like_url("http://a.b"));
        assert!(!looks_like_url("看看这个 https://example.com"));
        assert!(!looks_like_url("just some text"));
    }

    #[test]
    fn patch_updates_in_place_without_reordering() {
        let dir = tempdir("patch");
        for id in ["a", "b", "c"] {
            let mut entry = text_entry(id);
            entry.created_at = id.into();
            upsert(&dir, entry).expect("upsert");
        }
        let entries = update_entry(
            &dir,
            "b",
            &MemoPatch {
                title: Some("renamed".into()),
                content: Some("edited body".into()),
                tags: Some(vec!["  work ".into(), "".into(), "work".into()]),
                pinned: Some(true),
            },
        )
        .expect("patch");
        // Feed order is untouched — b stays in the middle.
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["c", "b", "a"]);
        let b = entries.iter().find(|e| e.id == "b").unwrap();
        assert_eq!(b.title, "renamed");
        assert_eq!(b.content.as_deref(), Some("edited body"));
        assert_eq!(b.tags, vec!["work".to_string()]);
        assert!(b.pinned);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn patch_rejects_content_on_file_kind() {
        let dir = tempdir("patchfile");
        let mut file_entry = text_entry("f1");
        file_entry.kind = "file".into();
        file_entry.content = None;
        file_entry.path = Some("C:\\somewhere\\x.txt".into());
        upsert(&dir, file_entry).expect("upsert");
        let err = update_entry(
            &dir,
            "f1",
            &MemoPatch {
                content: Some("nope".into()),
                ..MemoPatch::default()
            },
        )
        .expect_err("must reject");
        assert!(err.contains("text/link"));
        // Title-only patch on a file entry is fine.
        update_entry(
            &dir,
            "f1",
            &MemoPatch {
                title: Some("renamed file".into()),
                ..MemoPatch::default()
            },
        )
        .expect("title patch ok");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn content_edit_flips_link_and_text_kinds() {
        let dir = tempdir("patchkind");
        let mut link = text_entry("l1");
        link.kind = "link".into();
        link.content = Some("https://example.com".into());
        upsert(&dir, link).expect("upsert");
        update_entry(
            &dir,
            "l1",
            &MemoPatch {
                content: Some("now plain text".into()),
                ..MemoPatch::default()
            },
        )
        .expect("patch");
        let entries = read_index(&dir);
        assert_eq!(entries[0].kind, "text");
        update_entry(
            &dir,
            "l1",
            &MemoPatch {
                content: Some("https://other.example.com".into()),
                ..MemoPatch::default()
            },
        )
        .expect("patch back");
        let entries = read_index(&dir);
        assert_eq!(entries[0].kind, "link");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tag_sanitization_caps_and_dedups() {
        let tags = sanitize_tags(vec![
            "  a  ".into(),
            "a".into(),
            "".into(),
            "   ".into(),
            "x".repeat(40),
            "b".into(),
            "c".into(),
            "d".into(),
            "e".into(),
            "f".into(),
            "g".into(),
            "h".into(),
            "i".into(),
        ]);
        assert_eq!(tags.len(), 8);
        assert_eq!(tags[0], "a");
        assert_eq!(tags[1].chars().count(), 24);
        assert_eq!(tags[7], "g"); // "h"/"i" are cut by the 8-tag cap
    }

    #[test]
    fn old_index_without_new_fields_parses_with_defaults() {
        let dir = tempdir("legacy");
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(
            dir.join("index.json"),
            r#"[{"id":"t9","kind":"text","title":"old","content":"legacy body","createdAt":"42"}]"#,
        )
        .expect("write legacy index");
        let entries = read_index(&dir);
        assert_eq!(entries.len(), 1);
        assert!(!entries[0].pinned);
        assert!(entries[0].tags.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn patch_unknown_id_is_an_error() {
        let dir = tempdir("patchmissing");
        let err = update_entry(
            &dir,
            "ghost",
            &MemoPatch {
                pinned: Some(true),
                ..MemoPatch::default()
            },
        )
        .expect_err("must fail");
        assert!(err.contains("not found"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
