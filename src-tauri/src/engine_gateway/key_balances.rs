//! 钥匙余额的补全来源（合同 C3 的 balance 类卡）——Go `balance.go` /
//! `balance_probe.go` 里 quotas.rs 没收的那一半。
//!
//! quotas.rs 已有 DeepSeek / Moonshot / OpenRouter / SiliconFlow / StepFun /
//! Zhipu 按量六家；本文件补：
//! - Command Code（`/alpha/billing/credits`，本月额度 + 买的 + 送的，美元）、
//!   ModelScope（魔粒）、Ollama Cloud（含额之外买的 credits）、AiHubMix
//!   （钥匙自己剩的 / 账号级——带 BalanceToken 时问 `/api/user/self`）；
//! - 用户自报的 `balanceURL` + `balancePath`（+`balanceMethod` / `balanceToken`），
//!   中继站（new-api / sub2api / OpenAI credit_grants）的读法；路径表达式
//!   支持 Go readBalancePath 的标量子集（点路径、数组下标、`+ - * /`、括号、
//!   `$ ¥ € £` 前缀、`%` 后缀、`label: expr; …` 多段）；
//! - 手加的中继站没写 balanceURL 时，按 Go balance_probe 的两个常见查询
//!   （sub2api `/v1/usage`、new-api `/api/usage/token`）探测，命中缓存 24h、
//!   没命中 6h、连不上 5min（进程内存，不落盘）。
//!
//! 「用户自建 `source-*` 来源出行」的真相：Go 里它们不是靠通用余额探测，而是
//! 靠 host 命中 plan 表（opencode.ai Go、智谱 coding、ollama.com，见
//! plan_quotas.rs）；balance_probe 只对**没有已知来源的中继站**起作用。
//!
//! **不做**：列表聚合表达式（`a.*.used` / `[status=active]` / `sum()` 等）、
//! sub2api 钥匙限额窗口（readSub2APIKeyLimits）、余额拆分（BalancePart 卡）、
//! 账号级去重标注（SharedBalance——这里按「同来源同读数」折叠成一行）、
//! 探测结果落盘。
//!
//! 红线同 quotas.rs：展示面，出站失败安静跳过；密钥只进 header/URL，不进错误
//! 文本；单来源 10s 封顶；HTTP 不持 store 锁（拿到的是快照）。
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::quota_kit::*;
use super::store::Provider;

const MODELSCOPE_URL: &str = "https://modelscope.cn/openapi/v1/magicubes/balance";
const OLLAMA_URL: &str = "https://ollama.com/api/balance";
const COMMAND_CODE_URL: &str = "https://api.commandcode.ai/alpha/billing/credits";
const AIHUBMIX_KEY_URL: &str = "https://aihubmix.com/dashboard/billing/remain";
const AIHUBMIX_ACCOUNT_URL: &str = "https://aihubmix.com/api/user/self";

const NEW_API_USER_SELF: &str = "/api/user/self";
const NEW_API_KEY_USAGE: &str = "/api/usage/token";
const NEW_API_QUOTA_PATH: &str = "$data.quota / 500000";
const NEW_API_KEY_PATH: &str = "$data.total_available / 500000";
const SUB2API_PROFILE: &str = "/api/v1/user/profile";
const SUB2API_KEY_USAGE: &str = "/v1/usage";
const CREDIT_GRANTS: &str = "/dashboard/billing/credit_grants";

const PROBE_HIT: Duration = Duration::from_secs(24 * 3600);
const PROBE_MISS: Duration = Duration::from_secs(6 * 3600);
const PROBE_DOWN: Duration = Duration::from_secs(5 * 60);

/// 用户自报了 balanceURL 的来源由本文件接管（Go：BalanceURL 优先于一切 host 表）。
/// quotas.rs 的 host 循环据此让位，避免同一来源出两行。
pub fn takes_over(p: &Provider) -> bool {
    !extra_str(p, "balanceURL").trim().is_empty()
}

// ---------------------------------------------------------------------------
// 来源
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum Reader {
    /// 用户的 balancePath（或已知查询的默认字段）。
    Path(String),
    CommandCode,
    ModelScope,
    OllamaCredits,
    AiHubMixKey,
    AiHubMixAccount,
}

#[derive(Clone, Debug, PartialEq)]
struct Source {
    url: String,
    method: &'static str,
    /// 非空 = 整个 Authorization 头（账号级余额，不是钥匙的）。
    token: String,
    reader: Reader,
    /// 用户自报端点（要带 provider 自己的 headers）。
    custom: bool,
    /// 读数是账号级的（同账号多把钥匙读到一样的钱）。
    account_wide: bool,
}

fn url_path_of(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    match rest.find('/') {
        Some(i) => rest[i..].split(['?', '#']).next().unwrap_or(""),
        None => "",
    }
}

fn origin_of(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => format!("{scheme}://{}", rest.split(['/', '?', '#']).next().unwrap_or("")),
        None => String::new(),
    }
}

/// Go keyNotToken：balanceToken 只是钥匙自己（可带 "Bearer "）= 当没有 token；
/// 否则保留原始写法（带 scheme 的 token 要原样当 Authorization）。
fn effective_token(p: &Provider) -> String {
    let raw = extra_str(p, "balanceToken").trim().to_string();
    let mut t = raw.clone();
    if t.len() > 7 && t.get(..7).is_some_and(|h| h.eq_ignore_ascii_case("bearer ")) {
        t = t[7..].trim().to_string();
    }
    if !t.is_empty() && t == p.key.trim() {
        return String::new();
    }
    raw
}

