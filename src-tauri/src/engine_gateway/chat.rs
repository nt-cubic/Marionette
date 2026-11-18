//! `/v1/chat/completions` 转发（合同 C5 的 openai→openai 直通形态）。
//! 模型 id 剥 `provider/` 前缀后按来源出站；SSE 流式透传（边读边 flush）。
//! 字节上限 refit（丢最老图重发）在 bodycap.rs，由这里在 4xx 应答后调用。
//!
//! 组路由与失败转移（groups.rs）：asked 解析成有序候选表，[`attempt`] 打一个
//! 成员——连接失败 / `groups::retryable` 口径的状态时返回 `Attempt::Retry`
//! 换下一个；**只在应答头阶段换**，流式 2xx 一旦交出
//! `Stream` 就是终局。responses/messages 的翻译路径复用同一个 [`attempt`]。
//!
//! 批 3「流可靠性」（D7/D10，Go fallback.go holdWriter + routing.go rest +
//! chattidy.go/grok_integral.go 的最小组合）：
//! - **扣流首**（D7）：2xx 流式应答先扣住流首——上游吐出第一条完整 SSE 行
//!   之前一个字节都不给客户端；死在流首之前的候选静默换下一个重发整轮。
//!   流首一旦放行，流就是终局。
//! - **可判定收尾**（D7）：上游流 EOF 时若从未见过 `data: [DONE]`
//!   （非流式 = 整块 JSON 解析失败），且没有候选可换，由 [`ClosingReader`]
//!   补一段显式 error 帧 + `[DONE]`——裸 EOF 违约，agent 必须能判定这轮
//!   坏掉了。
//! - **直通改写**（D10）：流输出包 `chattidy::ChattidyReader`，非流式整块
//!   过 `chattidy::mend_whole_reply`——工具参数零分数（2500.0→2500）、
//!   usage 口径、空名工具剔除、同帧同 index 合并（详见 chattidy.rs）。
//! - **休息记忆**：5xx/429/断流的候选 60s 内跳过（全在休息则无视）。
//!
//! 出站前的 chat 体「合身」（Go gateway.go `case provider.Chat` 那一段的
//! 子集，[`fit_for_vendor`]）：`developer`→`system`、纯文本 parts 折成字符串、
//! `max_tokens`↔`max_completion_tokens`（OpenAI 自家域名主动用后者；其它厂商
//! 400 点名另一个字段时改名重发一次并记住）、DeepSeek 系回放 `reasoning_content`
//! （translate.rs 的 responses→chat 把 reasoning item 暂存在 [`THINK_KEY`]，
//! 这里按厂商决定是回放还是丢弃）。
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::bodycap;
use super::chattidy;
use super::groups::{self, Attempt, Candidate};
use super::httpd::Reply;

/// translate.rs 把 responses `reasoning` item 的文本暂存在 assistant 消息的
/// 这个键上；出站前 [`fit_for_vendor`] 一定会摘掉它（回放成 `reasoning_content`
/// 或直接丢弃），它绝不能漏到厂商那里。
pub(crate) const THINK_KEY: &str = "x_marionette_think";

/// Go `noReasoning`：DeepSeek 要求带工具调用的 assistant 回合必须有
/// reasoning_content，没有思考可回放时塞一个空格占位（#1462/#1822）。
pub(crate) const NO_REASONING: &str = " ";

/// 出站请求的目标（bodycap 重发复用）。
pub(crate) struct Outbound {
    pub(crate) provider_id: String,
    key: String,
    headers: Vec<(String, String)>,
    bare_model: String,
    url: String,
}

impl Outbound {
    /// 姊妹模块（messages.rs 的 anthropic 入站翻译路径）构造出站目标用：
    /// 字段是模块私有的、resolve 又只服务 chat 端点自己——开一个
    /// pub(crate) 构造口，让翻译路径与 chat.rs 共用同一套出站与
    /// bodycap 重发（maybe_refit 的签名收 &Outbound）。
    pub(crate) fn new(
        provider_id: String,
        key: String,
        headers: Vec<(String, String)>,
        bare_model: String,
        url: String,
    ) -> Outbound {
        Outbound { provider_id, key, headers, bare_model, url }
    }
}

/// 候选 → 出站目标（锁内只查表克隆，不等网络）。
pub(crate) fn outbound_for(gw: &super::Gateway, cand: &Candidate) -> Result<Outbound, String> {
    let store = gw.store.lock().map_err(|_| "store poisoned".to_string())?;
    let provider = store
        .find(&cand.provider)
        .ok_or_else(|| format!("no provider for model: {}/{}", cand.provider, cand.model))?;
    let base = provider
        .chat_base()
        .ok_or_else(|| format!("{} has no endpoint configured", cand.provider))?
        .to_string();
    Ok(Outbound {
        provider_id: provider.id.clone(),
        key: provider.key.clone(),
        headers: provider.headers.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        bare_model: cand.model.clone(),
        url: format!("{}/chat/completions", base),
    })
}

/// 把请求体里的 `model` 换成厂商裸名。
fn rewrite_model(body: &[u8], bare: &str) -> Vec<u8> {
    let mut value = match serde_json::from_slice::<Value>(body) {
        Ok(v) => v,
        Err(_) => return body.to_vec(),
    };
    if let Some(map) = value.as_object_mut() {
        map.insert("model".into(), Value::String(bare.to_string()));
    }
    serde_json::to_vec(&value).unwrap_or_else(|_| body.to_vec())
}

pub(crate) fn send(out: &Outbound, body: &[u8]) -> Result<ureq::Response, String> {
    let agent = super::outbound_agent();
    let mut request = agent
        .post(&out.url)
        .set("Content-Type", "application/json")
        .set("Accept", "application/json");
    if !out.key.is_empty() {
        request = request.set("Authorization", &format!("Bearer {}", out.key));
    }
    for (k, v) in &out.headers {
        request = request.set(k, v);
    }
    // 上游的 Retry-After / request-id / ratelimit 头记下，写层随应答带给客户端
    // （审计 #9；最后一次上游应答 = 终局应答）。
    match request.send_bytes(body) {
        Ok(response) => {
            super::httpd::note_upstream(&response);
            Ok(response)
        }
        // ureq 2 把非 2xx 当 Err(Status(code, resp))——厂商的 4xx/5xx 是
        // 合同面的一部分（body-cap refit 靠它），原样取回 response。
        Err(ureq::Error::Status(_, response)) => {
            super::httpd::note_upstream(&response);
            Ok(response)
        }
        Err(e) => Err(e.to_string()),
    }
}

fn read_all(mut reader: impl Read) -> Vec<u8> {
    let mut buf = Vec::new();
    let _ = reader.read_to_end(&mut buf);
    buf
}

// ---------------------------------------------------------------------------
// 出站合身
// ---------------------------------------------------------------------------

const MAX_TOKENS: &str = "max_tokens";
const MAX_COMPLETION_TOKENS: &str = "max_completion_tokens";

/// URL 的主机名（小写，无端口）。
pub(crate) fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let authority = rest.split('/').next().unwrap_or("");
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    authority.split(':').next().unwrap_or("").to_ascii_lowercase()
}

/// Go `OpenAIHost`：api.openai.com 自家（不是 Azure、不是中转）。
fn is_openai_host(host: &str) -> bool {
    host == "openai.com" || host.ends_with(".openai.com")
}

/// Go chatReplaysReasoning 的主体：DeepSeek 系（域名或模型名含 deepseek）
/// 要求把上一回合的思考以 `reasoning_content` 回放。
fn replays_reasoning(host: &str, model: &str) -> bool {
    host.contains("deepseek") || model.to_ascii_lowercase().contains("deepseek")
}

/// 进程内记住的「这个来源要哪个回复长度字段」（Go LearnTokenField 的内存版）。
fn learned_token_fields() -> &'static std::sync::Mutex<std::collections::HashMap<String, &'static str>> {
    static MAP: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, &'static str>>,
    > = std::sync::OnceLock::new();
    MAP.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn token_field_for(provider_id: &str, host: &str) -> Option<&'static str> {
    if let Ok(map) = learned_token_fields().lock() {
        if let Some(field) = map.get(provider_id) {
            return Some(field);
        }
    }
    is_openai_host(host).then_some(MAX_COMPLETION_TOKENS)
}

