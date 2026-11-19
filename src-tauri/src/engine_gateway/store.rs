//! providers.json 存储——与 magpie 逐字段互通（docs/plans/plan-engine-contract.md
//! C6）。未知字段用 `serde(flatten)` 原样保留：我们重写文件时不丢 magpie
//! 认的任何东西。
//!
//! **读取可 fail-open，写回必须 fail-closed**：文件不存在是正常首次运行；
//! 文件存在但读不了/JSON 坏了时展示空表并给路由一个 503，但任何 CLI 编辑
//! 都必须拒绝保存。否则 `provider add` 会把损坏文件覆盖成只剩新项的空表。
//! 每次替换前保留最后一份成功读取/保存的 `providers.json.bak`，并检测本进程
//! 加载后磁盘是否已被外部改写，避免静默丢更新。
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Map;

/// providers.json 顶层（magpie internal/provider file struct）。
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ProvidersFile {
    #[serde(default)]
    pub providers: Vec<Provider>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<Group>,
    /// magpie 的 searches/noAutoGroups/autoGroupSkip/order 等字段原样保留。
    #[serde(flatten)]
    pub extra: Map<String, serde_json::Value>,
}

/// 一个来源。字段名与 magpie Provider 的 json tag 一一对应；`models: None`
/// = 未钉死（live 清单语义），`Some(list)` = 钉死。
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Provider {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub key: String,
    /// OpenAI 兼容 chat 基址（含 /v1）。
    #[serde(default)]
    pub chat: String,
    #[serde(default)]
    pub responses: String,
    #[serde(default)]
    pub anthropic: String,
    #[serde(default)]
    pub gemini: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub headers: HashMap<String, String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preset: String,
    /// models.dev 目录 id（名字/窗口兜底用）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub catalog: String,
    #[serde(flatten)]
    pub extra: Map<String, serde_json::Value>,
}

impl Provider {
    /// 出站用基址：chat 优先，退 responses / anthropic 的主机根。
    pub fn chat_base(&self) -> Option<&str> {
        for url in [&self.chat, &self.responses] {
            if !url.is_empty() {
                return Some(url.trim_end_matches('/'));
            }
        }
        if !self.anthropic.is_empty() {
            return Some(self.anthropic.trim_end_matches('/'));
        }
        None
    }
}

/// 路由组（magpie Group）。members 是 `provider/model` 形态；pick 是手动
/// 选中的成员。id/name/note/members/former/match 的 json tag 与 magpie
/// 一致，其余字段 flatten 保留。
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct Group {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default)]
    pub members: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub former: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pick: String,
    /// magpie 的 Routing（"manual"）；未知值 flatten 之外单存。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub routing: String,
    #[serde(flatten)]
    pub extra: Map<String, serde_json::Value>,
}

pub struct Store {
    path: PathBuf,
    file: ProvidersFile,
    /// 最近一次成功读取的原始字节。None = 加载时文件不存在。
    disk_raw: Option<Vec<u8>>,
    /// 当前表不能作为目录事实源：只用于 503 展示。
    read_blocked: Option<String>,
    /// 非空时当前内存只可用于展示，任何写回都会被拒绝。
    write_blocked: Option<String>,
}

impl Store {
    /// 从 config_dir() 读；没有目录/文件 → 空 store（首次运行）。
    pub fn load() -> Result<Store, String> {
        let dir = super::config_dir().ok_or("no config dir (set USERPROFILE)")?;
        Store::load_at(dir.join(PROVIDERS_FILE))
    }

    fn load_at(path: PathBuf) -> Result<Store, String> {
        let (file, disk_raw, read_blocked, write_blocked) = match read_file(&path) {
            Ok(Some((file, raw))) => {
                let blocked = ensure_backup(&path, &raw)
                    .err()
                    .map(|e| format!("cannot preserve providers.json backup: {e}"));
                (file, Some(raw), None, blocked)
            }
            Ok(None) => (ProvidersFile::default(), None, None, None),
            Err(error) => (
                ProvidersFile::default(),
                None,
                Some(error.clone()),
                Some(error),
            ),
        };
        Ok(Store { path, file, disk_raw, read_blocked, write_blocked })
    }

    /// 从磁盘重读（magpie heldFresh 同语义：跑着的网关在下一个请求看到
    /// CLI/别的进程写进 providers.json 的变更）。读失败保持旧内容。
    pub fn reload(&mut self) {
        match read_file(&self.path) {
            Ok(Some((file, raw))) => {
                self.file = file;
                self.disk_raw = Some(raw.clone());
                self.read_blocked = None;
                self.write_blocked = ensure_backup(&self.path, &raw)
                    .err()
                    .map(|e| format!("cannot preserve providers.json backup: {e}"));
            }
            Ok(None) if self.disk_raw.is_none() => {
                self.file = ProvidersFile::default();
                self.read_blocked = None;
                self.write_blocked = None;
            }
            Ok(None) => {
                let reason = super::localized(
                    "providers.json 在运行中消失；为保护旧配置已暂停写回".to_string(),
                    "providers.json disappeared while running; writes are paused to protect the old configuration".to_string(),
                );
                self.read_blocked = Some(reason.clone());
                self.write_blocked = Some(reason);
            }
            Err(error) => {
                self.read_blocked = Some(error.clone());
                self.write_blocked = Some(error);
            }
        }
    }

