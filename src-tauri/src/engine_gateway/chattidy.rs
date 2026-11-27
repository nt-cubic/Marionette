//! chat→chat 直通流的修补层（Go `chattidy.go` + `grok_integral.go` 的
//! `intTidy` 段的 1:1 子集；批 3「流可靠性」N4/N7，合同 D10）。
//!
//! Go 参照（行号为 third_party/magpie 内位置）：
//! - `grok_integral.go:168` `intTidy`——直通流里工具调用参数的零分数丢弃
//!   （`"yield_ms":2500.0` → `2500`；Codex 用 u64/usize 读参数，serde 对
//!   浮点整单拒绝，Go #1431/#3152）。`delta.tool_calls[].function.arguments`
//!   分片按 `(choice index, call index)` 各挂一个 [`WholeNums`] 状态机
//!   （integral.rs，批 2 入库），行内改写后重新序列化；行不完整（跨 chunk
//!   切断）缓冲到行尾再处理；解析不了的行原样放行——**改写失败绝不
//!   断流**。
//! - `chattidy.go:209` `openAIUsage`——厂商把缓存读数只写在别家名字里
//!   （Anthropic 的 `cache_read_input_tokens`/`cache_creation_input_tokens`、
//!   DeepSeek 的 `prompt_cache_hit_tokens`、Mistral 的 `num_cached_tokens`、
//!   Moonshot 顶层 `cached_tokens`）时补写 OpenAI 口径的
//!   `prompt_tokens_details.cached_tokens`（pi-ai 先读它）；`prompt_tokens`
//!   只数非缓存（Anthropic 口径）时重写成 OpenAI 的「含缓存」口径并调整
//!   `total_tokens`。已经是 OpenAI 口径的**原字节不动**。usage 帧缺
//!   `choices` 数组时补 `"choices":[]`（规整成标准
//!   `{"choices":[],"usage":{…}}` 尾帧——严格客户端对缺字段的
//!   chat.completion.chunk 会整帧拒绝）。
//! - `chattidy.go:418` `withoutEmptyNames`——分片里 `"name":""` 的重复空名
//!   字段剔掉（按「delta 带什么就覆盖什么」合并的客户端会把首片的真名
//!   盖成空，grok CLI 实锤 "Tool not found"）。
//! - `chattidy.go:1357` `joinSplitChatCalls` 的直通等价——同一帧
//!   `tool_calls` 数组里**同 index** 的多项合并成一个（厂商把同一条调用
//!   的分片拆成多个数组项；跨消息的合并属于请求侧，翻译路径的
//!   ChatToolAccum 已做拼装）。
//!
//! **不做**（与 Go 的刻意收敛，实锤驱动的修补先不上）：
//! - `chattidy.go:846` `withReasoningOnToolTurns`——依赖 DeepSeek 系语义
//!   判断（带工具调用的回合必须回放 reasoning_content），chat.rs 的
//!   `fit_for_vendor`（`replays_reasoning` + `NO_REASONING` 占位）在出站
//!   合身里已有同款近似，流侧不重复实现。
//! - `chatTidy` 的其余流修补（空 `reasoning_content` 剔除、
//!   `finish_reason:""`→null、空 `tool_calls:[]` 剔除、content parts 折平、
//!   流式 id/index 补齐、无名调用整链丢弃 #1475、finish_reason 翻转）——
//!   各自依赖逐厂商实锤，本批只做合同面（D7/D10）需要的件。
//! - Go `mayMendUsage` 的字节级预筛不做：needs 检查按 `"usage"` 子串触发，
//!   `mend_usage` 对已是 OpenAI 口径的 usage 只读几个字段就退出，足够便宜。

use std::collections::HashMap;
use std::io::Read;

use serde_json::{Map, Value};

use super::integral::{whole_args, WholeNums};

// ---------------------------------------------------------------------------
// 行级状态机（Go intTidy 的 write/flush/lines 三段）
// ---------------------------------------------------------------------------

/// chat 直通 SSE 流的逐行改写器。喂入任意切割的字节段，吐出可立即转发
/// 的字节；未完结的行缓冲在内部，流结束时 [`Chattidy::flush`] 冲掉。
pub(crate) struct Chattidy {
    /// 未到行尾的残段（跨 chunk 切断的行）。
    buf: Vec<u8>,
    /// 每条流式调用（按 `choice/index` 键）的零分数状态机（Go
    /// intTidy.calls）。
    machines: HashMap<String, WholeNums>,
    /// 每个键上最近一次见到的调用 id——同键 id 变了 = 新调用，重置状态机
    /// （防同 index 复用时的旧 `str_/zeros` 状态串进新调用的参数）。
    last_ids: HashMap<String, String>,
}

