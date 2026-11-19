//! ZCode 订阅额度行（provider `zcode`）——只读移植。
//!
//! Go 版里 ZCode 是「已退役内置、由插件独占」（zcode.go 头注释）：额度卡由
//! `@magpie-community/opencode-zcode-auth` 的 `usage()` 出，Go 只留读账号的
//! `zcodeOwn`（zcode.go / zcode_team.go）。所以本文件对照两处：Go 的凭据
//! 发现（zcodeSecret / zcodeDecrypt / zcodeOwn / zcodeConfigKey）和插件 index.mjs 的
//! `usage()`（codingUsage / teamUsage / giftOf / dualUsage / limitWindows /
//! termOf）。
//!
//! 凭据来源（只读，取先到者）：
//! 1. `<config_dir>/plugin-auth.json` 的 `zcode` 条目——引擎里登录过 ZCode 的
//!    账号（oauth：refresh 是 JSON `{source,base,device,jwt,key?,org?,project?,
//!    token?,site}`；api：`key`+`metadata.site`）。`source=="zcode"` 的条目与
//!    ZCode 自己的登录同账号时跟随 ZCode 现读的值（插件 stateOf 同款）。
//! 2. ZCode 自己的 `~/.zcode/v2/credentials.json`：值是 `enc:v1:` + iv.tag.密文
//!    （base64url），AES-256-GCM，密钥 = sha256($ZCODE_CREDENTIAL_SECRET，或
//!    "zcode-credential-fallback:<platform>:<home>:<user>")。**`enc:v2:`（ZCode
//!    3.18 起，绑系统密钥库）读不了**——Go 与插件同样不认，这类账号只能靠来源 1。
//!
//! 额度读法：
//! - 有 Coding Plan 钥匙：`{root}/api/monitor/usage/quota/limit`（钥匙裸放
//!   Authorization）；有效订阅才算有套餐（订阅表读到且没有有效订阅、无 level、
//!   无计数窗口 = 这把钥匙没套餐，不出行）。
//! - 团队席位（带 org+project 与钥匙）：`?type=2` + `Bigmodel-Organization/
//!   Project` 头；团队钥匙**本模块不创建**（创建是写操作）——登录态没存钥匙就
//!   不出团队行。
//! - ZCode 会话 JWT（Start Plan 等赠送套餐）：`zcode.z.ai/api/v1/zcode-plan/
//!   billing/balance`，每个活跃套餐的每个余额桶一个窗口。
//!
//! **不做**：赠送套餐的「待领取 / 未开始」提示行、领取（claim，写操作）、
//! 重置券、团队钥匙创建、任何凭据刷新。**绝不写/刷新任何凭据文件，令牌/密钥
//! 永不进日志与错误文本。**
use std::path::PathBuf;
#[cfg(test)]
use std::sync::Mutex;
use std::sync::OnceLock;

use base64::Engine as _;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::quota_kit::*;

const ZCODE_API: &str = "https://zcode.z.ai";
const ZAI_API: &str = "https://api.z.ai";
const BIGMODEL_API: &str = "https://bigmodel.cn";
const ZAI_BASE: &str = "https://api.z.ai/api/anthropic";
const BIGMODEL_BASE: &str = "https://open.bigmodel.cn/api/anthropic";
/// 插件当前冒充的 ZCode 版本：余额接口按它判能力（插件注释）。
const APP_VERSION: &str = "3.17.2";

// =====================================================================
// 测试缝（cfg(test) 才存在；默认读不到任何真实账号）
// =====================================================================

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct Seam {
    pub config_dir: PathBuf,
    pub home: PathBuf,
    pub secret_seed: String,
}

#[cfg(test)]
pub(crate) static TEST_SEAM: Mutex<Option<Seam>> = Mutex::new(None);

#[cfg(test)]
fn seam() -> Option<Seam> {
    TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

#[cfg(test)]
fn plugin_config_dir() -> Option<PathBuf> {
    seam().map(|s| s.config_dir)
}
#[cfg(not(test))]
fn plugin_config_dir() -> Option<PathBuf> {
    super::config_dir()
}

#[cfg(test)]
fn user_home() -> Option<PathBuf> {
    seam().map(|s| s.home)
}
#[cfg(not(test))]
fn user_home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// ZCode 读的家目录（Go ZCodeHome）：Windows 上用户自设且是绝对路径的 HOME
/// 优先（ZCode 是 Node 应用，HOME 先于 homedir()）。
#[cfg(test)]
fn zcode_home() -> Option<PathBuf> {
    user_home()
}
#[cfg(not(test))]
fn zcode_home() -> Option<PathBuf> {
    let home = user_home()?;
    if cfg!(windows) {
        if let Some(h) = std::env::var_os("HOME").map(PathBuf::from) {
            if h.is_absolute() {
                return Some(h);
            }
        }
    }
    Some(home)
}

#[cfg(test)]
fn secret_seed() -> String {
    seam().map(|s| s.secret_seed).unwrap_or_default()
}
#[cfg(not(test))]
fn secret_seed() -> String {
    if let Ok(s) = std::env::var("ZCODE_CREDENTIAL_SECRET") {
        if !s.is_empty() {
            return s;
        }
    }
    let home = user_home().map(|h| h.to_string_lossy().into_owned()).unwrap_or_default();
    let user = std::env::var("USERNAME").or_else(|_| std::env::var("USER")).unwrap_or_default();
    // DOMAIN\user：node 的 userInfo() 只有用户名
    let user = user.rsplit('\\').next().unwrap_or("").to_string();
    let platform = if cfg!(windows) {
        "win32"
    } else if cfg!(target_os = "macos") {
        "darwin"
    } else {
        "linux"
    };
    format!("zcode-credential-fallback:{platform}:{home}:{user}")
}

// =====================================================================
// AES-256-GCM（只解密；纯 Rust、零新依赖）
// =====================================================================
//
// 为什么手写：Cargo.lock 里没有 aes/aes-gcm，体积预算又明令不加新依赖；ZCode
// 的 GCM 用任意长度 nonce（Go NewGCMWithNonceSize(len(iv))），CNG 只认 12 字节。
// 用 NIST GCM 测试向量 13/14 与 node:crypto 生成的向量（见测试）钉死。

fn sbox() -> &'static [u8; 256] {
    static TABLE: OnceLock<[u8; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut s = [0u8; 256];
        let (mut p, mut q) = (1u8, 1u8);
        loop {
            // p *= 3（GF(2^8)）
            p = p ^ (p << 1) ^ if p & 0x80 != 0 { 0x1b } else { 0 };
            // q /= 3
            q ^= q << 1;
            q ^= q << 2;
            q ^= q << 4;
            if q & 0x80 != 0 {
                q ^= 0x09;
            }
            let x = q ^ q.rotate_left(1) ^ q.rotate_left(2) ^ q.rotate_left(3) ^ q.rotate_left(4);
            s[p as usize] = x ^ 0x63;
            if p == 1 {
                break;
            }
        }
        s[0] = 0x63;
        s
    })
}

fn xtime(x: u8) -> u8 {
    (x << 1) ^ if x & 0x80 != 0 { 0x1b } else { 0 }
}

struct Aes256 {
    round_keys: [[u8; 16]; 15],
}

impl Aes256 {
    fn new(key: &[u8; 32]) -> Aes256 {
        let sb = sbox();
        let mut w = [[0u8; 4]; 60];
        for i in 0..8 {
            w[i] = [key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]];
        }
        let mut rcon = 1u8;
        for i in 8..60 {
            let mut t = w[i - 1];
            if i % 8 == 0 {
                t = [sb[t[1] as usize] ^ rcon, sb[t[2] as usize], sb[t[3] as usize], sb[t[0] as usize]];
                rcon = xtime(rcon);
            } else if i % 8 == 4 {
                for b in t.iter_mut() {
                    *b = sb[*b as usize];
                }
            }
            for j in 0..4 {
                w[i][j] = w[i - 8][j] ^ t[j];
            }
        }
        let mut round_keys = [[0u8; 16]; 15];
        for (r, rk) in round_keys.iter_mut().enumerate() {
            for c in 0..4 {
                rk[4 * c..4 * c + 4].copy_from_slice(&w[4 * r + c]);
            }
        }
        Aes256 { round_keys }
    }

    fn encrypt(&self, block: &[u8; 16]) -> [u8; 16] {
        let sb = sbox();
        let mut s = *block;
        let add = |s: &mut [u8; 16], r: usize| {
            for i in 0..16 {
                s[i] ^= self.round_keys[r][i];
            }
        };
        add(&mut s, 0);
        for round in 1..=14 {
            for b in s.iter_mut() {
                *b = sb[*b as usize];
            }
            // ShiftRows：行 r 左移 r 列（状态按列主序：下标 = 4*列 + 行）
            let old = s;
            for c in 0..4 {
                for r in 0..4 {
                    s[4 * c + r] = old[4 * ((c + r) % 4) + r];
                }
            }
            if round != 14 {
                for c in 0..4 {
                    let a = [s[4 * c], s[4 * c + 1], s[4 * c + 2], s[4 * c + 3]];
                    let t = a[0] ^ a[1] ^ a[2] ^ a[3];
                    s[4 * c] = a[0] ^ t ^ xtime(a[0] ^ a[1]);
                    s[4 * c + 1] = a[1] ^ t ^ xtime(a[1] ^ a[2]);
                    s[4 * c + 2] = a[2] ^ t ^ xtime(a[2] ^ a[3]);
                    s[4 * c + 3] = a[3] ^ t ^ xtime(a[3] ^ a[0]);
                }
            }
            add(&mut s, round);
        }
        s
    }
}

