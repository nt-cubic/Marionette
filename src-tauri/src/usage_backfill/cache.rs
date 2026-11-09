//! Incremental cache: `~/.marionette/usage/backfill-cache.json`.
//!
//! Keyed by `"<agent>::<file path>"`, invalidated by
//! `(length, modified-time)` fingerprint of the underlying file(s). A broken
//! cache fails open (treated as empty and rewritten on the next scan). Writes
//! go through [`crate::storage::write_atomic`]; SQLite sources fingerprint the
//! db together with its `-wal` so un-checkpointed writes invalidate too.
//!
//! Tests never touch the real `~/.marionette`: [`super::cache_root`] refuses
//! to run under `cfg!(test)` unless an override was installed (usage_store's
//! pattern), and the unit tests pass explicit temp paths to
//! [`load_at`]/[`save_at`] anyway.

use super::facts::SessionFacts;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct CacheEntry {
    pub fingerprint: String,
    #[serde(default)]
    pub sessions: Vec<SessionFacts>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Cache {
    #[serde(default)]
    pub version: u32,
    /// `"<agent>::<path>"` → entry.
    #[serde(default)]
    pub entries: BTreeMap<String, CacheEntry>,
}

static ROOT_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

#[cfg(test)]
pub(crate) fn set_root_override_for_tests(path: Option<PathBuf>) {
    *ROOT_OVERRIDE
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = path;
}

/// Directory holding the cache file. Under `cfg!(test)` the real
/// `~/.marionette` is off limits unless an override was installed (the
/// `#[ignore]` real-machine smoke test points the override at the real dir on
/// purpose).
pub(crate) fn cache_root() -> Result<PathBuf, String> {
    if let Some(path) = ROOT_OVERRIDE
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
    {
        return Ok(path);
    }
    if cfg!(test) {
        return Err("usage_backfill cache root not set for tests".to_string());
    }
    Ok(crate::app_paths::global_dir()?.join("usage"))
}

pub(crate) fn fingerprint(parts: &[(u64, u64)]) -> String {
    let mut s = String::new();
    for (i, (len, mtime)) in parts.iter().enumerate() {
        if i > 0 {
            s.push('|');
        }
        s.push_str(&len.to_string());
        s.push(':');
        s.push_str(&mtime.to_string());
    }
    s
}

/// `(length, mtime)` of one file; missing files contribute `(0, 0)`.
pub(crate) fn file_stamp(path: &Path) -> (u64, u64) {
    match fs::metadata(path) {
        Ok(meta) => {
            let len = meta.len();
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            (len, mtime)
        }
        Err(_) => (0, 0),
    }
}

/// Fingerprint a file plus optional satellites (SQLite `-wal`).
pub(crate) fn stamp_files(main: &Path, sats: &[&Path]) -> String {
    let mut parts = vec![file_stamp(main)];
    for sat in sats {
        parts.push(file_stamp(sat));
    }
    fingerprint(&parts)
}

pub(crate) fn load_at(path: &Path) -> Cache {
    let Ok(text) = fs::read_to_string(path) else {
        return Cache::default();
    };
    match serde_json::from_str(&text) {
        Ok(cache) => cache,
        Err(_) => Cache::default(), // fail open; the next scan rewrites it
    }
}

pub(crate) fn save_at(path: &Path, cache: &Cache) -> Result<(), String> {
    let body = serde_json::to_string(cache).map_err(|e| format!("serialize cache: {e}"))?;
    crate::storage::write_atomic(path, &body)
}

/// Drop entries whose key no longer matches a live file, so vanished sessions
/// stop resurrecting.
pub(crate) fn prune_to(cache: &mut Cache, live_keys: &[String]) {
    let live: std::collections::HashSet<&String> = live_keys.iter().collect();
    cache.entries.retain(|key, _| live.contains(key));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marionette-backfill-cache-test-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn cache_round_trip_and_corrupt_fail_open() {
        let dir = temp_dir();
        let path = dir.join("backfill-cache.json");
        let mut cache = Cache {
            version: 1,
            entries: BTreeMap::new(),
        };
        cache.entries.insert(
            "grok::/x/updates.jsonl".into(),
            CacheEntry {
                fingerprint: "123:456".into(),
                sessions: vec![SessionFacts {
                    agent: "grok-build".into(),
                    native_id: "n1".into(),
                    ..Default::default()
                }],
            },
        );
        save_at(&path, &cache).unwrap();
        let loaded = load_at(&path);
        assert_eq!(loaded.entries.len(), 1);
        assert_eq!(
            loaded.entries["grok::/x/updates.jsonl"].sessions[0].native_id,
            "n1"
        );
        // Corrupt file → empty cache, no panic.
        std::fs::write(&path, "{ not json").unwrap();
        assert!(load_at(&path).entries.is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fingerprint_changes_with_file_content() {
        let dir = temp_dir();
        let f = dir.join("a.jsonl");
        std::fs::write(&f, "hello").unwrap();
        let fp1 = stamp_files(&f, &[]);
        std::fs::write(&f, "hello world").unwrap();
        let fp2 = stamp_files(&f, &[]);
        assert_ne!(fp1, fp2);
        // Satellite (wal) changes also invalidate.
        let wal = dir.join("a.jsonl-wal");
        std::fs::write(&wal, "x").unwrap();
        let fp3 = stamp_files(&f, &[&wal]);
        std::fs::write(&wal, "xy").unwrap();
        let fp4 = stamp_files(&f, &[&wal]);
        assert_ne!(fp3, fp4);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn prune_drops_dead_keys() {
        let mut cache = Cache::default();
        cache.entries.insert("a".into(), CacheEntry::default());
        cache.entries.insert("b".into(), CacheEntry::default());
        prune_to(&mut cache, &["b".into()]);
        assert_eq!(cache.entries.len(), 1);
        assert!(cache.entries.contains_key("b"));
    }

    #[test]
    fn cache_root_refuses_real_dir_in_tests_without_override() {
        let prev = ROOT_OVERRIDE
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        set_root_override_for_tests(None);
        assert!(cache_root().is_err());
        set_root_override_for_tests(prev);
    }
}
