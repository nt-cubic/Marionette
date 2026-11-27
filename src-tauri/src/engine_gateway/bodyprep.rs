//! 请求体预处理（`base64_input.go` + `body_text.go` + `webp.go` 的对应）。
//!
//! 三件读体之后、任何路由/翻译/bodycap 之前对对话体做的事：
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
//! - **webp 内联图重写成 PNG**（`webpBody`，webp.go:47；#4732，
//!   zhangqingwen204 的报告）：Claude Desktop 的浏览器工具给 webp 截图，
//!   GLM 读不了——400 [1210]「图片输入格式/解析错误」把它打回，它的
//!   Anthropic 式端点又把同一张当成自家文件回 400 [1214]「必须传入
//!   file_id、file_url、file_data 至少之一」，两个交替；图片骑在会话历史
//!   里，之后每轮都被拒、会话卡死。Go 按厂商学（webpRefused 的 unfit 记
//!   号，首回合付一次拒答学费；不透明时写 JPEG 省体积，heic.go 的
//!   pictureWritten）；**Rust 半步**：预处理链无条件转、一律 PNG（GLM 两个
//!   400 的共同根因是 webp 本身，PNG 谁都收；按厂商学表与 converted 缓存
//!   ——图骑在历史里每轮重转——是路由层的后续任务）。解码失败原样放行
//!   （厂商自己说）。排在 stdBase64 之后：字母表归一了，`webp_of` 的标准
//!   base64 解码才对得上 URL-safe 形态来的图。
//!
//! - **出站体脱敏（链末位，2026-11-18 接上）**：三步之后、任何路由/翻译
//!   之前 `redact::scrub_cow`（`super::redact`，Go `internal/redact` 主 rules
//!   表的纯字符串半步；**默认关**，`MARIONETTE_REDACT=1` 才开）。Err 是
//!   fails-closed 的拒发错误：`try_prepared_request` 把它交给调用方（routes
//!   四臂 Err → 503，Go maskStatus 的非拦截分支，internal/gateway/
//!   redact.go:229）；老形状 `prepare`/`prepared_request` 没有错误通道，Err
//!   消化成**空体**（各对话臂拨号前就地 4xx，扫不了的体一个字节不出门——
//!   给不走 try 形状的调用方兜底）。关态 `scrub_cow` 零拷贝借回——接上后
//!   关态零行为变化。
//!
//! 接线在 routes.rs 的对话臂（含 count_tokens——Go countTokens 同样在计数
//! 前跑 stdBase64Body，gateway.go:2112）：httpd.rs 读体（含 Content-Encoding
//! 解码）之后、route() 分发之前正是 Go 的位置。字节级预检
//! [`may_hold_odd_base64`] / [`may_hold_webp`] 是性能闸门（Go 注释：2MB 请求
//! ~2ms），不过闸的体零拷贝原样返回。
//!
//! **HEIC 转码挂起（如实记档，2026-11-18）**：Go webp 之外还有 heic.go
//! （iPhone 照片直拖，DeepSeek on WorkBuddy 的拒答）——解码器是
//! gen2brain/h265 的**纯 Go HEIC 实现**（heic.go:23-25，magpie 全系统免
//! cgo）。Rust 侧没有等价的纯 Rust HEVC 解码器，而引擎的体积/依赖红线
//! 禁止新增 crate（libheif 系全是 C 绑定）——**不做**，iPhone HEIC 原样
//! 发给厂商由它自己拒答。红线松动（或社区出现可维护的纯 Rust HEVC 解码
//! 器）时再按 heic.go 的形状补：字节嗅探 ftyp brand（heic.go:31）→
//! 透明 PNG / 不透明 JPEG（pictureWritten 同款）。
use std::borrow::Cow;

use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, STANDARD};
use base64::engine::DecodePaddingMode;
use base64::Engine as _;
use serde_json::Value;