/// Go TokenFieldWanted：400 的话明说「请用另一个字段」才算（`max_tokens must be
/// at most 32768` 是上限超了，不是名字错）。
fn token_field_wanted(body: &[u8]) -> Option<&'static str> {
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = text[from..].find("use") {
        let mut at = from + i + 3;
        from = at;
        let rest = &text[at..];
        let trimmed = rest.trim_start();
        if trimmed.len() == rest.len() {
            continue; // `use` 后必须有空白（\buse\s+）
        }
        at += rest.len() - trimmed.len();
        let rest = text[at..].trim_start_matches(['\'', '"', '`']);
        if rest.starts_with(MAX_COMPLETION_TOKENS) {
            return Some(MAX_COMPLETION_TOKENS);
        }
        if rest.starts_with(MAX_TOKENS) {
            return Some(MAX_TOKENS);
        }
    }
    None
}

/// 把体里的回复长度字段改名成 `want`（两个都有就保留 want 的值）。
/// 没有可改的返回 None。
fn with_token_field(body: &[u8], want: &str) -> Option<Vec<u8>> {
    let from = if want == MAX_TOKENS {
        MAX_COMPLETION_TOKENS
    } else if want == MAX_COMPLETION_TOKENS {
        MAX_TOKENS
    } else {
        return None;
    };
    let mut v: Value = serde_json::from_slice(body).ok()?;
    let map = v.as_object_mut()?;
    let value = map.remove(from)?;
    map.entry(want.to_string()).or_insert(value);
    serde_json::to_vec(&v).ok()
}

/// 全是裸 `{type:text,text}` 的 content 数组 → 拼成一个字符串（Go
/// plainTextParts：多段之间补换行；带图/文件/cache_control 等任何别的东西
/// 就整个保持原样）。
fn plain_text_of(parts: &[Value]) -> Option<String> {
    if parts.is_empty() {
        return None;
    }
    let mut texts = Vec::new();
    for p in parts {
        let obj = p.as_object()?;
        if obj.get("type").and_then(|t| t.as_str()) != Some("text") {
            return None;
        }
        if obj.keys().any(|k| k != "type" && k != "text") {
            return None;
        }
        texts.push(obj.get("text")?.as_str()?.to_string());
    }
    Some(texts.join("\n"))
}

/// chat 体出站前按厂商合身。没有任何改动时原样返回字节。
pub(crate) fn fit_for_vendor(
    body: &[u8],
    host: &str,
    model: &str,
    token_field: Option<&str>,
) -> Vec<u8> {
    let Ok(mut v) = serde_json::from_slice::<Value>(body) else { return body.to_vec() };
    let Some(map) = v.as_object_mut() else { return body.to_vec() };
    let mut changed = false;
    if let Some(want) = token_field {
        let from = if want == MAX_TOKENS { MAX_COMPLETION_TOKENS } else { MAX_TOKENS };
        if let Some(value) = map.remove(from) {
            map.entry(want.to_string()).or_insert(value);
            changed = true;
        }
    }
    let replay = replays_reasoning(host, model);
    if let Some(msgs) = map.get_mut("messages").and_then(|m| m.as_array_mut()) {
        let mut after_tool = false;
        for msg in msgs.iter_mut() {
            let Some(m) = msg.as_object_mut() else { continue };
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("").to_string();
            if role == "developer" {
                m.insert("role".into(), Value::String("system".into()));
                changed = true;
            }
            if let Some(Value::Array(parts)) = m.get("content") {
                if let Some(text) = plain_text_of(parts) {
                    m.insert("content".into(), Value::String(text));
                    changed = true;
                }
            }
            match role.as_str() {
                "assistant" => {
                    let hidden = m.remove(THINK_KEY);
                    if hidden.is_some() {
                        changed = true;
                    }
                    if replay {
                        let said = |v: Option<&Value>| {
                            v.and_then(|x| x.as_str()).filter(|s| !s.trim().is_empty()).map(String::from)
                        };
                        let think = said(hidden.as_ref()).or_else(|| said(m.get("reasoning_content")));
                        let has_calls = m
                            .get("tool_calls")
                            .and_then(|c| c.as_array())
                            .is_some_and(|c| !c.is_empty());
                        let target = match think {
                            Some(t) => Some(t),
                            None if has_calls || after_tool => Some(NO_REASONING.to_string()),
                            None => None,
                        };
                        if let Some(t) = target {
                            if m.get("reasoning_content").and_then(|x| x.as_str()) != Some(t.as_str()) {
                                m.insert("reasoning_content".into(), Value::String(t));
                                changed = true;
                            }
                        }
                    }
                    after_tool = false;
                }
                "tool" => after_tool = true,
                _ => after_tool = false,
            }
        }
    }
    if !changed {
        return body.to_vec();
    }
    serde_json::to_vec(&v).unwrap_or_else(|_| body.to_vec())
}

// ---------------------------------------------------------------------------
// 一个成员的一次尝试
// ---------------------------------------------------------------------------

/// 打一个候选成员（`body` 是 chat 体，`model` 字段任意——这里换成厂商裸名）。
/// 流式 2xx 立刻交出 `Stream`（终局）；错误应答读全后先过 bodycap（字节上限
/// refit）与回复长度字段自适应，最后按 `groups::retryable` 决定是否换成员。
pub(crate) fn attempt(gw: &super::Gateway, cand: &Candidate, body: &[u8]) -> Attempt {
    let out = match outbound_for(gw, cand) {
        Ok(o) => o,
        Err(e) => return Attempt::Retry(Reply::error(404, &e)),
    };
    let host = host_of(&out.url);
    let fitted = fit_for_vendor(
        &rewrite_model(body, &out.bare_model),
        &host,
        &cand.model,
        token_field_for(&out.provider_id, &host),
    );
    // cap 已知时先合身再发（免吃一次拒答）。
    let mut current = bodycap::prefit(gw, &out.provider_id, &fitted).unwrap_or(fitted);
    let streaming = serde_json::from_slice::<Value>(&current)
        .ok()
        .and_then(|v| v.get("stream").and_then(|s| s.as_bool()))
        .unwrap_or(false);

    let mut field_retried = false;
    loop {
        let response = match send(&out, &current) {
            Ok(r) => r,
            // 连接失败 / 超时：成员不可达，换下一个。
            Err(e) => {
                return Attempt::Retry(Reply::error(502, &format!("{}: {}", out.provider_id, e)))
            }
        };
        let status = response.status();
        if status < 400 {
            if streaming {
                let content_type = response.content_type().to_string();
                return Attempt::Done(Reply::Stream {
                    status,
                    content_type,
                    reader: Box::new(response.into_reader()),
                });
            }
            return Attempt::Done(Reply::Json { status, body: read_all(response.into_reader()) });
        }
        let vendor_body = read_all(response.into_reader());
        // 回复长度字段：400 明说要另一个名字 → 改名重发一次并记住。
        if status == 400 && !field_retried {
            if let Some(want) = token_field_wanted(&vendor_body) {
                if let Some(renamed) = with_token_field(&current, want) {
                    if let Ok(mut map) = learned_token_fields().lock() {
                        map.insert(out.provider_id.clone(), want);
                    }
                    current = renamed;
                    field_retried = true;
                    continue;
                }
            }
        }
        // C5+C7：字节上限拒答（ollama「failed to read request body」/ 明说超限）→
        // 学 cap、丢最老图重发；bodycap 没命中时原样透传错误。
        if let Some(reply) = bodycap::maybe_refit(gw, &out, &current, status, &vendor_body) {
            return Attempt::Done(reply);
        }
        let retry = groups::retryable(status, &vendor_body);
        let reply = Reply::Json { status, body: vendor_body };
        return if retry { Attempt::Retry(reply) } else { Attempt::Done(reply) };
    }
}

// ---------------------------------------------------------------------------
// D7：断流可判定收尾 + 扣流首换人 + 休息记忆
//（Go fallback.go:2786 holdWriter + routing.go:69 served/rest 的最小组合；
//  continuation 断流续答（continuation.go:364）不做——见文件尾差距清单）
// ---------------------------------------------------------------------------

/// 流首扣留的体量上限：单行超长的厂商流（百 KB 的 arguments 分片）不再
/// 等换行，按活流放行。
const HEAD_HOLD_MAX: usize = 256 * 1024;

/// 休息时长：5xx/429/断流的候选 60s 内被候选循环跳过。
/// 与 Go 的差距（routing.go:69 `served` / restingUntil 家族）：不落盘、不分
/// 错误等级（Go 按额度耗尽/限流 Retry-After/挂了分窗口长度且逐次失败退避
/// 递增）、无 per-account/per-model 细分——这里按 provider id 一把抓。
const REST_FOR: Duration = Duration::from_secs(60);

fn rests() -> &'static Mutex<HashMap<String, Instant>> {
    static RESTS: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    RESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 标记一个来源休息（5xx / 429 / 断流）。
fn rest_provider(provider_id: &str) {
    if let Ok(mut map) = rests().lock() {
        map.retain(|_, until| *until > Instant::now());
        map.insert(provider_id.to_string(), Instant::now() + REST_FOR);
    }
}

