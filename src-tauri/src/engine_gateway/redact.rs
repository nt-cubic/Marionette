//! 出站体脱敏（Go `redact.go` + `redact_phase.go` 的纯模块半步，survey N13）。
//!
//! Go 的全貌是 Settings → Privacy 的 masking（plugins/redact 服务
//! internal/masking 的 masker，规则在 internal/redact）：占位符替换 + 回答
//! 还原 + 本地 pii 模型 + 用户词表/规则/动作，**fails closed**——设置要脱敏
//! 而没有 masker 服务/脱敏失败 = 错误，请求不出门（redactedHits）。
//!
//! Rust 半步（默认**关**、无消费者、不接进请求路径）：
//!
//! - [`enabled()`]：环境变量 `MARIONETTE_REDACT=1` 才开（近似 Go 的
//!   `masking.WithForced`——secrets 与 personal 两类全开；settings 的分级
//!   开关/kinds 不搬）。
//! - [`scrub`]：**单向**打码（占位符换掉密钥类模式后再发）；Go 的占位符记忆
//!   与回答还原（vendor 侧写回）、本地模型、用户词表都不在——同一值同一
//!   占位符（会话历史逐轮可读，Go placeholder 的同款性质）用的是进程内
//!   随机键哈希（SipHash），不是 Go 的 HMAC 换原表。
//! - 模式表：**internal/redact/redact.go 主 `rules` 表的词面 1:1**（纯字符串
//!   实现，无 regex 依赖）——`sk-` 钥匙族 / `Authorization`·`Cookie` 头 /
//!   `AKIA`·`ASIA` / GitHub·Google·Slack·Cloudflare 等厂商前缀 / JWT /
//!   私钥块 / URL 密码 / 密码·密钥·令牌字段赋值 / 邮箱 / 中国身份证 / 手机号 /
//!   银行卡（各自带 ok 判定：notASlug / secretValue / headerValue / realEmail
//!   近似 / luhn / 身份证校验位；手机号的 phoneAt 含 fieldName/phoneName/
//!   nameWords 整族，2026-11-18 收口）。**没搬**：crypto.go 与 more.go 的附加
//!   规则、CJK 命名规则（密码：/密钥为/apikey 为）、本地 pii 模型、用户
//!   words/rules/allow/block/warn 动作、SECRET 规则的 `at:` 上下文判定
//!   fieldAt/notCode 一族（字段赋值规则少了「值其实是下一个字段名/代码调用」
//!   的排除；PHONE 的 fieldName 已搬、EMAIL 的 emailAt/hiddenInURL 一直有）、
//!   publicsuffix 的 ICANN 后缀表（邮箱 TLD 用「2–63 个纯字母或 localTLD」
//!   近似）。字节级扫描 raw body（Go 的 MaskJSON 走解析后的 JSON 值，转义
//!   形态的密钥两边读法不同）。
//! - fails closed：开着时体不是合法 UTF-8（没法整体扫描）= [`Err`](scrub)，
//!   调用方拒发；关着原样（什么都不做）。
//!
//! **接线（2026-11-18 已接）**：bodyprep 链末位——`bodyprep::try_prepare`
//! 在三步（textBody → stdBase64Body → webpBody）之后、任何路由/翻译之前调
//! [`scrub_cow`]；Err 是 fails-closed 的拒发错误（Go maskStatus 的非拦截分支
//! 503 同款，internal/gateway/redact.go:229）。老形状 `bodyprep::prepare`
//! 没有错误通道，Err 在 bodyprep 内部消化成空体（拨号前就地 4xx，一个字节
//! 不出门）；503 文案要 routes.rs 换用 `try_prepare` 才有（主会话一行收口）。
//! 默认关 = 零行为变化（`scrub_cow` 关态零拷贝借回）。
use std::borrow::Cow;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hash, Hasher};

/// 一处命中：要换成占位符的区间（只换值本身——字段/头规则换值不换名字，
/// Go 的 capture group 1 同款）。
struct Hit {
    start: usize,
    end: usize,
    kind: &'static str,
}

/// 脱敏开着吗：`MARIONETTE_REDACT=1`（默认 false——Go 的 Settings › Privacy
/// 默认也不脱敏）。
pub fn enabled() -> bool {
    std::env::var("MARIONETTE_REDACT").as_deref() == Ok("1")
}

/// 把 body 里的密钥类模式换成占位符再发。关着原样；开着时体不是合法 UTF-8
/// = Err（调用方拒发，fails closed）。Go 的失败面更宽（masker 不在/panic/被
/// 拦截都是错），这里唯一的失败面是没法整体扫描的坏字节。
pub fn scrub(body: &[u8]) -> Result<Vec<u8>, String> {
    Ok(scrub_cow(enabled(), body)?.into_owned())
}

/// [`scrub`] 的 Cow 形态（bodyprep 接线用，2026-11-18）：关 = 零拷贝借回
/// （几十 MB 的图体别为「看一眼开关」复制一遍）；开 = 打码后 Owned、
/// 零命中也借回（多数体没有密钥）。
pub(crate) fn scrub_cow<'a>(on: bool, body: &'a [u8]) -> Result<Cow<'a, [u8]>, String> {
    if !on {
        return Ok(Cow::Borrowed(body));
    }
    let text = std::str::from_utf8(body)
        .map_err(|e| format!("redact: body is not valid UTF-8 ({e}); fails closed, nothing sent"))?;
    let mut hits = Vec::new();
    scan(text, &mut hits);
    if hits.is_empty() {
        return Ok(Cow::Borrowed(body));
    }
    // 同起点取最长，其后与已换区间重叠的丢弃（「Authorization: Bearer sk-…」
    // 会同时命中头值规则与 sk- 规则——先到的整段赢）
    hits.sort_by_key(|h| (h.start, std::cmp::Reverse(h.end)));
    let mut out = String::with_capacity(text.len());
    let mut at = 0usize;
    let mut i = 0usize;
    while i < hits.len() {
        let hit = &hits[i];
        out.push_str(&text[at..hit.start]);
        out.push_str(&placeholder(hit.kind, &text[hit.start..hit.end]));
        at = hit.end;
        i += 1;
        while i < hits.len() && hits[i].start < at {
            i += 1;
        }
    }
    out.push_str(&text[at..]);
    Ok(Cow::Owned(out.into_bytes()))
}

/// [`scrub`] 的可注入开关版（测试用，免碰进程环境）。
fn scrub_with(on: bool, body: &[u8]) -> Result<Vec<u8>, String> {
    Ok(scrub_cow(on, body)?.into_owned())
}

/// Go 的占位符形状（placeholderRe）：`{{KIND_xxxxxxxx}` 的完整形态
/// `{{KIND_xxxxxxxx}}`，8 个小写 base32 字符。同一值同一占位符（历史逐轮可
/// 读）——进程内随机键（SipHash）的哈希：没有还原、没有跨进程稳定（Go 用
/// HMAC + 内存换原表，这里单向，见模块头）。
fn placeholder(kind: &str, value: &str) -> String {
    static KEY: std::sync::OnceLock<RandomState> = std::sync::OnceLock::new();
    let key = KEY.get_or_init(RandomState::new);
    let mut h = key.build_hasher();
    value.hash(&mut h);
    kind.hash(&mut h);
    let bits = h.finish();
    let alphabet = b"abcdefghijklmnopqrstuvwxyz234567";
    let code: String = (0..8).map(|i| alphabet[((bits >> (5 * i)) & 31) as usize] as char).collect();
    format!("{{{{{kind}_{code}}}}}")
}

// ---------------------------------------------------------------------------
// ok 判定（redact.go 的同名函数，1:1）
// ---------------------------------------------------------------------------

/// notAVariable（redact.go:340）：`${PASS}`、`<password>`、`****` 这类值占位
/// 不是值。
fn not_a_variable(v: &str) -> bool {
    if let Some(c) = v.as_bytes().first() {
        if b"$%{<[*".contains(c) {
            return false;
        }
    }
    !v.trim_matches(|c| matches!(c, '*' | 'x' | 'X' | '.')).is_empty()
}

/// notASlug（redact.go:525）：小写连字符词（页面地址 `sk-hynix-…`）不是钥匙；
/// 钥匙的字符是随机的——有大写、或去 `sk-` 后全十六进制（UUID 形）、或有
/// ≥16 的无分隔段。
fn not_a_slug(v: &str) -> bool {
    if v.bytes().any(|b| b.is_ascii_uppercase()) {
        return true;
    }
    let body = v.strip_prefix("sk-").unwrap_or(v);
    if body.bytes().all(|b| b == b'-' || b == b'_' || b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return true;
    }
    v.split(['-', '_']).any(|w| w.len() >= 16)
}