use super::effort::Proto;
use super::httpd::Request;
use super::redact;

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
    let alphabet = if url_safe {
        &base64::alphabet::URL_SAFE
    } else {
        &base64::alphabet::STANDARD
    };
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
    serde_json::to_vec(&v)
        .map(Cow::Owned)
        .unwrap_or(Cow::Borrowed(body))
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
                        let data = source
                            .get("data")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                            .to_string();
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
    let mt = ct
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
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
    match std::str::from_utf8(&body[i..i + len])
        .ok()
        .and_then(|s| s.chars().next())
    {
        Some(ch) => (Some(ch), len),
        None => (None, 1),
    }
}

// ---------------------------------------------------------------------------
// webp.go
// ---------------------------------------------------------------------------

/// Go heicMaxBase64（heic.go:55）：一张内联图的 base64 上限（48MB）。
const HEIC_MAX_BASE64: usize = 48 << 20;
/// Go heicMaxPixels（heic.go:57）：解码前的像素上限（50M——48MP 的照片解码
/// 时驻留数百 MB）。
const HEIC_MAX_PIXELS: u64 = 50_000_000;
/// Go heicMaxSide（heic.go:62）：重写后最长边 4096（pictureWritten 的
/// triangleResize 门槛——超长边重写会把体积放大到失控）。
const HEIC_MAX_SIDE: u32 = 4096;

/// mayHoldWebP（webp.go:40）：体里可能有 webp 吗——`image/webp` 类型、它的
/// JSON 转义形态 `image\/webp`（中继可能 \u 转义转发，#1669）、或 RIFF 头的
/// 标准 base64 前缀 `UklG`（webp 挂在别的 mime 下也是 webp，Go 对 HEIC 同款）。
fn may_hold_webp(body: &[u8]) -> bool {
    fn has_sub(hay: &[u8], needle: &[u8]) -> bool {
        hay.windows(needle.len()).any(|w| w == needle)
    }
    has_sub(body, b"image/webp") || has_sub(body, b"image\\/webp") || has_sub(body, b"UklG")
}

/// isWebP（webp.go:90）：按文件头认 webp（RIFF....WEBP）。
fn is_webp(b: &[u8]) -> bool {
    b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP"
}

/// webpOf（webp.go:69）：data URI 是 webp 图时给它的字节。Go 对每个内联图
/// 按头 16 个 base64 字符嗅探——mime 声明什么不算数（挂在 image/jpeg 下的
/// webp 也转，HEIC 假 mime 同款）。长度门与 Go 一致（<16 不嗅、>48MB 不碰）。
/// 字母表是标准 padded 形态（prepare 链里 std_base64_body 在前面跑过；Go
/// 在 serve() 更早处同样归一过），尾随 `=` 剥掉按 Raw 语义解。
fn webp_of(src: &str) -> Option<Vec<u8>> {
    let rest = src.strip_prefix("data:")?;
    let (head, b64) = rest.split_once(',')?;
    head.strip_suffix(";base64")?;
    if b64.len() < 16 || b64.len() > HEIC_MAX_BASE64 {
        return None;
    }
    let sniff = STANDARD.decode(&b64[..16]).ok()?;
    if !is_webp(&sniff) {
        return None;
    }
    base64::engine::general_purpose::STANDARD_NO_PAD
        .decode(b64.trim_end_matches('='))
        .ok()
}

