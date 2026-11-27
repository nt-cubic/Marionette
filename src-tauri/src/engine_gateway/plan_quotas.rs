//! 按钥匙买的套餐额度（合同 C3 的 plan 类卡）——Go `planquota.go` 的移植。
//!
//! 一把钥匙背后的套餐（智谱/Z.ai GLM Coding Plan、OpenCode Go、Kimi Code、
//! Command Code、MiniMax Coding Plan、Ollama Cloud 含额、Mistral Vibe）有窗口
//! 额度，厂商只告诉钥匙本身。来源判定、端点、字段读法、窗口命名照抄 Go
//! （planQuotaSourceOf / readZhipuPlan / readOpenCodeGo / readKimiCode /
//! readMiniMaxPlan / readOllamaPlan / cmdKeyWindows / mistralKeyWindows），别猜。
//!
//! **刻意不做**（需要账号登录或云厂商访问密钥，不是「钥匙 → 窗口」）：
//! StepFun Step Plan / 千问 Token Plan（登录态，stepfun_plan.go /
//! qianwen_plan.go）、火山方舟 / 腾讯云 / 阿里云百炼（access key，
//! volcengine_usage.go / tencent_usage.go）；智谱「重置券」明细
//! （customer-package-reset，只读展示，面板暂无位置）；按钥匙 plan 区域
//! （KeyAccount.Plan）。
//!
//! 红线：只读；出站失败安静跳过（不出行）；HTTP 一律在调用方已放 store 锁之后
//! （本模块拿到的是 provider 快照）；单来源 10s 封顶（quota_kit::FETCH_TIMEOUT）；
//! 钥匙只进 header，不进任何字符串/日志。
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::quota_kit::*;
use super::store::Provider;

const OLLAMA_BALANCE_URL: &str = "https://ollama.com/api/balance";
const COMMAND_CODE_API: &str = "https://api.commandcode.ai";
const MISTRAL_BUDGET_URL: &str = "https://api.mistral.ai/v1/billing/budget";
const MISTRAL_SUBSCRIPTION_URL: &str = "https://api.mistral.ai/v1/billing/subscription";
const MISTRAL_WHOAMI_URL: &str = "https://console.mistral.ai/api/vibe/whoami";
const ZCODE_APP_VERSION: &str = "3.14.3";

/// 一个套餐来源的读法。
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Kind {
    /// 智谱 / Z.ai：host 是 open.bigmodel.cn 或 api.z.ai；coding = base 落在
    /// `/api/coding/` 上（是套餐而不只是厂商）。
    Zhipu {
        host: String,
        coding: bool,
    },
    OpenCodeGo,
    Kimi {
        url: String,
    },
    CommandCode,
    MiniMax {
        host: String,
    },
    Ollama,
    Mistral,
}

/// 读到的套餐：名称、窗口、套餐期（智谱）。
pub(super) struct Read {
    pub plan: String,
    pub windows: Vec<Win>,
    pub until: Option<DateTime<Utc>>,
    pub renew: String,
}

