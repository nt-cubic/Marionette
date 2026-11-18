//! 请求体预处理（`base64_input.go` + `body_text.go` 的 1:1 对应）。
//!
//! 两件读体之后、任何路由/翻译/bodycap 之前对对话体做的事：
//!
//! - **坏文本换成 U+FFFD**（`textBody`，body_text.go:32；laincatx on X 的
//!   事故）：agent 发来的 JSON 里混进非 UTF-8 字节（GBK 源文件、被截断的
//!   日志）或孤代理转义（`\ud800` 无配对）时，各家厂商各有各的拒法
//!   （DeepSeek/OpenAI 400、MiniMax 空 200），路由组里一个请求成不成全看
//!   落到哪个成员。Go 在读体出口（request_bounds.go:284）把每个坏字节换成
//!   一个 U+FFFD、孤代理转义换成 `\ufffd` 字面转义——回合不为此被拒（agent
//!   的工具结果原样带文件字节；Anthropic 在这里 400 会让会话后续每一轮都
//!   重发同样的历史再失败一次）。只碰 JSON 体：multipart/音频不是文本；
//!   已合法的体原样零拷贝返回。
//!
//! - **内联 base64 统一成标准 padded 字母表**（`stdBase64Body`，
//!   base64_input.go:101；#5697）：Google 的 Python SDK 用
//!   `base64.urlsafe_b64encode` 写图片字节（`-`/`_`），Google 自家 API 两种
//!   都读，magpie 后面的厂商/中继可能只读标准的——dludream 的图经 Gemini
//!   来源回来 400 "Failed to base64-decode image data"，同一请求换成
//!   `+`/`/` 就通。Go 在 serve() 最前（gateway.go:2544）把客户端送的内联
//!   base64 写成标准 padded 形态，之后每条路由（直通、翻译、转发）读到的
//!   都已经是它。Gemini inlineData 分支不做（Rust 引擎无 Gemini 入站）。
//!
//! 接线在 routes.rs 的对话臂（含 count_tokens——Go countTokens 同样在计数
//! 前跑 stdBase64Body，gateway.go:2112）：httpd.rs 读体（含 Content-Encoding
//! 解码）之后、route() 分发之前正是 Go 的两个位置。字节级预检
//! [`may_hold_odd_base64`] 是性能闸门（Go 注释：2MB 请求 ~2ms），不过闸的
//! 体零拷贝原样返回。
use std::borrow::Cow;

use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, STANDARD};
use base64::engine::DecodePaddingMode;
use base64::Engine as _;
use serde_json::Value;

use super::effort::Proto;
use super::httpd::Request;

// ---------------------------------------------------------------------------
// base64_input.go
// ---------------------------------------------------------------------------

/// stdBase64（base64_input.go:33）：`s` 是 URL-safe 字母表（RFC 4648 §5）或
/// 未补 padding 的 base64 时，重写成标准 padded 形态（§4）；None = 已是标准
/// padded、或两种字母表都不是（原样发，厂商爱说什么说什么）。
pub(crate) fn std_base64(s: &str) -> Option<String> {
    if s.is_empty() {
        return None;
    }
    let url_safe = s.contains('-') || s.contains('_');
    if !url_safe && s.len() % 4 == 0 {
        return None;
    }
    if url_safe && (s.contains('+') || s.contains('/')) {
        return None; // 两种字母表混用：谁的也不是
    }
    // Go 先 TrimRight 掉全部尾随 '=' 再用 Raw 编码解码（中缝的 '=' 会失败）。
    let trimmed = s.trim_end_matches('=');
    let alphabet = if url_safe { &base64::alphabet::URL_SAFE } else { &base64::alphabet::STANDARD };
    // Go 的 Raw*Encoding 解码不查尾随位是否为零（"YR" -> "a"，实测），crate
    // 默认查——显式放开保持 1:1。
    let engine = GeneralPurpose::new(
        alphabet,
        GeneralPurposeConfig::new()
            .with_decode_padding_mode(DecodePaddingMode::RequireNone)
            .with_decode_allow_trailing_bits(true),
    );
    let raw = engine.decode(trimmed.as_bytes()).ok()?;
    Some(STANDARD.encode(raw))
}