/// Go keyQueryOf：账号查询（new-api /api/user/self、sub2api /api/v1/user/profile）
/// 没有 token 又没有自带 Authorization/Cookie 头时，改问同一站点「钥匙自己」的查询。
/// 返回 (balanceURL, balancePath)；保存的值不动。
fn key_query(p: &Provider, token: &str) -> (String, String) {
    let url = extra_str(p, "balanceURL").trim().to_string();
    let path = extra_str(p, "balancePath").to_string();
    if url.is_empty() || !token.is_empty() {
        return (url, path);
    }
    let own_auth = p.headers.iter().any(|(k, v)| {
        !v.trim().is_empty() && (k.eq_ignore_ascii_case("authorization") || k.eq_ignore_ascii_case("cookie"))
    });
    if own_auth {
        return (url, path);
    }
    let origin = origin_of(&url);
    if origin.is_empty() {
        return (url, path);
    }
    let url_path = url_path_of(&url).trim_end_matches('/').to_string();
    for (account, key_query, field) in [
        (SUB2API_PROFILE, SUB2API_KEY_USAGE, "$data.balance"),
        (NEW_API_USER_SELF, NEW_API_KEY_USAGE, NEW_API_QUOTA_PATH),
    ] {
        if let Some(base) = url_path.strip_suffix(account) {
            let new_path = if path.trim() == field { String::new() } else { path };
            return (format!("{origin}{base}{key_query}"), new_path);
        }
    }
    (url, path)
}

/// Go balancePathOf：用户写的字段，没写按已知查询给默认。
fn path_of(url: &str, user_path: &str) -> String {
    if !user_path.trim().is_empty() {
        return user_path.to_string();
    }
    let path = url_path_of(url).trim_end_matches('/');
    if path == NEW_API_USER_SELF {
        NEW_API_QUOTA_PATH.to_string()
    } else if path == NEW_API_KEY_USAGE {
        NEW_API_KEY_PATH.to_string()
    } else if path == SUB2API_PROFILE {
        "$data.balance".to_string()
    } else if path == SUB2API_KEY_USAGE {
        "$remaining".to_string()
    } else if path.ends_with(CREDIT_GRANTS) {
        "$total_available".to_string()
    } else {
        String::new()
    }
}

fn source_of(p: &Provider) -> Option<Source> {
    let token = effective_token(p);
    let (custom_url, custom_path) = key_query(p, &token);
    if !custom_url.is_empty() {
        let method = if extra_str(p, "balanceMethod").trim().eq_ignore_ascii_case("POST") { "POST" } else { "GET" };
        let path = path_of(&custom_url, &custom_path);
        return Some(Source {
            url: custom_url,
            method,
            token: token.clone(),
            reader: Reader::Path(path),
            custom: true,
            account_wide: !token.is_empty(),
        });
    }
    for base in [&p.chat, &p.responses, &p.anthropic] {
        let (url, reader, wide): (&str, Reader, bool) = match host_of(base).as_str() {
            "api.commandcode.ai" => (COMMAND_CODE_URL, Reader::CommandCode, true),
            "api-inference.modelscope.cn" => (MODELSCOPE_URL, Reader::ModelScope, false),
            "ollama.com" => (OLLAMA_URL, Reader::OllamaCredits, true),
            "aihubmix.com" => {
                if !token.is_empty() {
                    return Some(Source {
                        url: AIHUBMIX_ACCOUNT_URL.into(),
                        method: "GET",
                        token,
                        reader: Reader::AiHubMixAccount,
                        custom: false,
                        account_wide: true,
                    });
                }
                (AIHUBMIX_KEY_URL, Reader::AiHubMixKey, false)
            }
            _ => continue,
        };
        return Some(Source { url: url.into(), method: "GET", token: String::new(), reader, custom: false, account_wide: wide });
    }
    None
}

// ---------------------------------------------------------------------------
// 读法
// ---------------------------------------------------------------------------

