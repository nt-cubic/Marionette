//! 跨 agent 模型桥 — 凭据层（docs/cross-agent-bridge.md §4.1/§4.2/§5）。
//!
//! 本模块是桥的第一块地基，只做两件事：
//!
//! - **只读扫描**：发现 opencode / Claude Code / Codex / Grok 落在本机的 key 与
//!   登录态，按 §2 分类成 `key-plan` / `subscription` / `reference`。扫描绝不写
//!   任何文件（包括我们自己的 store）；住在别的 agent 文件里的 key 不复制第二份
//!   —— reference 条目只记 `文件路径#provider-id`，用时现场读原文件。
//! - **DPAPI 存储**：手动录入的 API key 落 `~/.marionette/credentials.json`。
//!   Windows 上用 DPAPI（user 作用域）加密成 `dpapi:<base64>`；临时兄弟文件 +
//!   rename 原子写（沿用 provider_usage::write_auth_atomic 的模式）；文件损坏时
//!   改名留档后重新初始化，绝不静默清空。
//!
//! 安全底线（§5）：密钥明文只存在于内存与 `peek_secret` 的返回值里 —— 不打日志、
//! 不进扫描结果、不复制进任何别的文件。本模块还不接 Tauri command（§4.7 是下一步）。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

// ─── 数据模型（§2 来源分类 / §4.2 存储 schema）──────────────────────────────

/// 凭据来源类型。kebab-case 序列化，与 §4.2 schema 里的
/// `key-plan | subscription | reference` 逐字对齐。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CredentialKind {
    /// API key 型，厂商同时提供 OpenAI / Anthropic 兼容端点 → L1 直连。
    KeyPlan,
    /// OAuth / 登录态，上游是私有端点 → L2 引擎。Marionette 只看存在性，不解读内容。
    Subscription,
    /// 住在别的 agent 自己文件里的凭证，不复制第二份，用时现场读原文件。
    Reference,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialEntry {
    /// 稳定 id：手动录入时是 preset 名，扫描生成的是 `agent/…` 形态。
    pub id: String,
    pub kind: CredentialKind,
    pub label: String,
    /// L1 preset 表（§4.3）的钥匙名；subscription / reference 无此字段。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// 存盘形态 `dpapi:<base64>`（非 Windows 测试降级为 `plain:<base64>`）。
    /// reference 型没有这个字段 —— 别人文件里的 key 不复制进来。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
    /// reference 型的 `文件路径#provider-id`。存储 schema 里键名固定为 `ref`（§4.2），
    /// 与结构体其余字段的 camelCase 并行。
    #[serde(rename = "ref", skip_serializing_if = "Option::is_none")]
    pub ref_path: Option<String>,
    /// 这条凭据宿主 agent 的 id（"opencode" / "claude" / "codex" / "grok"）。
    /// 扫描型条目必填；手动录入为 None。跨用目录（model_bridge）用它执行
    /// 「宿主 agent 自己看不到自己文件里的凭证」的去重规则（§4.8）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_agent: Option<String>,
    /// `scan:<origin>` | `manual`。
    pub source: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_provider: Option<CustomProviderConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomProviderConfig {
    pub endpoint: String,
    pub protocol: String,
    pub models: Vec<String>,
}

/// 存储文件 `~/.marionette/credentials.json` 的整体结构。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialStore {
    pub version: u32,
    #[serde(default)]
    pub entries: Vec<CredentialEntry>,
}

const STORE_VERSION: u32 = 1;

impl Default for CredentialStore {
    fn default() -> Self {
        Self {
            version: STORE_VERSION,
            entries: Vec::new(),
        }
    }
}

/// Unix epoch 秒 —— 与 provider_usage 的时间戳同款：不引时间库，前端自行格式化。
fn now_epoch_secs() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .to_string()
}

// ─── DPAPI（§4.2 / §5.1）────────────────────────────────────────────────────

/// Windows：CryptProtectData（user 作用域）。免主密码，同机同用户自动解密；
/// 文件被单独拷走时只是密文。
#[cfg(windows)]
fn protect_secret(plain: &str) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let bytes = plain.as_bytes();
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    // 只给 UI_FORBIDDEN：绝不弹 DPAPI 系统对话框；不设 LOCAL_MACHINE → user 作用域。
    let ok = unsafe {
        CryptProtectData(
            &input,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err("CryptProtectData 失败（DPAPI user 作用域加密）".to_string());
    }
    let cipher =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    // DPAPI 输出 blob 由调用方负责 LocalFree 释放；LocalFree 恰好在已启用的
    // Win32_Foundation feature 里，无需为它追加 feature。
    unsafe { LocalFree(output.pbData.cast()) };
    Ok(cipher)
}

#[cfg(windows)]
fn unprotect_secret(cipher: &[u8]) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: cipher.len() as u32,
        pbData: cipher.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err("CryptUnprotectData 失败（密文不属于当前用户或已损坏）".to_string());
    }
    let plain =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { LocalFree(output.pbData.cast()) };
    Ok(plain)
}

/// 非 Windows 的显式降级：无 DPAPI 就原样返回字节，由 `encrypt_stored` 加上
/// `plain:` 前缀。**仅测试用** —— Marionette 只发布 Windows 便携版，这条路径
/// 只是让代码在其他平台可编译、单测可跑；前缀保证它不会被误当成密文。
#[cfg(not(windows))]
fn protect_secret(plain: &str) -> Result<Vec<u8>, String> {
    Ok(plain.as_bytes().to_vec())
}

#[cfg(not(windows))]
fn unprotect_secret(cipher: &[u8]) -> Result<Vec<u8>, String> {
    Ok(cipher.to_vec())
}

/// 明文 → 存盘字符串（Windows：`dpapi:<base64>`）。
#[cfg(windows)]
fn encrypt_stored(plain: &str) -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    Ok(format!("dpapi:{}", B64.encode(protect_secret(plain)?)))
}

/// 非 Windows 的降级存盘：`plain:<base64>`（仅测试用，见 [`protect_secret`]）。
#[cfg(not(windows))]
fn encrypt_stored(plain: &str) -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    Ok(format!("plain:{}", B64.encode(protect_secret(plain)?)))
}

/// 存盘字符串 → 明文。前缀不认识、base64 损坏、解密失败一律返回错误，绝不 panic。
fn decrypt_stored(stored: &str) -> Result<String, String> {
    use base64::{engine::general_purpose::STANDARD as B64, Engine};
    let Some((prefix, payload)) = stored.split_once(':') else {
        return Err("无法识别的密文格式（缺少前缀）".to_string());
    };
    let raw = B64
        .decode(payload)
        .map_err(|e| format!("密文 base64 解码失败: {e}"))?;
    match prefix {
        // dpapi 密文只能在产生它的 Windows 用户上下文里解开；其他平台明确拒绝，
        // 而不是把密文字节当明文吐出去。
        "dpapi" => {
            if cfg!(windows) {
                let plain = unprotect_secret(&raw)?;
                String::from_utf8(plain).map_err(|_| "解密结果不是合法 UTF-8".to_string())
            } else {
                Err("dpapi: 前缀的密文只能在 Windows 上解开".to_string())
            }
        }
        "plain" => String::from_utf8(raw).map_err(|_| "内容不是合法 UTF-8".to_string()),
        other => Err(format!(
            "无法识别的密文前缀 `{other}`（期望 dpapi: 或 plain:）"
        )),
    }
}

/// crate 内通用密文出口：Telegram bot token 等非模型桥凭据同样走 DPAPI
/// （与 key-plan 同一套前缀格式；明文只存在于内存，绝不进日志）。
pub(crate) fn encrypt_secret(plain: &str) -> Result<String, String> {
    encrypt_stored(plain)
}

/// [`encrypt_secret`] 的逆操作。前缀不认识 / 解密失败一律报错。
pub(crate) fn decrypt_secret(stored: &str) -> Result<String, String> {
    decrypt_stored(stored)
}

// ─── DPAPI 存储：核心（路径由调用方注入，测试不碰真实用户目录）──────────────

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