/// GF(2^128) 乘法（GCM 规范的位序：位 0 是块的最高位）。
fn gf_mul(x: u128, y: u128) -> u128 {
    let mut z = 0u128;
    let mut v = y;
    for i in 0..128 {
        if (x >> (127 - i)) & 1 == 1 {
            z ^= v;
        }
        v = if v & 1 == 1 { (v >> 1) ^ (0xe1u128 << 120) } else { v >> 1 };
    }
    z
}

fn ghash_update(h: u128, mut y: u128, data: &[u8]) -> u128 {
    for chunk in data.chunks(16) {
        let mut block = [0u8; 16];
        block[..chunk.len()].copy_from_slice(chunk);
        y = gf_mul(y ^ u128::from_be_bytes(block), h);
    }
    y
}

fn inc32(block: &mut [u8; 16]) {
    let counter = u32::from_be_bytes([block[12], block[13], block[14], block[15]]).wrapping_add(1);
    block[12..].copy_from_slice(&counter.to_be_bytes());
}

/// 解 GCM：`combined` = 密文‖tag（Go gcm.Open 的入参形态：tag 取最后 16 字节）。
/// tag 对不上 / 参数不合法 = None。无附加数据。
fn gcm_open(key: &[u8; 32], iv: &[u8], combined: &[u8]) -> Option<Vec<u8>> {
    if iv.is_empty() || combined.len() < 16 {
        return None;
    }
    let (ciphertext, tag) = combined.split_at(combined.len() - 16);
    let aes = Aes256::new(key);
    let h = u128::from_be_bytes(aes.encrypt(&[0u8; 16]));
    let mut j0 = [0u8; 16];
    if iv.len() == 12 {
        j0[..12].copy_from_slice(iv);
        j0[15] = 1;
    } else {
        let y = ghash_update(h, 0, iv);
        let y = gf_mul(y ^ ((iv.len() as u128) * 8), h);
        j0 = y.to_be_bytes();
    }
    // tag = E(J0) ^ GHASH(C ‖ len)
    let mut y = ghash_update(h, 0, ciphertext);
    y = gf_mul(y ^ ((ciphertext.len() as u128) * 8), h);
    let expect = u128::from_be_bytes(aes.encrypt(&j0)) ^ y;
    let given = u128::from_be_bytes(tag.try_into().ok()?);
    if expect != given {
        return None;
    }
    let mut counter = j0;
    let mut plain = Vec::with_capacity(ciphertext.len());
    for chunk in ciphertext.chunks(16) {
        inc32(&mut counter);
        let stream = aes.encrypt(&counter);
        plain.extend(chunk.iter().zip(stream.iter()).map(|(c, k)| c ^ k));
    }
    Some(plain)
}

/// Go zcodeDecrypt：`enc:v1:` + iv.tag.data（base64url，容忍 `=`）。
fn zcode_decrypt(secret: &[u8; 32], value: &str) -> Option<String> {
    let rest = value.strip_prefix("enc:v1:")?;
    let parts: Vec<&str> = rest.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let mut raw: Vec<Vec<u8>> = Vec::new();
    for p in parts {
        raw.push(base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(p.trim_end_matches('=')).ok()?);
    }
    let mut combined = raw[2].clone();
    combined.extend_from_slice(&raw[1]);
    String::from_utf8(gcm_open(secret, &raw[0], &combined)?).ok()
}

fn secret_key() -> [u8; 32] {
    Sha256::digest(secret_seed().as_bytes()).into()
}

// =====================================================================
// 凭据发现（只读）
// =====================================================================

#[derive(Clone, Default)]
struct Sign {
    key: String,
    jwt: String,
    base: String,
    device: String,
    org: String,
    project: String,
    token: String,
    user: String,
    plan: String,
}

impl Sign {
    fn team(&self) -> bool {
        !self.org.is_empty() && !self.project.is_empty()
    }
}