/// mayHoldOddBase64（base64_input.go:65）：体里可能有 stdBase64 会重写的
/// base64 吗——从每个 `"` 往回扫一段 ≥64 个 base64 字符（两种字母表都算），
/// 段里含 URL-only 字符（`-`/`_`）或长度非 4 的倍数才算。标准 base64 的图
/// 每几十个字符就有一个 `+`/`/`（截断 URL-safe 段），散文有空格；判错的
/// 代价只是白走一遍。字节级扫描（Go 实测 2MB 请求 ~2ms）。
fn may_hold_odd_base64(body: &[u8]) -> bool {
    const LEAST: usize = 64;
    let mut i = 0usize;
    loop {
        let Some(q) = body[i..].iter().position(|&b| b == b'"') else {
            return false;
        };
        i += q;
        let (mut start, mut odd) = (i, false);
        while start > 0 {
            match base64_class(body[start - 1]) {
                0 => break,
                2 => odd = true, // 剩 1 的话原样
                _ => {}
            }
            start -= 1;
        }
        let run = i - start;
        if run >= LEAST && (odd || run % 4 != 0) {
            return true;
        }
        i += 1;
    }
}

/// base64Class（base64_input.go:90）：标准 base64 字节（含 `=`）= 1，
/// URL-only 的（`-`/`_`）= 2，其余 0。
fn base64_class(c: u8) -> u8 {
    match c {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' | b'=' => 1,
        b'-' | b'_' => 2,
        _ => 0,
    }
}

/// stdBase64Body（base64_input.go:101）：体里每张图的内联 base64 写成标准
/// padded 形态；没有可写的原样返回（零拷贝）。
///
/// Go 经 walkImages 按协议定位图块（chat `image_url` / responses
/// `input_image` 的 data URI 字符串；anthropic `source{type:"base64"}` 的裸
/// b64 `data` 字段）。这里解析成 [`Value`] 后递归走一遍：**所有** data URI
/// 形态的字符串值统一改写（对三种协议一律成立——bodycap.rs 的 Shape 只找
/// 「最新」的图，这里要全部），anthropic 的 source 对象单独改 `data` 字段
/// （它不是 data URI 字符串，Go withImageData 的 anthropic 分支同款，
/// heic.go:277）。预检不过或解析失败 → 原样返回。
pub(crate) fn std_base64_body(proto: Proto, body: &[u8]) -> Cow<'_, [u8]> {
    if !may_hold_odd_base64(body) {
        return Cow::Borrowed(body);
    }
    let mut v: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return Cow::Borrowed(body),
    };
    let mut changed = false;
    walk_values(&mut v, proto, &mut changed);
    if !changed {
        return Cow::Borrowed(body);
    }
    serde_json::to_vec(&v).map(Cow::Owned).unwrap_or(Cow::Borrowed(body))
}

/// 递归改写：data URI 字符串统一标准 base64；anthropic 的 base64 source
/// 改裸 `data` 字段（Go walkImages + withImageData 的合并形状）。
fn walk_values(v: &mut Value, proto: Proto, changed: &mut bool) {
    match v {
        Value::String(s) => {
            if let Some(std) = std_data_uri(s) {
                *s = std;
                *changed = true;
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_values(item, proto, changed);
            }
        }
        Value::Object(map) => {
            if proto == Proto::Anthropic {
                if let Some(source) = map.get_mut("source").and_then(|s| s.as_object_mut()) {
                    let is_base64 = source.get("type").and_then(|t| t.as_str()) == Some("base64");
                    if is_base64 && source.get("data").map(|d| d.is_string()).unwrap_or(false) {
                        // Go imageSrc 只认 type=="base64" 且 data 非空；
                        // media_type 原样带过（withImageData 写回同 mime）。
                        let data = source.get("data").and_then(|d| d.as_str()).unwrap_or("").to_string();
                        if let Some(std) = std_base64(&data) {
                            source.insert("data".into(), Value::String(std));
                            *changed = true;
                        }
                    }
                }
            }
            for (_, val) in map.iter_mut() {
                walk_values(val, proto, changed);
            }
        }
        _ => {}
    }
}

