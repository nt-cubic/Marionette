//! Google Antigravity（Gemini 订阅）的只读额度支持——magpie google.go 里
//! `retrieveUserQuotaSummary` 一段的最小移植。
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

const AGY_ID: &str = "antigravity";
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

    /// 造一个与生产 quota_row 完全同形的真实请求（真实登录 + 生产端点 +
    /// 生产 UA）。UA 形状决定应答里有哪些组（见 USER_AGENT 处注释）——
    /// 这条 oracle 守住「 aidev 形状回齐全部族」的契约：哪天 Google 收紧、
    /// 或有人把 UA 改回扁平形状，跑它就能当场看到组缺了。
    fn production_summary_request() -> serde_json::Value {
        // cfg(test) 的 credential_path 走 TEST_SEAM；oracle 自己按生产规则
        // 找真实登录文件（$GEMINI_HOME 优先，否则 %USERPROFILE%\.gemini）。
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
        let cred = super::Credential {
            client_id: super::trim_non_empty(&raw, "client_id").expect("client_id"),
            client_secret: super::trim_non_empty(&raw, "client_secret").expect("client_secret"),
            refresh_token: super::trim_non_empty(&raw, "refresh_token").expect("refresh_token"),
            token_uri: super::trim_non_empty(&raw, "token_uri").expect("token_uri"),
            project_id: super::trim_non_empty(&raw, "project_id").expect("project_id"),
        };
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
                        let reply = r
                            .lock()
                            .unwrap()
                            .iter()
                            .find(|(prefix, ..)| path.starts_with(prefix.as_str()))
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
}