/// 临时兄弟文件，唯一化到「每进程 × 每次调用」。
///
/// 固定一个 `credentials.json.tmp` 会让两次并发写（保存撞上删除）互相覆盖
/// 对方的暂存文件，把半合并的结果 rename 进盘 —— 与 provider_usage::temp_sibling
/// 同一个教训，同一个解法。
fn temp_sibling(path: &Path) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("credentials.json");
    path.with_file_name(format!("{name}.{}.{seq}.tmp", std::process::id()))
}

/// 原子写：先写临时兄弟文件，再 rename 顶上（镜像 write_auth_atomic）。
fn write_store_atomic(path: &Path, store: &CredentialStore) -> Result<(), String> {
    let body = serde_json::to_string_pretty(store)
        .map_err(|e| format!("序列化 credentials.json 失败: {e}"))?;
    let temp_path = temp_sibling(path);
    fs::write(&temp_path, format!("{body}\n")).map_err(|e| format!("写入临时文件失败: {e}"))?;
    if let Err(error) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(format!("重命名临时文件失败: {error}"));
    }
    Ok(())
}

/// 把损坏的 store 改名留档为 `credentials.corrupt-<ts>.json`。
///
/// 立场与 provider_usage 的 `corrupt_auth_json_aborts_instead_of_wiping` 一致：
/// 宁可把原始数据留档给用户手工救，也绝不静默清空。rename 本身失败时返回错误
/// 并让上层中止 —— 留不住档就更不能动原文件。
fn archive_corrupt_store(path: &Path) -> Result<PathBuf, String> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("credentials.json");
    let stem = name.strip_suffix(".json").unwrap_or(name);
    let mut candidate = path.with_file_name(format!("{stem}.corrupt-{ts}.json"));
    // 同一毫秒内两次损坏：加序号，绝不覆盖上一份留档。
    let mut n = 1u32;
    while candidate.exists() {
        candidate = path.with_file_name(format!("{stem}.corrupt-{ts}-{n}.json"));
        n += 1;
    }
    fs::rename(path, &candidate)
        .map_err(|e| format!("credentials.json 已损坏且留档失败（{e}）— 为避免覆盖原始数据已中止"))?;
    Ok(candidate)
}

/// 读取 store。文件不存在 → 全新空 store；解析失败 / 版本不认识 → 先留档再
/// 返回空 store（「重新初始化」），数据永远躺在留档文件里。
pub fn load_store_at(path: &Path) -> Result<CredentialStore, String> {
    if !path.is_file() {
        return Ok(CredentialStore::default());
    }
    let text = fs::read_to_string(path).map_err(|e| format!("读取 credentials.json 失败: {e}"))?;
    if text.trim().is_empty() {
        // 空文件是合法的全新开始，不是损坏（与 provider_usage 对空 auth.json 的立场一致）。
        return Ok(CredentialStore::default());
    }
    let store = serde_json::from_str::<CredentialStore>(&text);
    match store {
        Ok(store) if store.version == STORE_VERSION => Ok(store),
        Ok(store) => {
            // 认不出的版本（未来的 schema）按损坏处理：解释不了就不能假装读到了。
            let _ = store;
            archive_corrupt_store(path)?;
            Ok(CredentialStore::default())
        }
        Err(_) => {
            archive_corrupt_store(path)?;
            Ok(CredentialStore::default())
        }
    }
}

/// 保存 store（原子写；父目录缺失时创建）。
pub fn save_store_at(path: &Path, store: &CredentialStore) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("创建 .marionette 目录失败: {e}"))?;
    }
    write_store_atomic(path, store)
}

/// 录入 / 更新一个 key 型 plan：id = preset（§4.2「稳定 id，preset 或自动生成」）。
/// 同 id 重新录入视为换 key —— 更新密钥与标签，保留创建时间。
pub fn add_key_plan_at(path: &Path, preset: &str, label: &str, key: &str) -> Result<(), String> {
    let preset = preset.trim();
    if preset.is_empty() {
        return Err("preset 不能为空（它同时是这条凭证的稳定 id）".to_string());
    }
    if !preset
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("preset 只能用字母、数字、连字符、下划线".to_string());
    }
    // 粘贴进来的 key 常带首尾换行；厂商 key 没有首尾空白语义，trim 掉。
    let key = key.trim();
    if key.is_empty() {
        return Err("API Key 不能为空".to_string());
    }
    let secret = encrypt_stored(key).map_err(|e| format!("加密 API Key 失败: {e}"))?;
    let label = if label.trim().is_empty() {
        preset.to_string()
    } else {
        label.trim().to_string()
    };

    let mut store = load_store_at(path)?;
    if let Some(existing) = store.entries.iter_mut().find(|e| e.id == preset) {
        existing.kind = CredentialKind::KeyPlan;
        existing.label = label;
        existing.preset = Some(preset.to_string());
        existing.secret = Some(secret);
        existing.ref_path = None;
        existing.host_agent = None; // 手动录入的 key 不住在任何 agent 的文件里。
        existing.source = "manual".to_string();
        existing.last_used_at = None; // 旧 key 的使用记录对新 key 没有意义。
    } else {
        store.entries.push(CredentialEntry {
            custom_provider: None,
            id: preset.to_string(),
            kind: CredentialKind::KeyPlan,
            label,
            preset: Some(preset.to_string()),
            secret: Some(secret),
            ref_path: None,
            host_agent: None,
            source: "manual".to_string(),
            created_at: now_epoch_secs(),
            last_used_at: None,
        });
    }
    save_store_at(path, &store)
}

pub fn add_custom_provider(label: &str, key: &str, config: CustomProviderConfig) -> Result<String, String> {
    let path=credentials_store_path().ok_or_else(||"无法定位用户目录".to_string())?;
    add_custom_provider_at(&path,label,key,config)
}

fn add_custom_provider_at(path: &Path, label: &str, key: &str, config: CustomProviderConfig) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let label=label.trim();
    if label.is_empty() { return Err("请输入自定义来源名称".into()); }
    if !matches!(config.protocol.as_str(), "chat" | "responses" | "anthropic") { return Err("请选择支持的接口协议".into()); }
    let endpoint=config.endpoint.trim().trim_end_matches('/');
    let authority=endpoint.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') || endpoint.contains('?') || endpoint.contains('#') || !(endpoint.starts_with("https://") || endpoint.starts_with("http://")) {
        return Err("请输入有效的 http/https 接口地址，认证信息请填在 API Key 中".into());
    }
    let mut models:Vec<String>=config.models.iter().map(|m|m.trim().to_string()).filter(|m|!m.is_empty()).collect();
    models.dedup();
    if models.iter().any(|m|m.contains(',') || m.chars().any(char::is_control)) { return Err("模型 ID 不能包含逗号或控制字符".into()); }
    let digest=Sha256::digest(format!("{label}|{endpoint}|{}",config.protocol));
    let id=format!("custom-{digest:x}")[..23].to_string();
    let mut store=load_store_at(&path)?;
    let created_at=store.entries.iter().find(|e|e.id==id).map(|e|e.created_at.clone()).unwrap_or_else(now_epoch_secs);
    let secret=if key.trim().is_empty() {None} else {Some(encrypt_stored(key.trim())?)};
    store.entries.retain(|e|e.id!=id);
    store.entries.push(CredentialEntry { id:id.clone(),kind:CredentialKind::KeyPlan,label:label.into(),preset:Some("custom".into()),secret,ref_path:None,host_agent:None,source:"manual".into(),created_at,last_used_at:None,
        custom_provider:Some(CustomProviderConfig {endpoint:endpoint.into(),protocol:config.protocol,models}) });
    save_store_at(&path,&store)?;
    Ok(id)
}

/// 删除一条凭证。id 不存在时报错而不是假装成功（对齐 delete_provider_key）。
pub fn delete_entry_at(path: &Path, id: &str) -> Result<(), String> {
    let mut store = load_store_at(path)?;
    let before = store.entries.len();
    store.entries.retain(|e| e.id != id);
    if store.entries.len() == before {
        return Err(format!("未找到凭证 `{id}`"));
    }
    save_store_at(path, &store)
}