impl Chattidy {
    pub(crate) fn new() -> Chattidy {
        Chattidy {
            buf: Vec::new(),
            machines: HashMap::new(),
            last_ids: HashMap::new(),
        }
    }

    /// 喂入一段刚读到的字节，返回现在可以转发的字节（可能为空——整段都
    /// 落在未完结的行里）。
    pub(crate) fn write(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.buf.extend_from_slice(bytes);
        let Some(cut) = self.buf.iter().rposition(|&b| b == b'\n') else {
            return Vec::new();
        };
        let tail = self.buf.split_off(cut + 1);
        let whole = std::mem::replace(&mut self.buf, tail);
        self.lines(&whole)
    }

    /// 流结束：处理残缓冲里最后一行（可能没有行尾符）。
    pub(crate) fn flush(&mut self) -> Vec<u8> {
        let rest = std::mem::take(&mut self.buf);
        self.lines(&rest)
    }

    /// 逐行过改写；行与行之间的 `\n` 原样保留。
    fn lines(&mut self, bytes: &[u8]) -> Vec<u8> {
        if !bytes.iter().any(|&b| b == b'\n') && !line_carries(bytes) {
            // 快路径：整段没有任何可改写物（没有 \n 时也要看内容——最后
            // 一行可能是 flush 冲出来的 data 行）。
            return bytes.to_vec();
        }
        let mut out = Vec::with_capacity(bytes.len());
        let mut start = 0;
        for (i, &b) in bytes.iter().enumerate() {
            if b == b'\n' {
                out.extend_from_slice(&self.line(&bytes[start..=i]));
                start = i + 1;
            }
        }
        out.extend_from_slice(&self.line(&bytes[start..]));
        out
    }

    /// 改写一行 SSE。任何解析失败都原样放行（绝不因改写断流）。
    fn line(&mut self, line: &[u8]) -> Vec<u8> {
        if !line_carries(line) {
            return line.to_vec();
        }
        let body_len = line
            .iter()
            .rposition(|&b| b != b'\n' && b != b'\r')
            .map_or(0, |i| i + 1);
        let (body, tail) = line.split_at(body_len);
        let Some(data) = strip_data_prefix(body) else {
            return line.to_vec();
        };
        let Ok(mut value) = serde_json::from_str::<Value>(data) else {
            return line.to_vec(); // `data: [DONE]` 与坏行都走这里
        };
        let Some(obj) = value.as_object_mut() else {
            return line.to_vec();
        };
        let mut changed = false;

        // usage 修补（openAIUsage）+ 尾帧规整（缺 choices 补空数组）。
        if let Some(usage) = obj.get("usage") {
            if let Some(mended) = mend_usage(usage) {
                obj.insert("usage".into(), mended);
                changed = true;
            }
            let choices_ok = obj.get("choices").is_some_and(|c| c.is_array());
            if !choices_ok {
                obj.insert("choices".into(), Value::Array(Vec::new()));
                changed = true;
            }
        }

        // intTidy：choices[].delta（流式分片）与 choices[].message（整块）。
        if let Some(choices) = obj.get_mut("choices").and_then(|c| c.as_array_mut()) {
            for choice in choices.iter_mut() {
                let Some(cm) = choice.as_object_mut() else {
                    continue;
                };
                let choice_key = match cm.get("index").and_then(|v| v.as_i64()) {
                    Some(n) => n.to_string(),
                    None => "null".to_string(), // Go jsonText(nil)
                };
                let machines = &mut self.machines;
                let last_ids = &mut self.last_ids;
                if let Some(calls) = cm
                    .get_mut("delta")
                    .and_then(|d| d.as_object_mut())
                    .and_then(|d| d.get_mut("tool_calls"))
                    .and_then(|t| t.as_array_mut())
                {
                    if mend_delta_calls(calls, &choice_key, machines, last_ids) {
                        changed = true;
                    }
                }
                if let Some(calls) = cm
                    .get_mut("message")
                    .and_then(|m| m.as_object_mut())
                    .and_then(|m| m.get_mut("tool_calls"))
                    .and_then(|t| t.as_array_mut())
                {
                    if mend_whole_calls(calls) {
                        changed = true;
                    }
                }
            }
        }

        if !changed {
            return line.to_vec();
        }
        let Ok(json) = serde_json::to_vec(&value) else {
            return line.to_vec();
        };
        let mut out = Vec::with_capacity(5 + json.len() + tail.len());
        out.extend_from_slice(b"data: ");
        out.extend_from_slice(&json);
        out.extend_from_slice(tail);
        out
    }
}