/// `data:<mime>;base64,<b64>` 的 b64 换成标准 padded 形态；其余 None
/// （Go stdBase64Body 对每张图 Src 的判定：`data:` 前缀 + 逗号 + head 以
/// `;base64` 结尾，mime 可为空）。
fn std_data_uri(s: &str) -> Option<String> {
    let rest = s.strip_prefix("data:")?;
    let (head, b64) = rest.split_once(',')?;
    let mime = head.strip_suffix(";base64")?;
    let std = std_base64(b64)?;
    Some(format!("data:{mime};base64,{std}"))
}

// ---------------------------------------------------------------------------
// body_text.go
// ---------------------------------------------------------------------------

/// textBody（body_text.go:32）：JSON 体里的坏文本换成 U+FFFD；非 JSON / 已
/// 合法 → 原样（零拷贝）。
pub(crate) fn text_body<'a>(content_type: Option<&str>, body: &'a [u8]) -> Cow<'a, [u8]> {
    if !json_body(content_type, body) || !needs_text(body) {
        return Cow::Borrowed(body);
    }
    Cow::Owned(fix_text(body))
}

/// jsonBody（body_text.go:41）：体是 JSON 文档吗——以 `{` 或 `[` 开头（前导
/// 空白不算），且 Content-Type 不属于装「另一种字节」的媒体类型。
fn json_body(content_type: Option<&str>, body: &[u8]) -> bool {
    if let Some(ct) = content_type.filter(|ct| !ct.is_empty()) {
        let mt = media_type(ct);
        if mt.starts_with("multipart/")
            || mt.starts_with("audio/")
            || mt.starts_with("image/")
            || mt.starts_with("video/")
            || mt == "application/octet-stream"
            || mt == "application/x-www-form-urlencoded"
        {
            return false;
        }
    }
    let skip = body
        .iter()
        .position(|&b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        .unwrap_or(body.len());
    let t = &body[skip..];
    !t.is_empty() && (t[0] == b'{' || t[0] == b'[')
}

/// mime.ParseMediaType 的够用近似：`;` 参数段之前为主类型，去空白小写；取
/// 出来是空的（畸形头）时退回整段小写（Go err 分支的同款兜底——两种结果
/// 对上面的前缀判定等价）。
fn media_type(ct: &str) -> String {
    let mt = ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    if mt.is_empty() {
        ct.trim().to_ascii_lowercase()
    } else {
        mt
    }
}

/// needsText（body_text.go:58）：体里有非 UTF-8 字节或孤代理转义吗。
fn needs_text(body: &[u8]) -> bool {
    if std::str::from_utf8(body).is_err() {
        return true;
    }
    let mut i = 0usize;
    while i < body.len() {
        let Some(j) = body[i..].iter().position(|&b| b == b'\\') else {
            return false;
        };
        i += j;
        if i + 1 >= body.len() {
            return false;
        }
        if body[i + 1] != b'u' {
            i += 2; // \\、\" 之类：下一字节不是转义的起点
            continue;
        }
        let r = hex4(&body[i..]);
        if (0xD800..=0xDBFF).contains(&r) {
            if !low_next(body, i + 6) {
                return true;
            }
            i += 12; // 一对（Python json.dumps 写 emoji 的形态）
        } else if (0xDC00..=0xDFFF).contains(&r) {
            return true;
        } else {
            i += 2;
        }
    }
    false
}

/// lowNext（body_text.go:91）：body[i..] 起是一个低代理的 `\uXXXX` 转义吗。
fn low_next(body: &[u8], i: usize) -> bool {
    if i > body.len() {
        return false;
    }
    let lo = hex4(&body[i..]);
    (0xDC00..=0xDFFF).contains(&lo)
}

/// hex4（Go fastjson.go:781，encoding/json 的 getu4）：`\uXXXX` 的值；形状
/// 不对或非 hex → -1。
fn hex4(s: &[u8]) -> i32 {
    if s.len() < 6 || s[0] != b'\\' || s[1] != b'u' {
        return -1;
    }
    let mut r: i32 = 0;
    for &c in &s[2..6] {
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => return -1,
        };
        r = r * 16 + d as i32;
    }
    r
}