/// 解密并返回某条凭证的明文，供未来的注入层使用。
///
/// 结果只进内存、绝不打日志（§5.3：日志只记事件不记值）。reference 型没有
/// secret，返回 `Ok(None)`。此 `_at` 变体保留错误信息（解密失败与「不存在」
/// 是两回事）；默认路径包装 [`peek_secret`] 按规格收 flat 成 `Option`。
pub fn peek_secret_at(path: &Path, id: &str) -> Result<Option<String>, String> {
    let store = load_store_at(path)?;
    let Some(entry) = store.entries.iter().find(|e| e.id == id) else {
        return Ok(None);
    };
    let Some(stored) = entry.secret.as_deref() else {
        return Ok(None);
    };
    Ok(Some(decrypt_stored(stored)?))
}

/// 记录「这条凭证刚被用过」。写入 store 是存储层的正常职责 —— 纯读约束只针对扫描。
pub fn touch_last_used_at(path: &Path, id: &str) -> Result<(), String> {
    let mut store = load_store_at(path)?;
    let entry = store
        .entries
        .iter_mut()
        .find(|e| e.id == id)
        .ok_or_else(|| format!("未找到凭证 `{id}`"))?;
    entry.last_used_at = Some(now_epoch_secs());
    save_store_at(path, &store)
}

// ─── 只读扫描（§4.1 扫描清单）───────────────────────────────────────────────

/// 一家 agent 的扫描结果。`note` 是预期状态（缺失 / 受限），`error` 是扫描本身
/// 失败（文件在但读不了 / 解析不了）—— 两者分开，UI 才不会把「没装」画成「坏了」。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentScan {
    /// "opencode" | "claude" | "codex" | "grok"
    pub agent: String,
    pub entries: Vec<CredentialEntry>,
    pub note: Option<String>,
    pub error: Option<String>,
}

impl AgentScan {
    fn new(agent: &str) -> Self {
        Self {
            agent: agent.to_string(),
            entries: Vec::new(),
            note: None,
            error: None,
        }
    }

    /// 扫描本身失败（区别于 note 的「预期缺失」）。
    fn failed(agent: &str, reason: &str) -> Self {
        Self {
            agent: agent.to_string(),
            entries: Vec::new(),
            note: None,
            error: Some(reason.to_string()),
        }
    }
}

/// 四家扫描的汇总。一家失败不影响其他家：每家把错误装进自己的 `AgentScan::error`。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub opencode: AgentScan,
    pub claude: AgentScan,
    pub codex: AgentScan,
    pub grok: AgentScan,
}

fn absent(agent: &str, note: &str) -> AgentScan {
    AgentScan {
        agent: agent.to_string(),
        entries: Vec::new(),
        note: Some(note.to_string()),
        error: None,
    }
}

/// 扫描条目：不带 secret / preset / lastUsedAt —— 扫描结果只描述「有什么、在哪」，
/// 永远不含值本身。`host_agent` = 这条凭据住在哪个 agent 的文件里。
fn scan_entry(
    id: &str,
    kind: CredentialKind,
    label: &str,
    source: &str,
    ref_path: Option<String>,
    host_agent: &str,
) -> CredentialEntry {
    CredentialEntry {
        custom_provider: None,
        id: id.to_string(),
        kind,
        label: label.to_string(),
        preset: None,
        secret: None,
        ref_path,
        host_agent: Some(host_agent.to_string()),
        source: source.to_string(),
        created_at: now_epoch_secs(),
        last_used_at: None,
    }
}

/// 分类 auth.json 里的一个条目。
///
/// 语义对齐 provider_usage::classify_entry（它在别的模块里是私有的，这里按同规则
/// 重述）：OAuth 优先于 api —— refresh token 丢了不可恢复，混合条目宁可当 OAuth
/// 展示。没有 type 的旧条目按字段推断（refresh/access → OAuth；key → api）。
enum OpencodeAuthType {
    Api,
    OAuth,
    Unknown,
}

fn classify_opencode_entry(entry: &Value) -> OpencodeAuthType {
    let non_empty = |field: &str| {
        entry
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty())
    };
    if entry.get("type").and_then(Value::as_str) == Some("oauth")
        || non_empty("refresh")
        || non_empty("access")
    {
        return OpencodeAuthType::OAuth;
    }
    if non_empty("key") {
        return OpencodeAuthType::Api;
    }
    OpencodeAuthType::Unknown
}

/// 引擎注册专用：读出一份 auth.json 里 api 型条目的 (provider-id, 明文 key)。
///
/// 这是全仓库唯一把扫描引用的 key **值**读出来的地方——2026-10-16 产品定稿
/// 「无脑打开软件就能用」：跨用引擎注册需要 key 进 magpie CLI 的命令行。
/// 除此之外的扫描路径仍然绝不复制/存储 key（`scan_entry` 的 secret 恒为
/// None 不变）；解析失败按无 key 处理（空表），绝不 panic。
pub fn peek_opencode_api_keys_at(path: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(text) = fs::read_to_string(path) else {
        return out;
    };
    let Ok(auth) = serde_json::from_str::<Value>(&text) else {
        return out;
    };
    let Some(obj) = auth.as_object() else {
        return out;
    };
    for (provider, entry) in obj {
        if !matches!(classify_opencode_entry(entry), OpencodeAuthType::Api) {
            continue;
        }
        let Some(key) = entry
            .get("key")
            .and_then(Value::as_str)
            .filter(|key| !key.is_empty())
        else {
            continue;
        };
        out.push((provider.clone(), key.to_string()));
    }
    out
}

/// opencode 新版凭据库（2026-10 起）：`opencode.db` 的 `credential` 表。
/// `integration_id` 对应旧 auth.json 的键名；`value` 是 `{"type":"api","key":…}`
/// JSON；同一 integration 可有多行，`active=1` 是活钥匙、`active=0` 是被替换的
/// 旧钥匙（auth.json 里躺的就是这种）、NULL 是迁移拷贝。迁移完成后 TUI 只写
/// 库不写 auth.json（2026-10-23 实锤：本机 auth.json 停在 09-02，库里
/// opencode-go 已是 10-13 的新钥匙——引擎拿 auth.json 的旧钥匙查 Go 额度
/// 401，换库里的活钥匙立即 200）。只读打开（WAL 下与 TUI 并发安全），
/// 任何失败按空表处理。输出顺序：active=1 在前、NULL 在后（同组保持表序）
/// ——下游 registration_plan 先到先得，活钥匙要排前面。
fn peek_opencode_db_keys_at(db_path: &Path) -> Vec<(String, String)> {
    use rusqlite::{Connection, OpenFlags};
    let Ok(conn) = Connection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return Vec::new();
    };
    let mut stmt = match conn.prepare(
        "SELECT integration_id, value, active FROM credential WHERE active IS NOT 0",
    ) {
        Ok(stmt) => stmt,
        Err(_) => return Vec::new(),
    };
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0).unwrap_or_default(),
                row.get::<_, Option<String>>(1).ok().flatten().unwrap_or_default(),
                row.get::<_, Option<i64>>(2).ok().flatten().unwrap_or(-1),
            ))
        })
        .map(|rows| rows.flatten().collect::<Vec<_>>())
        .unwrap_or_default();
    let mut out: Vec<(bool, String, String)> = rows
        .into_iter()
        .filter_map(|(integration, value, active)| {
            // OAuth 型条目（value 是令牌包）不进 key 注册；只收 api 型。
            let parsed: serde_json::Value = serde_json::from_str(&value).ok()?;
            if parsed.get("type").and_then(|t| t.as_str()) != Some("api") {
                return None;
            }
            let key = parsed.get("key")?.as_str()?.trim().to_string();
            if key.is_empty() {
                return None;
            }
            Some((active == 1, integration, key))
        })
        .collect();
    out.sort_by_key(|(live, _, _)| !*live);
    out.into_iter().map(|(_, i, k)| (i, k)).collect()
}

/// 合并新旧两个凭据源：库里有钥匙的 integration 以库为准（活钥匙覆盖
/// auth.json 的同名旧条目），库里没有的 integration 保留 auth.json 兜底
/// （老版本 opencode / 手写场景）。库条目整体排在 auth 兜底前。
fn merge_opencode_key_sources(
    db: Vec<(String, String)>,
    auth: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let owned: std::collections::HashSet<String> = db
        .iter()
        .map(|(id, _)| id.to_ascii_lowercase())
        .collect();
    let mut out = db;
    out.extend(
        auth.into_iter()
            .filter(|(id, _)| !owned.contains(&id.to_ascii_lowercase())),
    );
    out
}