/// 行里有没有本层会动的东西：`"tool_calls"`（整数化/合名/剔空名）或
/// `"usage"`（口径修补）。其余行字节原样过（Go needsTidy 的子集——我们
/// 只做这两类）。
fn line_carries(line: &[u8]) -> bool {
    has_sub(line, b"\"tool_calls\"") || has_sub(line, b"\"usage\"")
}

fn has_sub(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// `data:` 前缀剥掉（允许 `data:` 与 JSON 之间的空白）；不是 data 行
/// 返回 None。
fn strip_data_prefix(body: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(body).ok()?;
    let rest = text.trim_start();
    let data = rest.strip_prefix("data:")?;
    Some(data)
}

// ---------------------------------------------------------------------------
// tool_calls 改写（delta 与 message 两种形态）
// ---------------------------------------------------------------------------

/// 「空名工具」判定（任务口径）：带 `function` 对象、name 与 arguments
/// 都没有非空字符串——这种项跑了也是空转，还会让严格解析的客户端整帧
/// 拒绝。Go `nameless`（#1475，无名调用整链丢弃）**不**搬：直通路径上
/// 「首片无名 = 整条调用作废」的判定风险高，只剔完全空转的项。
fn call_junk(call: &Value) -> bool {
    let Some(f) = call.get("function").and_then(|f| f.as_object()) else {
        return false;
    };
    let name_said = f
        .get("name")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());
    let args_said = f
        .get("arguments")
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());
    !name_said && !args_said
}

/// 同帧同 index 合并（joinSplitChatCalls 的直通等价）：数组里显式 index
/// 相同的项并进首个——id/type 取先见非空，name 取先见非空，arguments
/// 按出现顺序拼接。没显式 index 的项不并（位置即身份，Go intTidy 同款）。
fn merge_same_index(calls: &mut Vec<Value>) -> bool {
    let mut first_at: HashMap<i64, usize> = HashMap::new();
    let mut out: Vec<Value> = Vec::with_capacity(calls.len());
    let mut merged = false;
    for call in calls.drain(..) {
        let Some(idx) = call.get("index").and_then(|v| v.as_i64()) else {
            out.push(call);
            continue;
        };
        match first_at.get(&idx) {
            Some(&at) => match out.get_mut(at).and_then(|t| t.as_object_mut()) {
                Some(target) => {
                    merge_call_into(target, call);
                    merged = true;
                }
                None => out.push(call),
            },
            None => {
                first_at.insert(idx, out.len());
                out.push(call);
            }
        }
    }
    // drain 已把 calls 倒空：out 里始终是全量（合并与否都按原顺序），无条件
    // 写回。
    *calls = out;
    merged
}

fn merge_call_into(target: &mut Map<String, Value>, donor: Value) {
    let Some(d) = donor.as_object() else { return };
    for key in ["id", "type"] {
        let target_empty = target
            .get(key)
            .map(|v| v.is_null() || v.as_str() == Some(""))
            .unwrap_or(true);
        if target_empty
            && d.get(key)
                .and_then(|v| v.as_str())
                .is_some_and(|s| !s.is_empty())
        {
            target.insert(key.into(), d.get(key).cloned().expect("checked"));
        }
    }
    let donor_fn = d.get("function").and_then(|f| f.as_object()).cloned();
    match (
        target.get_mut("function").and_then(|f| f.as_object_mut()),
        donor_fn,
    ) {
        (Some(tf), Some(df)) => {
            let tname_empty = tf
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .is_empty();
            if tname_empty
                && df
                    .get("name")
                    .and_then(|v| v.as_str())
                    .is_some_and(|s| !s.is_empty())
            {
                tf.insert("name".into(), df.get("name").cloned().expect("checked"));
            }
            let mut args = tf
                .get("arguments")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            args.push_str(df.get("arguments").and_then(|v| v.as_str()).unwrap_or(""));
            if tf.get("arguments").and_then(|v| v.as_str()) != Some(args.as_str()) {
                tf.insert("arguments".into(), Value::String(args));
            }
        }
        (None, Some(df)) => {
            target.insert("function".into(), Value::Object(df));
        }
        _ => {}
    }
}