/// secretValue（redact.go:392）：字母与数字都有、不是 `process.env`/`os.` 开
/// 头的代码读法、不是变量占位。
fn secret_value(v: &str) -> bool {
    if !not_a_variable(v) || v.starts_with("process.env") || v.starts_with("os.") {
        return false;
    }
    v.bytes().any(|b| b.is_ascii_digit()) && v.bytes().any(|b| b.is_ascii_alphabetic())
}

/// headerValue（redact.go:547）：Authorization/Cookie 行的余下部分是头的值而
/// 不是以这个词开头的散文（"Authorization: the user must sign in first"）——
/// 凭据（一个词，或跟在 Bearer/Basic 这类 scheme 词后）里有数字或符号且
/// ≥8；或 Cookie 的 name=value 对。
fn header_value(v: &str) -> bool {
    let v = v.trim();
    if v.is_empty() || !not_a_variable(v) {
        return false;
    }
    let not_letter = |c: char| !c.is_ascii_alphabetic();
    let fields: Vec<&str> = v.split_whitespace().collect();
    if fields.len() == 1 || (fields.len() == 2 && !fields[0].contains(not_letter)) {
        let cred = fields[fields.len() - 1];
        return cred.len() >= 8 && not_a_variable(cred) && cred.contains(not_letter);
    }
    let has_kv = v.split(';').any(|kv| {
        let kv = kv.trim();
        !kv.is_empty()
            && match kv.split_once('=') {
                Some((name, _)) => !name.is_empty() && !name.contains([' ', '\t']),
                None => false,
            }
    });
    has_kv && v.contains('=')
}

/// luhn（redact.go:739）——顺带限定 15–19 位数字。
fn luhn(v: &str) -> bool {
    let ds: Vec<u32> = v.bytes().filter(|b| b.is_ascii_digit()).map(|b| u32::from(b - b'0')).collect();
    if !(15..=19).contains(&ds.len()) {
        return false;
    }
    let mut sum = 0u32;
    for i in 0..ds.len() {
        let mut d = ds[ds.len() - 1 - i];
        if i % 2 == 1 {
            d *= 2;
            if d > 9 {
                d -= 9;
            }
        }
        sum += d;
    }
    sum % 10 == 0
}

/// chineseID（redact.go:730）：二代身份证校验位。
fn chinese_id(v: &str) -> bool {
    const W: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
    let b = v.as_bytes();
    if b.len() != 18 {
        return false;
    }
    let mut sum = 0u32;
    for i in 0..17 {
        if !b[i].is_ascii_digit() {
            return false;
        }
        sum += u32::from(b[i] - b'0') * W[i];
    }
    let want = b"10X98765432"[(sum % 11) as usize];
    b[17].to_ascii_uppercase() == want
}

// ---------------------------------------------------------------------------
// 邮箱的 realEmail 近似（redact.go:601）——publicsuffix 不搬
// ---------------------------------------------------------------------------

/// localTLDs（redact.go:669）。
const LOCAL_TLDS: [&str; 7] = ["local", "internal", "lan", "corp", "home", "intranet", "localdomain"];
/// fileExt（redact.go:689）＋scaleExt（:704）：文件扩展名不是邮箱 TLD
/// （`@2x.jpg` 是文件名；`.zip/.mov/.md/.map` 前带倍率也是）。
const FILE_EXT: [&str; 50] = [
    "jpg", "jpeg", "png", "gif", "webp", "svg", "heic", "heif", "avif", "bmp", "tif", "tiff", "ico", "icns", "pdf",
    "svgz", "apng", "jxl", "psd", "eps", "lottie", "mp4", "m4v", "webm", "mkv", "avi", "mp3", "m4a", "wav", "ogg",
    "flac", "aac", "ttf", "otf", "woff", "woff2", "eot", "json", "yaml", "yml", "toml", "xml", "txt", "csv", "tsv",
    "log", "html", "htm", "css", "js",
];
const SCALE_EXT: [&str; 4] = ["zip", "mov", "md", "map"];
/// docFiles（redact.go:661）：`xxx@AGENTS.md` 是提示词点名文件不是地址。
const DOC_FILES: [&str; 15] = [
    "agents.md", "claude.md", "gemini.md", "readme.md", "skill.md", "changelog.md", "contributing.md", "license.md",
    "lessons.md", "todo.md", "copilot-instructions.md", "crush.md", "qwen.md", "iflow.md", "warp.md",
];
/// mailHost 的 linkerWords（redact.go:656）：`system@plt`、`puts@got.plt` 是反
/// 汇编符号不是域名。
const LINKER_WORDS: [&str; 5] = ["plt", "got", "sec", "iplt", "igot"];

/// realEmail（redact.go:601）的近似：git@ 排除、假域排除、文件名形态排除、
/// 链接器符号排除；**publicsuffix 的 ICANN 判断换成「TLD 是 2–63 个纯字母或
/// localTLD」**（差见模块头）。
fn real_email(v: &str) -> bool {
    let Some(at) = v.rfind('@') else { return false };
    let (local, host) = (&v[..at], v[at + 1..].to_ascii_lowercase());
    if local.eq_ignore_ascii_case("git") {
        return false; // git@github.com 这类 SSH 用户名
    }
    if !mail_host(local, &host) {
        return false;
    }
    let mut host = host.as_str();
    if file_host(host) {
        // 文件名的尾巴：`alice@acme.com.pdf` 仍是人的地址；`@2x.jpg` 不是域
        host = file_stem(host);
        if !host.contains('.') || file_host(host) || !mail_host(local, host) {
            return false;
        }
    }
    let tld = host.rsplit('.').next().unwrap_or("");
    if DOC_FILES.contains(&host) || FILE_EXT.contains(&tld) {
        return false;
    }
    if !LOCAL_TLDS.contains(&tld) && !(tld.len() >= 2 && tld.len() <= 63 && tld.bytes().all(|b| b.is_ascii_alphabetic())) {
        return false;
    }
    for fake in ["example.com", "example.org", "example.net", "localhost", ".test", ".invalid", ".example"] {
        let bare = fake.trim_start_matches('.');
        if host == bare || host.ends_with(fake) {
            return false;
        }
    }
    !host.starts_with("noreply") && !v.contains("noreply@")
}

/// mailHost（redact.go:638）：有点、标签是字母数字连字符、不是链接器符号表。
fn mail_host(local: &str, host: &str) -> bool {
    if local.ends_with('@') || !host.contains('.') || host.trim_matches('.') != host {
        return false;
    }
    let mut linker = true;
    for label in host.split('.') {
        if label.is_empty() || label.bytes().any(|b| b < 0x80 && b != b'-' && !b.is_ascii_alphanumeric()) {
            return false;
        }
        if !LINKER_WORDS.contains(&label) {
            linker = false;
        }
    }
    !linker
}

/// fileHost（redact.go:712）＋scaleExt 的倍率判定。
fn file_host(host: &str) -> bool {
    let Some(dot) = host.rfind('.') else { return false };
    let ext = &host[dot + 1..];
    if FILE_EXT.contains(&ext) {
        return true;
    }
    if SCALE_EXT.contains(&ext) {
        // 倍率（2x、1.5x）＋扩展名 → 文件名；`ops@files.zip` 仍是地址
        let stem = &host[..dot];
        if let Some(x) = stem.strip_suffix('x') {
            if !x.is_empty() && x.split('.').all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())) {
                return true;
            }
        }
    }
    false
}

/// fileStem（redact.go:719）。
fn file_stem(host: &str) -> &str {
    let mut host = host;
    while let Some(dot) = host.rfind('.') {
        if FILE_EXT.contains(&&host[dot + 1..]) {
            host = &host[..dot];
        } else {
            return host;
        }
    }
    host
}

/// emailAt（redact.go:442）+ notUserinfo（:431）：前面贴着 `.` 的不是地址；
/// 后面跟 `.`+token 字符的（`a@b.io.12`）不是；URL userinfo 里的不是
/// （`postgres://app:pw@host` 的整段归 PASSWORD 规则）。
fn email_at(s: &str, a: usize, b: usize) -> bool {
    let bytes = s.as_bytes();
    if a > 0 && bytes[a - 1] == b'.' {
        return false;
    }
    if b + 1 < bytes.len() && bytes[b] == b'.' && is_token(bytes[b + 1]) {
        return false;
    }
    !in_userinfo(s, a)
}

/// inUserinfo（redact.go:455）：a 在 `scheme://` 之后、host 的 @ 之前。
fn in_userinfo(s: &str, a: usize) -> bool {
    let bytes = s.as_bytes();
    let mut i = a;
    while i > 0 && !b" \t\r\n/?#\"'<>@`".contains(&bytes[i - 1]) {
        i -= 1;
    }
    if i < 3 || bytes[i - 1] != b'/' || bytes[i - 2] != b'/' || bytes[i - 3] != b':' {
        return false;
    }
    let j = i - 3; // scheme 的冒号
    let mut k = j;
    while k > 0 && (bytes[k - 1].is_ascii_alphanumeric() || b"+.-".contains(&bytes[k - 1])) {
        k -= 1;
    }
    while k < j && !bytes[k].is_ascii_alphabetic() {
        k += 1; // scheme 以字母开头
    }
    k < j
}

