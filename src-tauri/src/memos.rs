//! Memo inbox (edge dock stage 3, docs/plan-edge-dock.md §4).
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
use std::path::{Path, PathBuf};

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

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "ico", "avif"];

fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| IMAGE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Copy one dragged-in FILE into files/<hash>.<ext>; content hash is both the
/// dedup key (identical content re-dropped returns the existing entry) and
/// the stored name (stable across restarts, no unicode pitfalls).
fn save_one_file(source: &Path, source_label: &str, created_at: &str) -> Result<MemoEntry, String> {
    let bytes = std::fs::read(source).map_err(|e| format!("read {source_label} failed: {e}"))?;
    let hash = sha256_hex(&bytes);
    let ext = source
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let stored_name = format!("{hash}{ext}");
    let target = files_dir()?.join(&stored_name);
    if !target.exists() {
        std::fs::create_dir_all(target.parent().unwrap_or(Path::new(".")))
            .map_err(|e| format!("create files dir failed: {e}"))?;
        std::fs::write(&target, &bytes).map_err(|e| format!("store memo file failed: {e}"))?;
    }
    let title = source
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(source_label)
        .to_string();
    Ok(MemoEntry {
        id: hash,
        kind: if is_image_path(source) { "image" } else { "file" }.to_string(),
        title,
        path: Some(target.to_string_lossy().to_string()),
        content: None,
        size: Some(bytes.len() as u64),
        created_at: created_at.to_string(),
        source: Some(source_label.to_string()),
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

fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target).map_err(|e| format!("create memo folder failed: {e}"))?;
    let entries = std::fs::read_dir(source).map_err(|e| format!("read folder failed: {e}"))?;
    for entry in entries.flatten() {
        let to = target.join(entry.file_name());
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            copy_tree(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)
                .map_err(|e| format!("copy into memo folder failed: {e}"))?;
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
    entries.retain(|e| e.id != entry.id);
    entries.insert(0, entry);
    // Newest-first, cap at 500: the box is a tray, not an archive backend.
    entries.truncate(500);
    write_index(dir, &entries)?;
    Ok(entries)
}

#[tauri::command]
pub fn memo_list() -> Result<Vec<MemoEntry>, String> {
    Ok(read_index(&memos_dir()?))
}

/// Explorer drag-in: copy each path (file or folder) into the box.
/// Returns the full list so callers refresh count + rows in one round trip.
#[tauri::command]
pub fn memo_save_paths(paths: Vec<String>, source: Option<String>) -> Result<Vec<MemoEntry>, String> {
    let dir = memos_dir()?;
    let created_at = now_ms();
    let mut latest_err: Option<String> = None;
    for raw in &paths {
        let path = PathBuf::from(raw);
        let label = source.clone().unwrap_or_else(|| crate::ts!("拖入", "Dropped in"));
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
pub fn memo_save_text(text: String, title: Option<String>, source: Option<String>) -> Result<MemoEntry, String> {
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
    };
    upsert(&dir, entry.clone())?;
    Ok(entry)
}

/// File or directory — folder memos store a tree path.
fn remove_any(path: &str) -> std::io::Result<()> {
    let meta = std::fs::metadata(path)?;
    if meta.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Remove one entry; a stored file goes only when no other entry uses it.
#[tauri::command]
pub fn memo_delete(id: String) -> Result<Vec<MemoEntry>, String> {
    let dir = memos_dir()?;
    let mut entries = read_index(&dir);
    let removed = entries.iter().position(|e| e.id == id);
    let Some(at) = removed else {
        return Ok(entries);
    };
    let entry = entries.remove(at);
    if let Some(path) = entry.path.as_deref() {
        let still_used = entries
            .iter()
            .any(|e| e.path.as_deref() == Some(path));
        if !still_used {
            // Best effort: a locked file must not block the list removal.
            if let Err(err) = remove_any(path) {
                crate::debug_log::append(
                    "memos",
                    "warn",
                    "",
                    "memo backing file delete failed",
                    Some(&format!("{path}: {err}")),
                );
            }
        }
    }
    write_index(&dir, &entries)?;
    Ok(entries)
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

    #[test]
    fn index_round_trip_and_fail_open() {
        let dir = tempdir("roundtrip");
        assert!(read_index(&dir).is_empty());
        let entry = MemoEntry {
            id: "t1".into(),
            kind: "text".into(),
            title: "hello".into(),
            path: None,
            content: Some("hello world".into()),
            size: None,
            created_at: "1".into(),
            source: None,
        };
        upsert(&dir, entry.clone()).expect("upsert");
        let read = read_index(&dir);
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].id, "t1");
        // Corrupt index must fail open to an empty list, not error out.
        std::fs::write(dir.join("index.json"), "{not json").expect("corrupt");
        assert!(read_index(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn upsert_dedups_by_id_newest_first_and_caps() {
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
            };
            upsert(&dir, entry).expect("upsert");
        }
        let entries = read_index(&dir);
        assert_eq!(entries.len(), 500);
        assert_eq!(entries[0].id, "e504");
        // Re-upserting an old id moves it to the front without growing.
        let again = MemoEntry {
            id: "e10".into(),
            kind: "text".into(),
            title: "again".into(),
            path: None,
            content: None,
            size: None,
            created_at: "999".into(),
            source: None,
        };
        upsert(&dir, again).expect("upsert again");
        let entries = read_index(&dir);
        assert_eq!(entries.len(), 500);
        assert_eq!(entries[0].id, "e10");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn url_detection() {
        assert!(looks_like_url("https://example.com/x?y=1"));
        assert!(looks_like_url("http://a.b"));
        assert!(!looks_like_url("看看这个 https://example.com"));
        assert!(!looks_like_url("just some text"));
    }
}