fn read_json(path: &std::path::Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn trimmed(v: Option<&Value>) -> String {
    text(v).trim().to_string()
}

/// ZCode 数据目录（Go ZCodeDataIn）：setting.json 的 dataBaseDir（绝对路径）
/// 优先，否则家目录。
fn zcode_v2_dir(setting_dir: &std::path::Path) -> PathBuf {
    let moved = read_json(&setting_dir.join("setting.json"))
        .and_then(|s| s.get("dataBaseDir").and_then(Value::as_str).map(|d| d.trim().to_string()))
        .filter(|d| !d.is_empty() && std::path::Path::new(d).is_absolute());
    match (moved, zcode_home()) {
        (Some(dir), _) => PathBuf::from(dir).join(".zcode").join("v2"),
        (None, Some(home)) => home.join(".zcode").join("v2"),
        (None, None) => PathBuf::new(),
    }
}

/// Go zcodeConfigKey：config.json 里内置 coding plan provider 的 options.apiKey
/// （明文，或 `enc:` 开头的加密形态）。
fn config_key(dir: &std::path::Path, secret: &[u8; 32]) -> Option<(String, &'static str)> {
    let cfg = read_json(&dir.join("config.json"))?;
    let providers = cfg.get("provider")?;
    for (ids, base) in [
        (["builtin:zai-coding-plan", "account:zai-individual-coding-plan"], ZAI_BASE),
        (["builtin:bigmodel-coding-plan", "account:bigmodel-individual-coding-plan"], BIGMODEL_BASE),
    ] {
        for id in ids {
            let mut key = trimmed(providers.get(id).and_then(|p| p.pointer("/options/apiKey")));
            if key.starts_with("enc:") {
                key = zcode_decrypt(secret, &key).unwrap_or_default().trim().to_string();
            }
            if key.contains('.') && !key.contains([' ', '{', '$']) {
                return Some((key, base));
            }
        }
    }
    None
}

/// ZCode 自己的登录（Go zcodeOwn 的只读部分，加插件的 JWT/用户名读法）。
fn own_sign_in() -> Option<Sign> {
    let setting_dir = zcode_home()?.join(".zcode").join("v2");
    let dir = zcode_v2_dir(&setting_dir);
    let store: Value = read_json(&dir.join("credentials.json"))?;
    let store = store.as_object()?;
    let secret = secret_key();
    let decrypt = |name: &str| -> Option<String> { zcode_decrypt(&secret, store.get(name)?.as_str()?) };
    let mut key = String::new();
    let mut base = String::new();
    for (name, v) in store {
        // account-provider:coding-plan:account:zai-individual-coding-plan:account:<uuid>:api-key
        if !name.contains(":coding-plan:") || !name.ends_with(":api-key") {
            continue;
        }
        let Some(k) = v.as_str().and_then(|v| zcode_decrypt(&secret, v)) else { continue };
        if !k.contains('.') {
            continue;
        }
        let b = if name.contains(":bigmodel-") { BIGMODEL_BASE } else { ZAI_BASE };
        // Z.ai 的先，ZCode 列表同款
        if key.is_empty() || (b == ZAI_BASE && base != ZAI_BASE) {
            key = k;
            base = b.to_string();
        }
    }
    if key.is_empty() {
        if let Some((k, b)) = config_key(&dir, &secret) {
            key = k;
            base = b.to_string();
        }
    }
    let jwt = decrypt("zcodejwttoken")
        .map(|s| s.trim().trim_start_matches("Bearer ").trim().to_string())
        .unwrap_or_default();
    if key.is_empty() && jwt.is_empty() {
        return None;
    }
    let mut info = Value::Null;
    for name in ["oauth:zai:user_info", "oauth:zai_cn:user_info", "oauth:bigmodel:user_info"] {
        if info.is_null() {
            if let Some(parsed) = decrypt(name).and_then(|s| serde_json::from_str::<Value>(&s).ok()) {
                info = parsed;
            }
        }
    }
    let email = trimmed(info.get("email"));
    let email = email.strip_suffix("@phone.local").unwrap_or(&email).to_string();
    let user = [email, trimmed(info.get("name")), trimmed(info.get("user_id"))]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or_else(|| "ZCode".to_string());
    Some(Sign {
        key,
        jwt,
        base: if base.is_empty() { ZAI_BASE.to_string() } else { base },
        user,
        ..Default::default()
    })
}

/// 引擎的 plugin-auth.json 里登录过的 ZCode 账号（插件 stateOf）。
fn plugin_sign_in() -> Option<Sign> {
    let path = plugin_config_dir()?.join("plugin-auth.json");
    let all = read_json(&path)?;
    let auth = all.get("zcode")?;
    let site_base = |site: &str| if site == "bigmodel" { BIGMODEL_BASE } else { ZAI_BASE };
    match text(auth.get("type")) {
        "oauth" => {
            let state: Value = serde_json::from_str(text(auth.get("refresh"))).unwrap_or(Value::Null);
            let site = if text(state.get("site")) == "bigmodel" { "bigmodel" } else { "zai" };
            let jwt = trimmed(state.get("jwt"));
            let key = match state.get("key").and_then(Value::as_str) {
                Some(k) => k.trim().to_string(),
                None if !jwt.is_empty() => String::new(),
                None => trimmed(auth.get("access")),
            };
            let mut s = Sign {
                key,
                jwt,
                base: {
                    let b = trimmed(state.get("base"));
                    if b.is_empty() { site_base(site).to_string() } else { b }
                },
                device: trimmed(state.get("device")),
                org: trimmed(state.get("org")),
                project: trimmed(state.get("project")),
                token: trimmed(state.get("token")),
                user: trimmed(auth.get("accountId")),
                plan: trimmed(state.get("plan")),
            };
            // 「跟随 ZCode」的账号：与 ZCode 自己登录同一账号时取它现读的值
            if text(state.get("source")) == "zcode" {
                if let Some(own) = own_sign_in() {
                    if !s.user.is_empty() && own.user == s.user {
                        s.key = own.key;
                        s.jwt = own.jwt;
                        s.base = own.base;
                        s.org.clear();
                        s.project.clear();
                        s.token.clear();
                    }
                }
            }
            Some(s)
        }
        "api" => {
            let site = if text(auth.pointer("/metadata/site")) == "bigmodel" { "bigmodel" } else { "zai" };
            let key = trimmed(auth.get("key"));
            (!key.is_empty()).then(|| Sign { key, base: site_base(site).to_string(), user: "ZCode".into(), ..Default::default() })
        }
        _ => None,
    }
}

fn discover() -> Option<Sign> {
    plugin_sign_in().or_else(own_sign_in)
}

// =====================================================================
// 额度
// =====================================================================

fn jwt_expiry_secs(jwt: &str) -> Option<i64> {
    let payload = jwt.split('.').nth(1).filter(|_| jwt.split('.').count() == 3)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(payload.trim_end_matches('=')).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    claims.get("exp").and_then(Value::as_f64).filter(|e| *e > 0.0).map(|e| e as i64)
}

fn jwt_expired(jwt: &str) -> bool {
    jwt_expiry_secs(jwt).is_some_and(|exp| chrono::Utc::now().timestamp() > exp)
}

fn root_of(base: &str) -> String {
    let rest = base.split_once("://");
    match rest {
        Some((scheme, r)) => format!("{scheme}://{}", r.split(['/', '?', '#']).next().unwrap_or("")),
        None => ZAI_API.to_string(),
    }
}

fn biz_root(base: &str) -> String {
    let host = hostname_of(base);
    if host == "open.bigmodel.cn" || host.ends_with("bigmodel.cn") {
        BIGMODEL_API.to_string()
    } else {
        root_of(base)
    }
}

fn team_headers(base: &str, org: &str, project: &str) -> Vec<(String, String)> {
    vec![
        h("Bigmodel-Organization", org),
        h("Bigmodel-Project", project),
        h("Set-Language", if biz_root(base) == BIGMODEL_API { "zh" } else { "en" }),
        h("Accept-Language", "en-US,en"),
    ]
}

fn zcode_ua() -> String {
    format!("ZCode/{APP_VERSION}")
}

fn base_headers(auth: &str) -> Vec<(String, String)> {
    vec![
        h("Authorization", auth),
        h("Accept", "application/json"),
        h("User-Agent", &zcode_ua()),
    ]
}

/// 插件 uuidOf：同一个种子永远同一个 UUID（版本 4 形状）。
fn uuid_of(seed: &str) -> String {
    let digest = Sha256::digest(seed.as_bytes());
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    let variant = (u8::from_str_radix(&hex[16..17], 16).unwrap_or(0) & 3) | 8;
    format!("{}-{}-4{}-{:x}{}-{}", &hex[0..8], &hex[8..12], &hex[13..16], variant, &hex[17..20], &hex[20..32])
}

/// 紧凑计数：整数原样，否则两位小数去尾零（插件 compact）。
fn compact(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        let s = format!("{n:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// 插件 spanOf：unit 3 小时、6 周、1 分钟、4 天、5 月；团队五小时不带 number。
fn span_of(unit: i64, number: i64) -> i64 {
    let mut n = number;
    if unit == 3 && n <= 0 {
        n = 5;
    }
    n = n.max(1);
    let per = match unit {
        1 => 60,
        3 => 3600,
        4 => 86400,
        5 => 30 * 86400,
        6 => 7 * 86400,
        _ => 0,
    };
    per * n
}

fn window_name(span: i64) -> String {
    if span == 0 {
        "Credits".to_string()
    } else if span < 86400 {
        format!("{} hours", span / 3600)
    } else if span == 7 * 86400 {
        "Weekly".to_string()
    } else if span >= 28 * 86400 {
        "Monthly".to_string()
    } else {
        format!("{} days", span / 86400)
    }
}

/// 插件 limitWindows：`/api/monitor/usage/quota/limit` 的 limits → 窗口。
fn limit_windows(data: &Value) -> Vec<Win> {
    let mut out = Vec::new();
    for l in data.get("limits").and_then(Value::as_array).into_iter().flatten() {
        let unit = num(l.get("unit")).unwrap_or(0.0) as i64;
        let span = span_of(unit, num(l.get("number")).unwrap_or(0.0) as i64);
        let mut w = Win::new(&window_name(span), 0.0);
        if text(l.get("type")).eq_ignore_ascii_case("TIME_LIMIT") {
            w.name = "MCP · Month".into();
            w.aside = true;
        }
        let percentage = l.get("percentage").and_then(Value::as_f64);
        if let Some(p) = percentage {
            w.used = p;
        }
        let usage = l.get("usage").and_then(Value::as_f64);
        if usage == Some(0.0) {
            w.used = 0.0;
            w.aside = true;
        }
        if let Some(total) = usage.filter(|t| *t > 0.0) {
            if let Some(remaining) = l.get("remaining").and_then(Value::as_f64) {
                let used = total - remaining;
                w.used = 100.0 * used / total;
                w.amount = Some(used);
                w.limit = Some(total);
                w.display = format!("{} / {}", compact(used), compact(total));
            } else if let Some(current) = l.get("currentValue").and_then(Value::as_f64) {
                if percentage.is_none() {
                    w.used = 100.0 * current / total;
                }
                w.amount = Some(current);
                w.limit = Some(total);
                w.display = format!("{} / {}", compact(current), compact(total));
            }
        }
        let reset = num(l.get("nextResetTime")).unwrap_or(0.0) as i64;
        if reset > 0 {
            w.resets_at = from_millis(reset);
        }
        w.span_secs = span;
        out.push(w);
    }
    out
}

struct Card {
    plan: String,
    windows: Vec<Win>,
    until: Option<chrono::DateTime<chrono::Utc>>,
    renew: String,
}

fn plan_name(level: &str) -> String {
    let mut chars = level.chars();
    match chars.next() {
        Some(c) => format!("GLM Coding {}{}", c.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// 个人 Coding Plan：额度 + 订阅期。`Err(())` = 这把钥匙没有 GLM Coding Plan
/// （订阅表读到了、没有有效订阅、无 level、无计数窗口）或读不到。
fn coding_card(s: &Sign, subs: Option<&Value>) -> Option<Card> {
    let root = root_of(&s.base);
    let data = call_env(&format!("{root}/api/monitor/usage/quota/limit"), &base_headers(&s.key))?;
    let level = text(data.get("level")).to_string();
    let windows = limit_windows(&data);
    let mut card = Card { plan: plan_name(&level), windows, until: None, renew: String::new() };
    if let Some(subs) = subs {
        if let Some((until, renew)) = super::plan_quotas::zhipu_term_of(subs) {
            card.until = Some(until);
            card.renew = renew.to_string();
        }
    }
    let valid = subs
        .and_then(Value::as_array)
        .is_some_and(|l| l.iter().any(|x| text(x.get("status")).eq_ignore_ascii_case("VALID")));
    let subs_read = subs.is_some_and(|v| v.is_array());
    if subs_read && !valid && level.is_empty() && s.plan.is_empty() && !card.windows.iter().any(|w| !w.aside) {
        return None;
    }
    Some(card)
}

/// 插件 when()：团队套餐结束时间——北京时间串，或秒/毫秒时间戳。
fn when(v: Option<&Value>) -> Option<chrono::DateTime<chrono::Utc>> {
    if let Some(t) = v.and_then(Value::as_str).and_then(beijing_time) {
        return Some(t);
    }
    let n = num(v).filter(|n| *n > 0.0)?;
    if n > 1e12 { from_millis(n as i64) } else { from_secs(n as i64) }
}

/// 团队席位：type=2 额度 + 套餐名/结束（需要登录 token；没有就只有窗口）。
fn team_card(s: &Sign) -> Option<Card> {
    if s.key.is_empty() {
        return None;
    }
    let root = biz_root(&s.base);
    let headers_extra = team_headers(&s.base, &s.org, &s.project);
    let mut headers = base_headers(&s.key);
    headers.extend(headers_extra.clone());
    let data = call_env(&format!("{root}/api/monitor/usage/quota/limit?type=2"), &headers)?;
    let mut card = Card { plan: String::new(), windows: limit_windows(&data), until: None, renew: String::new() };
    if !s.token.is_empty() {
        let mut headers = base_headers(&s.token);
        headers.extend(headers_extra);
        if let Some(t) = call_env(&format!("{root}/api/biz/team/subscribe/product/querySubscribeDetail"), &headers) {
            let has = t.get("hasSubscription").map(|v| v.as_bool() == Some(true)).unwrap_or(true);
            let usable = has
                && text(t.get("status")).eq_ignore_ascii_case("EFFECTIVE")
                && text(t.get("memberGrantStatus")).eq_ignore_ascii_case("VALID");
            if usable {
                card.plan = text(t.get("productName")).to_string();
                if let Some(end) = when(t.get("subscribeEndTime")) {
                    card.until = Some(end);
                    card.renew = "off".into();
                }
            }
        }
    }
    Some(card)
}

fn bucket_models(x: &Value) -> Vec<String> {
    x.get("capabilities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| c.as_str().unwrap_or("").trim().to_string())
        .filter(|c| c.starts_with("model:") || !c.contains(':'))
        .map(|c| c.trim_start_matches("model:").trim().to_string())
        .filter(|c| !c.is_empty())
        .collect()
}

fn gift_spent(x: &Value) -> bool {
    if let Some(left) = num(x.get("remaining_units")) {
        return left <= 0.0;
    }
    matches!((num(x.get("total_units")), num(x.get("used_units"))), (Some(total), Some(used)) if used >= total)
}

fn period_secs(period: &str) -> i64 {
    let p = period.to_ascii_lowercase();
    if p.contains("day") || p.contains("daily") {
        86400
    } else if p.contains("week") {
        7 * 86400
    } else if p.contains("month") {
        30 * 86400
    } else {
        0
    }
}

/// 插件 giftOf：ZCode 会话读到的赠送套餐。每个活跃套餐的每个余额桶一格；
/// 没有活跃套餐 = None。返回 (套餐名, 结束, 窗口 [(窗口, 所属套餐名)])。
fn gift_windows(s: &Sign) -> Option<(String, Option<chrono::DateTime<chrono::Utc>>, Vec<(Win, String)>)> {
    if s.jwt.is_empty() || jwt_expired(&s.jwt) {
        return None;
    }
    let device = if !s.device.is_empty() {
        s.device.clone()
    } else {
        uuid_of(&format!("zcode-device\0{}", if s.key.is_empty() { &s.jwt } else { &s.key }))
    };
    let mut headers = base_headers(&format!("Bearer {}", s.jwt));
    headers.push(h("X-Device-Mid", &device));
    let url = format!("{ZCODE_API}/api/v1/zcode-plan/billing/balance?app_version={APP_VERSION}");
    let b = call_env(&url, &headers)?;
    let server_now = num(b.get("server_time")).filter(|t| *t > 0.0).unwrap_or_else(|| chrono::Utc::now().timestamp() as f64);
    let status_of = |p: &Value| text(p.get("status")).trim().to_ascii_lowercase();
    let same = |x: &Value, p: &Value| {
        let (xu, pu) = (text(x.get("user_plan_id")), text(p.get("user_plan_id")));
        if !xu.is_empty() && !pu.is_empty() {
            xu == pu
        } else {
            plan_id_str(x) == plan_id_str(p)
        }
    };
    let plans: Vec<&Value> = b.get("plans").and_then(Value::as_array).map(|a| a.iter().collect()).unwrap_or_default();
    let ended = |p: &Value| num(p.get("ends_at")).is_some_and(|e| e > 0.0 && e <= server_now);
    let not_started = |p: &Value| {
        num(p.get("starts_at")).is_some_and(|at| at > server_now) && status_of(p) != "expired"
    };
    // 活跃 = status active、已开始、没过期
    let live: Vec<&Value> = plans
        .iter()
        .copied()
        .filter(|p| status_of(p) == "active" && !not_started(p) && !ended(p))
        .collect();
    let mut active: Option<(String, f64)> = None;
    for p in &live {
        let end = num(p.get("ends_at")).unwrap_or(0.0);
        let name = {
            let n = text(p.get("name")).trim();
            if n.is_empty() { "Start Plan".to_string() } else { n.to_string() }
        };
        let better = match &active {
            None => true,
            Some((_, cur)) => end > 0.0 && (*cur <= 0.0 || end < *cur),
        };
        if better {
            active = Some((name, end));
        }
    }
    let (plan, end) = active?;
    let until = (end > 0.0).then(|| from_secs(end as i64)).flatten();
    let expired_plans: Vec<&Value> =
        plans.iter().copied().filter(|p| status_of(p) == "expired" || ended(p)).collect();
    let mut out = Vec::new();
    for x in b.get("balances").and_then(Value::as_array).into_iter().flatten() {
        // 属于已过期套餐的桶不要（插件 over 集合）
        if expired_plans.iter().any(|p| same(x, p)) && !live.iter().any(|p| same(x, p)) {
            continue;
        }
        let Some(owner) = live.iter().find(|p| same(x, p)) else { continue };
        let exp = num(x.get("expires_at")).unwrap_or(0.0);
        if exp > 0.0 && exp <= server_now {
            continue; // 过了自己期限的桶
        }
        let (total, left) = (num(x.get("total_units")), num(x.get("remaining_units")));
        let mut used = num(x.get("used_units"));
        if total.is_none() && used.is_none() && left.is_none() {
            continue;
        }
        let models = bucket_models(x);
        let show = text(x.get("show_name")).trim().to_string();
        let name = if !show.is_empty() {
            show
        } else if !models.is_empty() {
            models.join(", ")
        } else {
            "Credits".to_string()
        };
        let spent = gift_spent(x);
        let mut w = Win::new(&name, 0.0);
        if used.is_none() {
            used = Some(match (total, left) {
                (Some(t), Some(l)) => t - l,
                _ => 0.0,
            });
        }
        let used = used.unwrap_or(0.0);
        if let Some(total) = total.filter(|t| *t > 0.0) {
            w.used = 100.0 * used / total;
            w.amount = Some(used);
            w.limit = Some(total);
            w.display = format!("{} / {}", compact(used), compact(total));
        }
        if spent {
            w.used = w.used.max(100.0);
        }
        if exp > 0.0 {
            w.resets_at = from_secs(exp as i64);
        }
        let mut span = 0;
        for e in owner.get("entitlements").and_then(Value::as_array).into_iter().flatten() {
            if text(e.get("entitlement_id")) == text(x.get("entitlement_id")) {
                span = period_secs(text(e.get("period")));
            }
        }
        if span == 0 {
            if let (Some(a), Some(z)) = (num(x.get("period_start")), num(x.get("period_end"))) {
                if z > a {
                    span = (z - a) as i64;
                }
            }
        }
        w.span_secs = span;
        out.push((w, text(owner.get("name")).to_string()));
    }
    Some((plan, until, out))
}

fn plan_id_str(v: &Value) -> String {
    match v.get("plan_id") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// 赠送桶的窗口名：多个套餐时桶名后缀套餐名；与 Coding Plan 并列时一律后缀。
fn gift_label(items: Vec<(Win, String)>, always: bool) -> Vec<Win> {
    let mut plans: Vec<&str> = items.iter().map(|(_, p)| p.as_str()).collect();
    plans.sort_unstable();
    plans.dedup();
    let several = plans.len() > 1;
    items
        .into_iter()
        .map(|(mut w, plan)| {
            if always {
                let tag = if plan.is_empty() { "Start Plan" } else { plan.as_str() };
                w.name = format!("{} · {}", w.name, tag);
            } else if several && !plan.is_empty() {
                w.name = format!("{} · {}", w.name, plan);
            }
            w
        })
        .collect()
}

/// ZCode 订阅额度行。没有账号 / 登录过期 / 没套餐 / 请求失败 → None
/// （展示面，安静跳过）。
pub fn quota_row() -> Option<Value> {
    let s = discover()?;
    let jwt_ok = !s.jwt.is_empty() && !jwt_expired(&s.jwt);
    let card = if s.team() {
        // 团队席位：有钥匙读团队额度，赠送套餐并列；没钥匙只能读赠送套餐
        let team = team_card(&s);
        match (team, jwt_ok.then(|| gift_windows(&s)).flatten()) {
            (Some(mut card), Some((_, _, items))) => {
                card.windows.extend(gift_label(items, true));
                card
            }
            (Some(card), None) => card,
            (None, Some((plan, until, items))) => gift_card(plan, until, items),
            (None, None) => return None,
        }
    } else if !s.key.is_empty() {
        let subs = call_env(&format!("{}/api/biz/subscription/list", root_of(&s.base)), &base_headers(&s.key));
        let valid = subs
            .as_ref()
            .and_then(Value::as_array)
            .map(|l| l.iter().any(|x| text(x.get("status")).eq_ignore_ascii_case("VALID")));
        let gift = if jwt_ok { gift_windows(&s) } else { None };
        // 没有有效订阅、有 ZCode 会话：账号只有赠送套餐（插件 plansOf 的 start）
        if valid == Some(false) && jwt_ok {
            let (plan, until, items) = gift?;
            gift_card(plan, until, items)
        } else {
            let mut card = coding_card(&s, subs.as_ref())?;
            if let Some((_, _, items)) = gift {
                card.windows.extend(gift_label(items, true));
            }
            card
        }
    } else if jwt_ok {
        let (plan, until, items) = gift_windows(&s)?;
        gift_card(plan, until, items)
    } else {
        return None;
    };
    if card.windows.is_empty() {
        return None;
    }
    let mut row = json!({
        "provider": "zcode",
        "name": "ZCode",
        "windows": windows_json(&card.windows),
    });
    if !s.user.is_empty() {
        row["user"] = json!(s.user);
    }
    let plan = if card.plan.is_empty() { s.plan.clone() } else { card.plan.clone() };
    if !plan.is_empty() {
        row["plan"] = json!(plan);
    }
    if let Some(until) = card.until {
        row["until"] = json!(rfc3339(until));
        if !card.renew.is_empty() {
            row["renew"] = json!(card.renew);
        }
    }
    Some(row)
}

fn gift_card(plan: String, until: Option<chrono::DateTime<chrono::Utc>>, items: Vec<(Win, String)>) -> Card {
    Card {
        plan,
        windows: gift_label(items, false),
        renew: if until.is_some() { "off".into() } else { String::new() },
        until,
    }
}

// =====================================================================
// trial 条目拒答判定（Go zcode_trial.go，词表 1:1）
// =====================================================================
//
// Go 的两个词表（zcode_trial.go:20，ZCode 插件对自己发不出的 Start Plan
// trial 条目（`<model>-Trial`）的拒答词，任何 `<provider>: ` 前缀之后）：
//
// - `zcodeEndedWords`（HTTP 400，trial 已结束）：
//   `(?:^|: )ZCode has no active limited quota for (\S+)$`
// - `zcodeSpentWords`（HTTP 429，trial 的额度桶耗尽）：
//   `(?:^|: )ZCode's limited quota for (\S+) is exhausted or temporarily unavailable$`
//
// 语义（Go zcodeTrialRefused，#3661 / orion-nebula-dev）：两个过期 trial 曾把
// 整个 `zcode@zcode` 账号 rest 掉，GLM-5.3 / GLM-5.3-Flash 跟着陪葬——trial
// 是那个模型自己的，账号的 GLM Coding Plan 还能出自己的模型，**只 rest 该
// trial 条目**。词表点名模型：ended 说 `-Trial` 全名（`EqualFold` 核对候选模
// 型），spent 说去掉 `-Trial` 的桶模型（核对 `m[1]+"-Trial"`）。zcode 0.1.49
// 把原因写在词前（`… so it is not asked again for a minute: ZCode's limited
// quota for …`，`(?:^|: )` 的 `: ` 分支接住）；0.1.48 把括号注记放在词后
// （`… unavailable (ZCode answered 429 …)`），词表判不中，账号整体 rest——
// 有意的行为差，别「修」。其余拒答（账号级 429、1113、无效钥匙）照旧 rest
// 整个账号。同文件另有 `zcodeTrialBlocked`（zcode.z.ai 边缘防火墙 405 拦截
// 页）/ `zcodeTrialRestricted`（风控 3012）也只 rest trial 条目——它们的词表
// 在 provider.EdgeBlocked / failRestricted，不进本判定。
//
// **Rust 现状（2026-11-18 已联动）**：chat.rs 的候选循环错误路径（vendor
// 错误体已读到的 `Attempt::Retry` 分支）调 [`trial_refused_for`]，命中时只
// rest 该 `<model>-Trial` 的模型级条目（chat.rs 的 rest 表加 `<provider_id>
// ::<model>` 键——Go `candidate.restID()` = restKey()+"/"+model 的对应，
// fallback.go:211-213），同 provider 的账号其它模型照常候选。判不中回落
// chat.rs 原有行为（429/5xx 整 provider rest、400 不 rest——Go 的 failQuota
// 400 会 rest 整账号，那是 Rust 极简 rest 的既有差距，见 chat.rs 尾部清单）。
// 判不中的情形含：词表没中（0.1.48 词形）、点名模型对不上（EqualFold）、
// 来源不是 zcode、候选模型不带 `-Trial`、status 不是 400/429。

/// Go `vendorMessage`（draw.go:1586）的 JSON 分支：厂商错误体里「说了什么」——
/// `errors.message`（ModelScope）、无 `error` 字段时的 `msg`(+`code`)（WorkBuddy）、
/// `error.message` / `error` 字符串。网页标题分支（provider.WebPage）不搬——
/// trial 拒答都是 JSON 体。都读不到时整串兜底（Go 同款）。
fn vendor_message(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return body.to_string() };
    if let Some(m) = v
        .get("errors")
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .filter(|m| !m.is_empty())
    {
        return m.to_string();
    }
    // Go：len(e.Error)==0 才看 msg（"error" 字段在就不走这里，null 也算在）
    if v.get("error").is_none() {
        if let Some(msg) = v.get("msg").and_then(Value::as_str).filter(|m| !m.is_empty()) {
            let code = match v.get("code") {
                Some(Value::String(s)) => Some(s.trim_matches('"').to_string()),
                Some(Value::Number(n)) => Some(n.to_string()),
                _ => None,
            };
            return match code {
                Some(c) if !c.is_empty() && c != "0" && c != "null" => format!("{msg} ({c})"),
                _ => msg.to_string(),
            };
        }
    }
    if let Some(err) = v.get("error") {
        if let Some(m) = err.get("message").and_then(Value::as_str).filter(|m| !m.is_empty()) {
            return m.to_string();
        }
        if let Some(s) = err.as_str() {
            return s.to_string();
        }
    }
    body.to_string()
}

/// trial 拒答的两种词形与它点名的模型。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TrialForm {
    /// 400：trial 已结束（点名 `-Trial` 全名）
    Ended,
    /// 429：trial 的额度桶耗尽（点名去 `-Trial` 的桶模型）
    Spent,
}

/// Go 两个词表的纯字符串对应。`(?:^|: )` = 串首或任何 `": "` 之后；`\S+` =
/// 非空且不含 ASCII 空白（Go RE2 的 `\s` 是 `[\t\n\f\r ]`，非 Unicode）；
/// `$` = 串尾（无 `(?m)`，注记跟在词后不算）。大小写敏感（Go lazyre 无
/// `(?i)`；候选模型的 EqualFold 核对在 [`trial_refused_for`]）。
fn trial_words(said: &str) -> Option<(TrialForm, String)> {
    const ENDED: &str = "ZCode has no active limited quota for ";
    const SPENT_HEAD: &str = "ZCode's limited quota for ";
    const SPENT_TAIL: &str = " is exhausted or temporarily unavailable";
    let solid = |w: &str| !w.is_empty() && !w.bytes().any(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r'));
    let mut starts = vec![0usize];
    let mut from = 0usize;
    while let Some(i) = said[from..].find(": ") {
        from += i + 2;
        starts.push(from);
    }
    for start in starts {
        let seg = &said[start..];
        if let Some(word) = seg.strip_prefix(ENDED) {
            if solid(word) {
                return Some((TrialForm::Ended, word.to_string()));
            }
        }
        if let Some(mid) = seg.strip_prefix(SPENT_HEAD) {
            if let Some(word) = mid.strip_suffix(SPENT_TAIL) {
                if solid(word) {
                    return Some((TrialForm::Spent, word.to_string()));
                }
            }
        }
    }
    None
}

/// 这具 body 是 ZCode 对某个 Start Plan trial 条目（`<model>-Trial`）自己的
/// 拒答吗（两种词形任一）？**词表 1:1**，只看 body 的词形——status / 来源 /
/// 候选模型几道门在 [`trial_refused_for`]（chat.rs 的 rest 分级走它），本函数
/// 保留给词表自身的测试。
pub fn trial_refused(body: &str) -> bool {
    let said = vendor_message(body);
    trial_words(said.trim()).is_some()
}

/// Go `zcodeTrialRefused`（zcode_trial.go:33-48）的完整判定：这具错误应答是
/// ZCode 账号对自己某个 Start Plan trial 条目（`<model>-Trial`）的拒答吗？
/// 四道门全过才认（返回词形，供 rest 分级记条目）：
///
/// 1. **来源门**：provider id 是 zcode（Go 是 `c.p.PluginProvider() == "zcode"`
///    ——查插件注册表；Rust 网关没有插件层，按 store 的 provider id 近似，
///    `zcode-plugin` 是 Go `PluginID` 撞名时的变体，plugins.go:102-110）；
/// 2. **候选门**：本轮候选模型自身以 `-trial` 结尾（大小写不敏感；Go
///    `strings.HasSuffix(strings.ToLower(c.model), "-trial")`，zcode_trial.go:34
///    ——plain 模型的拒答不进本判定，照旧整账号）；
/// 3. **status 门**：400（trial 已结束）或 429（额度桶耗尽），其余不认；
/// 4. **点名核对**：词表点名的模型与候选模型 EqualFold——ended 说 `-Trial`
///    全名（`m[1]` 直比，zcode_trial.go:41），spent 说去 `-Trial` 的桶模型
///    （比 `m[1]+"-Trial"`，zcode_trial.go:45）。对不上 = 不明拒答，返回
///    None，调用方走原有 rest 行为（Go 里整账号 rest，zcode_trial_test.go:61-62
///    「another trial named / spent names another」）。
///
/// EqualFold 语义：Go `strings.EqualFold` 是 Unicode 简单折叠；这里用
/// `eq_ignore_ascii_case`（ASCII 折叠，非 ASCII 逐字符精确比对）。trial
/// 条目名与厂商模型 id 实际都是 ASCII（`GLM-5.3-Trial`），差异只在理论面；
/// 如实记下，不搬 Unicode 折叠表。
pub(crate) fn trial_refused_for(provider_id: &str, cand_model: &str, status: u16, body: &[u8]) -> Option<TrialForm> {
    const ZCODE_IDS: [&str; 2] = ["zcode", "zcode-plugin"];
    if !ZCODE_IDS.contains(&provider_id) {
        return None;
    }
    if !cand_model.to_ascii_lowercase().ends_with("-trial") {
        return None;
    }
    let said = vendor_message(&String::from_utf8_lossy(body));
    let said = said.trim();
    match status {
        400 => match trial_words(said) {
            Some((TrialForm::Ended, named)) if named.eq_ignore_ascii_case(cand_model) => Some(TrialForm::Ended),
            _ => None,
        },
        429 => match trial_words(said) {
            Some((TrialForm::Spent, named)) if format!("{named}-Trial").eq_ignore_ascii_case(cand_model) => {
                Some(TrialForm::Spent)
            }
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::quota_kit::testkit::{redirect, Mock, Redirects};
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn gcm_matches_nist_vectors() {
        // NIST GCM 测试向量 13 / 14（AES-256，全零密钥与 96 位 IV）。
        let key = [0u8; 32];
        let iv = [0u8; 12];
        let empty = gcm_open(&key, &iv, &hex("530f8afbc74536b9a963b4f1c4cb738b")).expect("tc13");
        assert!(empty.is_empty());
        let plain = gcm_open(
            &key,
            &iv,
            &hex("cea7403d4d606b6e074ec5d3baf39d18d0d1c8a799996bf0265b98b5d48ab919"),
        )
        .expect("tc14");
        assert_eq!(plain, vec![0u8; 16]);
        // 密文被改一位 → tag 不过
        let mut bad = hex("cea7403d4d606b6e074ec5d3baf39d18d0d1c8a799996bf0265b98b5d48ab919");
        bad[0] ^= 1;
        assert!(gcm_open(&key, &iv, &bad).is_none());
    }

    #[test]
    fn gcm_matches_node_crypto_for_odd_nonce_sizes() {
        // 向量由 node:crypto 的 aes-256-gcm 生成（见交付说明）：密钥 = sha256("seed-A")，
        // 16 字节 nonce（非 12——Go NewGCMWithNonceSize 的场景）与 12 字节 nonce 各一份，
        // 明文 49 字节（多块 + 半块）。
        let key: [u8; 32] = Sha256::digest(b"seed-A").into();
        let v1 = include_str!("zcode_account_vectors.txt");
        let mut seen = 0;
        for line in v1.lines().filter(|l| !l.trim().is_empty()) {
            let (value, expect) = line.split_once('|').expect("vector line");
            let plain = zcode_decrypt(&key, value.trim()).expect("decrypt");
            assert_eq!(plain, expect.trim());
            seen += 1;
        }
        assert_eq!(seen, 3);
        // 错密钥不解
        let other: [u8; 32] = Sha256::digest(b"seed-B").into();
        let first = v1.lines().next().unwrap().split_once('|').unwrap().0.trim();
        assert!(zcode_decrypt(&other, first).is_none());
        // v2 与坏形状一律读不了
        assert!(zcode_decrypt(&key, "enc:v2:AAAA.BBBB.CCCC").is_none());
        assert!(zcode_decrypt(&key, "enc:v1:AAAA.BBBB").is_none());
        assert!(zcode_decrypt(&key, "plain").is_none());
    }

    struct World {
        dir: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for World {
        fn drop(&mut self) {
            *TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()) = None;
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn world() -> World {
        let lock = super::super::TEST_SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = std::env::temp_dir().join(format!(
            "marionette-engine-zcode-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("cfg")).unwrap();
        std::fs::create_dir_all(dir.join("home").join(".zcode").join("v2")).unwrap();
        *TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()) = Some(Seam {
            config_dir: dir.join("cfg"),
            home: dir.join("home"),
            secret_seed: "seed-A".into(),
        });
        World { dir, _lock: lock }
    }

    fn write_plugin_auth(w: &World, entry: Value) {
        std::fs::write(w.dir.join("cfg").join("plugin-auth.json"), serde_json::to_vec(&json!({ "zcode": entry })).unwrap())
            .unwrap();
    }

    fn fake_jwt(exp: i64) -> String {
        let b = |v: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(v).unwrap());
        format!("{}.{}.sig", b(&json!({"alg":"none"})), b(&json!({"exp": exp})))
    }

    #[test]
    fn no_account_means_no_row() {
        let _w = world();
        assert!(discover().is_none());
        assert!(quota_row().is_none());
    }

    #[test]
    fn coding_plan_key_row_with_term_and_bare_authorization() {
        let w = world();
        write_plugin_auth(
            &w,
            json!({"type":"oauth","access":"sk-unused","accountId":"me@example.test",
                   "refresh": json!({"source":"stored","base":"https://api.z.ai/api/anthropic","site":"zai",
                                     "key":"id.secret","device":"dev-1"}).to_string()}),
        );
        let mock = Mock::start();
        mock.route(
            "/api/monitor/usage/quota/limit",
            200,
            r#"{"code":200,"msg":"ok","success":true,"data":{"level":"pro","limits":[
                {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":12,"nextResetTime":1758800000000},
                {"type":"TOKENS_LIMIT","unit":6,"number":1,"usage":1000,"remaining":600,"percentage":40,"nextResetTime":1759400000000},
                {"type":"TIME_LIMIT","unit":5,"number":1,"usage":0}]}}"#,
        );
        mock.route(
            "/api/biz/subscription/list",
            200,
            r#"{"code":0,"data":[{"status":"VALID","autoRenew":true,"nextRenewTime":"2026-10-18 12:00:00"}]}"#,
        );
        let _r = Redirects::new(&[redirect("https://api.z.ai", &mock.base)]);
        let row = quota_row().expect("row");
        assert_eq!(row["provider"], "zcode");
        assert_eq!(row["name"], "ZCode");
        assert_eq!(row["user"], "me@example.test");
        assert_eq!(row["plan"], "GLM Coding Pro");
        assert_eq!(row["renew"], "auto");
        assert_eq!(row["until"], "2026-10-18T04:00:00Z");
        let ws = row["windows"].as_array().unwrap();
        assert_eq!(ws[0]["name"], "5 hours");
        assert_eq!(ws[0]["used"], 12.0);
        assert_eq!(ws[1]["name"], "Weekly");
        assert_eq!(ws[1]["used"], 40.0, "usage/remaining 同时给时按计数算：(1000-600)/1000");
        assert_eq!(ws[1]["display"], "400 / 1000");
        assert_eq!(ws[2]["name"], "MCP · Month");
        let requests = mock.requests();
        let quota = requests.iter().find(|r| r.path.starts_with("/api/monitor")).unwrap();
        assert_eq!(quota.header("authorization"), "id.secret", "钥匙裸放 Authorization");
        assert_eq!(quota.header("user-agent"), format!("ZCode/{APP_VERSION}"));
    }

    #[test]
    fn key_without_a_valid_subscription_has_no_card() {
        let w = world();
        write_plugin_auth(
            &w,
            json!({"type":"api","key":"id.secret","metadata":{"site":"zai"}}),
        );
        let mock = Mock::start();
        mock.route("/api/monitor/usage/quota/limit", 200, r#"{"code":200,"data":{"limits":[]}}"#);
        mock.route("/api/biz/subscription/list", 200, r#"{"code":200,"data":[{"status":"EXPIRED"}]}"#);
        let _r = Redirects::new(&[redirect("https://api.z.ai", &mock.base)]);
        assert!(quota_row().is_none());
    }

    #[test]
    fn jwt_only_account_reads_gift_buckets() {
        let w = world();
        let jwt = fake_jwt(chrono::Utc::now().timestamp() + 3600);
        write_plugin_auth(
            &w,
            json!({"type":"oauth","access":jwt,"accountId":"gift@example.test",
                   "refresh": json!({"source":"stored","site":"zai","device":"dev-9","jwt":jwt}).to_string()}),
        );
        let now = chrono::Utc::now().timestamp();
        let mock = Mock::start();
        mock.route(
            "/api/v1/zcode-plan/billing/balance",
            200,
            &json!({"code":0,"data":{"server_time":now,
                "plans":[
                    {"name":"Start Plan","status":"active","user_plan_id":"u1","plan_id":"p1","starts_at":now-100,"ends_at":now+86400,
                     "entitlements":[{"entitlement_id":"e1","period":"daily"}]},
                    {"name":"Old","status":"expired","user_plan_id":"u2","plan_id":"p2","ends_at":now-5}],
                "balances":[
                    {"user_plan_id":"u1","plan_id":"p1","entitlement_id":"e1","show_name":"GLM-5.3-Flash","capabilities":["model:glm-5.3-flash"],
                     "total_units":100,"remaining_units":25,"used_units":75,"expires_at":now+7200},
                    {"user_plan_id":"u1","plan_id":"p1","entitlement_id":"e2","capabilities":["glm-5.2"],
                     "total_units":10,"remaining_units":0,"expires_at":now+7200},
                    {"user_plan_id":"u2","plan_id":"p2","entitlement_id":"e9","show_name":"gone","total_units":5,"remaining_units":5}]}}).to_string(),
        );
        let _r = Redirects::new(&[redirect("https://zcode.z.ai", &mock.base)]);
        let row = quota_row().expect("row");
        assert_eq!(row["plan"], "Start Plan");
        assert_eq!(row["renew"], "off");
        let ws = row["windows"].as_array().unwrap();
        assert_eq!(ws.len(), 2, "过期套餐的桶不出");
        assert_eq!(ws[0]["name"], "GLM-5.3-Flash");
        assert_eq!(ws[0]["used"], 75.0);
        assert_eq!(ws[0]["display"], "75 / 100");
        assert!(ws[0]["resetsAt"].is_string());
        assert_eq!(ws[1]["name"], "glm-5.2", "没有 show_name 取模型名");
        assert_eq!(ws[1]["used"], 100.0, "用尽的桶至少 100%");
        let requests = mock.requests();
        let balance = &requests[0];
        assert!(balance.path.contains("app_version="));
        assert!(balance.header("authorization").starts_with("Bearer "));
        assert_eq!(balance.header("x-device-mid"), "dev-9");
    }

    #[test]
    fn expired_jwt_and_failures_are_quiet() {
        let w = world();
        let jwt = fake_jwt(chrono::Utc::now().timestamp() - 10);
        write_plugin_auth(
            &w,
            json!({"type":"oauth","access":jwt,"accountId":"x",
                   "refresh": json!({"site":"zai","jwt":jwt}).to_string()}),
        );
        // 没有任何 redirect：真网被测试缝挡死，过期 JWT 也不该发请求
        assert!(quota_row().is_none());
    }

    #[test]
    fn own_sign_in_decrypts_v1_credentials_and_follows_them() {
        let w = world();
        // 用与测试向量同一套参数造 v1 值：这里直接用 node 生成的向量里没有的
        // 内容，所以走「明文 config.json 钥匙」这条只读路径验证发现链。
        std::fs::write(
            w.dir.join("home").join(".zcode").join("v2").join("config.json"),
            serde_json::to_vec(&json!({"provider":{"builtin:zai-coding-plan":{"options":{"apiKey":"abc.def"}}}})).unwrap(),
        )
        .unwrap();
        // credentials.json 必须存在才算登录过（Go：读不到 store 就没有账号）
        std::fs::write(w.dir.join("home").join(".zcode").join("v2").join("credentials.json"), b"{}").unwrap();
        let own = own_sign_in().expect("own");
        assert_eq!(own.key, "abc.def");
        assert_eq!(own.base, ZAI_BASE);
        assert_eq!(own.user, "ZCode");
        // 含空格 / 占位符的不是钥匙
        std::fs::write(
            w.dir.join("home").join(".zcode").join("v2").join("config.json"),
            serde_json::to_vec(&json!({"provider":{"builtin:zai-coding-plan":{"options":{"apiKey":"${ENV}.x"}}}})).unwrap(),
        )
        .unwrap();
        assert!(own_sign_in().is_none());
    }

    #[test]
    fn team_seat_needs_a_stored_key_and_sends_team_headers() {
        let w = world();
        write_plugin_auth(
            &w,
            json!({"type":"oauth","access":"x","accountId":"boss@example.test",
                   "refresh": json!({"site":"bigmodel","base":"https://open.bigmodel.cn/api/anthropic",
                                     "key":"team.key","org":"org-9","project":"proj-9","token":"biz-token"}).to_string()}),
        );
        let mock = Mock::start();
        mock.route(
            "/api/monitor/usage/quota/limit?type=2",
            200,
            r#"{"code":200,"data":{"limits":[{"type":"TOKENS_LIMIT","unit":3,"percentage":30}]}}"#,
        );
        mock.route(
            "/api/biz/team/subscribe/product/querySubscribeDetail",
            200,
            r#"{"code":200,"data":{"hasSubscription":true,"status":"EFFECTIVE","memberGrantStatus":"VALID",
                "productName":"GLM Coding Team","subscribeEndTime":"2026-12-01 00:00:00"}}"#,
        );
        let _r = Redirects::new(&[redirect("https://bigmodel.cn", &mock.base)]);
        let row = quota_row().expect("row");
        assert_eq!(row["plan"], "GLM Coding Team");
        assert_eq!(row["windows"][0]["name"], "5 hours");
        assert_eq!(row["until"], "2026-11-30T16:00:00Z");
        let requests = mock.requests();
        let quota = requests.iter().find(|r| r.path.contains("type=2")).unwrap();
        assert_eq!(quota.header("bigmodel-organization"), "org-9");
        assert_eq!(quota.header("bigmodel-project"), "proj-9");
        assert_eq!(quota.header("set-language"), "zh");
        assert_eq!(quota.header("authorization"), "team.key");
    }

    // ----- trial 拒答词（Go zcode_trial_test.go:18 的体 1:1，zcode 0.1.31 的
    // fetch JSON.stringify 形态）-----

    const ZCODE_FLASH_TRIAL_ENDED: &str = r#"{"type":"error","error":{"type":"invalid_request_error","message":"ZCode has no active limited quota for GLM-5.3-Flash-Trial"}}"#;
    const ZCODE_TRIAL_ENDED: &str = r#"{"type":"error","error":{"type":"invalid_request_error","message":"ZCode has no active limited quota for GLM-5.3-Trial"}}"#;
    const ZCODE_TRIAL_SPENT: &str = r#"{"type":"error","error":{"type":"rate_limit_error","message":"ZCode's limited quota for glm-5.3 is exhausted or temporarily unavailable"}}"#;
    // zcode 0.1.49 把原因放词前（": " 分支接住）；0.1.48 把注记放词后（判不中，
    // 账号整体 rest——有意的行为差）
    const ZCODE_TRIAL_RESTED: &str = r#"{"type":"error","error":{"type":"rate_limit_error","message":"ZCode said the last request for GLM-5.3 was over its quota, so it is not asked again for a minute: ZCode's limited quota for GLM-5.3 is exhausted or temporarily unavailable"}}"#;
    const ZCODE_TRIAL_RESTED_048: &str = r#"{"type":"error","error":{"type":"rate_limit_error","message":"ZCode's limited quota for GLM-5.3 is exhausted or temporarily unavailable (ZCode answered 429 to the last request for it, so it is not asked again for a minute)"}}"#;

    #[test]
    fn trial_refused_matches_the_go_word_table() {
        assert!(trial_refused(ZCODE_FLASH_TRIAL_ENDED));
        assert!(trial_refused(ZCODE_TRIAL_ENDED));
        assert!(trial_refused(ZCODE_TRIAL_SPENT));
        assert!(trial_refused(ZCODE_TRIAL_RESTED), "0.1.49 的话：': ' 前缀分支");
        // 0.1.48 的话跟在词后：词表到串尾为止，判不中（Go 同侧 rest 整账号）
        assert!(!trial_refused(ZCODE_TRIAL_RESTED_048));
    }

    #[test]
    fn trial_refused_ignores_account_level_refusals() {
        // 账号级的额度/钥匙拒答不是 trial 词：Go 里它们 rest 整个账号
        assert!(!trial_refused(r#"{"error":{"type":"rate_limit_error","message":"quota exhausted"}}"#));
        assert!(!trial_refused(
            r#"{"error":{"type":"invalid_request_error","message":"quota exhausted for this account"}}"#
        ));
        assert!(!trial_refused(r#"{"error":{"message":"invalid api key"}}"#));
        // 405 拦截页 / 风控 3012 的话不是本词表（Go 走 zcodeTrialBlocked /
        // zcodeTrialRestricted，词表在 provider.EdgeBlocked / failRestricted）
        assert!(!trial_refused(
            r#"{"type":"error","error":{"type":"permission_error","message":"405 Method Not Allowed — the provider's network firewall blocked requests from this IP; wait a while, or switch to another network or proxy"}}"#
        ));
        assert!(!trial_refused(r#"{"ok":true}"#));
        assert!(!trial_refused(""));
    }

    #[test]
    fn trial_refused_takes_provider_prefix_and_raw_text() {
        // 中继加的 "<provider>: " 前缀
        assert!(trial_refused(
            r#"{"error":{"message":"zcode: ZCode has no active limited quota for GLM-5.3-Trial"}}"#
        ));
        // 非 JSON 体：vendorMessage 整串兜底（Go 同款）
        assert!(trial_refused("ZCode has no active limited quota for GLM-5.3-Trial"));
        assert!(trial_refused("ZCode's limited quota for glm-5.3 is exhausted or temporarily unavailable"));
        // 大小写敏感（Go lazyre 无 (?i)）
        assert!(!trial_refused("zcode has no active limited quota for GLM-5.3-Trial"));
        // (\S+) 要非空且无空白：空词、带空格的词都不是
        assert!(!trial_refused("ZCode has no active limited quota for "));
        assert!(!trial_refused("ZCode has no active limited quota for GLM 5.3"));
        assert!(!trial_refused("ZCode's limited quota for glm 5.3 is exhausted or temporarily unavailable"));
    }

    #[test]
    fn trial_words_captures_form_and_model() {
        // ended 点名 -Trial 全名；spent 点名去 -Trial 的桶模型（EqualFold 核对
        // 在 trial_refused_for）
        assert_eq!(
            trial_words("ZCode has no active limited quota for GLM-5.3-Flash-Trial"),
            Some((TrialForm::Ended, "GLM-5.3-Flash-Trial".to_string()))
        );
        assert_eq!(
            trial_words("ZCode's limited quota for glm-5.3 is exhausted or temporarily unavailable"),
            Some((TrialForm::Spent, "glm-5.3".to_string()))
        );
        assert_eq!(trial_words("quota exhausted"), None);
    }

    /// Go TestZCodeTrialRefusalRestsTheTrialAlone（zcode_trial_test.go:45）的
    /// 纯函数面：四道门（来源/候选/status/点名核对）全过才认，任一不过 =
    /// None（调用方走原有 rest 行为）。
    #[test]
    fn trial_refused_for_gates_on_provider_model_status_and_naming() {
        let f = |p: &str, m: &str, s: u16, b: &str| trial_refused_for(p, m, s, b.as_bytes());
        // 全过：ended（400）点名 -Trial 全名 + 候选同名
        assert_eq!(f("zcode", "GLM-5.3-Flash-Trial", 400, ZCODE_FLASH_TRIAL_ENDED), Some(TrialForm::Ended));
        assert_eq!(f("zcode", "GLM-5.3-Trial", 400, ZCODE_TRIAL_ENDED), Some(TrialForm::Ended));
        // EqualFold：候选与点名两侧的大小写都不敏感
        assert_eq!(f("zcode", "glm-5.3-trial", 400, ZCODE_TRIAL_ENDED), Some(TrialForm::Ended));
        // Go PluginID 的撞名变体（plugins.go:102-110）也是 zcode 插件的账号
        assert_eq!(f("zcode-plugin", "GLM-5.3-Trial", 400, ZCODE_TRIAL_ENDED), Some(TrialForm::Ended));
        // 全过：spent（429）点名去 -Trial 的桶模型，核对 m[1]+"-Trial"
        assert_eq!(f("zcode", "GLM-5.3-Trial", 429, ZCODE_TRIAL_SPENT), Some(TrialForm::Spent));
        assert_eq!(f("zcode", "GLM-5.3-Trial", 429, ZCODE_TRIAL_RESTED), Some(TrialForm::Spent));
        // 词表门：0.1.48 注记跟词后判不中 → None（Go 同侧 rest 整账号）
        assert_eq!(f("zcode", "GLM-5.3-Trial", 429, ZCODE_TRIAL_RESTED_048), None);
        // 点名核对：ended 词名 GLM-5.3-Trial，候选是 Flash-Trial → 不认
        assert_eq!(f("zcode", "GLM-5.3-Flash-Trial", 400, ZCODE_TRIAL_ENDED), None);
        // 点名核对：spent 词名 glm-5.3（glm-5.3-Trial ≠ Flash-Trial）→ 不认
        assert_eq!(f("zcode", "GLM-5.3-Flash-Trial", 429, ZCODE_TRIAL_SPENT), None);
        // status 门：ended 词配 429 / spent 词配 400 / 405 拦截页都不认
        assert_eq!(f("zcode", "GLM-5.3-Trial", 429, ZCODE_TRIAL_ENDED), None);
        assert_eq!(f("zcode", "GLM-5.3-Trial", 400, ZCODE_TRIAL_SPENT), None);
        assert_eq!(f("zcode", "GLM-5.3-Trial", 405, ZCODE_TRIAL_ENDED), None);
        // 来源门：别的 provider 同词同状态不认
        assert_eq!(f("factory", "GLM-5.3-Trial", 400, ZCODE_TRIAL_ENDED), None);
        // 候选门：plain 模型（不带 -Trial）不进判定——账号级拒答照旧整账号
        assert_eq!(
            f("zcode", "GLM-5.3", 429, r#"{"error":{"type":"rate_limit_error","message":"quota exhausted"}}"#),
            None
        );
    }

    #[test]
    fn vendor_message_reads_the_json_shapes_go_does() {
        // errors.message（ModelScope）
        assert_eq!(
            vendor_message(r#"{"errors":{"message":"ModelScope says no"}}"#),
            "ModelScope says no"
        );
        // msg + code（WorkBuddy）；code 为 "0"/"null"/缺省时不带后缀
        assert_eq!(vendor_message(r#"{"code":1113,"msg":"被限流"}"#), "被限流 (1113)");
        assert_eq!(vendor_message(r#"{"code":"0","msg":"正常"}"#), "正常");
        assert_eq!(vendor_message(r#"{"msg":"plain"}"#), "plain");
        // error 是字符串
        assert_eq!(vendor_message(r#"{"error":"nope"}"#), "nope");
        // error 字段在时 msg 不看（Go：len(e.Error)==0 才看 msg）
        assert_eq!(vendor_message(r#"{"error":{"message":"e"},"msg":"m"}"#), "e");
        // 都读不到：整串兜底
        assert_eq!(vendor_message(r#"{"code":200}"#), r#"{"code":200}"#);
    }

    #[test]
    fn small_helpers_follow_the_plugin() {
        assert_eq!(span_of(3, 0), 5 * 3600, "团队五小时不带 number");
        assert_eq!(span_of(6, 1), 7 * 86400);
        assert_eq!(window_name(0), "Credits");
        assert_eq!(window_name(5 * 3600), "5 hours");
        assert_eq!(window_name(7 * 86400), "Weekly");
        assert_eq!(window_name(30 * 86400), "Monthly");
        assert_eq!(window_name(3 * 86400), "3 days");
        assert_eq!(compact(400.0), "400");
        assert_eq!(compact(12.5), "12.5");
        assert_eq!(compact(1.0 / 3.0), "0.33");
        assert_eq!(uuid_of("a"), uuid_of("a"));
        assert_ne!(uuid_of("a"), uuid_of("b"));
        assert_eq!(uuid_of("a").len(), 36);
        assert_eq!(plan_name("pro"), "GLM Coding Pro");
        assert_eq!(plan_name(""), "");
        assert_eq!(biz_root("https://open.bigmodel.cn/api/anthropic"), BIGMODEL_API);
        assert_eq!(biz_root("https://api.z.ai/api/anthropic"), "https://api.z.ai");
    }
}