// ---------------------------------------------------------------------------
// 手机号的 phoneAt 近似（phone.go:41）
// ---------------------------------------------------------------------------

/// phoneWords / phoneMarks（phone.go:31/34）——fax 是电话词不是编号词
/// （Go phoneWords 收尾的 fax，2026-11-18 收口时从 ID_WORDS 挪回原位）。
const PHONE_WORDS: [&str; 14] = [
    "phone", "phones", "mobile", "mobiles", "tel", "telephone", "cell", "cellphone", "contact", "contacts",
    "whatsapp", "sms", "msisdn", "fax",
];
const PHONE_MARKS: [&str; 6] = ["手机", "电话", "電話", "联系", "聯繫", "联络"];
/// idWords（phone.go:37）：数字前面贴着这些词 = 编号不是手机号（词面 1:1）。
const ID_WORDS: [&str; 23] = [
    "order", "id", "uid", "no", "no.", "#", "number", "ticket", "invoice", "订单", "订单号", "单号", "编号", "流水号",
    "工单", "工单号", "运单号", "快递单号", "订单编号", "交易号", "序号", "账单号", "id号",
];

/// phoneAt（phone.go:41）：>11 位（+86/86 前缀）直接是；恰 11 位先看
/// **字段形态**（fieldName：`name:`/`name=`/`name：`/`<tag>` 的值位——字段名
/// 里有电话词才是电话，`id:` 是编号），再看 `#` 前缀，最后散文窗（32 字节）
/// 里紧贴的 idWords（整词）。2026-11-18 收口：fieldName 从「窗口后缀」近似
/// 升级成 Go 整读（name/prev/长 CJK 取尾 6 字都搬了）。
fn phone_at(s: &str, a: usize, b: usize) -> bool {
    if b - a > 11 {
        return true; // +86 / 86
    }
    if let Some((name, prev)) = field_name(s, a) {
        return phone_name(name) || (!prev.is_empty() && phone_name(prev)) || name.eq_ignore_ascii_case("number");
    }
    let bytes = s.as_bytes();
    if a > 0 && bytes[a - 1] == b'#' {
        return false; // #13912345678（phone.go:49）
    }
    // 32 字节窗（phone.go:51），对齐字符边界（窗口里可能是中文）
    let mut window_start = a.saturating_sub(32);
    while window_start < a && !s.is_char_boundary(window_start) {
        window_start += 1;
    }
    let mut before = s[window_start..a]
        .trim_end_matches([' ', '\t', '(', '"', '\'', '\u{201c}', '\u{300c}'])
        .to_ascii_lowercase();
    // 收尾的 是/为/為 剥一次（phone.go:52：「我的手机号是 138…」的系词；
    // 「订单号是 138…」剥完才是订单号）
    for c in ["是", "为", "為"] {
        if let Some(rest) = before.strip_suffix(c) {
            before = rest.trim_end_matches([' ', '\t']).to_string();
        }
    }
    for w in ID_WORDS {
        let Some(rest) = before.strip_suffix(w) else { continue };
        // 整词：ASCII idWord 前面贴着 [a-z0-9_] 的是更长词的尾巴（recorder
        // 的 order）；中文 idWord 没有这条（Go 只对 w[0] < 0x80 查，phone.go:62）
        if !rest.is_empty() && w.as_bytes()[0] < 0x80 {
            if let Some(c) = rest.chars().next_back() {
                if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' {
                    continue;
                }
            }
        }
        // 但电话自己的编号词：phone no. 138…（phone.go:66，前一词是电话名）
        if w == "no" || w == "no." || w == "number" {
            let rest = rest.trim_end_matches(' ');
            let from = name_start(rest, rest.len());
            if phone_name(&rest[from..]) {
                return true;
            }
        }
        return false;
    }
    true
}

/// fieldName（phone.go:100）：s[a] 起的值的字段名——`:`、`=`、`：` 或 XML
/// 元素的 `>` 前的词（中间可隔引号空白），JSON/k=v/YAML/SQL/XML 的形态；
/// prev 是再往前隔一个空格的词（「phone no: …」的电话词）。散文（值前没有
/// 这些分隔符）返回 None。
fn field_name<'a>(s: &'a str, a: usize) -> Option<(&'a str, &'a str)> {
    let bytes = s.as_bytes();
    let mut i = a;
    while i > 0 && matches!(bytes[i - 1], b' ' | b'\t' | b'"' | b'\'') {
        i -= 1;
    }
    if i > 0 && bytes[i - 1] == b'>' {
        // <tel>13800138000</tel>：元素名（phone.go:107，窗 64 字节找 <）
        let mut lo = i.saturating_sub(65);
        while lo < i && !s.is_char_boundary(lo) {
            lo += 1;
        }
        let Some(k) = s[lo..i - 1].rfind('<') else { return None };
        let mut tag = &s[lo + k + 1..i - 1];
        if let Some(sp) = tag.find([' ', '\t']) {
            tag = &tag[..sp];
        }
        if tag.is_empty() || !tag.bytes().all(|c| is_alnum(c) || matches!(c, b'_' | b'.' | b':' | b'-')) {
            return None;
        }
        return Some((tag, ""));
    }
    if i > 0 && (bytes[i - 1] == b':' || bytes[i - 1] == b'=') {
        i -= 1;
    } else if i >= 3 && s.is_char_boundary(i - 3) && &s[i - 3..i] == "：" {
        i -= 3;
    } else {
        return None;
    }
    while i > 0 && matches!(bytes[i - 1], b' ' | b'\t' | b'"' | b'\'') {
        i -= 1;
    }
    let j = name_start(s, i);
    if j == i {
        return None;
    }
    let mut name = &s[j..i];
    let runes = name.chars().count();
    if runes > 6 && has_cjk(name) {
        // 一长串 CJK 后跟分隔符是句子：取最后 6 个字符当值的名字
        // （「我的手机号是：…」，phone.go:137）
        let skip = runes - 6;
        let at = name.char_indices().nth(skip).map(|(b, _)| b).unwrap_or(0);
        name = &name[at..];
    }
    let mut prev = "";
    if j >= 2 && bytes[j - 1] == b' ' && bytes[j - 2] != b' ' {
        let pj = name_start(s, j - 1);
        prev = &s[pj..j - 1];
    }
    Some((name, prev))
}

/// phoneName（phone.go:79）：字段名是电话的——词表里有它的词
/// （phone_number、mobileNo），或带 phoneMarks（手机号）。
fn phone_name(name: &str) -> bool {
    if name_words(name).iter().any(|w| PHONE_WORDS.contains(&w.as_str())) {
        return true;
    }
    PHONE_MARKS.iter().any(|m| name.contains(m))
}

/// nameWords（ids.go:284）：字段名的词（小写）——next_page_token、
/// nextPageToken、X-Next-Page-Token → next/page/token。字节级照抄 Go
/// （分隔符 _-. 断词、小写字母后跟大写断词），中文整段成一个词。
fn name_words(name: &str) -> Vec<String> {
    let bytes = name.as_bytes();
    let mut out = Vec::new();
    let mut w: Vec<u8> = Vec::new();
    for i in 0..bytes.len() {
        let c = bytes[i];
        if c == b'_' || c == b'-' || c == b'.' {
            if !w.is_empty() {
                out.push(String::from_utf8_lossy(&w).into_owned());
                w.clear();
            }
        } else if c.is_ascii_uppercase() {
            if i > 0 && bytes[i - 1].is_ascii_lowercase() && !w.is_empty() {
                out.push(String::from_utf8_lossy(&w).into_owned());
                w.clear();
            }
            w.push(c + 32);
        } else {
            w.push(c);
        }
    }
    if !w.is_empty() {
        out.push(String::from_utf8_lossy(&w).into_owned());
    }
    out
}

/// nameStart（phone.go:150）：s[..i] 结尾的名字从哪开始——字母数字、
/// `_` `.` `-` 或 CJK 字符（U+4E00–U+9FFF）往前。
fn name_start(s: &str, i: usize) -> usize {
    let mut j = i;
    while j > 0 {
        let Some(c) = s[..j].chars().next_back() else { break };
        let ok = (c.is_ascii() && (is_alnum(c as u8) || matches!(c as u8, b'_' | b'.' | b'-')))
            || ('\u{4e00}'..='\u{9fff}').contains(&c);
        if !ok {
            break;
        }
        j -= c.len_utf8();
    }
    j
}

