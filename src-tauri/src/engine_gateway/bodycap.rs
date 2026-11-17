//! 字节上限自适配（合同 C5+C7；body_cap.go + marionette_body_cap.go 的 Rust 对应）。
//!
//! 厂商拒收超限体时（明说超限；代理拿 502/5xx 网页顶替；或 ollama.com 那句
//! 不读体的 "failed to read request body"——2026-11-12 会话死锁事故）：学到 cap
//! （厂商说了数就用它的，否则按本次的 3/4），把**最老**的图换成占位文本
//! 重发，直到压到 cap 的 3/4 以下（给对话留增长余量，别每轮都破坏缓存）；
//! 最新一条用户消息的图保留。无图可丢 → 413（带尺寸语义，codex 能读懂
//! 方向而不是当传输错误原样重发）。后续请求先按已学 cap 预剪裁。
//!
//! 词表判定与 Go 逐条对齐（不引 regex crate，用 contains/starts_with 组合
//! 手写等价匹配），三条路：
//! - [`byte_cap_refusal`]：bodyCapWords 精确正则的字符串等价。**tooLong
//!   （「prompt too long / context length」类 = 窗口满）绝不算是字节上限**——
//!   那是 Go 版 tooLong() 的职责（走压缩提示，不走丢图重发），连 413 都会被
//!   它排除；401/403/429（鉴权/限流）同样不算。
//! - [`page_over_size`]：代理页判定——空体或 HTML 起头的体 + 413/500/502/
//!   503/504/520-524 + 本次体 ≥ 4 MiB（Go pageCapLeast；小请求的页是代理
//!   自己的毛病，维持原样）。代理页猜出的 cap 不进长期学习表（Go 同款）。
//! - [`unread_body_over_cap`]：marionette 补丁语义——400 + "failed to read
//!   request body" 且体 ≥ 8 MiB 才算（小请求上这句话更可能是连接被掐）。
use serde_json::Value;

use super::httpd::Reply;

/// ollama 那句拒答只在大体上才可信（marionette 补丁同款 8MiB 门槛）。
const UNREAD_FLOOR: usize = 8 << 20;
/// 代理页判定的大体门槛（Go pageCapLeast = 4 MiB）。
const PAGE_CAP_LEAST: usize = 4 << 20;

/// 有界扫描里的「标点」集（Go `[^，。,.;；!！]` 同款）。
const SCAN_PUNCT: &str = "，。,.;；!！";

// ---------------------------------------------------------------------------
// 判定主体
// ---------------------------------------------------------------------------

/// 合成判定（Go refitOverCap 两个决策点 `byteCapRefusal || pageOverSize`，
/// marionette 补丁把 unread-body 第三路并进前者）。
fn is_cap_refusal(status: u16, err_body: &[u8], sent: usize) -> bool {
    byte_cap_refusal(status, err_body, sent) || page_over_size(status, err_body, sent)
}

/// 词表路判定（Go byteCapRefusal + marionette unread-body 补丁）。
fn byte_cap_refusal(status: u16, err_body: &[u8], sent: usize) -> bool {
    // marionette 补丁：ollama.com 的 400 "failed to read request body"
    // （它根本没读体，既不说上限也不说窗口）。只在大体上算。
    if unread_body_over_cap(status, err_body, sent) {
        return true;
    }
    if status < 400 || status == 401 || status == 403 || status == 429 {
        return false;
    }
    let words = error_words(err_body);
    // 窗口满（tooLong）不是字节上限：绝不能算，否则会去丢图而不是提示压缩。
    if too_long(status, &words) {
        return false;
    }
    status == 413 || body_cap_words_match(&words)
}

/// marionette 补丁（marionette_body_cap.go）：400 + 那句不读体的话 + 体 ≥ 8MiB。
fn unread_body_over_cap(status: u16, err_body: &[u8], sent: usize) -> bool {
    const NEEDLE: &[u8] = b"failed to read request body";
    status == 400 && sent >= UNREAD_FLOOR && err_body.windows(NEEDLE.len()).any(|w| w == NEEDLE)
}

/// Go pageOverSize：代理（nginx/CDN/中转）拿自己的页顶替 API 应答——
/// 413/500/502/503/504/520-524 或空体，来应一个 ≥ 4 MiB 的大请求
/// （chenxiang0010 中转 502 九连发的死循环回归）。小请求的页是代理自己的
/// 毛病，维持原样透传。
fn page_over_size(status: u16, err_body: &[u8], sent: usize) -> bool {
    if sent < PAGE_CAP_LEAST {
        return false;
    }
    let page_status = matches!(status, 413 | 500 | 502 | 503 | 504) || (520..=524).contains(&status);
    if !page_status {
        return false;
    }
    unsaid(err_body)
}