/// 来源答上来了（流首放行 / 整块 JSON 解析成功）：清掉它的休息。
fn wake(provider_id: &str) {
    if let Ok(mut map) = rests().lock() {
        map.remove(provider_id);
    }
}

/// 该来源还剩多久休息（None = 不在休息）。pub(crate)：后续批次（订阅线/
/// 额度联动）复用这张表。
pub(crate) fn rest_left(provider_id: &str) -> Option<Duration> {
    let map = rests().lock().ok()?;
    map.get(provider_id).map(|until| until.saturating_duration_since(Instant::now()))
}

/// 测试隔离：routing_tests 的沙箱起手清表（并行测试互不串休息）。
#[cfg(test)]
fn clear_rests_for_tests() {
    let _ = rests().lock().map(|mut m| m.clear());
}

/// 扣流首的结果。
enum HeadHold {
    /// 上游吐出了第一条完整 SSE 行（流活着）：扣住的字节 + 余下 reader。
    Live { head: Vec<u8>, rest: Box<dyn Read + Send> },
    /// 上游在第一条完整行之前就 EOF/读错误（一条完整行都没给）——可以静默
    /// 换下一个候选重发整轮（客户端一个字节都还没收到）。
    Dead,
}

/// 缓冲上游输出直到第一条完整行（`\n`）——Go holdWriter 扣流首的最小版：
/// 应答头已写出（状态/Content-Type 描述的是形状不是内容），但内容字节一
/// 个都没上路，此时换人客户端无感。单行超过 [`HEAD_HOLD_MAX`] 不再等。
fn hold_head(mut reader: Box<dyn Read + Send>) -> HeadHold {
    let mut head = Vec::new();
    let mut chunk = [0u8; 8 * 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => return HeadHold::Dead,
            Ok(n) => {
                head.extend_from_slice(&chunk[..n]);
                if head.contains(&b'\n') || head.len() >= HEAD_HOLD_MAX {
                    return HeadHold::Live { head, rest: reader };
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => return HeadHold::Dead,
        }
    }
}

/// 流首字节 + 余下 reader 的拼接（扣流首放行后把扣住的字节还给流）。
struct HeadThen {
    head: Vec<u8>,
    at: usize,
    rest: Box<dyn Read + Send>,
}

impl Read for HeadThen {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.at < self.head.len() {
            let n = buf.len().min(self.head.len() - self.at);
            buf[..n].copy_from_slice(&self.head[self.at..self.at + n]);
            self.at += n;
            return Ok(n);
        }
        self.rest.read(buf)
    }
}

/// `data: [DONE]` 行的 O(1) 匹配器：逐字节喂入，**行首锚定**（只容忍行首
/// 空白），`data:` 后容忍 0..n 个空格，行尾容忍 `\r`/空白；出现任何别的
/// 字节即判本行不可能（直到下一个换行重新开始）。大小写敏感（OpenAI 固定
/// 大写）。
#[derive(Default)]
struct DoneMatcher {
    state: DoneState,
}

#[derive(Default, PartialEq, Clone, Copy)]
enum DoneState {
    /// 行首（到目前为止只有空白）。
    #[default]
    Start,
    /// `data:` 前缀匹配到第几字节（1..=4）。
    Prefix(usize),
    /// 冒号已过，跳空白。
    Gap,
    /// `[DONE]` 匹配到第几字节（1..=6）。
    Done(usize),
    /// `[DONE]` 已完整，只等行尾空白。
    Trailing,
    /// 本行已不可能。
    Dead,
}

impl DoneMatcher {
    const PREFIX: &'static [u8] = b"data:";
    const DONE: &'static [u8] = b"[DONE]";

    fn byte(&mut self, b: u8) {
        self.state = match self.state {
            DoneState::Start => match b {
                b' ' | b'\t' => DoneState::Start,
                b'd' => DoneState::Prefix(1),
                _ => DoneState::Dead,
            },
            DoneState::Prefix(at) => {
                if at < Self::PREFIX.len() && b == Self::PREFIX[at] {
                    if at + 1 == Self::PREFIX.len() {
                        DoneState::Gap
                    } else {
                        DoneState::Prefix(at + 1)
                    }
                } else {
                    DoneState::Dead
                }
            }
            DoneState::Gap => match b {
                b' ' | b'\t' => DoneState::Gap,
                b'[' => DoneState::Done(1),
                _ => DoneState::Dead,
            },
            DoneState::Done(at) => {
                if at < Self::DONE.len() && b == Self::DONE[at] {
                    if at + 1 == Self::DONE.len() {
                        DoneState::Trailing
                    } else {
                        DoneState::Done(at + 1)
                    }
                } else {
                    DoneState::Dead
                }
            }
            DoneState::Trailing => match b {
                b' ' | b'\t' | b'\r' => DoneState::Trailing,
                _ => DoneState::Dead,
            },
            DoneState::Dead => DoneState::Dead,
        };
    }

    /// 行结束（\n 到达）。返回这一行是不是合法的 `[DONE]` 终止行，并复位。
    fn end_of_line(&mut self) -> bool {
        let hit = self.state == DoneState::Trailing;
        self.state = DoneState::Start;
        hit
    }
}

/// 收尾守卫（D7）：上游 EOF/读错误时若从未见过 `data: [DONE]`，向客户端
/// 补一段显式 error 帧 + `[DONE]`——裸 EOF 违约（agent 挂在 EOF 上分不清
/// 这轮是完了还是坏了）。已见 [DONE] 的流按原样收尾；读错误视同断流。
struct ClosingReader {
    provider: String,
    inner: Box<dyn Read + Send>,
    matcher: DoneMatcher,
    seen_done: bool,
    finished: bool,
    pending: Vec<u8>,
    at: usize,
}

impl ClosingReader {
    fn new(provider: String, inner: Box<dyn Read + Send>) -> ClosingReader {
        ClosingReader {
            provider,
            inner,
            matcher: DoneMatcher::default(),
            seen_done: false,
            finished: false,
            pending: Vec::new(),
            at: 0,
        }
    }

    /// 终局：没见过 [DONE] 就补显式收尾（对话流形态：error 帧 + [DONE]）。
    fn finish(&mut self) {
        self.finished = true;
        if !self.seen_done {
            let frame = format!(
                "data: {{\"error\":{{\"message\":\"{} stream ended without its terminator\",\"type\":\"engine_error\",\"code\":null}}}}\n\ndata: [DONE]\n\n",
                self.provider
            );
            self.pending = frame.into_bytes();
            self.at = 0;
        }
    }
}

impl Read for ClosingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.at < self.pending.len() {
                let n = buf.len().min(self.pending.len() - self.at);
                buf[..n].copy_from_slice(&self.pending[self.at..self.at + n]);
                self.at += n;
                return Ok(n);
            }
            if self.finished {
                return Ok(0);
            }
            let mut chunk = [0u8; 16 * 1024];
            match self.inner.read(&mut chunk) {
                Ok(0) => self.finish(),
                Ok(n) => {
                    for &b in &chunk[..n] {
                        if b == b'\n' {
                            if self.matcher.end_of_line() {
                                self.seen_done = true;
                            }
                        } else {
                            self.matcher.byte(b);
                        }
                    }
                    self.pending = chunk[..n].to_vec();
                    self.at = 0;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => self.finish(), // 断流：补收尾（不把错误裸抛给写层）
            }
        }
    }
}

/// POST /v1/chat/completions 入口（D7 后的候选循环，替代纯头阶段换人的
/// `groups::drive`）：头阶段失败换成员（原语义保留）→ 2xx 流式扣流首 →
/// 死在流首之前换下一个重发整轮 → 流首放行即终局（断流由 [`ClosingReader`]
/// 补可判定收尾）→ 2xx 非流式以「整块 JSON 解析成功」为合法收尾。休息中
/// 的候选跳过（全在休息则无视）。
/// 成功回合收账：粘性（曾答账号）+ usage 台账（token 桶从流里提留给后续
/// 批次——capture 层还没建，先记 millis/status/session 骨架）。
fn note_turn(gw: &super::Gateway, headers: &[(String, String)], body: &[u8], cand: &Candidate, status: u16) {
    let parsed: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let sk = super::affinity::stick_key(headers, &parsed);
    if let Some(sk) = &sk {
        gw.affinity.answered(sk, &cand.provider, &cand.model);
    }
    let asked = parsed.get("model").and_then(|m| m.as_str()).unwrap_or_default().to_string();
    gw.usage.record(super::usage_ledger::UsageRecord {
        ts: 0,
        agent: "chat".into(),
        provider: cand.provider.clone(),
        model: asked,
        millis: 0,
        status,
        rejected: false,
        session: sk,
        kind: "turn".into(),
        in_tokens: None, out_tokens: None, cache_read: None, cache_write: None,
    });
}