fn with_key(s: &str, key: &str, in_url: bool) -> String {
    // Go balanceKeyNames：`{key}` `{apiKey}` `{api_key}` `{token}` `{{key}}` `${API_KEY}`（不分大小写）
    let key = if in_url { query_escape(key) } else { key.to_string() };
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let start = if chars[i] == '$' && chars.get(i + 1) == Some(&'{') { i + 1 } else { i };
        if chars.get(start) == Some(&'{') {
            let mut j = start + 1;
            if chars.get(j) == Some(&'{') {
                j += 1;
            }
            let name_start = j;
            while j < chars.len() && chars[j] != '}' && chars[j] != '{' {
                j += 1;
            }
            let name: String = chars[name_start..j].iter().collect::<String>().trim().to_ascii_lowercase();
            if j < chars.len() && chars[j] == '}' && matches!(name.as_str(), "key" | "apikey" | "api_key" | "api-key" | "token") {
                j += 1;
                if chars.get(j) == Some(&'}') {
                    j += 1;
                }
                out.push_str(&key);
                i = j;
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn query_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Go balanceAuthorization：带空格（已含 scheme）原样；JWT（eyJ…，两个点）加 Bearer；
/// 其余（new-api 的 access token）原样。
fn balance_authorization(token: &str) -> String {
    if token.contains(' ') {
        return token.to_string();
    }
    if token.starts_with("eyJ") && token.matches('.').count() == 2 {
        return format!("Bearer {token}");
    }
    token.to_string()
}

/// Go balanceRefusal：`{"success":false,"message":…}`（new-api 中继拒绝的方式，
/// 200 与 401 都可能）。
fn refusal(body: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(body) else { return false };
    v.get("success").and_then(Value::as_bool) == Some(false) && !text(v.get("message")).trim().is_empty()
}

fn read_with(reader: &Reader, body: &str) -> Option<String> {
    match reader {
        Reader::Path(path) => read_balance_path(body, path),
        Reader::CommandCode => {
            let c: Value = serde_json::from_str(body).ok()?;
            let (_, left) = super::plan_quotas::cmd_left(&c)?;
            Some(money("$", left))
        }
        Reader::ModelScope => {
            let v: Value = serde_json::from_str(body).ok()?;
            let available = num(v.pointer("/data/available_balance"))?;
            Some(format!("{} 魔粒", available))
        }
        Reader::OllamaCredits => {
            let v: Value = serde_json::from_str(body).ok()?;
            // 没有 purchased = 余额卡不出（含额是 plan 卡的事）
            Some(money("$", num(v.pointer("/purchased/balance_usd"))?))
        }
        Reader::AiHubMixKey => {
            let v: Value = serde_json::from_str(body).ok()?;
            let remain = num(v.get("total_usage"))?;
            // 无限额的钥匙回 -1：它自己没有余额，账号的只告诉账号 token
            (remain >= 0.0).then(|| money("$", remain))
        }
        Reader::AiHubMixAccount => {
            let v: Value = serde_json::from_str(body).ok()?;
            if v.get("success").and_then(Value::as_bool) != Some(true) {
                return None;
            }
            Some(money("$", num(v.pointer("/data/quota"))? / 500_000.0))
        }
    }
}

/// 问一次余额（Go balanceRead 的出站半边）。
fn ask(p: &Provider, src: &Source, key: &str) -> Option<String> {
    let url = with_key(&src.url, key, true);
    let mut headers: Vec<(String, String)> = Vec::new();
    if !src.token.is_empty() {
        if src.custom {
            for (k, v) in &p.headers {
                headers.push((k.clone(), with_key(v, key, false)));
            }
        }
        headers.push(h("Authorization", &balance_authorization(&src.token)));
    } else {
        headers.push(h("Authorization", &format!("Bearer {key}")));
        for (k, v) in &p.headers {
            headers.push((k.clone(), with_key(v, key, false)));
        }
    }
    headers.push(h("Accept", "application/json"));
    let body = if src.method == "POST" { Some("") } else { None };
    let reply = send(src.method, &url, &headers, body)?;
    if refusal(&reply.body) || !reply.ok() {
        return None;
    }
    read_with(&src.reader, &reply.body).filter(|s| !s.is_empty())
}

// ---------------------------------------------------------------------------
// 中继站探测（Go balance_probe.go）
// ---------------------------------------------------------------------------

type ProbeEntry = (Option<String>, Instant);

fn probe_cache() -> &'static Mutex<HashMap<String, ProbeEntry>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<String, ProbeEntry>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
pub(super) fn reset_probes_for_tests() {
    probe_cache().lock().unwrap_or_else(|p| p.into_inner()).clear();
}

/// 厂商自己的 host（Go presetHosts 的 vendor/local 部分）与不探测的中继：
/// 绝不拿它们当「手加中继站」去问中继查询。
const NO_PROBE_HOSTS: &[&str] = &[
    "opencode.ai", "api.cline.bot", "api.openai.com", "api.anthropic.com", "generativelanguage.googleapis.com",
    "api.x.ai", "api.mistral.ai", "api.groq.com", "api.together.xyz", "api.fireworks.ai", "api.cerebras.ai",
    "api.perplexity.ai", "api.cohere.com", "api.deepinfra.com", "api.sambanova.ai", "integrate.api.nvidia.com",
    "api.deepseek.com", "api.moonshot.cn", "api.moonshot.ai", "api.kimi.com", "api.kimi.ai", "openrouter.ai",
    "api.siliconflow.cn", "api.siliconflow.com", "api.stepfun.com", "api.stepfun.ai", "open.bigmodel.cn",
    "api.z.ai", "api.minimaxi.com", "api.minimax.cn", "api.minimax.io", "dashscope.aliyuncs.com",
    "dashscope-intl.aliyuncs.com", "ark.cn-beijing.volces.com", "api.commandcode.ai", "ollama.com",
    "aihubmix.com", "api-inference.modelscope.cn", "api.githubcopilot.com", "models.github.ai",
    "api.hunyuan.cloud.tencent.com", "qianfan.baidubce.com", "api.lingyiwanwu.com", "api.baichuan-ai.com",
    "api.xiaomimimo.com",
];

/// 私网 / 链路本地 / CGNAT（Tailscale）的 IPv4 字面量：自己局域网里的服务不是
/// 「中继站」，不拿钥匙去探（比 Go 保守一点——Go 只排除回环）。
fn private_ip_literal(host: &str) -> bool {
    let octets: Vec<u8> = host.split('.').filter_map(|p| p.parse().ok()).collect();
    if octets.len() != 4 || host.split('.').count() != 4 {
        return false;
    }
    match (octets[0], octets[1]) {
        (10, _) | (192, 168) | (169, 254) => true,
        (172, b) => (16..=31).contains(&b),
        (100, b) => (64..=127).contains(&b),
        _ => false,
    }
}

fn not_probed(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    if private_ip_literal(&host) {
        return true;
    }
    #[cfg(not(test))]
    if host == "localhost" || host.ends_with(".localhost") || host == "127.0.0.1" || host == "::1" || host == "[::1]" {
        return true;
    }
    if NO_PROBE_HOSTS.contains(&host.as_str()) || host.ends_with(".opencode.ai") {
        return true;
    }
    super::vendor_presets::TABLE.iter().any(|preset| {
        let mut bases = vec![preset.chat, preset.responses, preset.anthropic];
        for r in preset.regions {
            bases.extend([r.chat, r.responses, r.anthropic]);
        }
        bases.into_iter().any(|b| !b.is_empty() && hostname_of(b) == host)
    })
}

fn probe_origins(p: &Provider) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in [&p.chat, &p.responses, &p.anthropic] {
        let raw = raw.trim();
        let scheme = raw.split_once("://").map(|(s, _)| s.to_ascii_lowercase()).unwrap_or_default();
        if (scheme != "https" && scheme != "http") || hostname_of(raw).is_empty() {
            continue;
        }
        if not_probed(&hostname_of(raw)) {
            continue;
        }
        let origin = format!("{scheme}://{}", host_of(raw));
        if !out.contains(&origin) {
            out.push(origin);
        }
    }
    out
}

/// Go probesBalance：手加的中继站（无 preset、无账号、有钥匙、没自报 URL、没有
/// 已知来源、不是套餐来源）。
fn probes(p: &Provider) -> bool {
    if extra_bool(p, "noBalanceProbe") || p.key.is_empty() || !p.preset.is_empty() {
        return false;
    }
    if super::quotas::has_balance_source(p) || source_of(p).is_some() {
        return false;
    }
    if super::plan_quotas::plan_source_of(p).is_some() {
        return false;
    }
    for base in [&p.chat, &p.responses, &p.anthropic] {
        if hostname_of(base) == "opencode.ai" || hostname_of(base).ends_with(".opencode.ai") {
            return false;
        }
    }
    !probe_origins(p).is_empty()
}

fn probe_key(origins: &[String], key: &str) -> String {
    let digest = Sha256::digest(key.as_bytes());
    let tag: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("{} {tag}", origins.join(" "))
}

fn probe_balance(p: &Provider, key: &str) -> Option<String> {
    let origins = probe_origins(p);
    let cache_key = probe_key(&origins, key);
    let previous = {
        let cache = probe_cache().lock().unwrap_or_else(|e| e.into_inner());
        cache.get(&cache_key).cloned()
    };
    if let Some((found, until)) = &previous {
        if Instant::now() < *until {
            return found.clone();
        }
    }
    let previous_url = previous.and_then(|(u, _)| u);
    let mut found: Option<String> = None;
    let mut reached = false;
    let mut refused = false;
    'origins: for origin in &origins {
        for path in [SUB2API_KEY_USAGE, NEW_API_KEY_USAGE] {
            let url = format!("{origin}{path}");
            let mut headers = vec![h("Authorization", &format!("Bearer {key}")), h("Accept", "application/json")];
            for (k, v) in &p.headers {
                headers.push((k.clone(), with_key(v, key, false)));
            }
            let Some(reply) = send("GET", &url, &headers, None) else { continue };
            reached = true;
            if previous_url.as_deref() == Some(url.as_str()) && (reply.status == 401 || reply.status == 403) {
                refused = true;
            }
            if reply.status == 200 {
                let field = path_of(&url, "");
                if read_balance_path(&reply.body, &field).is_some_and(|s| !s.is_empty()) {
                    found = Some(url);
                    break 'origins;
                }
            }
        }
    }
    let wait;
    match (&found, &previous_url) {
        (Some(_), _) => wait = PROBE_HIT,
        (None, Some(prev)) if refused || !reached => {
            found = Some(prev.clone());
            wait = if refused { PROBE_HIT } else { PROBE_DOWN };
        }
        (None, _) if !reached => wait = PROBE_DOWN,
        _ => wait = PROBE_MISS,
    }
    probe_cache().lock().unwrap_or_else(|e| e.into_inner()).insert(cache_key, (found.clone(), Instant::now() + wait));
    found
}

// ---------------------------------------------------------------------------
// readBalancePath（标量子集）
// ---------------------------------------------------------------------------

fn path_value<'a>(reply: &'a Value, path: &str) -> Option<&'a Value> {
    let mut v = reply;
    for seg in path.split('.') {
        if seg.is_empty() || seg.contains(['[', ']', '*']) {
            return None;
        }
        v = match v {
            Value::Object(m) => m.get(seg)?,
            Value::Array(a) => a.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(v)
}

struct Expr<'a> {
    src: Vec<char>,
    i: usize,
    reply: &'a Value,
}