/// 真实路径版（路径探测与扫描一致：.local/share → .config → %APPDATA%）。
/// auth.json 之上叠加 opencode.db 的活钥匙（新版 opencode 的凭据正位，
/// 见 [`peek_opencode_db_keys_at`]）。
pub fn peek_opencode_api_keys() -> Vec<(String, String)> {
    let Some(auth_path) = crate::provider_usage::opencode_auth_path() else {
        return Vec::new();
    };
    let auth = peek_opencode_api_keys_at(&auth_path);
    let db = auth_path
        .parent()
        .map(|dir| peek_opencode_db_keys_at(&dir.join("opencode.db")))
        .unwrap_or_default();
    merge_opencode_key_sources(db, auth)
}

/// ZCode 桌面版「个人 provider 层」（`~/.zcode/v2/provider_config.json`，
/// `ZCODE_HOME` 可挪根，语义同 parsers/zcode.rs）里的 Coding Plan 钥匙。
/// 钥匙在 `providerRules[].config.access`：`type` = `zhipu-coding-plan-api-key`
/// + `apiKey`——**不在** v2/config.json 的 provider 条目里（那里 options.apiKey
/// 恒为空；2026-10-23 实测：设置页配了钥匙、config.json 仍空、钥匙本体在
/// 个人层）。该文件同时是跨用桥 model_bridge 的托管位——这里只读不写。
/// 返回 (preset-id, key)：智谱 Coding Plan → glm（open.bigmodel.cn 的 coding
/// 端点族，`magpie_provider_for_preset("glm")` 直达）。
pub fn peek_zcode_coding_plan_keys() -> Vec<(String, String)> {
    let Some(root) = crate::parsers::util::env_dir("ZCODE_HOME")
        .or_else(|| crate::parsers::util::home_join(".zcode"))
    else {
        return Vec::new();
    };
    peek_zcode_coding_plan_keys_at(&root.join("v2").join("provider_config.json"))
}

fn peek_zcode_coding_plan_keys_at(path: &Path) -> Vec<(String, String)> {
    let Ok(text) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(rules) = value
        .pointer("/config/providerConfigRules/providerRules")
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    rules
        .iter()
        .filter_map(|rule| {
            let access = rule.pointer("/config/access")?;
            let kind = access.get("type").and_then(serde_json::Value::as_str)?;
            let key = access
                .get("apiKey")
                .and_then(serde_json::Value::as_str)?
                .trim()
                .to_string();
            if key.is_empty() {
                return None;
            }
            match kind {
                "zhipu-coding-plan-api-key" => Some(("glm".to_string(), key)),
                _ => None,
            }
        })
        .collect()
}

/// 解析一份 opencode auth.json。解析失败 → `error`，绝不 panic、绝不猜半截文件。
fn scan_opencode_auth(path: &Path) -> AgentScan {
    let mut scan = AgentScan::new("opencode");
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            scan.error = Some(format!("读取 OpenCode auth.json 失败: {e}"));
            return scan;
        }
    };
    let auth: Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(e) => {
            scan.error = Some(format!("OpenCode auth.json 解析失败（{e}）"));
            return scan;
        }
    };
    let Some(obj) = auth.as_object() else {
        scan.error = Some("OpenCode auth.json 顶层不是 JSON 对象".to_string());
        return scan;
    };
    for (provider, entry) in obj {
        match classify_opencode_entry(entry) {
            // key 留在原文件里，我们只记「路径#provider-id」（nothing is stored twice）。
            OpencodeAuthType::Api => scan.entries.push(scan_entry(
                &format!("opencode/{provider}"),
                CredentialKind::Reference,
                provider,
                "scan:opencode",
                Some(format!("{}#{provider}", path.display())),
                "opencode",
            )),
            // OAuth 登录态：仅展示存在性，内容只属于未来的 L2 引擎。
            OpencodeAuthType::OAuth => scan.entries.push(scan_entry(
                &format!("opencode/{provider}"),
                CredentialKind::Subscription,
                provider,
                "scan:opencode",
                None,
                "opencode",
            )),
            OpencodeAuthType::Unknown => {}
        }
    }
    scan
}

/// 测试 / 注入用：给定假 home 扫 OpenCode。
/// 候选顺序与 provider_usage::opencode_auth_path 保持一致（.local/share → .config）；
/// 真实机器上的探测（含 %APPDATA%）走 [`scan_opencode`]，那里直接复用它。
pub fn scan_opencode_at(home: &Path) -> AgentScan {
    let candidates = [
        home.join(".local").join("share").join("opencode").join("auth.json"),
        home.join(".config").join("opencode").join("auth.json"),
    ];
    match candidates.iter().find(|p| p.is_file()) {
        Some(path) => scan_opencode_auth(path),
        None => absent("opencode", "未检测到 OpenCode（auth.json 不存在）"),
    }
}

pub fn scan_claude_at(home: &Path) -> AgentScan {
    let mut scan = AgentScan::new("claude");
    let credentials = home.join(".claude").join(".credentials.json");
    let state = home.join(".claude.json");
    if credentials.is_file() {
        // 只看存在性：OAuth token 的内容只交给未来的 L2 引擎，Marionette 不解读（§2）。
        scan.entries.push(scan_entry(
            "claude/login",
            CredentialKind::Subscription,
            "Claude Code 登录",
            "scan:claude",
            None,
            "claude",
        ));
    }
    if scan.entries.is_empty() {
        scan.note = Some(if state.is_file() {
            "检测到 ~/.claude.json（Claude Code 状态文件），但未找到 ~/.claude/.credentials.json".to_string()
        } else {
            "未检测到 Claude Code".to_string()
        });
    } else if state.is_file() {
        scan.note = Some("检测到 ~/.claude.json（Claude Code 状态文件）".to_string());
    }
    scan
}

pub fn scan_codex_at(home: &Path) -> AgentScan {
    let codex_dir = home.join(".codex");
    let auth = codex_dir.join("auth.json");
    if auth.is_file() {
        // 只看存在性，不读内容（auth.json 里是 OAuth token，属于 L2 引擎）。
        let mut scan = AgentScan::new("codex");
        scan.entries.push(scan_entry(
            "codex/login",
            CredentialKind::Subscription,
            "Codex 登录",
            "scan:codex",
            None,
            "codex",
        ));
        return scan;
    }
    if codex_dir.is_dir() {
        // Windows 默认 secret_auth_storage：凭证在系统 secret store 里，读不到也
        // 不破解（§9 风险表），只留一条受限提示。
        let mut scan = AgentScan::new("codex");
        scan.entries.push(scan_entry(
            "codex/login",
            CredentialKind::Subscription,
            "Codex 登录（凭证在系统 secret storage，无法读取）",
            "scan:codex",
            None,
            "codex",
        ));
        return scan;
    }
    absent("codex", "未检测到 Codex")
}

/// 从 config.toml 文本里列出 `[model."…"]` 自定义 channel 名。
///
/// 刻意不引 toml 依赖、也不做完整 parse：这里只需要表头里的一个名字，而用户的
/// config.toml 可能包含我们不认识的语法，严格 parse 会在无关行上误报失败。
/// 表头行以 `[` 开头、到第一个 `]` 结束 —— 对 grok 的真实形态足够。
fn grok_config_channels(text: &str) -> Vec<String> {
    let mut names = std::collections::BTreeSet::new();
    for raw_line in text.lines() {
        let line = raw_line.trim();
        // `[[…]]` 数组表不是 grok channel 的形态，跳过。
        if !line.starts_with('[') || line.starts_with("[[") {
            continue;
        }
        let Some(end) = line.find(']') else { continue };
        let header = line[1..end].trim();
        // 只认 `model.<name>`；`models`、`[model]` 本体、其他表都不算 channel。
        let Some(rest) = header.strip_prefix("model") else { continue };
        let Some(key_part) = rest.strip_prefix('.') else { continue };
        if let Some(name) = parse_toml_key_name(key_part) {
            if !name.is_empty() {
                names.insert(name);
            }
        }
    }
    names.into_iter().collect()
}