/// encodeWebP（webp.go:106）+ pictureWritten（heic.go:248）的合并：webp 字节
/// 解码后写成 PNG。None = 解不开 / 超像素限（Go 返回 `("", nil)` → 块原样，
/// 厂商自己说）。与 Go 的差：Go 无透明通道时写 JPEG（体积），Rust 半步一律
/// PNG；Go 的 converted 缓存与单并发槽（heicSlots）不搬（模块头）。
fn webp_to_png(raw: &[u8]) -> Option<Vec<u8>> {
    // Go 先 DecodeConfig 量尺寸再解码；Rust 用 WebPDecoder::dimensions 同序：
    // 头过了 50M 像素门，才取像素。
    use image::ImageDecoder as _;
    let dec = image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(raw)).ok()?;
    let (w, h) = dec.dimensions();
    if w == 0 || h == 0 || u64::from(w) * u64::from(h) > HEIC_MAX_PIXELS {
        return None;
    }
    let img = cap_side(image::DynamicImage::from_decoder(dec).ok()?);
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
        .ok()?;
    Some(out)
}

/// pictureWritten 的 heicMaxSide 半边：最长边超 4096 时 Triangle 滤镜缩回
/// （Go triangleResize 同款滤镜；Go 在超大图后还要 debug.FreeOSMemory——Rust
/// 的解码缓冲由 Drop 回收，不搬）。
fn cap_side(img: image::DynamicImage) -> image::DynamicImage {
    let (w, h) = (img.width(), img.height());
    if w.max(h) <= HEIC_MAX_SIDE {
        return img;
    }
    let (nw, nh) = if w >= h {
        (
            HEIC_MAX_SIDE,
            (((h as u64) * HEIC_MAX_SIDE as u64) / w as u64).max(1) as u32,
        )
    } else {
        (
            (((w as u64) * HEIC_MAX_SIDE as u64) / h as u64).max(1) as u32,
            HEIC_MAX_SIDE,
        )
    };
    image::DynamicImage::ImageRgba8(image::imageops::resize(
        &img,
        nw,
        nh,
        image::imageops::FilterType::Triangle,
    ))
}

/// webpAsJPEG（webp.go:47）的 Rust 半步：体里每个 webp 内联图写成 PNG（Go：
/// JPEG，透明时 PNG）；没有可写的原样返回（零拷贝），解码失败原样。走的
/// 形状同 [`std_base64_body`]：所有 data URI 字符串统一处理（对三种协议一律
/// 成立），anthropic 的 `source{type:"base64"}` 单独走（Go imageSrc 的
/// anthropic 分支合成 `data:<media_type>;base64,<data>` 再拆回）。
pub(crate) fn webp_body(proto: Proto, body: &[u8]) -> Cow<'_, [u8]> {
    if !may_hold_webp(body) {
        return Cow::Borrowed(body);
    }
    let mut v: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(_) => return Cow::Borrowed(body),
    };
    let mut changed = false;
    walk_webp(&mut v, proto, &mut changed);
    if !changed {
        return Cow::Borrowed(body);
    }
    serde_json::to_vec(&v)
        .map(Cow::Owned)
        .unwrap_or(Cow::Borrowed(body))
}