impl Expr<'_> {
    fn space(&mut self) {
        while self.i < self.src.len() && self.src[self.i] == ' ' {
            self.i += 1;
        }
    }

    fn number(&mut self) -> Option<f64> {
        let v = self.sum()?;
        (self.i >= self.src.len()).then_some(v)
    }

    fn sum(&mut self) -> Option<f64> {
        let mut v = self.product()?;
        loop {
            self.space();
            match self.src.get(self.i) {
                Some('+') => {
                    self.i += 1;
                    v += self.product()?;
                }
                Some('-') => {
                    self.i += 1;
                    v -= self.product()?;
                }
                _ => return Some(v),
            }
        }
    }

    fn product(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        loop {
            self.space();
            match self.src.get(self.i) {
                Some('*') => {
                    self.i += 1;
                    v *= self.unary()?;
                }
                Some('/') => {
                    self.i += 1;
                    let d = self.unary()?;
                    if d == 0.0 {
                        return None; // Go："divides by 0"
                    }
                    v /= d;
                }
                _ => return Some(v),
            }
        }
    }

    fn unary(&mut self) -> Option<f64> {
        self.space();
        let c = *self.src.get(self.i)?;
        if c == '-' {
            self.i += 1;
            return Some(-self.unary()?);
        }
        if c == '(' {
            self.i += 1;
            let v = self.sum()?;
            self.space();
            if self.src.get(self.i) != Some(&')') {
                return None;
            }
            self.i += 1;
            return Some(v);
        }
        if c.is_ascii_digit() || c == '.' {
            let start = self.i;
            while self.i < self.src.len() && (self.src[self.i].is_ascii_digit() || self.src[self.i] == '.') {
                self.i += 1;
            }
            return self.src[start..self.i].iter().collect::<String>().parse().ok();
        }
        let start = self.i;
        while self.i < self.src.len() && !"+-*/() ".contains(self.src[self.i]) {
            self.i += 1;
        }
        if start == self.i {
            return None;
        }
        let path: String = self.src[start..self.i].iter().collect();
        num(path_value(self.reply, &path))
    }
}

