//! 额度读取共用件（zcode_account / plan_quotas / key_balances 三家共用）。
//!
//! 只放无状态小工具：出站 HTTP（10s 封顶、响应体 1MiB 封顶、状态码与体都带回，
//! 因为 Go 里「403 + 体」这类语义要自己判）、窗口行（Go QuotaSpan 的行形态）、
//! JSON 取数/时间/金额小函数。**密钥只经 header 出站，从不进错误文本或日志**
//! ——本文件没有任何打印路径。
//!
//! 测试缝（仅 cfg(test)）：`REDIRECTS` 把 URL 前缀改写到本地假服务，生产路径
//! 恒用真实地址。
use std::io::Read;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, SecondsFormat, TimeZone, Utc};
use serde_json::{json, Value};

/// 单来源出站超时（quotas.rs 同款 10s）。
pub(super) const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// 一次出站的结果：状态码 + 体（最多 1MiB）。网络层失败（连不上/超时）= None。
pub(super) struct Reply {
    pub status: u16,
    pub body: String,
}

impl Reply {
    pub fn ok(&self) -> bool {
        self.status >= 200 && self.status < 300
    }
}

#[cfg(test)]
pub(super) static REDIRECTS: std::sync::Mutex<Vec<(String, String)>> =
    std::sync::Mutex::new(Vec::new());

/// 测试缝：URL 以某个前缀开头就换成假服务的前缀。
#[cfg(test)]
fn rewrite(url: &str) -> String {
    let guard = REDIRECTS.lock().unwrap_or_else(|p| p.into_inner());
    for (from, to) in guard.iter() {
        if let Some(rest) = url.strip_prefix(from.as_str()) {
            return format!("{to}{rest}");
        }
    }
    url.to_string()
}

#[cfg(not(test))]
fn rewrite(url: &str) -> String {
    url.to_string()
}

/// 通用请求。headers 是 (名, 值)；body 为 Some 时以 JSON 发出。
pub(super) fn send(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: Option<&str>,
) -> Option<Reply> {
    let url = rewrite(url);
    // 测试里绝不出真网：没被 REDIRECTS 改写到回环的 URL 一律当网络失败。
    #[cfg(test)]
    {
        let host = hostname_of(&url);
        if host != "127.0.0.1" && host != "localhost" {
            return None;
        }
    }
    let mut request = super::outbound_agent().request(method, &url);
    for (k, v) in headers {
        request = request.set(k, v);
    }
    request = request.timeout(FETCH_TIMEOUT);
    let result = match body {
        Some(b) => request.set("Content-Type", "application/json").send_string(b),
        None => request.call(),
    };
    let response = match result {
        Ok(r) => r,
        Err(ureq::Error::Status(_, r)) => r,
        Err(_) => return None,
    };
    let status = response.status();
    let mut buf = String::new();
    // 读体失败（非 UTF-8 / 断流）当作体为空：状态码仍可判。
    let _ = response.into_reader().take(1 << 20).read_to_string(&mut buf);
    Some(Reply { status, body: buf })
}

pub(super) fn get(url: &str, headers: &[(String, String)]) -> Option<Reply> {
    send("GET", url, headers, None)
}