/// 递归转写：webp 的 data URI 换成 PNG 的；anthropic 的 base64 source 改
/// `data` 与 `media_type`（Go withImageData 的 anthropic 分支同款——其它
/// 字段原样带过）。
fn walk_webp(v: &mut Value, proto: Proto, changed: &mut bool) {
    match v {
        Value::String(s) => {
            if let Some(png) = webp_of(s).as_deref().and_then(webp_to_png) {
                *s = format!("data:image/png;base64,{}", STANDARD.encode(&png));
                *changed = true;
            }
        }
        Value::Array(items) => {
            for item in items {
                walk_webp(item, proto, changed);
            }
        }
        Value::Object(map) => {
            if proto == Proto::Anthropic {
                if let Some(source) = map.get_mut("source").and_then(|s| s.as_object_mut()) {
                    let is_b64 = source.get("type").and_then(Value::as_str) == Some("base64");
                    if is_b64 {
                        if let Some(data) = source.get("data").and_then(Value::as_str) {
                            let media = source
                                .get("media_type")
                                .and_then(Value::as_str)
                                .unwrap_or("");
                            let uri = format!("data:{media};base64,{data}");
                            if let Some(png) = webp_of(&uri).as_deref().and_then(webp_to_png) {
                                source
                                    .insert("media_type".into(), Value::String("image/png".into()));
                                source.insert("data".into(), Value::String(STANDARD.encode(&png)));
                                *changed = true;
                            }
                        }
                    }
                }
            }
            for (_, val) in map.iter_mut() {
                walk_webp(val, proto, changed);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// 接线入口
// ---------------------------------------------------------------------------

/// 三步合并 + redact 收口（顺序同 Go：textBody 在读体出口、stdBase64Body 在
/// serve() 最前、webp 转码在 stdBase64 之后——字母表归一了 webp_of 才解得开
/// URL-safe 形态来的图；都先于任何路由/翻译/bodycap。第四步 redact::scrub 是
/// 链末位——redact.rs 模块头的既定接线点，2026-11-18 接上）。体没变时零拷贝
/// 借回；scrub 开态扫不了（体不是合法 UTF-8）= Err——调用方拒发
/// （fails-closed，Go maskStatus 的非拦截分支 503 同款，
/// internal/gateway/redact.go:229）。
pub fn try_prepare<'a>(
    proto: Proto,
    content_type: Option<&str>,
    body: &'a [u8],
) -> Result<Cow<'a, [u8]>, String> {
    try_prepare_with(redact::enabled(), proto, content_type, body)
}

/// [`try_prepare`] 的可注入开关版（测试用，免碰进程环境）。
fn try_prepare_with<'a>(
    on: bool,
    proto: Proto,
    content_type: Option<&str>,
    body: &'a [u8],
) -> Result<Cow<'a, [u8]>, String> {
    let prepared = match text_body(content_type, body) {
        Cow::Borrowed(b) => std_then_webp(proto, b),
        Cow::Owned(fixed) => match std_then_webp(proto, &fixed) {
            Cow::Borrowed(_) => Cow::Owned(fixed),
            Cow::Owned(out) => Cow::Owned(out),
        },
    };
    // scrub_cow 关态/开态零命中都借回——这里只在真打码时才 Owned（别为
    // 几十 MB 的图体付一次复制）。
    let masked = match redact::scrub_cow(on, &prepared) {
        Ok(Cow::Owned(masked)) => Some(masked),
        Ok(Cow::Borrowed(_)) => None,
        Err(e) => return Err(e),
    };
    Ok(match masked {
        Some(m) => Cow::Owned(m),
        None => prepared,
    })
}

/// 老形状（routes.rs 的四臂已换 [`try_prepared_request`]，这里留给不走 try
/// 形状的调用方兜底）：三步 + redact，Err 在内部消化——**空体**。每个对话臂
/// 都在拨号前就地 4xx（chat 的 model 解析 404 no provider、messages/
/// responses/count_tokens 的 JSON 解析 400 invalid body），扫不了的体一个
/// 字节不出门——fails-closed 的语义保住；状态码形不是 Go 的 503。默认关 =
/// 这里永远 Ok（唯一新增成本 = 一次 env 查询）。
pub fn prepare<'a>(proto: Proto, content_type: Option<&str>, body: &'a [u8]) -> Cow<'a, [u8]> {
    prepare_with(redact::enabled(), proto, content_type, body)
}

/// [`prepare`] 的可注入开关版（测试用，免碰进程环境）。
fn prepare_with<'a>(
    on: bool,
    proto: Proto,
    content_type: Option<&str>,
    body: &'a [u8],
) -> Cow<'a, [u8]> {
    match try_prepare_with(on, proto, content_type, body) {
        Ok(prepared) => prepared,
        Err(_) => Cow::Owned(Vec::new()),
    }
}