/// fixText（body_text.go:101）：每个非 UTF-8 字节写成一个 UTF-8 编码的
/// U+FFFD（EF BF BD）、每个孤代理转义写成 `\ufffd` 六字符字面转义；合法
/// 转义、代理对、普通字节原样。
fn fix_text(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(body.len() + 16);
    let mut i = 0usize;
    while i < body.len() {
        let c = body[i];
        if c == b'\\' && i + 1 < body.len() {
            if body[i + 1] != b'u' {
                out.push(c);
                out.push(body[i + 1]);
                i += 2;
                continue;
            }
            let r = hex4(&body[i..]);
            if r < 0 {
                out.push(c);
                out.push(b'u');
                i += 2;
            } else if (0xD800..=0xDBFF).contains(&r) {
                if low_next(body, i + 6) {
                    out.extend_from_slice(&body[i..i + 12]);
                    i += 12;
                } else {
                    out.extend_from_slice(b"\\ufffd");
                    i += 6;
                }
            } else if (0xDC00..=0xDFFF).contains(&r) {
                out.extend_from_slice(b"\\ufffd");
                i += 6;
            } else {
                out.extend_from_slice(&body[i..i + 6]);
                i += 6;
            }
            continue;
        }
        if c < 0x80 {
            out.push(c);
            i += 1;
            continue;
        }
        let (ch, size) = decode_rune_at(body, i);
        if ch.is_none() {
            out.extend_from_slice("\u{FFFD}".as_bytes());
            i += 1;
            continue;
        }
        out.extend_from_slice(&body[i..i + size]);
        i += size;
    }
    out
}

/// Go `utf8.DecodeRune` 的最小对应：body[i..] 起一个 UTF-8 序列 →
/// (Some(ch), 字节数)；非法（尾随字节缺失/过长/代理区/越界）→ (None, 1)。
/// 输入里本来就编码着的 U+FFFD（EF BF BD）返回 (Some, 3)——与 Go
/// 「RuneError 且 size<=1 才算坏」同判。
fn decode_rune_at(body: &[u8], i: usize) -> (Option<char>, usize) {
    let b = body[i];
    if b < 0x80 {
        return (Some(b as char), 1);
    }
    let len = if b >= 0xF0 {
        4
    } else if b >= 0xE0 {
        3
    } else if b >= 0xC2 {
        2
    } else {
        // 0x80..=0xBF（无引导字节）、0xC0/0xC1（过长）、0xF5..（越界）都非法
        return (None, 1);
    };
    if i + len > body.len() {
        return (None, 1);
    }
    match std::str::from_utf8(&body[i..i + len]).ok().and_then(|s| s.chars().next()) {
        Some(ch) => (Some(ch), len),
        None => (None, 1),
    }
}

// ---------------------------------------------------------------------------
// 接线入口
// ---------------------------------------------------------------------------

/// 两步合并（顺序同 Go：textBody 在读体出口、stdBase64Body 在 serve() 最前，
/// 都先于任何路由/翻译/bodycap）。体没变时零拷贝借回。
pub fn prepare<'a>(proto: Proto, content_type: Option<&str>, body: &'a [u8]) -> Cow<'a, [u8]> {
    match text_body(content_type, body) {
        Cow::Borrowed(b) => std_base64_body(proto, b),
        Cow::Owned(fixed) => match std_base64_body(proto, &fixed) {
            Cow::Borrowed(_) => Cow::Owned(fixed),
            Cow::Owned(out) => Cow::Owned(out),
        },
    }
}