/// Go provider.Unsaid：错误体里没有厂商自己的话——空体，或一张网页
/// （pageStart：`^<(!doctype\s+html|html[\s>]|head[\s>]|…)`）。
fn unsaid(body: &[u8]) -> bool {
    let t = String::from_utf8_lossy(body);
    let t = t.trim();
    if t.is_empty() {
        return true;
    }
    let t = t.trim_start_matches('\u{feff}');
    let lower = t.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("<!doctype") {
        if rest.starts_with(|c: char| c.is_whitespace()) && rest.trim_start().starts_with("html") {
            return true;
        }
    }
    for tag in ["<html", "<head", "<body", "<title", "<meta"] {
        if let Some(rest) = lower.strip_prefix(tag) {
            if rest.starts_with(|c: char| c.is_whitespace()) || rest.starts_with('>') {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// 错误体取词（Go wordsOf / catalog.Words）
// ---------------------------------------------------------------------------

/// Go wordsOf：JSON 体把非 ASCII 转义成 `\uXXXX`（Python json.dumps 默认）时，
/// 解码出的字符串逐行追加在原文之后再统一小写（#1669：转义后的中文限流/
/// 窗口词也要读得到）。三张词表全是 `(?i)`，这里一次小写后全用小写词表。
fn error_words(body: &[u8]) -> String {
    let text = String::from_utf8_lossy(body).into_owned();
    if !text.contains("\\u") {
        return text.to_ascii_lowercase();
    }
    let Ok(value) = serde_json::from_slice::<Value>(body) else {
        return text.to_ascii_lowercase();
    };
    let mut out = text;
    fn walk(v: &Value, out: &mut String) {
        match v {
            Value::String(s) => {
                out.push('\n');
                out.push_str(s);
            }
            Value::Array(a) => {
                for x in a {
                    walk(x, out);
                }
            }
            Value::Object(o) => {
                for x in o.values() {
                    walk(x, out);
                }
            }
            _ => {}
        }
    }
    walk(&value, &mut out);
    out.to_ascii_lowercase()
}

// ---------------------------------------------------------------------------
// tooLong：窗口满的词（Go gateway.tooLong + tooLongRe + paceWords）
// ---------------------------------------------------------------------------

/// Go tooLong：拒答说的是「会话超了模型窗口」而不是「请求体超了字节上限」。
/// max_tokens 族（回复上限，压缩也没用）与限速词先排除。
fn too_long(status: u16, m: &str) -> bool {
    // Go 的守卫：只看 4xx（5xx 不进 tooLong——那是代理/网关的事）；429 限流。
    if status < 400 || status >= 500 || status == 429 {
        return false;
    }
    if m.contains("max_tokens")
        || m.contains("max_output_tokens")
        || m.contains("max_completion_tokens")
    {
        return false;
    }
    if pace_words_match(m) {
        return false;
    }
    too_long_words(m)
}

/// Go paceWords：限速/限量词（不是一条提示词的大小）。
fn pace_words_match(m: &str) -> bool {
    const CN: [&str; 12] = [
        "速率", "频繁", "频率", "限流", "并发", "每秒", "每分钟", "每小时", "每天", "每日", "稍后再试", "稍后重试",
    ];
    if CN.iter().any(|w| m.contains(w)) {
        return true;
    }
    if m.contains("per minute")
        || m.contains("per hour")
        || m.contains("per day")
        || m.contains("please wait")
        || m.contains("try again later")
        || m.contains("throttl")
        || m.contains("ratelimit")
    {
        return true;
    }
    // rate.?limit：rate 与 limit 之间隔任意一个字符（rate-limit / rate limit / rate_limit）
    let mut from = 0;
    while let Some(pos) = m[from..].find("rate") {
        let at = from + pos + 4;
        if m[at..].starts_with("limit") {
            return true;
        }
        if let Some(ch) = m[at..].chars().next() {
            if m[at + ch.len_utf8()..].starts_with("limit") {
                return true;
            }
        }
        from = from + pos + 1;
    }
    // \bTP[MD]\b
    word_bounded(m, "tpm") || word_bounded(m, "tpd")
}

/// Go tooLongRe 的手写等价：英文模式全量 + 中文常用（`[^标点]{0,N}` 类用
/// [`bounded_chain`] 有界扫描）。
fn too_long_words(m: &str) -> bool {
    // —— 简单 contains 词 ——
    const PLAIN: [&str; 17] = [
        "context_length_exceeded",
        "prompt is too long",
        "input is too long",
        "maximum context length",
        "reduce the length of the messages",
        "maximum allowed input length",
        "available context size",
        "greater than the context length",
        "context window exceeds limit",
        "exceeded model token limit",
        "configured context size",
        "prompt too long",
        "prompt exceeds max length",
        "context_window_exceeded",
        "range of input length should be",
        "token limit exceeded",
        "上下文过长",
    ];
    if PLAIN.iter().any(|w| m.contains(w)) {
        return true;
    }
    // too many (input |prompt )?tokens
    for mid in ["", "input ", "prompt "] {
        if m.contains(&format!("too many {mid}tokens")) {
            return true;
        }
    }
    // 上下文(长度)?(超|过长)
    for len in ["", "长度"] {
        for t in ["超", "过长"] {
            if m.contains(&format!("上下文{len}{t}")) {
                return true;
            }
        }
    }
    // 超(过|出)(了)?(模型)?(的)?(最大)?上下文
    for v in ["过", "出"] {
        for l in ["", "了"] {
            for mo in ["", "模型"] {
                for d in ["", "的"] {
                    for mx in ["", "最大"] {
                        if m.contains(&format!("超{v}{l}{mo}{d}{mx}上下文")) {
                            return true;
                        }
                    }
                }
            }
        }
    }
    // (内容|输入|文本|提示词?|prompt|问题|对话|消息)(内容|文本)?(长度)?\s*(太长|过长|超长)
    for s in ["内容", "输入", "文本", "提示词", "提示", "prompt", "问题", "对话", "消息"] {
        for mid in ["", "内容", "文本"] {
            for l in ["", "长度"] {
                for ws in ["", " ", "\t"] {
                    for t in ["太长", "过长", "超长"] {
                        if m.contains(&format!("{s}{mid}{l}{ws}{t}")) {
                            return true;
                        }
                    }
                }
            }
        }
    }
    // 字数(太多|过多)
    if m.contains("字数太多") || m.contains("字数过多") {
        return true;
    }
    // context (length|limit|window) (exceeded|is exceeded)
    for a in ["length", "limit", "window"] {
        for b in ["exceeded", "is exceeded"] {
            if m.contains(&format!("context {a} {b}")) {
                return true;
            }
        }
    }
    // context (window )?(is )?full\b
    for w in ["", "window "] {
        for i in ["", "is "] {
            if word_ends_at_boundary(m, &format!("context {w}{i}full")) {
                return true;
            }
        }
    }
    // exceeded (max |maximum )?context length
    for pre in ["", "max ", "maximum "] {
        if m.contains(&format!("exceeded {pre}context length")) {
            return true;
        }
    }
    // longer than the model'?s? context length
    for t in ["model", "model'", "models", "model's"] {
        if m.contains(&format!("longer than the {t} context length")) {
            return true;
        }
    }
    // (exceeds?|exceeded|over|beyond)( the)?( model'?s?)?( maximum)? context
    for v in ["exceeds", "exceed", "exceeded", "over", "beyond"] {
        for the in ["", " the"] {
            for mo in ["", " model", " model'", " models", " model's"] {
                for mx in ["", " maximum"] {
                    if m.contains(&format!("{v}{the}{mo}{mx} context")) {
                        return true;
                    }
                }
            }
        }
    }
    // input token count.*exceeds the maximum
    if let Some(pos) = m.find("input token count") {
        if m[pos..].contains("exceeds the maximum") {
            return true;
        }
    }
    // maximum prompt length is \d
    if prefixed_by_digit(m, "maximum prompt length is ") {
        return true;
    }
    // prompt token count of [\d,]+ exceeds the limit
    if digits_commas_then(m, "prompt token count of ", " exceeds the limit") {
        return true;
    }
    // input tokens? (count )?(exceeds?|exceeded) (the )?(max(imum)? )?limit
    for tok in ["input token", "input tokens"] {
        for c in ["", "count "] {
            for v in ["exceeds", "exceed", "exceeded"] {
                for t in ["", "the "] {
                    for mx in ["", "max ", "maximum "] {
                        if m.contains(&format!("{tok} {c}{v} {t}{mx}limit")) {
                            return true;
                        }
                    }
                }
            }
        }
    }
    // (内容|输入|文本|提示词?|prompt|问题|对话|消息)[^，。,.;；!！]{0,12}超(过|出)[^，。,.;；!！]{0,12}(长度|token|令牌|字数)
    for s in ["内容", "输入", "文本", "提示词", "提示", "prompt", "问题", "对话", "消息"] {
        for v in ["过", "出"] {
            for unit in ["长度", "token", "令牌", "字数"] {
                if bounded_chain(m, s, 12, &format!("超{v}"), 12, unit) {
                    return true;
                }
            }
        }
    }
    // (输入|提示词?|prompt)\s*的?\s*(长度|token|令牌|字数)[^，。,.;；!！]{0,8}超(过|出)
    for s in ["输入", "提示词", "提示", "prompt"] {
        for ws1 in ["", " "] {
            for d in ["", "的"] {
                for ws2 in ["", " "] {
                    for unit in ["长度", "token", "令牌", "字数"] {
                        for v in ["过", "出"] {
                            let head = format!("{s}{ws1}{d}{ws2}{unit}");
                            if bounded_chain(m, &head, 8, &format!("超{v}"), 0, "") {
                                return true;
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// bodyCapWords：超字节上限的措辞（Go bodyCapWords 正则的字符串等价）
// ---------------------------------------------------------------------------

/// Go bodyCapWords 手写等价，逐条对应正则的 10 个分支：
/// 1. `request[ _](body|entity|payload)( size)?( is| was)? (too large|too big|over|larger than|exceeds?|exceeded)`
/// 2. `request body is [\d.]+ ?[kmg]i?b`（Command Code 的 "request body is 40.1 MB"）
/// 3-5. `payload too large` / `entity too large` / `request_too_large`
/// 6. `(body|request|payload) (size )?(exceeds?|exceeded|is over|is larger than) (the )?(max|limit|\d)`
/// 7-9. `length limit exceeded` / `maximum allowed number of bytes` / `limit for one request`
/// 10. `(max(imum)?|request) (request |body )*size (limit )?(exceeded|of)`
fn body_cap_words_match(m: &str) -> bool {
    for w in [
        "payload too large",
        "entity too large",
        "request_too_large",
        "length limit exceeded",
        "maximum allowed number of bytes",
        "limit for one request",
    ] {
        if m.contains(w) {
            return true;
        }
    }
    // 分支 1：request[ _]<noun>( size)?( is| was)? <verb>
    const VERBS: [&str; 6] = ["too large", "too big", "over", "larger than", "exceeds", "exceed"];
    for sep in [' ', '_'] {
        for noun in ["body", "entity", "payload"] {
            let head = format!("request{sep}{noun}");
            let mut from = 0;
            while let Some(pos) = m[from..].find(&head) {
                let at = from + pos + head.len();
                // ( size)? 之后 ( is| was)?，两个可选独立组合
                let mut bases = vec![at];
                if m[at..].starts_with(" size") {
                    bases.push(at + 5);
                }
                for base in bases {
                    let mut cands = vec![base];
                    if m[base..].starts_with(" is") {
                        cands.push(base + 3);
                    }
                    if m[base..].starts_with(" was") {
                        cands.push(base + 4);
                    }
                    for cand in cands {
                        for verb in VERBS {
                            if m[cand..].starts_with(&format!(" {verb}")) {
                                return true;
                            }
                        }
                    }
                }
                from = at;
            }
        }
    }
    // 分支 2：request body is [\d.]+ ?[kmg]i?b
    let mut from = 0;
    while let Some(pos) = m[from..].find("request body is ") {
        let at = from + pos + "request body is ".len();
        let mut cursor = at;
        while m.as_bytes().get(cursor).is_some_and(|b| b.is_ascii_digit() || *b == b'.') {
            cursor += 1;
        }
        if cursor > at {
            let mut u = cursor;
            if m.as_bytes().get(u) == Some(&b' ') {
                u += 1;
            }
            for unit in ["kb", "kib", "mb", "mib", "gb", "gib"] {
                if m[u..].starts_with(unit) {
                    return true;
                }
            }
        }
        from = from + pos + 1;
    }
    // 分支 6：(body|request|payload) (size )?(exceeds?|exceeded|is over|is larger than) (the )?(max|limit|\d)
    for subj in ["body", "request", "payload"] {
        let mut from = 0;
        while let Some(pos) = m[from..].find(subj) {
            let at = from + pos + subj.len();
            let mut next = at;
            if m[next..].starts_with(' ') {
                let mut v = next + 1;
                if m[v..].starts_with("size ") {
                    v += 5;
                }
                for verb in ["exceeds", "exceed", "exceeded", "is over", "is larger than"] {
                    if !m[v..].starts_with(verb) {
                        continue;
                    }
                    let after_verb = v + verb.len();
                    if !m[after_verb..].starts_with(' ') {
                        continue;
                    }
                    let mut o = after_verb + 1;
                    if m[o..].starts_with("the ") {
                        o += 4;
                    }
                    if m[o..].starts_with("max")
                        || m[o..].starts_with("limit")
                        || m.as_bytes().get(o).is_some_and(|b| b.is_ascii_digit())
                    {
                        return true;
                    }
                }
                next = v;
            }
            from = next;
        }
    }
    // 分支 10：(max(imum)?|request) (request |body )*size (limit )?(exceeded|of)
    for head in ["maximum", "max", "request"] {
        let mut from = 0;
        while let Some(pos) = m[from..].find(head) {
            let at = from + pos + head.len();
            if m[at..].starts_with(' ') {
                let mut cursor = at + 1;
                loop {
                    // 在每个候选位置试 `size (limit )?(exceeded|of)`
                    if m[cursor..].starts_with("size") {
                        let s = cursor + 4;
                        if m[s..].starts_with(' ') {
                            let s = s + 1;
                            for t in ["exceeded", "of"] {
                                if m[s..].starts_with(t) {
                                    return true;
                                }
                            }
                            if m[s..].starts_with("limit ") {
                                let s2 = s + 6;
                                for t in ["exceeded", "of"] {
                                    if m[s2..].starts_with(t) {
                                        return true;
                                    }
                                }
                            }
                        }
                    }
                    // 再吞一个 "request " 或 "body "
                    if m[cursor..].starts_with("request ") {
                        cursor += 8;
                    } else if m[cursor..].starts_with("body ") {
                        cursor += 5;
                    } else {
                        break;
                    }
                }
            }
            from = at;
        }
    }
    false
}

// ---------------------------------------------------------------------------
// 小工具（词表匹配用）
// ---------------------------------------------------------------------------

/// Go `\bWORD\b`：词边界匹配（词 = [0-9a-z_]，`m` 已小写）。
fn word_bounded(m: &str, w: &str) -> bool {
    let mut from = 0;
    while let Some(pos) = m[from..].find(w) {
        let at = from + pos;
        let before = m[..at].chars().next_back().is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        let end = at + w.len();
        let after = m[end..].chars().next().is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        if before && after {
            return true;
        }
        from = at + 1;
    }
    false
}

/// `pat` 出现且**结尾**落在词边界上（Go `full\b` 类）。
fn word_ends_at_boundary(m: &str, pat: &str) -> bool {
    let mut from = 0;
    while let Some(pos) = m[from..].find(pat) {
        let end = from + pos + pat.len();
        let after = m[end..].chars().next().is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        if after {
            return true;
        }
        from = from + pos + 1;
    }
    false
}

/// `head` 之后紧跟一个 ASCII 数字（Go `is \d` 类）。
fn prefixed_by_digit(m: &str, head: &str) -> bool {
    let mut from = 0;
    while let Some(pos) = m[from..].find(head) {
        let at = from + pos + head.len();
        if m.as_bytes().get(at).is_some_and(|b| b.is_ascii_digit()) {
            return true;
        }
        from = from + pos + 1;
    }
    false
}

/// `head` 之后一段 [\d,]+ 再接 `tail`（Go `of [\d,]+ exceeds` 类）。
fn digits_commas_then(m: &str, head: &str, tail: &str) -> bool {
    let mut from = 0;
    while let Some(pos) = m[from..].find(head) {
        let at = from + pos + head.len();
        let mut cursor = at;
        while m.as_bytes().get(cursor).is_some_and(|b| b.is_ascii_digit() || *b == b',') {
            cursor += 1;
        }
        if cursor > at && m[cursor..].starts_with(tail) {
            return true;
        }
        from = at;
    }
    false
}

/// head 之后跳过至多 `skip1` 个非标点字符、`mid` 起头，mid 之后再跳至多
/// `skip2` 个非标点字符、`tail` 起头（Go `[^，。,.;；!！]{0,N}` 类的有界扫描
/// 等价；`tail` 为空只要求 mid 出现）。
fn bounded_chain(m: &str, head: &str, skip1: usize, mid: &str, skip2: usize, tail: &str) -> bool {
    let mut from = 0;
    while let Some(pos) = m[from..].find(head) {
        let head_end = from + pos + head.len();
        let mut cursor = head_end;
        let mut used = 0usize;
        loop {
            if m[cursor..].starts_with(mid)
                && (tail.is_empty() || after_skip_starts(m, cursor + mid.len(), skip2, tail))
            {
                return true;
            }
            if used >= skip1 {
                break;
            }
            match m[cursor..].chars().next() {
                Some(ch) if !SCAN_PUNCT.contains(ch) => {
                    cursor += ch.len_utf8();
                    used += 1;
                }
                _ => break,
            }
        }
        from = head_end;
    }
    false
}

/// 从 `at` 起跳过至多 `skip` 个非标点字符，看任一停点上是否 `then` 起头。
fn after_skip_starts(m: &str, at: usize, skip: usize, then: &str) -> bool {
    let mut cursor = at;
    let mut used = 0usize;
    loop {
        if m[cursor..].starts_with(then) {
            return true;
        }
        if used >= skip {
            return false;
        }
        match m[cursor..].chars().next() {
            Some(ch) if !SCAN_PUNCT.contains(ch) => {
                cursor += ch.len_utf8();
                used += 1;
            }
            _ => return false,
        }
    }
}

// ---------------------------------------------------------------------------
// statedCap：拒答里点名的 cap
// ---------------------------------------------------------------------------

/// Go statedCap：拒答里点名的尺寸取**最小且小于本次发送量**的那个（正文常
/// 把请求自身的大小也写在旁边），1 KiB 以下不算 cap，MB 按 2^20（"32 MB …
/// (33554432 bytes)" 即 33554432）。没点名返回 None。
fn stated_cap(err_body: &[u8], sent: usize) -> Option<usize> {
    let m = error_words(err_body);
    let bytes = m.as_bytes();
    let mut least: Option<usize> = None;
    let mut i = 0usize;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        // (\d+(?:\.\d+)?)
        let num_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if bytes.get(i) == Some(&b'.') && bytes.get(i + 1).is_some_and(|b| b.is_ascii_digit()) {
            i += 1;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
        }
        let Ok(num) = m[num_start..i].parse::<f64>() else {
            continue;
        };
        // \s*
        let mut u = i;
        while matches!(bytes.get(u), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            u += 1;
        }
        // (bytes|b|kb|kib|mb|mib|gb|gib)\b——长的先试，b 最后
        let mut matched_end: Option<(usize, f64)> = None;
        for (unit, mult) in [
            ("bytes", 1.0),
            ("kib", 1024.0),
            ("kb", 1024.0),
            ("mib", 1048576.0),
            ("mb", 1048576.0),
            ("gib", 1073741824.0),
            ("gb", 1073741824.0),
            ("b", 1.0),
        ] {
            if m[u..].starts_with(unit) {
                let after = u + unit.len();
                // \b：单位后是非词字符（数字/字母/下划线之外）或串尾
                let boundary = match bytes.get(after) {
                    None => true,
                    Some(&c) => !(c.is_ascii_alphanumeric() || c == b'_'),
                };
                if boundary {
                    matched_end = Some((after, mult));
                    break;
                }
            }
        }
        match matched_end {
            Some((after, mult)) => {
                i = after;
                let n = (num * mult) as usize;
                // 1 KiB 以下不是 cap；≥ 发送量的是正文自报的请求大小
                if n >= 1024 && n < sent && least.is_none_or(|l| n < l) {
                    least = Some(n);
                }
            }
            None => {
                // 数字后没接单位：从数字末尾继续扫
            }
        }
    }
    least
}

// ---------------------------------------------------------------------------
// 学 cap 与重发回路
// ---------------------------------------------------------------------------

/// Go capRoom：目标压到 cap 的 3/4（给对话留增长余量，别每轮都破坏缓存）。
fn cap_room(n: usize) -> usize {
    n / 4 * 3
}

/// cap 学习表（Gateway.caps 的类型；单收此表让测试不必构造 Store/碰盘）。
type Caps = std::sync::Mutex<std::collections::HashMap<String, usize>>;

/// Go bodyCaps.learn：只收更小的 cap（厂商后来才说真话时别把学紧的放宽），
/// 返回生效值。
fn learn(caps: &Caps, provider_id: &str, n: usize) -> usize {
    match caps.lock() {
        Ok(mut caps) => {
            if let Some(old) = caps.get(provider_id) {
                if *old > 0 && *old <= n {
                    return *old;
                }
            }
            caps.insert(provider_id.to_string(), n);
            n
        }
        Err(_) => n,
    }
}

/// 代理页猜出的 cap 不进长期表（Go：a page is a guess, not a vendor's word）；
/// 已知更小的 cap 优先。
fn page_cap(caps: &Caps, provider_id: &str, guessed: usize) -> usize {
    if let Ok(caps) = caps.lock() {
        if let Some(k) = caps.get(provider_id) {
            if *k > 0 && *k < guessed {
                return *k;
            }
        }
    }
    guessed
}

/// 序列化后的字节数。
fn size_of(value: &Value) -> usize {
    serde_json::to_vec(value).map(|b| b.len()).unwrap_or(usize::MAX)
}

/// 把最老的图换成占位文本直到压到 `target` 以下；返回 (新体, 丢了几张)。
/// 最新一条用户消息的图不动；没有可丢的图/压不进去返回 None。
fn without_oldest_images(body: &[u8], target: usize) -> Option<(Vec<u8>, usize)> {
    let mut value = serde_json::from_slice::<Value>(body).ok()?;
    let count = value
        .get("messages")?
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    // 最新一条用户消息的下标（它的图保留——magpie 同款规则）。
    let last_user = value
        .get("messages")
        .and_then(|m| m.as_array())
        .and_then(|a| a.iter().rposition(|m| m.get("role").and_then(|r| r.as_str()) == Some("user")));

    let mut shed = 0usize;
    loop {
        if size_of(&value) <= target {
            break;
        }
        // 每轮从最老的消息重新扫起，丢**一张**图（天然最老优先）。
        let mut replaced = false;
        'messages: for index in 0..count {
            if Some(index) == last_user {
                continue;
            }
            let Some(parts) = value
                .get_mut("messages")
                .and_then(|m| m.as_array_mut())
                .and_then(|a| a.get_mut(index))
                .and_then(|m| m.get_mut("content"))
                .and_then(|c| c.as_array_mut())
            else {
                continue;
            };
            for part in parts.iter_mut() {
                if part.get("type").and_then(|t| t.as_str()) == Some("image_url") {
                    *part = Value::String(
                        "[image left out: the request was over the provider's byte cap]".into(),
                    );
                    shed += 1;
                    replaced = true;
                    break 'messages;
                }
            }
        }
        if !replaced {
            return None;
        }
    }
    if shed == 0 {
        return None;
    }
    serde_json::to_vec(&value).ok().map(|b| (b, shed))
}

fn over_cap_reply(sent: usize, cap: usize, shed_attempted: bool) -> Reply {
    let mb = |n: usize| format!("{:.1} MB", n as f64 / 1_048_576.0);
    Reply::error(
        413,
        &format!(
            "the request is {}, over the {} it takes in one request{}",
            mb(sent),
            mb(cap),
            if shed_attempted {
                "; leaving out its earlier images didn't bring it under"
            } else {
                "; it holds no image to leave out"
            }
        ),
    )
}

/// chat.rs 在 4xx 应答后调用：命中 cap 拒答 → 学 cap、丢最老图重发（至多
/// 3 轮）；不是 cap 拒答返回 None（原样透传）。
pub fn maybe_refit(
    gw: &super::Gateway,
    out: &super::chat::Outbound,
    sent: &[u8],
    status: u16,
    err_body: &[u8],
) -> Option<Reply> {
    let sent_len = sent.len();
    let page = page_over_size(status, err_body, sent_len);
    if !is_cap_refusal(status, err_body, sent_len) {
        return None;
    }
    let mut cap = stated_cap(err_body, sent_len)
        .unwrap_or_else(|| cap_room(sent_len))
        .max(1);
    if page {
        cap = page_cap(&gw.caps, &out.provider_id, cap);
    } else {
        cap = learn(&gw.caps, &out.provider_id, cap);
    }
    let mut current = sent.to_vec();
    for _ in 0..3 {
        let target = cap_room(cap);
        let Some((fitted, shed)) = without_oldest_images(&current, target) else {
            return Some(over_cap_reply(sent_len, cap, true));
        };
        if fitted.len() >= current.len() {
            return Some(over_cap_reply(sent_len, cap, true));
        }
        match super::chat::send(out, &fitted) {
            Err(e) => return Some(Reply::error(502, &format!("{}: {}", out.provider_id, e))),
            Ok(response) => {
                let restatus = response.status();
                let mut reader = response.into_reader();
                let mut vendor_body = Vec::new();
                use std::io::Read;
                let _ = reader.read_to_end(&mut vendor_body);
                if restatus >= 400 && is_cap_refusal(restatus, &vendor_body, fitted.len()) {
                    let again_page = page_over_size(restatus, &vendor_body, fitted.len());
                    cap = stated_cap(&vendor_body, fitted.len())
                        .unwrap_or_else(|| cap_room(fitted.len()))
                        .max(1);
                    if again_page {
                        cap = page_cap(&gw.caps, &out.provider_id, cap);
                    } else {
                        cap = learn(&gw.caps, &out.provider_id, cap);
                    }
                    current = fitted;
                    let _ = shed;
                    continue;
                }
                return Some(Reply::Json { status: restatus, body: vendor_body });
            }
        }
    }
    Some(over_cap_reply(sent_len, cap, true))
}

/// 出站前的预剪裁：cap 已知且体逼得够近才剪（Go capNear = 90%——75–90%
/// 区间的预剪裁是白丢图破缓存），目标压到 cap 的 3/4。返回 None = 不需要剪。
pub fn prefit(gw: &super::Gateway, provider_id: &str, body: &[u8]) -> Option<Vec<u8>> {
    let cap = *gw.caps.lock().ok()?.get(provider_id)?;
    if body.len() <= cap / 10 * 9 {
        return None;
    }
    without_oldest_images(body, cap_room(cap)).map(|(fitted, _)| fitted)
}

// ---------------------------------------------------------------------------
// 测试（任务 T10 验收：词表对齐的关键样本）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Command Code 的原话（Discord EDwinpzy，截图大户 codex 会话）。
    const COMMAND_CODE: &str =
        "Request body is 40.1 MB, over the 32 MB limit for one request (33554432 bytes)";

    #[test]
    fn command_code_sentence_is_cap_refusal() {
        // Command Code 用自己的状态码发这句话（Go 注释：came as its own status）
        assert!(is_cap_refusal(400, COMMAND_CODE.as_bytes(), 42_000_000));
        assert!(is_cap_refusal(413, COMMAND_CODE.as_bytes(), 42_000_000));
    }

    #[test]
    fn ollama_unread_body_small_request_is_not_cap() {
        let body = b"ollama: failed to read request body (ref: 5f5b-8c1d)";
        assert!(!is_cap_refusal(400, body, 1024));
    }

    #[test]
    fn ollama_unread_body_large_request_is_cap() {
        let body = b"ollama: failed to read request body (ref: 5f5b-8c1d)";
        assert!(is_cap_refusal(400, body, 40 << 20));
        // 那句话只在 400 上算（marionette 补丁原文）
        assert!(!is_cap_refusal(500, body, 40 << 20));
    }

    #[test]
    fn window_full_is_never_cap_refusal() {
        // ollama 30 MB 时还说得出这句（体被读了）——窗口满，走压缩不走丢图
        let body = b"prompt too long; exceeded max context length";
        assert!(!is_cap_refusal(400, body, 40 << 20));
        // Go：tooLong 连 413 都排除（overflowed 也认 413）
        assert!(!is_cap_refusal(413, body, 40 << 20));
        // \uXXXX 转义后的窗口词同样要读得到（#1669）
        let escaped = br#"{"error":{"message":"prompt is too long; maximum context length"}}"#;
        assert!(!is_cap_refusal(400, escaped, 40 << 20));
    }

    #[test]
    fn plain_413_is_cap_refusal() {
        assert!(is_cap_refusal(413, b"limit for one request", 1));
        assert!(is_cap_refusal(413, b"", 1));
    }

    #[test]
    fn auth_and_rate_never_cap() {
        let big = 40 << 20;
        assert!(!is_cap_refusal(401, b"request body is too large", big));
        assert!(!is_cap_refusal(403, b"payload too large", big));
        assert!(!is_cap_refusal(429, b"limit for one request", big));
        // 限速词把窗口词也压下去（Bedrock 的 "Too many tokens, please wait"）
        assert!(!is_cap_refusal(400, b"too many tokens, please wait", big));
        // max_tokens 族是回复上限，不算窗口满也不算字节上限——原样透传
        assert!(!is_cap_refusal(400, b"maximum context length with max_tokens", big));
    }

    #[test]
    fn proxy_page_on_big_request_is_cap() {
        let page = b"<html>\r\n<head><title>502 Bad Gateway</title></head>\r\n<body>nginx</body></html>";
        // 大请求 + 代理页（chenxiang0010 中转回归：502 九连发）
        assert!(is_cap_refusal(502, page, 8 << 20));
        assert!(is_cap_refusal(504, b"", 8 << 20));
        assert!(is_cap_refusal(521, b"<!doctype html><html>error</html>", 8 << 20));
        // 小请求的页是代理自己的毛病，维持原样
        assert!(!is_cap_refusal(502, page, 1024));
        // 有话说的 5xx 不是页
        assert!(!is_cap_refusal(502, br#"{"error":"upstream connect error"}"#, 8 << 20));
        // 页判定的状态面外（501/505）不算
        assert!(!is_cap_refusal(501, b"", 8 << 20));
    }

    #[test]
    fn stated_cap_picks_smallest_named_size() {
        assert_eq!(stated_cap(COMMAND_CODE.as_bytes(), 42_100_000), Some(33554432));
        assert_eq!(stated_cap(b"no numbers here", 1000), None);
        // 比发送量还大的尺寸不算 cap（那是正文自报的请求大小）
        assert_eq!(stated_cap(b"maximum 40.1 MB allowed", 1000), None);
        // 1 KiB 以下不是 cap
        assert_eq!(stated_cap(b"over 500 bytes", 10_000), None);
        // MiB 按 2^20
        assert_eq!(stated_cap(b"limit is 32MiB please", 100_000_000), Some(32 << 20));
    }

    #[test]
    fn cap_words_cover_go_branches() {
        let big = 40 << 20;
        let samples: [(u16, &str); 12] = [
            (400, "request body is too large"),
            (400, "Request payload size was too big"),
            (400, "request_entity exceeds"),
            (400, "request body is 34 MB"),
            (400, "body exceeds the max"),
            (400, "request size exceeded limit"),
            (400, "payload is over 100"),
            (400, "maximum request body size of 10485760 bytes exceeded"),
            (400, "maximum allowed number of bytes"),
            (400, "length limit exceeded"),
            (400, "request_too_large"),
            (500, "request body is over the 32 MB limit for one request"),
        ];
        for (status, text) in samples {
            assert!(
                is_cap_refusal(status, text.as_bytes(), big),
                "should match: {status} {text}"
            );
        }
    }

    #[test]
    fn too_long_chinese_words_are_recognized() {
        // 中文窗口词（#4420/#1669 的中文家族）：不是字节上限
        for text in [
            "上下文长度超限",
            "输入内容太长",
            "请求超过最大上下文",
            "内容超过长度限制",
            "提示词的长度超过了最大值",
        ] {
            assert!(too_long(400, text), "should be too long: {text}");
            assert!(!is_cap_refusal(400, text.as_bytes(), 40 << 20), "not a byte cap: {text}");
        }
    }

    #[test]
    fn learn_keeps_smaller_cap() {
        // learn 只收更小——厂商后来才说真话时别把学紧的放宽（Go bodyCaps.learn）
        let caps: Caps = std::sync::Mutex::new(std::collections::HashMap::new());
        assert_eq!(learn(&caps, "p", 1000), 1000);
        assert_eq!(learn(&caps, "p", 2000), 1000);
        assert_eq!(learn(&caps, "p", 500), 500);
        assert_eq!(caps.lock().unwrap().get("p"), Some(&500));
        // 代理页的猜测不进表，只借用已知更小的值
        assert_eq!(page_cap(&caps, "p", 900), 500);
    }
}