pub fn forward(gw: &super::Gateway, headers: &[(String, String)], body: &[u8]) -> Reply {
    let model = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(String::from))
        .unwrap_or_default();
    let cands = groups::plan_for(gw, &model);
    if cands.is_empty() {
        return Reply::error(404, &format!("no provider for model: {model}"));
    }
    let live: Vec<Candidate> = {
        let awake: Vec<&Candidate> = cands.iter().filter(|c| rest_left(&c.provider).is_none()).collect();
        let base: Vec<&Candidate> = if awake.is_empty() { cands.iter().collect() } else { awake };
        // 会话粘性（affinity.rs）：有种子时曾答账号浮最前（Go affine 最小版）
        let owned: Vec<Candidate> = base.into_iter().cloned().collect();
        match super::affinity::stick_key(headers, &serde_json::from_slice::<serde_json::Value>(body).unwrap_or(serde_json::Value::Null)) {
            Some(sk) => gw.affinity.affine(&sk, &owned.iter().map(|c| (c.provider.clone(), c.model.clone())).collect::<Vec<_>>())
                .into_iter().filter_map(|(pid, m)| owned.iter().find(|c| c.provider == pid && c.model == m).cloned())
                .collect(),
            None => owned,
        }
    };
    let mut last: Option<Reply> = None;
    for cand in &live {
        let body = groups::body_for(gw, super::effort::Proto::Chat, cand, body);
        match attempt(gw, cand, &body) {
            Attempt::Done(reply) => match reply {
                Reply::Stream { status, content_type, reader } => match hold_head(reader) {
                    HeadHold::Live { head, rest } => {
                        wake(&cand.provider);
                        note_turn(gw, headers, &body, cand, 200);
                        let relay = ClosingReader::new(
                            cand.provider.clone(),
                            Box::new(chattidy::ChattidyReader::new(HeadThen { head, at: 0, rest })),
                        );
                        return Reply::Stream { status, content_type, reader: Box::new(relay) };
                    }
                    HeadHold::Dead => {
                        rest_provider(&cand.provider);
                        last = Some(Reply::error(
                            502,
                            &format!("{}: stream ended before its first line", cand.provider),
                        ));
                        continue;
                    }
                },
                Reply::Json { status, body } if status < 400 => {
                    // 非流式的合法收尾 = 整块 JSON 解析成功（D7）；顺手过
                    // chattidy 的整块修补（D10 非流式半边）。
                    if serde_json::from_slice::<Value>(&body).is_ok() {
                        wake(&cand.provider);
                        note_turn(gw, headers, &body, cand, status);
                        return Reply::Json { status, body: chattidy::mend_whole_reply(&body) };
                    }
                    rest_provider(&cand.provider);
                    last = Some(Reply::error(
                        502,
                        &format!("{}: reply ended without whole JSON", cand.provider),
                    ));
                    continue;
                }
                other => return other,
            },
            Attempt::Retry(reply) => {
                if let Reply::Json { status, .. } = &reply {
                    if *status == 429 || *status >= 500 {
                        rest_provider(&cand.provider);
                    }
                }
                last = Some(reply);
                continue;
            }
        }
    }
    last.unwrap_or_else(|| Reply::error(502, "no candidate answered"))
}

// ---------------------------------------------------------------------------
// 与 Go 的已知差距（批 3 收口清单；survey N1/N3/N4 的 Rust 侧残留）
// ---------------------------------------------------------------------------

// - **continuation 断流续答不做**（Go continuation.go:364 again / :378
//   prefillText / :455 streamTranslated）：上游在流中途死亡且字节已发给
//   客户端时，Go 会用 prefill 续写拼回完整回复；这里只保证可判定收尾
//   （ClosingReader 的显式 error 帧 + [DONE]），不续答。
// - **扣流首只护第一条完整行**：Go holdWriter 有 firstToken/patience 计时
//   （组里等多久放弃首字节）与 refusalHandover（读流首开头识别整流拒答
//   后换人）；这里流首一旦放行即终局，慢上游会让 agent 一直挂着（与现状
//   一致），流首之后的 200 拒答体原样透传。
// - **休息记忆极简**（见 [`REST_FOR`] 注释）：不落盘、不分错误等级、无
//   Retry-After 解析、成功只清本来源（Go served 还记 token 用量做 usage
//   路由）；且只挂在 chat 直通候选循环上——responses/messages 翻译路径
//   的 `groups::drive` 不查休息表（后续批次接线）。
// - **chattidy 只做合同面子集**（见 chattidy.rs 文件头）：withReasoning
//   OnToolTurns、空 reasoning_content 剔除、finish_reason 规整、流式
//   id/index 补齐、无名调用整链丢弃等未搬。

// ---------------------------------------------------------------------------
// textcalls 的两个端点共用件（responses.rs / messages.rs 翻译路径的应答侧）
// ---------------------------------------------------------------------------

/// 正文书写的调用变真调用后的 call id（Go textCallID，textcalls.go:737-739：
/// `call_text_<进程内序号>`）。端点无关——responses 与 messages 方言共用
/// 同一序号空间。
pub(crate) fn text_call_id() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!("call_text_{}", SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1)
}

/// 非流式整块应答里的正文书调用（Go wholeTextCalls，textcalls.go:990-1039）：
/// 把 chat completion JSON 的 `choices[0].message.content` 整块喂给
/// [`super::textcalls::TextCallScout`]，读出的调用追加进 `message.tool_calls`
/// （id 用 [`text_call_id`]），content 改写成「标记前正文 + 解析剩余」（空则
/// null——下游 responses/messages 转换器对 null content 都不出文本块/文本 item）。
/// **消费 scout**（finish 按值取）——每个应答各带各的 scout，一次性。
///
/// 原生 tool_calls 在场时（Go `made`）：note_native_call 先行——持有的
/// 标签尾巴丢弃、非标签原文照放，不再从正文读调用（语义与 Go 的 tagsOnly
/// 分支一致，仅尾随空白口径走 scout 的 keepSpace）。
pub(crate) fn whole_text_calls(chat_reply: &mut Value, mut scout: super::textcalls::TextCallScout) {
    // 原生 tool_calls 在场：正文里的标签只清不读（Go wholeTextCalls 的 made 分支
    // ↔ scout 的 note_native_call settle 语义）。
    let native = chat_reply
        .pointer("/choices/0/message/tool_calls")
        .and_then(|t| t.as_array())
        .is_some_and(|a| !a.is_empty());
    let text = match chat_reply.pointer("/choices/0/message/content") {
        Some(Value::String(s)) => s.clone(),
        // Go wholeTextCalls：content 不是串（null / parts 数组）就不动它。
        _ => return,
    };
    if native {
        let _ = scout.note_native_call();
    }
    let released = scout.feed_text(&text);
    let fin = scout.finish();
    let full = format!("{released}{}", fin.text);
    if fin.calls.is_empty() && full == text {
        return; // 没有标记 / 读不出调用：原样（Go !ok 分支）
    }
    let made_calls = !fin.calls.is_empty();
    {
        let Some(msg) = chat_reply
            .pointer_mut("/choices/0/message")
            .and_then(|m| m.as_object_mut())
        else {
            return;
        };
        if full != text {
            msg.insert("content".into(), if full.is_empty() { Value::Null } else { json!(full) });
        }
        if made_calls {
            let calls = msg.entry("tool_calls".to_string()).or_insert_with(|| json!([]));
            if let Some(arr) = calls.as_array_mut() {
                for (name, args) in fin.calls {
                    arr.push(json!({
                        "id": text_call_id(), "type": "function",
                        "function": {"name": name, "arguments": args},
                    }));
                }
            }
        }
    }
    if made_calls {
        // 回合作为调过工具的回合结束（Go chatWhole.flush：`called` →
        // finish_reason="tool_calls"；responses 方言 status 不变，anthropic
        // 方言 stop_reason → tool_use）。
        if let Some(choice) = chat_reply.pointer_mut("/choices/0").and_then(|c| c.as_object_mut()) {
            choice.insert("finish_reason".into(), json!("tool_calls"));
        }
    }
}

#[cfg(test)]
mod fit_tests {
    use super::*;
    use serde_json::json;

    fn run(body: Value, host: &str, model: &str, field: Option<&str>) -> Value {
        let out = fit_for_vendor(&serde_json::to_vec(&body).unwrap(), host, model, field);
        serde_json::from_slice(&out).unwrap()
    }