/// hasCJK（phone.go:162）。
fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c))
}

// ---------------------------------------------------------------------------
// URL 密码的 hiddenInURL（redact.go:489）
// ---------------------------------------------------------------------------

/// hiddenInURL：URL 的密码只在没在同一个 URL 里到处出现时才打码
/// （`postgresql://geo:geo@127.0.0.1/geo` 的用户名=密码=路径，见 Go 注释）。
fn hidden_in_url(s: &str, pw_start: usize, pw_end: usize) -> bool {
    let pw = &s[pw_start..pw_end];
    if pw_start == 0 || s.as_bytes()[pw_start - 1] != b':' {
        return true;
    }
    let Some(i) = s[..pw_start - 1].rfind("//") else { return true };
    if &s[i + 2..pw_start - 1] == pw {
        return false; // 它就是用户名
    }
    let rest = &s[pw_end + 1..]; // @ 之后的 host 与 path
    let end = rest.find(|c: char| " \t\r\n\"'<>`\\)]}".contains(c)).unwrap_or(rest.len());
    for part in rest[..end].split(|c: char| "/:.?#&=;,@".contains(c)) {
        if part == pw {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// 扫描器（redact.go `rules` 主表，词面 1:1，纯字符串）
// ---------------------------------------------------------------------------

fn is_token(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

fn is_secret_ch(c: u8) -> bool {
    matches!(c, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'-' | b'.' | b'/' | b'+' | b'=' | b'~' | b'!' | b'@' | b'#' | b'%' | b'^' | b'&' | b'*')
}

fn is_field_name(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-')
}

fn is_alnum(c: u8) -> bool {
    c.is_ascii_alphanumeric()
}

/// bound 检查（rule.bound）：两侧贴着 bound 字符集内的字符 = 更长词的一部
/// 分，不算命中。
fn bound_ok(s: &str, a: usize, b: usize, bound: fn(u8) -> bool) -> bool {
    let bytes = s.as_bytes();
    (a == 0 || !bound(bytes[a - 1])) && (b >= bytes.len() || !bound(bytes[b]))
}

/// 从 from 起把 class 的连续段吃完，返回段尾（class 全 ASCII，落点是字符
/// 边界）。
fn run_end(s: &str, from: usize, class: fn(u8) -> bool) -> usize {
    let bytes = s.as_bytes();
    let mut i = from;
    while i < bytes.len() && class(bytes[i]) {
        i += 1;
    }
    i
}

/// 每个 `needle` 出现处都看一眼（大小写敏感）。
fn each_hit(s: &str, needle: &str, mut f: impl FnMut(&str, usize)) {
    let mut from = 0usize;
    while let Some(i) = s[from..].find(needle) {
        let i = from + i;
        f(s, i);
        from = i + needle.len();
    }
}

/// 每个 `needle` 出现处（大小写不敏感；每次调用把全文小写一遍——脱敏默认
/// 关，这条路径不为热身付费）。
fn each_hit_fold(s: &str, needle: &str, mut f: impl FnMut(&str, usize)) {
    let lower = s.to_ascii_lowercase();
    let needle = needle.to_ascii_lowercase();
    let mut from = 0usize;
    while let Some(i) = lower[from..].find(needle.as_str()) {
        let i = from + i;
        f(s, i);
        from = i + needle.len();
    }
}

/// Go 的 quote 片段 `(?:\\?["'])?`：可选一个（可带反斜杠的）引号。
fn skip_quote(s: &str, at: usize) -> usize {
    let bytes = s.as_bytes();
    if bytes.get(at) == Some(&b'\\') && matches!(bytes.get(at + 1), Some(b'"') | Some(b'\'')) {
        return at + 2;
    }
    if matches!(bytes.get(at), Some(b'"') | Some(b'\'')) {
        return at + 1;
    }
    at
}

/// 名字（b 之后）跟的是不是字段赋值的尾巴：可选引号、空白、`:`/`=`、空白、
/// 可选引号——是就返回值的起点。
fn field_value_start(s: &str, b: usize) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut j = skip_quote(s, b);
    while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
        j += 1;
    }
    if !matches!(bytes.get(j), Some(b':') | Some(b'=')) {
        return None;
    }
    j += 1;
    while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
        j += 1;
    }
    Some(skip_quote(s, j))
}

fn scan(s: &str, out: &mut Vec<Hit>) {
    scan_private_key(s, out);
    scan_sk_key(s, out);
    scan_relay_sk(s, out);
    scan_ark(s, out);
    scan_gh_pat(s, out);
    scan_vendor_prefixed(s, out);
    scan_aws(s, out);
    scan_cf(s, out);
    scan_tunnel_token(s, out);
    scan_jwt(s, out);
    scan_x_auth_key(s, out);
    scan_url_password(s, out);
    scan_secret_fields(s, out);
    scan_env_key_pass(s, out);
    scan_header_lines(s, out);
    scan_bare_key(s, out);
    scan_email(s, out);
    scan_id_card(s, out);
    scan_phone(s, out);
    scan_bank_card(s, out);
}

/// PRIVATE_KEY：`-----BEGIN ([A-Z0-9]+ )*PRIVATE KEY( BLOCK)?-----` 到配对的
/// END 标记（`[\s\S]+?` 非贪婪 = 第一个，且至少隔一个字符），整块换。
fn scan_private_key(s: &str, out: &mut Vec<Hit>) {
    each_hit(s, "-----BEGIN ", |s, i| {
        let rest = &s[i + 11..];
        let Some(rel) = rest.find("-----") else { return };
        let label = &rest[..rel];
        if !private_key_label_ok(label) {
            return;
        }
        let end_mark = format!("-----END {label}-----");
        let after = &rest[rel + 5..];
        if let Some(j) = after.find(&end_mark).filter(|j| *j >= 1) {
            let end = i + 11 + rel + 5 + j + end_mark.len();
            out.push(Hit { start: i, end, kind: "PRIVATE_KEY" });
        }
    });
}

fn private_key_label_ok(label: &str) -> bool {
    let mut words: Vec<&str> = label.split(' ').collect();
    if words.last() == Some(&"BLOCK") {
        words.pop();
    }
    if words.len() < 2 || words[words.len() - 1] != "KEY" || words[words.len() - 2] != "PRIVATE" {
        return false;
    }
    words[..words.len() - 2]
        .iter()
        .all(|w| !w.is_empty() && w.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()))
}

/// `sk-(?:ant-|proj-|or-|svcacct-|admin-)?[A-Za-z0-9_-]{20,}`（ok notASlug）。
fn scan_sk_key(s: &str, out: &mut Vec<Hit>) {
    each_hit(s, "sk-", |s, i| {
        let end = run_end(s, i + 3, is_token);
        let mut body = &s[i + 3..end];
        for p in ["ant-", "proj-", "or-", "svcacct-", "admin-"] {
            if let Some(rest) = body.strip_prefix(p) {
                body = rest;
                break;
            }
        }
        if body.len() >= 20 && not_a_slug(&s[i..end]) && bound_ok(s, i, end, is_token) {
            out.push(Hit { start: i, end, kind: "API_KEY" });
        }
    });
}

/// `[a-z]{2,4}_sk_[A-Za-z0-9_-]{20,}`（ok secretValue；`oc_sk_…` 中继自家）。
fn scan_relay_sk(s: &str, out: &mut Vec<Hit>) {
    each_hit(s, "_sk_", |s, i| {
        let bytes = s.as_bytes();
        let mut a = i;
        while a > 0 && bytes[a - 1].is_ascii_lowercase() && i - a < 4 {
            a -= 1;
        }
        if !(2..=4).contains(&(i - a)) {
            return;
        }
        let end = run_end(s, i + 4, is_token);
        if end - (i + 4) >= 20 && secret_value(&s[a..end]) && bound_ok(s, a, end, is_token) {
            out.push(Hit { start: a, end, kind: "API_KEY" });
        }
    });
}

/// `ark-[A-Za-z0-9][A-Za-z0-9_-]{15,}`（ok secretValue；火山方舟）。
fn scan_ark(s: &str, out: &mut Vec<Hit>) {
    each_hit(s, "ark-", |s, i| {
        let end = run_end(s, i + 4, is_token);
        if end - (i + 4) >= 16 && secret_value(&s[i..end]) && bound_ok(s, i, end, is_token) {
            out.push(Hit { start: i, end, kind: "API_KEY" });
        }
    });
}