pub(super) fn h(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// Z.ai 一类端点的 `{code,msg,data}` 信封（Go zcodeCallH）：2xx 且 code 缺省/0/200
/// → Some(data)（data 缺失或 null → Some(Null)）；其余一律 None。
pub(super) fn call_env(url: &str, headers: &[(String, String)]) -> Option<Value> {
    let reply = get(url, headers)?;
    if !reply.ok() {
        return None;
    }
    let env: Value = serde_json::from_str(&reply.body).ok()?;
    if !env.is_object() {
        return None;
    }
    let code = match env.get("code") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    if !matches!(code.as_str(), "" | "0" | "200" | "null") {
        return None;
    }
    Some(env.get("data").cloned().unwrap_or(Value::Null))
}

// ---------------------------------------------------------------------------
// 窗口行（Go QuotaSpan 的行形态：name/used/remaining/resetsAt + 可选计数）
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub(super) struct Win {
    pub name: String,
    pub used: f64,
    pub resets_at: Option<DateTime<Utc>>,
    pub amount: Option<f64>,
    pub limit: Option<f64>,
    pub unit: String,
    pub display: String,
    /// 钱包读数（Go QuotaSpan.Balance）：此时 used/remaining 不是份额。
    pub balance: Option<f64>,
    /// 内部用：Go 的 Aside（不挡模型的窗口）。QuotaSpan 不输出它，仅供排序/判定。
    pub aside: bool,
    /// 内部用：窗口时长（Go Span），仅供排序/去重。
    pub span_secs: i64,
}

impl Win {
    pub fn new(name: &str, used: f64) -> Win {
        Win { name: name.to_string(), used, ..Default::default() }
    }

    pub fn to_json(&self) -> Value {
        let mut row = json!({
            "name": self.name,
            "used": self.used,
            "remaining": (100.0 - self.used).max(0.0),
        });
        if let Some(at) = self.resets_at {
            row["resetsAt"] = json!(rfc3339(at));
        }
        if let Some(v) = self.amount {
            row["amount"] = json!(v);
        }
        if let Some(v) = self.limit {
            row["limit"] = json!(v);
        }
        if !self.unit.is_empty() {
            row["unit"] = json!(self.unit);
        }
        if !self.display.is_empty() {
            row["display"] = json!(self.display);
        }
        if let Some(v) = self.balance {
            row["balance"] = json!(v);
        }
        row
    }
}

pub(super) fn windows_json(ws: &[Win]) -> Vec<Value> {
    ws.iter().map(Win::to_json).collect()
}

// ---------------------------------------------------------------------------
// 小函数
// ---------------------------------------------------------------------------

pub(super) fn rfc3339(t: DateTime<Utc>) -> String {
    t.to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub(super) fn parse_rfc3339(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s.trim()).ok().map(|t| t.with_timezone(&Utc))
}

pub(super) fn from_millis(ms: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_millis_opt(ms).single()
}

pub(super) fn from_secs(s: i64) -> Option<DateTime<Utc>> {
    Utc.timestamp_opt(s, 0).single()
}

/// 数字或数字串（Go number()/zcodeNum 同款）。
pub(super) fn num(v: Option<&Value>) -> Option<f64> {
    match v {
        Some(Value::Number(n)) => n.as_f64(),
        Some(Value::String(s)) => s.trim().parse::<f64>().ok().filter(|f| f.is_finite()),
        _ => None,
    }
}

pub(super) fn text<'a>(v: Option<&'a Value>) -> &'a str {
    v.and_then(Value::as_str).unwrap_or("")
}

/// 智谱/ZCode 时间串（北京时间 "2026-10-18 12:00:00" 或 "2026-10-18"）。
pub(super) fn beijing_time(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim().replacen(' ', "T", 1);
    let cst = FixedOffset::east_opt(8 * 3600)?;
    for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d"] {
        let parsed = if fmt.len() == 8 {
            chrono::NaiveDate::parse_from_str(&s, fmt).ok().and_then(|d| d.and_hms_opt(0, 0, 0))
        } else {
            chrono::NaiveDateTime::parse_from_str(&s, fmt).ok()
        };
        if let Some(naive) = parsed {
            if let Some(t) = cst.from_local_datetime(&naive).single() {
                return Some(t.with_timezone(&Utc));
            }
        }
    }
    None
}

/// 金额带符号（Go money：符号贴前、两位小数）。
pub(super) fn money(sign: &str, v: f64) -> String {
    format!("{}{:.2}", sign, v)
}

/// Go Mask：≤8 位全遮，否则首 4 + … + 末 4（多钥匙时当卡片的用户名）。
pub(super) fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "•".repeat(chars.len());
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}…{tail}")
}

/// URL 的主机（含端口，小写，Go hostOf 同款）；不是通用解析器。
pub(super) fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    authority.rsplit('@').next().unwrap_or(authority).to_ascii_lowercase()
}

/// 不含端口的主机名（小写）。
pub(super) fn hostname_of(url: &str) -> String {
    let host = host_of(url);
    if host.starts_with('[') {
        return host.split(']').next().unwrap_or(&host).trim_start_matches('[').to_string();
    }
    host.split(':').next().unwrap_or(&host).to_string()
}

/// Provider.extra 里的布尔/字符串字段（flatten 保留的未知字段）。
pub(super) fn extra_str<'a>(p: &'a super::store::Provider, key: &str) -> &'a str {
    p.extra.get(key).and_then(Value::as_str).unwrap_or("")
}