/// delta 形态的 tool_calls 改写：同 index 合并 → 空名空参项剔除 →
/// `"name":""` 字段剔除 → arguments 过零分数状态机。
fn mend_delta_calls(
    calls: &mut Vec<Value>,
    choice_key: &str,
    machines: &mut HashMap<String, WholeNums>,
    last_ids: &mut HashMap<String, String>,
) -> bool {
    let mut changed = merge_same_index(calls);
    let before = calls.len();
    calls.retain(|c| !call_junk(c));
    if calls.len() != before {
        changed = true;
    }
    for (j, call) in calls.iter_mut().enumerate() {
        let Some(m) = call.as_object_mut() else {
            continue;
        };
        // withoutEmptyNames（chattidy.go:418）：`"name":""` 剔字段不剔项。
        if let Some(f) = m.get_mut("function").and_then(|f| f.as_object_mut()) {
            if f.get("name").and_then(|v| v.as_str()) == Some("") {
                f.remove("name");
                changed = true;
            }
        }
        // 状态机键（Go intTidy.chat:365）：显式 index → 它；没有 index →
        // 数组位置。
        let key = match m.get("index").and_then(|v| v.as_i64()) {
            Some(n) => format!("{choice_key}/{n}"),
            None => format!("{choice_key}/{j}"),
        };
        // 新调用首片重置：同键上 id 变了 = 新调用。
        let id_now = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if !id_now.is_empty() && last_ids.get(&key).map(String::as_str) != Some(id_now) {
            machines.remove(&key);
            last_ids.insert(key.clone(), id_now.to_string());
        }
        if let Some(args) = m.get_mut("function").and_then(|f| f.get_mut("arguments")) {
            if let Some(s) = args.as_str() {
                let machine = machines.entry(key).or_default();
                let fed = machine.feed(s);
                if &fed != s {
                    *args = Value::String(fed);
                    changed = true;
                }
            }
        }
    }
    changed
}

/// message 形态（整块应答/非流式）的 tool_calls 改写：参数整块、无状态。
fn mend_whole_calls(calls: &mut Vec<Value>) -> bool {
    let mut changed = false;
    let before = calls.len();
    calls.retain(|c| !call_junk(c));
    if calls.len() != before {
        changed = true;
    }
    for call in calls.iter_mut() {
        let Some(f) = call.get_mut("function").and_then(|f| f.as_object_mut()) else {
            continue;
        };
        if f.get("name").and_then(|v| v.as_str()) == Some("") {
            f.remove("name");
            changed = true;
        }
        if let Some(args) = f.get_mut("arguments") {
            if let Some(s) = args.as_str() {
                let a = whole_args(s);
                if &a != s {
                    *args = Value::String(a);
                    changed = true;
                }
            }
        }
    }
    changed
}

// ---------------------------------------------------------------------------
// usage 修补（Go chattidy.go:209 openAIUsage 1:1）
// ---------------------------------------------------------------------------