/// `gh[pousr]_[A-Za-z0-9]{30,}` / `github_pat_[A-Za-z0-9_]{40,}` /
/// `glpat-[A-Za-z0-9_-]{20,}`。
fn scan_gh_pat(s: &str, out: &mut Vec<Hit>) {
    for p in [b'p', b'o', b'u', b's', b'r'] {
        let needle = format!("gh{}", p as char);
        each_hit(s, &needle, |s, i| {
            if s.as_bytes().get(i + 3) != Some(&b'_') {
                return;
            }
            let end = run_end(s, i + 4, is_alnum);
            if end - (i + 4) >= 30 && bound_ok(s, i, end, is_token) {
                out.push(Hit { start: i, end, kind: "API_KEY" });
            }
        });
    }
    each_hit(s, "github_pat_", |s, i| {
        let end = run_end(s, i + 11, |c| c.is_ascii_alphanumeric() || c == b'_');
        if end - (i + 11) >= 40 && bound_ok(s, i, end, is_token) {
            out.push(Hit { start: i, end, kind: "API_KEY" });
        }
    });
    each_hit(s, "glpat-", |s, i| {
        let end = run_end(s, i + 6, is_token);
        if end - (i + 6) >= 20 && bound_ok(s, i, end, is_token) {
            out.push(Hit { start: i, end, kind: "API_KEY" });
        }
    });
}

/// 厂商前缀族：`AIza[0-9A-Za-z_-]{35}`、`xox[abposr]-[0-9A-Za-z-]{10,}`、
/// `(?:sk|rk)_live_[0-9A-Za-z]{20,}`、`hf_…{30,}`、`gsk_…{40,}`、`xai-…{40,}`、
/// `npm_…{36}`、`pypi-…{50,}`、`dop_v1_[a-f0-9]{64}`、
/// `SG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43}`（SendGrid）。
fn scan_vendor_prefixed(s: &str, out: &mut Vec<Hit>) {
    each_hit(s, "AIza", |s, i| {
        let end = run_end(s, i + 4, is_token);
        if end - (i + 4) >= 35 && bound_ok(s, i, end, is_token) {
            out.push(Hit { start: i, end, kind: "API_KEY" });
        }
    });
    for p in [b'a', b'b', b'p', b'o', b's', b'r'] {
        let needle = format!("xox{}", p as char);
        each_hit(s, &needle, |s, i| {
            if s.as_bytes().get(i + 4) != Some(&b'-') {
                return;
            }
            let end = run_end(s, i + 5, |c| c.is_ascii_alphanumeric() || c == b'-');
            if end - (i + 5) >= 10 && bound_ok(s, i, end, is_token) {
                out.push(Hit { start: i, end, kind: "API_KEY" });
            }
        });
    }
    for head in ["sk", "rk"] {
        each_hit(s, &format!("{head}_live_"), |s, i| {
            let end = run_end(s, i + 8, is_alnum);
            if end - (i + 8) >= 20 && bound_ok(s, i, end, is_token) {
                out.push(Hit { start: i, end, kind: "API_KEY" });
            }
        });
    }
    for (needle, at, min, class) in [
        ("hf_", 3usize, 30usize, is_alnum as fn(u8) -> bool),
        ("gsk_", 4, 40, is_alnum),
        ("xai-", 4, 40, is_alnum),
        ("npm_", 4, 36, is_alnum),
        ("pypi-", 5, 50, is_token),
    ] {
        each_hit(s, needle, |s, i| {
            let end = run_end(s, i + at, class);
            if end - (i + at) >= min && bound_ok(s, i, end, is_token) {
                out.push(Hit { start: i, end, kind: "API_KEY" });
            }
        });
    }
    each_hit(s, "dop_v1_", |s, i| {
        let hex_lower = |c: u8| c.is_ascii_digit() || (b'a'..=b'f').contains(&c);
        let end = run_end(s, i + 7, hex_lower);
        if end - (i + 7) == 64 && bound_ok(s, i, end, is_token) {
            out.push(Hit { start: i, end, kind: "API_KEY" });
        }
    });
    each_hit(s, "SG.", |s, i| {
        let mid = run_end(s, i + 3, is_token);
        if mid - (i + 3) != 22 || !s[mid..].starts_with('.') {
            return;
        }
        let end = run_end(s, mid + 1, is_token);
        if end - (mid + 1) == 43 && bound_ok(s, i, end, is_token) {
            out.push(Hit { start: i, end, kind: "API_KEY" });
        }
    });
}

/// `(?:AKIA|ASIA)[0-9A-Z]{16}`（bound alnum）。
fn scan_aws(s: &str, out: &mut Vec<Hit>) {
    let upper = |c: u8| c.is_ascii_uppercase() || c.is_ascii_digit();
    for head in ["AKIA", "ASIA"] {
        each_hit(s, head, |s, i| {
            let end = run_end(s, i + 4, upper);
            if end - (i + 4) >= 16 && bound_ok(s, i, end, is_alnum) {
                out.push(Hit { start: i, end, kind: "API_KEY" });
            }
        });
    }
}

/// Cloudflare：`cf(?:ut|at|k|oat)_[A-Za-z0-9_-]{40,}`（用户/账号令牌、Global
/// API Key、Wrangler OAuth）与 Origin CA 的 `v1\.0-[A-Za-z0-9]{16,}-[A-Za-z0-9]{40,}`。
fn scan_cf(s: &str, out: &mut Vec<Hit>) {
    for needle in ["cfut_", "cfat_", "cfk_", "cfoat_"] {
        each_hit(s, needle, |s, i| {
            let end = run_end(s, i + needle.len(), is_token);
            if end - (i + needle.len()) >= 40 && bound_ok(s, i, end, is_token) {
                out.push(Hit { start: i, end, kind: "API_KEY" });
            }
        });
    }
    each_hit(s, "v1.0-", |s, i| {
        let mid = run_end(s, i + 5, is_alnum);
        if mid - (i + 5) < 16 || !s[mid..].starts_with('-') {
            return;
        }
        let end = run_end(s, mid + 1, is_alnum);
        if end - (mid + 1) >= 40 && bound_ok(s, i, end, is_token) {
            out.push(Hit { start: i, end, kind: "API_KEY" });
        }
    });
}

/// Cloudflare tunnel 令牌 `eyJhIjoi[A-Za-z0-9+/_-]{40,}={0,2}`（base64 的
/// `{"a":…}` 头）。
fn scan_tunnel_token(s: &str, out: &mut Vec<Hit>) {
    let class = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'_' | b'-');
    each_hit(s, "eyJhIjoi", |s, i| {
        let mut end = run_end(s, i + 8, class);
        if end - (i + 8) >= 40 {
            if s[end..].starts_with("==") {
                end += 2;
            } else if s[end..].starts_with('=') {
                end += 1;
            }
            if bound_ok(s, i, end, is_token) {
                out.push(Hit { start: i, end, kind: "TOKEN" });
            }
        }
    });
}

/// JWT `eyJ…\.eyJ…\.[A-Za-z0-9_-]{8,}`（三段各 ≥8/≥8/≥8——头两段还有
/// `eyJ` 本身）。
fn scan_jwt(s: &str, out: &mut Vec<Hit>) {
    let b64u = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'-';
    each_hit(s, "eyJ", |s, i| {
        let p1 = run_end(s, i + 3, b64u);
        if p1 - (i + 3) < 8 || !s[p1..].starts_with(".eyJ") {
            return;
        }
        let p2 = run_end(s, p1 + 4, b64u);
        if p2 - (p1 + 4) < 8 || !s[p2..].starts_with('.') {
            return;
        }
        let end = run_end(s, p2 + 1, b64u);
        if end - (p2 + 1) >= 8 && bound_ok(s, i, end, is_token) {
            out.push(Hit { start: i, end, kind: "TOKEN" });
        }
    });
}

/// `(?i)x-auth-(?:user-service-)?key…[:=]…([A-Za-z0-9._-]{16,})`（ok
/// notAVariable）——只换值。`(?i)` 贯到 `user-service-` 与 `key`：
/// `X-Auth-User-Service-Key=…` 与 `"x-auth-key": …` 都命中（2026-11-18 收口，
/// 旧实现 `user-service-`/`key` 按原文比对，混大小写变体漏掉）。
fn scan_x_auth_key(s: &str, out: &mut Vec<Hit>) {
    let value_class = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-');
    each_hit_fold(s, "x-auth-", |s, i| {
        let mut at = i + 7;
        if s.get(at..at + 13).is_some_and(|seg| seg.eq_ignore_ascii_case("user-service-")) {
            at += 13;
        }
        if !s.get(at..at + 3).is_some_and(|k| k.eq_ignore_ascii_case("key")) {
            return;
        }
        at += 3;
        let Some(j) = field_value_start(s, skip_quote(s, at)) else { return };
        let end = run_end(s, j, value_class);
        if end - j >= 16 && not_a_variable(&s[j..end]) {
            out.push(Hit { start: j, end, kind: "SECRET" });
        }
    });
}