pub(super) fn extra_bool(p: &super::store::Provider, key: &str) -> bool {
    p.extra.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// 一个 provider 在额度面上的全部钥匙：主钥匙 + 额外钥匙（`keys` 数组里没停用、
/// 与主钥匙不同的）。返回 (key, 多钥匙时的卡片用户名)。Go PlanQuotas/KeyBalances 同款：
/// 只有一把时没有用户名；多把时取 name，没 name 取 Mask(key)。
pub(super) fn keys_of(p: &super::store::Provider) -> Vec<(String, String)> {
    let mut keys: Vec<(String, String)> = vec![(p.key.clone(), String::new())];
    if let Some(list) = p.extra.get("keys").and_then(Value::as_array) {
        for k in list {
            let key = text(k.get("key")).to_string();
            if key.is_empty() || key == p.key || k.get("off").and_then(Value::as_bool).unwrap_or(false) {
                continue;
            }
            if keys.iter().any(|(existing, _)| *existing == key) {
                continue;
            }
            let name = text(k.get("name")).trim().to_string();
            keys.push((key.clone(), if name.is_empty() { mask(&key) } else { name }));
        }
    }
    if keys.len() > 1 && keys[0].1.is_empty() {
        let first = keys[0].0.clone();
        keys[0].1 = mask(&first);
    }
    keys
}

/// provider 本身该不该被额度面跳过（Go：Hidden / Off / 账号登录型）。
pub(super) fn skipped(p: &super::store::Provider) -> bool {
    extra_bool(p, "hidden") || extra_bool(p, "off") || p.extra.get("account").is_some_and(|a| !a.is_null())
}

/// 测试脚手架：本地假厂商（std TcpListener，零新依赖；按路径**最长前缀**
/// 回固定应答，记录每个请求的路径与头）+ REDIRECTS 守卫。
#[cfg(test)]
pub(super) mod testkit {
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    pub struct Recorded {
        pub path: String,
        pub headers: HashMap<String, String>,
        pub body: String,
    }

    impl Recorded {
        pub fn header(&self, name: &str) -> String {
            self.headers.get(&name.to_ascii_lowercase()).cloned().unwrap_or_default()
        }
    }

    pub struct Mock {
        /// `http://127.0.0.1:port`（无路径后缀）。
        pub base: String,
        routes: Arc<Mutex<Vec<(String, u16, String)>>>,
        seen: Arc<Mutex<Vec<Recorded>>>,
    }

    impl Mock {
        pub fn start() -> Mock {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
            let port = listener.local_addr().expect("port").port();
            let routes: Arc<Mutex<Vec<(String, u16, String)>>> = Arc::new(Mutex::new(Vec::new()));
            let seen: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
            let (r, s) = (routes.clone(), seen.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let (r, s) = (r.clone(), s.clone());
                    std::thread::spawn(move || {
                        let Ok(clone) = stream.try_clone() else { return };
                        let mut reader = BufReader::new(clone);
                        let mut request_line = String::new();
                        reader.read_line(&mut request_line).ok();
                        let path = request_line.split_whitespace().nth(1).unwrap_or("").to_string();
                        let mut headers = HashMap::new();
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
                                headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                            }
                        }
                        let len: usize = headers.get("content-length").and_then(|l| l.parse().ok()).unwrap_or(0);
                        let mut body = vec![0u8; len];
                        reader.read_exact(&mut body).ok();
                        let (status, reply) = {
                            let routes = r.lock().unwrap_or_else(|p| p.into_inner());
                            routes
                                .iter()
                                .filter(|(prefix, ..)| path.starts_with(prefix.as_str()))
                                .max_by_key(|(prefix, ..)| prefix.len())
                                .map(|(_, status, body)| (*status, body.clone()))
                                .unwrap_or((404, "no route".to_string()))
                        };
                        s.lock().unwrap_or_else(|p| p.into_inner()).push(Recorded {
                            path,
                            headers,
                            body: String::from_utf8_lossy(&body).into_owned(),
                        });
                        let mut stream = stream;
                        let out = format!(
                            "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                            reply.len()
                        );
                        let _ = stream.write_all(out.as_bytes());
                        let _ = stream.flush();
                    });
                }
            });
            Mock { base: format!("http://127.0.0.1:{port}"), routes, seen }
        }

        pub fn route(&self, prefix: &str, status: u16, body: &str) {
            let mut routes = self.routes.lock().unwrap_or_else(|p| p.into_inner());
            routes.retain(|(p, ..)| p != prefix);
            routes.push((prefix.to_string(), status, body.to_string()));
        }

        pub fn requests(&self) -> Vec<Recorded> {
            let seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
            seen.iter()
                .map(|r| Recorded { path: r.path.clone(), headers: r.headers.clone(), body: r.body.clone() })
                .collect()
        }
    }

    pub fn redirect(from: &str, to: &str) -> (String, String) {
        (from.to_string(), to.to_string())
    }

    /// 守卫：设置时覆盖整张表，Drop 清空。调用方必须已持 TEST_SANDBOX_LOCK。
    pub struct Redirects;

    impl Redirects {
        pub fn new(list: &[(String, String)]) -> Redirects {
            *super::REDIRECTS.lock().unwrap_or_else(|p| p.into_inner()) = list.to_vec();
            Redirects
        }
    }

    impl Drop for Redirects {
        fn drop(&mut self) {
            super::REDIRECTS.lock().unwrap_or_else(|p| p.into_inner()).clear();
        }
    }
}