    #[test]
    fn developer_role_and_text_parts_are_normalised() {
        let v = run(
            json!({"messages": [
                {"role": "developer", "content": "rules"},
                {"role": "user", "content": [{"type":"text","text":"a"},{"type":"text","text":"b"}]},
                {"role": "user", "content": [{"type":"text","text":"a"},{"type":"image_url","image_url":{"url":"x"}}]},
                {"role": "user", "content": [{"type":"text","text":"c","cache_control":{"type":"ephemeral"}}]},
            ]}),
            "api.example.com",
            "m",
            None,
        );
        let m = v["messages"].as_array().unwrap();
        assert_eq!(m[0]["role"], "system");
        assert_eq!(m[1]["content"], "a\nb");
        // 带图 / cache_control 的 parts 保持原样。
        assert!(m[2]["content"].is_array());
        assert!(m[3]["content"].is_array());
    }

    #[test]
    fn untouched_body_keeps_its_bytes() {
        let raw = br#"{"messages":[{"role":"user","content":"hi"}],"model":"m"}"#;
        assert_eq!(fit_for_vendor(raw, "h", "m", None), raw.to_vec());
    }

    #[test]
    fn token_field_follows_host_and_vendor_words() {
        assert_eq!(token_field_for("p", "api.openai.com"), Some(MAX_COMPLETION_TOKENS));
        assert_eq!(token_field_for("p-none", "api.deepseek.com"), None);
        assert_eq!(token_field_for("p", "myco.openai.azure.com"), None);
        let v = run(json!({"max_tokens": 10, "messages": []}), "api.openai.com", "m", Some(MAX_COMPLETION_TOKENS));
        assert_eq!(v["max_completion_tokens"], 10);
        assert!(v.get("max_tokens").is_none());
        // 两个都有：保留目标字段的值。
        let v = run(json!({"max_tokens": 1, "max_completion_tokens": 2, "messages": []}), "h", "m", Some(MAX_COMPLETION_TOKENS));
        assert_eq!(v["max_completion_tokens"], 2);
        assert!(v.get("max_tokens").is_none());
        assert_eq!(
            token_field_wanted(b"Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead."),
            Some(MAX_COMPLETION_TOKENS)
        );
        assert_eq!(token_field_wanted(b"please use max_tokens instead"), Some(MAX_TOKENS));
        // 上限超了不是名字错
        assert_eq!(token_field_wanted(b"max_tokens must be at most 32768"), None);
        assert_eq!(
            String::from_utf8(with_token_field(br#"{"max_tokens":5}"#, MAX_COMPLETION_TOKENS).unwrap()).unwrap(),
            r#"{"max_completion_tokens":5}"#
        );
    }

    #[test]
    fn deepseek_gets_reasoning_replay_and_placeholder() {
        let body = json!({"messages": [
            {"role": "user", "content": "q"},
            {"role": "assistant", "content": null, "x_marionette_think": "thought A",
             "tool_calls": [{"id":"c1","type":"function","function":{"name":"f","arguments":"{}"}}]},
            {"role": "tool", "tool_call_id": "c1", "content": "r"},
            {"role": "assistant", "content": "done"},
            {"role": "user", "content": "again"},
            {"role": "assistant", "content": "plain reply"},
        ]});
        let v = run(body.clone(), "api.deepseek.com", "deepseek-v4", None);
        let m = v["messages"].as_array().unwrap();
        assert_eq!(m[1]["reasoning_content"], "thought A");
        assert!(m[1].get(THINK_KEY).is_none());
        // 工具结果之后的回复没有思考：占位（DeepSeek 否则整条拒绝）。
        assert_eq!(m[3]["reasoning_content"], " ");
        // 普通回合（无调用、不跟在工具结果后）不加。
        assert!(m[5].get("reasoning_content").is_none());
        // 非 DeepSeek 厂商：暂存键被摘掉，思考不回放。
        let v = run(body, "api.other.com", "gpt-x", None);
        let m = v["messages"].as_array().unwrap();
        assert!(m[1].get(THINK_KEY).is_none());
        assert!(m[1].get("reasoning_content").is_none());
        assert!(m[3].get("reasoning_content").is_none());
    }

    #[test]
    fn host_parsing() {
        assert_eq!(host_of("https://API.openai.com/v1/chat/completions"), "api.openai.com");
        assert_eq!(host_of("http://127.0.0.1:8080/v1"), "127.0.0.1");
        assert_eq!(host_of("https://u:p@h.example.com:443/x"), "h.example.com");
    }
}

/// 组路由失败转移 + bodycap 三形状 + 厂商合身的端到端测试（本地假厂商；
/// std TcpListener 手写 HTTP，responses.rs 同款写法，零新依赖）。
#[cfg(test)]
mod routing_tests {
    use super::super::httpd::Reply;
    use super::super::Gateway;
    use super::*;
    use serde_json::json;
    use std::io::{BufRead, BufReader, Read as _, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex, MutexGuard};

    static DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

    struct Sandbox {
        dir: std::path::PathBuf,
        prior_xdg: Option<String>,
        #[allow(dead_code)]
        lock: MutexGuard<'static, ()>,
    }

    impl Drop for Sandbox {
        fn drop(&mut self) {
            match &self.prior_xdg {
                Some(prior) => std::env::set_var("XDG_CONFIG_HOME", prior),
                None => std::env::remove_var("XDG_CONFIG_HOME"),
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn sandbox(providers_json: &str) -> (Gateway, Sandbox) {
        let lock = super::super::TEST_SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // 休息表是进程级的：沙箱起手清掉，串行测试之间不串休息。
        clear_rests_for_tests();
        let dir = std::env::temp_dir().join(format!(
            "marionette-engine-routing-{}-{}",
            std::process::id(),
            DIR_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("magpie")).expect("temp config dir");
        std::fs::write(dir.join("magpie").join("providers.json"), providers_json).expect("providers.json");
        let prior_xdg = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let gateway = Gateway::open().expect("gateway");
        (gateway, Sandbox { dir, prior_xdg, lock })
    }

    struct Seen {
        body: Vec<u8>,
    }

    struct Fake {
        base: String,
        hits: Arc<AtomicUsize>,
        seen: Arc<Mutex<Vec<Seen>>>,
    }

    impl Fake {
        fn bodies(&self) -> Vec<Value> {
            self.seen
                .lock()
                .unwrap()
                .iter()
                .map(|s| serde_json::from_slice(&s.body).unwrap_or(Value::Null))
                .collect()
        }
    }

    type Handler = Box<dyn Fn(&Seen) -> (u16, &'static str, Vec<u8>) + Send + 'static>;

    fn fake(handler: Handler) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (h, s) = (hits.clone(), seen.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let mut reader = BufReader::new(match stream.try_clone() {
                    Ok(x) => x,
                    Err(_) => continue,
                });
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut length = 0usize;
                loop {
                    let mut l = String::new();
                    if reader.read_line(&mut l).unwrap_or(0) == 0 {
                        break;
                    }
                    let t = l.trim_end().to_ascii_lowercase();
                    if t.is_empty() {
                        break;
                    }
                    if let Some(v) = t.strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                if length > 0 {
                    let _ = reader.read_exact(&mut body);
                }
                h.fetch_add(1, Ordering::SeqCst);
                let rec = Seen { body };
                let (status, ct, out) = handler(&rec);
                s.lock().unwrap().push(rec);
                let mut stream = stream;
                let head = format!(
                    "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    status,
                    ct,
                    out.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&out);
                let _ = stream.flush();
            }
        });
        Fake { base: format!("http://127.0.0.1:{port}"), hits, seen }
    }

    fn ok_chat(text: &str) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "id": "c1", "object": "chat.completion", "model": "m",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": text}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }))
        .unwrap()
    }

    const SSE: &str = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\ndata: [DONE]\n\n";

    fn json_body(v: Value) -> Vec<u8> {
        serde_json::to_vec(&v).unwrap()
    }

    fn status_of(r: &Reply) -> u16 {
        match r {
            Reply::Json { status, .. } | Reply::Stream { status, .. } => *status,
        }
    }

    fn text_of(r: Reply) -> String {
        match r {
            Reply::Json { body, .. } => String::from_utf8_lossy(&body).into_owned(),
            Reply::Stream { mut reader, .. } => {
                let mut b = Vec::new();
                let _ = reader.read_to_end(&mut b);
                String::from_utf8_lossy(&b).into_owned()
            }
        }
    }

    /// 两个 chat 来源 + 一个组 g。
    fn two_member_world(a: &Fake, b: &Fake, routing: &str) -> String {
        json!({
            "providers": [
                {"id": "pa", "name": "A", "key": "ka", "chat": format!("{}/v1", a.base)},
                {"id": "pb", "name": "B", "key": "kb", "chat": format!("{}/v1", b.base)},
            ],
            "groups": [{"id": "g", "name": "g", "members": ["pa/ma", "pb/mb"], "routing": routing}],
        })
        .to_string()
    }

    fn chat_request(model: &str, stream: bool) -> Vec<u8> {
        json_body(json!({"model": model, "stream": stream, "messages": [{"role": "user", "content": "hi"}]}))
    }

    #[test]
    fn chat_group_fails_over_on_5xx() {
        let a = fake(Box::new(|_| (503, "application/json", b"{\"error\":\"overloaded\"}".to_vec())));
        let b = fake(Box::new(|_| (200, "application/json", ok_chat("from b"))));
        let (gw, _sb) = sandbox(&two_member_world(&a, &b, "order"));
        let reply = forward(&gw, &[], &chat_request("g", false));
        assert_eq!(status_of(&reply), 200);
        assert!(text_of(reply).contains("from b"));
        assert_eq!(a.hits.load(Ordering::SeqCst), 1);
        assert_eq!(b.bodies()[0]["model"], "mb");
    }

    #[test]
    fn chat_group_fails_over_on_connect_errors() {
        let b = fake(Box::new(|_| (200, "application/json", ok_chat("from b"))));
        let dead = format!("http://127.0.0.1:{}", {
            let l = TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        });
        let world = json!({
            "providers": [
                {"id": "pa", "name": "A", "chat": format!("{dead}/v1")},
                {"id": "pb", "name": "B", "chat": format!("{}/v1", b.base)},
            ],
            "groups": [{"id": "g", "name": "g", "members": ["pa/ma", "pb/mb"], "routing": "order"}],
        })
        .to_string();
        let (gw, _sb) = sandbox(&world);
        let reply = forward(&gw, &[], &chat_request("g", false));
        assert_eq!(status_of(&reply), 200);
        assert_eq!(b.hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn request_errors_stay_put_and_total_outage_returns_the_last_error() {
        let a = fake(Box::new(|_| (400, "application/json", b"{\"error\":{\"message\":\"bad parameter\"}}".to_vec())));
        let b = fake(Box::new(|_| (200, "application/json", ok_chat("never"))));
        let (gw, sb) = sandbox(&two_member_world(&a, &b, "order"));
        let reply = forward(&gw, &[], &chat_request("g", false));
        assert_eq!(status_of(&reply), 400);
        assert_eq!(b.hits.load(Ordering::SeqCst), 0, "request errors must not move to the next member");
        drop(sb);
        let a = fake(Box::new(|_| (503, "application/json", b"{\"error\":\"a down\"}".to_vec())));
        let b = fake(Box::new(|_| (502, "application/json", b"{\"error\":\"b down\"}".to_vec())));
        let (gw, _sb) = sandbox(&two_member_world(&a, &b, "order"));
        let reply = forward(&gw, &[], &chat_request("g", false));
        assert_eq!(status_of(&reply), 502);
        assert!(text_of(reply).contains("b down"));
    }

    #[test]
    fn streaming_failover_only_before_bytes_and_stream_is_final() {
        // A 在应答头阶段就 503：换成员，B 的流照常透传。
        let a = fake(Box::new(|_| (503, "application/json", b"{\"error\":\"down\"}".to_vec())));
        let b = fake(Box::new(|_| (200, "text/event-stream", SSE.as_bytes().to_vec())));
        let (gw, sb) = sandbox(&two_member_world(&a, &b, "order"));
        let reply = forward(&gw, &[], &chat_request("g", true));
        assert!(matches!(reply, Reply::Stream { status: 200, .. }));
        assert!(text_of(reply).contains("[DONE]"));
        drop(sb);
        // A 已经开流（2xx）：流就是终局，B 一次都不碰。
        let a = fake(Box::new(|_| (200, "text/event-stream", SSE.as_bytes().to_vec())));
        let b = fake(Box::new(|_| (200, "application/json", ok_chat("never"))));
        let (gw, _sb) = sandbox(&two_member_world(&a, &b, "order"));
        let reply = forward(&gw, &[], &chat_request("g", true));
        assert!(matches!(reply, Reply::Stream { status: 200, .. }));
        assert_eq!(b.hits.load(Ordering::SeqCst), 0);
    }

    fn big_image() -> String {
        format!("data:image/png;base64,{}", "A".repeat(6000))
    }

    #[test]
    fn chat_streaming_413_is_refit_and_comes_back_as_a_stream() {
        // 体里还带图就 413；丢掉最老图后流式 200。
        let v = fake(Box::new(|seen| {
            if String::from_utf8_lossy(&seen.body).contains("AAAAAAAA") {
                (413, "application/json", b"{\"error\":\"request body too large\"}".to_vec())
            } else {
                (200, "text/event-stream", SSE.as_bytes().to_vec())
            }
        }));
        let (gw, _sb) = sandbox(
            &json!({"providers": [{"id": "pv", "name": "V", "chat": format!("{}/v1", v.base)}]}).to_string(),
        );
        let body = json_body(json!({
            "model": "pv/m", "stream": true,
            "messages": [
                {"role": "user", "content": [{"type": "image_url", "image_url": {"url": big_image()}}, {"type": "text", "text": "old"}]},
                {"role": "assistant", "content": "ok"},
                {"role": "user", "content": "latest"},
            ]
        }));
        let reply = forward(&gw, &[], &body);
        assert!(matches!(reply, Reply::Stream { status: 200, .. }), "refit should return the stream");
        let bodies = v.bodies();
        assert_eq!(bodies.len(), 2);
        let second = bodies[1].to_string();
        assert!(!second.contains("AAAAAAAA"));
        assert!(second.contains("image left out"));
        // 占位是合法的 chat 文本块，不是裸字符串。
        assert_eq!(bodies[1]["messages"][0]["content"][0]["type"], "text");
    }

    #[test]
    fn responses_passthrough_refits_input_image_bodies() {
        let v = fake(Box::new(|seen| {
            if String::from_utf8_lossy(&seen.body).contains("AAAAAAAA") {
                (413, "application/json", b"{\"error\":{\"message\":\"Request body is too large\"}}".to_vec())
            } else {
                (200, "application/json", b"{\"id\":\"resp_1\",\"object\":\"response\",\"status\":\"completed\",\"output\":[]}".to_vec())
            }
        }));
        let (gw, _sb) = sandbox(
            &json!({"providers": [{"id": "pv", "name": "V", "responses": format!("{}/v1", v.base)}]}).to_string(),
        );
        let body = json_body(json!({
            "model": "pv/m",
            "input": [
                {"type": "message", "role": "user", "content": [
                    {"type": "input_image", "image_url": big_image()},
                    {"type": "input_text", "text": "old"}]},
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "ok"}]},
                {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "latest"}]},
            ]
        }));
        let reply = super::super::responses::handle(&gw, &body);
        assert_eq!(status_of(&reply), 200, "refit should have rescued the request");
        let bodies = v.bodies();
        assert_eq!(bodies.len(), 2);
        assert!(!bodies[1].to_string().contains("AAAAAAAA"));
        assert_eq!(bodies[1]["input"][0]["content"][0]["type"], "input_text");
        assert!(bodies[1]["input"][0]["content"][0]["text"].as_str().unwrap().contains("image left out"));
        // 学到的 cap 让后续同厂商请求先合身再发（不再吃一次拒答）。
        let before = v.hits.load(Ordering::SeqCst);
        let again = super::super::responses::handle(&gw, &body);
        assert_eq!(status_of(&again), 200);
        assert_eq!(v.hits.load(Ordering::SeqCst) - before, 1, "prefit should avoid the refusal");
    }

    #[test]
    fn anthropic_direct_refits_image_blocks_including_tool_results() {
        let v = fake(Box::new(|seen| {
            if String::from_utf8_lossy(&seen.body).contains("AAAAAAAA") {
                (413, "application/json", b"{\"type\":\"error\",\"error\":{\"type\":\"request_too_large\",\"message\":\"Request exceeds the maximum size\"}}".to_vec())
            } else {
                (200, "application/json", b"{\"id\":\"msg_1\",\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"ok\"}],\"stop_reason\":\"end_turn\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}".to_vec())
            }
        }));
        let (gw, _sb) = sandbox(
            &json!({"providers": [{"id": "pv", "name": "V", "anthropic": v.base.clone()}]}).to_string(),
        );
        let img = |data: String| json!({"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": data}});
        let body = json_body(json!({
            "model": "pv/m", "max_tokens": 10,
            "messages": [
                {"role": "user", "content": [img("A".repeat(6000)), {"type": "text", "text": "old"}]},
                {"role": "assistant", "content": [{"type": "tool_use", "id": "t1", "name": "shot", "input": {}}]},
                {"role": "user", "content": [{"type": "tool_result", "tool_use_id": "t1", "content": [img("B".repeat(6000))]}]},
                {"role": "assistant", "content": "seen"},
                {"role": "user", "content": "latest"},
            ]
        }));
        let reply = super::super::messages::handle(&gw, &body);
        assert_eq!(status_of(&reply), 200);
        let bodies = v.bodies();
        assert!(bodies.len() >= 2);
        let last = bodies.last().unwrap().to_string();
        assert!(!last.contains("AAAAAAAA"));
        assert!(last.contains("image left out"));
    }

