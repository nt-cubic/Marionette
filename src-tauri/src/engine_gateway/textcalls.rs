//! 正文里的工具调用（textcalls）——Go `third_party/magpie/internal/gateway/textcalls.go`
//! （1039 行）的纯逻辑移植。**零 IO、零新依赖**；mod.rs 注册与翻译路径接线由接线批次做。
//!
//! 语义（textcalls.go:14-27 文件头）：长对话深处，模型把工具调用写进正文而不是发起它们
//! （#823，Dazzle-sys：DeepSeek 经组、在 Pi 里 ~70 万 token）——Hermes 的
//! `<tool_call>{"name":…,"arguments":…}</tool_call>`、GLM 的 `<tool_call>name<arg_key>…
//! </arg_key><arg_value>…</arg_value></tool_call>`（#906）、DeepSeek 自家的
//! `<｜DSML｜invoke name="…">` 加参数（后头常散落别家的闭合标签），以及 Claude 形的
//! `<invoke name="…"><parameter name="…">…</parameter></invoke>`（可包在
//! `<function_calls>` / `<minimax:tool_call>` 里）。agent 拿到的是纯文本、什么都没跑、
//! 回合结束；让它继续，模型读回自己写进正文的调用、再照样写下一个，会话就丢了。
//! **当请求提供了工具时**：从第一个此类标记起的正文被持有；回合结束时其中每一块若
//! 「点名了请求里有的工具且参数可解析」→ 变成真 tool call，回合作为调过工具的回合
//! 结束。持有文本若不全是此类块 → 按它原来的文本放出（Go 的精确语义是**整段判定**：
//! `parseTextCalls`（:360-426）任一块失败即 ok=false，调用方把整段持有文本按原文放出；
//! 而块读出**之后**的非标签文本——模型自编的「*Result: …*」——被丢弃：调用真跑、模型
//! 读到真结果，packinggoodsllc_07668）。
//!
//! 移植范围与 Go 的差距：
//! - **不在本文件**：`textcalls_anthropic.go`（anthropicTextCalls，Messages 直通流的
//!   块级持有）、`textcalls_relay.go`（responsesTextCalls / Gemini 直通）两个变体——
//!   留给接线批次；textcalls.go 自带的 Chat 直通变体 `textCallTidy` / `wholeTextCalls` /
//!   `mayWriteCalls`（:828-1039）同样留给接线批次；历史改写族 `dropPseudoCalls` /
//!   `withoutPseudoCall` / `historyCall` / `callResult` / `saysAgain` / `plainWords`
//!   （:143-306）需要 wiring 层的 Message/Tool 形状，也未移植（四种块解析器在这里，
//!   接线时可复用）。call id（`textCallID` :737-739）由接线层生成；`Finish.calls`
//!   非空即「回合理应因工具而停」（Go 置 `ev.Stop = "tool"`）。
//! - **数字按原文保留**：Go 经 `map[string]any` / float64 往返再 marshal（>2^53 的
//!   整数会被改写；serde_json 默认会把整数写成 `5.0`）——这里的 mini JSON 把数字存成
//!   原文，语义等价、更保真。marshal 与 Go `json.Marshal` 逐字节同款：键排序、
//!   `<` `>` `&` 转义成 `\u003c` `\u003e` `\u0026`、U+2028/2029 转义（Go 测试
//!   `{"city":"a \u003c "}` 佐证）。
//! - **JSON 嵌套深度上限 500**（Go 10000）：超出按「读不出」处理 → 保持文本（安全
//!   方向，不会把坏数据变成调用）。
//! - **无工具时惰性直通**：Go 在安装点就跳过整个包装器（gateway.go:9644-9647，
//!   `names == nil` 时原样返回 `see`）——`TextCallScout::new(&[])` 同样从不持有。
//! - **feed_text 返回 `String`**（任务简报写的 `(String, ())` 元组第二元不承载信息，
//!   持有量经 `finish()` 观察）。
//! - `cut_mark` 里 Go 的 `"\xef"` 守卫（:474）按原样保留为 `'\u{ef}'`（ï）字符判断：
//!   Go 源里那是 rune 不是字节，对合法 UTF-8 输入与这里的写法同样恒为死分支。
//!
//! 回归：`cargo test --features engine-rust --bin marionette textcalls`（临时在
//! engine_gateway/mod.rs 加 `pub mod textcalls;`）。

use std::collections::HashSet;

/// textCallMarks（textcalls.go:37）：正文书写的调用以什么开头（DSML 标记除外，
/// `dsml_mark` 读它）。裸 `<invoke name=` 是 Claude 自家调用的形状（MiniMax 包一层
/// `<minimax:tool_call>`，Claude 包 `<function_calls>`）。
const TEXT_CALL_MARKS: [&str; 4] = ["<tool_call>", "<invoke name=", "<function_calls>", "<minimax:tool_call>"];

/// cutMark（textcalls.go:463-480）扫的闭合在前、开口在后的九个标记。
const CUT_MARKS: [&str; 9] = [
    "</tool_call>",
    "</parameter>",
    "</invoke>",
    "</function_calls>",
    "</minimax:tool_call>",
    "<tool_call>",
    "<invoke name=",
    "<function_calls>",
    "<minimax:tool_call>",
];

/// Go 正则 `\s` = [\t\n\f\r ]（含 \f）。与 `TrimLeft(s, " \t\r\n")` 的集合**不是**一个。
fn is_regex_ws_byte(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\x0c' | b'\r')
}

fn is_regex_ws_at(b: &[u8], i: usize) -> bool {
    b.get(i).is_some_and(|&c| is_regex_ws_byte(c))
}

fn regex_ws_skip(b: &[u8], mut i: usize) -> usize {
    while is_regex_ws_at(b, i) {
        i += 1;
    }
    i
}

/// Go `TrimLeft(s, " \t\r\n")`（textcalls.go 里反复出现的那个集合，无 \f）。
fn trim_a_start(s: &str) -> &str {
    s.trim_start_matches(|c| matches!(c, ' ' | '\t' | '\r' | '\n'))
}

fn next_lt(b: &[u8], from: usize) -> Option<usize> {
    b[from..].iter().position(|&c| c == b'<').map(|p| p + from)
}

fn lit(b: &[u8], i: usize, word: &[u8]) -> Option<usize> {
    if b[i..].starts_with(word) {
        Some(i + word.len())
    } else {
        None
    }
}

fn expect(b: &[u8], i: usize, c: u8) -> Option<usize> {
    if b.get(i) == Some(&c) {
        Some(i + 1)
    } else {
        None
    }
}

fn find_lit(b: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || b.len() < needle.len() {
        return None;
    }
    (from..=b.len() - needle.len()).find(|&i| &b[i..i + needle.len()] == needle)
}

// ---------------------------------------------------------------------------
// DSML 标记（textcalls.go:49-104 dsmlMark + :112-126 的 dsmlTag/invokePre 正则）
// ---------------------------------------------------------------------------

/// 全角管道 ｜ = U+FF5C = EF BD 9C。
fn pipe_one(b: &[u8], i: usize) -> Option<usize> {
    match b.get(i) {
        Some(b'|') => Some(i + 1),
        Some(0xEF) if b.get(i + 1) == Some(&0xBD) && b.get(i + 2) == Some(&0x9C) => Some(i + 3),
        _ => None,
    }
}

/// s[i..] 是否是 ｜ 的前 1–2 个字节（Go `strings.HasPrefix("｜", s[i:])`；对合法
/// UTF-8 输入恒为 false——Go 防的是字节级截断的流）。
fn fw_pipe_partial(rest: &[u8]) -> bool {
    rest == [0xEF] || rest == [0xEF, 0xBD]
}

/// 正则 `[\s|｜]*`：管道与空白任意混合的一段。
fn pipes_spaces_run(b: &[u8], mut i: usize) -> usize {
    loop {
        match b.get(i) {
            Some(&c) if is_regex_ws_byte(c) => i += 1,
            Some(b'|') => i += 1,
            Some(0xEF) if b.get(i + 1) == Some(&0xBD) && b.get(i + 2) == Some(&0x9C) => i += 3,
            _ => return i,
        }
    }
}

/// 正则 `[|｜][\s|｜]*DSML[\s|｜]*`（invokePre 的第一支，textcalls.go:117）。
fn dsml_pipes_prefix(b: &[u8], i: usize) -> Option<usize> {
    let j = pipe_one(b, i)?;
    let j = pipes_spaces_run(b, j);
    if !b[j..].starts_with(b"DSML") {
        return None;
    }
    Some(pipes_spaces_run(b, j + 4))
}

/// 正则 `[A-Za-z_][\w-]*:`（invokePre 的第二支：命名空间，如 minimax:）。
fn ns_prefix(b: &[u8], i: usize) -> Option<usize> {
    match b.get(i) {
        Some(&c) if c.is_ascii_alphabetic() || c == b'_' => {}
        _ => return None,
    }
    let mut j = i + 1;
    while b.get(j).is_some_and(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-') {
        j += 1;
    }
    if b.get(j) == Some(&b':') {
        Some(j + 1)
    } else {
        None
    }
}

/// invokePre 可选组的候选终点，按正则左支优先：DSML 管道 → 命名空间 → 空。
fn prefix_candidate_ends(b: &[u8], i: usize) -> Vec<usize> {
    let mut v = Vec::with_capacity(3);
    if let Some(e) = dsml_pipes_prefix(b, i) {
        v.push(e);
    }
    if let Some(e) = ns_prefix(b, i) {
        v.push(e);
    }
    v.push(i);
    v
}

/// dsmlMark（textcalls.go:49-104，1:1）：s 是否以一个 DSML 标记的「管道头」开头——
/// `<`、空白、闭合时 `/` 加空白、一根管道、然后管道与空白的任意一段（可为空）、
/// `DSML`、再一段。管道是 ASCII `|` 或全角 `｜`，任意数量任意宽度混用（#2623，
/// OKURBITO：DeepSeek 写一到两根，宽度在标签内也不总一致，所以不列拼法）。
/// 返回 (n, more)：n>0 = 标记长度；more = s 太短、还可能是「正在写」的标记开头
/// （连 `<` 都算）。注意 Go 这个函数的空白集是 `" \t\r\n"`（:55），**不含 \f**。
fn dsml_mark(b: &[u8], closing: bool) -> (usize, bool) {
    fn space(b: &[u8], i: &mut usize) {
        while matches!(b.get(*i), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            *i += 1;
        }
    }
    fn pipe(b: &[u8], i: &mut usize) -> (bool, bool) {
        match b.get(*i) {
            Some(b'|') => {
                *i += 1;
                (true, false)
            }
            Some(0xEF) if b.get(*i + 1) == Some(&0xBD) && b.get(*i + 2) == Some(&0x9C) => {
                *i += 3;
                (true, false)
            }
            _ => (false, *i < b.len() && fw_pipe_partial(&b[*i..])),
        }
    }
    fn run(b: &[u8], i: &mut usize) -> bool {
        loop {
            space(b, i);
            let (ok, part) = pipe(b, i);
            if !ok {
                return part;
            }
        }
    }
    if b.first() != Some(&b'<') {
        return (0, false);
    }
    let mut i = 1usize;
    space(b, &mut i);
    if closing && b.get(i) == Some(&b'/') {
        i += 1;
        space(b, &mut i);
    }
    if i == b.len() {
        return (0, true);
    }
    let (ok, part) = pipe(b, &mut i);
    if !ok {
        return (0, part);
    }
    if run(b, &mut i) || i == b.len() {
        return (0, true);
    }
    if !b[i..].starts_with(b"DSML") {
        return (0, b"DSML".starts_with(&b[i..]));
    }
    i += 4;
    if run(b, &mut i) || i == b.len() {
        // DSML 之后的管道段可能延续到下一片
        return (0, true);
    }
    (i, false)
}