/// 解析 TOML dotted key 的第一段：`"quoted"`（含 `\"` 转义）、`'literal'` 或裸 key。
fn parse_toml_key_name(part: &str) -> Option<String> {
    if let Some(quoted) = part.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = quoted.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => out.push(chars.next()?), // \"→"、\\→\，其余转义原样保留。
                '"' => return Some(out),
                _ => out.push(c),
            }
        }
        None // 没有闭合引号 —— 不是合法表头，跳过。
    } else if let Some(literal) = part.strip_prefix('\'') {
        literal.split_once('\'').map(|(name, _)| name.to_string())
    } else {
        part.split('.').next().map(|bare| bare.trim().to_string())
    }
}

pub fn scan_grok_at(home: &Path) -> AgentScan {
    let mut scan = AgentScan::new("grok");
    let grok_dir = home.join(".grok");
    let auth_path = grok_dir.join("auth.json");
    let config_path = grok_dir.join("config.toml");

    if auth_path.is_file() {
        // 只取 email 用于显示；key / expires_at 不读不存（§4.1 扫描清单）。
        let parsed = fs::read_to_string(&auth_path)
            .map_err(|e| format!("读取 Grok auth.json 失败: {e}"))
            .and_then(|text| {
                serde_json::from_str::<Value>(&text)
                    .map_err(|e| format!("Grok auth.json 解析失败（{e}）"))
            });
        match parsed {
            Ok(auth) => {
                let email = auth
                    .get("email")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty());
                let label = match email {
                    Some(email) => format!("Grok 订阅（{email}）"),
                    None => "Grok 订阅".to_string(),
                };
                scan.entries.push(scan_entry(
                    "grok/login",
                    CredentialKind::Subscription,
                    &label,
                    "scan:grok",
                    None,
                    "grok",
                ));
            }
            Err(error) => scan.error = Some(error),
        }
    }

    if config_path.is_file() {
        match fs::read_to_string(&config_path) {
            Ok(text) => {
                for name in grok_config_channels(&text) {
                    scan.entries.push(scan_entry(
                        &format!("grok/channel/{name}"),
                        CredentialKind::Reference,
                        &format!("Grok 自定义 channel {name}"),
                        "scan:grok",
                        Some(format!("{}#model.\"{name}\"", config_path.display())),
                        "grok",
                    ));
                }
            }
            Err(e) => scan.error = Some(format!("读取 Grok config.toml 失败: {e}")),
        }
    }

    if scan.entries.is_empty() && scan.error.is_none() {
        scan.note = Some("未检测到 Grok".to_string());
    }
    scan
}

/// 汇总四家扫描。纯读：这里（以及上面的每个扫描器）不写任何文件。
pub fn scan_all_at(home: &Path) -> ScanReport {
    ScanReport {
        opencode: scan_opencode_at(home),
        claude: scan_claude_at(home),
        codex: scan_codex_at(home),
        grok: scan_grok_at(home),
    }
}

// ─── 默认路径包装（真实家目录 / ~/.marionette/credentials.json）──────────────
// 命令层（commands.rs）与注入层（acp.rs / model_bridge.rs）调用这一层。
// 单测全部走上面的 *_at 注入路径，绝不写真实用户目录；还没有调用者的包装
// 保留 allow(dead_code)，接上后即可删掉属性。

fn credentials_store_path() -> Option<PathBuf> {
    Some(home_dir()?.join(".marionette").join("credentials.json"))
}

#[allow(dead_code)]
pub fn load_store() -> Result<CredentialStore, String> {
    let path = credentials_store_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    load_store_at(&path)
}

#[allow(dead_code)]
pub fn save_store(store: &CredentialStore) -> Result<(), String> {
    let path = credentials_store_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    save_store_at(&path, store)
}

#[allow(dead_code)]
pub fn add_key_plan(preset: &str, label: &str, key: &str) -> Result<(), String> {
    let path = credentials_store_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    add_key_plan_at(&path, preset, label, key)
}

/// 删除一条凭证（bridge_delete_entry 走这里）。id 不存在时报错。
pub fn delete_entry(id: &str) -> Result<(), String> {
    let path = credentials_store_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    delete_entry_at(&path, id)
}

/// 解密后供注入层用（§5.3：绝不打日志）。
/// 错误按规格收 flat 成 `None`；需要区分「解密失败」与「不存在」时用 [`peek_secret_at`]。
/// 注入层（acp.rs spawn env）现在统一走 `model_bridge::peek_spawn_key`（key-plan
/// 与 reference 双来源），它直接调 [`peek_secret_at`]；这个默认路径包装暂无
/// 调用者，保留给未来的单条 reveal 类命令。
#[allow(dead_code)]
pub fn peek_secret(id: &str) -> Option<String> {
    let path = credentials_store_path()?;
    peek_secret_at(&path, id).ok().flatten()
}

#[allow(dead_code)]
pub fn touch_last_used(id: &str) -> Result<(), String> {
    let path = credentials_store_path().ok_or_else(|| "无法定位用户目录".to_string())?;
    touch_last_used_at(&path, id)
}

/// 真实路径的 opencode 扫描（含 %APPDATA% fallback；模型桥的 list_models /
/// 注入解析在默认路径上走这里）。纯读。
pub fn scan_opencode() -> AgentScan {
    // 真实路径探测完全复用 provider_usage 的多路径逻辑（.local/share → .config →
    // %APPDATA%），不在本模块复制第二份。
    match crate::provider_usage::opencode_auth_path() {
        Some(path) => scan_opencode_auth(&path),
        None => absent("opencode", "未检测到 OpenCode（auth.json 不存在）"),
    }
}

#[allow(dead_code)]
pub fn scan_claude() -> AgentScan {
    match home_dir() {
        Some(home) => scan_claude_at(&home),
        None => AgentScan::failed("claude", "无法定位用户目录"),
    }
}

#[allow(dead_code)]
pub fn scan_codex() -> AgentScan {
    match home_dir() {
        Some(home) => scan_codex_at(&home),
        None => AgentScan::failed("codex", "无法定位用户目录"),
    }
}

#[allow(dead_code)]
pub fn scan_grok() -> AgentScan {
    match home_dir() {
        Some(home) => scan_grok_at(&home),
        None => AgentScan::failed("grok", "无法定位用户目录"),
    }
}

