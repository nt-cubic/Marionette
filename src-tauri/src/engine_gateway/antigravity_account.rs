//! Google Antigravity（Gemini 订阅）账号：只读额度行 + Code Assist 对话桥与模型
//! 目录——magpie google.go 里 `retrieveUserQuotaSummary` 一段、antigravity 路径的
//! 项目/档位/清单（`provider/antigravity_models.go` 的家族折叠）与
//! `gateway/codeassist.go` 账号段的最小移植。线协议两侧在
//! `antigravity_codeassist.rs`（请求）与 `antigravity_stream.rs`（应答）。
//!
//! 范围：
//! - 凭据发现：`$GEMINI_HOME|~/.gemini` 下的 `antigravity-acp/acp_token.json`
//!   （agy_acp_server 自己的登录文件）。字段 client_id / client_secret /
//!   refresh_token / token_uri / project_id，缺一不出行。
//! - access token 在内存里用 refresh_token 换新（Google 的 refresh token
//!   不轮换，无需写回；缓存 45 分钟）。**红线（照 grok_account.rs，只读）**：
//!   绝不写/轮换/刷新任何凭据文件，绝不拉起 agy 二进制；令牌值永不进
//!   日志/错误文本/测试输出。token 换新失败就安静跳过。
//! - 额度：POST `https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary`
//!   `{"project": <凭据里的 project_id>}`（antigravity 走 daily 基址，agy
//!   onboard.py 的 consumer 路径同款），应答 `groups[].buckets[]` 折成窗口，
//!   名字带族 `"Claude & GPT · 5 hours"`——族名归一（去 " models" 尾缀、
//!   and→&）与原生 `/usage` 解析同口径，前端 quotaDisplaySections 靠它拆族。
//!   **请求头必须用 agy 自己的 aidev User-Agent**（见 USER_AGENT 处注释）：
//!   magpie 的扁平形状 Google 只回 3P 一组，Gemini / Argon 组整个缺失
//!   （2026-11-21 oracle 对真账号实测）。
//!
//! 展示面：没登录 / 请求失败 / 读不懂 → None，安静跳过；用户自建同名
//! `antigravity` 来源让位。解析宽容：少组少桶不是错。

use std::io::Read;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::store::Provider;

pub const AGY_ID: &str = "antigravity";
const QUOTA_NAME: &str = "Google Antigravity";
/// magpie google.go：antigravity 的额度走 daily 基址（prod 只做 loadCodeAssist）。
const SUMMARY_URL: &str =
    "https://daily-cloudcode-pa.googleapis.com/v1internal:retrieveUserQuotaSummary";
/// **UA 形状决定应答里有哪些组（2026-11-21 oracle 实锤）**：额度端点按
/// UA 解析 client type（agy useragent.py 注释里的 extractClientType）——
/// magpie 的扁平形状（`antigravity/hub/1.5.0 windows/x86_64`）同 token 同
/// project 只回 3P 一组（Claude & GPT），agy 自己的 aidev 结构形状回齐
/// 三组（Gemini Models / Claude & GPT / Gemini 4 Argon）。所以这里必须
/// 逐字镜像 agy `useragent.build_user_agent` 的默认形状（surface 落
/// unknown/unknown 是 agy 自己的 fallback），版本字段端点不校验。
const USER_AGENT: &str = "antigravity/acp/1.5.0 (aidev_client; os_type=windows; arch=amd64; host_path=unknown/unknown; proxy_client=antigravity/sdk)";

const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// access token 通常 1 小时；45 分钟就换新，别贴边。
const TOKEN_TTL: Duration = Duration::from_secs(45 * 60);

// ─── 测试缝（cfg(test)）：额度端点与凭据目录都指到沙箱，测试默认读不到
// 真实账号。与 grok_account 的缝同款形状但各自独立；两种测试都持有
// TEST_SANDBOX_LOCK 串行，不会互相踩。────────────────────────────────────
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct Seam {
    pub base: String,
    pub home: std::path::PathBuf,
}

#[cfg(test)]
pub(crate) static TEST_SEAM: Mutex<Option<Seam>> = Mutex::new(None);

#[cfg(test)]
fn seam() -> Option<Seam> {
    TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

#[cfg(test)]
fn summary_url() -> String {
    // 缝里是 mock 的源地址；测试 URL 现拼同款路径（prod 恒 SUMMARY_URL）。
    format!("{}/v1internal:retrieveUserQuotaSummary", seam().map(|s| s.base).unwrap_or_default())
}

#[cfg(not(test))]
fn summary_url() -> String {
    SUMMARY_URL.to_string()
}

/// 凭据目录：`$GEMINI_HOME` 优先（Marionette 给 agent 设的 .gemini 目录本身），
/// 否则 `~/.gemini`；登录文件在 `antigravity-acp/acp_token.json`。
#[cfg(not(test))]
fn credential_path() -> Option<std::path::PathBuf> {
    let base = match std::env::var_os("GEMINI_HOME").filter(|h| !h.is_empty()) {
        Some(h) => std::path::PathBuf::from(h),
        None => home_dir()?.join(".gemini"),
    };
    Some(base.join("antigravity-acp").join("acp_token.json"))
}

#[cfg(test)]
fn credential_path() -> Option<std::path::PathBuf> {
    Some(seam()?.home.join(".gemini").join("antigravity-acp").join("acp_token.json"))
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|h| !h.is_empty())
        .map(std::path::PathBuf::from)
}

struct Credential {
    client_id: String,
    client_secret: String,
    refresh_token: String,
    token_uri: String,
    project_id: String,
}

fn trim_non_empty(v: &Value, key: &str) -> Option<String> {
    let s = v.get(key)?.as_str()?.trim();
    if s.is_empty() {
        None
    } else {
        Some(s.to_string())
    }
}

fn read_credential() -> Option<Credential> {
    let path = credential_path()?;
    let bytes = std::fs::read(path).ok()?;
    let raw: Value = serde_json::from_slice(&bytes).ok()?;
    Some(Credential {
        client_id: trim_non_empty(&raw, "client_id")?,
        client_secret: trim_non_empty(&raw, "client_secret")?,
        refresh_token: trim_non_empty(&raw, "refresh_token")?,
        token_uri: trim_non_empty(&raw, "token_uri")?,
        project_id: trim_non_empty(&raw, "project_id")?,
    })
}

/// 本机有没有 Antigravity（Gemini 订阅）登录：只看登录文件读不读得出凭据，
/// 不换 token、不联网。模型共享面板「订阅共享」的已登录判断走这里——与额度行
/// 同一份凭据，面板说已登录，额度行就一定拿得到账号。
pub(crate) fn signed_in() -> bool {
    read_credential().is_some()
}

static ACCESS_TOKEN: Mutex<Option<(Instant, String)>> = Mutex::new(None);

/// access token：内存缓存 45 分钟，过期用 refresh_token 现换
/// （grant_type=refresh_token，表单编码）。失败清缓存返回 None。
fn access_token(cred: &Credential) -> Option<String> {
    if let Ok(guard) = ACCESS_TOKEN.lock() {
        if let Some((at, token)) = guard.as_ref() {
            if at.elapsed() < TOKEN_TTL {
                return Some(token.clone());
            }
        }
    }
    let response = super::outbound_agent()
        .post(&cred.token_uri)
        .timeout(FETCH_TIMEOUT)
        .send_form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", cred.refresh_token.as_str()),
            ("client_id", cred.client_id.as_str()),
            ("client_secret", cred.client_secret.as_str()),
        ])
        .ok()?;
    let mut buf = Vec::new();
    response.into_reader().take(1 << 20).read_to_end(&mut buf).ok()?;
    let data: Value = serde_json::from_slice(&buf).ok()?;
    let token = trim_non_empty(&data, "access_token")?;
    if let Ok(mut guard) = ACCESS_TOKEN.lock() {
        *guard = Some((Instant::now(), token.clone()));
    }
    Some(token)
}

/// magpie `poolName`：族名去 " models"/" model" 尾缀、and→&。
/// "Gemini Models" → "Gemini"，"Claude and GPT models" → "Claude & GPT"。
fn pool_name(s: &str) -> String {
    let trimmed = s.trim();
    let lower = trimmed.to_ascii_lowercase();
    let stripped = if let Some(head) = lower
        .strip_suffix(" models")
        .or_else(|| lower.strip_suffix(" model"))
    {
        trimmed[..head.len()].trim_end()
    } else {
        trimmed
    };
    stripped.replace(" and ", " & ")
}

/// magpie pools() 的窗口名：window 字段是干净口径（5h / weekly），
/// displayName 是给人看的（"Weekly Limit Remaining"）。
fn bucket_period(b: &Value) -> Option<String> {
    match b
        .get("window")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "5h" => Some("5 hours".into()),
        "weekly" => Some("7 days".into()),
        _ => {
            for key in ["displayName", "window", "bucketId"] {
                if let Some(s) = trim_non_empty(b, key) {
                    return Some(s);
                }
            }
            None
        }
    }
}

/// 摘要 → 窗口行。名字带族（"Claude & GPT · 5 hours"）；没有可读族名的组
/// 退回裸周期名。used = (1 - remainingFraction) × 100（缺席按 magpie 当满用）。
fn windows_from_summary(data: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    let Some(groups) = data.get("groups").and_then(Value::as_array) else {
        return out;
    };
    for group in groups {
        let pool = group
            .get("displayName")
            .and_then(Value::as_str)
            .map(pool_name)
            .unwrap_or_default();
        let Some(buckets) = group.get("buckets").and_then(Value::as_array) else {
            continue;
        };
        for bucket in buckets {
            if bucket.get("disabled").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            let Some(period) = bucket_period(bucket) else { continue };
            let fraction = bucket
                .get("remainingFraction")
                .and_then(Value::as_f64)
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            let used = (1.0 - fraction) * 100.0;
            let mut window = json!({
                "name": if pool.is_empty() { period.clone() } else { format!("{pool} · {period}") },
                "used": (used * 10.0).round() / 10.0,
                "remaining": (fraction * 1000.0).round() / 10.0,
            });
            if let Some(reset) = trim_non_empty(bucket, "resetTime") {
                window["resetsAt"] = json!(reset);
            }
            out.push(window);
        }
    }
    out
}

/// 订阅额度行。没登录 / 换新失败 / 请求失败 / 读不懂 → None（展示面，
/// 安静跳过）；用户自建同名来源让位。
pub fn quota_row(providers: &[Provider]) -> Option<Value> {
    if providers.iter().any(|p| p.id == AGY_ID) {
        return None;
    }
    let cred = read_credential()?;
    let token = access_token(&cred)?;
    let response = super::outbound_agent()
        .post(&summary_url())
        .timeout(FETCH_TIMEOUT)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .set("User-Agent", USER_AGENT)
        .send_string(&json!({ "project": cred.project_id }).to_string())
        .ok()?;
    let mut buf = Vec::new();
    response.into_reader().take(1 << 20).read_to_end(&mut buf).ok()?;
    let data: Value = serde_json::from_slice(&buf).ok()?;
    let windows = windows_from_summary(&data);
    if windows.is_empty() {
        return None;
    }
    Some(json!({
        "provider": AGY_ID,
        "name": QUOTA_NAME,
        "kind": "subscription",
        "windows": windows,
    }))
}

// =====================================================================
// Code Assist：对话桥 + 模型目录
// =====================================================================
//
// 「Gemini 订阅」这一族在引擎里的来源 id 是 `antigravity`：凭据是上面那份
// agy_acp_server 的登录文件，对话走 Google Code Assist 的 Antigravity 线协议
// （`v1internal:streamGenerateContent`；请求侧 antigravity_codeassist.rs、应答侧
// antigravity_stream.rs），模型清单走 `fetchAvailableModels`。逐条照 magpie
// `internal/provider/google.go` 的 antigravity 路径、`antigravity_models.go` 与
// `gateway/codeassist.go` 的账号段移植。
//
// 模型 id 有两层：Antigravity 把同一模型的每个思考档位列成一行
// （gemini-3.7-flash-low / -medium / -high，名字 "Gemini 3.7 Flash (Low)" …），
// 目录把它们折成一个模型 + 档位表（`collapse`）；出站时再按强度换回
// Antigravity 自己的变体 id（`sent_id`）。
//
// 基址分工（google.go codeAssistProd / codeAssistDaily）：loadCodeAssist 走
// prod，对话与 fetchAvailableModels 走 daily。Code Assist 的 UA 是 magpie 的
// 扁平形状 `antigravity/hub/<版本> <os>/<arch>`——**与额度行的 aidev 形状不是
// 同一个**（额度端点按 UA 决定回哪几组，见 USER_AGENT 注释；对话与清单端点
// 用的是 magpie 实证的形状）。
//
// 红线（同上，只读账号）：绝不写/轮换任何凭据文件、绝不拉起 agy 二进制；
// 令牌值永不进日志/错误文本/测试输出；单账号世界，换 token 失败 = 如实 401，
// 不换账号。联网一律在 STATE 锁外（抓清单要几秒，对话路径的 knows/sent_id
// 同时要读目录）。

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use super::antigravity_codeassist::{
    cloak_system, cloak_user_turns, refuses, reword, split_antigravity_id, turned_away_hint, turns_away,
};
use super::antigravity_stream::ChatStream;
use super::catalog::VendorRow;
use super::effort::Proto;
use super::httpd::{Reply, Request};

/// 订阅共享的族名（engine.rs `SUBSCRIPTION_FAMILIES` 里的 gemini——主程序推进
/// 网关闸的是族名，不是来源 id）。
const FAMILY: &str = "gemini";
const CODE_ASSIST_PROD: &str = "https://cloudcode-pa.googleapis.com";
const CODE_ASSIST_DAILY: &str = "https://daily-cloudcode-pa.googleapis.com";
const VERSION_URL: &str =
    "https://antigravity-hub-auto-updater-974169037036.us-central1.run.app/manifest/latest-arm64-mac.yml";
/// 内置的 Antigravity 版本（更新器清单问不到时用它，magpie 同值）。
const DEFAULT_VERSION: &str = "2.9.1";
const VERSION_TTL: Duration = Duration::from_secs(6 * 60 * 60);
/// 档位（loadCodeAssist）多久问一次（google.go googleTiersFor）。
const TIERS_TTL: Duration = Duration::from_secs(60 * 60);
/// Google 对某个档位回过 400 之后，多久内不再点名（tierRefusedFor）。
const TIER_REFUSED_FOR: Duration = Duration::from_secs(24 * 60 * 60);
const CACHE_TTL: Duration = Duration::from_secs(30);
const FAIL_BACKOFF: Duration = Duration::from_secs(60);
const SIGNED_OUT: &str = "Antigravity is signed out; sign in to Antigravity once and try again";