/// textCallAt（textcalls.go:314-348）：s 里第一个书写调用的位置（None = 没有），
/// 以及（没有时）s 结尾有多少字节可能是「正在写」的标记开头。DSML 的**闭合**标签
/// 也算开始（DeepSeek 随手写 `</｜｜DSML｜｜ calls>`，喜欢一个人 on Discord）——
/// 上游把 invoke 做成调用后剩下的空闭合，按「该丢弃」持有。
fn text_call_at(s: &str) -> (Option<usize>, usize) {
    let b = s.as_bytes();
    let mut at: Option<usize> = None;
    for m in TEXT_CALL_MARKS {
        if let Some(i) = s.find(m) {
            if at.is_none_or(|a| i < a) {
                at = Some(i);
            }
        }
    }
    let mut tail = 0usize;
    let mut probe = next_lt(b, 0);
    while let Some(pos) = probe {
        if !at.is_none_or(|a| pos < a) {
            break;
        }
        let (n, more) = dsml_mark(&b[pos..], true);
        if n > 0 {
            at = Some(pos);
            break;
        }
        if more && tail == 0 {
            tail = b.len() - pos;
        }
        probe = next_lt(b, pos + 1);
    }
    if let Some(a) = at {
        return (Some(a), 0);
    }
    for m in TEXT_CALL_MARKS {
        let mb = m.as_bytes();
        let mut n = (mb.len() - 1).min(b.len());
        while n > tail {
            if b.ends_with(&mb[..n]) {
                tail = n;
                break;
            }
            n -= 1;
        }
    }
    (None, tail)
}

/// cutMark（textcalls.go:463-480，1:1）：s 结尾是某个标记被拦腰截断的前缀
/// （如 `</tool_call>` 的 `</`）就返回它；单独一个 `<` 可能只是文本。九个标记从
/// 最长前缀往下试（n≥2），再试 DSML 形（任意根数的管道，只有最后一个 `<` 可能是
/// 截断的；它已经写出管道字节才算——`"x < "` 是文本）。
fn cut_mark(s: &str) -> &str {
    for m in CUT_MARKS {
        let mb = m.as_bytes();
        let mut n = mb.len() - 1;
        while n >= 2 {
            if s.as_bytes().ends_with(&mb[..n]) {
                return &s[s.len() - n..];
            }
            n -= 1;
        }
    }
    if let Some(i) = s.rfind('<') {
        let rest = &s[i..];
        if rest.contains('|') || rest.contains('｜') || rest.contains('\u{ef}') {
            let (_, more) = dsml_mark(rest.as_bytes(), true);
            if more {
                return rest;
            }
        }
    }
    ""
}

fn strip_suffix_cutmark(s: &str) -> &str {
    s.strip_suffix(cut_mark(s)).unwrap_or(s)
}

// ---------------------------------------------------------------------------
// textCallJunk（textcalls.go:125）的手写等价——块读出后剩下的标签、GLM 留在闭合
// 前的 `}` `]` `"` `>`（#1945）、裸 invoke 的包裹层，全部剥掉。
// ---------------------------------------------------------------------------

/// 正则四支（左支优先，Go ReplaceAll 的 leftmost-first）：
/// 1. `<\s*/?\s*[|｜][\s|｜]*DSML[\s|｜]*[^<>]*>?`
/// 2. `[}\]"'>]*\s*</tool_calls?>`
/// 3. `<tool_calls?>`
/// 4. `<\s*/?\s*(?:[A-Za-z_][\w-]*:)?(?:invoke|parameter|function_calls|tool_calls?)\b[^<>]*>`
fn junk_match_at(b: &[u8], i: usize) -> Option<usize> {
    // 支 1：DSML 标签 + 到下一个 < 或 > 的内容 + 可选 >
    if b.get(i) == Some(&b'<') {
        let mut j = regex_ws_skip(b, i + 1);
        if b.get(j) == Some(&b'/') {
            j = regex_ws_skip(b, j + 1);
        }
        if let Some(j) = pipe_one(b, j) {
            let j = pipes_spaces_run(b, j);
            if b[j..].starts_with(b"DSML") {
                let mut j = pipes_spaces_run(b, j + 4);
                while !matches!(b.get(j), None | Some(b'<') | Some(b'>')) {
                    j += 1;
                }
                if b.get(j) == Some(&b'>') {
                    j += 1;
                }
                return Some(j);
            }
        }
    }
    // 支 2：`}]"'>` 一段 + 空白 + </tool_call> / </tool_calls>（s? 贪心，先长后短）
    {
        let mut j = i;
        while matches!(b.get(j), Some(b'}' | b']' | b'"' | b'\'' | b'>')) {
            j += 1;
        }
        let j = regex_ws_skip(b, j);
        for close in ["</tool_calls>", "</tool_call>"] {
            if b[j..].starts_with(close.as_bytes()) {
                return Some(j + close.len());
            }
        }
    }
    // 支 3：<tool_call> / <tool_calls>
    for open in ["<tool_calls>", "<tool_call>"] {
        if b[i..].starts_with(open.as_bytes()) {
            return Some(i + open.len());
        }
    }
    // 支 4：invoke / parameter / function_calls / tool_call(s) 的开或闭标签，
    // 可带命名空间；名字后必须是词边界（`<invoked>` 不算，`<invoke-x>` 算）
    if b.get(i) == Some(&b'<') {
        let mut j = regex_ws_skip(b, i + 1);
        if b.get(j) == Some(&b'/') {
            j = regex_ws_skip(b, j + 1);
        }
        let after_slash = j;
        // 命名空间可选组（只有这一支，无 DSML 管道）的回溯：先带后不带
        let mut cands = Vec::with_capacity(2);
        if let Some(e) = ns_prefix(b, after_slash) {
            cands.push(e);
        }
        cands.push(after_slash);
        for cand in cands {
            // 名字按正则的左支顺序试；tool_calls 的 s? 贪心 = 长形先试
            for name in ["invoke", "parameter", "function_calls", "tool_calls", "tool_call"] {
                if let Some(k) = lit(b, cand, name.as_bytes()) {
                    if matches!(b.get(k), Some(&c) if c.is_ascii_alphanumeric() || c == b'_') {
                        continue; // \b 失败
                    }
                    let mut m = k;
                    while !matches!(b.get(m), None | Some(b'<') | Some(b'>')) {
                        m += 1;
                    }
                    if b.get(m) == Some(&b'>') {
                        return Some(m + 1);
                    }
                }
            }
        }
    }
    None
}

/// `textCallJunk.ReplaceAllString(s, "")` 的等价。
fn strip_junk(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < b.len() {
        if let Some(n) = junk_match_at(b, i) {
            i += n;
            continue;
        }
        let mut j = i + 1;
        while j < b.len() && b[j] & 0xC0 == 0x80 {
            j += 1;
        }
        out.push_str(&s[i..j]);
        i = j;
    }
    out
}

// ---------------------------------------------------------------------------
// mini JSON（Go encoding/json 的语义子集；不引 serde_json——它会把整数写成 5.0、
// 键序看 feature；Go 经 float64 往返会改写 >2^53 的整数，这里按原文保留更保真）
// ---------------------------------------------------------------------------

/// JSON 嵌套深度上限（Go jsonMaxDepth = 10000；这里收紧到 500，超出按「读不出」
/// 处理 → 保持文本，方向安全）。
const JSON_DEPTH_MAX: usize = 500;

#[derive(Debug, Clone)]
enum JVal {
    Null,
    Bool(bool),
    Num(String),
    Str(String),
    Arr(Vec<JVal>),
    Obj(Vec<(String, JVal)>),
}

impl JVal {
    fn get(&self, key: &str) -> Option<&JVal> {
        if let JVal::Obj(m) = self {
            m.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
        } else {
            None
        }
    }

    fn as_str(&self) -> Option<&str> {
        if let JVal::Str(s) = self {
            Some(s)
        } else {
            None
        }
    }
}

fn json_ws(b: &[u8], mut i: usize) -> usize {
    while matches!(b.get(i), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        i += 1;
    }
    i
}

/// 严格整串解析（Go `json.Unmarshal` 语义：首尾空白可、一个值、到尾）。
fn parse_json_strict(s: &str) -> Option<JVal> {
    let b = s.as_bytes();
    let i = json_ws(b, 0);
    let (v, end) = parse_json_value(b, i, 0)?;
    if json_ws(b, end) == b.len() {
        Some(v)
    } else {
        None
    }
}

/// 解析 s 开头的一个值，返回它结束的字节位置（Go `json.Decoder.Decode` +
/// `InputOffset`：后面还有什么是调用方的事，textcalls.go:494-516）。
fn parse_json_one(s: &str) -> Option<(JVal, usize)> {
    let b = s.as_bytes();
    let i = json_ws(b, 0);
    parse_json_value(b, i, 0)
}

fn parse_json_value(b: &[u8], i: usize, depth: usize) -> Option<(JVal, usize)> {
    if depth > JSON_DEPTH_MAX {
        return None;
    }
    match *b.get(i)? {
        b'{' => {
            let mut entries: Vec<(String, JVal)> = Vec::new();
            let mut i = json_ws(b, i + 1);
            if b.get(i) == Some(&b'}') {
                return Some((JVal::Obj(entries), i + 1));
            }
            loop {
                let (key, j) = parse_json_string(b, i)?;
                let j = json_ws(b, j);
                let j = expect(b, j, b':')?;
                let j = json_ws(b, j);
                let (v, j) = parse_json_value(b, j, depth + 1)?;
                // 重复键后者覆盖（encoding/json 的 map 语义）
                if let Some(slot) = entries.iter_mut().find(|(k, _)| *k == key) {
                    slot.1 = v;
                } else {
                    entries.push((key, v));
                }
                let j = json_ws(b, j);
                match b.get(j) {
                    Some(b',') => i = json_ws(b, j + 1),
                    Some(b'}') => return Some((JVal::Obj(entries), j + 1)),
                    _ => return None,
                }
            }
        }
        b'[' => {
            let mut items: Vec<JVal> = Vec::new();
            let mut i = json_ws(b, i + 1);
            if b.get(i) == Some(&b']') {
                return Some((JVal::Arr(items), i + 1));
            }
            loop {
                let (v, j) = parse_json_value(b, i, depth + 1)?;
                items.push(v);
                let j = json_ws(b, j);
                match b.get(j) {
                    Some(b',') => i = json_ws(b, j + 1),
                    Some(b']') => return Some((JVal::Arr(items), j + 1)),
                    _ => return None,
                }
            }
        }
        b'"' => parse_json_string(b, i).map(|(s, e)| (JVal::Str(s), e)),
        b't' => lit(b, i, b"true").map(|e| (JVal::Bool(true), e)),
        b'f' => lit(b, i, b"false").map(|e| (JVal::Bool(false), e)),
        b'n' => lit(b, i, b"null").map(|e| (JVal::Null, e)),
        b'-' | b'0'..=b'9' => parse_json_number(b, i),
        _ => None,
    }
}