/// `[A-Za-z][A-Za-z0-9+.-]*://user:password@` 的密码（组 1；ok notAVariable、
/// at hiddenInURL）——只换密码。user 可为空（`:pw@` 的 Redis 形）。
fn scan_url_password(s: &str, out: &mut Vec<Hit>) {
    // Go 的字符类补集：\s : @ / ? # " ' < >
    const STOP: &[u8] = b" \t\n\x0c\r:@/?#\"'<>";
    each_hit(s, "://", |s, i| {
        let bytes = s.as_bytes();
        let mut a = i;
        while a > 0 && (bytes[a - 1].is_ascii_alphanumeric() || matches!(bytes[a - 1], b'+' | b'.' | b'-')) {
            a -= 1;
        }
        if a == i || !bytes[a].is_ascii_alphabetic() {
            return; // 左边不是 scheme
        }
        let mut j = i + 3;
        while j < bytes.len() && !STOP.contains(&bytes[j]) {
            j += 1;
        }
        if j >= bytes.len() || bytes[j] != b':' {
            return;
        }
        let pw_start = j + 1;
        let mut k = pw_start;
        while k < bytes.len() && !STOP.contains(&bytes[k]) {
            k += 1;
        }
        if k >= bytes.len() || bytes[k] != b'@' {
            return;
        }
        if k - pw_start >= 3 && not_a_variable(&s[pw_start..k]) && hidden_in_url(s, pw_start, k) {
            out.push(Hit { start: pw_start, end: k, kind: "PASSWORD" });
        }
    });
}

/// 密码/密钥名的字段赋值（(?i)，secretNames + 首尾 ident 字符）：
/// `DB_PASSWORD=…`、`"apiKey": "…"`、`x-api-key: …`；值 secretCh{8,} 且 ok
/// secretValue——只换值。Go 的 at: fieldAt（值其实是下一个字段名/代码调用的
/// 排除）没搬。
fn scan_secret_fields(s: &str, out: &mut Vec<Hit>) {
    const NAMES: [&str; 18] = [
        "password", "passwd", "passphrase", "secret", "token", "api_key", "apikey", "api-key", "access_key",
        "accesskey", "access-key", "private_key", "privatekey", "private-key", "priv_key", "privkey", "priv-key",
        "credential",
    ];
    for name in NAMES {
        each_hit_fold(s, name, |s, mid| {
            // 名字整体：往两侧扩 [A-Za-z0-9_.-]（DB_PASSWORD、myApiKey2 这类）
            let bytes = s.as_bytes();
            let mut a = mid;
            while a > 0 && is_field_name(bytes[a - 1]) {
                a -= 1;
            }
            let mut b = mid + name.len();
            while b < bytes.len() && is_field_name(bytes[b]) {
                b += 1;
            }
            let Some(j) = field_value_start(s, b) else { return };
            let end = run_end(s, j, is_secret_ch);
            if end - j >= 8 && secret_value(&s[j..end]) {
                out.push(Hit { start: j, end, kind: "SECRET" });
            }
        });
    }
}

/// 环境变量风格的名字（**只认大写**）：`SERVICE_X_KEY=…`、`DB_PASS: …`——
/// `[A-Za-z0-9_.-]*_(?:KEY|PASS)` + 值 secretCh{8,}（ok secretValue）。Go 的
/// notAPlainKey（sort_key/cache_key/public_key）与 notCode 排除没搬（只认大
/// 写已挡掉大多数）。只换值。
fn scan_env_key_pass(s: &str, out: &mut Vec<Hit>) {
    for suffix in ["_KEY", "_PASS"] {
        each_hit(s, suffix, |s, i| {
            let bytes = s.as_bytes();
            let mut a = i;
            while a > 0 && is_field_name(bytes[a - 1]) {
                a -= 1;
            }
            let Some(j) = field_value_start(s, i + suffix.len()) else { return };
            let end = run_end(s, j, is_secret_ch);
            if end - j >= 8 && secret_value(&s[j..end]) {
                out.push(Hit { start: j, end, kind: "SECRET" });
            }
        });
    }
}

/// `(?im)^[ \t]*(?:authorization|cookie)[ \t]*:[ \t]*(\S[^\r\n]{7,})`（ok
/// headerValue）——多行文本里的整行头值：Bearer/Basic 后面的 token、Cookie
/// 的 name=value。只换值。逐行实现（(?im) 的 ^ 语义）。
fn scan_header_lines(s: &str, out: &mut Vec<Hit>) {
    let mut offset = 0usize;
    for line in s.split(['\n', '\r']) {
        let trimmed = line.trim_start_matches([' ', '\t']);
        let lead = line.len() - trimmed.len();
        for head in ["authorization", "cookie"] {
            let rest = match trimmed.get(..head.len()) {
                Some(h) if h.eq_ignore_ascii_case(head) => &trimmed[head.len()..],
                _ => continue,
            };
            let bytes = rest.as_bytes();
            let mut j = 0usize;
            while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
                j += 1;
            }
            if bytes.get(j) != Some(&b':') {
                continue;
            }
            j += 1;
            while j < bytes.len() && matches!(bytes[j], b' ' | b'\t') {
                j += 1;
            }
            let value = &rest[j..];
            // (\S[^\r\n]{7,})：非空白起头、总长 ≥8（行内已无换行）
            if value.len() >= 8 && header_value(value) {
                let start = offset + lead + head.len() + j;
                out.push(Hit { start, end: start + value.len(), kind: "SECRET" });
            }
        }
        offset += line.len() + 1;
    }
}

/// 裸 `key` 字段（providers.json 形）：`(?i)(?:^|[^A-Za-z0-9_.-])key…[:=]
/// …(secretCh{16,})`（ok secretValue——字母数字都有的 ≥16，code 里的 map
/// "key" 不会被认）。只换值。
fn scan_bare_key(s: &str, out: &mut Vec<Hit>) {
    each_hit_fold(s, "key", |s, i| {
        let bytes = s.as_bytes();
        if i > 0 && is_field_name(bytes[i - 1]) {
            return; // 前面贴着 ident 字符 → 不是裸 key（sort_key 一类）
        }
        let Some(j) = field_value_start(s, i + 3) else { return };
        let end = run_end(s, j, is_secret_ch);
        if end - j >= 16 && secret_value(&s[j..end]) {
            out.push(Hit { start: j, end, kind: "SECRET" });
        }
    });
}

/// `[A-Za-z0-9._%+-]+@[A-Za-z0-9-]+(?:\.[A-Za-z0-9-]+)*\.[A-Za-z]{2,}`（bound
/// alnum+_%+-、ok realEmail、at emailAt）。
fn scan_email(s: &str, out: &mut Vec<Hit>) {
    let local = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'%' | b'+' | b'-');
    let host = |c: u8| c.is_ascii_alphanumeric() || c == b'-';
    let bound = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'%' | b'+' | b'-');
    each_hit(s, "@", |s, i| {
        let bytes = s.as_bytes();
        let mut a = i;
        while a > 0 && local(bytes[a - 1]) {
            a -= 1;
        }
        if a == i {
            return;
        }
        let b = run_end(s, i + 1, host);
        let mut e = b;
        let mut dots = 0;
        while e < bytes.len() && bytes[e] == b'.' {
            let next = run_end(s, e + 1, host);
            if next == e + 1 {
                break;
            }
            dots += 1;
            e = next;
        }
        if dots < 1 {
            return;
        }
        let tld = s[b..e].rsplit('.').next().unwrap_or("");
        if tld.len() < 2 || !tld.bytes().all(|c| c.is_ascii_alphabetic()) {
            return;
        }
        if bound_ok(s, a, e, bound) && real_email(&s[a..e]) && email_at(s, a, e) {
            out.push(Hit { start: a, end: e, kind: "EMAIL" });
        }
    });
}

/// `[1-9]\d{5}(?:18|19|20)\d{2}(?:0[1-9]|1[0-2])(?:0[1-9]|[12]\d|3[01])\d{3}[\dXx]`
/// （bound alnum、ok chineseID——出生段的结构由校验位兜底，18 位里错一个数
/// 就过不了权重和）。
fn scan_id_card(s: &str, out: &mut Vec<Hit>) {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i + 18 <= bytes.len() {
        if (b'1'..=b'9').contains(&bytes[i]) {
            let end = run_end(s, i, |c| c.is_ascii_digit() || c == b'X' || c == b'x');
            if end - i >= 18 && chinese_id(&s[i..i + 18]) && bound_ok(s, i, i + 18, is_alnum) {
                out.push(Hit { start: i, end: i + 18, kind: "ID_CARD" });
            }
        }
        i += 1;
    }
}