/// Code Assist 的两个基址：prod 只做 loadCodeAssist，daily 做对话与模型清单。
#[cfg(not(test))]
fn code_assist_bases() -> (String, String) {
    (CODE_ASSIST_PROD.to_string(), CODE_ASSIST_DAILY.to_string())
}

#[cfg(test)]
fn code_assist_bases() -> (String, String) {
    // 缝里是 mock 的源地址：两个基址同一个，mock 按路径分流；没挂缝（真机
    // oracle 单独跑）= 生产基址。
    match seam() {
        Some(s) => (s.base.clone(), s.base),
        None => (CODE_ASSIST_PROD.to_string(), CODE_ASSIST_DAILY.to_string()),
    }
}

static VERSION: Mutex<Option<(Instant, String)>> = Mutex::new(None);

/// Go 的 os/arch 叫法（runtime.GOOS / GOARCH）。
fn go_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    }
}

fn go_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    }
}

fn hub_user_agent() -> String {
    format!("antigravity/hub/{} {}/{}", antigravity_version(), go_os(), go_arch())
}

/// 最新 Antigravity 的版本（antigravityVersion，google.go:481）：问它的更新器，
/// 至多每 6 小时一次；问不到用上次的，再没有用内置的。
#[cfg(not(test))]
fn antigravity_version() -> String {
    {
        let guard = VERSION.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((at, v)) = guard.as_ref() {
            if at.elapsed() < VERSION_TTL {
                return v.clone();
            }
        }
    }
    let mut current = {
        let mut guard = VERSION.lock().unwrap_or_else(|p| p.into_inner());
        let v = guard
            .as_ref()
            .map(|(_, v)| v.clone())
            .unwrap_or_else(|| DEFAULT_VERSION.to_string());
        *guard = Some((Instant::now(), v.clone()));
        v
    };
    if let Some(v) = fetch_latest_version() {
        current = v;
        if let Ok(mut guard) = VERSION.lock() {
            if let Some((_, kept)) = guard.as_mut() {
                *kept = current.clone();
            }
        }
    }
    current
}

/// 测试构建不联网：恒为内置版本。
#[cfg(test)]
fn antigravity_version() -> String {
    DEFAULT_VERSION.to_string()
}

/// 更新器清单里的 `version:` 行；只认纯数字和点的版本串。
#[cfg(not(test))]
fn fetch_latest_version() -> Option<String> {
    let response = super::outbound_agent()
        .get(VERSION_URL)
        .set("User-Agent", "electron-builder")
        .timeout(Duration::from_secs(5))
        .call()
        .ok()?;
    let body = read_body(response, 64 << 10)?;
    let text = String::from_utf8_lossy(&body);
    let line = text.lines().find(|l| l.starts_with("version:"))?;
    let v = line["version:".len()..].trim().trim_matches(|c| c == '"' || c == '\'').to_string();
    (!v.is_empty() && v.bytes().all(|b| b.is_ascii_digit() || b == b'.')).then_some(v)
}

fn read_body(response: ureq::Response, cap: u64) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    response.into_reader().take(cap).read_to_end(&mut buf).ok()?;
    Some(buf)
}

// ---------------------------------------------------------------------
// 账号态：项目与档位
// ---------------------------------------------------------------------

/// Code Assist 对一次调用的非 200 应答（codeAssistError）：状态码 + 原话。
#[derive(Debug)]
struct Refused {
    status: u16,
    said: String,
}

/// Google 的错误体 `{"error":{"message":…}}` 里的话；读不出就给截短的原文。
fn api_error(body: &[u8], status: u16) -> String {
    if let Ok(v) = serde_json::from_slice::<Value>(body) {
        for p in ["/error/message", "/error", "/message"] {
            if let Some(s) = v.pointer(p).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
                return s.to_string();
            }
        }
    }
    let text = String::from_utf8_lossy(body);
    let t = text.trim();
    if t.is_empty() {
        format!("HTTP {status}")
    } else {
        t.chars().take(500).collect()
    }
}

/// POST `{base}/v1internal:<method>`（googleAccount.call，google.go:517）：200 →
/// 应答 JSON；其它 → [`Refused`]。元数据类调用 10s 超时（对话流另走
/// [`generate`]，不设整单超时——长思考回合不能被掐）。
fn code_assist(cred: &Credential, base: &str, method: &str, body: &Value) -> Result<Value, Refused> {
    let token = access_token(cred).ok_or_else(|| Refused { status: 401, said: SIGNED_OUT.to_string() })?;
    let refused = |status: u16, body: &[u8]| {
        Refused { status, said: format!("Antigravity: {}", api_error(body, status)) }
    };
    let response = super::outbound_agent()
        .post(&format!("{}/v1internal:{method}", base.trim_end_matches('/')))
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {token}"))
        .set("User-Agent", &hub_user_agent())
        .timeout(FETCH_TIMEOUT)
        .send_string(&body.to_string());
    let response = match response {
        Ok(r) => r,
        Err(ureq::Error::Status(code, r)) => {
            return Err(refused(code, &read_body(r, 1 << 20).unwrap_or_default()));
        }
        Err(e) => return Err(Refused { status: 502, said: format!("Antigravity: {e}") }),
    };
    let buf = read_body(response, 4 << 20)
        .ok_or_else(|| Refused { status: 502, said: "Antigravity: reply could not be read".to_string() })?;
    serde_json::from_slice(&buf)
        .map_err(|_| Refused { status: 502, said: "Antigravity: reply is not JSON".to_string() })
}

/// 账号的 Cloud 项目与档位（googleProject）。
#[derive(Clone, Debug, Default, PartialEq)]
struct Project {
    id: String,
    /// 账号所在的档（"free-tier" / "standard-tier"）与付费的 Google One 档
    /// （"g1-pro-tier"）；不知道 = ""。
    tier: String,
    paid: String,
    /// 上次问 loadCodeAssist 的时刻。
    at: Option<Instant>,
}

impl Project {
    /// fetchAvailableModels 的 `entitlement.userTier` 该点名的档位，好的在前
    /// （entitlements，google.go:414）：免费档与 legacy 不点名（Google 会拒）。
    fn entitlements(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for t in [&self.paid, &self.tier] {
            if !t.is_empty() && t != "free-tier" && t != "legacy-tier" && !out.contains(t) {
                out.push(t.clone());
            }
        }
        out
    }
}

fn tier_key(project: &str, tier: &str) -> String {
    format!("{project}\0{tier}")
}

/// 一次清单抓取要读写的账号态：从 STATE 里克隆出来、抓完并回去——抓取要联网，
/// 绝不持锁等网络。
#[derive(Clone, Default)]
struct Scratch {
    project: Option<Project>,
    /// Google 对哪个 (项目, 档位) 回过 400，什么时候（tierRefused）。
    refused: HashMap<String, Instant>,
}

/// 账号的项目与档位（antigravityProject + antigravityTiers）：项目就是登录文件里
/// 的 project_id（一直是它）；档位问 loadCodeAssist（prod 基址），1 小时问一次，
/// 问不到就不带档位去要清单、1 小时后再问。只有 401（没登录态）往上抛。
fn project_of(cred: &Credential, scratch: &mut Scratch, prod: &str) -> Result<Project, Refused> {
    if let Some(p) = &scratch.project {
        if p.at.is_some_and(|at| at.elapsed() < TIERS_TTL) {
            return Ok(p.clone());
        }
    }
    let mut p = Project { id: cred.project_id.clone(), ..Default::default() };
    let body = json!({ "metadata": { "ideType": "ANTIGRAVITY" } });
    match code_assist(cred, prod, "loadCodeAssist", &body) {
        Ok(load) => {
            let tier = |k: &str| {
                load.pointer(&format!("/{k}/id")).and_then(Value::as_str).unwrap_or("").to_string()
            };
            p.paid = tier("paidTier");
            p.tier = tier("currentTier");
        }
        Err(e) if e.status == 401 => return Err(e),
        Err(_) => {}
    }
    p.at = Some(Instant::now());
    scratch.project = Some(p.clone());
    Ok(p)
}

/// fetchAvailableModels 一页（modelInfo，google.go:1041）：`tier` 非空时点名
/// `entitlement.userTier`，照 Antigravity 自己的问法。
fn fetch_models_page(cred: &Credential, daily: &str, project: &str, tier: &str) -> Result<Value, Refused> {
    let mut body = json!({ "project": project });
    if !tier.is_empty() {
        body["entitlement"] = json!({ "userTier": tier });
    }
    code_assist(cred, daily, "fetchAvailableModels", &body)
}

/// 这次该点名的档位，好的在前，最后不带（askTiers）：去掉 24h 内被 Google 拒过的。
fn ask_tiers(scratch: &Scratch, project: &Project) -> Vec<String> {
    let mut out: Vec<String> = project
        .entitlements()
        .into_iter()
        .filter(|t| {
            !scratch
                .refused
                .get(&tier_key(&project.id, t))
                .is_some_and(|at| at.elapsed() < TIER_REFUSED_FOR)
        })
        .collect();
    out.push(String::new());
    out
}

// ---------------------------------------------------------------------
// 模型目录：清单 → 家族折叠 → 目录行
// ---------------------------------------------------------------------

/// Antigravity 清单里的一行（googleModelInfo 的目录部分）。
#[derive(Clone, Debug, Default, PartialEq)]
struct ModelInfo {
    id: String,
    name: String,
    context: u64,
    output: u64,
    /// None = Antigravity 没说收不收图。
    supports_images: Option<bool>,
    /// 在 Antigravity 模型选择器里的位置，从 1 起；0 = 不在里面。
    picker: usize,
}

/// 一个目录模型（catalog.Model 的目录字段）。
#[derive(Clone, Debug, Default, PartialEq)]
struct Model {
    id: String,
    name: String,
    context: u64,
    output: u64,
    images: bool,
    /// 这个模型能要的思考档位，低 → 高。
    efforts: Vec<String>,
}

/// 一个家族：同一个 id 加各档位（antigravityFamily）。
#[derive(Clone)]
struct Family {
    id: String,
    variants: Vec<Model>,
    /// 各变体的档位，没带强度词的是 ""。
    efforts: Vec<String>,
}

/// Antigravity 列了但不是给对话用的（antigravityHidden）。
const HIDDEN: [&str; 6] = [
    "chat_20706",
    "chat_23310",
    "tab_flash_lite_preview",
    "tab_jump_flash_lite_preview",
    "gemini-2.5-flash-thinking",
    "gemini-2.5-pro",
];

/// Antigravity 不说时它提供的模型（antigravityModels）。
const FALLBACK: [(&str, &str); 7] = [
    ("gemini-pro-agent", "Gemini 3.1 Pro (High)"),
    ("gemini-3.1-pro-low", "Gemini 3.1 Pro (Low)"),
    ("gemini-3-flash", "Gemini 3 Flash"),
    ("gemini-3.1-flash-lite", "Gemini 3.1 Flash Lite"),
    ("claude-opus-4-6-thinking", "Claude Opus 4.6 (Thinking)"),
    ("claude-sonnet-4-6", "Claude Sonnet 4.6"),
    ("gpt-oss-120b-medium", "GPT-OSS 120B (Medium)"),
];

fn fallback_name(id: &str) -> &'static str {
    FALLBACK.iter().find(|(i, _)| *i == id).map(|(_, n)| *n).unwrap_or("")
}

/// 兜底表（fallbackModels）：gpt-oss 以外都收图。
fn fallback_models() -> Vec<Model> {
    FALLBACK
        .iter()
        .map(|(id, name)| Model {
            id: id.to_string(),
            name: name.to_string(),
            images: !id.starts_with("gpt-oss"),
            ..Default::default()
        })
        .collect()
}

/// 名字最后括号里说的档位，与去掉它的名字（antigravityNameEffort，:52）。
fn name_effort(name: &str) -> (String, String) {
    let Some(open) = name.rfind(" (") else { return (name.to_string(), String::new()) };
    if !name.ends_with(')') {
        return (name.to_string(), String::new());
    }
    let level = match name[open + 2..name.len() - 1].to_ascii_lowercase().as_str() {
        "minimal" | "extra low" => "minimal",
        "low" => "low",
        "medium" => "medium",
        "high" => "high",
        "extra high" | "xhigh" => "xhigh",
        _ => return (name.to_string(), String::new()),
    };
    (name[..open].trim().to_string(), level.to_string())
}

/// 名字（去掉括号里的档位）是不是这个 id 的家族写出来（antigravityNameSpells，
/// :79）："Gemini 3.5 Flash Lite" 拼作 gemini-3.5-flash-lite。
fn name_spells(name: &str, id: &str) -> bool {
    let (n, _) = name_effort(name);
    let n = n.split_whitespace().collect::<Vec<_>>().join("-");
    !n.is_empty() && n.eq_ignore_ascii_case(split_antigravity_id(id).0)
}

/// Antigravity 的枚举名点的是不是这个 id 本身（antigravityEnumNames，:88）：
/// MODEL_GOOGLE_GEMINI_2_5_FLASH 是 gemini-2.5-flash；它替别的模型起的名是
/// MODEL_PLACEHOLDER_M…。
fn enum_names(enum_name: &str, id: &str) -> bool {
    match enum_name.strip_prefix("MODEL_GOOGLE_") {
        Some(rest) if !rest.is_empty() => rest.eq_ignore_ascii_case(&id.replace(['-', '.'], "_")),
        _ => false,
    }
}