/// 汇总四家扫描（bridge_scan 命令透传这一份）。纯读：不写任何文件。
pub fn scan_all() -> ScanReport {
    match home_dir() {
        Some(home) => scan_all_at(&home),
        None => ScanReport {
            opencode: AgentScan::failed("opencode", "无法定位用户目录"),
            claude: AgentScan::failed("claude", "无法定位用户目录"),
            codex: AgentScan::failed("codex", "无法定位用户目录"),
            grok: AgentScan::failed("grok", "无法定位用户目录"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        add_key_plan_at, decrypt_stored, delete_entry_at, encrypt_stored, load_store_at,
        merge_opencode_key_sources, peek_opencode_api_keys_at, peek_opencode_db_keys_at,
        peek_secret_at, peek_zcode_coding_plan_keys_at, scan_all_at, scan_claude_at,
        scan_codex_at, scan_grok_at, scan_opencode_at, touch_last_used_at, CredentialEntry,
        CredentialKind,
    };
    use std::fs;
    use std::path::{Path, PathBuf};

    /// 每个测试自建假 home / store 目录，绝不碰真实用户目录（仓库测试惯例：手动临时目录）。
    fn temp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marionette_test_credentials_{tag}_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_file(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    /// 清理（不复用 temp_home，那会先重建目录）。
    fn remove_home(tag: &str) {
        let dir = std::env::temp_dir().join(format!(
            "marionette_test_credentials_{tag}_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
    }

    fn find_entry<'a>(entries: &'a [CredentialEntry], id: &str) -> &'a CredentialEntry {
        entries.iter().find(|e| e.id == id).unwrap()
    }

    #[test]
    fn custom_provider_roundtrip_rotation_and_deletion() {
        let root=temp_home("custom_provider");
        let path=root.join("credentials.json");
        let config=super::CustomProviderConfig {endpoint:"https://ollama.com/v1/".into(),protocol:"chat".into(),models:vec!["deepseek-v4.1-flash:1017".into()]};
        let id=super::add_custom_provider_at(&path,"Ollama","sk-fixture-private",config.clone()).unwrap();
        let stored=super::load_store_at(&path).unwrap();
        assert_eq!(stored.entries.len(),1);
        assert_eq!(stored.entries[0].custom_provider.as_ref().unwrap().endpoint,"https://ollama.com/v1");
        assert!(!fs::read_to_string(&path).unwrap().contains("sk-fixture-private"));
        assert_eq!(super::peek_secret_at(&path,&id).unwrap().as_deref(),Some("sk-fixture-private"));
        let rotated=super::add_custom_provider_at(&path,"Ollama","sk-fixture-rotated",config).unwrap();
        assert_eq!(id,rotated);
        assert_eq!(super::load_store_at(&path).unwrap().entries.len(),1);
        assert_eq!(super::peek_secret_at(&path,&id).unwrap().as_deref(),Some("sk-fixture-rotated"));
        super::delete_entry_at(&path,&id).unwrap();
        assert!(super::load_store_at(&path).unwrap().entries.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    fn corrupt_archives(dir: &Path) -> Vec<PathBuf> {
        let mut out: Vec<PathBuf> = fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_str()
                    .is_some_and(|n| n.starts_with("credentials.corrupt-"))
            })
            .map(|e| e.path())
            .collect();
        out.sort();
        out
    }

    // ─── DPAPI 往返 ─────────────────────────────────────────────────────────

    /// roundtrip 后明文一致；密文带平台前缀，且（Windows 上）不含明文子串 ——
    /// 这是对 §5.1「文件被拷走是密文」的最小验证。
    #[test]
    fn secret_roundtrip_matches_plaintext_and_ciphertext_carries_prefix() {
        let plain = "sk-roundtrip-4f9a2c";
        let stored = encrypt_stored(plain).unwrap();
        let expected_prefix = if cfg!(windows) { "dpapi:" } else { "plain:" };
        assert!(stored.starts_with(expected_prefix), "stored = {stored}");
        if cfg!(windows) {
            assert!(!stored.contains(plain), "密文里不能出现明文子串");
        }
        assert_eq!(decrypt_stored(&stored).unwrap(), plain);
    }

    #[test]
    fn rejects_unrecognized_cipher_prefixes() {
        assert!(decrypt_stored("根本不是密文").is_err());
        assert!(decrypt_stored("xxx:QUJD").is_err());
        // Windows 密文在非 Windows 上必须明确拒绝，而不是把密文字节当明文吐出。
        if !cfg!(windows) {
            assert!(decrypt_stored("dpapi:QUJD").is_err());
        }
    }

    // ─── 存储生命周期 ───────────────────────────────────────────────────────

    /// add → 落盘为密文 schema → peek 解回明文 → touch → 换 key（同 id 覆盖）→ delete。
    #[test]
    fn key_plan_lifecycle_persists_encrypts_peeks_and_deletes() {
        let dir = temp_home("lifecycle");
        let store_path = dir.join("credentials.json");

        // 文件不存在 → 全新空 store。
        let store = load_store_at(&store_path).unwrap();
        assert!(store.entries.is_empty());
        assert_eq!(store.version, 1);

        add_key_plan_at(&store_path, "opencode-go", "OpenCode Go", "sk-lifecycle-9f2").unwrap();

        // 落盘是 schema 形状 + 密文，绝不出现明文。
        let raw = fs::read_to_string(&store_path).unwrap();
        assert!(!raw.contains("sk-lifecycle-9f2"), "store 落盘不能含明文 key");
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["version"], serde_json::json!(1));
        assert_eq!(parsed["entries"][0]["kind"], serde_json::json!("key-plan"));
        assert_eq!(parsed["entries"][0]["id"], serde_json::json!("opencode-go"));
        assert_eq!(parsed["entries"][0]["preset"], serde_json::json!("opencode-go"));
        assert_eq!(parsed["entries"][0]["source"], serde_json::json!("manual"));
        assert!(parsed["entries"][0]["createdAt"].is_string());
        let stored_secret = parsed["entries"][0]["secret"].as_str().unwrap();
        assert!(
            stored_secret.starts_with("dpapi:") || stored_secret.starts_with("plain:"),
            "secret = {stored_secret}"
        );

        // peek 解回明文。
        assert_eq!(
            peek_secret_at(&store_path, "opencode-go").unwrap().as_deref(),
            Some("sk-lifecycle-9f2")
        );

        // touch 记录 lastUsedAt。
        touch_last_used_at(&store_path, "opencode-go").unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&store_path).unwrap()).unwrap();
        assert!(parsed["entries"][0]["lastUsedAt"].is_string());

        // 同 id 重新录入 = 换 key，条目数不变。
        add_key_plan_at(&store_path, "opencode-go", "OpenCode Go", "sk-rotated-77").unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&store_path).unwrap()).unwrap();
        assert_eq!(parsed["entries"].as_array().unwrap().len(), 1);
        assert_eq!(
            peek_secret_at(&store_path, "opencode-go").unwrap().as_deref(),
            Some("sk-rotated-77")
        );

        // delete 后 peek 不到，再删报错。
        delete_entry_at(&store_path, "opencode-go").unwrap();
        assert_eq!(peek_secret_at(&store_path, "opencode-go").unwrap(), None);
        let err = delete_entry_at(&store_path, "opencode-go").unwrap_err();
        assert!(err.contains("未找到"), "unexpected: {err}");

        // 非法输入被拒绝，文件不动。
        let before = fs::read_to_string(&store_path).unwrap();
        assert!(add_key_plan_at(&store_path, "", "x", "sk-1").is_err());
        assert!(add_key_plan_at(&store_path, "bad preset!", "x", "sk-1").is_err());
        assert!(add_key_plan_at(&store_path, "glm", "GLM", "   ").is_err());
        assert_eq!(fs::read_to_string(&store_path).unwrap(), before);

        let _ = fs::remove_dir_all(&dir);
    }

    /// 损坏的 store 改名留档后重建 —— 原始字节一份不丢，绝不静默清空
    /// （立场对齐 provider_usage 的 corrupt_auth_json_aborts_instead_of_wiping）。
    #[test]
    fn corrupt_store_is_archived_and_rebuilt_instead_of_wiped() {
        let dir = temp_home("corrupt");
        let store_path = dir.join("credentials.json");

        let corrupt = r#"{"version":1,"entries":[{"id":"half","#;
        fs::write(&store_path, corrupt).unwrap();
        let store = load_store_at(&store_path).unwrap();
        assert!(store.entries.is_empty(), "损坏后应重新初始化");

        let archives = corrupt_archives(&dir);
        assert_eq!(archives.len(), 1, "应恰好留档一份");
        assert_eq!(
            fs::read_to_string(&archives[0]).unwrap(),
            corrupt,
            "留档必须逐字节保留原始内容"
        );
        assert!(!store_path.exists(), "原路径已改名让位");

        // 重建后正常可用。
        add_key_plan_at(&store_path, "glm", "GLM", "sk-after-recover").unwrap();
        assert_eq!(
            peek_secret_at(&store_path, "glm").unwrap().as_deref(),
            Some("sk-after-recover")
        );

        // 认不出的未来版本同样走留档，而不是假装读到了。
        fs::write(&store_path, r#"{"version":2,"entries":[]}"#).unwrap();
        let store = load_store_at(&store_path).unwrap();
        assert!(store.entries.is_empty());
        assert_eq!(corrupt_archives(&dir).len(), 2, "第二份留档不应覆盖第一份");

        // 空文件是全新开始，不是损坏 —— 不产生留档。
        fs::write(&store_path, "").unwrap();
        let store = load_store_at(&store_path).unwrap();
        assert!(store.entries.is_empty());
        assert_eq!(corrupt_archives(&dir).len(), 2);

        let _ = fs::remove_dir_all(&dir);
    }

    // ─── OpenCode 扫描 ─────────────────────────────────────────────────────

    /// api 条目 → Reference（只记路径引用）；oauth 条目 → Subscription（仅展示）；
    /// key 永远不出现在扫描结果里。
    #[test]
    fn opencode_scan_classifies_api_and_oauth_without_copying_keys() {
        let home = temp_home("opencode");
        let auth_path = home.join(".local").join("share").join("opencode").join("auth.json");
        write_file(
            &auth_path,
            r#"{
  "deepseek": {"type": "api", "key": "sk-opencode-secret"},
  "anthropic": {"type": "oauth", "refresh": "rt-x", "access": "at-x"},
  "legacy": {"key": "sk-legacy-no-type"},
  "empty": {}
}"#,
        );

        let scan = scan_opencode_at(&home);
        assert!(scan.error.is_none());

        let ds = find_entry(&scan.entries, "opencode/deepseek");
        assert_eq!(ds.kind, CredentialKind::Reference);
        assert!(ds.secret.is_none(), "reference 条目绝不带 secret 值");
        assert_eq!(ds.source, "scan:opencode");
        let reference = ds.ref_path.as_deref().unwrap();
        assert!(
            reference.ends_with("#deepseek") && reference.contains("auth.json"),
            "ref 应为 文件路径#provider-id，实际 = {reference}"
        );

        let an = find_entry(&scan.entries, "opencode/anthropic");
        assert_eq!(an.kind, CredentialKind::Subscription);
        assert!(an.secret.is_none());
        assert!(an.ref_path.is_none());

        // 旧格式（无 type 有 key）按 api 推断成 Reference。
        assert_eq!(
            find_entry(&scan.entries, "opencode/legacy").kind,
            CredentialKind::Reference
        );
        assert!(scan.entries.iter().all(|e| e.preset.is_none()));

        // 「绝不把 key 写进扫描结果」：整体序列化后无任何明文。
        let leaked = serde_json::to_string(&scan).unwrap();
        assert!(!leaked.contains("sk-opencode-secret"));
        assert!(!leaked.contains("sk-legacy-no-type"));
        assert!(!leaked.contains("rt-x"));

        // 纯读：原文件一字未动。
        assert!(fs::read_to_string(&auth_path).unwrap().contains("sk-opencode-secret"));

        let _ = fs::remove_dir_all(&home);
    }

    /// 引擎注册专用的 key 读取：api 条目（含无 type 的旧格式）带出明文 key，
    /// oauth / 空条目跳过；坏文件 / 缺文件 → 空表，绝不 panic。
    #[test]
    fn peek_opencode_api_keys_reads_only_api_entries() {
        let home = temp_home("opencode_peek");
        let auth_path = home.join(".local").join("share").join("opencode").join("auth.json");
        write_file(
            &auth_path,
            r#"{
  "deepseek": {"type": "api", "key": "sk-ds"},
  "opencode-go": {"key": "sk-go-legacy"},
  "anthropic": {"type": "oauth", "refresh": "rt-x", "access": "at-x"},
  "empty": {}
}"#,
        );
        let mut keys = peek_opencode_api_keys_at(&auth_path);
        keys.sort();
        assert_eq!(
            keys,
            vec![
                ("deepseek".to_string(), "sk-ds".to_string()),
                ("opencode-go".to_string(), "sk-go-legacy".to_string()),
            ]
        );

        // 纯读：原文件一字未动。
        assert!(fs::read_to_string(&auth_path).unwrap().contains("sk-ds"));

        // 坏 JSON / 缺文件 → 空表。
        write_file(&auth_path, r#"{"deepseek":{"key":"sk-x","#);
        assert!(peek_opencode_api_keys_at(&auth_path).is_empty());
        let missing = home.join("nowhere").join("auth.json");
        assert!(peek_opencode_api_keys_at(&missing).is_empty());

        let _ = fs::remove_dir_all(&home);
    }

    /// opencode.db credential 表：active=1 是活钥匙、active=0 剔除、OAuth 型
    /// 不收；活钥匙排在 NULL 迁移拷贝前（2026-10-23 实锤场景：库里
    /// opencode-go 已换新钥匙，auth.json 还躺旧钥匙）。
    #[test]
    fn opencode_db_keys_prefer_active_and_skip_oauth() {
        let home = temp_home("opencode_db");
        let db_path = home.join(".local").join("share").join("opencode").join("opencode.db");
        fs::create_dir_all(db_path.parent().unwrap()).unwrap();
        {
            use rusqlite::{params, Connection};
            let conn = Connection::open(&db_path).unwrap();
            conn.execute(
                "CREATE TABLE credential (integration_id TEXT, value TEXT, active INTEGER)",
                [],
            )
            .unwrap();
            let ins = |conn: &Connection, id: &str, value: String, active: Option<i64>| {
                conn.execute(
                    "INSERT INTO credential (integration_id, value, active) VALUES (?1, ?2, ?3)",
                    params![id, value, active],
                )
                .unwrap();
            };
            let api = |key: &str| format!(r#"{{"type":"api","key":"{key}"}}"#);
            ins(&conn, "opencode", api("stale-console"), None);
            ins(&conn, "opencode-go", api("dead-old"), Some(0));
            ins(&conn, "opencode-go", api("live-new"), Some(1));
            ins(
                &conn,
                "xai",
                r#"{"type":"oauth","access":"at-x"}"#.to_string(),
                None,
            );
        }

        let keys = peek_opencode_db_keys_at(&db_path);
        assert_eq!(
            keys,
            vec![
                ("opencode-go".to_string(), "live-new".to_string()),
                ("opencode".to_string(), "stale-console".to_string()),
            ],
            "active=1 在前、NULL 拷贝在后、active=0 与 OAuth 剔除"
        );

        // 库缺席 / 表缺席 → 空表，绝不 panic。
        assert!(peek_opencode_db_keys_at(&home.join("nowhere.db")).is_empty());
        let empty_db = home.join("empty.db");
        {
            use rusqlite::Connection;
            let conn = Connection::open(&empty_db).unwrap();
            conn.execute_batch("CREATE TABLE other (x);").unwrap();
        }
        assert!(peek_opencode_db_keys_at(&empty_db).is_empty());

        let _ = fs::remove_dir_all(&home);
    }

    /// 库里的活钥匙按 integration 覆盖 auth.json 同名条目；库里没有的
    /// integration 保留 auth.json 兜底；库条目整体在前（先到先得的注册
    /// 计划因此优先拿活钥匙）。
    #[test]
    fn merge_prefers_db_live_keys_over_auth_json() {
        let merged = merge_opencode_key_sources(
            vec![("opencode-go".to_string(), "live-new".to_string())],
            vec![
                ("opencode".to_string(), "stale-console".to_string()),
                ("opencode-go".to_string(), "dead-old".to_string()),
                ("deepseek".to_string(), "ds-key".to_string()),
            ],
        );
        assert_eq!(
            merged,
            vec![
                ("opencode-go".to_string(), "live-new".to_string()),
                ("opencode".to_string(), "stale-console".to_string()),
                ("deepseek".to_string(), "ds-key".to_string()),
            ]
        );
        assert!(merge_opencode_key_sources(Vec::new(), Vec::new()).is_empty());
    }

    /// ZCode 个人 provider 层的 Coding Plan 钥匙：只认
    /// access.type=zhipu-coding-plan-api-key 且 key 非空；其它类型 / 缺文件 /
    /// 坏 JSON → 空表（2026-10-23 实测：钥匙在此层，config.json 的
    /// options.apiKey 恒空）。
    #[test]
    fn zcode_coding_plan_keys_read_personal_provider_layer() {
        let home = temp_home("zcode_keys");
        let path = home.join("provider_config.json");
        write_file(
            &path,
            r#"{
  "schemaVersion": "1",
  "config": {
    "providerConfigRules": {
      "providerRules": [
        {
          "providerId": "bigmodel-api",
          "providerName": "BigModel Coding Plan",
          "config": {
            "group": "standard-personal",
            "access": {"type": "zhipu-coding-plan-api-key", "apiKey": "sk-zhipu-plan"}
          }
        },
        {
          "providerId": "zai-api",
          "config": {
            "access": {"type": "zai-coding-plan-api-key", "apiKey": "sk-zai-plan"}
          }
        },
        {
          "providerId": "empty-key",
          "config": {"access": {"type": "zhipu-coding-plan-api-key", "apiKey": "  "}}
        }
      ]
    }
  }
}"#,
        );
        assert_eq!(
            peek_zcode_coding_plan_keys_at(&path),
            vec![("glm".to_string(), "sk-zhipu-plan".to_string())]
        );

        // 缺文件 / 坏 JSON / 结构缺失 → 空表，绝不 panic。
        assert!(peek_zcode_coding_plan_keys_at(&home.join("nowhere.json")).is_empty());
        write_file(&path, "{\"schemaVersion\":");
        assert!(peek_zcode_coding_plan_keys_at(&path).is_empty());
        write_file(&path, r#"{"config": {}}"#);
        assert!(peek_zcode_coding_plan_keys_at(&path).is_empty());

        let _ = fs::remove_dir_all(&home);
    }

    /// auth.json 解析失败 → error（而不是 panic、不是静默空列表），文件原样。
    #[test]
    fn opencode_scan_reports_corrupt_auth_json_as_error() {
        let home = temp_home("opencode_corrupt");
        let auth_path = home.join(".local").join("share").join("opencode").join("auth.json");
        write_file(&auth_path, r#"{"deepseek":{"key":"sk-x","#);

        let scan = scan_opencode_at(&home);
        let error = scan.error.expect("损坏的 auth.json 应报 error");
        assert!(error.contains("解析失败"), "unexpected: {error}");
        assert!(scan.entries.is_empty());
        assert_eq!(
            fs::read_to_string(&auth_path).unwrap(),
            r#"{"deepseek":{"key":"sk-x","#,
            "扫描是纯读，不能改动别人的文件"
        );

        let _ = fs::remove_dir_all(&home);
    }

    // ─── Claude / Codex 扫描 ────────────────────────────────────────────────

    #[test]
    fn claude_scan_detects_login_and_state_hint() {
        // 有 .credentials.json → Subscription；同时有 ~/.claude.json → 附带提示。
        let home = temp_home("claude_full");
        write_file(&home.join(".claude").join(".credentials.json"), "{}");
        write_file(&home.join(".claude.json"), "{}");
        let scan = scan_claude_at(&home);
        assert!(scan.error.is_none());
        let entry = find_entry(&scan.entries, "claude/login");
        assert_eq!(entry.kind, CredentialKind::Subscription);
        assert_eq!(entry.label, "Claude Code 登录");
        assert!(entry.secret.is_none());
        assert!(scan.note.as_deref().unwrap_or_default().contains(".claude.json"));

        // 只有 ~/.claude.json（状态文件在、凭证不在）→ 只有提示，没有条目。
        let home = temp_home("claude_state_only");
        write_file(&home.join(".claude.json"), "{}");
        let scan = scan_claude_at(&home);
        assert!(scan.entries.is_empty());
        let note = scan.note.expect("应有提示信息");
        assert!(note.contains(".claude.json"));

        // 什么都没有 → 明确的缺失提示。
        let home = temp_home("claude_absent");
        let scan = scan_claude_at(&home);
        assert!(scan.entries.is_empty());
        assert!(scan.note.is_some());

        for tag in ["claude_full", "claude_state_only", "claude_absent"] {
            remove_home(tag);
        }
    }

    #[test]
    fn codex_scan_flags_secret_storage_when_auth_file_missing() {
        // auth.json 在 → Subscription「Codex 登录」。
        let home = temp_home("codex_file");
        write_file(&home.join(".codex").join("auth.json"), "{}");
        let scan = scan_codex_at(&home);
        let entry = find_entry(&scan.entries, "codex/login");
        assert_eq!(entry.kind, CredentialKind::Subscription);
        assert_eq!(entry.label, "Codex 登录");
        assert!(entry.secret.is_none());

        // .codex 目录在、auth.json 不在 → 受限提示条目（系统 secret storage 读不到）。
        let home = temp_home("codex_dir_only");
        fs::create_dir_all(home.join(".codex")).unwrap();
        let scan = scan_codex_at(&home);
        let entry = find_entry(&scan.entries, "codex/login");
        assert_eq!(entry.kind, CredentialKind::Subscription);
        assert!(entry.label.contains("secret storage"), "label = {}", entry.label);

        // 什么都没有 → 缺失提示。
        let home = temp_home("codex_absent");
        let scan = scan_codex_at(&home);
        assert!(scan.entries.is_empty());
        assert!(scan.note.is_some());

        for tag in ["codex_file", "codex_dir_only", "codex_absent"] {
            remove_home(tag);
        }
    }

    // ─── Grok 扫描 ─────────────────────────────────────────────────────────

    /// auth.json 只取 email 用于显示（key 不读不存）；config.toml 的
    /// `[model."…"]` channel 列成 Reference；两者都不泄漏明文。
    #[test]
    fn grok_scan_shows_email_not_key_and_lists_custom_channels() {
        let home = temp_home("grok");
        write_file(
            &home.join(".grok").join("auth.json"),
            r#"{"key":"gsk-never-read","email":"dev@example.com","expires_at":4102444800}"#,
        );
        write_file(
            &home.join(".grok").join("config.toml"),
            r#"# grok fixture
[model."grok/custom-fast"]
api_backend = "chat_completions"
api_key = "gsk-inline-never-copy"

[model."a.b"] # 名字里带点
base_url = "https://example.invalid"

[model.bare]
base_url = "https://example.invalid"

[models]
default = "grok-4"

[other]
x = 1
"#,
        );

        let scan = scan_grok_at(&home);
        assert!(scan.error.is_none());

        let login = find_entry(&scan.entries, "grok/login");
        assert_eq!(login.kind, CredentialKind::Subscription);
        assert!(login.label.contains("dev@example.com"), "label = {}", login.label);
        assert!(login.secret.is_none());

        // channel：quoted / 带点 / 裸 key 都认；[models] 与 [other] 不算。
        let channels: Vec<&str> = scan
            .entries
            .iter()
            .filter(|e| e.kind == CredentialKind::Reference)
            .map(|e| e.id.as_str())
            .collect();
        assert_eq!(
            channels,
            vec![
                "grok/channel/a.b",
                "grok/channel/bare",
                "grok/channel/grok/custom-fast",
            ]
        );
        let fast = find_entry(&scan.entries, "grok/channel/grok/custom-fast");
        assert!(fast.ref_path.as_deref().unwrap().contains("#model.\"grok/custom-fast\""));

        // 明文 key（auth.json 的、config.toml 内联的）都不进扫描结果。
        let leaked = serde_json::to_string(&scan).unwrap();
        assert!(!leaked.contains("gsk-never-read"));
        assert!(!leaked.contains("gsk-inline-never-copy"));

        // 损坏的 auth.json → error，而不是静默跳过。
        let home2 = temp_home("grok_corrupt");
        write_file(&home2.join(".grok").join("auth.json"), "{oops");
        let scan = scan_grok_at(&home2);
        let error = scan.error.expect("损坏的 auth.json 应报 error");
        assert!(error.contains("解析失败"), "unexpected: {error}");
        assert!(scan.entries.is_empty());

        let _ = fs::remove_dir_all(&home);
        let _ = fs::remove_dir_all(&home2);
    }

    // ─── 汇总 ──────────────────────────────────────────────────────────────

    /// 一家扫描失败（error）不影响其他家照常出结果。
    #[test]
    fn scan_all_isolates_a_failing_agent() {
        let home = temp_home("all");
        // opencode 损坏 → error；grok 只有 config.toml → 正常出 channel 条目。
        write_file(
            &home.join(".local").join("share").join("opencode").join("auth.json"),
            "{broken",
        );
        write_file(
            &home.join(".grok").join("config.toml"),
            "[model.\"only-channel\"]\napi_backend = \"chat_completions\"\n",
        );

        let report = scan_all_at(&home);
        assert_eq!(report.opencode.agent, "opencode");
        assert_eq!(report.claude.agent, "claude");
        assert_eq!(report.codex.agent, "codex");
        assert_eq!(report.grok.agent, "grok");

        assert!(report.opencode.error.is_some());
        assert!(report.grok.error.is_none());
        assert_eq!(report.grok.entries.len(), 1);
        assert_eq!(report.grok.entries[0].id, "grok/channel/only-channel");
        assert!(report.claude.note.is_some());
        assert!(report.codex.note.is_some());

        let _ = fs::remove_dir_all(&home);
    }
}