/// routes.rs 对话臂共用：[`prepare`] 之后带上预处理体的请求。体没变时借回
/// 原请求（零拷贝）；变了才重建（method/path/headers 浅克隆——对话体几十
/// MB，头几百字节，不值当为此改 httpd 的读体链）。
pub fn prepared_request<'a>(request: &'a Request, proto: Proto) -> Cow<'a, Request> {
    let prepared = prepare(proto, request.header("content-type"), &request.body);
    match prepared {
        Cow::Borrowed(_) => Cow::Borrowed(request),
        Cow::Owned(body) => Cow::Owned(Request {
            method: request.method.clone(),
            path: request.path.clone(),
            headers: request.headers.clone(),
            body,
        }),
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ----- stdBase64（base64_input.go:33）-----

    #[test]
    fn std_base64_rewrites_url_safe_alphabet() {
        // #5697 场景：google-genai 的 base64.urlsafe_b64encode（'-'/'_'）。
        // "----" 解码 fb ef be → 标准 "++++"；"-w" 解码 fb → "+w=="。
        assert_eq!(std_base64("----").as_deref(), Some("++++"));
        assert_eq!(std_base64("-w").as_deref(), Some("+w=="));
        assert_eq!(std_base64("a-b_").as_deref(), Some("a+b/"));
    }

    #[test]
    fn std_base64_pads_non_multiple_of_four() {
        // 已是标准字母表但没补 padding：重写成 padded 形态。
        assert_eq!(std_base64("abc").as_deref(), Some("abc="));
        assert_eq!(std_base64("+w").as_deref(), Some("+w=="));
        // 尾随 '=' 全剥掉再解（Go TrimRight 同款）
        assert_eq!(std_base64("abc==").as_deref(), Some("abc="));
    }

    #[test]
    fn std_base64_leaves_standard_padded_alone() {
        for s in ["abcd", "+w==", "aGVsbG8=", "PD/PZQ=="] {
            assert_eq!(std_base64(s), None, "{s}");
        }
        // 7 字符（非 4 倍长）即使字母表是标准的也要重写（补正 padding）——
        // Go 同款：早退只认「无 -_ 且 len%4==0」。
        assert_eq!(std_base64("PD/PZQ=").as_deref(), Some("PD/PZQ=="));
    }

    #[test]
    fn std_base64_rejects_mixed_alphabets_and_bad_input() {
        // 同时含 -_ 与 +/：两种字母表混用，谁的也不是
        assert_eq!(std_base64("-+ab"), None);
        assert_eq!(std_base64("_/ab"), None);
        // 解码失败：非法字符、中缝 '='
        assert_eq!(std_base64("ab-d!"), None);
        assert_eq!(std_base64("a=bc-"), None);
        // 空串
        assert_eq!(std_base64(""), None);
        // 非零尾随位照解（Go Raw*Encoding 实测 "YR" -> "a" -> "YQ=="，
        // crate 默认拒——见 std_base64 里的配置注释）
        assert_eq!(std_base64("YR").as_deref(), Some("YQ=="));
    }

    // ----- mayHoldOddBase64（base64_input.go:65）-----

    #[test]
    fn may_hold_finds_long_url_safe_run() {
        let run = "-_".repeat(32); // 64 个 URL-safe 字符
        let body = format!(r#"{{"a":"{run}"}}"#);
        assert!(may_hold_odd_base64(body.as_bytes()));
    }

    #[test]
    fn may_hold_passes_standard_multiple_of_four() {
        // 标准字母表、含 +/、长度是 4 的倍数（真·标准 base64 图片的样子）
        let run = "ab+/cd".repeat(11); // 66 → 取 64
        let run = &run[..64];
        assert_eq!(run.len() % 4, 0);
        let body = format!(r#"{{"a":"{run}"}}"#);
        assert!(!may_hold_odd_base64(body.as_bytes()));
    }

    #[test]
    fn may_hold_requires_sixty_four() {
        // 63 个 base64 字符（含 URL-only）：差一个不算
        let run = format!("{}-", "-_".repeat(31));
        assert_eq!(run.len(), 63);
        let body = format!(r#"{{"a":"{run}"}}"#);
        assert!(!may_hold_odd_base64(body.as_bytes()));
    }

    #[test]
    fn may_hold_passes_prose() {
        // 散文：空格截断 run
        let body = format!(r#"{{"a":"{}"}}"#, "word ".repeat(40));
        assert!(!may_hold_odd_base64(body.as_bytes()));
        // 非多倍数的标准 run（65 个）也算——长度非 4 的倍数
        let run = "a".repeat(65);
        let body = format!(r#"{{"a":"{run}"}}"#);
        assert!(may_hold_odd_base64(body.as_bytes()));
        // 没有引号的体
        assert!(!may_hold_odd_base64(b"plain bytes no quotes"));
    }

    // ----- stdBase64Body（base64_input.go:101）-----

    /// 64+ 个 URL-safe base64（每 6 bit 一组手搓，保证可解码）。
    fn url_safe_run() -> String {
        // "-_"*32 解码是 fbefbe fbefbe...（3 字节/4 字符），整段合法。
        "-_".repeat(32)
    }

    #[test]
    fn std_base64_body_rewrites_chat_data_uri() {
        let b64 = url_safe_run();
        let body = format!(
            r#"{{"model":"p/m","messages":[{{"role":"user","content":[{{"type":"image_url","image_url":{{"url":"data:image/png;base64,{b64}"}}}}]}}]}}"#
        );
        let out = std_base64_body(Proto::Chat, body.as_bytes());
        let v: Value = serde_json::from_slice(&out).expect("json");
        let url = v["messages"][0]["content"][0]["image_url"]["url"].as_str().unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
        let std = url.strip_prefix("data:image/png;base64,").unwrap();
        assert!(!std.contains('-') && !std.contains('_'), "{std}");
        // 解码回来是同一份字节
        let raw_std = STANDARD.decode(std).unwrap();
        let raw_url = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(b64.as_bytes()).unwrap();
        assert_eq!(raw_std, raw_url);
        // 其余字段不动
        assert_eq!(v["model"], "p/m");
    }

    #[test]
    fn std_base64_body_rewrites_anthropic_source_data() {
        let b64 = url_safe_run();
        let body = format!(
            r#"{{"model":"p/m","messages":[{{"role":"user","content":[{{"type":"image","source":{{"type":"base64","media_type":"image/png","data":"{b64}"}}}}]}}]}}"#
        );
        let out = std_base64_body(Proto::Anthropic, body.as_bytes());
        let v: Value = serde_json::from_slice(&out).expect("json");
        let src = &v["messages"][0]["content"][0]["source"];
        assert_eq!(src["type"], "base64");
        assert_eq!(src["media_type"], "image/png");
        let data = src["data"].as_str().unwrap();
        assert!(!data.contains('-') && !data.contains('_'), "{data}");
    }

    #[test]
    fn std_base64_body_zero_copy_when_prefilter_passes() {
        // 标准字母表、4 倍长 → 预检不过 → 原样借回（零拷贝）
        let run = "ab+/cd".repeat(11);
        let run = &run[..64];
        let body = format!(r#"{{"a":"{run}"}}"#);
        let out = std_base64_body(Proto::Chat, body.as_bytes());
        assert!(matches!(out, Cow::Borrowed(b) if b.as_ptr() == body.as_bytes().as_ptr()));
    }

    #[test]
    fn std_base64_body_bad_json_returned_as_is() {
        // 预检过了但不是 JSON：原样返回（不编造）
        let run = url_safe_run();
        let body = format!(r#"x = "{run}""#);
        let out = std_base64_body(Proto::Chat, body.as_bytes());
        assert_eq!(out.as_ref(), body.as_bytes());
    }

    #[test]
    fn std_base64_body_data_uri_anywhere_is_unified() {
        // 泛化（任务简报许可）：data URI 不在图块位置也一样统一——改写的只是
        // 字母表形态，解码字节不变，对三种协议一律成立。
        let b64 = url_safe_run();
        let body = format!(r#"{{"model":"p/m","note":"data:image/jpeg;base64,{b64}"}}"#);
        let out = std_base64_body(Proto::Chat, body.as_bytes());
        let v: Value = serde_json::from_slice(&out).expect("json");
        let note = v["note"].as_str().unwrap();
        assert!(note.starts_with("data:image/jpeg;base64,"), "{note}");
        assert!(note.contains('+') || note.contains('/'), "{note}");
    }

    // ----- textBody / jsonBody（body_text.go:32/41）-----

    #[test]
    fn text_body_mends_each_bad_byte_as_one_replacement() {
        // GBK「你好」的字节：每个坏字节一个 U+FFFD（EF BF BD）
        let mut body = br#"{"a":""#.to_vec();
        body.extend_from_slice(&[0xc4, 0xe3, 0xba, 0xc3]);
        body.push(b'"');
        body.push(b'}');
        let out = text_body(Some("application/json"), &body);
        let mut want = br#"{"a":""#.to_vec();
        for _ in 0..4 {
            want.extend_from_slice("\u{FFFD}".as_bytes());
        }
        want.extend_from_slice(b"\"}");
        assert_eq!(out.as_ref(), want.as_slice());
    }

    #[test]
    fn text_body_mends_lone_surrogate_escapes() {
        // \ud800 孤高代理 → \ufffd 字面六字符
        let out = text_body(Some("application/json"), br#"{"a":"\ud800"}"#);
        assert_eq!(out.as_ref(), br#"{"a":"\ufffd"}"#.as_slice());
        // 孤低代理同样
        let out = text_body(Some("application/json"), br#"{"a":"x\udc00y"}"#);
        assert_eq!(out.as_ref(), br#"{"a":"x\ufffdy"}"#.as_slice());
        // 高代理后不是低代理转义（\n 打断）→ 孤
        let out = text_body(Some("application/json"), br#"{"a":"\ud800\nz"}"#);
        assert_eq!(out.as_ref(), br#"{"a":"\ufffd\nz"}"#.as_slice());
    }

    #[test]
    fn text_body_keeps_surrogate_pairs_and_valid_escapes() {
        // 合法代理对（Python json.dumps 写 emoji 的形态）原样保留
        let body = br#"{"a":"\ud83d\ude00 \u4e2d"}"#;
        let out = text_body(Some("application/json"), body);
        assert_eq!(out.as_ref(), body.as_slice());
        // 大写 hex 的代理对也是一对
        let body = br#"{"a":"\uD83D\uDE00"}"#;
        let out = text_body(Some("application/json"), body);
        assert_eq!(out.as_ref(), body.as_slice());
        // 坏 hex（\uZZZZ）不是转义：原样（零拷贝）
        let body = br#"{"a":"\uZZZZ"}"#;
        assert!(matches!(text_body(Some("application/json"), body), Cow::Borrowed(_)));
        // 尾部截断的 \u12：原样
        let body = br#"{"a":"\u12"#;
        assert!(matches!(text_body(Some("application/json"), body), Cow::Borrowed(_)));
    }

    #[test]
    fn text_body_valid_body_is_zero_copy() {
        let body = "{\"a\":\"你好 emoji 😄\"}".as_bytes();
        let out = text_body(Some("application/json; charset=utf-8"), body);
        assert!(matches!(out, Cow::Borrowed(b) if b.as_ptr() == body.as_ptr()));
        // 没有 Content-Type 也按 JSON 处理（Go：空 ct 跳过媒体判定）
        let out = text_body(None, body);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    #[test]
    fn text_body_skips_non_json_content_and_bodies() {
        // 字节媒体类型：不碰（哪怕有坏字节）
        let gbk = b"{\"a\":\"\xc4\xe3\"}".as_slice();
        for ct in [
            "image/png",
            "multipart/form-data; boundary=x",
            "audio/wav",
            "video/mp4",
            "application/octet-stream",
            "application/x-www-form-urlencoded",
            "IMAGE/PNG",
        ] {
            assert!(matches!(text_body(Some(ct), gbk), Cow::Borrowed(_)), "{ct}");
        }
        // 体不以 {/[ 开头：不碰
        let plain = b"hello \xc4\xe3".as_slice();
        assert!(matches!(text_body(Some("application/json"), plain), Cow::Borrowed(_)));
        // 前导空白后是 {：算 JSON
        let mut body = b" \r\n\t".to_vec();
        body.extend_from_slice(b"{\"a\":\"\xc4\"}");
        let out = text_body(Some("application/json"), &body);
        assert!(matches!(out, Cow::Owned(_)));
    }

    #[test]
    fn fix_text_handles_raw_utf8_lone_surrogate_encoding() {
        // UTF-8 编码的孤代理（ED A0 80，JSON 里非法但字节流可能带上）：
        // 三个字节各成一个 U+FFFD（Go utf8.DecodeRune 每次回 size 1）。
        let mut body = br#"{"a":""#.to_vec();
        body.extend_from_slice(&[0xed, 0xa0, 0x80]);
        body.extend_from_slice(b"\"}");
        let out = fix_text(&body);
        let mut want = br#"{"a":""#.to_vec();
        for _ in 0..3 {
            want.extend_from_slice("\u{FFFD}".as_bytes());
        }
        want.extend_from_slice(b"\"}");
        assert_eq!(out, want);
        // 输入里本来就有的 U+FFFD 不再翻倍
        let body = "a\u{FFFD}b".as_bytes();
        assert_eq!(fix_text(body), body);
        // 截断的 4 字节序列（F0 9F 98 后没字节了）：Go 实测每字节一个 U+FFFD
        let body = b"{\"a\":\"\xf0\x9f\x98".as_slice();
        let out = fix_text(body);
        assert_eq!(out.len(), body.len() - 3 + 9); // 3 个坏字节 → 3×(EF BF BD)
        assert_eq!(&out[..6], b"{\"a\":\"");
        assert_eq!(&out[6..], "\u{FFFD}\u{FFFD}\u{FFFD}".as_bytes());
    }

    #[test]
    fn needs_text_covers_both_defect_classes() {
        // 非 UTF-8
        assert!(needs_text(b"{\"a\":\"\xff\"}"));
        // 孤代理转义
        assert!(needs_text(b"{\"a\":\"\\ud800\"}"));
        assert!(needs_text(b"{\"a\":\"\\udc00\"}"));
        // 干净体
        assert!(!needs_text(b"{\"a\":\"ok \\n \\\" \\\\ \\ud83d\\ude00\"}"));
        // 尾部单个反斜杠：i+1 越界 → false
        assert!(!needs_text(b"{\"a\":\"x\\"));
    }

    // ----- prepare（顺序：先 text 后 base64，与 Go 两个挂点同序）-----

    #[test]
    fn prepare_runs_text_fix_then_base64_unify() {
        // 同一个体里两类毛病都修：GBK 字节 + URL-safe 内联图。
        // 拼法：{"model":"p/m","note":"<GBK 两字节>","u":"data:image/png;base64,<URL-safe>"}
        let b64 = url_safe_run();
        let mut body = br#"{"model":"p/m","note":""#.to_vec();
        body.extend_from_slice(&[0xc4, 0xe3]);
        body.extend_from_slice(br#"","u":"data:image/png;base64,"#);
        body.extend_from_slice(b64.as_bytes());
        body.extend_from_slice(b"\"}");
        let out = prepare(Proto::Chat, Some("application/json"), &body);
        let v: Value = serde_json::from_slice(&out).expect("json");
        let note = v["note"].as_str().unwrap();
        assert_eq!(note.as_bytes(), "\u{FFFD}\u{FFFD}".as_bytes());
        let u = v["u"].as_str().unwrap();
        assert!(u.starts_with("data:image/png;base64,"));
        assert!(!u.contains('-') && !u.contains('_'), "{u}");
    }
}