impl Read {
    fn new(plan: &str, windows: Vec<Win>) -> Read {
        Read {
            plan: plan.to_string(),
            windows,
            until: None,
            renew: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// 来源判定
// ---------------------------------------------------------------------------

fn url_path(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    match rest.find('/') {
        Some(i) => rest[i..].split(['?', '#']).next().unwrap_or(""),
        None => "",
    }
}

/// Go openCodeGateway：`"go"`（…/zen/go、…/inference/go）/ `"zen"` / `""`。
fn open_code_gateway(base: &str) -> &'static str {
    if !base.to_ascii_lowercase().contains("opencode.ai") {
        return "";
    }
    let host = hostname_of(base);
    let host = host.trim_end_matches('.');
    if host != "opencode.ai" && !host.ends_with(".opencode.ai") {
        return "";
    }
    let path = url_path(base.trim());
    let mut seg = path.trim_matches('/').split('/');
    match seg.next().unwrap_or("") {
        "zen" | "inference" => {}
        _ => return "",
    }
    match seg.next() {
        Some("go") => "go",
        Some("custom") => "",
        _ => "zen",
    }
}

fn kimi_usage_url(base: &str) -> String {
    let mut u = base.trim_end_matches('/').to_string();
    if u.ends_with("/coding") {
        u.push_str("/v1");
    }
    format!("{}/usages", u.trim_end_matches('/'))
}

/// Go planQuotaSourceOf：chat / anthropic / responses 逐个看，先命中先得。
pub(super) fn plan_source_of(p: &Provider) -> Option<Kind> {
    for base in [&p.chat, &p.anthropic, &p.responses] {
        let coding = base.contains("/api/coding/");
        if open_code_gateway(base) == "go" {
            return Some(Kind::OpenCodeGo);
        }
        match host_of(base).as_str() {
            h @ ("open.bigmodel.cn" | "api.z.ai") => {
                return Some(Kind::Zhipu {
                    host: h.to_string(),
                    coding,
                })
            }
            "api.kimi.com" | "api.kimi.ai" => {
                if base.contains("/coding") {
                    return Some(Kind::Kimi {
                        url: kimi_usage_url(base),
                    });
                }
            }
            "api.commandcode.ai" => return Some(Kind::CommandCode),
            h @ ("api.minimaxi.com" | "api.minimax.cn" | "api.minimax.io") => {
                return Some(Kind::MiniMax {
                    host: h.to_string(),
                })
            }
            "ollama.com" => return Some(Kind::Ollama),
            "api.mistral.ai" => return Some(Kind::Mistral),
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 出站
// ---------------------------------------------------------------------------

static SESSION_SEQ: AtomicU64 = AtomicU64::new(0);

/// OpenCode 网关要求每个请求带会话（Go OpenCodeSession）：`ses_` + 12 位十六进制
/// （毫秒×0x1000+计数，取反——最新排前）+ 14 位 base62 随机。
fn opencode_session_id() -> String {
    const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seq = SESSION_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let current = !((now.as_millis() as u64)
        .wrapping_mul(0x1000)
        .wrapping_add(seq & 0xfff));
    let digest =
        Sha256::digest(format!("{}-{}-{}", now.as_nanos(), seq, std::process::id()).as_bytes());
    let tail: String = digest[..14]
        .iter()
        .map(|b| BASE62[*b as usize % BASE62.len()] as char)
        .collect();
    format!("ses_{:012x}{}", current & 0xffff_ffff_ffff, tail)
}

fn plan_headers(url: &str, key: &str, bearer: bool) -> Vec<(String, String)> {
    let mut headers = vec![
        h(
            "Authorization",
            &if bearer {
                format!("Bearer {key}")
            } else {
                key.to_string()
            },
        ),
        h("Accept", "application/json"),
        h("Accept-Language", "en-US,en"),
    ];
    let host = hostname_of(url);
    if host == "opencode.ai" || host.ends_with(".opencode.ai") {
        let id = opencode_session_id();
        headers.push(h("x-opencode-session", &id));
        headers.push(h("x-opencode-session-id", &id));
    }
    headers
}

/// Go planBody：状态 < 300 才有体；其余一律当读不出（安静跳过）。
fn plan_body(url: &str, key: &str, bearer: bool) -> Option<String> {
    let reply = get(url, &plan_headers(url, key, bearer))?;
    reply.ok().then_some(reply.body)
}

// ---------------------------------------------------------------------------
// 各家读法
// ---------------------------------------------------------------------------

#[derive(Debug, PartialEq)]
enum ReadErr {
    /// 厂商拒了钥匙本身（智谱 1000–1005 / 401）——团队套餐兜底也不必问。
    Refused,
    Other,
}

/// Go readZhipuPlan。`{"success":true,"data":{"level":"pro","limits":[…]}}`：
/// unit 3 = 小时、6 = 周；TIME_LIMIT 是月度 MCP 调用（不挡模型，aside）。
fn read_zhipu(body: &str) -> Result<(String, Vec<Win>), ReadErr> {
    let v: Value = serde_json::from_str(body).map_err(|_| ReadErr::Other)?;
    let success_false = v.get("success").and_then(Value::as_bool) == Some(false);
    if success_false {
        let code = match v.get("code") {
            Some(Value::Number(n)) => n.as_i64().unwrap_or(0),
            Some(Value::String(s)) => s.trim().parse::<i64>().unwrap_or(0),
            _ => 0,
        };
        let msg = text(v.get("msg")).trim();
        if (code != 0 || !msg.is_empty()) && (1000..=1005).contains(&code) {
            return Err(ReadErr::Refused);
        }
        return Err(ReadErr::Other);
    }
    let data = v
        .get("data")
        .filter(|d| d.is_object())
        .ok_or(ReadErr::Other)?;
    let level = text(data.get("level")).to_string();
    let mut out = Vec::new();
    for l in data
        .get("limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let mut w = Win::new("", num(l.get("percentage")).unwrap_or(0.0));
        let reset = num(l.get("nextResetTime")).unwrap_or(0.0) as i64;
        if reset > 0 {
            w.resets_at = from_millis(reset);
        }
        let unit = num(l.get("unit")).unwrap_or(0.0) as i64;
        let number = num(l.get("number")).unwrap_or(0.0) as i64;
        if text(l.get("type")).eq_ignore_ascii_case("TIME_LIMIT") {
            w.name = "MCP · Month".into();
            w.aside = true;
        } else if unit == 3 {
            // 团队套餐的五小时不带 number
            let n = if number <= 0 { 5 } else { number.max(1) };
            w.name = format!("{n} hours");
            w.span_secs = n * 3600;
        } else if unit == 6 {
            w.name = "7 days".into();
            w.span_secs = 7 * 86400;
        } else {
            w.name = "Allowance".into();
        }
        out.push(w);
    }
    Ok((level, out))
}

/// 智谱套餐期（Go zhipuTermOf）：有效订阅的下次续费（自动续费 "auto"，否则
/// "off"），读不到续费时间取 valid 区间的最后日期。
pub(super) fn zhipu_term_of(subs: &Value) -> Option<(DateTime<Utc>, &'static str)> {
    for s in subs.as_array()? {
        if !text(s.get("status")).eq_ignore_ascii_case("VALID") {
            continue;
        }
        // autoRenew 是 true 或数字 1（Go：AutoRenew == true || == float64(1)）
        let auto = match s.get("autoRenew") {
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_f64() == Some(1.0),
            _ => false,
        };
        if let Some(t) = beijing_time(text(s.get("nextRenewTime"))) {
            return Some((t, if auto { "auto" } else { "off" }));
        }
        if !auto {
            // valid 区间 "2026-09-18 12:00:00-2026-10-18 12:00:00"：最后一个日期
            if let Some(last) = last_date(text(s.get("valid"))) {
                if let Some(t) = beijing_time(&last) {
                    return Some((t, "off"));
                }
            }
        }
        return None;
    }
    None
}

/// 取串里最后一个 `YYYY-MM-DD( HH:MM:SS)?`（Go zhipuDate 的 FindAll 末项）。
fn last_date(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let is_digit = |i: usize| bytes.get(i).is_some_and(u8::is_ascii_digit);
    let mut found: Option<String> = None;
    let mut i = 0;
    while i + 10 <= bytes.len() {
        let date = (0..4).all(|k| is_digit(i + k))
            && bytes[i + 4] == b'-'
            && (5..7).all(|k| is_digit(i + k))
            && bytes[i + 7] == b'-'
            && (8..10).all(|k| is_digit(i + k));
        if date {
            let mut end = i + 10;
            if i + 19 <= bytes.len()
                && (bytes[end] == b' ' || bytes[end] == b'T')
                && [1, 2, 4, 5, 7, 8].iter().all(|k| is_digit(end + k))
                && bytes[end + 3] == b':'
                && bytes[end + 6] == b':'
            {
                end += 9;
            }
            found = Some(s[i..end].to_string());
            i = end;
        } else {
            i += 1;
        }
    }
    found
}

/// 智谱团队套餐窗口（Go zhipuKeyTeamWindows）：type=2，团队的 org/project 进
/// header（用户给了才带——没给是猜，Go 同款）；自己的 host 先、再 ZCode 问的地方。
fn zhipu_team_windows(
    host: &str,
    key: &str,
    team: Option<(String, String)>,
) -> Option<(String, Vec<Win>)> {
    let own = format!("https://{host}");
    let biz = if host == "open.bigmodel.cn" || host.ends_with("bigmodel.cn") {
        "https://bigmodel.cn".to_string()
    } else {
        own.clone()
    };
    let mut roots = vec![own.clone()];
    if biz != own {
        roots.push(biz.clone());
    }
    for root in roots {
        let mut headers = vec![
            h("Authorization", key),
            h("Accept", "application/json"),
            h("Accept-Language", "en-US,en"),
        ];
        if let Some((org, project)) = &team {
            headers.push(h("Bigmodel-Organization", org));
            headers.push(h("Bigmodel-Project", project));
            headers.push(h(
                "Set-Language",
                if biz == "https://bigmodel.cn" {
                    "zh"
                } else {
                    "en"
                },
            ));
        }
        let url = format!("{root}/api/monitor/usage/quota/limit?type=2");
        let Some(reply) = get(&url, &headers) else {
            continue;
        };
        if !reply.ok() {
            continue;
        }
        if let Ok((_, ws)) = read_zhipu(&reply.body) {
            if !ws.is_empty() {
                return Some(("GLM Coding Team".to_string(), ws));
            }
        }
    }
    None
}

fn read_zhipu_plan(p: &Provider, host: &str, key: &str) -> Option<Read> {
    let root = format!("https://{host}");
    let url = format!("{root}/api/monitor/usage/quota/limit");
    let result: Result<(String, Vec<Win>), ReadErr> =
        match get(&url, &plan_headers(&url, key, false)) {
            None => Err(ReadErr::Other),
            Some(r) if r.status == 401 => Err(ReadErr::Refused),
            Some(r) if !r.ok() => Err(ReadErr::Other),
            Some(r) => read_zhipu(&r.body),
        };
    let refused = matches!(result, Err(ReadErr::Refused));
    let empty = !matches!(&result, Ok((_, ws)) if !ws.is_empty());
    if empty && !refused {
        let team = {
            let t = p.extra.get("zhipuTeam");
            let org = text(t.and_then(|t| t.get("org"))).trim().to_string();
            let project = text(t.and_then(|t| t.get("project"))).trim().to_string();
            (!org.is_empty() && !project.is_empty()).then_some((org, project))
        };
        if let Some((plan, ws)) = zhipu_team_windows(host, key, team) {
            return Some(Read::new(&plan, ws));
        }
    }
    let (level, ws) = result.ok()?;
    if ws.is_empty() {
        return None;
    }
    let mut read = Read::new(&level, ws);
    // 套餐期（#1191）：订阅列表，同一把钥匙问
    let sub_url = format!("{root}/api/biz/subscription/list");
    let headers = vec![
        h("Authorization", key),
        h("Accept", "application/json"),
        h("Accept-Language", "en-US,en"),
        h("User-Agent", &format!("ZCode/{ZCODE_APP_VERSION}")),
    ];
    if let Some(subs) = call_env(&sub_url, &headers) {
        if let Some((until, renew)) = zhipu_term_of(&subs) {
            read.until = Some(until);
            read.renew = renew.to_string();
        }
    }
    Some(read)
}

/// Go readOpenCodeGo：rolling / weekly / monthly；0% 的窗口 resetsAt 是「现在 +
/// 窗口长度」（什么都不会发生的时刻），丢掉。
fn read_opencode_go(body: &str) -> Option<Vec<Win>> {
    let v: Value = serde_json::from_str(body).ok()?;
    let usage = v.get("usage")?;
    let mut out = Vec::new();
    for (key, name, span) in [
        ("rolling", "5 hours", 5 * 3600),
        ("weekly", "7 days", 7 * 86400),
        ("monthly", "Month", 0),
    ] {
        let Some(w) = usage.get(key) else { continue };
        let Some(percent) = num(w.get("percent")) else {
            continue;
        };
        let mut win = Win::new(name, percent);
        win.span_secs = span;
        if percent > 0.0 {
            win.resets_at = parse_rfc3339(text(w.get("resetsAt")));
        }
        out.push(win);
    }
    (!out.is_empty()).then_some(out)
}

fn reset_of(d: &Value) -> Option<DateTime<Utc>> {
    for k in ["resetTime", "resetAt", "reset_at", "reset_time"] {
        let s = text(d.get(k));
        if !s.is_empty() {
            if let Some(t) = parse_rfc3339(s) {
                return Some(t);
            }
        }
        if let Some(n) = num(d.get(k)) {
            if n > 0.0 {
                return if n > 1e11 {
                    from_millis(n as i64)
                } else {
                    from_secs(n as i64)
                };
            }
        }
    }
    None
}

fn camel(s: &str) -> String {
    let mut parts = s.split('_');
    let mut out = parts.next().unwrap_or("").to_string();
    for part in parts {
        let mut chars = part.chars();
        if let Some(c) = chars.next() {
            out.extend(c.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

fn span_name(span: i64) -> String {
    if span == 30 * 86400 {
        "Month".to_string()
    } else if span > 86400 && span % 86400 == 0 {
        format!("{} days", span / 86400)
    } else if span >= 3600 && span % 3600 == 0 {
        format!("{} hours", span / 3600)
    } else if span > 0 {
        format!("{} minutes", span / 60)
    } else {
        "Allowance".to_string()
    }
}

/// Go readKimiCode：`usage`（周额度）+ `limits[]`（更短窗口）+ `usages` 份额
/// （月度计划）+ 加油包钱包。
fn read_kimi(body: &str) -> Option<Vec<Win>> {
    let r: Value = serde_json::from_str(body).ok()?;
    let window = |d: &Value, name: &str, span: i64| -> Option<Win> {
        let limit = num(d.get("limit")).filter(|l| *l > 0.0)?;
        let used = match num(d.get("used")) {
            Some(u) => u,
            None => limit - num(d.get("remaining"))?,
        };
        let mut w = Win::new(name, (used / limit * 100.0).clamp(0.0, 100.0));
        w.span_secs = span;
        w.resets_at = reset_of(d);
        Some(w)
    };
    let mut out: Vec<Win> = Vec::new();
    for l in r
        .get("limits")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let d = l
            .get("detail")
            .filter(|d| d.is_object())
            .or_else(|| l.get("window"));
        let Some(d) = d else { continue };
        let win = l.get("window").cloned().unwrap_or(Value::Null);
        let n = num(win.get("duration")).unwrap_or(0.0) as i64;
        let unit = text(win.get("timeUnit"));
        let span = if unit.contains("MINUTE") {
            n * 60
        } else if unit.contains("HOUR") {
            n * 3600
        } else if unit.contains("DAY") {
            n * 86400
        } else if unit.contains("WEEK") {
            n * 7 * 86400
        } else if unit.contains("MONTH") {
            n * 30 * 86400
        } else {
            0
        };
        if let Some(w) = window(d, &span_name(span), span) {
            out.push(w);
        }
    }
    if let Some(u) = r.get("usage").filter(|u| u.is_object()) {
        if let Some(w) = window(u, "7 days", 7 * 86400) {
            out.push(w);
        }
    }
    let usages = r.get("usages");
    let mut shares: Vec<Win> = Vec::new();
    for (key, name, span) in [
        ("limit_5h", "5 hours", 5 * 3600),
        ("limit_7d", "7 days", 7 * 86400),
        ("limit_month_total", "Month", 30 * 86400),
        ("limit_month_code", "Month", 30 * 86400),
    ] {
        let d = usages.and_then(|u| u.get(key).or_else(|| u.get(camel(key))));
        let Some(d) = d.filter(|d| d.is_object()) else {
            continue;
        };
        let ratio = num(d.get("used_ratio")).or_else(|| num(d.get("usedRatio")));
        let Some(ratio) = ratio.filter(|r| r.is_finite()) else {
            continue;
        };
        if shares.iter().any(|s| s.span_secs == span) {
            continue;
        }
        let mut w = Win::new(name, (ratio * 100.0).clamp(0.0, 100.0));
        w.span_secs = span;
        w.resets_at = reset_of(d);
        shares.push(w);
    }
    if !shares.is_empty() {
        out.retain(|w| !shares.iter().any(|s| s.span_secs == w.span_secs));
        out.extend(shares);
        out.sort_by_key(|w| {
            if w.span_secs == 0 {
                i64::MAX
            } else {
                w.span_secs
            }
        });
    }
    if out.is_empty() {
        return None;
    }
    // 加油包（kimiBooster）：balance.type=BOOSTER 且 amount>0；amountLeft 是
    // 分×10^6，缺省 = 没有了。
    let wallet = r
        .get("booster_wallet")
        .filter(|w| w.as_object().is_some_and(|o| !o.is_empty()))
        .or_else(|| r.get("boosterWallet"));
    if let Some(wallet) = wallet {
        let bal = wallet.get("balance").filter(|b| b.is_object());
        if let Some(bal) = bal.filter(|b| {
            text(b.get("type")) == "BOOSTER" && num(b.get("amount")).is_some_and(|a| a > 0.0)
        }) {
            let left = num(bal.get("amountLeft"))
                .filter(|l| *l > 0.0)
                .map(|l| (l / 1e6).round() / 100.0)
                .unwrap_or(0.0);
            let mut unit = "USD".to_string();
            for k in ["monthlyChargeLimit", "monthlyUsed"] {
                let c = text(wallet.get(k).and_then(|m| m.get("currency")));
                if !c.is_empty() {
                    unit = c.to_string();
                    break;
                }
            }
            if left > 0.0 {
                for w in out.iter_mut() {
                    w.aside = false;
                }
            }
            let mut w = Win::new("Extra usage", 0.0);
            w.balance = Some(left);
            w.unit = unit;
            w.aside = true;
            out.push(w);
        }
    }
    Some(out)
}

/// Go miniMaxCount：usage_count 有时是「已用」有时是「剩余」，取与剩余百分比
/// 吻合（1 点内）的那种解读；都不吻合 / total=0 = 没有计数。
fn minimax_count(count: Option<f64>, total: Option<f64>, left: Option<f64>) -> Option<(f64, f64)> {
    let (c, t, l) = (count?, total?, left?);
    if t <= 0.0 || c < 0.0 || c > t {
        return None;
    }
    let as_used = ((t - c) / t * 100.0 - l).abs();
    let as_left = (c / t * 100.0 - l).abs();
    if as_used.min(as_left) > 1.0 {
        return None;
    }
    if as_used < as_left {
        Some((c, t))
    } else {
        Some((t - c, t))
    }
}

/// Go readMiniMaxPlan（/v1/token_plan/remains）。
fn read_minimax(body: &str) -> Option<Vec<Win>> {
    let r: Value = serde_json::from_str(body).ok()?;
    let base = r.get("base_resp").filter(|b| b.is_object())?;
    if base.get("status_code").and_then(Value::as_i64).unwrap_or(0) != 0 {
        return None;
    }
    let at = |n: i64| {
        if n < 1_000_000_000_000 {
            from_secs(n)
        } else {
            from_millis(n)
        }
    };
    let int = |v: Option<&Value>| num(v).unwrap_or(0.0) as i64;
    let zero = |v: Option<f64>| v == Some(0.0);
    let mut out = Vec::new();
    for k in r
        .get("model_remains")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = text(k.get("model_name")).trim().to_string();
        let (status, week_status) = (
            int(k.get("current_interval_status")),
            int(k.get("current_weekly_status")),
        );
        let (total, week_total) = (
            num(k.get("current_interval_total_count")),
            num(k.get("current_weekly_total_count")),
        );
        if name.is_empty() || (status == 3 && week_status == 3 && zero(total) && zero(week_total)) {
            continue;
        }
        let general = name.eq_ignore_ascii_case("general");
        let buckets = [
            (
                num(k.get("current_interval_remaining_percent")),
                status,
                int(k.get("start_time")),
                int(k.get("end_time")),
                false,
                num(k.get("current_interval_usage_count")),
                total,
            ),
            (
                num(k.get("current_weekly_remaining_percent")),
                week_status,
                int(k.get("weekly_start_time")),
                int(k.get("weekly_end_time")),
                true,
                num(k.get("current_weekly_usage_count")),
                week_total,
            ),
        ];
        for (left, st, start, end, week, count, tot) in buckets {
            if st == 3 || (left.is_none() && st != 2) {
                continue;
            }
            let mut w = Win::new("", 100.0);
            if st != 2 {
                w.used = (100.0 - left.unwrap_or(0.0)).clamp(0.0, 100.0);
            }
            let mut span = 0i64;
            if start > 0 && end > start {
                if let (Some(a), Some(b)) = (at(start), at(end)) {
                    span = (b - a).num_seconds();
                }
            } else if week {
                span = 7 * 86400;
            }
            w.span_secs = span;
            if end > 0 {
                w.resets_at = at(end);
            }
            w.name = if week {
                "7 days".to_string()
            } else if span > 86400 && span % 86400 == 0 {
                format!("{} days", span / 86400)
            } else if span >= 3600 && span % 3600 == 0 {
                format!("{} hours", span / 3600)
            } else if span > 0 {
                format!("{} minutes", span / 60)
            } else {
                "Allowance".to_string()
            };
            if let Some((used, of)) = minimax_count(count, tot, left) {
                w.amount = Some(if st == 2 { of } else { used });
                w.limit = Some(of);
                if name.eq_ignore_ascii_case("video") {
                    w.unit = "videos".into();
                }
            }
            if !general {
                let mut chars = name.chars();
                let head: String = chars
                    .next()
                    .map(|c| c.to_uppercase().collect())
                    .unwrap_or_default();
                w.name = format!("{head}{} · {}", chars.as_str(), w.name);
                w.aside = true;
            }
            out.push(w);
        }
    }
    Some(out)
}

/// Go readOllamaPlan：含额（USD，按周期）+ 老套餐的 session/weekly 限额。
fn read_ollama_plan(body: &str) -> Option<Vec<Win>> {
    let r: Value = serde_json::from_str(body).ok()?;
    let included = r.get("included").filter(|i| i.is_object());
    let Some(included) = included else {
        return Some(Vec::new());
    };
    let bought = num(r.pointer("/purchased/balance_usd")).is_some_and(|v| v > 0.0);
    let mut out = Vec::new();
    for (key, name, span) in [
        ("session", "Session limit", 0i64),
        ("weekly", "Weekly", 7 * 86400),
    ] {
        let Some(l) = included.get(key).filter(|l| l.is_object()) else {
            continue;
        };
        let Some(left) = num(l.get("remaining_percent")) else {
            continue;
        };
        let mut w = Win::new(name, (100.0 - left).clamp(0.0, 100.0));
        w.span_secs = span;
        w.resets_at = parse_rfc3339(text(l.get("resets_at")));
        w.aside = bought;
        out.push(w);
    }
    let (Some(allowance), Some(left)) = (
        num(included.get("allowance_usd")),
        num(included.get("balance_usd")),
    ) else {
        return Some(out);
    };
    if allowance <= 0.0 {
        return Some(out);
    }
    let used = allowance.min((allowance - left).max(0.0));
    let mut w = Win::new("Allowance", (used / allowance * 100.0).clamp(0.0, 100.0));
    w.amount = Some(used);
    w.limit = Some(allowance);
    w.unit = "USD".into();
    if let Some(period) = included.get("period").filter(|p| p.is_object()) {
        let until = parse_rfc3339(text(period.get("until")));
        w.resets_at = until;
        if let (Some(from), Some(until)) = (parse_rfc3339(text(period.get("from"))), until) {
            if until > from {
                w.span_secs = (until - from).num_seconds();
                let days = w.span_secs as f64 / 86400.0;
                if (27.0..=32.0).contains(&days) {
                    w.name = "Month".into();
                }
            }
        }
    }
    w.aside = bought;
    out.push(w);
    Some(out)
}

// ---- Command Code -----------------------------------------------------------

/// Go cmdPlans：按 id 前缀（长的在前），每月额度与是否「用量价值单位」。
const CMD_PLANS: &[(&str, &str, f64, bool)] = &[
    ("individual-provider", "Provider", 0.0, false),
    ("individual-pro-v1", "Pro", 80.0, true),
    ("individual-goat", "GOAT", 70.0, true),
    ("individual-ultra", "Ultra", 300.0, false),
    ("individual-max", "Max", 150.0, false),
    ("individual-pro", "Pro", 30.0, true),
    ("individual-go", "Go", 10.0, true),
    ("teams-pro", "Teams Pro", 40.0, false),
];

fn cmd_plan_of(id: &str) -> Option<(&'static str, f64, bool)> {
    let id = id.to_ascii_lowercase().replace('_', "-");
    CMD_PLANS
        .iter()
        .find(|(prefix, ..)| id.starts_with(prefix))
        .map(|(_, n, m, u)| (*n, *m, *u))
}

fn cmd_time(v: Option<&Value>) -> Option<DateTime<Utc>> {
    if let Some(s) = v.and_then(Value::as_str) {
        if let Some(t) = parse_rfc3339(s) {
            return Some(t);
        }
    }
    let n = num(v).filter(|n| *n > 0.0)?;
    from_millis(if n < 1e12 {
        (n * 1000.0) as i64
    } else {
        n as i64
    })
}

fn cmd_windows(c: &Value) -> Vec<Win> {
    let mut out = Vec::new();
    let Some(limits) = c.get("windowLimits").filter(|w| w.is_object()) else {
        return out;
    };
    for (key, name, span) in [
        ("fiveHour", "5 hours", 5 * 3600i64),
        ("weekly", "Weekly", 7 * 86400),
    ] {
        let Some(w) = limits.get(key).filter(|w| w.is_object()) else {
            continue;
        };
        let (Some(used), Some(cap)) = (num(w.get("used")), num(w.get("cap"))) else {
            continue;
        };
        if cap <= 0.0 {
            continue;
        }
        let mut win = Win::new(name, (100.0 * used.max(0.0) / cap).min(100.0));
        win.span_secs = span;
        win.resets_at = cmd_time(w.get("resetAt"));
        out.push(win);
    }
    out
}

/// Go cmdLeft：本月额度 + 买的 + 送的（月额度单独返回）。
pub(super) fn cmd_left(c: &Value) -> Option<(f64, f64)> {
    let credits = c.get("credits")?;
    let mut monthly = 0.0;
    let mut left = 0.0;
    let mut known = false;
    for (i, k) in ["monthlyCredits", "purchasedCredits", "freeCredits"]
        .iter()
        .enumerate()
    {
        if let Some(n) = num(credits.get(*k)) {
            let n = n.max(0.0);
            if i == 0 {
                monthly = n;
            }
            left += n;
            known = true;
        }
    }
    known.then_some((monthly, left))
}

fn cmd_subscription(key: &str) -> (String, Option<DateTime<Utc>>) {
    let url = format!("{COMMAND_CODE_API}/alpha/billing/subscriptions");
    let Some(body) = plan_body(&url, key, true) else {
        return (String::new(), None);
    };
    let Ok(r) = serde_json::from_str::<Value>(&body) else {
        return (String::new(), None);
    };
    if r.get("success").and_then(Value::as_bool) == Some(false) {
        return (String::new(), None);
    }
    let Some(d) = r.get("data").filter(|d| d.is_object()) else {
        return (String::new(), None);
    };
    let plan_id = text(d.get("planId"));
    let status = text(d.get("status"));
    if !plan_id.is_empty() && status != "canceled" && status != "incomplete_expired" {
        return (plan_id.to_string(), cmd_time(d.get("currentPeriodEnd")));
    }
    (String::new(), None)
}

/// Go cmdKeyWindows + cmdQuotaOf：5 小时 / 每周窗口 + 月度额度一格。
fn read_command_code(key: &str) -> Option<Read> {
    let url = format!("{COMMAND_CODE_API}/alpha/billing/credits");
    let body = plan_body(&url, key, true)?;
    let c: Value = serde_json::from_str(&body).ok()?;
    let windows = cmd_windows(&c);
    let mut plan_id = text(c.pointer("/credits/planId")).to_string();
    if windows.is_empty() {
        let name = cmd_plan_of(&plan_id).map(|p| p.0).unwrap_or("");
        return Some(Read::new(name, Vec::new())); // 按量钥匙：没有套餐、没有卡
    }
    // 已过去的期末说明不了额度何时回来（Go 同款：丢掉）
    let mut until = None;
    if plan_id.is_empty() {
        let (id, end) = cmd_subscription(key);
        plan_id = id;
        until = end.filter(|t| *t > Utc::now());
    }
    let (name, plan_monthly, units) = cmd_plan_of(&plan_id).unwrap_or(("", 0.0, false));
    let mut out = windows;
    if let Some((monthly, left)) = cmd_left(&c) {
        if plan_monthly > 0.0 && units {
            let pool = plan_monthly.max(monthly);
            let mut display = format!(
                "{} / {} credits",
                money("", pool - monthly),
                money("", pool)
            );
            let extra = left - monthly;
            if extra > 0.0 {
                display.push_str(&format!(" + {} extra", money("$", extra)));
            }
            let mut w = Win::new("Credits", (100.0 * (pool - monthly) / pool).min(100.0));
            w.display = display;
            w.resets_at = until;
            out.push(w);
        } else if plan_monthly > 0.0 {
            let pool = plan_monthly.max(monthly) + left - monthly;
            let mut w = Win::new("Credits", (100.0 * (pool - left) / pool).min(100.0));
            w.display = format!("{} / {}", money("$", pool - left), money("$", pool));
            w.resets_at = until;
            out.push(w);
        }
    }
    Some(Read::new(name, out))
}

// ---- Mistral -----------------------------------------------------------------

fn mistral_budget_window(body: &str, kind: &str, payg: Option<bool>) -> Vec<Win> {
    let Ok(r) = serde_json::from_str::<Value>(body) else {
        return Vec::new();
    };
    let (detail, name) = match kind {
        "api" => (r.get("api_budget"), "API · Month"),
        "chat" => (r.get("vibe_budget"), "Month"),
        _ => (None, "Month"),
    };
    let Some(v) = detail.filter(|v| v.is_object()) else {
        return Vec::new();
    };
    let initial = num(v.get("initial_budget")).unwrap_or(0.0);
    if initial <= 0.0 {
        return Vec::new();
    }
    let used = num(v.get("usage_percentage")).unwrap_or(0.0).max(0.0);
    let mut w = Win::new(name, used.min(100.0));
    w.limit = Some(initial);
    w.amount = Some(initial * used / 100.0);
    w.unit = text(v.get("currency")).trim().to_ascii_uppercase();
    if let Some(t) = parse_rfc3339(text(v.get("reset_at"))) {
        w.resets_at = Some(t);
    }
    let payg = v.get("payg_enabled").and_then(Value::as_bool).or(payg);
    w.aside = payg == Some(true);
    vec![w]
}

fn read_mistral(key: &str) -> Option<Read> {
    let who = plan_body(MISTRAL_WHOAMI_URL, key, true)?;
    let who: Value = serde_json::from_str(&who).ok()?;
    let kind = who.get("plan_type")?.as_str()?.trim().to_ascii_lowercase();
    if kind != "chat" && kind != "api" {
        return Some(Read::new("", Vec::new()));
    }
    let budget = plan_body(MISTRAL_BUDGET_URL, key, true)?;
    let mut windows = mistral_budget_window(&budget, &kind, None);
    if windows.is_empty() {
        return Some(Read::new("", windows));
    }
    let mut plan = String::new();
    if let Some(sub) = plan_body(MISTRAL_SUBSCRIPTION_URL, key, true)
        .and_then(|b| serde_json::from_str::<Value>(&b).ok())
    {
        let payg = if kind == "api" {
            sub.get("payg_enabled")
        } else {
            sub.get("vibe_payg_enabled")
        };
        windows = mistral_budget_window(&budget, &kind, payg.and_then(Value::as_bool));
        let raw = text(sub.get("plan_name")).trim();
        plan = match raw.to_ascii_uppercase().as_str() {
            "FREE" => "Free".to_string(),
            "INDIVIDUAL" => "Pro".to_string(),
            "EDU" => "Student".to_string(),
            "TEAM" => "Team".to_string(),
            _ => raw.to_string(),
        };
    }
    Some(Read::new(&plan, windows))
}

// ---------------------------------------------------------------------------
// 入口
// ---------------------------------------------------------------------------

fn read_plan(kind: &Kind, p: &Provider, key: &str) -> Option<Read> {
    match kind {
        Kind::Zhipu { host, .. } => read_zhipu_plan(p, host, key),
        Kind::OpenCodeGo => {
            let ws = read_opencode_go(&plan_body(
                "https://opencode.ai/zen/go/v1/usage",
                key,
                true,
            )?)?;
            Some(Read::new("", ws))
        }
        Kind::Kimi { url } => Some(Read::new("", read_kimi(&plan_body(url, key, true)?)?)),
        Kind::CommandCode => read_command_code(key),
        Kind::MiniMax { host } => {
            let url = format!("https://{host}/v1/token_plan/remains");
            Some(Read::new("", read_minimax(&plan_body(&url, key, true)?)?))
        }
        Kind::Ollama => Some(Read::new(
            "",
            read_ollama_plan(&plan_body(OLLAMA_BALANCE_URL, key, true)?)?,
        )),
        Kind::Mistral => read_mistral(key),
    }
}

fn row_of(p: &Provider, user: &str, read: &Read) -> Value {
    let mut row = json!({
        "provider": p.id,
        "name": p.name,
        "windows": windows_json(&read.windows),
    });
    if !user.is_empty() {
        row["user"] = json!(user);
    }
    if !read.plan.is_empty() {
        row["plan"] = json!(read.plan);
    }
    if let Some(until) = read.until {
        row["until"] = json!(rfc3339(until));
        if !read.renew.is_empty() {
            row["renew"] = json!(read.renew);
        }
    }
    row
}

/// 每个有钥匙的套餐来源一行（多钥匙各一行，带用户名）；读不到 / 没有窗口 =
/// 不出行（Go：「有钥匙没套餐」不出卡；失败安静跳过）。各来源并发读，互不拖累。
pub fn rows(providers: &[Provider]) -> Vec<Value> {
    struct Job<'a> {
        provider: &'a Provider,
        kind: Kind,
        key: String,
        user: String,
    }
    let mut jobs: Vec<Job> = Vec::new();
    for p in providers {
        if p.key.is_empty() || skipped(p) {
            continue;
        }
        let Some(kind) = plan_source_of(p) else {
            continue;
        };
        for (key, user) in keys_of(p) {
            jobs.push(Job {
                provider: p,
                kind: kind.clone(),
                key,
                user,
            });
        }
    }
    let mut results: Vec<Option<Value>> = Vec::new();
    std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .iter()
            .map(|job| {
                scope.spawn(move || {
                    let read = read_plan(&job.kind, job.provider, &job.key)?;
                    if read.windows.is_empty() {
                        return None;
                    }
                    Some(row_of(job.provider, &job.user, &read))
                })
            })
            .collect();
        for handle in handles {
            results.push(handle.join().ok().flatten());
        }
    });
    results.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::super::quota_kit::testkit::{redirect, Mock, Redirects};
    use super::*;

    fn provider(json: &str) -> Provider {
        serde_json::from_str(json).expect("provider json")
    }

    #[test]
    fn source_table_follows_go_host_rules() {
        let p = provider(
            r#"{"id":"zhipu","name":"Zhipu","key":"k","chat":"https://open.bigmodel.cn/api/coding/paas/v4"}"#,
        );
        assert_eq!(
            plan_source_of(&p),
            Some(Kind::Zhipu {
                host: "open.bigmodel.cn".into(),
                coding: true
            })
        );
        let p = provider(
            r#"{"id":"og","name":"og","key":"k","chat":"https://opencode.ai/inference/go/v1"}"#,
        );
        assert_eq!(plan_source_of(&p), Some(Kind::OpenCodeGo));
        // zen（按量）与 custom 不是 Go
        let p = provider(r#"{"id":"z","name":"z","key":"k","chat":"https://opencode.ai/zen/v1"}"#);
        assert_eq!(plan_source_of(&p), None);
        let p = provider(
            r#"{"id":"c","name":"c","key":"k","chat":"https://opencode.ai/inference/custom/v1"}"#,
        );
        assert_eq!(plan_source_of(&p), None);
        let p =
            provider(r#"{"id":"k","name":"k","key":"k","chat":"https://api.kimi.com/coding/v1"}"#);
        assert_eq!(
            plan_source_of(&p),
            Some(Kind::Kimi {
                url: "https://api.kimi.com/coding/v1/usages".into()
            })
        );
        let p = provider(
            r#"{"id":"k","name":"k","key":"k","anthropic":"https://api.kimi.com/coding"}"#,
        );
        assert_eq!(
            plan_source_of(&p),
            Some(Kind::Kimi {
                url: "https://api.kimi.com/coding/v1/usages".into()
            })
        );
        // Kimi 非 coding 基址不是套餐
        let p = provider(r#"{"id":"k","name":"k","key":"k","chat":"https://api.kimi.com/v1"}"#);
        assert_eq!(plan_source_of(&p), None);
        let p = provider(r#"{"id":"o","name":"o","key":"k","chat":"https://ollama.com/v1"}"#);
        assert_eq!(plan_source_of(&p), Some(Kind::Ollama));
        let p = provider(r#"{"id":"d","name":"d","key":"k","chat":"https://api.deepseek.com/v1"}"#);
        assert_eq!(plan_source_of(&p), None);
    }

    #[test]
    fn zhipu_limits_read_like_go() {
        let body = r#"{"success":true,"data":{"level":"pro","limits":[
            {"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":12,"nextResetTime":1758800000000},
            {"type":"TOKENS_LIMIT","unit":6,"number":1,"percentage":40,"nextResetTime":1759400000000},
            {"type":"TIME_LIMIT","unit":5,"number":1,"percentage":3,"nextResetTime":1759500000000},
            {"type":"TOKENS_LIMIT","unit":3,"percentage":7}]}}"#;
        let (level, ws) = read_zhipu(body).expect("read");
        assert_eq!(level, "pro");
        let names: Vec<&str> = ws.iter().map(|w| w.name.as_str()).collect();
        assert_eq!(names, vec!["5 hours", "7 days", "MCP · Month", "5 hours"]);
        assert_eq!(ws[0].used, 12.0);
        assert!(ws[0].resets_at.is_some());
        assert!(ws[2].aside);
        // 拒答信封：钥匙被拒与别的失败分开
        assert_eq!(
            read_zhipu(r#"{"success":false,"code":1001,"msg":"x"}"#).err(),
            Some(ReadErr::Refused)
        );
        assert_eq!(
            read_zhipu(r#"{"success":false,"code":1302,"msg":"x"}"#).err(),
            Some(ReadErr::Other)
        );
        assert_eq!(
            read_zhipu(r#"{"success":true}"#).err(),
            Some(ReadErr::Other)
        );
    }

    #[test]
    fn zhipu_term_prefers_next_renew_then_valid_span() {
        let subs: Value = serde_json::from_str(
            r#"[{"status":"EXPIRED","nextRenewTime":"2020-01-01 00:00:00"},
                {"status":"VALID","autoRenew":true,"nextRenewTime":"2026-10-18 12:00:00"}]"#,
        )
        .unwrap();
        let (t, renew) = zhipu_term_of(&subs).expect("term");
        assert_eq!(renew, "auto");
        assert_eq!(
            rfc3339(t),
            "2026-10-18T04:00:00Z",
            "北京时间 12:00 = UTC 04:00"
        );
        let subs: Value = serde_json::from_str(
            r#"[{"status":"VALID","autoRenew":0,"valid":"2026-09-18 12:00:00-2026-10-18 12:00:00"}]"#,
        )
        .unwrap();
        let (t, renew) = zhipu_term_of(&subs).expect("term");
        assert_eq!(renew, "off");
        assert_eq!(rfc3339(t), "2026-10-18T04:00:00Z");
        let subs: Value = serde_json::from_str(r#"[{"status":"VALID","autoRenew":true}]"#).unwrap();
        assert!(zhipu_term_of(&subs).is_none());
    }

    #[test]
    fn opencode_go_zero_percent_has_no_reset() {
        let body = r#"{"usage":{"rolling":{"status":"ok","percent":0,"resetsAt":"2026-08-26T14:12:03.000Z"},
            "weekly":{"percent":28,"resetsAt":"2026-10-26T00:00:00Z"},
            "monthly":{"status":"rate-limited","percent":94,"resetsAt":"2026-11-09T15:23:05Z"}}}"#;
        let ws = read_opencode_go(body).expect("read");
        assert_eq!(ws.len(), 3);
        assert_eq!(ws[0].name, "5 hours");
        assert!(ws[0].resets_at.is_none(), "0% 的窗口 resetsAt 是假的，丢掉");
        assert_eq!(ws[1].used, 28.0);
        assert!(ws[2].resets_at.is_some());
        assert!(read_opencode_go(r#"{"usage":{}}"#).is_none());
    }

    #[test]
    fn ollama_plan_reads_allowance_and_legacy_limits() {
        let body = r#"{"included":{"balance_usd":24.61189,"allowance_usd":60,
            "period":{"from":"2026-10-22T11:53:37Z","until":"2026-11-22T11:53:37Z"}},"purchased":{"balance_usd":0}}"#;
        let ws = read_ollama_plan(body).expect("read");
        assert_eq!(ws.len(), 1);
        let w = ws[0].to_json();
        assert_eq!(w["name"], "Month");
        assert_eq!(w["unit"], "USD");
        assert_eq!(w["limit"], 60.0);
        assert!((w["used"].as_f64().unwrap() - 58.98018333).abs() < 1e-3);
        assert_eq!(w["resetsAt"], "2026-11-22T11:53:37Z");
        let legacy = r#"{"included":{"session":{"remaining_percent":75,"resets_at":"2026-10-01T07:00:00Z"},
            "weekly":{"remaining_percent":40,"resets_at":"2026-10-05T00:00:00Z"}},"purchased":{"balance_usd":25}}"#;
        let ws = read_ollama_plan(legacy).expect("read");
        assert_eq!(
            ws.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(),
            vec!["Session limit", "Weekly"]
        );
        assert_eq!(ws[0].used, 25.0);
        assert!(ws[0].aside, "买过额外 credits：含额用完也不挡");
        // 没有 included → 没有窗口
        assert!(read_ollama_plan(r#"{"purchased":{"balance_usd":3}}"#)
            .expect("read")
            .is_empty());
    }

    #[test]
    fn kimi_reads_windows_shares_and_booster() {
        let body = r#"{"usage":{"limit":"100","used":"12","resetTime":"2026-09-30T05:24:18.44Z"},
            "limits":[{"window":{"duration":300,"timeUnit":"TIME_UNIT_MINUTE"},
                       "detail":{"limit":"100","remaining":"88","resetTime":"2026-09-29T10:00:00Z"}}],
            "booster_wallet":{"balance":{"type":"BOOSTER","amount":"500000000","amountLeft":"250000000"},
                              "monthlyUsed":{"currency":"CNY"}}}"#;
        let ws = read_kimi(body).expect("read");
        assert_eq!(ws[0].name, "5 hours");
        assert_eq!(ws[0].used, 12.0);
        assert_eq!(ws[1].name, "7 days");
        let extra = ws.last().unwrap();
        assert_eq!(extra.name, "Extra usage");
        assert_eq!(extra.balance, Some(2.5));
        assert_eq!(extra.unit, "CNY");
        // 份额（月度计划）压过同长度的计数窗口
        let shares = r#"{"usages":{"limit_5h":{"used_ratio":0.4372,"reset_time":"2026-09-29T10:00:00Z"},
            "limit_month_total":{"used_ratio":0.1}}}"#;
        let ws = read_kimi(shares).expect("read");
        assert_eq!(ws.len(), 2);
        assert!((ws[0].used - 43.72).abs() < 1e-9);
        assert_eq!(ws[1].name, "Month");
        assert!(read_kimi("{}").is_none());
    }

    #[test]
    fn minimax_reads_buckets_and_skips_absent_ones() {
        let body = r#"{"model_remains":[
            {"model_name":"general","start_time":1790000000000,"end_time":1790018000000,
             "current_interval_remaining_percent":80,"current_interval_status":1,"current_interval_total_count":0,
             "weekly_start_time":1789500000000,"weekly_end_time":1790104800000,
             "current_weekly_remaining_percent":90,"current_weekly_status":1,"current_weekly_total_count":0},
            {"model_name":"image","current_interval_status":3,"current_weekly_status":3,
             "current_interval_total_count":0,"current_weekly_total_count":0}],
            "base_resp":{"status_code":0,"status_msg":"success"}}"#;
        let ws = read_minimax(body).expect("read");
        assert_eq!(ws.len(), 2, "没开通的桶不出");
        assert_eq!(ws[0].name, "5 hours");
        assert_eq!(ws[0].used, 20.0);
        assert_eq!(ws[1].name, "7 days");
        assert!(read_minimax(
            r#"{"model_remains":[],"base_resp":{"status_code":1004,"status_msg":"bad"}}"#
        )
        .is_none());
        assert!(read_minimax(r#"{"model_remains":[]}"#).is_none());
    }

    #[test]
    fn command_code_plan_window_and_credits() {
        let _ = (); // 纯解析：窗口与 cmd_left
        let c: Value = serde_json::from_str(
            r#"{"credits":{"planId":"individual-goat-monthly","monthlyCredits":41.2,"purchasedCredits":5,"freeCredits":0},
                "windowLimits":{"limited":true,"fiveHour":{"used":3.1,"cap":10,"resetAt":1790000000000},
                                "weekly":{"used":12,"cap":40,"resetAt":1790400000}}}"#,
        )
        .unwrap();
        let ws = cmd_windows(&c);
        assert_eq!(ws.len(), 2);
        assert!((ws[0].used - 31.0).abs() < 1e-9);
        assert!(ws[1].resets_at.is_some(), "秒也认");
        let (monthly, left) = cmd_left(&c).unwrap();
        assert_eq!(monthly, 41.2);
        assert!((left - 46.2).abs() < 1e-9);
        assert_eq!(cmd_plan_of("individual-pro-v1-x").unwrap().0, "Pro");
        assert_eq!(cmd_plan_of("individual-pro").unwrap().1, 30.0);
        assert!(cmd_plan_of("nothing").is_none());
    }

    #[test]
    fn mistral_budget_picks_window_by_key_kind() {
        let body = r#"{"vibe_budget":{"usage_percentage":42.5,"initial_budget":300,"currency":"eur","reset_at":"2026-11-01T00:00:00Z"},
                       "api_budget":{"usage_percentage":3,"initial_budget":50,"currency":"EUR"}}"#;
        let vibe = mistral_budget_window(body, "chat", None);
        assert_eq!(vibe[0].name, "Month");
        assert_eq!(vibe[0].unit, "EUR");
        assert!((vibe[0].amount.unwrap() - 127.5).abs() < 1e-9);
        let api = mistral_budget_window(body, "api", Some(true));
        assert_eq!(api[0].name, "API · Month");
        assert!(api[0].aside);
        assert!(mistral_budget_window(body, "mistral_code", None).is_empty());
    }

    #[test]
    fn rows_end_to_end_with_mock_vendors() {
        let _lock = super::super::TEST_SANDBOX_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mock = Mock::start();
        mock.route(
            "/api/monitor/usage/quota/limit",
            200,
            r#"{"success":true,"data":{"level":"pro","limits":[{"type":"TOKENS_LIMIT","unit":3,"number":5,"percentage":12,"nextResetTime":1758800000000}]}}"#,
        );
        mock.route(
            "/api/biz/subscription/list",
            200,
            r#"{"code":200,"msg":"ok","data":[{"status":"VALID","autoRenew":true,"nextRenewTime":"2026-10-18 12:00:00"}]}"#,
        );
        mock.route(
            "/zen/go/v1/usage",
            200,
            r#"{"usage":{"rolling":{"percent":3,"resetsAt":"2026-10-23T14:38:17Z"}}}"#,
        );
        mock.route(
            "/api/balance",
            200,
            r#"{"included":{"balance_usd":10,"allowance_usd":40,"period":{"from":"2026-10-22T00:00:00Z","until":"2026-11-22T00:00:00Z"}}}"#,
        );
        let _r = Redirects::new(&[
            redirect("https://open.bigmodel.cn", &mock.base),
            redirect("https://opencode.ai", &mock.base),
            redirect("https://ollama.com", &mock.base),
        ]);
        let providers: Vec<Provider> = vec![
            provider(
                r#"{"id":"zhipu","name":"Zhipu GLM","key":"sk-zp","chat":"https://open.bigmodel.cn/api/coding/paas/v4"}"#,
            ),
            provider(
                r#"{"id":"source-og","name":"opencode.ai","key":"sk-og","chat":"https://opencode.ai/inference/go/v1"}"#,
            ),
            provider(
                r#"{"id":"source-ol","name":"ollama","key":"sk-ol","chat":"https://ollama.com/v1"}"#,
            ),
            provider(r#"{"id":"nokey","name":"x","chat":"https://ollama.com/v1"}"#),
            provider(
                r#"{"id":"hid","name":"x","key":"k","hidden":true,"chat":"https://ollama.com/v1"}"#,
            ),
            provider(
                r#"{"id":"plain","name":"plain","key":"k","chat":"https://api.example.test/v1"}"#,
            ),
        ];
        let rows = rows(&providers);
        let ids: Vec<&str> = rows
            .iter()
            .map(|r| r["provider"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            vec!["zhipu", "source-og", "source-ol"],
            "无钥匙/隐藏/无套餐来源不出行"
        );
        assert_eq!(rows[0]["plan"], "pro");
        assert_eq!(rows[0]["renew"], "auto");
        assert_eq!(rows[0]["windows"][0]["name"], "5 hours");
        assert_eq!(rows[0]["windows"][0]["used"], 12.0);
        assert!(rows[0].get("user").is_none());
        assert_eq!(rows[1]["windows"][0]["used"], 3.0);
        assert_eq!(rows[2]["windows"][0]["unit"], "USD");
        // 认证形态：智谱裸钥匙、OpenCode 带会话头、其余 Bearer
        let requests = mock.requests();
        let zhipu = requests
            .iter()
            .find(|r| r.path.starts_with("/api/monitor/usage/quota/limit"))
            .unwrap();
        assert_eq!(zhipu.header("authorization"), "sk-zp");
        let og = requests
            .iter()
            .find(|r| r.path.starts_with("/zen/go/v1/usage"))
            .unwrap();
        assert_eq!(og.header("authorization"), "Bearer sk-og");
        assert!(og.header("x-opencode-session").starts_with("ses_"));
        assert_eq!(
            og.header("x-opencode-session"),
            og.header("x-opencode-session-id")
        );
        let ol = requests
            .iter()
            .find(|r| r.path.starts_with("/api/balance"))
            .unwrap();
        assert_eq!(ol.header("authorization"), "Bearer sk-ol");
    }

    #[test]
    fn failures_are_quiet_and_zhipu_team_is_tried() {
        let _lock = super::super::TEST_SANDBOX_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mock = Mock::start();
        // 个人套餐读不出（空 limits）→ 团队套餐 type=2 带 org/project
        mock.route("/api/monitor/usage/quota/limit?type=2", 200,
            r#"{"success":true,"data":{"limits":[{"type":"TOKENS_LIMIT","unit":3,"percentage":9}]}}"#);
        mock.route(
            "/api/monitor/usage/quota/limit",
            200,
            r#"{"success":true,"data":{"level":"","limits":[]}}"#,
        );
        let _r = Redirects::new(&[
            redirect("https://api.z.ai", &mock.base),
            redirect("https://open.bigmodel.cn", &mock.base),
            redirect("https://opencode.ai", "http://127.0.0.1:9"),
        ]);
        let providers = vec![
            provider(
                r#"{"id":"team","name":"Team","key":"k","chat":"https://api.z.ai/api/coding/paas/v4","zhipuTeam":{"org":"org-1","project":"proj-1"}}"#,
            ),
            provider(
                r#"{"id":"down","name":"down","key":"k","chat":"https://opencode.ai/inference/go/v1"}"#,
            ),
        ];
        let rows = rows(&providers);
        assert_eq!(rows.len(), 1, "出站失败的来源安静跳过");
        assert_eq!(rows[0]["plan"], "GLM Coding Team");
        assert_eq!(
            rows[0]["windows"][0]["name"], "5 hours",
            "团队五小时不带 number → 5"
        );
        let requests = mock.requests();
        let team = requests.iter().find(|r| r.path.contains("type=2")).unwrap();
        assert_eq!(team.header("bigmodel-organization"), "org-1");
        assert_eq!(team.header("bigmodel-project"), "proj-1");
    }

    #[test]
    fn multiple_keys_get_user_names_on_their_own_rows() {
        let _lock = super::super::TEST_SANDBOX_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mock = Mock::start();
        mock.route(
            "/zen/go/v1/usage",
            200,
            r#"{"usage":{"weekly":{"percent":10}}}"#,
        );
        let _r = Redirects::new(&[redirect("https://opencode.ai", &mock.base)]);
        let providers = vec![provider(
            r#"{"id":"og","name":"OG","key":"sk-first-key-1234","chat":"https://opencode.ai/zen/go/v1",
                "keys":[{"name":"work","key":"sk-second"},{"key":"sk-off","off":true},{"key":"sk-first-key-1234"}]}"#,
        )];
        let rows = rows(&providers);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["user"], "sk-f…1234");
        assert_eq!(rows[1]["user"], "work");
    }
}