fn balance_amount(sign: &str, n: f64, percent: bool) -> String {
    if percent {
        let s = format!("{:.1}", n * 100.0);
        return format!("{sign}{}%", s.strip_suffix(".0").unwrap_or(&s));
    }
    money(sign, n)
}

fn read_balance_one(reply: &Value, path: &str) -> Option<String> {
    let mut path = path.trim();
    let mut sign = "";
    for s in ["$", "¥", "€", "£"] {
        if let Some(rest) = path.strip_prefix(s) {
            sign = s;
            path = rest.trim();
            break;
        }
    }
    let mut percent = false;
    if let Some(rest) = path.strip_suffix('%') {
        percent = true;
        path = rest.trim();
    }
    if path.is_empty() {
        return None;
    }
    let lone = !path.contains(['+', '-', '*', '/', '(', ')', ' ', '[']) && !path.starts_with(|c: char| c.is_ascii_digit());
    if lone {
        let v = path_value(reply, path)?;
        if let Some(n) = num(Some(v)) {
            return Some(balance_amount(sign, n, percent));
        }
        return match v.as_str() {
            Some(s) if !s.is_empty() && !percent => Some(format!("{sign}{s}")),
            _ => None,
        };
    }
    let mut expr = Expr { src: path.chars().collect(), i: 0, reply };
    let n = expr.number().filter(|n| n.is_finite())?;
    Some(balance_amount(sign, n, percent))
}