    pub fn read_blocked(&self) -> Option<&str> {
        self.read_blocked.as_deref()
    }

    /// 写回前的保护状态；`None` 才可编辑。
    pub fn write_blocked(&self) -> Option<&str> {
        self.write_blocked.as_deref()
    }

    /// 原子写（tmp + 平台替换；同进程同目录，tmp 名带 pid/序号防互吞）。
    /// 文件存在时先写 `.bak`，再替换；任一步失败都保留原文件。
    pub fn save(&self) -> Result<(), String> {
        if let Some(reason) = &self.write_blocked {
            return Err(format!("refusing to overwrite providers.json: {reason}"));
        }
        ensure_disk_unchanged(&self.path, self.disk_raw.as_deref())?;
        let body = serde_json::to_vec_pretty(&self.file).map_err(|e| e.to_string())?;
        if let Some(raw) = self.disk_raw.as_deref() {
            ensure_backup(&self.path, raw)?;
        }
        atomic_write(&self.path, &body)
    }

    pub fn find(&self, id: &str) -> Option<&Provider> {
        self.file.providers.iter().find(|p| p.id == id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut Provider> {
        self.file.providers.iter_mut().find(|p| p.id == id)
    }

    /// upsert：同 id 覆盖（engine.rs 注册链是幂等重放，不是报错）。
    pub fn upsert(&mut self, provider: Provider) {
        match self.file.providers.iter().position(|p| p.id == provider.id) {
            Some(i) => self.file.providers[i] = provider,
            None => self.file.providers.push(provider),
        }
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.file.providers.len();
        self.file.providers.retain(|p| p.id != id);
        self.file.providers.len() != before
    }

    pub fn providers(&self) -> &[Provider] {
        &self.file.providers
    }

    /// 组 upsert（按 id；`group add` 的 effort 重钉/改绑语义）。
    pub fn upsert_group(&mut self, group: Group) {
        match self.file.groups.iter().position(|g| g.id == group.id) {
            Some(i) => self.file.groups[i] = group,
            None => self.file.groups.push(group),
        }
    }

    pub fn remove_group(&mut self, id: &str) -> bool {
        let before = self.file.groups.len();
        self.file.groups.retain(|g| g.id != id);
        self.file.groups.len() != before
    }

    /// 按 id / name 找组；都没中再认改名前的旧 id（`former`，magpie
    /// RenameGroup 谱系：旧名仍路由到它，直到别的组占了这个 id）。
    pub fn group(&self, id: &str) -> Option<&Group> {
        self.file
            .groups
            .iter()
            .find(|g| g.id == id || g.name == id)
            .or_else(|| self.file.groups.iter().find(|g| g.former.iter().any(|f| f == id)))
    }

    pub fn groups(&self) -> &[Group] {
        &self.file.groups
    }
}

const PROVIDERS_FILE: &str = "providers.json";
const BACKUP_FILE: &str = "providers.json.bak";

fn decode(raw: &[u8]) -> Result<ProvidersFile, String> {
    let raw = raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(raw);
    serde_json::from_slice::<ProvidersFile>(raw).map_err(|e| e.to_string())
}

/// Ok(None) = 文件不存在；Err = 文件存在但读/解析失败。
fn read_file(path: &Path) -> Result<Option<(ProvidersFile, Vec<u8>)>, String> {
    match std::fs::read(path) {
        Ok(raw) => match decode(&raw) {
            Ok(file) => Ok(Some((file, raw))),
            Err(error) => Err(super::localized(
                format!("providers.json 无法解析（{}）: {error}；原文件保持不变", path.display()),
                format!("providers.json could not be parsed ({}): {error}; the original file was left untouched", path.display()),
            )),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(super::localized(
            format!("providers.json 无法读取（{}）: {error}；为避免覆盖已暂停写回", path.display()),
            format!("providers.json could not be read ({}): {error}; writes are paused to avoid overwriting it", path.display()),
        )),
    }
}

fn ensure_disk_unchanged(path: &Path, loaded: Option<&[u8]>) -> Result<(), String> {
    match (loaded, std::fs::read(path)) {
        (None, Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        (None, Ok(_)) => Err(super::localized(
            "providers.json 在加载后由另一进程创建；请重试当前操作以重新读取".to_string(),
            "providers.json was created by another process after loading; retry so it can be read again".to_string(),
        )),
        (Some(expected), Ok(current)) if current == expected => Ok(()),
        (Some(_), Ok(_)) => Err(super::localized(
            "providers.json 在加载后由另一进程修改；已拒绝覆盖，请重试".to_string(),
            "providers.json changed on disk after loading; refusing to overwrite it, please retry".to_string(),
        )),
        (Some(_), Err(error)) if error.kind() == std::io::ErrorKind::NotFound => Err(super::localized(
            "providers.json 在加载后消失；已拒绝用内存副本重建".to_string(),
            "providers.json disappeared after loading; refusing to recreate it from the in-memory copy".to_string(),
        )),
        (_, Err(error)) => Err(error.to_string()),
    }
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_file_name(BACKUP_FILE)
}

fn ensure_backup(path: &Path, raw: &[u8]) -> Result<(), String> {
    let backup = backup_path(path);
    if std::fs::read(&backup).ok().as_deref() == Some(raw) {
        return Ok(());
    }
    atomic_write(&backup, raw)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1);

    let parent = path.parent().ok_or_else(|| format!("no parent for {}", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = parent.join(format!(
        ".{}.{}.{}.tmp",
        path.file_name().and_then(|s| s.to_str()).unwrap_or("providers"),
        std::process::id(),
        seq
    ));
    let result = (|| -> Result<(), String> {
        let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        drop(file);
        replace_file(&tmp, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };
    let from: Vec<u16> = from.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = to.as_os_str().encode_wide().chain(Some(0)).collect();
    let ok = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 { Err(std::io::Error::last_os_error()) } else { Ok(()) }
}

#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::rename(from, to)
}

/// provider id slug（magpie provider_id_slug 同规则：小写字母/数字/横线）。
pub fn id_slug(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

    fn temp_path() -> PathBuf {
        let n = DIR_SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!("marionette-store-test-{}-{n}", std::process::id()))
            .join(PROVIDERS_FILE)
    }

    fn provider(id: &str, key: &str) -> Provider {
        Provider {
            id: id.into(),
            name: id.into(),
            key: key.into(),
            chat: "https://example.test/v1".into(),
            ..Default::default()
        }
    }

    #[test]
    fn missing_file_is_the_only_case_that_may_start_from_an_empty_table() {
        let path = temp_path();
        let mut store = Store::load_at(path.clone()).unwrap();
        assert!(store.providers().is_empty());
        assert!(store.write_blocked().is_none());
        store.upsert(provider("first", "k1"));
        store.save().unwrap();
        assert_eq!(decode(&std::fs::read(&path).unwrap()).unwrap().providers.len(), 1);
    }

    #[test]
    fn bom_is_read_and_the_previous_valid_file_is_kept_as_backup() {
        let path = temp_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let original = b"\xEF\xBB\xBF{\"providers\":[],\"custom\":{\"keep\":true}}";
        std::fs::write(&path, original).unwrap();
        let mut store = Store::load_at(path.clone()).unwrap();
        assert!(store.write_blocked().is_none());
        assert_eq!(std::fs::read(backup_path(&path)).unwrap(), original);
        store.upsert(provider("added", "secret"));
        store.save().unwrap();
        assert_eq!(std::fs::read(backup_path(&path)).unwrap(), original);
        let saved = decode(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.providers[0].id, "added");
        assert_eq!(saved.extra["custom"]["keep"], true, "unknown fields survive a write");
    }

    #[test]
    fn corrupt_or_unreadable_existing_file_can_never_be_overwritten() {
        let path = temp_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let corrupt = br#"{"providers":[{"id":"broken""#;
        std::fs::write(&path, corrupt).unwrap();
        let mut store = Store::load_at(path.clone()).unwrap();
        assert!(store.providers().is_empty());
        assert!(store.write_blocked().is_some());
        store.upsert(provider("new", "secret"));
        assert!(store.save().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), corrupt, "original bytes stay untouched");

        let unreadable = temp_path();
        std::fs::create_dir_all(&unreadable).unwrap();
        let mut store = Store::load_at(unreadable.clone()).unwrap();
        assert!(store.write_blocked().is_some());
        store.upsert(provider("new", "secret"));
        assert!(store.save().is_err());
        assert!(unreadable.is_dir(), "a path collision is not replaced by a file");
    }

    #[test]
    fn a_concurrent_writer_makes_the_stale_store_refuse_to_save() {
        let path = temp_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, br#"{"providers":[]}"#).unwrap();
        let mut store = Store::load_at(path.clone()).unwrap();
        std::fs::write(
            &path,
            br#"{"providers":[{"id":"external","name":"external","chat":"https://external.test/v1"}]}"#,
        )
        .unwrap();
        store.upsert(provider("stale", "secret"));
        let error = store.save().unwrap_err();
        assert!(
            error.contains("changed on disk") || error.contains("另一进程修改"),
            "{error}"
        );
        assert!(std::fs::read_to_string(&path).unwrap().contains("external"));
        assert!(!std::fs::read_to_string(&path).unwrap().contains("stale"));
    }

    #[test]
    fn reload_turns_a_newly_corrupt_file_into_read_only_state() {
        let path = temp_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, br#"{"providers":[]}"#).unwrap();
        let mut store = Store::load_at(path.clone()).unwrap();
        std::fs::write(&path, b"not json").unwrap();
        store.reload();
        assert!(store.write_blocked().is_some());
        store.upsert(provider("new", "secret"));
        assert!(store.save().is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"not json");
    }
}