fn parse_json_string(b: &[u8], i: usize) -> Option<(String, usize)> {
    if b.get(i) != Some(&b'"') {
        return None;
    }
    let mut out = String::new();
    let mut i = i + 1;
    loop {
        let c = *b.get(i)?;
        match c {
            b'"' => return Some((out, i + 1)),
            b'\\' => {
                let e = *b.get(i + 1)?;
                match e {
                    b'"' => {
                        out.push('"');
                        i += 2;
                    }
                    b'\\' => {
                        out.push('\\');
                        i += 2;
                    }
                    b'/' => {
                        out.push('/');
                        i += 2;
                    }
                    b'b' => {
                        out.push('\u{8}');
                        i += 2;
                    }
                    b'f' => {
                        out.push('\u{c}');
                        i += 2;
                    }
                    b'n' => {
                        out.push('\n');
                        i += 2;
                    }
                    b'r' => {
                        out.push('\r');
                        i += 2;
                    }
                    b't' => {
                        out.push('\t');
                        i += 2;
                    }
                    b'u' => {
                        let hi = hex4(b, i + 2)?;
                        if (0xD800..0xDC00).contains(&hi) {
                            // 高代理：紧跟着合法的低代理转义才成对（Go utf16.DecodeRune）
                            if b.get(i + 6) == Some(&b'\\') && b.get(i + 7) == Some(&b'u') {
                                if let Some(lo) = hex4(b, i + 8) {
                                    if (0xDC00..0xE000).contains(&lo) {
                                        let cp = 0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00);
                                        out.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
                                        i += 12;
                                        continue;
                                    }
                                }
                            }
                            out.push('\u{fffd}');
                        } else if (0xDC00..0xE000).contains(&hi) {
                            out.push('\u{fffd}');
                        } else {
                            out.push(char::from_u32(hi).unwrap_or('\u{fffd}'));
                        }
                        i += 6;
                    }
                    _ => return None, // \' 等标准 JSON 不认（fastjson 的 unquoteJSON 认,
                                      // 但 any 目标最终走标准 json.Unmarshal——同口径）
                }
                continue;
            }
            c if c < 0x20 => return None, // 字符串里的裸控制字符
            c if c < 0x80 => {
                out.push(c as char);
                i += 1;
                continue;
            }
            _ => {
                let mut j = i + 1;
                while j < b.len() && b[j] & 0xC0 == 0x80 {
                    j += 1;
                }
                out.push_str(std::str::from_utf8(&b[i..j]).ok()?);
                i = j;
                continue;
            }
        }
    }
}

fn hex4(b: &[u8], i: usize) -> Option<u32> {
    let mut v = 0u32;
    for k in 0..4 {
        let c = *b.get(i + k)?;
        let d = match c {
            b'0'..=b'9' => (c - b'0') as u32,
            b'a'..=b'f' => (c - b'a' + 10) as u32,
            b'A'..=b'F' => (c - b'A' + 10) as u32,
            _ => return None,
        };
        v = v * 16 + d;
    }
    Some(v)
}