/// `(?:\+?86[- ]?)?1[3-9]\d{9}`（bound alnum、at phoneAt 近似）。
fn scan_phone(s: &str, out: &mut Vec<Hit>) {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'+' || b == b'8' || (b == b'1' && bytes.get(i + 1).is_some_and(|c| (b'3'..=b'9').contains(c))) {
            let mut j = i;
            if bytes[j] == b'+' {
                j += 1;
            }
            if bytes.get(j) == Some(&b'8') && bytes.get(j + 1) == Some(&b'6') {
                j += 2;
                if matches!(bytes.get(j), Some(b'-') | Some(b' ')) {
                    j += 1;
                }
            }
            if bytes.get(j) == Some(&b'1') && bytes.get(j + 1).is_some_and(|c| (b'3'..=b'9').contains(c)) {
                let k = run_end(s, j + 2, is_alnum);
                if k - j == 11 && bound_ok(s, i, k, is_alnum) && phone_at(s, i, k) {
                    out.push(Hit { start: i, end: k, kind: "PHONE" });
                }
            }
        }
        i += 1;
    }
}

/// `[3-6]\d{3}(?:[ -]?\d{4}){2,3}(?:[ -]?\d{1,3})?`（bound alnum、ok luhn）。
/// 数字按组吃（组间单个空格/连字符），从最长的组前缀往回试——「4111… 6222…」
/// 两张卡挨着时各打各的码。
fn scan_bank_card(s: &str, out: &mut Vec<Hit>) {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if (b'3'..=b'6').contains(&bytes[i]) {
            let mut groups: Vec<(usize, usize)> = Vec::new();
            let mut j = i;
            loop {
                let start = j;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j == start {
                    break;
                }
                groups.push((start, j));
                if matches!(bytes.get(j), Some(b' ') | Some(b'-')) && bytes.get(j + 1).is_some_and(|c| c.is_ascii_digit()) {
                    j += 1; // 跳过分隔符，下一组从数字起
                } else {
                    break;
                }
            }
            let mut matched_to = None;
            let mut take = groups.len();
            while take > 0 {
                let span_end = groups[take - 1].1;
                if luhn(&s[i..span_end]) && bound_ok(s, i, span_end, is_alnum) {
                    out.push(Hit { start: i, end: span_end, kind: "BANK_CARD" });
                    matched_to = Some(span_end);
                    break;
                }
                take -= 1;
            }
            i = matched_to.unwrap_or(j).max(i + 1);
        } else {
            i += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_passes_through_untouched() {
        let body = br#"{"key":"sk-ant-api03-abcdefghijklmnopqrstuvwxyz"}"#;
        assert_eq!(scrub_with(false, body).unwrap(), body.to_vec());
    }

    #[test]
    fn bad_utf8_fails_closed_when_enabled() {
        assert!(scrub_with(true, &[0x7b, 0xff, 0x7d]).is_err());
        // 关着时连检查都不做
        assert_eq!(scrub_with(false, &[0xff]).unwrap(), vec![0xff]);
    }

    #[test]
    fn enabled_reads_the_env_var() {
        // 本模块唯一碰环境变量的测试；其余走 scrub_with
        std::env::set_var("MARIONETTE_REDACT", "1");
        assert!(enabled());
        std::env::remove_var("MARIONETTE_REDACT");
        assert!(!enabled());
        std::env::set_var("MARIONETTE_REDACT", "0");
        assert!(!enabled(), "只有 \"1\" 才开");
        std::env::remove_var("MARIONETTE_REDACT");
    }

    #[test]
    fn sk_keys_are_masked_and_stable() {
        let body = r#"{"a":"sk-ant-api03-Abc123def456ghi789","b":"sk-proj-or4xAbCdEf0123456789"}"#;
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(!out.contains("sk-ant-"), "{out}");
        assert!(out.contains("{{API_KEY_"), "{out}");
        // 同一值同一占位符（会话历史逐轮可读）；占位符 8 个小写 base32 字符。
        // 取完整形态 {{API_KEY_xxxxxxxx}}（placeholderRe，redact.go:331）——
        // match_indices 只回命中串本身，占位符正文要按 "}}" 截出来。
        let twice = r#"{"a":"sk-Abcdef12345678901234","b":"sk-Abcdef12345678901234"}"#;
        let out = String::from_utf8(scrub_with(true, twice.as_bytes()).unwrap()).unwrap();
        let ph: Vec<&str> = out
            .match_indices("{{API_KEY_")
            .map(|(i, _)| out[i..].split("}}").next().unwrap_or(""))
            .collect();
        assert_eq!(ph.len(), 2, "两处都换到了：{out}");
        assert_eq!(ph[0], ph[1], "同一值同一占位符：{out}");
        let code = ph[0].trim_start_matches("{{API_KEY_").trim_end_matches("}}");
        assert_eq!(code.len(), 8, "{}", ph[0]);
        assert!(code.chars().all(|c| c.is_ascii_lowercase() || ('2'..='7').contains(&c)), "{code}");
    }

    #[test]
    fn sk_slugs_are_not_keys() {
        // 全小写、短段、无十六进制——页面地址的样子（sk-hynix 新闻页），notASlug 放过
        let body = r#"see sk-this-is-a-slug-about-sk-hynix-profit now"#;
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert_eq!(out, body);
        // 大写 → 钥匙
        let body = r#"sk-lowercasebutUPPER1234567890"#;
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(out.contains("{{API_KEY_"), "{out}");
    }

    #[test]
    fn vendor_prefixed_keys_are_masked() {
        for k in [
            concat!("AIza", "SyA1234567890abcdefghijklmnopqrstuv"),
            "AKIAIOSFODNN7EXAMPLE",
            "ASIAIOSFODNN7EXAMPLE",
            concat!("ghp_", "AbCdEf0123456789abcdefghij012345"),
            concat!("github_pat_", "AbCdEf0123456789abcdefghij0123456789ABCDEFGH"),
            concat!("xoxb-", "1234567890abcdef-AbCd"),
            // 拆开写：GitHub push protection 会把连续字面量当 Stripe live key
            // （它是测试夹具；concat! 编译期拼回同一字符串）。
            concat!("sk_", "live_AbCdEf012345678901234567"),
            concat!("oc_sk_", "Abcdefghijklmnopqrst12"),
            concat!("ark-", "Abcdef123456789012"),
            concat!("hf_", "AbCdEf0123456789abcdefghijabcd"),
            concat!("gsk_", "AbCdEf0123456789abcdefghij0123456789abcd"),
            concat!("xai-", "AbCdEf0123456789abcdefghij0123456789abcd"),
            concat!("npm_", "AbCdEf0123456789abcdefghij0123456789ab"),
            concat!("pypi-", "AbCdEf0123456789abcdefghij0123456789abcdefghij01234567"),
            concat!("dop_v1_", "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
            concat!("SG.", "abcdefghij012345678901.abcdEf0123456789abcdefghij0123456789abcdefg"),
        ] {
            let body = format!(r#"{{"k":"{k}"}}"#);
            let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
            assert!(!out.contains(k), "{k} → {out}");
            assert!(out.contains("{{API_KEY_"), "{k} → {out}");
        }
    }

    #[test]
    fn short_or_wrong_vendor_keys_pass() {
        for k in ["AKIAIOSFODNN7EXAMP", "AIzaTooShort", "ghp_short", "dop_v1_0123456789abcdef", "hf_short"] {
            let body = format!("value {k} end");
            let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
            assert_eq!(out, body, "{k}");
        }
    }

    #[test]
    fn private_key_block_is_masked_whole() {
        let body = "-----BEGIN RSA PRIVATE KEY-----\nMIIEow...\n-----END RSA PRIVATE KEY-----";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(!out.contains("MIIEow"), "{out}");
        assert!(out.contains("{{PRIVATE_KEY_"), "{out}");
        // 配不上 END 标记 / 标签不合的（小写、BEGIN CERTIFICATE）不动
        for body in [
            "-----BEGIN RSA PRIVATE KEY-----\nno end here",
            "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----",
        ] {
            let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
            assert_eq!(out, body);
        }
    }

    #[test]
    fn jwt_and_tunnel_tokens_are_masked() {
        let jwt = "eyJhbGciOiJq.eyJzdWIiOiIxMjM0.Abc123def456";
        let body = format!("token {jwt} token");
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(!out.contains(jwt), "{out}");
        assert!(out.contains("{{TOKEN_"), "{out}");
        // 两段的不是
        let two = "eyJhbGciOiJq.eyJzdWIiOiIxMjM0";
        let body = format!("value {two} end");
        assert_eq!(String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap(), body);
    }

    #[test]
    fn authorization_and_cookie_lines_are_masked() {
        let body = "Authorization: Bearer AbCdEf12345678\nCookie: session=deadbeef1234; other=x1";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(!out.contains("Bearer AbCdEf12345678"), "{out}");
        assert!(!out.contains("deadbeef1234"), "{out}");
        assert!(out.contains("{{SECRET_"), "{out}");
        // 散文开头不算（headerValue：一个词且全字母）
        let prose = "Authorization: the user must sign in first";
        assert_eq!(String::from_utf8(scrub_with(true, prose.as_bytes()).unwrap()).unwrap(), prose);
        // 变量占位不算
        let var = "Authorization: Bearer ${TOKEN}";
        assert_eq!(String::from_utf8(scrub_with(true, var.as_bytes()).unwrap()).unwrap(), var);
        // CRLF 行尾也认
        let body = "Authorization: Bearer AbCdEf12345678\r\nX-Other: 1";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(!out.contains("AbCdEf12345678"), "{out}");
    }

    #[test]
    fn field_assignments_mask_only_the_value() {
        let body = r#"{"DB_PASSWORD":"hunter2secret1","api_key":"abcdef12345","x-api-key": "zz9za7Zulu0x"}"#;
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(!out.contains("hunter2secret1"), "{out}");
        assert!(!out.contains("abcdef12345"), "{out}");
        assert!(!out.contains("zz9za7Zulu0x"), "{out}");
        assert!(out.contains("{{SECRET_"), "{out}");
        // 字段名留着
        assert!(out.contains("DB_PASSWORD"), "{out}");
        assert!(out.contains("api_key"), "{out}");
        // .env 形态
        let body = "API_TOKEN=abcd1234efgh5678\n";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(!out.contains("abcd1234efgh5678"), "{out}");
        // 名字后面不是 [:=] 的不算；纯单词值不算（secretValue 要字母数字都有）
        let body = r#"{"note":"password is hunter2 here"}"#;
        assert_eq!(String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap(), body);
    }

    #[test]
    fn env_style_key_pass_and_bare_key() {
        let body = r#"{"SERVICE_X_KEY":"ab12cd34ef56","sort_key":"abcdef12"}"#;
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(!out.contains("ab12cd34ef56"), "{out}");
        // sort_key：小写 _key 不是 _KEY，名字类里也没有裸 key —— 不动
        assert!(out.contains("sort_key"), "{out}");
        assert!(out.contains("abcdef12"), "{out}");
        // 裸 "key" 字段：≥16 且字母数字都有
        let body = r#"{"key":"abcdef0123456789"}"#;
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(out.contains("{{SECRET_"), "{out}");
        assert!(out.contains("\"key\""), "字段名不动：{out}");
    }

    #[test]
    fn url_password_is_masked_but_not_when_repeated() {
        let body = "postgres://app:hunter2pass@db.example.cn:5432/x";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(!out.contains("hunter2pass"), "{out}");
        assert!(out.contains("postgres://app:{{PASSWORD_"), "{out}");
        assert!(out.contains("db.example.cn"), "host 不动：{out}");
        // 用户名=密码=路径（geodatafusion 形）：不打码（Go hiddenInURL）
        let body = "postgresql://geodatafusion:geodatafusion@127.0.0.1:55432/geodatafusion";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert_eq!(out, body);
        // 密码在 URL 别处出现（host 里）也不打码
        let body = "postgres://app:hunter2pass@hunter2pass.db/x";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert_eq!(out, body);
        // 无用户名的 Redis 形（:pw@）也换
        let body = "redis://:hunter2pass@127.0.0.1:6379";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(!out.contains("hunter2pass"), "{out}");
    }

    #[test]
    fn emails_are_masked_with_the_guards() {
        let body = "联系 alice.smith@acme-corp.com 谈";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(out.contains("{{EMAIL_"), "{out}");
        assert!(!out.contains("acme-corp.com"), "{out}");
        // git@ 是 SSH 用户名不是邮箱
        let body = "git@github.com:org/repo.git";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert_eq!(out, body);
        // example 域 / localhost / 保留尾
        for body in ["a@example.com", "ops@files.localhost", "x@y.test"] {
            assert_eq!(String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap(), body);
        }
        // 文件名形态（@2x.jpg / @AGENTS.md / 带倍率的 @1.5x.mov）
        for body in ["icon@2x.jpg", "prompt@AGENTS.md", "intro@1.5x.mov"] {
            assert_eq!(String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap(), body);
        }
        // 链接器符号
        let body = "system@plt calls puts@got.plt";
        assert_eq!(String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap(), body);
        // URL userinfo 里的不是邮箱（redis://bob@… 只有用户名没有密码）
        let body = "redis://bob@redis.example.com:6379";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert_eq!(out, body);
        // 句尾的句点是句子的（jo@quill.dev.）
        let body = "The address is jo@quill.dev.";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(out.contains("{{EMAIL_"), "{out}");
    }

    #[test]
    fn chinese_id_cards_need_the_checksum() {
        // 17 位 + 按权重算出的校验位 X（weights 7,9,10,5,8,4,2,1,6,3,7,9,10,5,8,4,2）
        let ok = "11010519491231002X";
        let body = format!("id card {ok} here");
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(!out.contains(ok), "{out}");
        assert!(out.contains("{{ID_CARD_"), "{out}");
        // 校验位错的不动
        let bad = "110105194912310021";
        let body = format!("id card {bad} here");
        assert_eq!(String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap(), body);
    }

    #[test]
    fn phones_respect_the_id_words() {
        // 裸的 11 位（1[3-9] 开头）是手机号
        let body = "打给他 13800138000 谈谈";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(out.contains("{{PHONE_"), "{out}");
        assert!(!out.contains("13800138000"), "{out}");
        // 编号形态不是
        for body in ["order 13800138000", "订单号 13800138000", "#13800138000", "id: 13800138000"] {
            assert_eq!(String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap(), body);
        }
        // phone no. / 电话 前缀仍是电话
        for body in ["phone no. 13800138000", "电话 13800138000"] {
            let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
            assert!(out.contains("{{PHONE_"), "{body} → {out}");
        }
        // +86 前缀也认
        let body = "+8613800138000";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(out.contains("{{PHONE_"), "{out}");
        // 更长的数字串（时间戳）不是——bound 挡
        let body = "ts 1380013800012345 here";
        assert_eq!(String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap(), body);
    }

    #[test]
    fn phone_field_names_decide_by_phone_name() {
        // 字段形态（fieldName，phone.go:100）：字段名带电话词才是电话
        let body = r#"{"phone":"13800138000","mobileNo":"13912345678"}"#;
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(out.matches("{{PHONE_").count() == 2, "{out}");
        // 编号字段（id/order_id/uid）不是——agent 查单不能被拦
        for body in [
            r#"{"id":"13800138000"}"#,
            r#"{"order_id": 13800138000}"#,
            "<uid>13800138000</uid>",
        ] {
            assert_eq!(String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap(), body);
        }
        // number: 字段（EqualFold 特例，phone.go:46）是电话；长 CJK 句子取
        // 尾 6 字命名（我的手机号是：…，phone.go:137）
        let body = r#"{"number":"13800138000"}"#;
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(out.contains("{{PHONE_"), "{out}");
        let body = "我的手机号是：13800138000";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(out.contains("{{PHONE_"), "{out}");
        // 前一词是电话名（phone no: …，prev 半边）
        let body = "phone no: 13800138000";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(out.contains("{{PHONE_"), "{out}");
    }

    #[test]
    fn bank_cards_need_luhn() {
        let body = "card 4111111111111111 ok";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(out.contains("{{BANK_CARD_"), "{out}");
        let body = "card 4111111111111112 bad";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert_eq!(out, body, "校验位不过");
        // 两张卡空格隔开：各打各的（第二张 19 位也过 luhn）
        let body = "4111111111111111 6212341234123456780";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(out.matches("{{BANK_CARD_").count() == 2, "{out}");
        assert!(!out.contains("4111111111111111"), "{out}");
        assert!(!out.contains("6212341234123456780"), "{out}");
    }

    #[test]
    fn whole_json_body_stays_valid_json() {
        // 域名得挑 realEmail 放过的：example.org 在假域清单里（redact.go:622，
        // 见 emails_are_masked_with_the_guards 对 example.com 的不动断言），
        // 这里要的是「值内命中不打穿 JSON 字符串边界」。
        let body = r#"{"model":"p/m","messages":[{"role":"user","content":"my key sk-Abcdef12345678901234 and mail me@fastmail.com"}]}"#;
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).expect("仍是合法 JSON");
        assert!(out.contains("{{API_KEY_"));
        assert!(out.contains("{{EMAIL_"));
        assert_eq!(v["model"], "p/m");
    }

    #[test]
    fn x_auth_key_header_value_is_masked() {
        let body = r#""x-auth-key": "Abcdef1234567890""#;
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(!out.contains("Abcdef1234567890"), "{out}");
        let body = "X-Auth-User-Service-Key=abcdef1234567890";
        let out = String::from_utf8(scrub_with(true, body.as_bytes()).unwrap()).unwrap();
        assert!(out.contains("{{SECRET_"), "{out}");
    }
}