/// fetchAvailableModels 的应答 → 清单行（modelInfo 的解析段，google.go:1108–:1191）：
/// 不给对话用的、被标 disabled 的留下；名字说的是别的模型的，改回它自己 id 的
/// 名字；位置按 agentModelSorts。先按 id 排（选择器的顺序在 [`antigravity_list`]
/// 里再排）。
fn parse_models_page(page: &Value) -> Vec<ModelInfo> {
    let Some(models) = page.get("models").and_then(Value::as_object) else { return Vec::new() };
    let mut out: Vec<ModelInfo> = Vec::new();
    for (id, m) in models {
        if HIDDEN.contains(&id.as_str()) || id.starts_with("chat_") || id.starts_with("tab_") {
            continue;
        }
        // Antigravity 把不给这个账号用的模型标 disabled，要它只会 404
        if m.get("disabled").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        out.push(ModelInfo {
            id: id.clone(),
            name: m.get("displayName").and_then(Value::as_str).unwrap_or("").to_string(),
            context: m.get("maxTokens").and_then(Value::as_u64).unwrap_or(0),
            output: m.get("maxOutputTokens").and_then(Value::as_u64).unwrap_or(0),
            supports_images: m.get("supportsImages").and_then(Value::as_bool),
            picker: 0,
        });
    }
    // Antigravity 把用户已经迁走的模型还列在迁去那个模型的名字下（gemini-2.5-flash
    // 与 gemini-2.5-flash-lite 都叫 "Gemini 3.5 Flash Lite"，真的 3.5 Flash Lite 也在），
    // 编辑器里三个芯片一个名（#1918）。枚举名点着自己 id 的、或同名里已有别的家族
    // 拼出来的，改回按它自己的 id 叫。
    let mut spelled: HashSet<String> = HashSet::new();
    for mi in &out {
        if name_spells(&mi.name, &mi.id) {
            spelled.insert(mi.name.clone());
        }
    }
    for mi in &mut out {
        if mi.name.is_empty() || name_spells(&mi.name, &mi.id) {
            continue;
        }
        let enum_name = models
            .get(&mi.id)
            .and_then(|m| m.get("model"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if spelled.contains(&mi.name) || enum_names(enum_name, &mi.id) {
            mi.name = super::presets::lookup(&mi.id).map(|p| p.display_name).unwrap_or_default();
        }
    }
    let mut rank: HashMap<&str, usize> = HashMap::new();
    let sorts = page
        .pointer("/agentModelSorts")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|s| s.get("groups").and_then(Value::as_array).into_iter().flatten());
    for group in sorts {
        for id in group.get("modelIds").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            if !rank.contains_key(id) {
                rank.insert(id, rank.len() + 1);
            }
        }
    }
    for mi in &mut out {
        mi.picker = rank.get(mi.id.as_str()).copied().unwrap_or(0);
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// 清单 → 目录模型（antigravityList，google.go:1256）：没名字的用兜底表的名字；
/// 收不收图按 Antigravity 说的，没说就 gpt-oss 以外都收；顺序照选择器（不在
/// 选择器里的排后），同位按 id。一行都没有 = 兜底表。
fn antigravity_list(infos: Vec<ModelInfo>) -> Vec<Model> {
    let mut infos = infos;
    infos.sort_by(|a, b| match (a.picker, b.picker) {
        (0, 0) => std::cmp::Ordering::Equal,
        (0, _) => std::cmp::Ordering::Greater,
        (_, 0) => std::cmp::Ordering::Less,
        (x, y) => x.cmp(&y),
    });
    let out: Vec<Model> = infos
        .into_iter()
        .map(|mi| Model {
            name: if mi.name.is_empty() { fallback_name(&mi.id).to_string() } else { mi.name },
            images: mi.supports_images.unwrap_or(!mi.id.starts_with("gpt-oss")),
            id: mi.id,
            context: mi.context,
            output: mi.output,
            efforts: Vec::new(),
        })
        .collect();
    if out.is_empty() {
        fallback_models()
    } else {
        out
    }
}

/// 一份清单的各家族，按清单里第一次出现的顺序（antigravityFamilies，:156）。
/// 一个变体的档位听它 id 的强度词；每个变体的名字都各说了一个档位时听名字的
/// （gemini-3.5-flash-extra-low 叫 "(Low)"、gemini-3.5-flash-low 叫 "(Medium)"）。
fn families_of(raw: &[Model]) -> Vec<Family> {
    let mut out: Vec<Family> = Vec::new();
    for m in raw {
        let (id, _) = split_antigravity_id(&m.id);
        if let Some(f) = out.iter_mut().find(|f| f.id == id) {
            f.variants.push(m.clone());
        } else {
            out.push(Family { id: id.to_string(), variants: vec![m.clone()], efforts: Vec::new() });
        }
    }
    for f in &mut out {
        let mut named: Vec<String> = Vec::new();
        for v in &f.variants {
            let (_, l) = name_effort(&v.name);
            if !l.is_empty() && !named.contains(&l) {
                named.push(l);
            }
        }
        let by_id: Vec<String> = f
            .variants
            .iter()
            .map(|v| split_antigravity_id(&v.id).1.to_string())
            .collect();
        let by_name: Vec<String> = f.variants.iter().map(|v| name_effort(&v.name).1).collect();
        f.efforts = (0..f.variants.len())
            .map(|i| if named.len() == f.variants.len() { by_name[i].clone() } else { by_id[i].clone() })
            .collect();
    }
    out
}

/// 各档位对应的 id，"" 是没要档位时发的那个（byEffort，:105）：家族不带强度词的
/// id，Antigravity 有的话；没有就 high（Gemini 3 没被告知档位时就想这么多，要
/// 了 none 的客户端拿到的是这个模型本来的样子）；再没有就最高的那个。
fn by_effort(f: &Family) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for (v, e) in f.variants.iter().zip(&f.efforts) {
        out.entry(e.clone()).or_insert_with(|| v.id.clone());
    }
    if !out.contains_key("") {
        let high = out.get("high").cloned().unwrap_or_default();
        out.insert(String::new(), high);
        for level in super::effort::LEVELS.iter().rev() {
            if let Some(id) = out.get(*level).cloned() {
                out.insert(String::new(), id);
                break;
            }
        }
    }
    out
}

/// 家族折成的一个模型（model，:124）：名字去掉档位、带它有的档位（低 → 高）、
/// 窗口/输出取各变体里最小的、收图取全部都收。
fn family_model(f: &Family) -> Model {
    let by = by_effort(f);
    let def = by.get("").cloned().unwrap_or_default();
    let mut m = f.variants.iter().find(|v| v.id == def).cloned().unwrap_or_default();
    m.id = f.id.clone();
    m.efforts.clear();
    m.name = name_effort(&m.name).0;
    if m.name.is_empty() || m.name == def {
        m.name = f.id.clone();
    }
    for v in &f.variants {
        if v.context > 0 && (m.context == 0 || v.context < m.context) {
            m.context = v.context;
        }
        if v.output > 0 && (m.output == 0 || v.output < m.output) {
            m.output = v.output;
        }
        m.images = m.images && v.images;
    }
    for l in super::effort::LEVELS {
        if f.efforts.iter().any(|e| e == l) {
            m.efforts.push(l.to_string());
        }
    }
    m
}

/// Antigravity 的清单，每个家族一行（collapseAntigravityModels，:198）：只有一个
/// 变体的家族保留它原来的 id 和名字。`-tiered` 的 id（一个 id 按请求的档位想）
/// 继承它所属模型的档位表；没名字的按 id 叫，tiered 的叫 "<家族> (Tiered)"。
fn collapse(raw: &[Model]) -> Vec<Model> {
    let mut out: Vec<Model> = Vec::new();
    for f in families_of(raw) {
        if f.variants.len() == 1 {
            out.push(f.variants[0].clone());
        } else {
            out.push(family_model(&f));
        }
    }
    let by_id: HashMap<String, Model> = out.iter().map(|m| (m.id.clone(), m.clone())).collect();
    for m in &mut out {
        let (base, tiered) = match m.id.strip_suffix("-tiered") {
            Some(b) => (b.to_string(), true),
            None => (m.id.clone(), false),
        };
        if tiered {
            if let Some(b) = by_id.get(&base) {
                if m.efforts.is_empty() {
                    m.efforts = b.efforts.clone();
                }
                if m.name.is_empty() && !b.name.is_empty() && b.name != base {
                    m.name = format!("{} (Tiered)", b.name);
                }
            }
        }
        if m.name.is_empty() {
            m.name = m.id.clone();
        }
    }
    out
}

/// 一份目录：Antigravity 自己的清单（折叠前）、它的各家族、目录行，和各 id 的
/// 档位表。
#[derive(Clone)]
struct Catalog {
    raw: Vec<Model>,
    families: Vec<Family>,
    rows: Vec<VendorRow>,
    /// id → 档位表：折叠后的家族行，和它各个变体 id（折叠前存的会话偏好还点名
    /// 它们）。
    levels: HashMap<String, Vec<String>>,
    /// raw 来自账号自己的清单（false = 兜底表，变体折叠不适用）。
    live: bool,
}

fn vendor_row(m: &Model) -> VendorRow {
    let mut raw = json!({ "id": m.id, "display_name": m.name });
    if !m.efforts.is_empty() {
        raw["reasoning_levels"] = json!(m.efforts);
    }
    VendorRow {
        id: m.id.clone(),
        context_window: m.context,
        max_output: m.output,
        images: m.images,
        raw,
    }
}

/// models.dev 目录里这个 id 的名字（catalog.Decorate 的一小半：只借名字，窗口/
/// 输出不借——Antigravity 给 Claude 250k，不是 Anthropic 的 1M）；目录里没有、或
/// 名字就是 id 本身 → None。
fn known_name(id: &str) -> Option<String> {
    let name = super::presets::lookup(id).map(|p| p.display_name)?;
    (!name.is_empty() && name != id).then_some(name)
}

fn catalog_of(raw: Vec<Model>, live: bool) -> Catalog {
    // 没名字的（Antigravity 没给它起名、或名字说的是别的模型被丢了）借 models.dev
    // 目录里的——要在折叠之前：-tiered 行的名字看它所属的模型叫什么。
    let raw: Vec<Model> = raw
        .into_iter()
        .map(|mut m| {
            if live && (m.name.is_empty() || m.name == m.id) {
                m.name = known_name(&m.id).unwrap_or_default();
            }
            m
        })
        .collect();
    let families = families_of(&raw);
    let folded = collapse(&raw);
    let mut levels: HashMap<String, Vec<String>> = HashMap::new();
    for m in &folded {
        levels.insert(m.id.clone(), m.efforts.clone());
    }
    for f in &families {
        if f.variants.len() > 1 {
            let efforts = folded.iter().find(|m| m.id == f.id).map(|m| m.efforts.clone()).unwrap_or_default();
            for v in &f.variants {
                levels.insert(v.id.clone(), efforts.clone());
            }
        }
    }
    let rows = folded.iter().map(vendor_row).collect();
    Catalog { raw, families, rows, levels, live }
}

// ---------------------------------------------------------------------
// 目录缓存
// ---------------------------------------------------------------------

struct State {
    cache: Option<(Instant, Catalog)>,
    backoff_until: Option<Instant>,
    scratch: Scratch,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

fn lock_state() -> std::sync::MutexGuard<'static, State> {
    STATE
        .get_or_init(|| {
            Mutex::new(State { cache: None, backoff_until: None, scratch: Scratch::default() })
        })
        .lock()
        .unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
fn reset_bridge_for_tests() {
    let mut s = lock_state();
    s.cache = None;
    s.backoff_until = None;
    s.scratch = Scratch::default();
}

/// 让缓存立刻过期但保留旧目录（测 stale-on-error 用）。
#[cfg(test)]
fn expire_cache_for_tests() {
    let mut s = lock_state();
    if let Some((at, _)) = s.cache.as_mut() {
        *at = Instant::now() - CACHE_TTL - Duration::from_secs(1);
    }
    s.backoff_until = None;
}

/// 向账号要一份目录（modelInfo + models + antigravityList，google.go:1041–:1292）。
fn fetch_catalog(cred: &Credential, scratch: &mut Scratch) -> Result<Catalog, Refused> {
    let (prod, daily) = code_assist_bases();
    let project = project_of(cred, scratch, &prod)?;
    let mut last: Option<Refused> = None;
    let mut refused: Vec<String> = Vec::new();
    let mut page: Option<Value> = None;
    for tier in ask_tiers(scratch, &project) {
        match fetch_models_page(cred, &daily, &project.id, &tier) {
            Ok(v) => {
                page = Some(v);
                break;
            }
            Err(e) if e.status == 401 => return Err(e),
            Err(e) => {
                // 点名被 400 拒的档位 24h 内不再点（tierRefused）
                if !tier.is_empty() && e.status == 400 {
                    refused.push(tier.clone());
                }
                last = Some(e);
            }
        }
    }
    for tier in refused {
        scratch.refused.insert(tier_key(&project.id, &tier), Instant::now());
    }
    let page = page.ok_or_else(|| last.unwrap_or(Refused { status: 502, said: "Antigravity listed no models".into() }))?;
    let listed = antigravity_list(parse_models_page(&page));
    Ok(if listed.is_empty() { catalog_of(fallback_models(), false) } else { catalog_of(listed, true) })
}

/// `/v1/models` 的 antigravity 行（30s 缓存、失败 60s 退避、失败回旧缓存）。没有
/// 登录 / 用户自有 `antigravity` 来源 / 订阅共享未开启（2026-11-22 四族默认不
/// 共享，gemini 族的闸）→ 空。
pub fn listed_rows(providers: &[Provider]) -> Vec<VendorRow> {
    rows_impl(providers, true)
}

/// 管理面（模型设置里点开订阅行）：无视共享开关的只读清单，不进菜单/注入。
pub fn listed_rows_for_inspection(providers: &[Provider]) -> Vec<VendorRow> {
    rows_impl(providers, false)
}

fn rows_impl(providers: &[Provider], gated: bool) -> Vec<VendorRow> {
    if providers.iter().any(|p| p.id == AGY_ID) || (gated && !super::family_enabled(FAMILY)) {
        return Vec::new();
    }
    let Some(cred) = read_credential() else { return Vec::new() };
    {
        let state = lock_state();
        if let Some((at, catalog)) = state.cache.as_ref() {
            if at.elapsed() < CACHE_TTL {
                return catalog.rows.clone();
            }
        }
        if state.backoff_until.is_some_and(|u| Instant::now() < u) {
            return state.cache.as_ref().map(|(_, c)| c.rows.clone()).unwrap_or_default();
        }
    }
    // 联网在锁外：抓取期间对话路径照常读旧目录。
    let mut scratch = lock_state().scratch.clone();
    let fetched = fetch_catalog(&cred, &mut scratch);
    let mut state = lock_state();
    state.scratch = scratch;
    match fetched {
        Ok(catalog) => {
            state.backoff_until = None;
            let rows = catalog.rows.clone();
            state.cache = Some((Instant::now(), catalog));
            rows
        }
        Err(e) => {
            state.backoff_until = Some(Instant::now() + FAIL_BACKOFF);
            if let Some((_, catalog)) = state.cache.as_ref() {
                return catalog.rows.clone();
            }
            // 一份都没拿到过：没登录态就什么都不列；其余（Google 忙、清单读不懂）
            // 给兜底表——对话路径才认得这些 id。
            if e.status == 401 {
                return Vec::new();
            }
            let catalog = catalog_of(fallback_models(), false);
            let rows = catalog.rows.clone();
            state.cache = Some((Instant::now(), catalog));
            rows
        }
    }
}

/// 合成来源 `antigravity`（只在内存，**绝不写进 providers.json**）。地址是不可达
/// 占位——真流量由 intercept 在路由层截走。用户在 providers.json 里自己定义了
/// `antigravity` 时调用方负责让位。
pub fn synthetic_provider() -> Option<Provider> {
    read_credential()?;
    Some(Provider {
        id: AGY_ID.to_string(),
        name: "Antigravity".to_string(),
        responses: "http://127.0.0.1:9/antigravity-bridge".to_string(),
        ..Default::default()
    })
}

/// 整串 `antigravity/<id>` 是不是这个账号能接的模型（纯内存，读目录缓存）：
/// 折叠后的家族行，和 Antigravity 自己清单里的每个变体 id（折叠前存的会话
/// 偏好还点名它们）。
fn knows(whole: &str) -> bool {
    let Some(id) = whole.strip_prefix("antigravity/") else { return false };
    if id.is_empty() {
        return false;
    }
    lock_state().cache.as_ref().is_some_and(|(_, c)| {
        c.levels.contains_key(id) || c.raw.iter().any(|m| m.id == id)
    })
}

/// 目录里某个 id 的档位表（家族的档位；变体 id 也算家族的）。
fn levels_of(id: &str) -> Vec<String> {
    lock_state().cache.as_ref().and_then(|(_, c)| c.levels.get(id)).cloned().unwrap_or_default()
}

/// levels 里离 want 最近的档位，同距取高的；levels 空或本来就有 want → want
/// （nearestEffort，modelprefs.go:1381）。
fn nearest_effort<'a>(want: &'a str, levels: &[&'a str]) -> &'a str {
    let Some(at) = super::effort::LEVELS.iter().position(|l| *l == want) else { return want };
    if levels.is_empty() || levels.contains(&want) {
        return want;
    }
    let mut best = want;
    let mut dist = super::effort::LEVELS.len();
    for l in levels {
        let Some(i) = super::effort::LEVELS.iter().position(|x| x == l) else { continue };
        if *l == "none" {
            continue;
        }
        let d = i.abs_diff(at);
        if d < dist || (d == dist && i > at) {
            best = l;
            dist = d;
        }
    }
    best
}

/// 一个请求按强度该发出去的 id（AntigravitySentID，modelprefs.go:1349）：家族
/// （几个档位变体折成的那一个）→ 强度挑的变体，没要强度 → 家族自己的默认；
/// 家族自己的某个变体 id（折叠前存的偏好还点名它）→ 没要强度原样发，要了就按
/// 强度挑；其它 id 原样。目录不是账号自己的清单（兜底表）时一律原样。
fn sent_id(model: &str, effort: &str) -> String {
    let state = lock_state();
    match state.cache.as_ref() {
        Some((_, catalog)) => sent_id_in(catalog, model, effort),
        None => model.to_string(),
    }
}

/// [`sent_id`] 对一份现成目录的算法（纯函数；真机 oracle 直接调）。
fn sent_id_in(catalog: &Catalog, model: &str, effort: &str) -> String {
    if !catalog.live {
        return model.to_string();
    }
    let model = model.to_string();
    // AntigravityBase：变体 id 归回它的家族
    let own = catalog
        .families
        .iter()
        .find(|f| f.variants.len() > 1 && f.id != model && f.variants.iter().any(|v| v.id == model))
        .map(|f| f.id.clone());
    let model = match own {
        Some(_) if effort.is_empty() => return model.to_string(),
        Some(base) => base,
        None => model.to_string(),
    };
    let Some(f) = catalog.families.iter().find(|f| f.id == model && f.variants.len() > 1) else { return model };
    let by = by_effort(f);
    if effort.is_empty() {
        return by.get("").cloned().unwrap_or(model);
    }
    let levels: Vec<&str> = super::effort::LEVELS.into_iter().filter(|l| by.get(*l).is_some()).collect();
    by.get(nearest_effort(effort, &levels))
        .or_else(|| by.get(""))
        .cloned()
        .unwrap_or(model)
}

// ---------------------------------------------------------------------
// 对话路径
// ---------------------------------------------------------------------

fn anthropic_error(status: u16, message: &str) -> Reply {
    let ty = match status {
        400 | 422 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        429 => "rate_limit_error",
        s if s >= 500 => "api_error",
        _ => "invalid_request_error",
    };
    Reply::json(status, &json!({ "type": "error", "error": { "type": ty, "message": message } }))
}

fn error_reply(proto: Proto, status: u16, message: &str) -> Reply {
    match proto {
        Proto::Anthropic => anthropic_error(status, message),
        _ => Reply::error(status, message),
    }
}

/// 发一轮对话（`v1internal:streamGenerateContent?alt=sse`，daily 基址）。成功 =
/// 200 的响应（流）；非 200 与传输失败变成 `(status, body)` 由调用方按各自协议
/// 说。不设整单超时：长思考回合不能被掐（连接 5s 仍在 outbound_agent 里）。
fn generate(cred: &Credential, body: &Value) -> Result<ureq::Response, (u16, Vec<u8>)> {
    let (_, daily) = code_assist_bases();
    let token = access_token(cred).ok_or_else(|| (401, SIGNED_OUT.as_bytes().to_vec()))?;
    let request = super::outbound_agent()
        .post(&format!("{}/v1internal:streamGenerateContent?alt=sse", daily.trim_end_matches('/')))
        .set("Content-Type", "application/json")
        .set("Authorization", &format!("Bearer {token}"))
        .set("User-Agent", &hub_user_agent())
        .set("Accept", "text/event-stream");
    match request.send_string(&body.to_string()) {
        Ok(r) => Ok(r),
        Err(ureq::Error::Status(code, r)) => Err((code, read_body(r, 1 << 20).unwrap_or_default())),
        Err(e) => Err((502, format!("antigravity: {e}").into_bytes())),
    }
}

/// 流中途的 error 事件（HTTP 200 之后才来）对客户端的说法。流式应答的头已经
/// 出去了，改不了状态码；非流式整块在这里定。
fn stream_error(proto: Proto, status: u16, message: &str) -> Reply {
    error_reply(proto, if status == 0 { 502 } else { status }, message)
}

/// 路由层钩子：请求的模型若只能由合成 `antigravity` 来源接住，就在这里走
/// Code Assist；否则 None（走原有路径）。
pub fn intercept(gateway: &super::Gateway, request: &Request, proto: Proto) -> Option<Reply> {
    // 订阅共享执行闸（2026-11-22）：gemini 族没开 → 让位（listed_rows 也不列，
    // 直连请求与未安装无异）。真实 providers 能接住的本来就走原路径。
    if !super::family_enabled(FAMILY) {
        return None;
    }
    let parsed: Value = serde_json::from_slice(&request.body).ok()?;
    let asked = parsed.get("model")?.as_str()?.to_string();
    let (groups, providers) = {
        let store = gateway.store.lock().ok()?;
        (store.groups().to_vec(), store.providers().to_vec())
    };
    let known = |id: &str| gateway.catalog.knows(&providers, id);
    // 真实来源（含用户自建的 antigravity）能接住的，一律走原路径。
    if super::groups::resolve_with(&groups, &providers, &asked, &known).is_some() {
        return None;
    }
    if providers.iter().any(|p| p.id == AGY_ID) {
        return None;
    }
    let synthetic = synthetic_provider()?;
    // 目录冷着（引擎刚起、agent 还没先列模型）就先要一份：否则已存会话里的
    // antigravity 模型要等谁先打一次 /v1/models 才认得。
    let cold = lock_state().cache.is_none();
    if cold {
        listed_rows(&providers);
    }
    let mut with = providers.clone();
    with.push(synthetic);
    let known2 = |id: &str| knows(id) || gateway.catalog.knows(&with, id);
    let (provider, bare_asked, levels) = super::groups::resolve_with(&groups, &with, &asked, &known2)?;
    // 直通 `antigravity/<id>` 形态 resolve 不查目录，这里统一认一次
    if provider != AGY_ID || !knows(&format!("{AGY_ID}/{bare_asked}")) {
        return None;
    }
    let cred = read_credential()?;
    // 成员强度后缀（`antigravity/gemini-3.7-flash:high`）：与其它来源同一套语义。
    let body_bytes = if levels.is_empty() {
        request.body.clone()
    } else {
        super::effort::apply(proto, &request.body, &levels, &levels_of(&bare_asked))
            .unwrap_or_else(|| request.body.clone())
    };
    let mut body: Value = serde_json::from_slice(&body_bytes).unwrap_or(parsed);
    let client_wants_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
    // usage 台账的会话键取入站体（翻译前的形状，三个处理器同口径）
    let session = super::affinity::stick_key(&[], &body);

    // 入站协议 → chat 请求体（三个入站协议都先折成 chat，再读成 Code Assist 的 IR）
    let mut table = super::translate::ToolTable::new();
    let mut prev = None;
    let chat = match proto {
        Proto::Chat => body,
        Proto::Responses => {
            // previous_response_id / store：与翻译路径同一张服务端回合表（miss =
            // 显式 400，绝不静默重放空历史）
            let (merged, saved) = match super::responses::prev_prepare(gateway, std::mem::take(&mut body)) {
                Ok(v) => v,
                Err(reply) => return Some(reply),
            };
            prev = saved;
            match super::translate::responses_to_chat_with_tools(&merged) {
                Ok((v, t)) => {
                    table = t;
                    v
                }
                Err(e) => return Some(error_reply(proto, 400, &e)),
            }
        }
        Proto::Anthropic => match super::translate::anthropic_to_chat(&body) {
            Ok(v) => v,
            Err(e) => return Some(error_reply(proto, 400, &e)),
        },
    };

    let mut ir = super::antigravity_codeassist::read_chat(&chat);
    // 被拒的判定对着**写下来的**系统提示词（Go antigravityTurnedAway 的
    // req.System）；发出去的是换过说法的那份。
    let system_as_written = ir.system.clone();
    ir.system = reword(&cloak_system(&ir.system));
    cloak_user_turns(&mut ir.messages);
    let sent = sent_id(&bare_asked, &ir.effort);
    let request_body = super::antigravity_codeassist::envelope(
        super::antigravity_codeassist::build_request(&ir, &sent),
        &sent,
        &cred.project_id,
    );

    let began = Instant::now();
    let log = super::usage_ledger::TurnLog::new(
        match proto {
            Proto::Chat => "chat",
            Proto::Responses => "responses",
            Proto::Anthropic => "anthropic",
        },
        &asked,
        session,
        began,
    );

    let response = match generate(&cred, &request_body) {
        Ok(r) => r,
        Err((status, buf)) => {
            let said = api_error(&buf, status);
            // Antigravity 对某些系统提示词一律回 429「Resource has been exhausted」：
            // 那是被拒不是额度——改成 400 并附提示，SDK 才不会把它当限速重试十遍
            // （Go turnedAwayStatus）。
            if status == 429 && turns_away(&system_as_written) && refuses(&said) {
                return Some(error_reply(
                    proto,
                    400,
                    &format!("{said} — {}", turned_away_hint(&system_as_written)),
                ));
            }
            return Some(match proto {
                // 厂商的 4xx/5xx 是合同面的一部分：anthropic 换信封，其余 JSON 原话透传
                Proto::Anthropic => anthropic_error(status, &said),
                _ => {
                    if serde_json::from_slice::<Value>(&buf).is_ok() {
                        Reply::Json { status, body: buf }
                    } else {
                        error_reply(proto, status, &said)
                    }
                }
            });
        }
    };

    let mut stream = ChatStream::new(Box::new(response.into_reader()), &asked);
    // 流里自己的 error 事件：被拒的同样改写（Go antigravityRound）
    if turns_away(&system_as_written) {
        let hint = turned_away_hint(&system_as_written);
        stream = stream.with_error_hook(Box::new(move |status, message| {
            if status == 429 && refuses(&message) {
                (400, format!("{message} — {hint}"))
            } else {
                (status, message)
            }
        }));
    }
    // 非流式客户端：整路读完折成整块 chat completion；流式：chat 流交给各协议
    // 的翻译器（responses::reshape / messages::chat_reply_to_anthropic，含 usage 记账）。
    let chat_reply = |stream: ChatStream| -> Reply {
        if client_wants_stream {
            return Reply::Stream {
                status: 200,
                content_type: "text/event-stream".into(),
                reader: Box::new(stream),
            };
        }
        let (completion, error) = stream.into_result();
        match error {
            Some((status, message)) => stream_error(proto, status, &message),
            None => Reply::Json { status: 200, body: serde_json::to_vec(&completion).unwrap_or_default() },
        }
    };
    Some(match proto {
        Proto::Chat => {
            let reply = chat_reply(stream);
            match reply {
                Reply::Stream { status, content_type, reader } => {
                    let tap = log.tap(gateway.usage.clone(), super::usage_ledger::Dialect::Chat, AGY_ID, status);
                    Reply::Stream {
                        status,
                        content_type,
                        reader: Box::new(super::usage_ledger::TapReader::new(tap, reader)),
                    }
                }
                Reply::Json { status, body } if status < 400 => {
                    log.whole(&gateway.usage, super::usage_ledger::Dialect::Chat, AGY_ID, status, &body);
                    Reply::Json { status, body }
                }
                other => other,
            }
        }
        Proto::Responses => {
            super::responses::reshape(chat_reply(stream), &asked, &table, None, prev, gateway, AGY_ID, &log)
        }
        Proto::Anthropic => super::messages::chat_reply_to_anthropic(
            chat_reply(stream),
            client_wants_stream,
            &asked,
            None,
            gateway,
            AGY_ID,
            &log,
        ),
    })
}

#[cfg(test)]
fn reset_token_cache_for_tests() {
    *ACCESS_TOKEN.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

/// Opt-in oracle against this machine's real Google sign-in (read-only, same
/// request the quota row makes):
/// `cargo test --bin marionette antigravity_quota_oracle -- --ignored --nocapture`
/// Prints group/bucket names and remaining fractions only — token values never
/// reach the output.
#[cfg(test)]
mod oracle {
    use std::io::Read;

    /// cfg(test) 的 credential_path 走 TEST_SEAM；oracle 自己按生产规则找真实
    /// 登录文件（$GEMINI_HOME 优先，否则 %USERPROFILE%\.gemini）。
    fn real_credential() -> super::Credential {
        let path = std::env::var_os("GEMINI_HOME")
            .filter(|h| !h.is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("USERPROFILE")
                    .filter(|h| !h.is_empty())
                    .map(|h| std::path::PathBuf::from(h).join(".gemini"))
            })
            .map(|base| base.join("antigravity-acp").join("acp_token.json"))
            .expect("no GEMINI_HOME / USERPROFILE");
        let bytes = std::fs::read(&path).expect("acp_token.json missing");
        let raw: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        super::Credential {
            client_id: super::trim_non_empty(&raw, "client_id").expect("client_id"),
            client_secret: super::trim_non_empty(&raw, "client_secret").expect("client_secret"),
            refresh_token: super::trim_non_empty(&raw, "refresh_token").expect("refresh_token"),
            token_uri: super::trim_non_empty(&raw, "token_uri").expect("token_uri"),
            project_id: super::trim_non_empty(&raw, "project_id").expect("project_id"),
        }
    }

    /// 造一个与生产 quota_row 完全同形的真实请求（真实登录 + 生产端点 +
    /// 生产 UA）。UA 形状决定应答里有哪些组（见 USER_AGENT 处注释）——
    /// 这条 oracle 守住「 aidev 形状回齐全部族」的契约：哪天 Google 收紧、
    /// 或有人把 UA 改回扁平形状，跑它就能当场看到组缺了。
    fn production_summary_request() -> serde_json::Value {
        let cred = real_credential();
        let token = super::access_token(&cred).expect("token refresh");
        let response = super::super::outbound_agent()
            .post(super::SUMMARY_URL)
            .timeout(super::FETCH_TIMEOUT)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "application/json")
            .set("User-Agent", super::USER_AGENT)
            .send_string(&serde_json::json!({ "project": cred.project_id }).to_string())
            .expect("quota summary request");
        let mut buf = Vec::new();
        response.into_reader().take(1 << 20).read_to_end(&mut buf).unwrap();
        serde_json::from_slice(&buf).unwrap()
    }

    /// 真账号的模型清单（只读：loadCodeAssist + fetchAvailableModels——与 Antigravity
    /// 自己启动时问的是同两个接口；不对话、不花额度）。只打印 id / 名字 / 档位 /
    /// 窗口，不打印令牌。
    /// `cargo test --bin marionette antigravity_models_oracle -- --ignored --nocapture`
    #[test]
    #[ignore = "reads this machine's real Antigravity sign-in and asks Google for its model list"]
    fn antigravity_models_oracle_real_account() {
        let _lock = crate::engine_gateway::TEST_SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let cred = real_credential();
        let mut scratch = super::Scratch::default();
        let catalog = super::fetch_catalog(&cred, &mut scratch).expect("catalog");
        if let Some(p) = &scratch.project {
            println!("tier: current={:?} paid={:?} (ids only)", p.tier, p.paid);
        }
        println!("live list: {} models", catalog.raw.len());
        for m in &catalog.raw {
            println!("  raw  {:<34} {:?} ctx={} out={} images={}", m.id, m.name, m.context, m.output, m.images);
        }
        println!("catalog rows: {}", catalog.rows.len());
        for row in &catalog.rows {
            println!(
                "  row  {:<34} {:?} levels={} ctx={} out={} images={}",
                row.id,
                row.raw.get("display_name"),
                row.raw.get("reasoning_levels").cloned().unwrap_or(serde_json::json!([])),
                row.context_window,
                row.max_output,
                row.images
            );
        }
        for f in catalog.families.iter().filter(|f| f.variants.len() > 1) {
            println!("  family {} efforts={:?} by_effort={:?}", f.id, f.efforts, super::by_effort(f));
            println!("    sent(no effort) = {}", super::sent_id_in(&catalog, &f.id, ""));
            println!("    sent(low)       = {}", super::sent_id_in(&catalog, &f.id, "low"));
            println!("    sent(high)      = {}", super::sent_id_in(&catalog, &f.id, "high"));
        }
        assert!(!catalog.rows.is_empty(), "no rows");
    }

    /// 真账号的一轮对话（发一句「Reply with the single word OK.」——花一点额度，
    /// 用的是本机这份订阅：跑之前想清楚）。只打印回了什么、finish、usage 与
    /// 流里的 error，不打印令牌。
    /// `cargo test --bin marionette antigravity_chat_oracle -- --ignored --nocapture`
    #[test]
    #[ignore = "sends one real prompt on this machine's real Antigravity account"]
    fn antigravity_chat_oracle_real_account() {
        let _lock = crate::engine_gateway::TEST_SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let cred = real_credential();
        let mut scratch = super::Scratch::default();
        let catalog = super::fetch_catalog(&cred, &mut scratch).expect("catalog");
        let plain = catalog
            .families
            .iter()
            .find(|f| f.variants.len() > 1)
            .map(|f| f.id.clone())
            .or_else(|| catalog.rows.first().map(|r| r.id.clone()))
            .expect("a model");
        let sent = super::sent_id_in(&catalog, &plain, "low");
        println!("model {plain} → sent {sent}");
        let chat = serde_json::json!({
            "model": format!("antigravity/{plain}"),
            "reasoning_effort": "low",
            "max_tokens": 16,
            "messages": [{"role": "user", "content": "Reply with the single word OK."}]
        });
        let ir = super::super::antigravity_codeassist::read_chat(&chat);
        let body = super::super::antigravity_codeassist::envelope(
            super::super::antigravity_codeassist::build_request(&ir, &sent),
            &sent,
            &cred.project_id,
        );
        let response = super::generate(&cred, &body).expect("generate");
        let (completion, error) =
            super::ChatStream::new(Box::new(response.into_reader()), "antigravity/x").into_result();
        println!("stream error: {:?}", error);
        println!("content: {:?}", completion["choices"][0]["message"]["content"]);
        println!("finish: {:?}", completion["choices"][0]["finish_reason"]);
        println!("usage: {}", completion["usage"]);
        assert!(error.is_none(), "{error:?}");
    }

    #[test]
    #[ignore = "depends on this machine's real ~/.gemini antigravity sign-in and the network"]
    fn antigravity_quota_oracle_real_account() {
        let data = production_summary_request();
        let groups = data.get("groups").and_then(|g| g.as_array()).cloned().unwrap_or_default();
        for group in &groups {
            let name = group.get("displayName").and_then(|s| s.as_str()).unwrap_or("(no name)");
            println!("group {name:?}");
            for bucket in group.get("buckets").and_then(|b| b.as_array()).unwrap_or(&vec![]) {
                println!(
                    "  bucket id={:?} window={:?} remaining={:?} disabled={:?}",
                    bucket.get("bucketId").and_then(|s| s.as_str()),
                    bucket.get("window").and_then(|s| s.as_str()),
                    bucket.get("remainingFraction").and_then(|f| f.as_f64()),
                    bucket.get("disabled").and_then(|f| f.as_bool()),
                );
            }
        }
        let windows = super::windows_from_summary(&data);
        println!("windows_from_summary -> {}", windows.len());
        for w in &windows {
            println!("  {}", w["name"].as_str().unwrap_or(""));
        }
        // 关键面：aidev UA 下应答不止一个族（3P + 至少一个 1P 组）。账号
        // 状态会变（组随订阅出现/消失），所以不断言具体名字。
        let families: std::collections::BTreeSet<String> = windows
            .iter()
            .filter_map(|w| {
                w["name"].as_str().and_then(|n| n.split(" · ").next().map(str::to_string))
            })
            .collect();
        println!("families: {families:?}");
        assert!(!windows.is_empty(), "no windows parsed");
        assert!(families.len() > 1, "expected more than one quota family, got {families:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::super::Gateway;
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    static DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

    struct Recorded {
        path: String,
        headers: std::collections::HashMap<String, String>,
        body: Vec<u8>,
    }

    /// 本地假后端：按路径前缀回固定应答；记录每个请求（grok_account 同款）。
    struct Mock {
        base: String,
        hits: Arc<AtomicUsize>,
        seen: Arc<Mutex<Vec<Recorded>>>,
        routes: Arc<Mutex<Vec<(String, u16, String, String)>>>,
    }

    impl Mock {
        fn start() -> Mock {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            let port = listener.local_addr().unwrap().port();
            let hits = Arc::new(AtomicUsize::new(0));
            let seen: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
            let routes: Arc<Mutex<Vec<(String, u16, String, String)>>> =
                Arc::new(Mutex::new(Vec::new()));
            let (h, s, r) = (hits.clone(), seen.clone(), routes.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    h.fetch_add(1, Ordering::SeqCst);
                    let (s, r) = (s.clone(), r.clone());
                    std::thread::spawn(move || {
                        let mut reader = BufReader::new(stream.try_clone().unwrap());
                        let mut request_line = String::new();
                        reader.read_line(&mut request_line).ok();
                        let path =
                            request_line.split_whitespace().nth(1).unwrap_or("").to_string();
                        let mut headers = std::collections::HashMap::new();
                        loop {
                            let mut line = String::new();
                            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                                break;
                            }
                            let t = line.trim_end();
                            if t.is_empty() {
                                break;
                            }
                            if let Some((k, v)) = t.split_once(':') {
                                headers
                                    .insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                            }
                        }
                        let len: usize = headers
                            .get("content-length")
                            .and_then(|l| l.parse().ok())
                            .unwrap_or(0);
                        let mut body = vec![0u8; len];
                        reader.read_exact(&mut body).ok();
                        // 路由键是 `前缀` 或 `前缀\0请求体里要含的片段`：带条件的优先
                        // （同一个端点按请求体分流用，fetchAvailableModels 的档位）。
                        let text = String::from_utf8_lossy(&body).into_owned();
                        let matches = |spec: &str| {
                            let (prefix, contains) = spec.split_once('\0').unwrap_or((spec, ""));
                            path.starts_with(prefix) && (contains.is_empty() || text.contains(contains))
                        };
                        let reply = r
                            .lock()
                            .unwrap()
                            .iter()
                            .filter(|(spec, ..)| matches(spec))
                            .max_by_key(|(spec, ..)| spec.contains('\0') as u8)
                            .map(|(_, status, ct, b)| (*status, ct.clone(), b.clone()))
                            .unwrap_or((404, "text/plain".into(), "no route".into()));
                        s.lock().unwrap().push(Recorded { path, headers, body });
                        let mut stream = stream;
                        let out = format!(
                            "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            reply.0,
                            reply.1,
                            reply.2.len(),
                            reply.2
                        );
                        let _ = stream.write_all(out.as_bytes());
                    });
                }
            });
            Mock { base: format!("http://127.0.0.1:{port}"), hits, seen, routes }
        }

        fn route(&self, prefix: &str, status: u16, content_type: &str, body: &str) {
            let mut routes = self.routes.lock().unwrap();
            routes.retain(|(p, ..)| p != prefix);
            routes.push((prefix.to_string(), status, content_type.to_string(), body.to_string()));
        }

        /// 同一个端点按请求体分流：请求体含 `contains` 时回这一条（优先于 `route`）。
        fn route_when(&self, prefix: &str, contains: &str, status: u16, content_type: &str, body: &str) {
            let mut routes = self.routes.lock().unwrap();
            let key = format!("{prefix}\0{contains}");
            routes.retain(|(p, ..)| p != &key);
            routes.push((key, status, content_type.to_string(), body.to_string()));
        }

        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }

        fn last(&self, path_prefix: &str) -> (std::collections::HashMap<String, String>, Vec<u8>) {
            let seen = self.seen.lock().unwrap();
            let r = seen
                .iter()
                .rev()
                .find(|r| r.path.starts_with(path_prefix))
                .expect("request seen");
            (r.headers.clone(), r.body.clone())
        }
    }

    struct World {
        dir: std::path::PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for World {
        fn drop(&mut self) {
            *TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()) = None;
            reset_token_cache_for_tests();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// 临时 providers.json + 自己的 TEST_SEAM（端点与凭据目录都落沙箱）。
    fn world(providers_json: &str, base: &str) -> (Gateway, World) {
        let lock = crate::engine_gateway::TEST_SANDBOX_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        reset_token_cache_for_tests();
        reset_bridge_for_tests();
        let dir = std::env::temp_dir().join(format!(
            "marionette-engine-agy-{}-{}",
            std::process::id(),
            DIR_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("magpie")).unwrap();
        std::fs::create_dir_all(dir.join("home").join(".gemini")).unwrap();
        std::fs::write(dir.join("magpie").join("providers.json"), providers_json).unwrap();
        *TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()) =
            Some(Seam { base: base.to_string(), home: dir.join("home") });
        let prior = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let gateway = Gateway::open().expect("gateway");
        match &prior {
            Some(p) => std::env::set_var("XDG_CONFIG_HOME", p),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        (gateway, World { dir, _lock: lock })
    }

    /// 假登录（全是测试假值）。token_uri 指向假后端。
    fn write_credential(world: &World, token_base: &str) {
        let dir = world.dir.join("home").join(".gemini").join("antigravity-acp");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("acp_token.json"),
            serde_json::to_vec(&json!({
                "client_id": "cid-test",
                "client_secret": "secret-test",
                "refresh_token": "rt-test",
                "token_uri": format!("{token_base}/token"),
                "project_id": "aicode-consumers",
            }))
            .unwrap(),
        )
        .unwrap();
    }

    const TOKEN_BODY: &str =
        r#"{"access_token":"atok-test","expires_in":3599,"scope":"https://www.googleapis.com/auth/cloud-platform"}"#;

    fn summary_body() -> String {
        json!({
            "groups": [
                {
                    "displayName": "Claude and GPT models",
                    "description": "Models within this group: Claude Opus 5.5",
                    "buckets": [
                        { "bucketId": "3p-weekly", "displayName": "Weekly Limit Remaining", "window": "weekly",
                          "resetTime": "2030-01-08T11:58:35Z", "remainingFraction": 0.4923596 },
                        { "bucketId": "3p-5h", "displayName": "Five Hour Limit Remaining", "window": "5h",
                          "resetTime": "2030-01-02T13:26:27Z", "remainingFraction": 1.0 }
                    ]
                },
                {
                    "displayName": "Gemini 4 Argon (High)",
                    "buckets": [
                        { "bucketId": "argon-weekly", "window": "weekly",
                          "resetTime": "2030-01-08T08:17:00Z", "remainingFraction": 0.79 }
                    ]
                }
            ]
        })
        .to_string()
    }

    #[test]
    fn quota_row_comes_from_quota_summary() {
        let mock = Mock::start();
        mock.route("/token", 200, "application/json", TOKEN_BODY);
        mock.route(
            "/v1internal",
            200,
            "application/json",
            &summary_body(),
        );
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_credential(&w, &mock.base);
        let rows = super::super::quotas::quota_entries(&gateway, true);
        let row = rows
            .iter()
            .find(|r| r["provider"] == "antigravity")
            .expect("antigravity row");
        assert_eq!(row["name"], "Google Antigravity");
        assert_eq!(row["kind"], "subscription");
        let windows = row["windows"].as_array().unwrap();
        let names: Vec<&str> = windows.iter().map(|x| x["name"].as_str().unwrap()).collect();
        // 族名归一 + 带族窗口名；单模型组（Argon）同样带族
        assert_eq!(
            names,
            vec![
                "Claude & GPT · 7 days",
                "Claude & GPT · 5 hours",
                "Gemini 4 Argon (High) · 7 days"
            ]
        );
        let weekly = &windows[0];
        // (1 - 0.4923596) × 100 = 50.76404 → 50.8；remaining 49.2
        assert_eq!(weekly["used"], 50.8);
        assert_eq!(weekly["remaining"], 49.2);
        assert_eq!(weekly["resetsAt"], "2030-01-08T11:58:35Z");
        // 额度请求带 Bearer 与 project；UA 必须是 aidev 形状——扁平形状
        // Google 只回 3P 一组（2026-11-21 oracle 实锤，见 USER_AGENT 注释）。
        let (headers, body) = mock.last("/v1internal");
        assert_eq!(headers["authorization"], "Bearer atok-test");
        assert_eq!(headers["user-agent"], USER_AGENT);
        assert!(headers["user-agent"].contains("aidev_client"));
        assert!(headers["user-agent"].contains("proxy_client=antigravity/sdk"));
        let sent: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(sent["project"], "aicode-consumers");
        // token 缓存生效：第二次只现问额度（force 语义），不再打 /token
        let hits = mock.hits();
        let _ = super::super::quotas::quota_entries(&gateway, true);
        assert_eq!(mock.hits(), hits + 1);
    }

    #[test]
    fn failing_antigravity_is_quiet_and_never_writes_credentials() {
        let mock = Mock::start();
        mock.route("/token", 200, "application/json", TOKEN_BODY);
        mock.route("/v1internal:retrieveUserQuotaSummary", 500, "text/plain", "boom");
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        let cred_path = w
            .dir
            .join("home")
            .join(".gemini")
            .join("antigravity-acp")
            .join("acp_token.json");
        std::fs::create_dir_all(cred_path.parent().unwrap()).unwrap();
        std::fs::write(
            &cred_path,
            serde_json::to_vec(&json!({
                "client_id": "cid", "client_secret": "sec", "refresh_token": "rt",
                "token_uri": format!("{}/token", mock.base), "project_id": "p",
            }))
            .unwrap(),
        )
        .unwrap();
        let before = std::fs::read(&cred_path).unwrap();
        let rows = super::super::quotas::quota_entries(&gateway, true);
        // 失败安静跳过；凭据文件一个字节没动
        assert!(rows.iter().all(|r| r["provider"] != "antigravity"));
        assert_eq!(before, std::fs::read(&cred_path).unwrap());
    }

    #[test]
    fn user_defined_antigravity_source_wins() {
        let mock = Mock::start();
        let (gateway, w) = world(
            r#"{"providers":[{"id":"antigravity","name":"My Antigravity Relay","key":"k","chat":"http://127.0.0.1:9"}]}"#,
            &mock.base,
        );
        write_credential(&w, &mock.base);
        let rows = super::super::quotas::quota_entries(&gateway, true);
        assert!(rows.iter().all(|r| r["name"] != "Google Antigravity"));
        assert_eq!(mock.hits(), 0);
    }

    #[test]
    fn missing_credential_means_no_row() {
        let mock = Mock::start();
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        // 只有目录没有文件 / 文件缺字段，都不出行
        std::fs::create_dir_all(w.dir.join("home").join(".gemini").join("antigravity-acp"))
            .unwrap();
        let rows = super::super::quotas::quota_entries(&gateway, true);
        assert!(rows.iter().all(|r| r["provider"] != "antigravity"));
        std::fs::write(
            w.dir.join("home").join(".gemini").join("antigravity-acp").join("acp_token.json"),
            serde_json::to_vec(&json!({ "refresh_token": "rt" })).unwrap(),
        )
        .unwrap();
        let rows = super::super::quotas::quota_entries(&gateway, true);
        assert!(rows.iter().all(|r| r["provider"] != "antigravity"));
        assert_eq!(mock.hits(), 0);
    }

    #[test]
    fn pool_name_strips_models_suffix_and_ands() {
        assert_eq!(pool_name("Gemini Models"), "Gemini");
        assert_eq!(pool_name("Claude and GPT models"), "Claude & GPT");
        assert_eq!(pool_name("Gemini 4 Argon (High)"), "Gemini 4 Argon (High)");
        assert_eq!(pool_name("  partial model "), "partial");
    }

    #[test]
    fn disabled_and_unknown_buckets_are_skipped() {
        let data = json!({
            "groups": [
                { "displayName": "Gemini Models", "buckets": [
                    { "bucketId": "a", "window": "5h", "remainingFraction": 0.5, "disabled": true },
                    { "bucketId": "b", "remainingFraction": 0.9, "displayName": "Mystery Window" },
                    { "bucketId": "c", "window": "5h", "remainingFraction": 0.25 }
                ] }
            ]
        });
        let windows = windows_from_summary(&data);
        let names: Vec<&str> = windows.iter().map(|x| x["name"].as_str().unwrap()).collect();
        assert_eq!(names, vec!["Gemini · Mystery Window", "Gemini · 5 hours"]);
        let mystery = &windows[0];
        assert_eq!(mystery["used"], 10.0);
    }

    // ─── Code Assist：模型目录 + 对话桥 ───────────────────────────────────

    fn post(path: &str, body: &Value) -> Request {
        Request { method: "POST".into(), path: path.into(), headers: Vec::new(), body: serde_json::to_vec(body).unwrap() }
    }

    fn get(path: &str) -> Request {
        Request { method: "GET".into(), path: path.into(), headers: Vec::new(), body: Vec::new() }
    }

    fn reply_text(reply: Reply) -> (u16, String) {
        match reply {
            Reply::Json { status, body } => (status, String::from_utf8_lossy(&body).into_owned()),
            Reply::Stream { status, mut reader, .. } => {
                let mut buf = Vec::new();
                reader.read_to_end(&mut buf).unwrap();
                (status, String::from_utf8_lossy(&buf).into_owned())
            }
        }
    }

    fn reply_json(reply: Reply) -> (u16, Value) {
        let (status, text) = reply_text(reply);
        (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    /// SSE 文本里每条 `data:` 的 JSON（`[DONE]` 除外）。
    fn sse_data(text: &str) -> Vec<Value> {
        text.lines()
            .filter_map(|l| l.strip_prefix("data:"))
            .filter(|d| d.trim() != "[DONE]")
            .map(|d| serde_json::from_str(d.trim()).unwrap())
            .collect()
    }

    const LOAD_BODY: &str = r##"{"currentTier":{"id":"standard-tier","name":"Standard"},"paidTier":{"id":"g1-pro-tier","name":"Google AI Pro"},"cloudaicompanionProject":"aicode-consumers"}"##;

    fn models_body() -> String {
        json!({
            "models": {
                "gemini-3.7-flash-low": {"displayName": "Gemini 3.7 Flash (Low)", "model": "MODEL_PLACEHOLDER_M198",
                    "maxTokens": 1000000, "maxOutputTokens": 64000, "supportsImages": true},
                "gemini-3.7-flash-medium": {"displayName": "Gemini 3.7 Flash (Medium)", "model": "MODEL_PLACEHOLDER_M198",
                    "maxTokens": 1000000, "maxOutputTokens": 64000, "supportsImages": true},
                "gemini-3.7-flash-high": {"displayName": "Gemini 3.7 Flash (High)", "model": "MODEL_PLACEHOLDER_M198",
                    "maxTokens": 900000, "maxOutputTokens": 32000, "supportsImages": false},
                "gemini-3-flash-agent": {"displayName": "Gemini 3.5 Flash (High)", "model": "MODEL_PLACEHOLDER_M201",
                    "maxTokens": 64000, "maxOutputTokens": 8192, "supportsImages": true},
                "claude-sonnet-4-6": {"displayName": "Claude Sonnet 4.6", "model": "MODEL_PLACEHOLDER_M202",
                    "maxTokens": 250000, "maxOutputTokens": 64000, "supportsImages": true},
                "gpt-oss-120b-medium": {"displayName": "GPT-OSS 120B (Medium)", "maxTokens": 128000},
                "gemini-4-argon-disabled": {"displayName": "Gemini 4 Argon", "disabled": true},
                "chat_20706": {"displayName": "Chat 20706"},
                "gemini-2.5-flash": {"displayName": "Gemini 3.5 Flash Lite", "model": "MODEL_GOOGLE_GEMINI_2_5_FLASH"},
                "gemini-3.5-flash-lite": {"displayName": "Gemini 3.5 Flash Lite", "model": "MODEL_PLACEHOLDER_M203"}
            },
            "agentModelSorts": [{"groups": [{"modelIds": ["claude-sonnet-4-6", "gemini-3.7-flash-high", "gemini-3-flash-agent"]}]}]
        })
        .to_string()
    }

    /// Code Assist 的流：思考 → 正文 → 调用 → 正文 → 收尾 + 账。
    fn gemini_sse() -> String {
        [
            r##"{"response":{"responseId":"resp-1","modelVersion":"gemini-3.7-flash","candidates":[{"content":{"role":"model","parts":[{"thought":true,"text":"hmm"}]}}]}}"##,
            r##"{"response":{"candidates":[{"content":{"role":"model","parts":[{"text":"Hello"}]}}]}}"##,
            r##"{"response":{"candidates":[{"content":{"role":"model","parts":[{"functionCall":{"name":"get_weather","args":{"city":"Paris"}}}]}}]}}"##,
            r##"{"response":{"candidates":[{"content":{"role":"model","parts":[{"text":"!"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":5,"thoughtsTokenCount":3,"cachedContentTokenCount":4}}}"##,
        ]
        .iter()
        .map(|d| format!("data: {d}\n\n"))
        .collect()
    }

    const TURNED_AWAY_BODY: &str = r##"{"error":{"code":429,"message":"Resource has been exhausted. Please try again later.","status":"RESOURCE_EXHAUSTED"}}"##;
    const QUOTA_BODY: &str = r##"{"error":{"code":429,"message":"You have exhausted your capacity on this model. Your quota will reset after 1h.","status":"RESOURCE_EXHAUSTED"}}"##;
    /// Claude Code 的系统提示词：带计费头那一行（Antigravity 一律拒的那个）。
    const CLAUDE_CODE_SYSTEM: &str = "You are Claude Code, Anthropic's official CLI for Claude.\nx-anthropic-billing-header: cc_version=2.1.63.c7e2015c; cc_entrypoint=cli";

    fn chat_request(system: &str) -> Value {
        json!({
            "model": "antigravity/gemini-3.7-flash",
            "stream": false,
            "reasoning_effort": "low",
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": "hi"}
            ],
            "tools": [{"type": "function", "function": {"name": "get_weather", "description": "Weather by city",
                "parameters": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"], "additionalProperties": false}}}]
        })
    }

    /// 一套能对话的假后端：token + 档位 + 清单 + Gemini 分块流，和一份假登录。
    fn bridge(mock: &Mock, stream_body: &str) -> (Gateway, World) {
        mock.route("/token", 200, "application/json", TOKEN_BODY);
        mock.route("/v1internal:loadCodeAssist", 200, "application/json", LOAD_BODY);
        mock.route("/v1internal:fetchAvailableModels", 200, "application/json", &models_body());
        mock.route("/v1internal:streamGenerateContent", 200, "text/event-stream", stream_body);
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_credential(&w, &mock.base);
        (gateway, w)
    }

    fn hits_on(mock: &Mock, prefix: &str) -> usize {
        mock.seen.lock().unwrap().iter().filter(|r| r.path.starts_with(prefix)).count()
    }

    /// 最近一次对话请求的信封。
    fn sent_envelope(mock: &Mock) -> Value {
        let (_, body) = mock.last("/v1internal:streamGenerateContent");
        serde_json::from_slice(&body).unwrap()
    }

    fn row_ids(body: &Value) -> Vec<&str> {
        body["data"].as_array().unwrap().iter().map(|r| r["id"].as_str().unwrap()).collect()
    }

    #[test]
    fn listed_rows_collapse_level_variants_and_follow_the_picker() {
        let mock = Mock::start();
        mock.route("/token", 200, "application/json", TOKEN_BODY);
        mock.route("/v1internal:loadCodeAssist", 200, "application/json", LOAD_BODY);
        mock.route("/v1internal:fetchAvailableModels", 200, "application/json", &models_body());
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_credential(&w, &mock.base);

        let body = reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1;
        // 选择器顺序（claude → 3.7 Flash → 3.5 Flash），其余按 id 排后；三个档位折成一个
        // gemini-3.7-flash；disabled / chat_ 的不出
        assert_eq!(
            row_ids(&body),
            vec![
                "antigravity/claude-sonnet-4-6",
                "antigravity/gemini-3.7-flash",
                "antigravity/gemini-3-flash-agent",
                "antigravity/gemini-2.5-flash",
                "antigravity/gemini-3.5-flash-lite",
                "antigravity/gpt-oss-120b-medium"
            ]
        );
        let data = body["data"].as_array().unwrap();
        let family = data.iter().find(|r| r["id"] == "antigravity/gemini-3.7-flash").unwrap();
        assert_eq!(family["display_name"], "Gemini 3.7 Flash");
        assert_eq!(family["reasoning_levels"], json!(["low", "medium", "high"]));
        // 窗口/输出取各变体里最小的，收图要全部都收
        assert_eq!(family["context_window"], 900000);
        assert_eq!(family["max_output_tokens"], 32000);
        assert_eq!(family["modalities"]["input"], json!(["text"]));
        let oss = data.iter().find(|r| r["id"] == "antigravity/gpt-oss-120b-medium").unwrap();
        assert_eq!(oss["modalities"]["input"], json!(["text"]), "gpt-oss takes no images unless Antigravity says so");
        let moved = data.iter().find(|r| r["id"] == "antigravity/gemini-2.5-flash").unwrap();
        assert_eq!(moved["display_name"], "gemini-2.5-flash", "a name that spells another model's id is dropped");

        // 清单按 Antigravity 自己的问法要：项目 + 账号付钱的档位（好的在前）
        let (headers, req) = mock.last("/v1internal:fetchAvailableModels");
        let req: Value = serde_json::from_slice(&req).unwrap();
        assert_eq!(req, json!({"project": "aicode-consumers", "entitlement": {"userTier": "g1-pro-tier"}}));
        assert_eq!(headers["authorization"], "Bearer atok-test");
        assert_eq!(headers["content-type"], "application/json");
        // Code Assist 的 UA 是 magpie 的扁平形状（不是额度行的 aidev 形状）
        assert_eq!(headers["user-agent"], format!("antigravity/hub/2.9.1 {}/{}", go_os(), go_arch()));
        let (_, req) = mock.last("/v1internal:loadCodeAssist");
        assert_eq!(
            serde_json::from_slice::<Value>(&req).unwrap(),
            json!({"metadata": {"ideType": "ANTIGRAVITY"}})
        );

        // 30s 缓存
        let hits = mock.hits();
        let _ = super::super::routes::route(&get("/v1/models"), &gateway);
        assert_eq!(mock.hits(), hits);
    }

    #[test]
    fn a_tier_google_refused_is_left_out_for_a_day() {
        let mock = Mock::start();
        mock.route("/token", 200, "application/json", TOKEN_BODY);
        mock.route("/v1internal:loadCodeAssist", 200, "application/json", LOAD_BODY);
        mock.route_when(
            "/v1internal:fetchAvailableModels",
            "userTier",
            400,
            "application/json",
            r##"{"error":{"code":400,"message":"INVALID_ARGUMENT","status":"INVALID_ARGUMENT"}}"##,
        );
        mock.route("/v1internal:fetchAvailableModels", 200, "application/json", &models_body());
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_credential(&w, &mock.base);
        let body = reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1;
        assert_eq!(row_ids(&body).len(), 6);
        // 付费档、当前档各被拒一次，然后不带档位要到了
        assert_eq!(hits_on(&mock, "/v1internal:fetchAvailableModels"), 3);

        expire_cache_for_tests();
        reset_token_cache_for_tests();
        let _ = super::super::routes::route(&get("/v1/models"), &gateway);
        // 被拒的档位不再点名，直接不带档位要
        assert_eq!(hits_on(&mock, "/v1internal:fetchAvailableModels"), 4);
        assert_eq!(hits_on(&mock, "/v1internal:loadCodeAssist"), 1, "loadCodeAssist is asked once an hour");
    }

    #[test]
    fn failing_listing_keeps_old_rows_and_a_cold_engine_falls_back_to_the_builtin_list() {
        let mock = Mock::start();
        mock.route("/token", 200, "application/json", TOKEN_BODY);
        mock.route("/v1internal:loadCodeAssist", 200, "application/json", LOAD_BODY);
        mock.route("/v1internal:fetchAvailableModels", 200, "application/json", &models_body());
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_credential(&w, &mock.base);
        assert_eq!(row_ids(&reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1).len(), 6);

        // 过期 + 500 → 回旧行；退避内不再打
        expire_cache_for_tests();
        reset_token_cache_for_tests();
        mock.route("/v1internal:fetchAvailableModels", 500, "text/plain", "boom");
        let body = reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1;
        assert_eq!(row_ids(&body).len(), 6);
        let after_fail = mock.hits();
        let _ = super::super::routes::route(&get("/v1/models"), &gateway);
        assert_eq!(mock.hits(), after_fail);
        drop(gateway);
        drop(w);

        // 冷引擎 + 清单要不到 → 兜底表（对话路径也认得这些 id）
        let cold = Mock::start();
        cold.route("/token", 200, "application/json", TOKEN_BODY);
        cold.route("/v1internal:loadCodeAssist", 500, "text/plain", "boom");
        cold.route("/v1internal:fetchAvailableModels", 500, "text/plain", "boom");
        cold.route("/v1internal:streamGenerateContent", 200, "text/event-stream", &gemini_sse());
        let (gateway, w) = world(r#"{"providers":[]}"#, &cold.base);
        write_credential(&w, &cold.base);
        assert_eq!(
            row_ids(&reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1),
            vec![
                "antigravity/gemini-pro-agent",
                "antigravity/gemini-3.1-pro-low",
                "antigravity/gemini-3-flash",
                "antigravity/gemini-3.1-flash-lite",
                "antigravity/claude-opus-4-6-thinking",
                "antigravity/claude-sonnet-4-6",
                "antigravity/gpt-oss-120b-medium"
            ]
        );
        let oss = reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1;
        let oss = &oss["data"].as_array().unwrap().iter().find(|r| r["id"] == "antigravity/gpt-oss-120b-medium").unwrap();
        assert_eq!(oss["modalities"]["input"], json!(["text"]));
        // 兜底表的 id 原样发（没有家族可折）
        let mut req = chat_request("You are helpful.");
        req["model"] = json!("antigravity/gemini-3.1-pro-low");
        let (status, _) = reply_json(super::super::routes::route(&post("/v1/chat/completions", &req), &gateway));
        assert_eq!(status, 200);
        assert_eq!(sent_envelope(&cold)["model"], "gemini-3.1-pro-low");
    }

    #[test]
    fn a_signed_out_account_lists_nothing_and_a_revoked_token_is_a_plain_401() {
        let mock = Mock::start();
        mock.route("/token", 400, "application/json", r##"{"error":"invalid_grant","error_description":"rt-test is bad"}"##);
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_credential(&w, &mock.base);
        let body = reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1;
        assert!(body["data"].as_array().unwrap().is_empty());
        assert_eq!(mock.hits(), 1, "only the token was asked for");
        let (status, _) = reply_json(super::super::routes::route(&post("/v1/chat/completions", &chat_request("You are helpful.")), &gateway));
        assert_eq!(status, 404, "nothing to route to");
        drop(gateway);
        drop(w);

        // 目录还在、令牌被吊销：对话如实 401，错误文本里没有任何令牌值
        let mock = Mock::start();
        let (gateway, _w) = bridge(&mock, &gemini_sse());
        let _ = super::super::routes::route(&get("/v1/models"), &gateway);
        mock.route("/token", 400, "application/json", r##"{"error":"invalid_grant","error_description":"rt-test is bad"}"##);
        reset_token_cache_for_tests();
        let (status, text) = reply_text(super::super::routes::route(&post("/v1/chat/completions", &chat_request("You are helpful.")), &gateway));
        assert_eq!(status, 401);
        assert!(text.contains("signed out"), "{text}");
        assert!(!text.contains("atok-test") && !text.contains("rt-test") && !text.contains("secret-test"), "{text}");
        assert_eq!(hits_on(&mock, "/v1internal:streamGenerateContent"), 0);
    }

    #[test]
    fn a_user_source_and_the_share_gate_win_over_the_bridge() {
        let mock = Mock::start();
        let (gateway, w) = world(
            r#"{"providers":[{"id":"antigravity","name":"My Relay","key":"k","chat":"http://127.0.0.1:9"}]}"#,
            &mock.base,
        );
        write_credential(&w, &mock.base);
        let body = reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1;
        assert!(body["data"].as_array().unwrap().is_empty());
        assert_eq!(mock.hits(), 0);
        drop(gateway);
        drop(w);

        // 共享闸只开了 codex：gemini 族不列、不接（直连请求与未安装无异）
        let mock = Mock::start();
        let (gateway, w) = world(r#"{"providers":[]}"#, &mock.base);
        write_credential(&w, &mock.base);
        super::super::set_subscription_enabled_for_tests(Some(vec!["codex".to_string()]));
        struct Gate;
        impl Drop for Gate {
            fn drop(&mut self) {
                super::super::set_subscription_enabled_for_tests(None);
            }
        }
        let _gate = Gate;
        let body = reply_json(super::super::routes::route(&get("/v1/models"), &gateway)).1;
        assert!(body["data"].as_array().unwrap().is_empty());
        let (status, _) = reply_json(super::super::routes::route(&post("/v1/chat/completions", &chat_request("You are helpful.")), &gateway));
        assert_eq!(status, 404);
        assert_eq!(mock.hits(), 0);
    }

    #[test]
    fn chat_turn_goes_out_as_a_code_assist_request() {
        let mock = Mock::start();
        let (gateway, _w) = bridge(&mock, &gemini_sse());
        // 目录冷着（没先列过模型）：桥自己要一份再认这个 id
        let mut req = chat_request(CLAUDE_CODE_SYSTEM);
        req["stream"] = json!(true);
        let (status, text) = reply_text(super::super::routes::route(&post("/v1/chat/completions", &req), &gateway));
        assert_eq!(status, 200);
        assert!(text.trim_end().ends_with("data: [DONE]"), "{text}");
        let frames = sse_data(&text);
        let deltas: Vec<&Value> = frames
            .iter()
            .filter(|c| c["choices"].as_array().is_some_and(|a| !a.is_empty()))
            .map(|c| &c["choices"][0]["delta"])
            .collect();
        assert!(deltas.iter().any(|d| d["reasoning_content"] == "hmm"), "{text}");
        assert!(deltas.iter().any(|d| d["content"] == "Hello"), "{text}");
        assert!(deltas.iter().any(|d| d["content"] == "!"), "{text}");
        let call = deltas
            .iter()
            .find_map(|d| d["tool_calls"].as_array().and_then(|a| a.first()))
            .expect("a tool call frame");
        assert_eq!(call["function"]["name"], "get_weather");
        assert_eq!(
            serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap()).unwrap(),
            json!({"city": "Paris"})
        );
        assert!(deltas.iter().any(|d| d["tool_calls"].as_array().is_some_and(|a| !a.is_empty())));
        assert!(frames.iter().any(|c| c["choices"][0]["finish_reason"] == "tool_calls"), "{text}");
        let usage = frames.iter().find(|c| c["usage"].is_object()).expect("a usage frame");
        // prompt 含缓存读（12 = 8 新鲜 + 4 命中）；completion 含思考
        assert_eq!(usage["usage"]["prompt_tokens"], 12);
        assert_eq!(usage["usage"]["completion_tokens"], 8);
        assert_eq!(usage["usage"]["completion_tokens_details"]["reasoning_tokens"], 3);
        assert_eq!(usage["usage"]["prompt_tokens_details"]["cached_tokens"], 4);

        // 信封：强度 low 挑的是 -low 变体；项目；Antigravity 的 userAgent / requestType /
        // requestId；会话名；计费头那一行被摘掉
        let sent = sent_envelope(&mock);
        assert_eq!(sent["model"], "gemini-3.7-flash-low");
        assert_eq!(sent["project"], "aicode-consumers");
        assert_eq!(sent["userAgent"], "antigravity");
        assert_eq!(sent["requestType"], "agent");
        assert!(sent["requestId"].as_str().unwrap().starts_with("agent-"));
        let request = &sent["request"];
        assert!(request["sessionId"].as_str().unwrap().starts_with('-'), "{request}");
        let system = request["systemInstruction"]["parts"][0]["text"].as_str().unwrap();
        assert!(system.starts_with("You are Claude Code, Anthropic's official CLI for Claude."), "{system}");
        assert!(!system.contains("x-anthropic-billing-header"), "{system}");
        assert!(request["contents"].to_string().contains("\"hi\""), "{request}");
        assert_eq!(request["tools"][0]["functionDeclarations"][0]["name"], "get_weather");
        assert_eq!(request["generationConfig"]["thinkingConfig"]["thinkingLevel"], "low");
        let (headers, _) = mock.last("/v1internal:streamGenerateContent");
        assert_eq!(headers["authorization"], "Bearer atok-test");
        assert_eq!(headers["user-agent"], format!("antigravity/hub/2.9.1 {}/{}", go_os(), go_arch()));

        // 没要强度：发家族的默认变体（这里是没有不带强度词的 id → high），不另说档位
        let plain = json!({"model": "antigravity/gemini-3.7-flash", "messages": [{"role": "user", "content": "hi"}]});
        let (status, _) = reply_json(super::super::routes::route(&post("/v1/chat/completions", &plain), &gateway));
        assert_eq!(status, 200);
        let sent = sent_envelope(&mock);
        assert_eq!(sent["model"], "gemini-3.7-flash-high");
        assert!(sent["request"]["generationConfig"]["thinkingConfig"].is_null(), "{}", sent["request"]["generationConfig"]);
    }

    #[test]
    fn non_stream_chat_responses_and_anthropic_inbound_come_back_in_their_own_shape() {
        let mock = Mock::start();
        let (gateway, _w) = bridge(&mock, &gemini_sse());

        // chat 非流式：整块 completion
        let (status, body) = reply_json(super::super::routes::route(&post("/v1/chat/completions", &chat_request("You are helpful.")), &gateway));
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["object"], "chat.completion");
        assert_eq!(body["model"], "antigravity/gemini-3.7-flash");
        let message = &body["choices"][0]["message"];
        assert_eq!(message["content"], "Hello!");
        assert_eq!(message["reasoning_content"], "hmm");
        assert_eq!(message["tool_calls"][0]["function"]["name"], "get_weather");
        assert_eq!(
            serde_json::from_str::<Value>(message["tool_calls"][0]["function"]["arguments"].as_str().unwrap()).unwrap(),
            json!({"city": "Paris"})
        );
        assert_eq!(body["choices"][0]["finish_reason"], "tool_calls");
        assert_eq!(body["usage"]["total_tokens"], 20);

        // responses 入站（非流式）：折成 response 对象
        let req = json!({"model": "antigravity/gemini-3.7-flash", "input": "hi", "stream": false,
            "tools": [{"type": "function", "name": "get_weather", "description": "Weather by city",
                "parameters": {"type": "object", "properties": {"city": {"type": "string"}}, "required": ["city"]}}]});
        let (status, body) = reply_json(super::super::routes::route(&post("/v1/responses", &req), &gateway));
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["object"], "response");
        assert_eq!(body["model"], "antigravity/gemini-3.7-flash");
        let output = body["output"].as_array().expect("output items");
        assert!(output.iter().any(|item| item["type"] == "message" && item["content"][0]["text"] == "Hello!"), "{body}");
        assert!(output.iter().any(|item| item["type"] == "function_call" && item["name"] == "get_weather"), "{body}");
        assert_eq!(body["usage"]["input_tokens"], 12);

        // anthropic 入站（非流式）：message 对象；流式：anthropic 事件流
        let req = json!({"model": "antigravity/gemini-3.7-flash", "max_tokens": 1024, "messages": [{"role": "user", "content": "hi"}]});
        let (status, body) = reply_json(super::super::routes::route(&post("/v1/messages", &req), &gateway));
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["type"], "message");
        assert_eq!(body["role"], "assistant");
        let content = body["content"].as_array().expect("content blocks");
        assert!(content.iter().any(|b| b["type"] == "text" && b["text"] == "Hello!"), "{body}");
        assert!(content.iter().any(|b| b["type"] == "tool_use" && b["name"] == "get_weather"), "{body}");
        assert_eq!(body["stop_reason"], "tool_use");
        // anthropic 口径：新鲜输入与缓存读分开记
        assert_eq!(body["usage"]["input_tokens"], 8);
        assert_eq!(body["usage"]["cache_read_input_tokens"], 4);
        assert_eq!(body["usage"]["output_tokens"], 8);

        let mut streamed = req.clone();
        streamed["stream"] = json!(true);
        let (status, text) = reply_text(super::super::routes::route(&post("/v1/messages", &streamed), &gateway));
        assert_eq!(status, 200);
        assert!(text.contains("event: message_start") && text.contains("event: message_stop"), "{text}");
        assert!(text.contains("Hello") && text.contains("tool_use"), "{text}");
    }

    #[test]
    fn turned_away_429_becomes_a_400_with_a_hint() {
        let mock = Mock::start();
        let (gateway, _w) = bridge(&mock, &gemini_sse());
        mock.route("/v1internal:streamGenerateContent", 429, "application/json", TURNED_AWAY_BODY);

        // Claude Code 的系统提示词 + 被拒的原话 → 400 + 提示（SDK 不重试 400）
        let (status, body) = reply_json(super::super::routes::route(&post("/v1/chat/completions", &chat_request(CLAUDE_CODE_SYSTEM)), &gateway));
        assert_eq!(status, 400, "{body}");
        let message = body["error"]["message"].as_str().unwrap();
        assert!(message.starts_with("Resource has been exhausted"), "{message}");
        assert!(message.contains("not a quota"), "{message}");

        // 中性的系统提示词吃到同一个 429：那就是它说的额度，原样透传
        let (status, body) = reply_json(super::super::routes::route(&post("/v1/chat/completions", &chat_request("You are helpful.")), &gateway));
        assert_eq!(status, 429, "{body}");
        assert_eq!(body["error"]["status"], "RESOURCE_EXHAUSTED");

        // 额度用完的 429 不算被拒，哪怕是 Claude Code 的提示词
        mock.route("/v1internal:streamGenerateContent", 429, "application/json", QUOTA_BODY);
        let (status, _) = reply_json(super::super::routes::route(&post("/v1/chat/completions", &chat_request(CLAUDE_CODE_SYSTEM)), &gateway));
        assert_eq!(status, 429);

        // anthropic 入站吃到被拒：换信封的错误，同样 400
        mock.route("/v1internal:streamGenerateContent", 429, "application/json", TURNED_AWAY_BODY);
        let req = json!({"model": "antigravity/gemini-3.7-flash", "max_tokens": 1024, "system": CLAUDE_CODE_SYSTEM,
            "messages": [{"role": "user", "content": "hi"}]});
        let (status, body) = reply_json(super::super::routes::route(&post("/v1/messages", &req), &gateway));
        assert_eq!(status, 400, "{body}");
        assert_eq!(body["type"], "error");
        assert!(body["error"]["message"].as_str().unwrap().contains("not a quota"), "{body}");
    }

    #[test]
    fn a_refusal_inside_the_stream_is_told_the_same_way() {
        let refused = format!("data: {TURNED_AWAY_BODY}\n\n");
        let mock = Mock::start();
        let (gateway, _w) = bridge(&mock, &refused);

        // HTTP 200 之后才来的 error 事件：非流式整块如实 400 + 提示
        let (status, body) = reply_json(super::super::routes::route(&post("/v1/chat/completions", &chat_request(CLAUDE_CODE_SYSTEM)), &gateway));
        assert_eq!(status, 400, "{body}");
        assert!(body["error"]["message"].as_str().unwrap().contains("not a quota"), "{body}");
        // 中性的提示词：照 Antigravity 报的状态说
        let (status, body) = reply_json(super::super::routes::route(&post("/v1/chat/completions", &chat_request("You are helpful.")), &gateway));
        assert_eq!(status, 429, "{body}");
        assert!(body["error"]["message"].as_str().unwrap().contains("Resource has been exhausted"), "{body}");

        // 流式：头已经出去了，错误帧带提示收尾
        let mut req = chat_request(CLAUDE_CODE_SYSTEM);
        req["stream"] = json!(true);
        let (status, text) = reply_text(super::super::routes::route(&post("/v1/chat/completions", &req), &gateway));
        assert_eq!(status, 200);
        let frames = sse_data(&text);
        let errors: Vec<&Value> = frames.iter().filter(|c| c["error"].is_object()).collect();
        assert_eq!(errors.len(), 1, "{text}");
        assert!(errors[0]["error"]["message"].as_str().unwrap().contains("not a quota"), "{text}");
        assert!(text.trim_end().ends_with("data: [DONE]"), "{text}");
    }

    #[test]
    fn previous_response_id_miss_is_a_400() {
        let mock = Mock::start();
        let (gateway, _w) = bridge(&mock, &gemini_sse());
        let req = json!({"model": "antigravity/gemini-3.7-flash", "input": "hi", "previous_response_id": "resp_missing"});
        let (status, _) = reply_json(super::super::routes::route(&post("/v1/responses", &req), &gateway));
        assert_eq!(status, 400);
        assert_eq!(hits_on(&mock, "/v1internal:streamGenerateContent"), 0, "never answered from an empty history");
    }

    #[test]
    fn a_group_member_level_picks_the_variant() {
        let mock = Mock::start();
        mock.route("/token", 200, "application/json", TOKEN_BODY);
        mock.route("/v1internal:loadCodeAssist", 200, "application/json", LOAD_BODY);
        mock.route("/v1internal:fetchAvailableModels", 200, "application/json", &models_body());
        mock.route("/v1internal:streamGenerateContent", 200, "text/event-stream", &gemini_sse());
        let (gateway, w) = world(
            r#"{"providers":[],"groups":[{"id":"g","members":["antigravity/gemini-3.7-flash:high"]}]}"#,
            &mock.base,
        );
        write_credential(&w, &mock.base);
        let _ = super::super::routes::route(&get("/v1/models"), &gateway);
        // 客户端要 low，成员固定 high：成员的档位赢
        let mut req = chat_request("You are helpful.");
        req["model"] = json!("g");
        let (status, body) = reply_json(super::super::routes::route(&post("/v1/chat/completions", &req), &gateway));
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["model"], "g", "the client is answered in the name it asked for");
        let sent = sent_envelope(&mock);
        assert_eq!(sent["model"], "gemini-3.7-flash-high");
        assert_eq!(sent["request"]["generationConfig"]["thinkingConfig"]["thinkingLevel"], "high");
    }

    #[test]
    fn sent_id_follows_the_family_rules() {
        let mock = Mock::start();
        let (gateway, _w) = bridge(&mock, &gemini_sse());
        let _ = super::super::routes::route(&get("/v1/models"), &gateway);
        // 没要强度：家族的默认（没有不带强度词的 id → high）
        assert_eq!(sent_id("gemini-3.7-flash", ""), "gemini-3.7-flash-high");
        assert_eq!(sent_id("gemini-3.7-flash", "low"), "gemini-3.7-flash-low");
        assert_eq!(sent_id("gemini-3.7-flash", "medium"), "gemini-3.7-flash-medium");
        // 家族没有的档位取最近的
        assert_eq!(sent_id("gemini-3.7-flash", "xhigh"), "gemini-3.7-flash-high");
        assert_eq!(sent_id("gemini-3.7-flash", "max"), "gemini-3.7-flash-high");
        // 家族自己的某个变体 id（折叠前存的偏好还点名它）：没要强度原样发
        assert_eq!(sent_id("gemini-3.7-flash-low", ""), "gemini-3.7-flash-low");
        assert_eq!(sent_id("gemini-3.7-flash-low", "medium"), "gemini-3.7-flash-medium");
        // 不是家族的：原样
        assert_eq!(sent_id("claude-sonnet-4-6", "high"), "claude-sonnet-4-6");
        assert_eq!(sent_id("gemini-3-flash-agent", "low"), "gemini-3-flash-agent");
        // 折叠后的家族行和每个变体 id 都认得
        for id in ["gemini-3.7-flash", "gemini-3.7-flash-low", "gemini-3.7-flash-high", "claude-sonnet-4-6"] {
            assert!(knows(&format!("antigravity/{id}")), "{id}");
        }
        assert!(!knows("antigravity/gemini-9-imaginary"));
        assert!(!knows("grok/gemini-3.7-flash"));
        assert_eq!(levels_of("gemini-3.7-flash-low"), vec!["low", "medium", "high"]);
    }

    #[test]
    fn family_levels_listen_to_the_names_when_each_says_one_and_tiered_inherits() {
        let m = |id: &str, name: &str| Model { id: id.into(), name: name.into(), images: true, ..Default::default() };
        let raw = vec![
            m("gemini-3.5-flash-extra-low", "Gemini 3.5 Flash (Low)"),
            m("gemini-3.5-flash-low", "Gemini 3.5 Flash (Medium)"),
            m("gemini-3.7-flash-tiered", ""),
            m("gemini-3.7-flash-low", "Gemini 3.7 Flash (Low)"),
            m("gemini-3.7-flash-high", "Gemini 3.7 Flash (High)"),
        ];
        let out = collapse(&raw);
        let ids: Vec<&str> = out.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, vec!["gemini-3.5-flash", "gemini-3.7-flash-tiered", "gemini-3.7-flash"]);
        let get = |id: &str| out.iter().find(|x| x.id == id).unwrap();
        // 两个变体的名字各说了一个档位：档位听名字的
        assert_eq!(get("gemini-3.5-flash").efforts, vec!["low", "medium"]);
        assert_eq!(get("gemini-3.5-flash").name, "Gemini 3.5 Flash");
        let tiered = get("gemini-3.7-flash-tiered");
        assert_eq!(tiered.name, "Gemini 3.7 Flash (Tiered)");
        assert_eq!(tiered.efforts, vec!["low", "high"], "a -tiered id thinks at the levels of its model");
        assert_eq!(get("gemini-3.7-flash").efforts, vec!["low", "high"]);

        let catalog = catalog_of(raw, true);
        assert_eq!(catalog.levels["gemini-3.5-flash-extra-low"], vec!["low", "medium"]);
        assert_eq!(catalog.levels["gemini-3.7-flash-tiered"], vec!["low", "high"]);

        // 默认变体：没有 high 时取最高的那个
        let families = families_of(&catalog.raw);
        let flash = families.iter().find(|f| f.id == "gemini-3.5-flash").unwrap();
        assert_eq!(by_effort(flash)[""], "gemini-3.5-flash-low");
    }

    #[test]
    fn names_that_spell_another_model_are_dropped() {
        assert!(name_spells("Gemini 3.5 Flash Lite", "gemini-3.5-flash-lite"));
        assert!(name_spells("Gemini 3.7 Flash (Low)", "gemini-3.7-flash-low"));
        assert!(!name_spells("Gemini 3.5 Flash Lite", "gemini-2.5-flash"));
        assert!(!name_spells("", "gemini-2.5-flash"));
        assert!(enum_names("MODEL_GOOGLE_GEMINI_2_5_FLASH", "gemini-2.5-flash"));
        assert!(!enum_names("MODEL_PLACEHOLDER_M198", "gemini-3.7-flash"));
        assert!(!enum_names("MODEL_GOOGLE_", "model_google_"));
        assert_eq!(name_effort("Gemini 3.7 Flash (XHigh)").1, "xhigh");
        assert_eq!(name_effort("Gemini 3.7 Flash (Extra Low)").1, "minimal");
        assert_eq!(name_effort("Claude Sonnet 4.6"), ("Claude Sonnet 4.6".to_string(), String::new()));
        assert_eq!(name_effort("GPT-OSS 120B (Medium)"), ("GPT-OSS 120B".to_string(), "medium".to_string()));
        assert_eq!(name_effort("Gemini 3.7 Flash (Preview)"), ("Gemini 3.7 Flash (Preview)".to_string(), String::new()));
    }

    #[test]
    fn nearest_effort_ties_go_up() {
        assert_eq!(nearest_effort("medium", &["low", "high"]), "high");
        assert_eq!(nearest_effort("medium", &["low"]), "low");
        assert_eq!(nearest_effort("medium", &[]), "medium");
        assert_eq!(nearest_effort("medium", &["low", "medium", "high"]), "medium");
        assert_eq!(nearest_effort("ultra", &["low", "high"]), "ultra");
        assert_eq!(nearest_effort("high", &["none", "low"]), "low", "none is never picked");
    }

    #[test]
    fn entitlements_name_the_paid_tier_first_and_never_the_free_one() {
        let project = |tier: &str, paid: &str| Project {
            id: "p".into(),
            tier: tier.into(),
            paid: paid.into(),
            at: None,
        };
        assert_eq!(project("standard-tier", "g1-pro-tier").entitlements(), vec!["g1-pro-tier", "standard-tier"]);
        assert_eq!(project("free-tier", "").entitlements(), Vec::<String>::new());
        assert_eq!(project("legacy-tier", "legacy-tier").entitlements(), Vec::<String>::new());
        assert_eq!(project("g1-pro-tier", "g1-pro-tier").entitlements(), vec!["g1-pro-tier"]);
    }
}