/// 数字按 Go validJSONNumber 的语法（-?(0|[1-9]\d*)(\.\d+)?([eE][+-]?\d+)?），
/// 原文保留。
fn parse_json_number(b: &[u8], i: usize) -> Option<(JVal, usize)> {
    let start = i;
    let mut j = i;
    if b.get(j) == Some(&b'-') {
        j += 1;
    }
    match b.get(j) {
        Some(b'0') => j += 1,
        Some(&c) if c.is_ascii_digit() => {
            while b.get(j).is_some_and(|&c| c.is_ascii_digit()) {
                j += 1;
            }
        }
        _ => return None,
    }
    if b.get(j) == Some(&b'.') {
        j += 1;
        if !b.get(j).is_some_and(|&c| c.is_ascii_digit()) {
            return None;
        }
        while b.get(j).is_some_and(|&c| c.is_ascii_digit()) {
            j += 1;
        }
    }
    if matches!(b.get(j), Some(b'e' | b'E')) {
        j += 1;
        if matches!(b.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        if !b.get(j).is_some_and(|&c| c.is_ascii_digit()) {
            return None;
        }
        while b.get(j).is_some_and(|&c| c.is_ascii_digit()) {
            j += 1;
        }
    }
    Some((JVal::Num(std::str::from_utf8(&b[start..j]).ok()?.to_string()), j))
}

/// Go `json.Marshal` 同款字符串转义：`"` `\\`、\b \f \n \r \t、其余 <0x20 走
/// `\u00xx`，且 `<` `>` `&` 与 U+2028/2029 也转义（Marshal 默认开 HTML 转义）。
fn marshal_str_into(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '&' => out.push_str("\\u0026"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u00{:02x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

/// Go `json.Marshal(map[string]any)` 同款：对象键排序（字节序）、无空白。
fn marshal_jval(v: &JVal) -> String {
    let mut out = String::new();
    marshal_into(v, &mut out);
    out
}

fn marshal_into(v: &JVal, out: &mut String) {
    match v {
        JVal::Null => out.push_str("null"),
        JVal::Bool(true) => out.push_str("true"),
        JVal::Bool(false) => out.push_str("false"),
        JVal::Num(n) => out.push_str(n),
        JVal::Str(s) => marshal_str_into(s, out),
        JVal::Arr(items) => {
            out.push('[');
            for (i, x) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                marshal_into(x, out);
            }
            out.push(']');
        }
        JVal::Obj(entries) => {
            let mut sorted: Vec<&(String, JVal)> = entries.iter().collect();
            sorted.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            out.push('{');
            for (i, (k, x)) in sorted.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                marshal_str_into(k, out);
                out.push(':');
                marshal_into(x, out);
            }
            out.push('}');
        }
    }
}

/// escapeRawControls（textcalls.go:707-735，1:1）：JSON 文本字符串里的裸
/// `\n` `\r` `\t`（文件内容原样写进 JSON 时会有）改写成它们的转义。grew 是
/// 「输出比输入长出一字节」的输出侧偏移表，`grew_back` 把输出侧偏移映射回输入侧。
fn escape_raw_controls(s: &str) -> (String, Vec<usize>) {
    let mut out: Vec<u8> = Vec::with_capacity(s.len());
    let mut grew: Vec<usize> = Vec::new();
    let mut in_str = false;
    let mut esc = false;
    for &c in s.as_bytes() {
        if esc {
            esc = false;
        } else if in_str && c == b'\\' {
            esc = true;
        } else if c == b'"' {
            in_str = !in_str;
        } else if in_str && matches!(c, b'\n' | b'\r' | b'\t') {
            match c {
                b'\n' => out.extend_from_slice(b"\\n"),
                b'\r' => out.extend_from_slice(b"\\r"),
                _ => out.extend_from_slice(b"\\t"),
            }
            grew.push(out.len());
            continue;
        }
        out.push(c);
    }
    // 逐字节搬运 + ASCII 转义替换,UTF-8 结构不受影响
    (String::from_utf8(out).unwrap_or_default(), grew)
}

fn grew_back(grew: &[usize], mut n: usize) -> usize {
    for &g in grew {
        if g <= n {
            n -= 1;
        }
    }
    n
}

/// callArgs（textcalls.go:683-702）：调用的参数成为一个 JSON 对象——给成字符串
/// 形式的 JSON 就读它，没有是 `{}`，别的形状读不出。
fn call_args(v: Option<&JVal>) -> Option<String> {
    match v {
        None => Some("{}".to_string()),
        Some(JVal::Null) => Some("{}".to_string()),
        Some(JVal::Str(s)) => {
            let (fixed, _) = escape_raw_controls(s.trim());
            match parse_json_strict(&fixed)? {
                o @ JVal::Obj(_) => Some(marshal_jval(&o)),
                _ => None,
            }
        }
        Some(o @ JVal::Obj(_)) => Some(marshal_jval(o)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 四种块解析器（paramCall / hermesCall / glmCall / dsmlCall）及其锚定正则的
// 手写等价（paramHead/paramOpen :537-539、glmName/glmPair :601-603、
// dsmlInvoke/dsmlParam/dsmlClose/dsmlString :118-121）
// ---------------------------------------------------------------------------

struct WrittenCall {
    name: String,
    args: String,
}

fn offered(names: Option<&HashSet<String>>, name: &str) -> bool {
    names.is_some_and(|n| n.contains(name))
}

/// paramHead：`^\s*\{\s*"name"\s*:\s*"([^"]+)"\s*\}?\s*>?`（:537）——DeepSeek
/// 半 Hermes 半自家模板的名字段（#917）：名字是 JSON，用 `}`、`>` 收或不收。
fn param_head(s: &str) -> Option<(String, usize)> {
    let b = s.as_bytes();
    let mut i = regex_ws_skip(b, 0);
    i = expect(b, i, b'{')?;
    i = regex_ws_skip(b, i);
    i = expect(b, i, b'"')?;
    i = lit(b, i, b"name")?;
    i = expect(b, i, b'"')?; // 键的闭合引号（"name"\s*: 的那半个）
    i = regex_ws_skip(b, i);
    i = expect(b, i, b':')?;
    i = regex_ws_skip(b, i);
    i = expect(b, i, b'"')?;
    let start = i;
    while matches!(b.get(i), Some(&c) if c != b'"') {
        i += 1;
    }
    if i == start || b.get(i) != Some(&b'"') {
        return None; // [^"]+ 至少一个字符,且必须闭合
    }
    let name = s[start..i].to_string();
    let mut i = regex_ws_skip(b, i + 1);
    if b.get(i) == Some(&b'}') {
        i = regex_ws_skip(b, i + 1);
    }
    if b.get(i) == Some(&b'>') {
        i += 1;
    }
    Some((name, i))
}

/// paramOpen：`^\s*<parameter(?:\s+name\s*=\s*"([^"]+)"|=([^\s>]+))\s*>`（:538）——
/// ` name="k"` 形或 Qwen 的 `=k` 形。
fn param_open(s: &str) -> Option<(String, usize)> {
    let b = s.as_bytes();
    let i = regex_ws_skip(b, 0);
    let after = lit(b, i, b"<parameter")?;
    // (a) \s+name\s*=\s*"([^"]+)"
    if is_regex_ws_at(b, after) {
        let j = regex_ws_skip(b, after);
        if let Some(j) = lit(b, j, b"name") {
            let j = regex_ws_skip(b, j);
            if let Some(j) = expect(b, j, b'=') {
                let j = regex_ws_skip(b, j);
                if let Some(j) = expect(b, j, b'"') {
                    let start = j;
                    let mut k = j;
                    while matches!(b.get(k), Some(&c) if c != b'"') {
                        k += 1;
                    }
                    if k > start && b.get(k) == Some(&b'"') {
                        let name = s[start..k].to_string();
                        let k = regex_ws_skip(b, k + 1);
                        if let Some(end) = expect(b, k, b'>') {
                            return Some((name, end));
                        }
                    }
                }
            }
        }
    }
    // (b) =([^\s>]+)
    let start = expect(b, after, b'=')?;
    let mut k = start;
    while b.get(k).is_some_and(|&c| !is_regex_ws_byte(c) && c != b'>') {
        k += 1;
    }
    if k == start {
        return None;
    }
    let name = s[start..k].to_string();
    let k = regex_ws_skip(b, k);
    let end = expect(b, k, b'>')?;
    Some((name, end))
}

/// dsmlInvoke（:118）：`<\s*(?:前缀)?invoke\s+name\s*=\s*"([^"]+)"\s*>` 的查找。
/// 返回 (start, end, name)。
fn dsml_invoke_find(s: &str) -> Option<(usize, usize, String)> {
    let b = s.as_bytes();
    let mut cur = 0usize;
    while let Some(pos) = next_lt(b, cur) {
        if let Some((end, ns, ne)) = dsml_invoke_at(b, pos) {
            return Some((pos, end, s[ns..ne].to_string()));
        }
        cur = pos + 1;
    }
    None
}

fn dsml_invoke_at(b: &[u8], i: usize) -> Option<(usize, usize, usize)> {
    if b.get(i) != Some(&b'<') {
        return None;
    }
    let after_ws = regex_ws_skip(b, i + 1);
    for cand in prefix_candidate_ends(b, after_ws) {
        // invoke \s+ name \s* = \s* " ([^"]+) " \s* >
        let Some(j) = lit(b, cand, b"invoke") else { continue };
        if !is_regex_ws_at(b, j) {
            continue;
        }
        let j = regex_ws_skip(b, j);
        let Some(j) = lit(b, j, b"name") else { continue };
        let j = regex_ws_skip(b, j);
        let Some(j) = expect(b, j, b'=') else { continue };
        let j = regex_ws_skip(b, j);
        let Some(j) = expect(b, j, b'"') else { continue };
        let start = j;
        let mut k = j;
        while matches!(b.get(k), Some(&c) if c != b'"') {
            k += 1;
        }
        if k == start || b.get(k) != Some(&b'"') {
            continue;
        }
        let k2 = regex_ws_skip(b, k + 1);
        if let Some(end) = expect(b, k2, b'>') {
            return Some((end, start, k));
        }
    }
    None
}

/// dsmlClose（:120）：`<\s*/\s*(?:前缀)?invoke\s*>` 的查找——返回 (start, end)。
fn dsml_close_find(s: &str) -> Option<(usize, usize)> {
    let b = s.as_bytes();
    let mut cur = 0usize;
    while let Some(pos) = next_lt(b, cur) {
        if let Some(end) = dsml_close_at(b, pos) {
            return Some((pos, end));
        }
        cur = pos + 1;
    }
    None
}

fn dsml_close_at(b: &[u8], i: usize) -> Option<usize> {
    if b.get(i) != Some(&b'<') {
        return None;
    }
    let j = regex_ws_skip(b, i + 1);
    let Some(j) = expect(b, j, b'/') else { return None };
    let j = regex_ws_skip(b, j);
    for cand in prefix_candidate_ends(b, j) {
        if let Some(k) = lit(b, cand, b"invoke") {
            let k = regex_ws_skip(b, k);
            if let Some(k) = expect(b, k, b'>') {
                return Some(k);
            }
        }
    }
    None
}

/// dsmlParam（:119，dotall）的非重叠全量查找：
/// `<\s*(?:前缀)?parameter\s+name\s*=\s*"([^"]+)"((?:\s+\w+\s*=\s*"[^"]*")*)\s*>(.*?)<闭合>`。
/// 值是惰性 `.*?`：跑到**第一个** parameter 闭合（`<\s*/?\s*(?:前缀)?parameter\s*>`）。
fn dsml_param_find_all(s: &str) -> Vec<(String, String, String)> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut cur = 0usize;
    while let Some(pos) = next_lt(b, cur) {
        let Some((open_end, ns, ne, a0, a1)) = dsml_param_at(b, pos) else {
            cur = pos + 1;
            continue;
        };
        let Some((cstart, cend)) = dsml_param_close_find(b, open_end) else {
            cur = pos + 1;
            continue;
        };
        out.push((s[ns..ne].to_string(), s[a0..a1].to_string(), s[open_end..cstart].to_string()));
        cur = cend;
    }
    out
}

fn dsml_param_at(b: &[u8], i: usize) -> Option<(usize, usize, usize, usize, usize)> {
    if b.get(i) != Some(&b'<') {
        return None;
    }
    let after_ws = regex_ws_skip(b, i + 1);
    for cand in prefix_candidate_ends(b, after_ws) {
        let Some(j) = lit(b, cand, b"parameter") else { continue };
        if !is_regex_ws_at(b, j) {
            continue;
        }
        let j = regex_ws_skip(b, j);
        let Some(j) = lit(b, j, b"name") else { continue };
        let j = regex_ws_skip(b, j);
        let Some(j) = expect(b, j, b'=') else { continue };
        let j = regex_ws_skip(b, j);
        let Some(j) = expect(b, j, b'"') else { continue };
        let start = j;
        let mut k = j;
        while matches!(b.get(k), Some(&c) if c != b'"') {
            k += 1;
        }
        if k == start || b.get(k) != Some(&b'"') {
            continue;
        }
        let name_end = k;
        let a0 = k + 1;
        let mut k = a0;
        // ((?:\s+\w+\s*=\s*"[^"]*")*) 贪心重复,单元失败即停（不回吐）
        while let Some(next) = attr_unit(b, k) {
            k = next;
        }
        let a1 = k;
        let k = regex_ws_skip(b, k);
        if let Some(end) = expect(b, k, b'>') {
            return Some((end, start, name_end, a0, a1));
        }
    }
    None
}

/// `\s+\w+\s*=\s*"[^"]*"` 一个属性单元。
fn attr_unit(b: &[u8], i: usize) -> Option<usize> {
    if !is_regex_ws_at(b, i) {
        return None;
    }
    let j = regex_ws_skip(b, i);
    if !matches!(b.get(j), Some(&c) if c.is_ascii_alphanumeric() || c == b'_') {
        return None;
    }
    let mut j = j;
    while b.get(j).is_some_and(|&c| c.is_ascii_alphanumeric() || c == b'_') {
        j += 1;
    }
    let j = regex_ws_skip(b, j);
    let j = expect(b, j, b'=')?;
    let j = regex_ws_skip(b, j);
    let j = expect(b, j, b'"')?;
    let mut j = j;
    while matches!(b.get(j), Some(&c) if c != b'"') {
        j += 1;
    }
    expect(b, j, b'"')
}

/// parameter 的闭合标签 `<\s*/?\s*(?:前缀)?parameter\s*>`（`/` 可省——DeepSeek
/// 的闭合常缺）。返回 (start, end)。
fn dsml_param_close_find(b: &[u8], from: usize) -> Option<(usize, usize)> {
    let mut cur = from;
    while let Some(pos) = next_lt(b, cur) {
        if let Some(end) = dsml_param_close_at(b, pos) {
            return Some((pos, end));
        }
        cur = pos + 1;
    }
    None
}

fn dsml_param_close_at(b: &[u8], i: usize) -> Option<usize> {
    if b.get(i) != Some(&b'<') {
        return None;
    }
    let mut j = regex_ws_skip(b, i + 1);
    if b.get(j) == Some(&b'/') {
        j = regex_ws_skip(b, j + 1);
    }
    for cand in prefix_candidate_ends(b, j) {
        if let Some(k) = lit(b, cand, b"parameter") {
            let k = regex_ws_skip(b, k);
            if let Some(k) = expect(b, k, b'>') {
                return Some(k);
            }
        }
    }
    None
}

/// dsmlString（:121）：`string\s*=\s*"true"`——参数开了它,值按原字符串收。
fn dsml_string_match(attrs: &str) -> bool {
    let b = attrs.as_bytes();
    let mut cur = 0usize;
    while let Some(pos) = find_lit(b, cur, b"string") {
        let j = regex_ws_skip(b, pos + 6);
        if let Some(k) = expect(b, j, b'=') {
            let k = regex_ws_skip(b, k);
            if b[k..].starts_with(b"\"true\"") {
                return true;
            }
        }
        cur = pos + 1;
    }
    false
}

/// glmName：`^\s*([^\s<>{}"]+)\s*`（:602）。
fn glm_name(s: &str) -> Option<(&str, usize)> {
    let b = s.as_bytes();
    let start = regex_ws_skip(b, 0);
    let mut j = start;
    while b
        .get(j)
        .is_some_and(|&c| !is_regex_ws_byte(c) && !matches!(c, b'<' | b'>' | b'{' | b'}' | b'"'))
    {
        j += 1;
    }
    if j == start {
        return None;
    }
    let name_end = j;
    let j = regex_ws_skip(b, j);
    Some((&s[start..name_end], j))
}

/// glmPair（:601，dotall）：`^\s*<arg_key>(.*?)</arg_key>\s*<arg_value>(.*?)</arg_value>`
/// ——惰性捕获 = 跑到第一个字面闭合。
fn glm_pair(s: &str) -> Option<(&str, &str, usize)> {
    let b = s.as_bytes();
    let i = regex_ws_skip(b, 0);
    let i = lit(b, i, b"<arg_key>")?;
    let close = find_lit(b, i, b"</arg_key>")?;
    let key = &s[i..close];
    let i = regex_ws_skip(b, close + "</arg_key>".len());
    let i = lit(b, i, b"<arg_value>")?;
    let vclose = find_lit(b, i, b"</arg_value>")?;
    Some((key, &s[i..vclose], vclose + "</arg_value>".len()))
}

/// 值「是 JSON 就按 JSON 收,否则按它说的字符串」（三处解析器同款）。
fn parse_or_string(val: &str) -> JVal {
    match parse_json_strict(val.trim()) {
        Some(v) => v,
        None => JVal::Str(val.to_string()),
    }
}

fn obj_set(obj: &mut Vec<(String, JVal)>, key: &str, v: JVal) {
    if let Some(slot) = obj.iter_mut().find(|(k, _)| k == key) {
        slot.1 = v;
    } else {
        obj.push((key.to_string(), v));
    }
}

/// hermesCall（textcalls.go:484-534）：读 `<tool_call>` 开的 JSON 对象。
/// 文件内容原样写进 JSON 字符串时的裸换行/制表先修成转义；对象后到闭合之间的
/// `}` `]` `"` `>` 残渣（GLM 留的，#1945）随闭合一起吃掉；对象之后紧跟着别的
/// 正文、后面又出现 `</tool_call>` 的，是对象在字符串里一个没转义的引号处提前
/// 收了（heredoc 命令被拦腰，#1945）——不是调用，免得收成截短的。
fn hermes_call(s: &str, names: Option<&HashSet<String>>) -> Option<(WrittenCall, usize)> {
    let lead = s.len() - trim_a_start(s).len();
    let body = &s[lead..];
    if !body.starts_with('{') {
        return None;
    }
    let (fixed, grew) = escape_raw_controls(body);
    let (v, end_fixed) = parse_json_one(&fixed)?;
    let JVal::Obj(_) = &v else { return None };
    let name = v.get("name")?.as_str()?.to_string();
    if !offered(names, &name) {
        return None;
    }
    let args_raw = v.get("arguments").or_else(|| v.get("parameters")).or_else(|| v.get("input"));
    let args = call_args(args_raw)?;
    let n = lead + grew_back(&grew, end_fixed);
    let rest = &s[n..];
    let trimmed = rest.trim_start_matches(|c| matches!(c, ' ' | '\t' | '\r' | '\n' | '}' | ']' | '"' | '\'' | '>'));
    let junk = rest.len() - trimmed.len();
    let next = &rest[junk..];
    if next.starts_with("</tool_call>") {
        return Some((WrittenCall { name: name.to_string(), args }, n + junk + "</tool_call>".len()));
    }
    if !trim_a_start(&rest[..junk]).is_empty() || (junk == 0 && !next.is_empty() && !next.starts_with('<')) {
        if next.contains("</tool_call>") {
            return None;
        }
    }
    Some((WrittenCall { name: name.to_string(), args }, n))
}

/// paramCall（textcalls.go:549-598）：DeepSeek 写的半 Hermes 半自家模板（#917）——
/// 名字是 JSON（`}`、`>` 收或不收），然后每个参数一条 `<parameter name="k">v
/// </parameter>`（或 Qwen 的 `<parameter=k>`），闭合常缺。值跑到闭合、下一个
/// parameter、`</tool_call>` 或末尾；是 JSON 按它收，否则按文本。
fn param_call(s: &str, names: Option<&HashSet<String>>) -> Option<(WrittenCall, usize)> {
    let (name, mut n) = param_head(s)?;
    if !offered(names, &name) {
        return None;
    }
    let mut obj: Vec<(String, JVal)> = Vec::new();
    while let Some((key, consumed)) = param_open(&s[n..]) {
        n += consumed;
        let mut end = s.len();
        let mut next = s.len();
        for stop in ["</parameter>", "<parameter", "</tool_call>", "<tool_call>"] {
            if let Some(i) = s[n..].find(stop) {
                let at = n + i;
                if at < end {
                    end = at;
                    next = at;
                    if stop == "</parameter>" {
                        next += stop.len();
                    }
                }
            }
        }
        let val = s[n..end].trim_matches(|c| c == '\r' || c == '\n');
        let val = strip_suffix_cutmark(val);
        let v = parse_or_string(val);
        obj_set(&mut obj, &key, v);
        n = next;
    }
    if obj.is_empty() {
        return None;
    }
    let t = trim_a_start(&s[n..]);
    if t.starts_with("</tool_call>") {
        n = s.len() - t.len() + "</tool_call>".len();
    }
    Some((WrittenCall { name, args: marshal_jval(&JVal::Obj(obj)) }, n))
}

/// glmCall（textcalls.go:611-645）：GLM 自家的调用（#906）——`<tool_call>` 后直接
/// 是工具名，然后每个参数 `<arg_key>…</arg_key><arg_value>…</arg_value>`，最后
/// `</tool_call>`。值是 JSON（数字、对象、列表、true）按它收，否则按字符串。
/// 名字后跟着别的东西（既不是闭合也不是下一个 `<tool_call>`）就不是 GLM 的。
fn glm_call(s: &str, names: Option<&HashSet<String>>) -> Option<(WrittenCall, usize)> {
    let (name, mut n) = glm_name(s).map(|(nm, end)| (nm.to_string(), end))?;
    if !offered(names, &name) {
        return None;
    }
    let mut obj: Vec<(String, JVal)> = Vec::new();
    while let Some((key, val, consumed)) = glm_pair(&s[n..]) {
        let v = parse_or_string(val);
        obj_set(&mut obj, key.trim(), v);
        n += consumed;
    }
    let t = trim_a_start(&s[n..]);
    if t.starts_with("</tool_call>") {
        n = s.len() - t.len() + "</tool_call>".len();
    } else if !t.is_empty() && !t.starts_with("<tool_call>") {
        return None;
    }
    Some((WrittenCall { name, args: marshal_jval(&JVal::Obj(obj)) }, n))
}

/// dsmlCall（textcalls.go:649-679）：DeepSeek 的 `<｜DSML｜invoke name="…">` 加
/// 参数，读到闭合、下一个 invoke 或 s 的末尾。开了 `string="true"` 的参数按原
/// 字符串收。
fn dsml_call(s: &str, name: &str, open: usize, names: Option<&HashSet<String>>) -> Option<(WrittenCall, usize)> {
    if !offered(names, name) {
        return None;
    }
    let mut end = s.len();
    if let Some((_, cend)) = dsml_close_find(&s[open..]) {
        end = open + cend;
    }
    if let Some((istart, _, _)) = dsml_invoke_find(&s[open..]) {
        if open + istart < end {
            end = open + istart;
        }
    }
    let mut obj: Vec<(String, JVal)> = Vec::new();
    for (pname, attrs, pvalue) in dsml_param_find_all(&s[open..end]) {
        let v = if dsml_string_match(&attrs) {
            JVal::Str(pvalue)
        } else {
            parse_or_string(&pvalue)
        };
        obj_set(&mut obj, &pname, v);
    }
    Some((WrittenCall { name: name.to_string(), args: marshal_jval(&JVal::Obj(obj)) }, end))
}

// ---------------------------------------------------------------------------
// parseTextCalls（textcalls.go:360-426）+ tagsOnly（:430-433）
// ---------------------------------------------------------------------------

/// 块与块之间的文本处理（parseTextCalls 的 text 闭包 :364-373）：已读出调用后，
/// 除了标签什么都没有才过得去——模型自编的「*Result: …*」及之后的一切都去
/// （调用真跑，模型读到真结果，packinggoodsllc_07668）。
fn text_between(t: &str, calls: usize, left: &mut String) -> bool {
    if calls > 0 {
        // Go: TrimSpace(textCallJunk.ReplaceAllString(t, "")) —— 先剥标签再 trim
        let stripped = strip_junk(t).trim().to_string();
        if !strip_suffix_cutmark(&stripped).trim().is_empty() {
            return false;
        }
        left.push_str(&stripped);
        return true;
    }
    left.push_str(t);
    true
}

/// parseTextCalls：读 s（以一个标记开头）里写下的调用。None = ok=false——除非
/// 其中每一块都是「点了 offered 工具名、参数读得出」的调用，否则整个不算；
/// rest 是其中不是调用也不是标签的文本（剥残渣后）。
fn parse_text_calls(s: &str, names: Option<&HashSet<String>>) -> Option<(Vec<WrittenCall>, String)> {
    let mut calls: Vec<WrittenCall> = Vec::new();
    let mut left = String::new();
    let mut s = s;
    while !s.is_empty() {
        let h = s.find("<tool_call>");
        let d = dsml_invoke_find(s);
        if let Some(hpos) = h.filter(|&h| d.as_ref().is_none_or(|(ds, _, _)| h < *ds)) {
            if !text_between(&s[..hpos], calls.len(), &mut left) {
                s = "";
                continue;
            }
            let after = trim_a_start(&s[hpos + "<tool_call>".len()..]);
            if after.is_empty() || after.starts_with('<') {
                // 后头没有 JSON：DeepSeek 自家的标签、一个 invoke 或闭合（#823），
                // 或什么都没有——开标签丢弃,后面的照旧读
                s = after;
                continue;
            }
            let body = &s[hpos + "<tool_call>".len()..];
            let parsed = param_call(body, names)
                .or_else(|| hermes_call(body, names))
                .or_else(|| {
                    if trim_a_start(body).starts_with('{') {
                        None
                    } else {
                        glm_call(body, names)
                    }
                });
            let Some((c, n)) = parsed else { return None };
            calls.push(c);
            s = &s[hpos + "<tool_call>".len() + n..];
        } else if let Some((dstart, dend, dname)) = d {
            if !text_between(&s[..dstart], calls.len(), &mut left) {
                s = "";
                continue;
            }
            let Some((c, n)) = dsml_call(&s[dstart..], &dname, dend - dstart, names) else { return None };
            calls.push(c);
            s = &s[dstart + n..];
        } else {
            text_between(s, calls.len(), &mut left);
            s = "";
        }
    }
    let rest = strip_junk(&left).trim().to_string();
    let rest = strip_suffix_cutmark(&rest).trim().to_string();
    // 只有标签、别无它物（#823 第二张截图：`<tool_call></｜DSML｜parameter>
    // </｜DSML｜invoke></` 就是一整条回复）也丢——给 agent 看到再读回,模型会把
    // 下一个调用也这么写
    if calls.is_empty() && !rest.is_empty() {
        return None;
    }
    Some((calls, rest))
}

/// tagsOnly（textcalls.go:430-433）：持有文本是不是只有调用的标签、别无它物。
fn tags_only(s: &str) -> bool {
    matches!(parse_text_calls(s, None), Some((calls, rest)) if calls.is_empty() && rest.is_empty())
}

// ---------------------------------------------------------------------------
// TextCallScout——Go textCallSee（textcalls.go:744-826）的流式持有者
// ---------------------------------------------------------------------------

/// 持有翻译后一条回复里「从书写调用开始的正文」，回合结束时交出它读出的调用，
/// 或没有调用时交出文本。接线层把正文 delta 喂给 `feed_text`，agent 自己发起
/// 工具时调 `note_native_call`，流错误调 `release_rest`，回合结束调 `finish`。
pub(crate) struct TextCallScout {
    /// 请求提供的工具名（None = 没提供 → 惰性直通，Go 在安装点就跳过，gateway.go:9644-9647）。
    names: Option<HashSet<String>>,
    pending: String,
    held: String,
    holding: bool,
    /// 回复自己发起了调用：正文不再读调用，但其中散落的标签仍要丢
    /// （上游把 DeepSeek 的 invoke 做成调用后留下的 `</｜｜DSML｜｜ calls>`）。
    native: bool,
}

/// `finish` 的产物：text = 该按文本放出的全部剩余（标记前的正文已在 feed 时放出，
/// 不在此列）；calls 非空 = 回合理应作为调过工具的回合结束（Go `ev.Stop = "tool"`）。
pub(crate) struct Finish {
    pub(crate) text: String,
    /// (工具名, 参数 JSON 串)
    pub(crate) calls: Vec<(String, String)>,
}

impl TextCallScout {
    /// offered_tools = 请求提供的工具名集合（textCallNames，textcalls.go:129-140：
    /// 空名跳过；整个为空 = 从不持有）。
    pub(crate) fn new(offered_tools: &[String]) -> TextCallScout {
        let names = if offered_tools.is_empty() {
            None
        } else {
            Some(offered_tools.iter().filter(|n| !n.is_empty()).cloned().collect())
        };
        TextCallScout { names, pending: String::new(), held: String::new(), holding: false, native: false }
    }

    /// 喂一片正文 delta，返回此刻可以放出的文本（可能为空——被持有了）。
    /// 标记未出现前的正文放行；**第一个标记出现后**正文开始持有（含跨 chunk 的
    /// 半截标记——chunk 尾的 `<tool_` 扣住等下一片，textCallAt 的 tail/more 语义）；
    /// 标记在 pending 与新片的接缝处补全时，**回溯释放**标记之前的那段正文。
    pub(crate) fn feed_text(&mut self, chunk: &str) -> String {
        if self.names.is_none() {
            return chunk.to_string();
        }
        let mut s = std::mem::take(&mut self.pending);
        s.push_str(chunk);
        if self.holding {
            self.held.push_str(&s);
            return String::new();
        }
        let (at, tail) = text_call_at(&s);
        let release = match at {
            Some(at) => {
                self.holding = true;
                let (before, from_mark) = s.split_at(at);
                self.held = from_mark.to_string();
                before.to_string()
            }
            None => {
                let cut = s.len() - tail;
                self.pending = s[cut..].to_string();
                s[..cut].to_string()
            }
        };
        self.keep_space(release)
    }

    /// 回复自己发起了工具调用（KToolStart，textcalls.go:776-778）：先按旧 native
    /// 值 settle（首个原生调用前 tagsOnly 仍生效），再置 native。返回 settle 放出
    /// 的文本。之后正文不再读调用。
    pub(crate) fn note_native_call(&mut self) -> String {
        let out = self.settle();
        self.native = true;
        out
    }

    /// 流错误路径（KError，textcalls.go:799-800）：持有的一律按文本放出，不再解析。
    pub(crate) fn release_rest(&mut self) -> String {
        let mut out = std::mem::take(&mut self.pending);
        out.push_str(&self.held);
        self.held.clear();
        self.holding = false;
        out
    }

    /// 回合结束（KStop，textcalls.go:779-798）：持有的文本按块解析——每一块都是
    /// 「点了 offered 工具、参数读得出」的调用 → calls；否则（或回复自己调用过）
    /// 持有文本按原文并入 text。
    pub(crate) fn finish(mut self) -> Finish {
        let mut text = String::new();
        let mut calls: Vec<(String, String)> = Vec::new();
        if self.native {
            text.push_str(&self.settle());
        }
        if self.holding && !self.native {
            if let Some((parsed, rest)) = parse_text_calls(&self.held, self.names.as_ref()) {
                self.holding = false;
                self.held.clear();
                if !rest.is_empty() {
                    text.push_str(&rest);
                }
                calls = parsed.into_iter().map(|c| (c.name, c.args)).collect();
            }
        }
        text.push_str(&self.release_rest());
        Finish { text, calls }
    }

    /// keepSpace（spaceBack，textcalls.go:438-446）：回复自己调用过之后，只有空白
    /// 的正文扣住（调用后标签前的换行跟着标签一起丢）。
    fn keep_space(&mut self, s: String) -> String {
        if !self.native || s.is_empty() || !s.trim().is_empty() {
            return s;
        }
        if self.holding {
            self.held.insert_str(0, &s);
        } else {
            self.pending.insert_str(0, &s);
        }
        String::new()
    }

    /// settle（textcalls.go:815-818 + settled :451-459）：持有的只有标签就丢
    /// （连 holding 一起解），原生调用之后只有空白的 pending/held 也丢，剩下的
    /// 按文本放出。
    fn settle(&mut self) -> String {
        if self.holding && tags_only(&self.held) {
            self.held.clear();
            self.holding = false;
        }
        if self.native && self.pending.trim().is_empty() && self.held.trim().is_empty() {
            self.pending.clear();
            self.held.clear();
            self.holding = false;
        }
        self.release_rest()
    }
}

// ---------------------------------------------------------------------------
// 测试（对照 Go 的 textcalls_*_test.go 关键用例；用例出处逐条注明）
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn scout(tools: &[&str]) -> TextCallScout {
        TextCallScout::new(&tools.iter().map(|t| t.to_string()).collect::<Vec<_>>())
    }

    fn feed_all(sc: &mut TextCallScout, chunks: &[&str]) -> String {
        let mut out = String::new();
        for c in chunks {
            out.push_str(&sc.feed_text(c));
        }
        out
    }

    fn nameset(tools: &[&str]) -> Option<HashSet<String>> {
        Some(tools.iter().map(|t| t.to_string()).collect())
    }

    /// Go TestTextWrittenCallRelayed（textcalls_test.go:86-98）：Hermes 块、标签跨
    /// chunk，标记前正文放行。
    #[test]
    fn hermes_call_from_split_chunks() {
        let mut sc = scout(&["bash", "edit"]);
        let released = feed_all(
            &mut sc,
            &[
                "我先看看目录。\n\n<tool",
                "_call>{\"name\":\"bash\",\"arguments\":{\"command\":\"ls -la\"}}",
                "</tool_call>",
            ],
        );
        assert_eq!(released, "我先看看目录。\n\n");
        let f = sc.finish();
        assert_eq!(f.text, "");
        assert_eq!(f.calls, vec![("bash".to_string(), "{\"command\":\"ls -la\"}".to_string())]);
    }

    /// Go TestTextWrittenCallLeftAsText（textcalls_test.go:119-144）：点名的工具没
    /// 提供 / 没提供任何工具 / 块被截断，都按原文本放出；长得像标记开头的文本整段放行。
    #[test]
    fn unoffered_and_cut_and_lookalike_stay_text() {
        let mut sc = scout(&["bash", "edit"]);
        let block = "<tool_call>{\"name\":\"rm_rf\",\"arguments\":{}}</tool_call>";
        let released = feed_all(&mut sc, &[&format!("Like this: {block}")]);
        assert_eq!(released, "Like this: ");
        let f = sc.finish();
        assert_eq!(f.text, block);
        assert!(f.calls.is_empty());

        // 没提供任何工具：惰性直通（Go 在安装点跳过，gateway.go:9644-9647）
        let mut sc = scout(&[]);
        let block = "<tool_call>{\"name\":\"bash\",\"arguments\":{\"command\":\"ls\"}}</tool_call>";
        assert_eq!(sc.feed_text(block), block);
        assert_eq!(sc.finish().text, "");

        // 块被截断（Go cut 用例）
        let mut sc = scout(&["bash", "edit"]);
        let cut = "<tool_call>{\"name\":\"bash\",\"arguments\":{\"command\":\"l";
        assert_eq!(feed_all(&mut sc, &[cut, "s"]), "");
        let f = sc.finish();
        assert_eq!(f.text, format!("{cut}s"));
        assert!(f.calls.is_empty());

        // 长得像标记开头（Go lookalike 用例）
        let mut sc = scout(&["bash", "edit"]);
        assert_eq!(feed_all(&mut sc, &["a <tool", "tip> b"]), "a <tooltip> b");
        assert_eq!(sc.finish().text, "");
    }

    /// 回复以半截标记结尾：pending 在 finish 时按文本放出（textCallAt 的 tail）。
    #[test]
    fn partial_mark_at_end_released_at_finish() {
        let mut sc = scout(&["bash"]);
        assert_eq!(feed_all(&mut sc, &["x <tool"]), "x ");
        let f = sc.finish();
        assert_eq!(f.text, "<tool");
        assert!(f.calls.is_empty());
    }

    /// Go TestTextWrittenCallTailDropped（textcalls_tail_test.go:12-30）：对象与
    /// `</tool_call>` 之间 GLM 留的残渣尾巴全吃掉；字符串形式的参数按 JSON 读
    /// （#823 第二张截图的 bash/edit 两调用，键排序）。
    #[test]
    fn hermes_junk_tails_and_string_args() {
        for tail in ["]", "}}", "\">", "\n}}", "]}]}}\"}}}}"] {
            let mut sc = scout(&["bash", "edit"]);
            let released = feed_all(
                &mut sc,
                &[
                    "Listing it.\n<tool_call>",
                    &format!("{{\"name\":\"bash\",\"arguments\":{{\"command\":\"ls\"}}}}{tail}"),
                    "</tool_call>",
                ],
            );
            assert_eq!(released, "Listing it.\n", "tail {tail:?}");
            let f = sc.finish();
            assert_eq!(f.calls, vec![("bash".to_string(), "{\"command\":\"ls\"}".to_string())], "tail {tail:?}");
            assert_eq!(f.text, "");
        }
        let mut sc = scout(&["bash", "edit"]);
        feed_all(
            &mut sc,
            &[
                "<tool_call>{\"name\":\"bash\",\"arguments\":{\"command\":\"cd /e && for s in a b; do\n\techo \\\"$s\\\"; done\"}}</｜DSML｜parameter>\n</｜DSML｜invoke",
                "<tool_call>{\"name\":\"edit\",\"arguments\":\"{\\\"path\\\":\\\"a.md\\\",\\\"oldText\\\":\\\"x\\\"}\"}</tool_call>",
            ],
        );
        let f = sc.finish();
        assert_eq!(
            f.calls,
            vec![
                ("bash".to_string(), "{\"command\":\"cd /e \\u0026\\u0026 for s in a b; do\\n\\techo \\\"$s\\\"; done\"}".to_string()),
                ("edit".to_string(), "{\"oldText\":\"x\",\"path\":\"a.md\"}".to_string()),
            ]
        );
    }

    /// Go TestTextWrittenCallCutEarlyNotCalled（textcalls_tail_test.go:36-53）：heredoc
    /// 的引号没转义，JSON 对象在命令中间一个引号处提前收了——这样的块不是调用，
    /// 文本按原样放出去（参数截短的比没有差）。后半：没有自己闭合的调用之后的
    /// 散文被丢弃（packinggoodsllc_07668）。
    #[test]
    fn hermes_heredoc_cut_early_stays_text() {
        let mut sc = scout(&["bash", "edit"]);
        let block = "<tool_call>{\"name\":\"bash\",\"arguments\":{\"command\":\"python3 - <<'PY'\\nimport json\\nprint(json.dumps({\\\"名\\\": 'v', \\\"n\\\": {\\\"x\\\": \\\"y\"}}))\\nPY\\n | tail -8\",\"timeout\":180000}}</tool_call>";
        assert_eq!(feed_all(&mut sc, &[block]), "");
        let f = sc.finish();
        assert_eq!(f.text, block);
        assert!(f.calls.is_empty());

        let mut sc = scout(&["bash", "edit"]);
        feed_all(&mut sc, &["<tool_call>{\"name\":\"bash\",\"arguments\":{\"command\":\"ls\"}}\nDone."]);
        let f = sc.finish();
        assert_eq!(f.calls, vec![("bash".to_string(), "{\"command\":\"ls\"}".to_string())]);
        assert!(!f.text.contains("Done."));
    }

    /// Go TestTextWrittenTagsOnly（textcalls_test.go:196-213）：只有标签、别无它物
    /// 的持有文本（含结尾截断的 `</`）在结束时丢弃；`<tool_call>` 包着 DeepSeek 自家
    /// invoke 的，是那个调用。
    #[test]
    fn tags_only_dropped_and_wrapped_invoke() {
        let mut sc = scout(&["bash", "edit"]);
        assert_eq!(
            feed_all(&mut sc, &["<tool_call>\n</｜DSML｜parameter>\n", "</｜DSML｜invoke>\n</"]),
            ""
        );
        let f = sc.finish();
        assert_eq!(f.text, "");
        assert!(f.calls.is_empty());

        let mut sc = scout(&["bash", "edit"]);
        let released = feed_all(
            &mut sc,
            &["看一下。<tool_call>\n<｜DSML｜invoke name=\"bash\">\n<｜DSML｜parameter name=\"command\" string=\"true\">ls</｜DSML｜parameter>\n</｜DSML｜invoke>\n</tool_call>"],
        );
        assert_eq!(released, "看一下。");
        let f = sc.finish();
        assert_eq!(f.calls, vec![("bash".to_string(), "{\"command\":\"ls\"}".to_string())]);
        assert_eq!(f.text, "");
    }

    /// Go TestTextWrittenDSMLCallTranslated（textcalls_test.go:161-190）+
    /// TestTextWrittenDSMLAnyPipes（textcalls_pipes_test.go:95-103）的若干用例。
    #[test]
    fn dsml_invoke_with_string_and_json_params() {
        let mut sc = scout(&["bash"]);
        let released = feed_all(
            &mut sc,
            &[
                "Running it.\n\n<｜DSML｜function_calls>\n<｜DSML｜invoke name=\"bash\">\n<｜DSML｜parameter name=\"command\" string=\"true\">ls -la</｜DSML｜parameter>\n",
                "<｜DSML｜parameter name=\"timeout\" string=\"false\">30</｜DSML｜parameter>\n</｜DSML｜invoke>\n</｜DSML｜function_calls>",
            ],
        );
        assert_eq!(released, "Running it.\n\n");
        let f = sc.finish();
        assert_eq!(f.calls, vec![("bash".to_string(), "{\"command\":\"ls -la\",\"timeout\":30}".to_string())]);
        assert_eq!(f.text, "");

        // 无参数 → {}（Go "no parameters"）；截断的参数读不进 → 也 {}（Go "cut short"）
        let mut sc = scout(&["get_weather"]);
        feed_all(&mut sc, &["<｜DSML｜invoke name=\"get_weather\"></｜DSML｜invoke>"]);
        assert_eq!(sc.finish().calls, vec![("get_weather".to_string(), "{}".to_string())]);
        let mut sc = scout(&["get_weather"]);
        feed_all(&mut sc, &["<｜DSML｜invoke name=\"get_weather\"><｜DSML｜parameter name=\"city\" string=\"true\">Beij"]);
        assert_eq!(sc.finish().calls, vec![("get_weather".to_string(), "{}".to_string())]);
    }

    /// Go TestTextWrittenDSMLDoublePipe（textcalls_pipes_test.go:29-79）：全角双管道
    /// 的两种写法（issue 的复现与实际观察到的回复）。
    #[test]
    fn dsml_fullwidth_double_pipe() {
        let issue = "<｜｜DSML｜｜invoke name=\"get_weather\"><｜｜DSML｜｜parameter name=\"city\" string=\"true\">Beijing</｜｜DSML｜｜parameter></｜｜DSML｜｜invoke>";
        let mut sc = scout(&["get_weather"]);
        assert_eq!(feed_all(&mut sc, &[issue]), "");
        assert_eq!(
            sc.finish().calls,
            vec![("get_weather".to_string(), "{\"city\":\"Beijing\"}".to_string())]
        );

        let observed = "<｜｜DSML｜｜ calls>\n<｜｜DSML｜｜ invoke name=\"web_search\">\n<｜｜DSML｜｜ parameter name=\"query\" string=\"true\">current Python version latest stable release</｜｜DSML｜｜ parameter>\n</｜｜DSML｜｜ invoke>\n</｜｜DSML｜｜ calls>";
        let mut sc = scout(&["web_search"]);
        assert_eq!(feed_all(&mut sc, &[observed]), "");
        let f = sc.finish();
        assert_eq!(
            f.calls,
            vec![("web_search".to_string(), "{\"query\":\"current Python version latest stable release\"}".to_string())]
        );
        assert_eq!(f.text, "");
    }

    /// Go TestTextWrittenDSMLMarkAsParsed（textcalls_pipes_test.go:153-176）：宽度在
    /// 标签内不一致、`<` 后与 `/` 周围的空格、DSML 后没管道、值以 `<` 结尾。
    #[test]
    fn dsml_mark_as_parsed() {
        for (reply, args) in [
            (
                "<｜|DSML|｜invoke name=\"get_weather\"><||DSML｜｜parameter name=\"city\" string=\"true\">Beijing</｜｜DSML||parameter></|DSML｜invoke>",
                "{\"city\":\"Beijing\"}",
            ),
            (
                "< ｜DSML｜invoke name=\"get_weather\">< ｜DSML｜parameter name=\"city\" string=\"true\">Beijing< / ｜DSML｜parameter></｜DSML｜invoke>",
                "{\"city\":\"Beijing\"}",
            ),
            (
                "<｜DSMLinvoke name=\"get_weather\"><｜DSMLparameter name=\"city\" string=\"true\">Beijing</｜DSMLparameter></｜DSMLinvoke>",
                "{\"city\":\"Beijing\"}",
            ),
            (
                "<｜DSML｜invoke name=\"get_weather\"><｜DSML｜parameter name=\"city\" string=\"true\">a < </｜DSML｜parameter></｜DSML｜invoke>",
                "{\"city\":\"a \\u003c \"}",
            ),
        ] {
            let mut sc = scout(&["get_weather"]);
            assert_eq!(feed_all(&mut sc, &[reply]), "", "{reply:?}");
            assert_eq!(sc.finish().calls, vec![("get_weather".to_string(), args.to_string())], "{reply:?}");
        }
        // 点名没提供的工具：整段按文本（Go "a tool not offered"）
        let unoffered = "<｜DSML｜ invoke name=\"web_search\">\n<｜DSML｜ parameter name=\"query\">x</｜DSML｜ parameter>\n</｜DSML｜ invoke>";
        let mut sc = scout(&["get_weather"]);
        assert_eq!(feed_all(&mut sc, &[unoffered]), "");
        assert_eq!(sc.finish().text, unoffered);
    }

    /// Go TestTextWrittenDSMLDoublePipeEveryCut（textcalls_pipes_test.go:125-146）
    /// 的流式半边：正文在任意字符边界切成两片，结果一致。
    #[test]
    fn dsml_every_char_cut_streaming() {
        let issue = "<｜｜DSML｜｜invoke name=\"get_weather\"><｜｜DSML｜｜parameter name=\"city\" string=\"true\">Beijing</｜｜DSML｜｜parameter></｜｜DSML｜｜invoke>";
        let s = format!("Sure.\n{issue}");
        let cuts: Vec<usize> = s
            .char_indices()
            .map(|(i, _)| i)
            .skip(1)
            .chain(std::iter::once(s.len()))
            .collect();
        for i in cuts {
            let mut sc = scout(&["get_weather"]);
            let released = feed_all(&mut sc, &[&s[..i], &s[i..]]);
            assert_eq!(released, "Sure.\n", "cut at {i}");
            let f = sc.finish();
            assert_eq!(f.calls, vec![("get_weather".to_string(), "{\"city\":\"Beijing\"}".to_string())], "cut at {i}");
            assert_eq!(f.text, "", "cut at {i}");
        }
    }

    /// 同上测试的扫描器半边：textCallAt 对「正在写」的 DSML 标记前缀扣住 tail
    /// （字符边界上的每种截法）。
    #[test]
    fn text_call_at_holds_partial_dsml() {
        let issue = "<｜｜DSML｜｜invoke name=\"get_weather\">";
        let head = &issue[..issue.find("invoke").unwrap()];
        for (i, _) in head.char_indices() {
            let s = format!("x{}", &head[..i]);
            let (at, tail) = text_call_at(&s);
            assert_eq!((at, tail), (None, i), "{s:?}");
        }
        let (at, _) = text_call_at(&format!("x{issue}"));
        assert_eq!(at, Some(1));
    }

    /// Go TestGLMTextCallCustomTool（textcalls_glm_test.go:61-85）：自定义工具的
    /// GLM 调用，标签跨 chunk，值是多行原始文本。
    #[test]
    fn glm_call_custom_tool_multiline_value() {
        let glm_exec = "<tool_call>exec\n<arg_key>input</arg_key>\n<arg_value>const r = await tools.shell({cmd: \"ls\"});\nconsole.log(r);</arg_value>\n</tool_call>";
        let text = format!("Let me look.\n\n{glm_exec}");
        let mut sc = scout(&["exec"]);
        let released = feed_all(&mut sc, &[&text[..20], &text[20..31], &text[31..52], &text[52..]]);
        assert_eq!(released, "Let me look.\n\n");
        let f = sc.finish();
        assert_eq!(
            f.calls,
            vec![(
                "exec".to_string(),
                "{\"input\":\"const r = await tools.shell({cmd: \\\"ls\\\"});\\nconsole.log(r);\"}".to_string()
            )]
        );
        assert_eq!(f.text, "");
    }

    /// Go TestGLMTextCallFunctionTool（textcalls_glm_test.go:89-115）：函数工具的
    /// 参数——是 JSON 的值按 JSON 收（数字/列表），别的按字符串；连排两个调用都做；
    /// 键排序。加 TestGLMTextCallUnofferedLeftAsText（:141-147）。
    #[test]
    fn glm_call_function_tool_json_values() {
        let mut sc = scout(&["shell", "read"]);
        feed_all(
            &mut sc,
            &[
                "<tool_call>shell<arg_key>command</arg_key><arg_value>[\"ls\",\"-la\"]</arg_value><arg_key>timeout_ms</arg_key><arg_value>",
                "5000</arg_value><arg_key>workdir</arg_key><arg_value>/tmp/x</arg_value></tool_call>\n<tool_call>read\n<arg_key>path</arg_key>\n<arg_value>a.md</arg_value>\n</tool_call>",
            ],
        );
        let f = sc.finish();
        assert_eq!(
            f.calls,
            vec![
                ("shell".to_string(), "{\"command\":[\"ls\",\"-la\"],\"timeout_ms\":5000,\"workdir\":\"/tmp/x\"}".to_string()),
                ("read".to_string(), "{\"path\":\"a.md\"}".to_string()),
            ]
        );
        assert_eq!(f.text, "");

        let block = "<tool_call>rm_rf<arg_key>path</arg_key><arg_value>/</arg_value></tool_call>";
        let mut sc = scout(&["shell"]);
        assert_eq!(feed_all(&mut sc, &[&format!("Like this: {block}")]), "Like this: ");
        let f = sc.finish();
        assert_eq!(f.text, block);
        assert!(f.calls.is_empty());
    }

    /// Go TestTextWrittenParamCallRelayed（textcalls_param_test.go:13-49）：半 Hermes
    /// 半 DeepSeek 模板（#917）——名字 JSON 用 `>` 收、参数闭合缺失、回复到头；
    /// 闭合完的带 Qwen `=` 形参数、JSON 值与多行原始值；名字后没参数 / 工具没提供，
    /// 整块按文本。加 TestTextWrittenDSMLMarkAsParsed 的「值以 < 结尾」parameter 形。
    #[test]
    fn param_call_half_hermes() {
        let mut sc = scout(&["bash", "edit"]);
        let released = feed_all(
            &mut sc,
            &[
                "Let me run it.\n\n<tool_call>{\"name\":\"bash\">\n<param",
                "eter name=\"command\">cd /e/Project/synara && node .review-tmp\n",
            ],
        );
        assert_eq!(released, "Let me run it.\n\n");
        let f = sc.finish();
        assert_eq!(
            f.calls,
            vec![("bash".to_string(), "{\"command\":\"cd /e/Project/synara \\u0026\\u0026 node .review-tmp\"}".to_string())]
        );

        let mut sc = scout(&["bash", "edit"]);
        feed_all(
            &mut sc,
            &[
                "<tool_call>{\"name\":\"edit\"}\n<parameter=path>a.ts</parameter>\n<parameter=edits>[{\"oldText\":\"x\",\"newText\":\"y\"}]</parameter>\n</tool_call>",
                "<tool_call>{\"name\":\"bash\"}<parameter name=\"command\">for f in *; do\n  echo \"$f\"\ndone</parameter></tool_call>",
            ],
        );
        let f = sc.finish();
        assert_eq!(f.text, "");
        assert_eq!(
            f.calls,
            vec![
                ("edit".to_string(), "{\"edits\":[{\"newText\":\"y\",\"oldText\":\"x\"}],\"path\":\"a.ts\"}".to_string()),
                ("bash".to_string(), "{\"command\":\"for f in *; do\\n  echo \\\"$f\\\"\\ndone\"}".to_string()),
            ]
        );

        let mut sc = scout(&["get_weather"]);
        feed_all(&mut sc, &["<tool_call>{\"name\":\"get_weather\">\n<parameter name=\"city\">a < "]);
        assert_eq!(
            sc.finish().calls,
            vec![("get_weather".to_string(), "{\"city\":\"a \\u003c \"}".to_string())]
        );

        for block in [
            "<tool_call>{\"name\":\"rm_rf\">\n<parameter name=\"path\">/</parameter>",
            "<tool_call>{\"name\":\"bash\"> and then",
        ] {
            let mut sc = scout(&["bash", "edit"]);
            assert_eq!(feed_all(&mut sc, &[block]), "");
            let f = sc.finish();
            assert_eq!(f.text, block);
            assert!(f.calls.is_empty(), "{block}");
        }
    }

    /// Go TestInvokeReportedParsed（textcalls_invoke_test.go:49-84）：Claude 形的裸
    /// invoke（packinggoodsllc_07668 的原话）——第一个调用是真调用，模型自编的结果
    /// 之后的一切都去；连排的调用都做；MiniMax / Claude 的包裹层；没提供的工具按文本。
    #[test]
    fn invoke_reported_parse() {
        let reported = concat!(
            "<invoke name=\"ToolSearch\">\n",
            "<parameter name=\"query\">select:mcp__magpie__magpie-browser__browser_navigate</parameter>\n",
            "</invoke>\n",
            "*Result: Tool loaded.*\n",
            "<invoke name=\"mcp__magpie__magpie-browser__browser_navigate\">\n",
            "<parameter name=\"url\">https://www.google.com</parameter>\n",
            "</invoke>\n",
            "*Result: Navigated to https://www.google.com — page title: \"Google\"*\n",
            "已用 magpie 的浏览器打开了谷歌 ..."
        );
        let want = ("ToolSearch".to_string(), "{\"query\":\"select:mcp__magpie__magpie-browser__browser_navigate\"}".to_string());
        let (calls, rest) = parse_text_calls(reported, nameset(&["ToolSearch", "Bash"]).as_ref()).unwrap();
        assert_eq!(calls.iter().map(|c| (c.name.clone(), c.args.clone())).collect::<Vec<_>>(), vec![want.clone()]);
        assert_eq!(rest, "");
        // 浏览器工具也提供了：仍只有第一个——第二个跟在自编的结果后面
        let (calls, _) = parse_text_calls(
            reported,
            nameset(&["ToolSearch", "Bash", "mcp__magpie__magpie-browser__browser_navigate"]).as_ref(),
        )
        .unwrap();
        assert_eq!(calls.len(), 1);

        let two = "<invoke name=\"Bash\">\n<parameter name=\"command\">ls</parameter>\n</invoke>\n<invoke name=\"Bash\">\n<parameter name=\"command\">pwd</parameter>\n</invoke>";
        let (calls, rest) = parse_text_calls(two, nameset(&["Bash"]).as_ref()).unwrap();
        assert_eq!(
            calls.iter().map(|c| c.args.clone()).collect::<Vec<_>>(),
            vec!["{\"command\":\"ls\"}", "{\"command\":\"pwd\"}"]
        );
        assert_eq!(rest, "");

        for (open, close) in [("<minimax:tool_call>", "</minimax:tool_call>"), ("<function_calls>", "</function_calls>")] {
            let s = format!("{open}\n<invoke name=\"Bash\">\n<parameter name=\"command\">ls -la</parameter>\n</invoke>\n{close}");
            let (at, _) = text_call_at(&format!("先看看。\n{s}"));
            assert_eq!(at, Some("先看看。\n".len()), "{open}");
            let (calls, rest) = parse_text_calls(&s, nameset(&["Bash"]).as_ref()).unwrap();
            assert_eq!(calls.iter().map(|c| c.args.clone()).collect::<Vec<_>>(), vec!["{\"command\":\"ls -la\"}"], "{open}");
            assert_eq!(rest, "");
        }

        assert!(parse_text_calls(
            "<invoke name=\"rm_rf\">\n<parameter name=\"p\">/</parameter>\n</invoke>",
            nameset(&["Bash"]).as_ref()
        )
        .is_none());
    }

    /// Go TestInvokeReportedRelayed（textcalls_invoke_test.go:88-98）：整条报告的
    /// 回复按 ~7 字节一片流式喂入——调用做出来，自编文本全去。
    #[test]
    fn invoke_reported_streamed_in_pieces() {
        let reported = concat!(
            "<invoke name=\"ToolSearch\">\n",
            "<parameter name=\"query\">select:mcp__magpie__magpie-browser__browser_navigate</parameter>\n",
            "</invoke>\n",
            "*Result: Tool loaded.*\n",
            "<invoke name=\"mcp__magpie__magpie-browser__browser_navigate\">\n",
            "<parameter name=\"url\">https://www.google.com</parameter>\n",
            "</invoke>\n",
            "*Result: Navigated to https://www.google.com — page title: \"Google\"*\n",
            "已用 magpie 的浏览器打开了谷歌 ..."
        );
        let mut sc = scout(&["ToolSearch"]);
        let mut released = String::new();
        let mut piece = String::new();
        for ch in reported.chars() {
            piece.push(ch);
            if piece.len() >= 7 {
                released.push_str(&sc.feed_text(&piece));
                piece.clear();
            }
        }
        if !piece.is_empty() {
            released.push_str(&sc.feed_text(&piece));
        }
        assert_eq!(released, "");
        let f = sc.finish();
        assert_eq!(
            f.calls,
            vec![("ToolSearch".to_string(), "{\"query\":\"select:mcp__magpie__magpie-browser__browser_navigate\"}".to_string())]
        );
        assert_eq!(f.text, "");
    }

    /// Go TestTextWrittenCallBesideNative（textcalls_test.go:148-156）：回复自己
    /// 发起了调用（note_native_call）——持有中的正文立即按文本放出，之后正文不再
    /// 读调用；native 之后只剩标签的持有文本在结束时丢弃（Go settle/native 语义）。
    #[test]
    fn native_call_releases_held_and_later_tags_drop() {
        let mut sc = scout(&["bash", "edit"]);
        let held = "<tool_call>{\"name\":\"bash\",\"arguments\":{\"command\":\"ls\"}}";
        assert_eq!(feed_all(&mut sc, &[held]), "");
        let out = sc.note_native_call();
        assert_eq!(out, held);
        assert_eq!(sc.feed_text(" tail"), " tail");
        let f = sc.finish();
        assert_eq!(f.text, "");
        assert!(f.calls.is_empty());

        let mut sc = scout(&["bash"]);
        sc.note_native_call();
        assert_eq!(sc.feed_text("</｜｜DSML｜｜ calls>"), "");
        let f = sc.finish();
        assert_eq!(f.text, "");
        assert!(f.calls.is_empty());
    }

    /// 多块混合里有一块解析不出（没提供的工具/坏参数）→ **整段**持有文本按原文
    /// 放出（Go parseTextCalls 的 all-or-nothing，文件头 "Held text that isn't all
    /// such blocks goes on as the text it was"）。
    #[test]
    fn mixed_blocks_all_or_nothing() {
        let mut sc = scout(&["bash", "edit"]);
        let text = "<tool_call>{\"name\":\"bash\",\"arguments\":{\"command\":\"ls\"}}</tool_call>\n<tool_call>{\"name\":\"nope\",\"arguments\":{}}</tool_call>";
        assert_eq!(feed_all(&mut sc, &[text]), "");
        let f = sc.finish();
        assert_eq!(f.text, text);
        assert!(f.calls.is_empty());

        // 坏 JSON 的 Hermes 块同理（hermes 截断早收）
        let mut sc = scout(&["bash", "edit"]);
        let text = "<tool_call>{\"name\":\"bash\",\"arguments\":{\"command\":\"ls\"}}</tool_call>\n<tool_call>{\"name\":\"edit\",\"arguments\":{\"path\"</tool_call>";
        assert_eq!(feed_all(&mut sc, &[text]), "");
        let f = sc.finish();
        assert_eq!(f.text, text);
        assert!(f.calls.is_empty());
    }
}