/// stdBase64Body 之后再跑 webpBody（`std_then_webp` 的两步形状，见
/// [`prepare`] 的顺序注释）。
fn std_then_webp<'a>(proto: Proto, body: &'a [u8]) -> Cow<'a, [u8]> {
    match std_base64_body(proto, body) {
        Cow::Borrowed(b) => webp_body(proto, b),
        Cow::Owned(std) => match webp_body(proto, &std) {
            Cow::Borrowed(_) => Cow::Owned(std),
            Cow::Owned(out) => Cow::Owned(out),
        },
    }
}

/// routes.rs 对话臂共用：[`prepare`] 之后带上预处理体的请求。体没变时借回
/// 原请求（零拷贝）；变了才重建（method/path/headers 浅克隆——对话体几十
/// MB，头几百字节，不值当为此改 httpd 的读体链）。
pub fn prepared_request<'a>(request: &'a Request, proto: Proto) -> Cow<'a, Request> {
    match try_prepared_request(request, proto) {
        Ok(req) => req,
        // 老形状没有错误通道：消化成空体（见 [`prepare`] 的注释——各对话臂
        // 拨号前就地 4xx，一个字节不出门）。
        Err(_) => Cow::Owned(Request {
            method: request.method.clone(),
            path: request.path.clone(),
            headers: request.headers.clone(),
            body: Vec::new(),
        }),
    }
}

/// routes.rs 四臂的现行形状：[`try_prepare`] 之后带上预处理体的请求。
/// Err（redact 开态扫不了的体）原样交给调用方——对话臂按 fails-closed
/// 回 503（Go maskStatus 的非拦截分支，internal/gateway/redact.go:229）。
/// 体没变时借回原请求（零拷贝）；变了才重建（同 [`prepared_request`]）。
pub fn try_prepared_request<'a>(
    request: &'a Request,
    proto: Proto,
) -> Result<Cow<'a, Request>, String> {
    try_prepared_request_with(redact::enabled(), request, proto)
}