    #[test]
    fn responses_reasoning_items_are_replayed_to_deepseek_and_dropped_elsewhere() {
        let make = |model: &str| {
            json_body(json!({
                "model": model,
                "input": [
                    {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "q"}]},
                    {"type": "reasoning", "id": "rs_1", "summary": [{"type": "summary_text", "text": "thought A"}]},
                    {"type": "function_call", "call_id": "c1", "name": "f", "arguments": "{}"},
                    {"type": "function_call_output", "call_id": "c1", "output": "r"},
                    {"type": "reasoning", "id": "rs_2", "summary": [], "content": [{"type": "reasoning_text", "text": "thought B"}]},
                    {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "done"}]},
                    {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "next"}]},
                    {"type": "reasoning", "id": "rs_3", "encrypted_content": "gAAAA", "summary": []},
                ]
            }))
        };
        let v = fake(Box::new(|_| (200, "application/json", ok_chat("ok"))));
        let (gw, _sb) = sandbox(
            &json!({"providers": [{"id": "pv", "name": "V", "chat": format!("{}/v1", v.base)}]}).to_string(),
        );
        let reply = super::super::responses::handle(&gw, &make("pv/deepseek-v4"));
        assert_eq!(status_of(&reply), 200);
        let sent = &v.bodies()[0];
        let msgs = sent["messages"].as_array().unwrap();
        let calls = msgs.iter().find(|m| m["tool_calls"].is_array()).unwrap();
        assert_eq!(calls["reasoning_content"], "thought A");
        let done = msgs.iter().find(|m| m["content"] == "done").unwrap();
        assert_eq!(done["reasoning_content"], "thought B");
        assert!(!sent.to_string().contains(THINK_KEY));
        // 尾部只有加密思考的 reasoning item 不会变成空 assistant 消息。
        assert_eq!(msgs.last().unwrap()["role"], "user");
        // 非 DeepSeek 模型：不回放（也没有任何暂存键漏出去）。
        let reply = super::super::responses::handle(&gw, &make("pv/gpt-x"));
        assert_eq!(status_of(&reply), 200);
        let sent = &v.bodies()[1];
        assert!(!sent.to_string().contains("reasoning_content"));
        assert!(!sent.to_string().contains(THINK_KEY));
    }

    #[test]
    fn max_tokens_field_is_renamed_when_the_vendor_asks_and_remembered() {
        let v = fake(Box::new(|seen| {
            if String::from_utf8_lossy(&seen.body).contains("\"max_tokens\"") {
                (
                    400,
                    "application/json",
                    b"{\"error\":{\"message\":\"Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.\"}}".to_vec(),
                )
            } else {
                (200, "application/json", ok_chat("ok"))
            }
        }));
        let (gw, _sb) = sandbox(
            &json!({"providers": [{"id": "pv-tokfield", "name": "V", "chat": format!("{}/v1", v.base)}]}).to_string(),
        );
        let body = json_body(json!({"model": "pv-tokfield/m", "max_tokens": 5, "messages": [{"role": "user", "content": "hi"}]}));
        assert_eq!(status_of(&forward(&gw, &[], &body)), 200);
        assert_eq!(v.hits.load(Ordering::SeqCst), 2);
        assert_eq!(v.bodies()[1]["max_completion_tokens"], 5);
        // 记住了：下一次直接用对的字段，不再吃 400。
        assert_eq!(status_of(&forward(&gw, &[], &body)), 200);
        assert_eq!(v.hits.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn weighted_group_spreads_requests_over_members() {
        let a = fake(Box::new(|_| (200, "application/json", ok_chat("a"))));
        let b = fake(Box::new(|_| (200, "application/json", ok_chat("b"))));
        let world = json!({
            "providers": [
                {"id": "pa", "name": "A", "chat": format!("{}/v1", a.base)},
                {"id": "pb", "name": "B", "chat": format!("{}/v1", b.base)},
            ],
            "groups": [{"id": "w-route", "name": "w-route", "members": ["pa/ma", "pb/mb"],
                        "routing": "weight", "weights": {"pa/ma": 3}}],
        })
        .to_string();
        let (gw, _sb) = sandbox(&world);
        for _ in 0..8 {
            assert_eq!(status_of(&forward(&gw, &[], &chat_request("w-route", false))), 200);
        }
        assert_eq!(a.hits.load(Ordering::SeqCst), 6);
        assert_eq!(b.hits.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn member_effort_is_applied_per_candidate_on_failover() {
        // 成员 A 固定 low、成员 B 固定 max：A 挂了转 B 时请求带的是 B 自己的强度。
        let a = fake(Box::new(|_| (503, "application/json", b"{\"error\":\"down\"}".to_vec())));
        let b = fake(Box::new(|_| (200, "application/json", ok_chat("ok"))));
        let world = json!({
            "providers": [
                {"id": "pa", "name": "A", "chat": format!("{}/v1", a.base)},
                {"id": "pb", "name": "B", "chat": format!("{}/v1", b.base)},
            ],
            "groups": [{"id": "g", "name": "g", "members": ["pa/ma:low", "pb/mb:max"], "routing": "order"}],
        })
        .to_string();
        let (gw, _sb) = sandbox(&world);
        let body = json_body(json!({"model": "g", "reasoning_effort": "high", "messages": [{"role": "user", "content": "hi"}]}));
        assert_eq!(status_of(&forward(&gw, &[], &body)), 200);
        assert_eq!(a.bodies()[0]["reasoning_effort"], "low");
        assert_eq!(b.bodies()[0]["reasoning_effort"], "max");
        assert_eq!(b.bodies()[0]["model"], "mb");
    }

    // ----- D7/D10（批 3：流可靠性）-------------------------------------------

    /// 掐流的时点。
    enum CutAt {
        /// 一条完整行都没给（`data: {"partial` 没有换行就断）。
        Before,
        /// 给了两条完整事件后断（没有 [DONE]）。
        After,
    }

    /// 掐流假厂商：应答 200 text/event-stream 后在指定点毁掉连接（上游
    /// 中途死亡的最小模拟；`fake()` 只会写 Content-Length 的完整应答，表达
    /// 不了「半截流」）。
    fn cut_fake(at: CutAt) -> Fake {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (h, s) = (hits.clone(), seen.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let mut reader = BufReader::new(match stream.try_clone() {
                    Ok(x) => x,
                    Err(_) => continue,
                });
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut length = 0usize;
                loop {
                    let mut l = String::new();
                    if reader.read_line(&mut l).unwrap_or(0) == 0 {
                        break;
                    }
                    let t = l.trim_end().to_ascii_lowercase();
                    if t.is_empty() {
                        break;
                    }
                    if let Some(v) = t.strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                if length > 0 {
                    let _ = reader.read_exact(&mut body);
                }
                h.fetch_add(1, Ordering::SeqCst);
                s.lock().unwrap().push(Seen { body });
                let mut stream = stream;
                let _ = stream.write_all(
                    b"HTTP/1.1 200 X\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                );
                match at {
                    CutAt::Before => {
                        // 半截 data 行，连换行都没有。
                        let _ = stream.write_all(b"data: {\"partial");
                    }
                    CutAt::After => {
                        let _ = stream.write_all(
                            b"data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"}}]}\n\n",
                        );
                        let _ = stream.write_all(
                            b"data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial-\"}}]}\n\n",
                        );
                    }
                }
                let _ = stream.flush();
                drop(stream); // 掐
            }
        });
        Fake { base: format!("http://127.0.0.1:{port}"), hits, seen }
    }

    /// 流被掐在第一条完整行之前：扣流首生效——客户端一个字节都没收到，
    /// 静默换下一个候选重发整轮，拿到完整流。
    #[test]
    fn stream_cut_before_first_line_fails_over_to_next_member() {
        let a = cut_fake(CutAt::Before);
        let b = fake(Box::new(|_| (200, "text/event-stream", SSE.as_bytes().to_vec())));
        let (gw, _sb) = sandbox(&two_member_world(&a, &b, "order"));
        let reply = forward(&gw, &[], &chat_request("g", true));
        assert!(matches!(reply, Reply::Stream { status: 200, .. }), "cut head must fail over to a stream");
        let text = text_of(reply);
        assert!(text.contains("hi"), "failover stream lost content: {text}");
        assert!(text.contains("[DONE]"), "failover stream must end properly: {text}");
        assert_eq!(a.hits.load(Ordering::SeqCst), 1);
        assert_eq!(b.hits.load(Ordering::SeqCst), 1);
    }

    /// 流首放行后断流且没有候选可换：ClosingReader 补显式 error 帧 +
    /// [DONE]——不许裸 EOF（合同 D7 的可判定收尾）。
    #[test]
    fn stream_cut_after_start_ends_with_explicit_error_and_done() {
        let a = cut_fake(CutAt::After);
        let (gw, _sb) = sandbox(
            &json!({"providers": [{"id": "pa", "name": "A", "chat": format!("{}/v1", a.base)}]}).to_string(),
        );
        let reply = forward(&gw, &[], &chat_request("pa/ma", true));
        assert!(matches!(reply, Reply::Stream { status: 200, .. }));
        let text = text_of(reply);
        assert!(text.contains("partial-"), "forwarded events lost: {text}");
        assert!(text.contains("\"error\""), "cut stream must end with an error frame: {text}");
        assert!(text.ends_with("data: [DONE]\n\n"), "error ending must close with [DONE]: {text}");
    }

    /// 死在流首之前且没有候选：502 可判定错误（不是把客户端挂着等 EOF）。
    #[test]
    fn stream_dead_before_first_line_with_no_candidate_is_a_502() {
        let a = cut_fake(CutAt::Before);
        let (gw, _sb) = sandbox(
            &json!({"providers": [{"id": "pa", "name": "A", "chat": format!("{}/v1", a.base)}]}).to_string(),
        );
        let reply = forward(&gw, &[], &chat_request("pa/ma", true));
        assert_eq!(status_of(&reply), 502);
        assert!(text_of(reply).contains("before its first line"));
    }

    /// 非流式 200 但体是半截 JSON：换下一个候选；全断时 502。
    #[test]
    fn non_stream_truncated_json_fails_over_then_errors() {
        let truncated = b"{\"id\":\"x\",\"choices\":[{\"index\":0,".to_vec();
        let a = fake(Box::new(move |_| (200, "application/json", truncated.clone())));
        let b = fake(Box::new(|_| (200, "application/json", ok_chat("from b"))));
        let (gw, sb) = sandbox(&two_member_world(&a, &b, "order"));
        let reply = forward(&gw, &[], &chat_request("g", false));
        assert_eq!(status_of(&reply), 200);
        assert!(text_of(reply).contains("from b"));
        assert_eq!(a.hits.load(Ordering::SeqCst), 1);
        assert_eq!(b.hits.load(Ordering::SeqCst), 1);
        drop(sb);
        let a = fake(Box::new(|_| (200, "application/json", b"{\"partial".to_vec())));
        let (gw, _sb) = sandbox(
            &json!({"providers": [{"id": "pa", "name": "A", "chat": format!("{}/v1", a.base)}]}).to_string(),
        );
        let reply = forward(&gw, &[], &chat_request("pa/ma", false));
        assert_eq!(status_of(&reply), 502);
        assert!(text_of(reply).contains("whole JSON"));
    }

    /// D10：直通 tool_calls 的零分数丢弃——流式分片（数字在分片中间被切）
    /// 与非流式整块都要。
    #[test]
    fn relay_sheds_zero_fractions_on_direct_tool_calls() {
        let tool_sse = concat!(
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"city\\\":\\\"Pa\"}}]}}]}\n\n",
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"ris\\\",\\\"yield_ms\\\":2500.0}\"}}]}}]}\n\n",
            "data: [DONE]\n\n",
        );
        let v = fake(Box::new(|_| (200, "text/event-stream", tool_sse.as_bytes().to_vec())));
        let (gw, _sb) = sandbox(
            &json!({"providers": [{"id": "pv", "name": "V", "chat": format!("{}/v1", v.base)}]}).to_string(),
        );
        let reply = forward(&gw, &[], &chat_request("pv/m", true));
        let text = text_of(reply);
        assert!(text.contains("\\\"yield_ms\\\":2500}"), "streamed fraction must be shed: {text}");
        assert!(!text.contains("2500.0"), "streamed fraction must not survive: {text}");
        assert!(text.ends_with("data: [DONE]\n\n"), "clean stream keeps its own ending: {text}");
    }

    /// D10 非流式半边：整块 JSON 应答的零分数丢弃。
    #[test]
    fn whole_reply_fraction_is_shed_on_non_stream() {
        let whole = serde_json::to_vec(&json!({
            "id": "c", "object": "chat.completion", "model": "m",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": null,
                "tool_calls": [{"id": "c1", "type": "function",
                    "function": {"name": "get_weather", "arguments": "{\"city\":\"Paris\",\"yield_ms\":2500.0}"}}]},
                "finish_reason": "tool_calls"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}
        }))
        .unwrap();
        let v = fake(Box::new(move |_| (200, "application/json", whole.clone())));
        let (gw, _sb) = sandbox(
            &json!({"providers": [{"id": "pv", "name": "V", "chat": format!("{}/v1", v.base)}]}).to_string(),
        );
        let reply = forward(&gw, &[], &chat_request("pv/m", false));
        let text = text_of(reply);
        assert!(text.contains("\\\"yield_ms\\\":2500"), "whole-reply fraction must be shed: {text}");
        assert!(!text.contains("2500.0"), "whole-reply fraction must not survive: {text}");
    }

    /// 休息记忆：5xx 的成员 60s 内被跳过；全在休息则无视（照发）。
    #[test]
    fn resting_member_is_skipped_until_all_rest() {
        let a = fake(Box::new(|_| (503, "application/json", b"{\"error\":\"down\"}".to_vec())));
        let b = fake(Box::new(|_| (200, "application/json", ok_chat("from b"))));
        let world = json!({
            "providers": [
                {"id": "pa", "name": "A", "chat": format!("{}/v1", a.base)},
                {"id": "pb", "name": "B", "chat": format!("{}/v1", b.base)},
            ],
            "groups": [
                {"id": "g", "name": "g", "members": ["pa/ma", "pb/mb"], "routing": "order"},
                {"id": "solo", "name": "solo", "members": ["pa/ma"], "routing": "order"},
            ],
        })
        .to_string();
        let (gw, _sb) = sandbox(&world);
        // 第 1 发：a 503 → 转 b；a 进休息。
        assert_eq!(status_of(&forward(&gw, &[], &chat_request("g", false))), 200);
        assert_eq!(a.hits.load(Ordering::SeqCst), 1);
        // 第 2 发：a 在休息 → 直接打 b，a 不再挨打。
        assert_eq!(status_of(&forward(&gw, &[], &chat_request("g", false))), 200);
        assert_eq!(a.hits.load(Ordering::SeqCst), 1, "resting member must be skipped");
        assert!(rest_left("pa").is_some());
        assert_eq!(rest_left("pb"), None, "the member that answered is awake");
        // 第 3 发：单成员组只剩休息中的 a → 全在休息，无视照发。
        assert_eq!(status_of(&forward(&gw, &[], &chat_request("solo", false))), 503);
        assert_eq!(a.hits.load(Ordering::SeqCst), 2, "all-resting must ignore the rest");
    }

    /// [DONE] 匹配器：行首锚定、容忍空格与 \r、行中/后缀垃圾不算。
    #[test]
    fn done_matcher_is_line_anchored() {
        let mut m = DoneMatcher::default();
        for b in b"data: [DONE]\n" {
            if *b == b'\n' {
                assert!(m.end_of_line());
            } else {
                m.byte(*b);
            }
        }
        for line in ["data:[DONE]\r\n", "data: [DONE] \n", "  data: [DONE]\n"] {
            let mut m = DoneMatcher::default();
            for b in line.as_bytes() {
                if *b == b'\n' {
                    assert!(m.end_of_line(), "{line} must hit");
                } else {
                    m.byte(*b);
                }
            }
        }
        for line in ["data: {\"x\":1}\n", "data: [DONE]x\n", "xxdata: [DONE]\n", "my data: [DONE]\n", "\n"] {
            let mut m = DoneMatcher::default();
            for b in line.as_bytes() {
                if *b == b'\n' {
                    assert!(!m.end_of_line(), "{line} must not hit");
                } else {
                    m.byte(*b);
                }
            }
        }
    }
}