/// Go readBalancePath：多段（`;` 分隔，可带 `label:`）整行 `label text · …`；
/// 一段读不出显示 `—`，全读不出才失败。
pub(super) fn read_balance_path(body: &str, path: &str) -> Option<String> {
    let reply: Value = serde_json::from_str(body).ok()?;
    if !path.contains(';') && !path.contains(':') {
        return read_balance_one(&reply, path);
    }
    let mut parts: Vec<String> = Vec::new();
    let mut read = false;
    for part in path.split(';') {
        if part.trim().is_empty() {
            continue;
        }
        let (label, expr) = match part.split_once(':') {
            Some((l, e)) => (l.trim(), e),
            None => ("", part),
        };
        let (text, ok) = match read_balance_one(&reply, expr) {
            Some(t) => (t, true),
            None => ("—".to_string(), false),
        };
        read |= ok;
        parts.push(if label.is_empty() { text } else { format!("{label} {text}") });
    }
    (read && !parts.is_empty()).then(|| parts.join(" · "))
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

fn row_of(p: &Provider, user: &str, balance: &str) -> Value {
    let mut row = json!({
        "provider": p.id,
        "name": p.name,
        "windows": [],
        "balance": balance,
    });
    if !user.is_empty() {
        row["user"] = json!(user);
    }
    row
}

fn card_for(p: &Provider, key: &str, user: &str) -> Option<Value> {
    let src = match source_of(p) {
        Some(src) => src,
        None if probes(p) => {
            let found = probe_balance(p, key)?;
            let mut q = p.clone();
            q.extra.insert("balanceURL".into(), json!(found));
            source_of(&q)?
        }
        None => return None,
    };
    let balance = ask(p, &src, key)?;
    Some(row_of(p, user, &balance))
}

/// 补全来源的余额行。没有已知读法 / 读不出 = 不出行；账号级读数（Command Code /
/// Ollama / 带 token 的）同来源同读数只留一行。
pub fn rows(providers: &[Provider]) -> Vec<Value> {
    struct Job<'a> {
        provider: &'a Provider,
        key: String,
        user: String,
    }
    let mut jobs: Vec<Job> = Vec::new();
    for p in providers {
        if p.key.is_empty() || skipped(p) {
            continue;
        }
        // quotas.rs 的六家 host 来源让位给它（除非用户自报了 balanceURL，那由本文件接管）
        if !takes_over(p) && super::quotas::has_balance_source(p) {
            continue;
        }
        let keys = keys_of(p);
        let wide = source_of(p).is_some_and(|s| s.account_wide);
        let usable: Vec<(String, String)> = if wide && !effective_token(p).is_empty() {
            vec![(p.key.clone(), String::new())] // 账号 token：钥匙无关，只问一次
        } else {
            keys
        };
        for (key, user) in usable {
            jobs.push(Job { provider: p, key, user });
        }
    }
    let mut results: Vec<Option<Value>> = Vec::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .iter()
            .map(|job| scope.spawn(move || card_for(job.provider, &job.key, &job.user)))
            .collect();
        for handle in handles {
            results.push(handle.join().ok().flatten());
        }
    });
    let mut seen: Vec<(String, String)> = Vec::new();
    let mut out = Vec::new();
    for (job, row) in jobs.iter().zip(results) {
        let Some(row) = row else { continue };
        if source_of(job.provider).is_some_and(|s| s.account_wide) {
            let id = (job.provider.id.clone(), text(row.get("balance")).to_string());
            if seen.contains(&id) {
                continue;
            }
            seen.push(id);
        }
        out.push(row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::super::quota_kit::testkit::{redirect, Mock, Redirects};
    use super::*;

    fn provider(json: &str) -> Provider {
        serde_json::from_str(json).expect("provider json")
    }

    #[test]
    fn path_expressions_follow_go() {
        let body = r#"{"data":{"quota":2500000,"total_available":1000000,"balance":12.5,"list":[{"v":3},{"v":4}],
            "name":"abc"},"remaining":7,"win":{"used":1,"cap":4}}"#;
        assert_eq!(read_balance_path(body, "$data.quota / 500000").as_deref(), Some("$5.00"));
        assert_eq!(read_balance_path(body, "$data.total_available / 500000").as_deref(), Some("$2.00"));
        assert_eq!(read_balance_path(body, "$data.balance").as_deref(), Some("$12.50"));
        assert_eq!(read_balance_path(body, "¥remaining").as_deref(), Some("¥7.00"));
        assert_eq!(read_balance_path(body, "data.list.1.v").as_deref(), Some("4.00"));
        assert_eq!(read_balance_path(body, "(data.balance - 2.5) * 2").as_deref(), Some("20.00"));
        // 字符串字段原样（不是数字）
        assert_eq!(read_balance_path(body, "data.name").as_deref(), Some("abc"));
        // 百分比：win.used / win.cap → 25%
        assert_eq!(read_balance_path(body, "win.used / win.cap %").as_deref(), Some("25%"));
        // 多段 + 读不出的一段显示 —
        assert_eq!(
            read_balance_path(body, "5h: win.used / win.cap %; week: nope.x; $remaining").as_deref(),
            Some("5h 25% · week — · $7.00")
        );
        assert!(read_balance_path(body, "5h: nope.a; week: nope.b").is_none(), "全读不出才失败");
        assert!(read_balance_path(body, "nope.field").is_none());
        assert!(read_balance_path(body, "data.balance / 0").is_none(), "除零");
        assert!(read_balance_path(body, "data.*.x").is_none(), "通配不在标量子集内");
        assert!(read_balance_path("not json", "data.balance").is_none());
        // 字符串金额
        assert_eq!(read_balance_path(r#"{"b":"3.5"}"#, "$b").as_deref(), Some("$3.50"));
    }

    #[test]
    fn default_paths_for_known_relay_queries() {
        assert_eq!(path_of("https://r.test/api/user/self", ""), NEW_API_QUOTA_PATH);
        assert_eq!(path_of("https://r.test/api/usage/token", ""), NEW_API_KEY_PATH);
        assert_eq!(path_of("https://r.test/v1/usage", ""), "$remaining");
        assert_eq!(path_of("https://r.test/api/v1/user/profile", ""), "$data.balance");
        assert_eq!(path_of("https://r.test/dashboard/billing/credit_grants", ""), "$total_available");
        assert_eq!(path_of("https://r.test/anything", ""), "");
        assert_eq!(path_of("https://r.test/anything", "$x.y"), "$x.y");
    }

    #[test]
    fn account_query_without_token_asks_the_keys_own() {
        let p = provider(
            r#"{"id":"r","name":"r","key":"sk-k","chat":"https://r.test/v1","balanceURL":"https://r.test/api/user/self"}"#,
        );
        let src = source_of(&p).expect("source");
        assert_eq!(src.url, "https://r.test/api/usage/token", "无 token 的账号查询退到钥匙自己的查询");
        assert_eq!(src.reader, Reader::Path(NEW_API_KEY_PATH.to_string()));
        // 有 token：账号查询原样，token 当整个 Authorization
        let p = provider(
            r#"{"id":"r","name":"r","key":"sk-k","chat":"https://r.test/v1","balanceURL":"https://r.test/api/user/self","balanceToken":"acc-token"}"#,
        );
        let src = source_of(&p).expect("source");
        assert_eq!(src.url, "https://r.test/api/user/self");
        assert_eq!(src.token, "acc-token");
        assert!(src.account_wide);
        // token 只是钥匙自己（可带 Bearer）= 当没有 token
        let p = provider(
            r#"{"id":"r","name":"r","key":"sk-k","chat":"https://r.test/v1","balanceURL":"https://r.test/api/user/self","balanceToken":"Bearer sk-k"}"#,
        );
        assert_eq!(effective_token(&p), "");
        // 自带 Authorization 头 = 不改写
        let p = provider(
            r#"{"id":"r","name":"r","key":"sk-k","chat":"https://r.test/v1","balanceURL":"https://r.test/api/user/self","headers":{"Authorization":"x"}}"#,
        );
        assert_eq!(source_of(&p).unwrap().url, "https://r.test/api/user/self");
    }

    #[test]
    fn key_placeholders_and_authorization_shapes() {
        assert_eq!(with_key("https://a/b?key={key}&x={API_KEY}&t=${token}&q={{apiKey}}", "k 1", true), "https://a/b?key=k+1&x=k+1&t=k+1&q=k+1");
        assert_eq!(with_key("Bearer {token}", "k 1", false), "Bearer k 1");
        assert_eq!(with_key("{other}", "k", false), "{other}");
        assert_eq!(balance_authorization("plain-token"), "plain-token");
        assert_eq!(balance_authorization("eyJa.b.c"), "Bearer eyJa.b.c");
        assert_eq!(balance_authorization("Bearer x"), "Bearer x");
    }

    #[test]
    fn host_table_picks_the_four_missing_vendors() {
        let p = provider(r#"{"id":"c","name":"c","key":"k","chat":"https://api.commandcode.ai/v1"}"#);
        assert_eq!(source_of(&p).unwrap().reader, Reader::CommandCode);
        let p = provider(r#"{"id":"m","name":"m","key":"k","chat":"https://api-inference.modelscope.cn/v1"}"#);
        assert_eq!(source_of(&p).unwrap().reader, Reader::ModelScope);
        let p = provider(r#"{"id":"o","name":"o","key":"k","chat":"https://ollama.com/v1"}"#);
        assert_eq!(source_of(&p).unwrap().reader, Reader::OllamaCredits);
        let p = provider(r#"{"id":"a","name":"a","key":"k","chat":"https://aihubmix.com/v1"}"#);
        assert_eq!(source_of(&p).unwrap().reader, Reader::AiHubMixKey);
        let p = provider(r#"{"id":"a","name":"a","key":"k","chat":"https://aihubmix.com/v1","balanceToken":"t"}"#);
        let src = source_of(&p).unwrap();
        assert_eq!(src.reader, Reader::AiHubMixAccount);
        assert_eq!(src.url, AIHUBMIX_ACCOUNT_URL);
        let p = provider(r#"{"id":"x","name":"x","key":"k","chat":"https://nobody.test/v1"}"#);
        assert!(source_of(&p).is_none());
    }

    #[test]
    fn vendor_readers() {
        assert_eq!(
            read_with(&Reader::CommandCode, r#"{"credits":{"planId":"x","monthlyCredits":41.2,"purchasedCredits":5,"freeCredits":0}}"#).as_deref(),
            Some("$46.20")
        );
        assert_eq!(
            read_with(&Reader::ModelScope, r#"{"success":true,"data":{"total_balance":1500,"available_balance":1300,"frozen_amount":200}}"#).as_deref(),
            Some("1300 魔粒")
        );
        assert_eq!(
            read_with(&Reader::OllamaCredits, r#"{"included":{"balance_usd":72.5},"purchased":{"balance_usd":25.0}}"#).as_deref(),
            Some("$25.00")
        );
        assert_eq!(
            read_with(&Reader::OllamaCredits, r#"{"included":{"balance_usd":72.5}}"#),
            None,
            "只有含额没有买的 credits：不出余额卡"
        );
        assert_eq!(read_with(&Reader::AiHubMixKey, r#"{"object":"list","total_usage":12.5}"#).as_deref(), Some("$12.50"));
        assert_eq!(read_with(&Reader::AiHubMixKey, r#"{"total_usage":-1}"#), None, "无限额钥匙没有自己的余额");
        assert_eq!(
            read_with(&Reader::AiHubMixAccount, r#"{"success":true,"data":{"quota":2500000}}"#).as_deref(),
            Some("$5.00")
        );
        assert_eq!(read_with(&Reader::AiHubMixAccount, r#"{"success":false,"message":"no"}"#), None);
        assert!(refusal(r#"{"success":false,"message":"bad token"}"#));
        assert!(!refusal(r#"{"success":false}"#));
        assert!(!refusal(r#"{"success":true,"message":"x"}"#));
    }

    #[test]
    fn rows_end_to_end_with_mock_vendors_and_dedupe() {
        let _lock = super::super::TEST_SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_probes_for_tests();
        let mock = Mock::start();
        mock.route(
            "/alpha/billing/credits",
            200,
            r#"{"credits":{"planId":"individual-go","monthlyCredits":3,"purchasedCredits":1,"freeCredits":0}}"#,
        );
        mock.route("/api/balance", 200, r#"{"included":{"balance_usd":10,"allowance_usd":40},"purchased":{"balance_usd":0}}"#);
        mock.route("/openapi/v1/magicubes/balance", 200, r#"{"success":true,"data":{"available_balance":1300}}"#);
        mock.route("/dashboard/billing/remain", 200, r#"{"total_usage":8.25}"#);
        mock.route("/relay/balance", 200, r#"{"data":{"left":12.5}}"#);
        let _r = Redirects::new(&[
            redirect("https://api.commandcode.ai", &mock.base),
            redirect("https://ollama.com", &mock.base),
            redirect("https://modelscope.cn", &mock.base),
            redirect("https://aihubmix.com", &mock.base),
            redirect("https://relay.test/relay", &format!("{}/relay", mock.base)),
        ]);
        let providers = vec![
            provider(r#"{"id":"cc","name":"Command Code","key":"sk-cc","chat":"https://api.commandcode.ai/v1","keys":[{"key":"sk-cc2"}]}"#),
            provider(r#"{"id":"ol","name":"ollama","key":"sk-ol","chat":"https://ollama.com/v1"}"#),
            provider(r#"{"id":"ms","name":"ModelScope","key":"sk-ms","chat":"https://api-inference.modelscope.cn/v1"}"#),
            provider(r#"{"id":"ah","name":"AiHubMix","key":"sk-ah","chat":"https://aihubmix.com/v1"}"#),
            provider(r#"{"id":"relay","name":"Relay","key":"sk-rl","chat":"https://relay.test/v1","balanceURL":"https://relay.test/relay/balance","balancePath":"$data.left"}"#),
            // 自报 balanceURL 的 DeepSeek host：由本文件接管（Go：BalanceURL 优先）
            provider(r#"{"id":"ds","name":"DS","key":"sk-ds","chat":"https://api.deepseek.com/v1","balanceURL":"https://relay.test/relay/balance","balancePath":"$data.left"}"#),
            provider(r#"{"id":"nokey","name":"x","chat":"https://ollama.com/v1"}"#),
            provider(r#"{"id":"gone","name":"x","key":"k","off":true,"chat":"https://ollama.com/v1"}"#),
        ];
        let rows = rows(&providers);
        let by_id = |id: &str| rows.iter().filter(|r| r["provider"] == id).count();
        assert_eq!(by_id("cc"), 1, "账号级读数同来源同读数折成一行");
        assert_eq!(rows.iter().find(|r| r["provider"] == "cc").unwrap()["balance"], "$4.00");
        assert_eq!(rows.iter().find(|r| r["provider"] == "ol").unwrap()["balance"], "$0.00", "purchased 给了 0 就是真实读数（Go 同款，owner 机上也是 $0.00）");
        assert_eq!(rows.iter().find(|r| r["provider"] == "ms").unwrap()["balance"], "1300 魔粒");
        assert_eq!(rows.iter().find(|r| r["provider"] == "ah").unwrap()["balance"], "$8.25");
        assert_eq!(rows.iter().find(|r| r["provider"] == "relay").unwrap()["balance"], "$12.50");
        assert_eq!(rows.iter().find(|r| r["provider"] == "ds").unwrap()["balance"], "$12.50");
        assert_eq!(rows.iter().find(|r| r["provider"] == "ms").unwrap()["windows"], json!([]));
        assert_eq!(by_id("nokey") + by_id("gone"), 0);
        let requests = mock.requests();
        let cc = requests.iter().find(|r| r.path.starts_with("/alpha/billing/credits")).unwrap();
        assert_eq!(cc.header("authorization"), "Bearer sk-cc");
    }

    #[test]
    fn relay_probe_finds_the_query_and_caches_misses() {
        let _lock = super::super::TEST_SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_probes_for_tests();
        let mock = Mock::start();
        // sub2api 的 /v1/usage 没有，new-api 的 /api/usage/token 有
        mock.route("/api/usage/token", 200, r#"{"code":true,"data":{"total_available":1500000}}"#);
        let _r = Redirects::new(&[redirect("https://relay.test", &mock.base)]);
        let providers = vec![provider(r#"{"id":"relay","name":"Relay","key":"sk-rl","chat":"https://relay.test/v1"}"#)];
        let first = rows(&providers);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0]["balance"], "$3.00", "1500000 / 500000");
        let seen_before = mock.requests().len();
        let second = rows(&providers);
        assert_eq!(second, first);
        // 命中缓存：只剩余额那一次请求，不再探 /v1/usage
        let after = mock.requests();
        assert_eq!(after.len(), seen_before + 1);
        assert!(after.last().unwrap().path.starts_with("/api/usage/token"));

        // 什么都没有的中继：探一轮（两个路径）后缓存 miss，下次不再打
        reset_probes_for_tests();
        let mock2 = Mock::start();
        let _r2 = Redirects::new(&[redirect("https://empty.test", &mock2.base)]);
        let empty = vec![provider(r#"{"id":"empty","name":"E","key":"sk-e","chat":"https://empty.test/v1"}"#)];
        assert!(rows(&empty).is_empty());
        let n = mock2.requests().len();
        assert_eq!(n, 2);
        assert!(rows(&empty).is_empty());
        assert_eq!(mock2.requests().len(), n, "miss 缓存 6h");
    }

    #[test]
    fn probe_skips_vendors_presets_and_opted_out() {
        let p = provider(r#"{"id":"o","name":"o","key":"k","chat":"https://api.openai.com/v1"}"#);
        assert!(!probes(&p), "厂商自己的 host 不探测");
        let p = provider(r#"{"id":"d","name":"d","key":"k","chat":"https://api.deepseek.com/v1"}"#);
        assert!(!probes(&p));
        let p = provider(r#"{"id":"r","name":"r","key":"k","chat":"https://relay.test/v1","noBalanceProbe":true}"#);
        assert!(!probes(&p));
        let p = provider(r#"{"id":"r","name":"r","key":"k","chat":"https://relay.test/v1","preset":"deepseek"}"#);
        assert!(!probes(&p), "有 preset 的不是手加中继");
        let p = provider(r#"{"id":"r","name":"r","key":"k","chat":"https://opencode.ai/inference/go/v1"}"#);
        assert!(!probes(&p), "套餐来源不探测余额");
        let p = provider(r#"{"id":"r","name":"r","key":"k","chat":"https://relay.test/v1"}"#);
        assert!(probes(&p));
        // 局域网 / Tailscale 的 IP 字面量不是中继站：不拿钥匙去探
        let p = provider(r#"{"id":"r","name":"r","key":"k","chat":"http://100.94.137.45:8082/v1"}"#);
        assert!(!probes(&p));
        assert!(private_ip_literal("192.168.1.5") && private_ip_literal("172.20.0.1") && private_ip_literal("10.0.0.2"));
        assert!(!private_ip_literal("8.8.8.8") && !private_ip_literal("172.32.0.1") && !private_ip_literal("relay.test"));
    }
}