/// [`try_prepared_request`] 的可注入开关版（测试用，免碰进程环境）。
fn try_prepared_request_with<'a>(
    on: bool,
    request: &'a Request,
    proto: Proto,
) -> Result<Cow<'a, Request>, String> {
    let prepared = try_prepare_with(on, proto, request.header("content-type"), &request.body)?;
    Ok(match prepared {
        Cow::Borrowed(_) => Cow::Borrowed(request),
        Cow::Owned(body) => Cow::Owned(Request {
            method: request.method.clone(),
            path: request.path.clone(),
            headers: request.headers.clone(),
            body,
        }),
    })
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
        let url = v["messages"][0]["content"][0]["image_url"]["url"]
            .as_str()
            .unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
        let std = url.strip_prefix("data:image/png;base64,").unwrap();
        assert!(!std.contains('-') && !std.contains('_'), "{std}");
        // 解码回来是同一份字节
        let raw_std = STANDARD.decode(std).unwrap();
        let raw_url = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(b64.as_bytes())
            .unwrap();
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
        assert!(matches!(
            text_body(Some("application/json"), body),
            Cow::Borrowed(_)
        ));
        // 尾部截断的 \u12：原样
        let body = br#"{"a":"\u12"#;
        assert!(matches!(
            text_body(Some("application/json"), body),
            Cow::Borrowed(_)
        ));
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
        assert!(matches!(
            text_body(Some("application/json"), plain),
            Cow::Borrowed(_)
        ));
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

    // ----- webpBody（webp.go:47）-----

    /// 手搓的最小合法 VP8L（无损）webp：1×1 蓝色不透明像素，34 字节。
    /// 位流：VP8L 头（0x2F + 14bit 宽-1 + 14bit 高-1 + alpha + version 全零）
    /// 之后三个零位（无 transform / 无色缓存 / 无 meta-huffman），再五个
    /// 单符号简单前缀码（simple=1、num_symbols-1=0、first8=1、8 位符号）：
    /// G=0、R=0、B=255、A=255、距离=0——LSB 先行逐位打包。下方的解码断言
    /// 钉住这串字节：image-webp 行为变了这里会响。
    const TINY_WEBP: &[u8] = &[
        0x52, 0x49, 0x46, 0x46, 0x1a, 0x00, 0x00, 0x00, // "RIFF" + 尺寸 26
        0x57, 0x45, 0x42, 0x50, // "WEBP"
        0x56, 0x50, 0x38, 0x4c, 0x0d, 0x00, 0x00, 0x00, // "VP8L" + 块长 13
        0x2f, // VP8L 签名
        0x00, 0x00, 0x00, 0x00, // 宽/高/alpha/version 头（32 个零位）
        0x28, 0x40, 0x01, 0xea, 0xdf, 0xff, 0x02, 0x00, // 五个单符号码 + 填充
        0x00, // 奇数块的 pad 字节
    ];

    #[test]
    fn tiny_webp_constant_decodes_as_one_blue_pixel() {
        let img = image::load_from_memory(TINY_WEBP).expect("hand-built VP8L decodes");
        assert_eq!((img.width(), img.height()), (1, 1));
        // 手工构造的 VP8L 常量经解码器色彩空间往返有 ±1 舍入（实测 B=254）；
        // 断言「近纯蓝」而不是逐位相等——测的是「这是那张图」而不是色彩管理。
        let [r, g, b, a] = img.to_rgba8().get_pixel(0, 0).0;
        assert!(
            r <= 2 && g <= 2 && b >= 250 && a == 255,
            "got [{r},{g},{b},{a}]"
        );
    }

    #[test]
    fn webp_body_rewrites_chat_data_uri_to_png() {
        let b64 = STANDARD.encode(TINY_WEBP);
        let body = format!(
            r#"{{"model":"p/m","messages":[{{"role":"user","content":[{{"type":"image_url","image_url":{{"url":"data:image/webp;base64,{b64}"}}}}]}}]}}"#
        );
        let out = webp_body(Proto::Chat, body.as_bytes());
        let v: Value = serde_json::from_slice(&out).expect("json");
        let url = v["messages"][0]["content"][0]["image_url"]["url"]
            .as_str()
            .unwrap();
        assert!(url.starts_with("data:image/png;base64,"), "{url}");
        let png = STANDARD
            .decode(url.strip_prefix("data:image/png;base64,").unwrap())
            .unwrap();
        let img = image::load_from_memory(&png).expect("rewritten picture decodes");
        // 与常量单测同口径：解码器色彩往返 ±1 舍入（实测 B=254）。
        let [r, g, b, a] = img.to_rgba8().get_pixel(0, 0).0;
        assert!(
            r <= 2 && g <= 2 && b >= 250 && a == 255,
            "像素不丢：got [{r},{g},{b},{a}]"
        );
        assert_eq!(v["model"], "p/m", "其余字段不动");
    }

    #[test]
    fn webp_body_sniffs_bytes_under_a_lying_mime() {
        // mime 声明 image/jpeg、字节是 webp：Go webpOf 只认字节（HEIC 假
        // mime 同款），一样转。
        let b64 = STANDARD.encode(TINY_WEBP);
        let body = format!(r#"{{"u":"data:image/jpeg;base64,{b64}"}}"#);
        let out = webp_body(Proto::Chat, body.as_bytes());
        let v: Value = serde_json::from_slice(&out).expect("json");
        assert!(v["u"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
    }

    #[test]
    fn webp_body_rewrites_anthropic_base64_source() {
        let b64 = STANDARD.encode(TINY_WEBP);
        let body = format!(
            r#"{{"model":"p/m","messages":[{{"role":"user","content":[{{"type":"image","source":{{"type":"base64","media_type":"image/webp","data":"{b64}"}}}}]}}]}}"#
        );
        let out = webp_body(Proto::Anthropic, body.as_bytes());
        let v: Value = serde_json::from_slice(&out).expect("json");
        let src = &v["messages"][0]["content"][0]["source"];
        assert_eq!(src["type"], "base64");
        assert_eq!(src["media_type"], "image/png");
        let png = STANDARD.decode(src["data"].as_str().unwrap()).unwrap();
        assert!(
            image::load_from_memory(&png).is_ok(),
            "写回的是可解码的 PNG"
        );
    }

    #[test]
    fn webp_body_reads_the_escaped_mime_form() {
        // 中继把 JSON 里的 "/" 转义成 "\/"（#1669）：预检认这个形态，解析后
        // serde 已还原成 data:image/webp;...。
        let b64 = STANDARD.encode(TINY_WEBP);
        let body = format!(r#"{{"u":"data:image\/webp;base64,{b64}"}}"#);
        let out = webp_body(Proto::Chat, body.as_bytes());
        let v: Value = serde_json::from_slice(&out).expect("json");
        assert!(v["u"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
    }

    #[test]
    fn webp_body_leaves_bad_webp_and_other_bodies_alone() {
        // 坏 webp（RIFF 头真、载荷截断）：解码失败原样放行——厂商自己说
        let b64 = STANDARD.encode(&TINY_WEBP[..24]);
        let body = format!(r#"{{"u":"data:image/webp;base64,{b64}"}}"#);
        let out = webp_body(Proto::Chat, body.as_bytes());
        assert_eq!(out.as_ref(), body.as_bytes());
        // 非 webp 的 data URI（PNG）：字节嗅不过，原样
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([1, 2, 3, 255]),
        ))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
        let body = format!(
            r#"{{"u":"data:image/png;base64,{}"}}"#,
            STANDARD.encode(png)
        );
        let out = webp_body(Proto::Chat, body.as_bytes());
        assert!(matches!(out, Cow::Borrowed(b) if b.as_ptr() == body.as_bytes().as_ptr()));
        // 没有任何标记的体：预检零拷贝
        let plain = br#"{"a":"hello"}"#;
        assert!(
            matches!(webp_body(Proto::Chat, plain), Cow::Borrowed(b) if b.as_ptr() == plain.as_ptr())
        );
        // 预检过了但不是 JSON：原样
        let raw = format!("x = image/webp {b64}", b64 = STANDARD.encode(TINY_WEBP));
        assert_eq!(
            webp_body(Proto::Chat, raw.as_bytes()).as_ref(),
            raw.as_bytes()
        );
    }

    #[test]
    fn cap_side_resizes_the_long_side_to_4096() {
        let wide = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8192,
            4,
            image::Rgba([0, 0, 0, 255]),
        ));
        let out = cap_side(wide);
        assert_eq!((out.width(), out.height()), (4096, 2));
        let tall = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            4,
            8192,
            image::Rgba([0, 0, 0, 255]),
        ));
        let out = cap_side(tall);
        assert_eq!((out.width(), out.height()), (2, 4096));
        // 门内不碰
        let small = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            4096,
            4096,
            image::Rgba([0, 0, 0, 255]),
        ));
        let (w, h) = (small.width(), small.height());
        let out = cap_side(small);
        assert_eq!((out.width(), out.height()), (w, h));
    }

    #[test]
    fn prepare_runs_text_std_then_webp() {
        // 同一个体：GBK 字节修掉 + webp data URI 转 PNG。
        let b64 = STANDARD.encode(TINY_WEBP);
        let mut body = br#"{"model":"p/m","note":""#.to_vec();
        body.extend_from_slice(&[0xc4, 0xe3]);
        body.extend_from_slice(br#"","u":"data:image/webp;base64,"#);
        body.extend_from_slice(b64.as_bytes());
        body.extend_from_slice(b"\"}");
        let out = prepare(Proto::Chat, Some("application/json"), &body);
        let v: Value = serde_json::from_slice(&out).expect("json");
        assert_eq!(
            v["note"].as_str().unwrap().as_bytes(),
            "\u{FFFD}\u{FFFD}".as_bytes()
        );
        assert!(v["u"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
    }

    // ----- redact 收口（链末位，默认关）-----

    #[test]
    fn redact_off_state_is_byte_identical_zero_copy() {
        // 关态：体里有钥匙也原样借回（ptr 相等 = 连复制都没有）
        let body = br#"{"model":"p/m","messages":[{"role":"user","content":"my key sk-Abcdef12345678901234"}]}"#;
        let out = try_prepare_with(false, Proto::Chat, Some("application/json"), body).unwrap();
        assert!(matches!(out, Cow::Borrowed(b) if b.as_ptr() == body.as_ptr()));
    }

    #[test]
    fn redact_on_masks_keys_and_leaves_images_alone() {
        // 开态：sk- 钥匙打码后才进路由。同体的标准 padded PNG data URI 与
        // stdBase64/webp 两步无关（字母表本就标准、字节非 webp），原样带过。
        let png = STANDARD.encode([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]); // PNG 魔数
        let body = format!(
            r#"{{"model":"p/m","u":"data:image/png;base64,{png}","content":"key sk-Abcdef12345678901234"}}"#
        );
        let out =
            try_prepare_with(true, Proto::Chat, Some("application/json"), body.as_bytes()).unwrap();
        let s = String::from_utf8(out.into_owned()).unwrap();
        assert!(s.contains("{{API_KEY_"), "{s}");
        assert!(!s.contains("sk-Abcdef12345678901234"), "{s}");
        assert!(s.contains(&png), "内联图不被动：{s}");
    }

    #[test]
    fn redact_on_bad_utf8_non_json_errs_fails_closed() {
        // 开态 + 扫不了的体 = Err（fails-closed 拒发）。JSON 体的坏字节在第
        // 一步 textBody 就修成 U+FFFD，走不到这个 Err——这里用 textBody 不碰
        // 的字节媒体类型。
        let bad: &[u8] = b"\xff\xfe\x00not json";
        assert!(
            try_prepare_with(true, Proto::Chat, Some("application/octet-stream"), bad).is_err()
        );
        // 同体关态：原样借回
        let out =
            try_prepare_with(false, Proto::Chat, Some("application/octet-stream"), bad).unwrap();
        assert!(matches!(out, Cow::Borrowed(b) if b.as_ptr() == bad.as_ptr()));
    }

    #[test]
    fn legacy_prepare_digests_redact_err_to_empty_body() {
        // 老形状没有错误通道：Err 消化成空体——chat 臂解析 model 得 ""、
        // 404 no provider，messages/responses 臂 400 invalid body，全都在
        // 拨号前就地回话，扫不了的体一个字节不出门。（pub prepare 读进程
        // 环境，测试走注入版。）
        let bad: &[u8] = b"\xff\xfe raw";
        assert!(
            try_prepare_with(true, Proto::Chat, Some("application/octet-stream"), bad).is_err(),
            "开态先确认这体真的 Err"
        );
        let digested = prepare_with(true, Proto::Chat, Some("application/octet-stream"), bad);
        assert!(digested.is_empty());
    }

    #[test]
    fn try_prepared_request_propagates_err_and_borrows_when_clean() {
        // routes 四臂的现行形状：开态扫不了的体 Err 交出去（调用方 503）；
        // 关态/干净体借回原请求（Request 级 ptr 相等 = 零拷贝）。
        let req = Request {
            method: "POST".into(),
            path: "/v1/chat/completions".into(),
            headers: vec![("content-type".into(), "application/octet-stream".into())],
            body: b"\xff\xfe raw".to_vec(),
        };
        assert!(try_prepared_request_with(true, &req, Proto::Chat).is_err());
        let out = try_prepared_request_with(false, &req, Proto::Chat).unwrap();
        assert!(matches!(&out, Cow::Borrowed(r) if std::ptr::eq(*r, &req)));
    }
}