/// 把只在别家名字里说缓存的 usage 补写成 OpenAI 口径。已是 OpenAI 口径
/// 或没说缓存 → None（原样不动）。口径细则见文件头。
pub(crate) fn mend_usage(raw: &Value) -> Option<Value> {
    let obj = raw.as_object()?;
    let g = |k: &str| obj.get(k).and_then(|v| v.as_i64()).unwrap_or(0);
    let prompt_tokens = g("prompt_tokens");
    let total_tokens = g("total_tokens");
    let details = obj.get("prompt_tokens_details").and_then(|d| d.as_object());
    let mut cache_read = details
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let mut cache_write = details
        .and_then(|d| d.get("cache_write_tokens"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    if cache_read == 0 {
        // DeepSeek / Moonshot / Mistral 的名字（cUsage.usage 的读取顺序）。
        for k in [
            "prompt_cache_hit_tokens",
            "cached_tokens",
            "num_cached_tokens",
        ] {
            let n = g(k);
            if n != 0 {
                cache_read = n;
                break;
            }
        }
    }
    let mut whole = true; // prompt_tokens 含缓存（OpenAI 口径）
    if cache_read == 0 && cache_write == 0 {
        // Anthropic 的名字：prompt_tokens 只数非缓存。
        cache_read = g("cache_read_input_tokens");
        cache_write = g("cache_creation_input_tokens");
        whole = prompt_tokens >= cache_read + cache_write;
    }
    if cache_read == 0 && cache_write == 0 {
        return None; // 什么缓存都没说
    }
    let mut input = prompt_tokens;
    if whole {
        input = (prompt_tokens - cache_read - cache_write).max(0);
    }
    let in_total = input + cache_read + cache_write; // Go u.prompt()
    if let Some(d) = details {
        let d_read = d.get("cached_tokens").and_then(|v| v.as_i64()).unwrap_or(0);
        let d_write = d
            .get("cache_write_tokens")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if d_read == cache_read && d_write == cache_write && prompt_tokens == in_total {
            return None; // 已经是 OpenAI 口径
        }
    }
    let mut m = obj.clone();
    let mut det = details.cloned().unwrap_or_default();
    det.insert("cached_tokens".into(), Value::from(cache_read));
    if det.contains_key("cache_write_tokens") || cache_write > 0 {
        det.insert("cache_write_tokens".into(), Value::from(cache_write));
    }
    m.insert("prompt_tokens_details".into(), Value::Object(det));
    if prompt_tokens != in_total {
        let had_total = m.contains_key("total_tokens");
        m.insert("prompt_tokens".into(), Value::from(in_total));
        if had_total {
            m.insert(
                "total_tokens".into(),
                Value::from(total_tokens + in_total - prompt_tokens),
            );
        }
    }
    Some(Value::Object(m))
}

// ---------------------------------------------------------------------------
// 整块（非流式）应答修补
// ---------------------------------------------------------------------------

/// 非流式整块 chat 应答的修补：`choices[].message.tool_calls` 的零分数/
/// 空名/空项 + usage 口径。没改动时**原字节**返回。
pub(crate) fn mend_whole_reply(body: &[u8]) -> Vec<u8> {
    if !has_sub(body, b"\"tool_calls\"") && !has_sub(body, b"\"usage\"") {
        return body.to_vec();
    }
    let Ok(mut value) = serde_json::from_slice::<Value>(body) else {
        return body.to_vec();
    };
    let Some(obj) = value.as_object_mut() else {
        return body.to_vec();
    };
    let mut changed = false;
    if let Some(usage) = obj.get("usage") {
        if let Some(mended) = mend_usage(usage) {
            obj.insert("usage".into(), mended);
            changed = true;
        }
    }
    if let Some(choices) = obj.get_mut("choices").and_then(|c| c.as_array_mut()) {
        for choice in choices.iter_mut() {
            if let Some(calls) = choice
                .get_mut("message")
                .and_then(|m| m.as_object_mut())
                .and_then(|m| m.get_mut("tool_calls"))
                .and_then(|t| t.as_array_mut())
            {
                if mend_whole_calls(calls) {
                    changed = true;
                }
            }
        }
    }
    if !changed {
        return body.to_vec();
    }
    serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec())
}

// ---------------------------------------------------------------------------
// Read 适配器
// ---------------------------------------------------------------------------

/// 挂在直通流上的 [`Read`] 适配器：字节照转，data 行过 [`Chattidy`]
/// 改写（Go 把 chatTidy/intTidy 包在 relay writer 上；Rust 的写层只认
/// reader）。上游读错误不吞：原样上抛（改写层只保证「解析失败不断流」，
/// 传输层错误如实交给外层——chat.rs 的收尾守卫把它翻成显式 error 收尾）。
pub(crate) struct ChattidyReader<R: Read> {
    inner: R,
    tidy: Chattidy,
    pending: Vec<u8>,
    at: usize,
    inner_done: bool,
    /// 上游读错误：先把 flush 出来的残行发完，下一次 read 再上抛。
    held_err: Option<std::io::Error>,
}

impl<R: Read> ChattidyReader<R> {
    pub(crate) fn new(inner: R) -> ChattidyReader<R> {
        ChattidyReader {
            inner,
            tidy: Chattidy::new(),
            pending: Vec::new(),
            at: 0,
            inner_done: false,
            held_err: None,
        }
    }
}

impl<R: Read> Read for ChattidyReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.at < self.pending.len() {
                let n = buf.len().min(self.pending.len() - self.at);
                buf[..n].copy_from_slice(&self.pending[self.at..self.at + n]);
                self.at += n;
                return Ok(n);
            }
            if self.inner_done {
                return match self.held_err.take() {
                    Some(e) => Err(e),
                    None => Ok(0),
                };
            }
            let mut chunk = [0u8; 16 * 1024];
            match self.inner.read(&mut chunk) {
                Ok(0) => {
                    self.inner_done = true;
                    self.pending = self.tidy.flush();
                    self.at = 0;
                }
                Ok(n) => {
                    self.pending = self.tidy.write(&chunk[..n]);
                    self.at = 0;
                    // 可能整段都在未完结的行里（pending 空）→ 回圈再读。
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    self.inner_done = true;
                    self.pending = self.tidy.flush();
                    self.at = 0;
                    if self.pending.is_empty() {
                        return Err(e);
                    }
                    self.held_err = Some(e);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// 按 pieces 切段喂 Chattidy，拼回完整输出。
    fn fed_in_pieces(pieces: &[&[u8]]) -> Vec<u8> {
        let mut t = Chattidy::new();
        let mut out = Vec::new();
        for p in pieces {
            out.extend_from_slice(&t.write(p));
        }
        out.extend_from_slice(&t.flush());
        out
    }

    const TOOL_STREAM: &[u8] = concat!(
        "data: {\"id\":\"c1\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"city\\\":\\\"Pa\"}}]}}]}\n\n",
        "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"ris\\\",\\\"yield_ms\\\":2500.0}\"}}]}}]}\n\n",
        "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    )
    .as_bytes();

    /// 整段一次喂入的输出（各断言在此基础上做语义比较——改写行的 JSON 键序
    /// 不做承诺，未改写行必须字节不变）。
    fn mended_tool_stream() -> Vec<u8> {
        fed_in_pieces(&[TOOL_STREAM])
    }

    #[test]
    fn streamed_tool_arguments_shed_zero_fractions() {
        let out = mended_tool_stream();
        let text = String::from_utf8(out.clone()).unwrap();
        // 零分数被丢弃（改写发生）。
        assert!(
            text.contains("\\\"yield_ms\\\":2500}"),
            "zero fraction must be shed: {text}"
        );
        assert!(
            !text.contains("2500.0"),
            "no zero fraction may survive: {text}"
        );
        // 未改写的行字节不变：首行（含 tool_calls 但无可改写物）与 [DONE]。
        let first_line =
            TOOL_STREAM[..TOOL_STREAM.iter().position(|&b| b == b'\n').unwrap() + 2].to_vec();
        assert!(
            out.starts_with(&first_line),
            "an unchanged line must keep its bytes: {text}"
        );
        assert!(text.ends_with("data: [DONE]\n\n"));
        // 改写后的第二行仍是合法 data JSON，且参数等于拼接后的去分数形态。
        let l2 = text
            .lines()
            .filter(|l| l.starts_with("data: {"))
            .nth(1)
            .unwrap();
        let v: Value = serde_json::from_str(l2.trim_start_matches("data: ")).unwrap();
        assert_eq!(
            v["choices"][0]["delta"]["tool_calls"][0]["function"]["arguments"],
            "ris\",\"yield_ms\":2500}"
        );
    }

    #[test]
    fn chunk_cut_anywhere_gives_the_same_stream() {
        // 行中间/数字中间/一字节一切，结果与整段喂入一致（Go intTidy 的流
        // 不变量：分片加起来等于整块的改写）。
        let whole = mended_tool_stream();
        for cut in [7usize, 60, 200, 262, 271] {
            let pieces = [&TOOL_STREAM[..cut], &TOOL_STREAM[cut..]];
            assert_eq!(fed_in_pieces(&pieces), whole, "cut at {cut} differs");
        }
        let singles = TOOL_STREAM.to_vec();
        let byte_by_byte: Vec<&[u8]> = singles.chunks(1).collect();
        assert_eq!(fed_in_pieces(&byte_by_byte), whole);
    }

    #[test]
    fn plain_content_frames_pass_byte_for_byte() {
        let plain = concat!(
            "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"fixture-\"}}]}\n\n",
            "data: {\"id\":\"c1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"id\":\"c1\",\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n",
            "data: [DONE]\n\n",
        )
        .as_bytes();
        // usage 是 OpenAI 口径（无缓存）→ 原样。
        assert_eq!(fed_in_pieces(&[plain]), plain.to_vec());
    }

    #[test]
    fn new_call_with_new_id_resets_the_machine() {
        // 调用 a1 的参数在数字中间被掐（`8.` 的点已持有）；同 index 换新
        // 调用（id 变了）必须重置状态机，否则 a2 的首段 `0}` 会把 0 当成
        // 前一个分数的零吞掉。
        let l1 = "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"a1\",\"function\":{\"name\":\"f\",\"arguments\":\"{\\\"n\\\":8.\"}}]}}]}\n".as_bytes();
        let l2 = "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"a2\",\"function\":{\"name\":\"g\",\"arguments\":\"0}\"}}]}}]}\n".as_bytes();
        let out = fed_in_pieces(&[l1, l2]);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("\"arguments\":\"0}\""),
            "a2 fragment must start clean: {text}"
        );
    }

    #[test]
    fn same_frame_same_index_entries_merge() {
        let line = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[",
            "{\"index\":0,\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"f\",\"arguments\":\"{\\\"x\\\":\"}},",
            "{\"index\":0,\"function\":{\"arguments\":\"2500.0}\"}}]}}]}\n",
        )
        .as_bytes();
        let out = fed_in_pieces(&[line]);
        let text = String::from_utf8(out).unwrap();
        let v: Value = serde_json::from_str(text.trim_start_matches("data: ").trim()).unwrap();
        let calls = v["choices"][0]["delta"]["tool_calls"].as_array().unwrap();
        assert_eq!(calls.len(), 1, "same-index entries must merge: {text}");
        assert_eq!(calls[0]["id"], "c1");
        assert_eq!(calls[0]["function"]["name"], "f");
        assert_eq!(calls[0]["function"]["arguments"], "{\"x\":2500}");
    }

    #[test]
    fn empty_name_and_arguments_item_is_dropped_and_empty_name_field_removed() {
        // 空名空参项整项剔除；只有空名、参数非空的项保留但剔掉 name 字段。
        let line = concat!(
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[",
            "{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"f\",\"arguments\":\"{\"}},",
            "{\"index\":1,\"function\":{\"name\":\"\",\"arguments\":\"\"}},",
            "{\"index\":2,\"function\":{\"name\":\"\",\"arguments\":\"1}\"}}]}}]}\n",
        )
        .as_bytes();
        let out = fed_in_pieces(&[line]);
        let text = String::from_utf8(out).unwrap();
        let v: Value = serde_json::from_str(text.trim_start_matches("data: ").trim()).unwrap();
        let calls = v["choices"][0]["delta"]["tool_calls"].as_array().unwrap();
        assert_eq!(calls.len(), 2, "empty name+args item dropped: {text}");
        assert_eq!(calls[0]["function"]["name"], "f");
        assert!(
            calls[1].get("function").unwrap().get("name").is_none(),
            "empty name field removed"
        );
        assert_eq!(calls[1]["function"]["arguments"], "1}");
    }

    #[test]
    fn unparseable_and_non_data_lines_pass_through() {
        let raw = concat!(
            "data: {\"choices\":[{\"index\":0,\n", // 坏 JSON 行
            ": keep-alive comment\n",              // SSE 注释
            "event: ping\n",
            "data: [DONE]\n",
        )
        .as_bytes();
        assert_eq!(fed_in_pieces(&[raw]), raw.to_vec());
    }

    #[test]
    fn deepseek_cache_hit_is_told_in_openai_names() {
        let line = concat!(
            "data: {\"id\":\"x\",\"choices\":[],\"usage\":",
            "{\"prompt_tokens\":1000,\"completion_tokens\":10,\"total_tokens\":1010,\"prompt_cache_hit_tokens\":800}}\n",
        )
        .as_bytes();
        let out = fed_in_pieces(&[line]);
        let v: Value = serde_json::from_str(
            String::from_utf8(out)
                .unwrap()
                .trim_start_matches("data: ")
                .trim(),
        )
        .unwrap();
        // DeepSeek 的 prompt_tokens 已含缓存 → 数值不动，只补 details。
        assert_eq!(v["usage"]["prompt_tokens"], 1000);
        assert_eq!(v["usage"]["prompt_tokens_details"]["cached_tokens"], 800);
        assert_eq!(v["usage"]["total_tokens"], 1010);
    }

    #[test]
    fn anthropic_names_are_converted_to_openai_counts() {
        let line = concat!(
            "data: {\"id\":\"x\",\"choices\":[],\"usage\":",
            "{\"prompt_tokens\":400,\"completion_tokens\":50,\"total_tokens\":450,",
            "\"cache_read_input_tokens\":500,\"cache_creation_input_tokens\":100}}\n",
        )
        .as_bytes();
        let out = fed_in_pieces(&[line]);
        let v: Value = serde_json::from_str(
            String::from_utf8(out)
                .unwrap()
                .trim_start_matches("data: ")
                .trim(),
        )
        .unwrap();
        // Anthropic 只数非缓存 → prompt_tokens 重写成含缓存口径，total 跟着调。
        assert_eq!(v["usage"]["prompt_tokens"], 1000);
        assert_eq!(v["usage"]["total_tokens"], 1050);
        assert_eq!(v["usage"]["prompt_tokens_details"]["cached_tokens"], 500);
        assert_eq!(
            v["usage"]["prompt_tokens_details"]["cache_write_tokens"],
            100
        );
        // 原字段保留（Go：every field the upstream sent stays）。
        assert_eq!(v["usage"]["cache_read_input_tokens"], 500);
    }

    #[test]
    fn usage_frame_without_choices_gets_an_empty_array() {
        let line = concat!(
            "data: {\"id\":\"x\",\"object\":\"chat.completion.chunk\",",
            "\"usage\":{\"prompt_tokens\":9,\"completion_tokens\":5,\"total_tokens\":14,\"prompt_cache_hit_tokens\":4}}\n",
        )
        .as_bytes();
        let out = fed_in_pieces(&[line]);
        let v: Value = serde_json::from_str(
            String::from_utf8(out)
                .unwrap()
                .trim_start_matches("data: ")
                .trim(),
        )
        .unwrap();
        assert_eq!(v["choices"], Value::Array(Vec::new()));
        assert_eq!(v["usage"]["prompt_tokens_details"]["cached_tokens"], 4);
    }

    #[test]
    fn whole_reply_is_mended_statelessly() {
        let body = concat!(
            "{\"id\":\"c\",\"object\":\"chat.completion\",\"choices\":[{\"index\":0,\"message\":",
            "{\"role\":\"assistant\",\"content\":null,\"tool_calls\":[{\"id\":\"c1\",\"type\":\"function\",",
            "\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"city\\\":\\\"Paris\\\",\\\"yield_ms\\\":2500.0}\"}}]},",
            "\"finish_reason\":\"tool_calls\"}],",
            "\"usage\":{\"prompt_tokens\":4,\"completion_tokens\":2,\"total_tokens\":6}}",
        )
        .as_bytes();
        let out = mend_whole_reply(body);
        let text = String::from_utf8(out.clone()).unwrap();
        // arguments 是字符串值，text 里引号带转义——对转义形态断言（结构断言在下一行）。
        assert!(
            text.contains("\\\"yield_ms\\\":2500"),
            "zero fraction must be shed: {text}"
        );
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(
            v["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
            "{\"city\":\"Paris\",\"yield_ms\":2500}"
        );
        // 无缓存的 OpenAI usage 不动。
        assert_eq!(v["usage"]["prompt_tokens"], 4);
        // 没有可改写物的体原字节返回。
        let plain = br#"{"id":"c","choices":[{"index":0,"message":{"role":"assistant","content":"plain"},"finish_reason":"stop"}]}"#;
        assert_eq!(mend_whole_reply(plain), plain.to_vec());
    }

    /// 一字节一段吐出的 reader。
    struct Dribble(Vec<u8>);

    impl Read for Dribble {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0.pop() {
                Some(b) => {
                    buf[0] = b;
                    Ok(1)
                }
                None => Ok(0),
            }
        }
    }

    #[test]
    fn reader_matches_bulk_feed() {
        let mut reversed = TOOL_STREAM.to_vec();
        reversed.reverse();
        let mut reader = ChattidyReader::new(Dribble(reversed));
        let mut out = Vec::new();
        reader.read_to_end(&mut out).unwrap();
        assert_eq!(out, mended_tool_stream());
    }
}
