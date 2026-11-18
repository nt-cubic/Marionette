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
//! - **断流续答**（continuation，Go continuation.go 的直通半边）：流首放行
//!   后中途断流且客户端已拿到正文时，[`ContinuationReader`] 用已发正文当
//!   prefill（DeepSeek prefix / Kimi partial 的厂商自家模式）向同一候选重发，
//!   续写拼回同一条流（回声剔除、思考不重放）；续不了才走可判定收尾。
//! - **直通改写**（D10）：流输出包 `chattidy::ChattidyReader`，非流式整块
//!   过 `chattidy::mend_whole_reply`——工具参数零分数（2500.0→2500）、
//!   usage 口径、空名工具剔除、同帧同 index 合并（详见 chattidy.rs）。
//! - **休息记忆**：5xx/429/断流的候选 60s 内跳过（全在休息则无视）；zcode
//!   trial 拒答分级——只 rest 该 `<model>-Trial` 条目（模型级 key
//!   `provider::model`，判定在 zcode_account.rs），账号其它模型照常候选。
//!
//! 出站前的 chat 体「合身」（Go gateway.go `case provider.Chat` 那一段的
//! 子集，[`fit_for_vendor`]）：`developer`→`system`、纯文本 parts 折成字符串、
//! `max_tokens`↔`max_completion_tokens`（OpenAI 自家域名主动用后者；其它厂商
//! 400 点名另一个字段时改名重发一次并记住）、DeepSeek 系回放 `reasoning_content`
//! （translate.rs 的 responses→chat 把 reasoning item 暂存在 [`THINK_KEY`]，
//! 这里按厂商决定是回放还是丢弃）。
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value};

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
#[derive(Clone)]
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

/// attempt 的连接前准备段（纯计算，无 IO）：候选 → 出站目标 + 合身后的
/// 请求体。[`attempt`] 内部与 forward 的续答链共用——续答要在「已发出的
/// 那份体」上追加 prefill，两边必须算出字节相同的底稿。
fn prepare_outbound(
    gw: &super::Gateway,
    cand: &Candidate,
    body: &[u8],
) -> Result<(Outbound, Vec<u8>), String> {
    let out = outbound_for(gw, cand)?;
    let host = host_of(&out.url);
    let fitted = fit_for_vendor(
        &rewrite_model(body, &out.bare_model),
        &host,
        &cand.model,
        token_field_for(&out.provider_id, &host),
    );
    // cap 已知时先合身再发（免吃一次拒答）。
    let current = bodycap::prefit(gw, &out.provider_id, &fitted).unwrap_or(fitted);
    Ok((out, current))
}

/// 打一个候选成员（`body` 是 chat 体，`model` 字段任意——这里换成厂商裸名）。
/// 流式 2xx 立刻交出 `Stream`（终局）；错误应答读全后先过 bodycap（字节上限
/// refit）与回复长度字段自适应，最后按 `groups::retryable` 决定是否换成员。
pub(crate) fn attempt(gw: &super::Gateway, cand: &Candidate, body: &[u8]) -> Attempt {
    let (out, mut current) = match prepare_outbound(gw, cand, body) {
        Ok(x) => x,
        Err(e) => return Attempt::Retry(Reply::error(404, &e)),
    };
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
// D7：断流可判定收尾 + 扣流首换人 + 休息记忆 + continuation 断流续答
//（Go fallback.go:2786 holdWriter + routing.go:69 served/rest + continuation.go
//  的直通半边——续答见 [`ContinuationReader`] 一节；没搬的子项见文件尾清单）
// ---------------------------------------------------------------------------

/// 流首扣留的体量上限：单行超长的厂商流（百 KB 的 arguments 分片）不再
/// 等换行，按活流放行。
const HEAD_HOLD_MAX: usize = 256 * 1024;

/// 休息时长：5xx/429/断流的候选 60s 内被候选循环跳过（zcode trial 拒答的
/// 模型级条目同窗，见 [`rest_entry`]）。
/// 与 Go 的差距（routing.go:69 `served` / restingUntil 家族）：不落盘、不分
/// 错误等级（Go 按额度耗尽/限流 Retry-After/挂了分窗口长度且逐次失败退避
/// 递增）、无 per-account 细分——provider 级仍按 id 一把抓；模型级条目目前
/// 只有 zcode trial 拒答一个写入者（Go 还有 modelNamed / PaidBesideFree /
/// modelLimitedBy 等任意厂商的模型级 rest 口，未搬）。
const REST_FOR: Duration = Duration::from_secs(60);

fn rests() -> &'static Mutex<HashMap<String, Instant>> {
    static RESTS: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    RESTS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 模型级休息条目的 key：`<provider_id>::<model>`。Go 的对应是
/// `candidate.restID()` = restKey()+"/"+model（fallback.go:211-213）；Rust 的
/// rest 表本来就按 provider id 键、目录 id 又是 `provider/model` 复合写法，
/// 用 `::` 分隔与两者都无歧义（engine 的 provider id 生成只出小写字母/
/// 数字/横线）。目前唯一的写入者是 zcode trial 拒答的 rest 分级。
fn entry_rest_key(provider_id: &str, model: &str) -> String {
    format!("{provider_id}::{model}")
}

fn rest_key_now(key: String) {
    if let Ok(mut map) = rests().lock() {
        map.retain(|_, until| *until > Instant::now());
        map.insert(key, Instant::now() + REST_FOR);
    }
}

/// 标记一个来源休息（5xx / 429 / 断流）：provider 一把抓。
fn rest_provider(provider_id: &str) {
    rest_key_now(provider_id.to_string());
}

/// 只标记一个模型级条目休息（zcode trial 拒答的分级，Go routing.go:1832-1836
/// `id = c.restID()`）：同 provider 的其它模型照常候选。窗口仍是统一的
/// [`REST_FOR`]——Go 按错误等级分窗口长度的 restingUntil 家族没搬（见文件尾）。
fn rest_entry(provider_id: &str, model: &str) {
    rest_key_now(entry_rest_key(provider_id, model));
}

/// 来源答上来了（流首放行 / 整块 JSON 解析成功）：清掉它的休息——provider
/// 级与当前模型的条目级一起清（Go servedCandidate 对 restID 也 clearRest，
/// routing.go:91-97）。
fn wake(provider_id: &str, model: &str) {
    if let Ok(mut map) = rests().lock() {
        map.remove(provider_id);
        map.remove(&entry_rest_key(provider_id, model));
    }
}

/// 候选是否在休息：provider 级条目或它自己模型的条目任一在休即算。
fn resting(provider_id: &str, model: &str) -> bool {
    rest_left(provider_id).is_some() || rest_left(&entry_rest_key(provider_id, model)).is_some()
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
/// `note` 是续答层（[`ContinuationReader`]）走到头时留下的终局文案——
/// Go cutMidReply 的「断它的事」口径；None = 基线文案。
struct ClosingReader {
    provider: String,
    inner: Box<dyn Read + Send>,
    matcher: DoneMatcher,
    seen_done: bool,
    finished: bool,
    pending: Vec<u8>,
    at: usize,
    note: Arc<Mutex<Option<String>>>,
}

impl ClosingReader {
    fn new(provider: String, inner: Box<dyn Read + Send>, note: Arc<Mutex<Option<String>>>) -> ClosingReader {
        ClosingReader {
            provider,
            inner,
            matcher: DoneMatcher::default(),
            seen_done: false,
            finished: false,
            pending: Vec::new(),
            at: 0,
            note,
        }
    }

    /// 终局：没见过 [DONE] 就补显式收尾（对话流形态：error 帧 + [DONE]）。
    fn finish(&mut self) {
        self.finished = true;
        if !self.seen_done {
            let note = self
                .note
                .lock()
                .ok()
                .and_then(|g| g.clone())
                .unwrap_or_else(|| format!("{} stream ended without its terminator", self.provider));
            let frame = format!(
                "data: {{\"error\":{{\"message\":\"{}\",\"type\":\"engine_error\",\"code\":null}}}}\n\ndata: [DONE]\n\n",
                note
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

// ---------------------------------------------------------------------------
// continuation 断流续答（Go continuation.go 的 chat 直通半边：:364 again /
// :378 prefillText / :455 streamTranslated 与字节级直通能共存的部分）
// ---------------------------------------------------------------------------
//
// 上游在流中途死亡（EOF/读错误、[DONE] 与 finish_reason 都没说）且客户端
// 已拿到正文时，把「已发出的正文」当 prefill 追加成最后一条 assistant 消息，
// 向**同一候选**重发一次（Go 的 again 循环不换成员——组换人是 #733 的
// mid-reply 错误事件路径，靠错误帧的 saw_error 守卫等效挡住），把续写拼回
// 同一条流。可续的前提是上游自己能「接着写」（Go prefillHow 的 chat 半边
// continuation.go:298）：DeepSeek 的 prefix / Kimi 的 partial（chat_prefill，
// 本机/LAN 中转带这两个模型名的照认）；普通 Chat 上游会从头重答——拼接是
// 改写过的重答，所以不续（Go TestStreamCutOnPlainChatEndsAsBefore）。
//
// 与 Go 的刻意差异（字节直通做不到的，见文件尾差距清单）：
// - 续答 try 的思考不重放（Go emit 的 KThink 分支）靠**改写流**实现；回声
//   整段剔除走 unecho（continuation.go:433 1:1）。
// - usage 台账：Go 把各 try 折进一行（Deferred→billed，claude_subscription.go
//   :5139 after）；Rust 的 TapReader 按「每桶最后一次非零上报」收口（客户端
//   流原样透传），断掉的 try 对同一桶的更早上报由续答层补一行——**总额同
//   Go 的 billed 口径，行数形态是「一次出站调用一行」**（usage_ledger.rs
//   差距清单的既有口径）。

/// 续答次数上限（Go streamRetries，continuation.go:37）：原 try 之后最多再
/// 问两次（总出站调用 ≤ 1+STREAM_RETRIES），用尽以断流自己的错误收尾。
const STREAM_RETRIES: u32 = 2;

/// 首次续答前的停顿（Go retryPause，fallback.go:2333 `time.Second`，按
/// `retryPause << again` 逐次翻倍）。测试里换成 1ms（沙箱起手钉住）。
static RETRY_PAUSE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1_000);

fn retry_pause(tries: u32) -> Duration {
    use std::sync::atomic::Ordering;
    let base = RETRY_PAUSE_MS.load(Ordering::Relaxed);
    Duration::from_millis(base << (tries.saturating_sub(1).min(4)))
}

/// Go chatPrefill（continuation.go:315）：chat 上游在自家 prefill 模式里的
/// 接续方式——DeepSeek 的 prefix（自家 API，路径在 /beta）或 Kimi 的
/// partial。中转只在本机/LAN（localhost/`.local`/回环/私网 IP）才被认定
/// 直连厂商照认；其它中转与公有云一律 ""（从头重答，不续）。
fn chat_prefill(host: &str, model: &str) -> &'static str {
    let h = host.to_ascii_lowercase();
    if h == "api.deepseek.com" || h.ends_with(".deepseek.com") {
        return "prefix";
    }
    if h.contains("moonshot") || h.contains("kimi") {
        return "partial";
    }
    // SplitHostPort + Trim "[]"（continuation.go:323-326）：host_of 已剥过
    // 端口与凭据，这里只兜 IPv6 字面量的残余括号。
    let h = h.split(':').next().unwrap_or(&h).trim_matches(['[', ']']);
    let local = h == "localhost"
        || h.ends_with(".local")
        || h
            .parse::<std::net::IpAddr>()
            .map(|ip| match ip {
                std::net::IpAddr::V4(v4) => v4.is_loopback() || v4.is_private(),
                std::net::IpAddr::V6(v6) => v6.is_loopback(),
            })
            .unwrap_or(false);
    if !local {
        return "";
    }
    let m = model.to_ascii_lowercase();
    if m.contains("deepseek") {
        return "prefix";
    }
    if m.contains("kimi") {
        return "partial";
    }
    ""
}

/// prefill 拒答记忆（Go prefill.go:41 prefillRefused + gateway.go:8307
/// unfit 的内存版，key = provider+model）：模型拒过一次（400/422 + 拒答词）
/// 就不再问它接着写——每次续答都白吃一次拒答还白等一场。
fn prefill_refusals() -> &'static Mutex<std::collections::HashSet<String>> {
    static SET: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

fn prefill_refusal_key(provider: &str, model: &str) -> String {
    format!("{provider}\x00prefill\x00{model}")
}

fn prefill_refused_before(provider: &str, model: &str) -> bool {
    prefill_refusals()
        .lock()
        .map(|set| set.contains(&prefill_refusal_key(provider, model)))
        .unwrap_or(false)
}

fn note_prefill_refused(provider: &str, model: &str) {
    if let Ok(mut set) = prefill_refusals().lock() {
        set.insert(prefill_refusal_key(provider, model));
    }
}

/// Go refusesPrefill（prefill.go:44）＝ badRequest（400/422，gateway.go:9428）
/// + 拒答词（prefill.go:37 prefillRefusal 的子串半边——零依赖不用正则，
/// 三条字面词大小写不敏感地找）。
fn refuses_prefill(status: u16, body: &[u8]) -> bool {
    if status != 400 && status != 422 {
        return false;
    }
    let text = String::from_utf8_lossy(body).to_ascii_lowercase();
    text.contains("does not support prefill")
        || text.contains("does not support assistant message prefill")
        || text.contains("requests ending with a model turn are not supported")
}

/// 续答失败终局文案里的厂商错误正文（Go explained 的最瘦版）：error.message
/// / message / error 三处能读就读，读不到 None。
fn vendor_error_text(body: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(body).ok()?;
    let text = v
        .pointer("/error/message")
        .or_else(|| v.get("message"))
        .and_then(|m| m.as_str())
        .map(String::from)
        .or_else(|| v.get("error").and_then(|e| e.as_str()).map(String::from))?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// unecho（Go continuation.go:433 1:1）：续答里对 prefill 全文的原样回声整段
/// 剔除——客户端已经有它了。没读出是不是回声前先持住，读出来（前缀断开）
/// 一次放行全部。
#[derive(Default)]
struct Unecho {
    echo: String,
    seen: String,
}

impl Unecho {
    fn strip(&mut self, s: &str) -> String {
        if self.echo.is_empty() {
            return s.to_string();
        }
        self.seen.push_str(s);
        if self.seen.len() < self.echo.len() && self.echo.starts_with(&self.seen) {
            return String::new(); // 还可能是回声：持住
        }
        let out = if self.seen.starts_with(&self.echo) {
            self.seen[self.echo.len()..].to_string()
        } else {
            self.seen.clone()
        };
        self.seen.clear();
        self.echo.clear();
        out
    }
}

/// usage 四桶的原生形态（chat 方言读取序，镜像 usage_ledger::chat_buckets
/// 的口径——续答补账行要与 TapReader 的收口互补，提取序必须一字不差）。
/// 位置：0=in 1=out 2=cache_read 3=cache_write；None = 厂商没说。
type UsageBuckets = [Option<u64>; 4];

fn chat_usage_buckets(u: &Value) -> UsageBuckets {
    let g = |k: &str| u.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
    let prompt = g("prompt_tokens");
    let details = u.get("prompt_tokens_details").and_then(|d| d.as_object());
    let dn = |k: &str| details.and_then(|d| d.get(k)).and_then(|v| v.as_u64()).unwrap_or(0);
    let mut read = dn("cached_tokens");
    let mut write = dn("cache_write_tokens");
    if read == 0 {
        for k in ["prompt_cache_hit_tokens", "cached_tokens", "num_cached_tokens"] {
            let n = g(k);
            if n != 0 {
                read = n;
                break;
            }
        }
    }
    let mut whole = true; // prompt_tokens 含缓存（OpenAI 口径）
    if read == 0 && write == 0 {
        read = g("cache_read_input_tokens");
        write = g("cache_creation_input_tokens");
        whole = prompt >= read + write;
    }
    let input = if whole { prompt.saturating_sub(read + write) } else { prompt };
    let opt = |n: u64| (n > 0).then_some(n);
    [opt(input), opt(g("completion_tokens")), opt(read), opt(write)]
}

/// 续答的常量配置（链构建时定死；reader 是 'static + Send，拿不住 &Gateway）。
struct ContinuationCfg {
    /// 出站目标（原 try 发出去的那份；prefix 模式在续答时换 /beta 路径）。
    out: Outbound,
    /// prefill 模式（chat_prefill；"" = 无续答能力，reader 走纯直通零解析）。
    mode: &'static str,
    /// 已发出的合身体（模型已换裸名、厂商合身已过）——prefill 的底稿。
    base_body: Vec<u8>,
    /// 拒答记忆与补账行的归属（候选的 provider id 与裸模型名）。
    provider: String,
    model: String,
    /// 台账与记账元数据（补账行同 tap 行的归属口径）。
    ledger: super::usage_ledger::UsageLedger,
    log: super::usage_ledger::TurnLog,
    status: u16,
    /// 终局文案（ClosingReader 收尾时取；None = 基线文案）。
    terminal: Arc<Mutex<Option<String>>>,
}

/// 续答阶段。
enum Phase {
    /// 正在转发当前 try 的流。
    Relay,
    /// 断流已判定：睡眠后重发 prefill 请求（上限与续答失败裁决都在这）。
    Pause,
    /// 终局：EOF/错误交给上层收尾。
    Done,
}

enum Ask {
    /// 续答流接上（成为新的 inner）。
    Stream(Box<dyn Read + Send>),
    /// 这次问失败但可以再问（连接失败 / 5xx / 429 / 非 SSE 200——Go again()
    /// 的放行半边），下一圈再睡再问。
    Retry,
    /// 走到头：以断流自己的错误收尾（Go: failed=cutBy；续答中途的厂商错误
    /// 只是原因，不进客户端的流）。
    Terminal(Option<String>),
}

/// 断流续答 reader（挂在直通链最底层，ChattidyReader 之下）：字节照转，
/// data 行过一遍观察器记「客户端已有什么」（正文/思考/工具调用/收尾帧/
/// usage 桶）；上游 EOF/读错误且没说过自己的收尾时，按 prefill 模式重发
/// 续答请求并把新流拼在已发字节后面。
///
/// - 无 prefill 能力（mode ==""）时是**零解析纯直通**——基线字节面不变。
/// - 有能力时逐行解析只为观察与续答改写；try0 不改写任何行（字节原样），
///   改写只发生在续答 try（摘思考、unecho 回声）。
/// - usage 台账：每 try 的非零桶记在 `reports`，流尾对「被后续 try 重新
///   上报过的桶」逐条补账（与 TapReader 的覆盖收口互补，不漏不重）。
struct ContinuationReader {
    cfg: ContinuationCfg,
    inner: Option<Box<dyn Read + Send>>,
    /// 跨 chunk 未完结行的残段（断流即丢——半截事件从未成为 SSE 事件）。
    buf: Vec<u8>,
    /// 已改写好待转发的字节。
    pending: Vec<u8>,
    at: usize,
    phase: Phase,
    /// 已发出的出站调用数（含原 try；Go again 循环的迭代序号 +1）。
    tries: u32,
    /// 当前 try 是续答（echo 抑制与思考不重放开）。
    resume: bool,
    // —— 客户端已有的内容（Go continuation.think/text/tools，跨 try 累积）——
    think: String,
    text: String,
    tools: bool,
    // —— 本 try 的观察（每 try 重置；tools 是 sticky 的）——
    seen_done: bool,
    said_finish: bool,
    saw_error: bool,
    /// 续答回声抑制的状态（arm 时 set，unecho 消费）。
    unecho: Unecho,
    /// 每 try 的非零 usage 桶（补账行与 tap 行互补）。
    reports: Vec<UsageBuckets>,
    finished: bool,
}

impl ContinuationReader {
    fn new(cfg: ContinuationCfg, inner: Box<dyn Read + Send>) -> ContinuationReader {
        ContinuationReader {
            cfg,
            inner: Some(inner),
            buf: Vec::new(),
            pending: Vec::new(),
            at: 0,
            phase: Phase::Relay,
            tries: 1,
            resume: false,
            think: String::new(),
            text: String::new(),
            tools: false,
            seen_done: false,
            said_finish: false,
            saw_error: false,
            unecho: Unecho::default(),
            reports: vec![[None; 4]],
            finished: false,
        }
    }

    /// 断流是否值得续（Go continuation.go:356 possible + again 的可续半边；
    /// 状态 400/422/404 与上限的裁决在 Pause 段做）。收尾帧/[DONE]/错误帧
    /// 已见 = 回复在流里说过它自己的事——不是「说了一半」，是基线口径的
    /// 可判定截断，续答会把整段话拼重（Go 的 errDoneSaid/errStreamCut 同旨）。
    fn cut_continuable(&self) -> bool {
        self.cfg.mode != ""
            && !prefill_refused_before(&self.cfg.provider, &self.cfg.model)
            && !self.tools
            && !self.seen_done
            && !self.said_finish
            && !self.saw_error
            && (!self.think.is_empty() || !self.text.is_empty())
    }

    /// 武装续答（Go continuation.go:369 again 的三连 + chat 版 prefill）：
    /// echo 取当前正文的 prefill 形态（:378 prefillText，尾随空白削掉——
    /// Anthropic 上游整单拒收尾随空白，客户端已有它所以不亏），回声状态清零，
    /// 新 try 的桶行开账。
    fn arm(&mut self) {
        self.resume = true;
        self.unecho.echo = self.text.trim_end().to_string();
        self.unecho.seen.clear();
        self.buf.clear(); // 半截行随断流作废
        self.reports.push([None; 4]);
    }

    /// 续答 try 起手的观察复位（tools 是 sticky 的，不重置）。
    fn new_try(&mut self) {
        self.seen_done = false;
        self.said_finish = false;
        self.saw_error = false;
        self.buf.clear();
    }

    /// 续答的出站目标：prefix 模式下 DeepSeek 的 /beta 路径（Go
    /// gateway.go:8664-8667：TrimSuffix(p.Chat, "/v1") 后拼 /beta）。
    fn continuation_out(&self) -> Outbound {
        let mut out = self.cfg.out.clone();
        if self.cfg.mode == "prefix" {
            let base = out.url.strip_suffix("/chat/completions").unwrap_or(&out.url);
            let base = base.strip_suffix("/v1").unwrap_or(base);
            out.url = format!("{base}/beta/chat/completions");
        }
        out
    }

    /// 续答请求体：已发出体 + 追加的 assistant prefill（Go continuation.go
    /// :384 request + chat.go:534-548 buildChat 的 Resume 分支——厂商自己的
    /// prefill 标记（prefix/partial）与思考回放（reasoning_content）都在
    /// 这条消息上；没有任何「接着写」的指令句，Go 的 prefill 就是那半截
    /// 正文本身，模型从它自然续起）。最后补一道 token 字段改名（attempt
    /// 在 400 里学到的名字，这里赶在出站前应用）。
    fn build_prefill_body(&self) -> Option<Vec<u8>> {
        let mut v = serde_json::from_slice::<Value>(&self.cfg.base_body).ok()?;
        let map = v.as_object_mut()?;
        let prefill = self.text.trim_end().to_string();
        let mut msg = Map::new();
        msg.insert("role".into(), json!("assistant"));
        // 空串也带（Go buildChat：无 tool_calls 时 content 恒设——纯思考
        // 断流的 prefill 是空正文 + 思考回放）。
        msg.insert("content".into(), json!(prefill));
        match self.cfg.mode {
            "prefix" => msg.insert("prefix".into(), json!(true)),
            "partial" => msg.insert("partial".into(), json!(true)),
            _ => None,
        };
        if !self.think.is_empty() {
            msg.insert("reasoning_content".into(), json!(self.think.clone()));
        }
        map.entry("messages".to_string())
            .or_insert_with(|| json!([]))
            .as_array_mut()?
            .push(Value::Object(msg));
        let bytes = serde_json::to_vec(&v).ok()?;
        let host = host_of(&self.cfg.out.url);
        Some(match token_field_for(&self.cfg.out.provider_id, &host) {
            Some(want) => with_token_field(&bytes, want).unwrap_or(bytes),
            None => bytes,
        })
    }

    /// 重发 prefill 请求（Go again 循环一体的 :585 forwardTranslated 段）。
    fn continuation_ask(&mut self) -> Ask {
        let Some(body) = self.build_prefill_body() else {
            return Ask::Terminal(None);
        };
        let out = self.continuation_out();
        match send(&out, &body) {
            // 连接失败：Go serr=err，again() 的状态门看的是应答状态（0=放行）
            Err(_) => Ask::Retry,
            Ok(resp) => {
                let status = resp.status();
                if status < 400 {
                    if resp.content_type().contains("text/event-stream") {
                        Ask::Stream(Box::new(resp.into_reader()))
                    } else {
                        // 厂商无视 stream:true（Go didNotStream → failed → again()）
                        Ask::Retry
                    }
                } else {
                    let mut vendor_body = Vec::new();
                    let _ = resp
                        .into_reader()
                        .take(1024 * 1024)
                        .read_to_end(&mut vendor_body);
                    if refuses_prefill(status, &vendor_body) {
                        // prefill.go:44 + gateway.go:8783：记 unfit，回复以
                        // 「断它的事」收尾，不是这个客户端没发起过的续答的
                        // 拒答（#1447）
                        note_prefill_refused(&self.cfg.provider, &self.cfg.model);
                        Ask::Terminal(None)
                    } else if status == 400 || status == 422 || status == 404 {
                        // again() 的状态门（continuation.go:365）
                        Ask::Terminal(vendor_error_text(&vendor_body))
                    } else {
                        Ask::Retry
                    }
                }
            }
        }
    }

    /// 终局：补账落行 + 终局文案（Go cutMidReply，gateway.go:8217；厂商错误
    /// 正文在手里就带上）。
    fn end_terminal(&mut self, vendor_text: Option<String>) {
        self.finish_extra_rows();
        self.phase = Phase::Done;
        if let Ok(mut note) = self.cfg.terminal.lock() {
            if note.is_none() {
                *note = Some(match vendor_text {
                    Some(text) => format!("{}: {}", self.cfg.provider, text),
                    None => format!("{}: connection lost mid-reply", self.cfg.provider),
                });
            }
        }
    }

    /// 补账（Go 的 billed 半边，claude_subscription.go:5139 after + ir.go
    /// u.billed()）：tap 行按「每桶最后一次非零上报」收口（客户端流原样
    /// 透传，usage_ledger::UsageTap::add 的覆盖语义），更早的 try 对**被
    /// 后续 try 重新上报过的桶**的值就是 tap 行之外的真账——每条断掉的
    /// try 一行，总额与 Go 的单行 billed 相同。
    fn finish_extra_rows(&mut self) {
        if self.finished {
            return;
        }
        self.finished = true;
        let n = self.reports.len();
        for i in 0..n.saturating_sub(1) {
            let mut row: UsageBuckets = [None; 4];
            for (b, v) in self.reports[i].iter().enumerate() {
                if v.is_some() && self.reports[i + 1..].iter().any(|r| r[b].is_some()) {
                    row[b] = *v;
                }
            }
            if row.iter().all(|x| x.is_none()) {
                continue;
            }
            let millis = self.cfg.log.began.elapsed().as_millis() as u64;
            self.cfg.ledger.record(super::usage_ledger::UsageRecord {
                ts: 0,
                agent: self.cfg.log.agent.into(),
                provider: self.cfg.provider.clone(),
                model: self.cfg.log.model.clone(),
                millis,
                status: self.cfg.status,
                rejected: false,
                session: self.cfg.log.session.clone(),
                kind: "turn".into(),
                in_tokens: row[0],
                out_tokens: row[1],
                cache_read: row[2],
                cache_write: row[3],
            });
        }
    }

    /// 喂入一段原始字节，返回可转发字节。无 prefill 能力时纯直通零解析。
    fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        if self.cfg.mode.is_empty() {
            return bytes.to_vec();
        }
        self.buf.extend_from_slice(bytes);
        let Some(cut) = self.buf.iter().rposition(|&b| b == b'\n') else {
            return Vec::new();
        };
        let tail = self.buf.split_off(cut + 1);
        let whole = std::mem::replace(&mut self.buf, tail);
        let mut out = Vec::with_capacity(whole.len());
        let mut start = 0;
        for (i, &b) in whole.iter().enumerate() {
            if b == b'\n' {
                out.extend_from_slice(&self.line(&whole[start..=i]));
                start = i + 1;
            }
        }
        out
    }

    /// 一条完整 SSE 行：观察（两态都要）+ 续答 try 的改写。解析不了的行
    /// 原样放行（keepalive 注释、[DONE]、坏行）。
    fn line(&mut self, line: &[u8]) -> Vec<u8> {
        let body_len = line.iter().rposition(|&b| b != b'\n' && b != b'\r').map_or(0, |i| i + 1);
        let (body, tail) = line.split_at(body_len);
        let Some(data) = strip_data_prefix(body) else {
            return line.to_vec();
        };
        // [DONE]（Go saidOver 的 chat 半边，continuation.go:845）：回复说过
        // 完了——不再是断流。
        if data.trim() == "[DONE]" {
            self.seen_done = true;
            return line.to_vec();
        }
        if !line_interesting(body) {
            return line.to_vec();
        }
        let Ok(mut v) = serde_json::from_str::<Value>(data) else {
            return line.to_vec();
        };
        if v.get("error").is_some() {
            // 厂商错误帧已在流里：客户端收到了失败信号，续答会把正文拼在
            // 错误后面（Go 的翻译路径把错误帧扣住不发，直通做不到——这里
            // 选择不续）。
            self.saw_error = true;
        }
        if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
            let buckets = chat_usage_buckets(u);
            if let Some(last) = self.reports.last_mut() {
                for (i, b) in buckets.iter().enumerate() {
                    if b.is_some() {
                        last[i] = *b;
                    }
                }
            }
        }
        let mut changed = false;
        if let Some(choices) = v.get_mut("choices").and_then(|c| c.as_array_mut()) {
            for choice in choices {
                if choice
                    .get("finish_reason")
                    .and_then(|f| f.as_str())
                    .is_some_and(|s| !s.is_empty())
                {
                    self.said_finish = true;
                }
                let Some(delta) = choice.get_mut("delta").and_then(|d| d.as_object_mut()) else {
                    continue;
                };
                if delta
                    .get("tool_calls")
                    .and_then(|t| t.as_array())
                    .is_some_and(|a| !a.is_empty())
                {
                    self.tools = true; // 调用一旦开始就续不了（Go cont.tools）
                }
                // 思考：续答 try 一律不重放（Go emit 的 KThink/KSig/
                // KThinkStart/KSealed 分支）；原 try 只记不改动。
                if delta.contains_key("reasoning_content") {
                    if self.resume {
                        delta.remove("reasoning_content");
                        changed = true;
                    } else if let Some(s) =
                        delta.get("reasoning_content").and_then(|r| r.as_str())
                    {
                        if !s.is_empty() {
                            self.think.push_str(s);
                        }
                    }
                }
                // 正文：续答 try 过 unecho（回声整段剔除）；累计的是客户端
                // 真正拿到的（Go add 只记 emit 过的事件）。
                if let Some(Value::String(s)) = delta.get("content") {
                    let s = s.clone();
                    if self.resume {
                        let stripped = self.unecho.strip(&s);
                        if stripped != s {
                            changed = true;
                        }
                        if stripped.is_empty() {
                            delta.remove("content");
                        } else {
                            delta.insert("content".into(), json!(stripped.clone()));
                            self.text.push_str(&stripped);
                        }
                    } else if !s.is_empty() {
                        self.text.push_str(&s);
                    }
                }
            }
        }
        if !self.resume || !changed {
            return line.to_vec(); // 原 try 与无需改写的行：字节原样
        }
        if line_is_empty_after(&v) {
            return Vec::new(); // 摘完只剩空壳：整行不发
        }
        let Ok(json) = serde_json::to_vec(&v) else {
            return line.to_vec();
        };
        let mut out = Vec::with_capacity(6 + json.len() + tail.len());
        out.extend_from_slice(b"data: ");
        out.extend_from_slice(&json);
        out.extend_from_slice(tail);
        out
    }
}

/// 行里有没有本层要观察的东西（预筛，chattidy.rs line_carries 同款思路）：
/// delta（正文/思考/调用）、usage、finish_reason、error 之外的原样放行。
fn line_interesting(line: &[u8]) -> bool {
    const NEEDLES: [&[u8]; 4] =
        [b"\"delta\"", b"\"usage\"", b"\"finish_reason\"", b"\"error\""];
    NEEDLES.iter().any(|needle| line.windows(needle.len()).any(|w| w == *needle))
}

/// `data:` 前缀剥掉（允许 `data:` 与 JSON 之间的空白）；不是 data 行返回
/// None（chattidy.rs 同款——那边的不 pub，直通链两层各有一份）。
fn strip_data_prefix(body: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(body).ok()?;
    let rest = text.trim_start();
    let data = rest.strip_prefix("data:")?;
    Some(data)
}

/// 摘除思考/回声后整行是否已无信息（delta 空壳、无 finish_reason、无
/// usage）——整行不发，不让空 chunk 噪声过 Chattidy。
fn line_is_empty_after(v: &Value) -> bool {
    if v.get("usage").is_some() {
        return false;
    }
    let Some(choices) = v.get("choices").and_then(|c| c.as_array()) else {
        return false;
    };
    choices.iter().all(|c| {
        let delta = c.get("delta").and_then(|d| d.as_object());
        let no_finish = c
            .get("finish_reason")
            .map(|f| f.is_null() || f.as_str().is_none_or(|s| s.is_empty()))
            .unwrap_or(true);
        let no_calls = delta
            .and_then(|d| d.get("tool_calls"))
            .map(|t| t.as_array().is_none_or(|a| a.is_empty()))
            .unwrap_or(true);
        delta.is_none_or(|d| d.is_empty()) && no_finish && no_calls
    })
}

impl Read for ContinuationReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        loop {
            if self.at < self.pending.len() {
                let n = buf.len().min(self.pending.len() - self.at);
                buf[..n].copy_from_slice(&self.pending[self.at..self.at + n]);
                self.at += n;
                return Ok(n);
            }
            match self.phase {
                Phase::Done => return Ok(0),
                Phase::Pause => {
                    // 上限裁决（Go again：again >= streamRetries 不再问）
                    if self.tries >= 1 + STREAM_RETRIES {
                        self.end_terminal(None);
                        return Ok(0);
                    }
                    std::thread::sleep(retry_pause(self.tries));
                    self.tries += 1;
                    match self.continuation_ask() {
                        Ask::Stream(reader) => {
                            self.inner = Some(reader);
                            self.new_try();
                            self.phase = Phase::Relay;
                        }
                        Ask::Retry => {} // 下一圈再睡再问（Go 逐次翻倍的停顿）
                        Ask::Terminal(text) => {
                            self.end_terminal(text);
                            return Ok(0);
                        }
                    }
                }
                Phase::Relay => {
                    let Some(inner) = self.inner.as_mut() else { return Ok(0) };
                    let mut chunk = [0u8; 16 * 1024];
                    match inner.read(&mut chunk) {
                        Ok(0) => {
                            if self.cut_continuable() {
                                self.arm();
                                self.phase = Phase::Pause;
                                continue;
                            }
                            // 不是可续的断流。流里说过自己的收尾（finish_reason
                            // /错误帧）或纯直通（mode==""，零观察）→ 基线文案
                            //（ClosingReader 补帧）；能观察的纯断流按 Go
                            // cutMidReply（gateway.go:8217）说「断它的事」。
                            if self.cfg.mode.is_empty() || self.said_finish || self.saw_error {
                                self.finish_extra_rows();
                                self.phase = Phase::Done;
                                return Ok(0);
                            }
                            self.end_terminal(None);
                            return Ok(0);
                        }
                        Ok(n) => {
                            self.pending = self.feed(&chunk[..n]);
                            self.at = 0; // pending 换血，下标复位（ClosingReader/Chattidy 同款）
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(e) => {
                            if self.cut_continuable() {
                                self.arm();
                                self.phase = Phase::Pause;
                                continue;
                            }
                            if self.cfg.mode.is_empty() || self.said_finish || self.saw_error {
                                self.finish_extra_rows();
                                self.phase = Phase::Done;
                                return Err(e);
                            }
                            self.end_terminal(None);
                            return Err(e);
                        }
                    }
                }
            }
        }
    }
}

impl Drop for ContinuationReader {
    fn drop(&mut self) {
        // 没读到 EOF 就被丢（客户端断开）：断掉的 try 的桶照补（tap 行由
        // TapReader::drop 落，两路互不重复）。
        self.finish_extra_rows();
    }
}

/// POST /v1/chat/completions 入口（D7 后的候选循环，替代纯头阶段换人的
/// `groups::drive`）：头阶段失败换成员（原语义保留）→ 2xx 流式扣流首 →
/// 死在流首之前换下一个重发整轮 → 流首放行即终局——中途断流先走
/// [`ContinuationReader`] 的续答（prefill 拼回，可续时客户端拿到的是一条
/// 完整回复），续不了才由 [`ClosingReader`] 补可判定收尾 → 2xx 非流式以
/// 「整块 JSON 解析成功」为合法收尾。休息中的候选跳过（全在休息则无视）。
/// 成功回合收账（capture 层 2026-11-18 接上，Go sniff.go/gateway.go:5883
/// 口径）：粘性（曾答账号）在流首放行 / 整块解析成功时就记；usage 台账行
/// 按路径落——非流式整块在手直接提桶，流式由
/// `usage_ledger::TapReader` 包在 relay 链外层，流尾收口（EOF / 断流 /
/// drop 都落，SSE 尾帧 usage 四桶照提，millis = 请求入口→收口）。
fn note_answered(
    gw: &super::Gateway,
    headers: &[(String, String)],
    body: &[u8],
    cand: &Candidate,
) -> (Option<String>, String) {
    let parsed: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let sk = super::affinity::stick_key(headers, &parsed);
    if let Some(sk) = &sk {
        gw.affinity.answered(sk, &cand.provider, &cand.model);
    }
    let asked = parsed.get("model").and_then(|m| m.as_str()).unwrap_or_default().to_string();
    (sk, asked)
}

pub fn forward(gw: &super::Gateway, headers: &[(String, String)], body: &[u8]) -> Reply {
    let began = std::time::Instant::now();
    let model = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(String::from))
        .unwrap_or_default();
    let cands = groups::plan_for(gw, &model);
    if cands.is_empty() {
        return Reply::error(404, &format!("no provider for model: {model}"));
    }
    let live: Vec<Candidate> = {
        let awake: Vec<&Candidate> = cands.iter().filter(|c| !resting(&c.provider, &c.model)).collect();
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
                        wake(&cand.provider, &cand.model);
                        let (session, asked) = note_answered(gw, headers, &body, cand);
                        // usage 捕获（Go usageSniffer 的 relay 拷贝路径）：包在
                        // 链最外层，流尾收口落账——SSE 尾帧 usage 四桶、断流
                        // 照落（已发字节算已服务）。
                        let log = super::usage_ledger::TurnLog::new("chat", &asked, session.clone(), began);
                        let tap = log.tap(
                            gw.usage.clone(),
                            super::usage_ledger::Dialect::Chat,
                            &cand.provider,
                            status,
                        );
                        // 续答底稿：attempt 刚用同一套准备段把这份体发了出去
                        //（重算字节相同）；拿不到就按无续答能力纯直通过。
                        let (out, sent_body) = prepare_outbound(gw, cand, &body).unwrap_or_else(
                            |_| {
                                (
                                    Outbound::new(
                                        cand.provider.clone(),
                                        String::new(),
                                        Vec::new(),
                                        cand.model.clone(),
                                        String::new(),
                                    ),
                                    Vec::new(),
                                )
                            },
                        );
                        // prefill 模式（Go continuation.go:458-462）：模型此前
                        // 拒过 prefill 的不再问（unfit 记忆）。
                        let mode = if prefill_refused_before(&cand.provider, &cand.model) {
                            ""
                        } else {
                            chat_prefill(&host_of(&out.url), &cand.model)
                        };
                        let terminal = Arc::new(Mutex::new(None::<String>));
                        let cont = ContinuationReader::new(
                            ContinuationCfg {
                                out,
                                mode,
                                base_body: sent_body,
                                provider: cand.provider.clone(),
                                model: cand.model.clone(),
                                ledger: gw.usage.clone(),
                                log: super::usage_ledger::TurnLog::new(
                                    "chat",
                                    &asked,
                                    session.clone(),
                                    began,
                                ),
                                status,
                                terminal: terminal.clone(),
                            },
                            Box::new(HeadThen { head, at: 0, rest }),
                        );
                        let relay = ClosingReader::new(
                            cand.provider.clone(),
                            Box::new(super::usage_ledger::TapReader::new(
                                tap,
                                chattidy::ChattidyReader::new(cont),
                            )),
                            terminal,
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
                Reply::Json { status, body: vendor_body } if status < 400 => {
                    // 非流式的合法收尾 = 整块 JSON 解析成功（D7）；顺手过
                    // chattidy 的整块修补（D10 非流式半边）。usage 整块提桶
                    // 在修补**前**的原始厂商体上做（别名桶提取序自带）；
                    // 会话/asked 取**请求**体（旧骨架版在应答体上取，
                    // session 几乎恒 None、model 记成厂商裸名——顺手纠正）。
                    if serde_json::from_slice::<Value>(&vendor_body).is_ok() {
                        wake(&cand.provider, &cand.model);
                        let (session, asked) = note_answered(gw, headers, &body, cand);
                        let log = super::usage_ledger::TurnLog::new("chat", &asked, session, began);
                        log.whole(&gw.usage, super::usage_ledger::Dialect::Chat, &cand.provider, status, &vendor_body);
                        return Reply::Json { status, body: chattidy::mend_whole_reply(&vendor_body) };
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
                if let Reply::Json { status, body } = &reply {
                    // zcode trial 拒答的 rest 分级（Go routing.go:1832-1836：
                    // `why == failQuota && zcodeTrialRefused(...)` 时 rest key
                    // 从 restKey 换成 restID——只 rest 该 `<model>-Trial` 条目，
                    // 账号其它模型不动）。判不中回落原有行为：429/5xx 整
                    // provider，400 不 rest。
                    if super::zcode_account::trial_refused_for(&cand.provider, &cand.model, *status, body)
                        .is_some()
                    {
                        rest_entry(&cand.provider, &cand.model);
                    } else if *status == 429 || *status >= 500 {
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

// - **usage 记账的覆盖面**（2026-11-18 capture 层接上后的残留）：成功回合
//   （流式 tap + 非流式整块）落真数；仍不落——提前拒答行（Go turnedAway
//   的 `Rejected:true`，gateway.go:2672-2690）、候选循环里失败 try 的中间
//   账（Go failedRow 的 HandedOn 行）、流断掉时的失败标记（Go sniff.go:26
//   `failed`——Rust 台账无 Error 侧字段，断流行与正常收尾都是 status 200）。
//   断流/失败落账语义：已发字节算已服务、桶按已见真数照记（毫秒到流尾）。
// - **continuation 断流续答已接（2026-11-19，chat 直通半边）**（Go
//   continuation.go:364 again / :378 prefillText / :455 streamTranslated）：
//   上游流中途死亡（[DONE] 与 finish_reason 都没说）且客户端已拿到正文时，
//   [`ContinuationReader`] 用已发正文当 prefill（厂商自家模式：DeepSeek
//   prefix / Kimi partial，chat_prefill；拒过的模型记 unfit 不再问）向同一
//   候选重发（上限 1+2 次出站调用、指数停顿），续写拼回同一条流，回声整段
//   剔除（unecho）、续答 try 的思考不重放；上限/拒答走到头以「断它的事」
//   收尾（cutMidReply 口径，ClosingReader 的 note）。没搬的子项：
//   ①**Anthropic 原生 prefill 半边**（prefillHow 的 "anthropic"）——那属于
//   messages.rs 入站翻译路径的输出流，本批只接 chat→chat 直通链，messages/
//   responses 的续答未做；②**错误帧扣住**：Go 翻译路径在可续时把上游错误
//   事件扣住不发，直通已发字节收不回——所以见到厂商错误帧直接判不可续
//   （saw_error 守卫），不让续写拼在错误后面；③**saysItsEnd/errEndedShort
//   与 chatTail 补 finish_reason**（Gemini 兼容口/Command Code 的「少说了
//   收尾」识别）未搬——以「finish_reason 已见 = 不可续」等效近似，不补帧；
//   ④**客户端断连感知**（again() 的 ctx.Err()）：reader 链看不到客户端
//   socket，续答照发到 Drop 收口；⑤**usage 台账形态**：Go 把各 try 折进
//   一行（Deferred→billed），Rust 是 tap 行（最后一次上报）+ 断 try 对被
//   重新上报的桶的补账行——总额同 billed，行数是「一次出站调用一行」；
//   ⑥续答 ask 的 bodycap prefit 未跑（原体已合身，续答体只多 prefill
//   正文；超限按 again 规则走重试/终局）；⑦**空回复重问（emptyRetries）/
//   loopGuard 重问/keepalive 注释**未搬——停顿期间不发 `: keepalive`
//   （#934 前科，直通不发 SSE 注释），空回复走基线错误帧。
// - **扣流首只护第一条完整行**：Go holdWriter 有 firstToken/patience 计时
//   （组里等多久放弃首字节）与 refusalHandover（读流首开头识别整流拒答
//   后换人）；这里流首一旦放行即终局，慢上游会让 agent 一直挂着（与现状
//   一致），流首之后的 200 拒答体原样透传。
// - **休息记忆极简**（见 [`REST_FOR`] 注释）：不落盘、不分错误等级、无
//   Retry-After 解析、成功只清本来源（Go served 还记 token 用量做 usage
//   路由）；且只挂在 chat 直通候选循环上——responses/messages 翻译路径
//   的 `groups::drive` 不查休息表（后续批次接线）。
//   **模型级条目已接一半（2026-11-18）**：zcode trial 拒答只 rest
//   `<provider>::<model>-Trial` 条目（Go routing.go:1834 的 restID），
//   账号其它模型照常候选——但 rest 窗口不分错误等级，Go 的 restingUntil
//   家族（额度耗尽/限流 Retry-After/挂了各分窗口长度 + 逐次失败退避递增）
//   没搬；Go 的 failQuota 400 也 rest 整账号，这里 400 判不中 trial 时仍
//   不 rest；zcodeTrialBlocked（405 拦截页）/ zcodeTrialRestricted（风控
//   3012）也只 rest trial 条目（zcode_trial.go:63/79），词表未移植；其它
//   厂商的模型级 rest 口（modelNamed / PaidBesideFree / modelLimitedBy）
//   未搬——模型级条目目前只有 zcode trial 拒答一个写入者。
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
    use std::io::{BufRead, BufReader, Write};
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
        // 休息表与 prefill 拒答记忆都是进程级的：沙箱起手清掉，串行测试之间
        // 不串状态；续答停顿钉成 1ms（生产默认 1s，Go retryPause）。
        clear_rests_for_tests();
        if let Ok(mut set) = prefill_refusals().lock() {
            set.clear();
        }
        RETRY_PAUSE_MS.store(1, std::sync::atomic::Ordering::Relaxed);
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
        /// 给了 delta + usage 尾帧后断（没有 [DONE]）——usage 已可见。
        AfterUsage,
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
                    CutAt::AfterUsage => {
                        let _ = stream.write_all(
                            b"data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"partial-\"}}]}\n\n",
                        );
                        let _ = stream.write_all(
                            b"data: {\"id\":\"c\",\"choices\":[],\"usage\":{\"prompt_tokens\":500,\"completion_tokens\":20,\"total_tokens\":520,\"prompt_tokens_details\":{\"cached_tokens\":150}}}\n\n",
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

    /// zcode trial 拒答的 rest 分级（Go zcode_trial.go:33 + routing.go:1834）：
    /// 词表点名的 `<model>-Trial` 条目自己休息，同 provider 的 plain 模型下一轮
    /// 照常由它服务；条目休息期间该模型的请求静默换下一成员。
    #[test]
    fn zcode_trial_refusal_rests_only_the_trial_entry() {
        // zcode 厂商：被问到 GLM-5.3-Trial 回 400 ended 词（点名同一模型），
        // 其它模型正常作答。词表体 1:1（Go zcode_trial_test.go:19）。
        let ended = b"{\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"ZCode has no active limited quota for GLM-5.3-Trial\"}}".to_vec();
        let z = fake(Box::new(move |s: &Seen| {
            let model = serde_json::from_slice::<Value>(&s.body)
                .ok()
                .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(String::from))
                .unwrap_or_default();
            if model == "GLM-5.3-Trial" {
                (400, "application/json", ended.clone())
            } else {
                (200, "application/json", ok_chat("from zcode"))
            }
        }));
        let b = fake(Box::new(|_| (200, "application/json", ok_chat("from b"))));
        let world = json!({
            "providers": [
                {"id": "zcode", "name": "Z", "key": "kz", "chat": format!("{}/v1", z.base)},
                {"id": "pb", "name": "B", "key": "kb", "chat": format!("{}/v1", b.base)},
            ],
            "groups": [
                {"id": "t", "name": "t", "members": ["zcode/GLM-5.3-Trial", "pb/mb"], "routing": "order"},
                {"id": "s", "name": "s", "members": ["zcode/GLM-5.3", "pb/mb"], "routing": "order"},
            ],
        })
        .to_string();
        let (gw, _sb) = sandbox(&world);
        // 第 1 发（trial）：zcode 拒 → 转 b。只 trial 条目休息，账号不休息。
        assert_eq!(status_of(&forward(&gw, &[], &chat_request("t", false))), 200);
        assert_eq!(z.hits.load(Ordering::SeqCst), 1);
        assert_eq!(b.hits.load(Ordering::SeqCst), 1);
        assert!(rest_left("zcode::GLM-5.3-Trial").is_some(), "the trial entry rests");
        assert_eq!(rest_left("zcode"), None, "the account itself must not rest");
        // 第 2 发（plain 模型）：zcode 没在休息 → 它自己答（Go：the account's
        // GLM-5.3 still served by it）。
        assert_eq!(status_of(&forward(&gw, &[], &chat_request("s", false))), 200);
        assert_eq!(z.hits.load(Ordering::SeqCst), 2, "the plain model must still ask zcode");
        assert_eq!(b.hits.load(Ordering::SeqCst), 1);
        // 第 3 发（trial 再来）：条目在休息 → 跳过 zcode，直接 b。
        let reply = forward(&gw, &[], &chat_request("t", false));
        assert_eq!(status_of(&reply), 200);
        assert!(text_of(reply).contains("from b"));
        assert_eq!(z.hits.load(Ordering::SeqCst), 2, "the resting entry must be skipped");
    }

    /// 判不中的组合照旧整 provider rest（Go zcode_trial_test.go:57-62「account
    /// quota / spent names another」：accountRest=true）：账号级 429 与「词表
    /// 命名了别的 trial」的 429 都 rest 整个 zcode；400 命中但模型对不上时
    /// Rust 不 rest（Go failQuota 400 rest 整账号——极简 rest 的既有差距）。
    #[test]
    fn zcode_trial_mismatch_and_account_refusals_rest_the_provider() {
        // spent 词点名 glm-5.3，候选是 Flash-Trial：EqualFold 对不上 → 整
        // provider rest。
        let spent = b"{\"type\":\"error\",\"error\":{\"type\":\"rate_limit_error\",\"message\":\"ZCode's limited quota for glm-5.3 is exhausted or temporarily unavailable\"}}".to_vec();
        let z = fake(Box::new(move |_| (429, "application/json", spent.clone())));
        let b = fake(Box::new(|_| (200, "application/json", ok_chat("from b"))));
        let world = json!({
            "providers": [
                {"id": "zcode", "name": "Z", "key": "kz", "chat": format!("{}/v1", z.base)},
                {"id": "pb", "name": "B", "key": "kb", "chat": format!("{}/v1", b.base)},
            ],
            "groups": [
                {"id": "t", "name": "t", "members": ["zcode/GLM-5.3-Flash-Trial", "pb/mb"], "routing": "order"},
                {"id": "s", "name": "s", "members": ["zcode/GLM-5.3", "pb/mb"], "routing": "order"},
            ],
        })
        .to_string();
        let (gw, _sb) = sandbox(&world);
        assert_eq!(status_of(&forward(&gw, &[], &chat_request("t", false))), 200);
        assert!(rest_left("zcode").is_some(), "a spent 429 naming another model rests the account");
        // plain 模型跟着被跳过（账号在休息）。
        assert_eq!(status_of(&forward(&gw, &[], &chat_request("s", false))), 200);
        assert_eq!(z.hits.load(Ordering::SeqCst), 1, "the rested account must be skipped");
        drop(_sb);

        // 账号级 429（普通 quota 词，不是 trial 词）照旧整 provider rest。
        let z2 = fake(Box::new(|_| {
            (429, "application/json", b"{\"error\":{\"type\":\"rate_limit_error\",\"message\":\"quota exhausted\"}}".to_vec())
        }));
        let b2 = fake(Box::new(|_| (200, "application/json", ok_chat("from b"))));
        let world2 = json!({
            "providers": [
                {"id": "zcode", "name": "Z", "key": "kz", "chat": format!("{}/v1", z2.base)},
                {"id": "pb", "name": "B", "key": "kb", "chat": format!("{}/v1", b2.base)},
            ],
            "groups": [
                {"id": "t", "name": "t", "members": ["zcode/GLM-5.3-Trial", "pb/mb"], "routing": "order"},
                {"id": "s", "name": "s", "members": ["zcode/GLM-5.3", "pb/mb"], "routing": "order"},
            ],
        })
        .to_string();
        let (gw, _sb) = sandbox(&world2);
        assert_eq!(status_of(&forward(&gw, &[], &chat_request("t", false))), 200);
        assert!(rest_left("zcode").is_some(), "an account-level 429 rests the account");
        assert_eq!(rest_left("zcode::GLM-5.3-Trial"), None, "no model-level entry for account refusals");
        assert_eq!(status_of(&forward(&gw, &[], &chat_request("s", false))), 200);
        assert_eq!(z2.hits.load(Ordering::SeqCst), 1, "the rested account must be skipped");
        drop(_sb);

        // 400 ended 词点名另一个 trial（候选 Flash-Trial）：判不中 → 不 rest
        //（Rust 现状；Go 会按 failQuota rest 整账号）——下一次仍问 zcode。
        let ended_other = b"{\"type\":\"error\",\"error\":{\"type\":\"invalid_request_error\",\"message\":\"ZCode has no active limited quota for GLM-5.3-Trial\"}}".to_vec();
        let z3 = fake(Box::new(move |s: &Seen| {
            let model = serde_json::from_slice::<Value>(&s.body)
                .ok()
                .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(String::from))
                .unwrap_or_default();
            if model == "GLM-5.3-Flash-Trial" {
                (400, "application/json", ended_other.clone())
            } else {
                (200, "application/json", ok_chat("from zcode"))
            }
        }));
        let b3 = fake(Box::new(|_| (200, "application/json", ok_chat("from b"))));
        let world3 = json!({
            "providers": [
                {"id": "zcode", "name": "Z", "key": "kz", "chat": format!("{}/v1", z3.base)},
                {"id": "pb", "name": "B", "key": "kb", "chat": format!("{}/v1", b3.base)},
            ],
            "groups": [
                {"id": "t", "name": "t", "members": ["zcode/GLM-5.3-Flash-Trial", "pb/mb"], "routing": "order"},
                {"id": "s", "name": "s", "members": ["zcode/GLM-5.3", "pb/mb"], "routing": "order"},
            ],
        })
        .to_string();
        let (gw, _sb) = sandbox(&world3);
        assert_eq!(status_of(&forward(&gw, &[], &chat_request("t", false))), 200, "400 quota words fail over to b");
        assert_eq!(rest_left("zcode"), None, "an unmatched 400 does not rest the provider in Rust");
        assert_eq!(rest_left("zcode::GLM-5.3-Flash-Trial"), None, "nor does it grade to the entry");
        assert_eq!(status_of(&forward(&gw, &[], &chat_request("s", false))), 200);
        assert_eq!(z3.hits.load(Ordering::SeqCst), 2, "zcode is still asked for its plain model");
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

    // ----- usage 捕获（capture 层，Go sniff.go 口径）--------------------------

    /// 带 prompt_cache_key 的请求体（session 种子）。
    fn keyed_request(model: &str, stream: bool) -> Vec<u8> {
        json_body(json!({
            "model": model, "stream": stream, "prompt_cache_key": "sess-42",
            "messages": [{"role": "user", "content": "hi"}]
        }))
    }

    /// 流式回合落账：SSE 尾帧 usage 四桶（含缓存拆桶）、真实 millis、
    /// session 种子随行——流排干即收口（Go serve 尽头一口账）。
    #[test]
    fn stream_turn_records_usage_buckets_with_real_millis() {
        let sse = concat!(
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n",
            "data: {\"id\":\"c\",\"choices\":[],\"usage\":{\"prompt_tokens\":1000,\"completion_tokens\":40,\"total_tokens\":1040,\"prompt_tokens_details\":{\"cached_tokens\":300,\"cache_write_tokens\":100}}}\n\n",
            "data: [DONE]\n\n",
        );
        let v = fake(Box::new(|_| (200, "text/event-stream", sse.as_bytes().to_vec())));
        let (gw, _sb) = sandbox(
            &json!({"providers": [{"id": "pv", "name": "V", "chat": format!("{}/v1", v.base)}]}).to_string(),
        );
        let reply = forward(&gw, &[], &keyed_request("pv/m", true));
        let text = text_of(reply); // 排干流 → tap 在 EOF 收口落账
        assert!(text.contains("[DONE]"));
        let rows = gw.usage.records_since(0);
        assert_eq!(rows.len(), 1, "一条回合一行");
        let r = &rows[0];
        assert_eq!(r.agent, "chat");
        assert_eq!(r.provider, "pv");
        assert_eq!(r.model, "pv/m", "asked 串（含 provider/ 前缀）");
        assert_eq!(r.status, 200);
        assert_eq!(r.kind, "turn");
        assert_eq!(r.session.as_deref(), Some("sess-42"), "affinity 同源种子随行");
        assert_eq!(r.in_tokens, Some(600), "OpenAI 口径：prompt 1000 − 300 − 100");
        assert_eq!(r.out_tokens, Some(40));
        assert_eq!(r.cache_read, Some(300));
        assert_eq!(r.cache_write, Some(100));
        assert!(r.millis > 0, "真实 millis（请求入口→流尾），got {}", r.millis);
    }

    /// 断流也落账：usage 帧已过、[DONE] 没来（ClosingReader 补显式收尾）——
    /// 已发字节算已服务，桶按已见真数照记，status 照实（200）。
    #[test]
    fn cut_stream_still_records_seen_usage() {
        let a = cut_fake(CutAt::AfterUsage);
        let (gw, _sb) = sandbox(
            &json!({"providers": [{"id": "pa", "name": "A", "chat": format!("{}/v1", a.base)}]}).to_string(),
        );
        let reply = forward(&gw, &[], &chat_request("pa/ma", true));
        let text = text_of(reply);
        assert!(text.contains("\"error\""), "断流补显式 error 帧: {text}");
        let rows = gw.usage.records_since(0);
        assert_eq!(rows.len(), 1, "断流照落一行");
        let r = &rows[0];
        assert_eq!(r.in_tokens, Some(350), "500 − 150 缓存读");
        assert_eq!(r.out_tokens, Some(20));
        assert_eq!(r.cache_read, Some(150));
        assert!(r.cache_write.is_none(), "厂商没说缓存写就是 None");
        assert_eq!(r.status, 200, "客户端看到的 HTTP 状态照实");
    }

    /// 非流式整块：usage 直接从厂商体提（别家缓存名走同一条提取序），
    /// 会话/asked 取**请求**体。
    #[test]
    fn non_stream_turn_records_whole_usage() {
        let whole = serde_json::to_vec(&json!({
            "id": "c", "object": "chat.completion", "model": "mb",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 900, "completion_tokens": 25, "total_tokens": 925,
                "prompt_cache_hit_tokens": 700}
        }))
        .unwrap();
        let v = fake(Box::new(move |_| (200, "application/json", whole.clone())));
        let (gw, _sb) = sandbox(
            &json!({"providers": [{"id": "pv", "name": "V", "chat": format!("{}/v1", v.base)}]}).to_string(),
        );
        let reply = forward(&gw, &[], &keyed_request("pv/m", false));
        assert_eq!(status_of(&reply), 200);
        let rows = gw.usage.records_since(0);
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.model, "pv/m", "asked（不是厂商裸名 mb）");
        assert_eq!(r.session.as_deref(), Some("sess-42"));
        assert_eq!(r.in_tokens, Some(200), "DeepSeek 名字：900 含缓存 → 900 − 700");
        assert_eq!(r.out_tokens, Some(25));
        assert_eq!(r.cache_read, Some(700), "prompt_cache_hit_tokens 别名桶");
        assert!(r.millis > 0);
    }

    // ----- continuation 断流续答（Go continuation.go 的直通半边）--------------

    /// 每个调用的剧本。
    #[derive(Clone)]
    enum Call {
        /// 完整流（这些事件 + [DONE]）。
        Whole(Vec<u8>),
        /// 发完这些事件就掐线（无 [DONE]，连接直落 EOF——半截回复）。
        Cut(Vec<u8>),
        /// 非 2xx 拒答体。
        Fail(u16, &'static str),
    }

    /// 按调用序出剧本的假厂商（cut_fake 的多调用版）：记 body 与请求路径
    /// （续答的 /beta 换道要断言）。
    fn scripted_fake(calls: Vec<Call>) -> (Fake, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let paths: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(Mutex::new(calls));
        let (h, s, p) = (hits.clone(), seen.clone(), paths.clone());
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
                let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
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
                let call = h.fetch_add(1, Ordering::SeqCst);
                s.lock().unwrap().push(Seen { body });
                p.lock().unwrap().push(path);
                let script = script.lock().unwrap();
                match script.get(call) {
                    Some(Call::Whole(events)) | Some(Call::Cut(events)) => {
                        let cut = matches!(script.get(call), Some(Call::Cut(_)));
                        let mut stream = stream;
                        let _ = stream.write_all(
                            b"HTTP/1.1 200 X\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                        );
                        let _ = stream.write_all(events);
                        if !cut {
                            let _ = stream.write_all(b"data: [DONE]\n\n");
                        }
                        let _ = stream.flush();
                        drop(stream); // Cut 在此掐线；Whole 也在 EOF 收（Connection: close）
                    }
                    Some(Call::Fail(status, body)) => {
                        let mut stream = stream;
                        let head = format!(
                            "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            status,
                            body.len()
                        );
                        let _ = stream.write_all(head.as_bytes());
                        let _ = stream.write_all(body.as_bytes());
                        let _ = stream.flush();
                        drop(stream);
                    }
                    None => {
                        // 剧本之外的多余调用：掐线（测试会断言调用数）
                        drop(stream);
                    }
                }
            }
        });
        (Fake { base: format!("http://127.0.0.1:{port}"), hits, seen }, paths)
    }

    fn ev_text(s: &str) -> Vec<u8> {
        format!(
            "data: {{\"id\":\"c\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{}\"}}}}]}}\n\n",
            s
        )
        .into_bytes()
    }

    fn ev_think(s: &str) -> Vec<u8> {
        format!(
            "data: {{\"id\":\"c\",\"choices\":[{{\"index\":0,\"delta\":{{\"reasoning_content\":\"{}\"}}}}]}}\n\n",
            s
        )
        .into_bytes()
    }

    fn ev_usage(prompt: u64, completion: u64, cached: u64) -> Vec<u8> {
        format!(
            "data: {{\"id\":\"c\",\"choices\":[],\"usage\":{{\"prompt_tokens\":{},\"completion_tokens\":{},\"total_tokens\":{},\"prompt_tokens_details\":{{\"cached_tokens\":{}}}}}}}\n\n",
            prompt, completion, prompt, cached
        )
        .into_bytes()
    }

    fn ev_tool_call() -> Vec<u8> {
        concat!(
            "data: {\"id\":\"c\",\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"get_weather\",\"arguments\":\"{\\\"ci\"}}]}}]}\n\n",
        )
        .as_bytes()
        .to_vec()
    }

    /// 客户端流里 delta.content 的拼接（诊断断言用）。
    fn relay_text(stream: &str) -> String {
        let mut out = String::new();
        for line in stream.lines() {
            let Some(data) = line.trim_start().strip_prefix("data: ") else { continue };
            let Ok(v) = serde_json::from_str::<Value>(data.trim()) else { continue };
            if let Some(s) = v.pointer("/choices/0/delta/content").and_then(|c| c.as_str()) {
                out.push_str(s);
            }
        }
        out
    }

    /// 客户端流里 delta.reasoning_content 的拼接。
    fn relay_think(stream: &str) -> String {
        let mut out = String::new();
        for line in stream.lines() {
            let Some(data) = line.trim_start().strip_prefix("data: ") else { continue };
            let Ok(v) = serde_json::from_str::<Value>(data.trim()) else { continue };
            if let Some(s) = v.pointer("/choices/0/delta/reasoning_content").and_then(|c| c.as_str()) {
                out.push_str(s);
            }
        }
        out
    }

    /// 客户端流里最后的 usage 帧（None = 没有）。
    fn relay_last_usage(stream: &str) -> Option<Value> {
        let mut out = None;
        for line in stream.lines() {
            let Some(data) = line.trim_start().strip_prefix("data: ") else { continue };
            let Ok(v) = serde_json::from_str::<Value>(data.trim()) else { continue };
            if v.get("usage").is_some() {
                out = Some(v["usage"].clone());
            }
        }
        out
    }

    /// 一次直连（单来源）的流式转发。
    fn ask(gw: &Gateway, model: &str) -> Reply {
        forward(gw, &[], &chat_request(model, true))
    }

    /// 断在中途的续答：同一对话被再问一次（带 prefill），客户端读到拼回的
    /// 一条完整回复（Go TestStreamCutMidReplyContinues）。
    #[test]
    fn cut_mid_reply_continuation_splices_one_reply() {
        let (a, _) = scripted_fake(vec![
            Call::Cut([ev_think("think-1"), ev_text("hel")].concat()),
            Call::Whole([ev_think("rethink"), ev_text("lo"), ev_usage(11, 4, 0)].concat()),
        ]);
        let (gw, _sb) = sandbox(&json!({"providers": [{"id": "pa", "name": "A", "key": "k", "chat": format!("{}/v1", a.base)}]}).to_string());
        let reply = ask(&gw, "pa/kimi-k3");
        assert_eq!(status_of(&reply), 200);
        let text = text_of(reply);
        assert_eq!(relay_text(&text), "hello", "拼回的正文: {text}");
        assert_eq!(relay_think(&text), "think-1", "续答的思考不重放: {text}");
        assert!(!text.contains("\"error\""), "拼回的流没有错误帧: {text}");
        assert!(text.ends_with("data: [DONE]\n\n"), "以 [DONE] 收尾: {text}");
        assert_eq!(a.hits.load(Ordering::SeqCst), 2, "同一对话被再问一次");
        // 第二问的最后一条消息就是 prefill（Kimi 的 partial 标记 + 思考回放）。
        let bodies = a.bodies();
        let last = bodies[1]["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["role"], "assistant");
        assert_eq!(last["content"], "hel");
        assert_eq!(last["partial"], true);
        assert_eq!(last["reasoning_content"], "think-1");
    }

    /// 续答把 prefill 原样回读一遍再接着写：回声整段剔除（Go
    /// TestStreamCutContinuationEchoDropped）。
    #[test]
    fn continuation_echo_is_dropped() {
        let (a, _) = scripted_fake(vec![
            Call::Cut(ev_text("hel")),
            Call::Whole([ev_text("hel"), ev_text("lo")].concat()),
        ]);
        let (gw, _sb) = sandbox(&json!({"providers": [{"id": "pa", "name": "A", "key": "k", "chat": format!("{}/v1", a.base)}]}).to_string());
        let text = text_of(ask(&gw, "pa/kimi-k3"));
        assert_eq!(a.hits.load(Ordering::SeqCst), 2);
        assert_eq!(relay_text(&text), "hello", "回声只读一次: {text}");
        assert!(!text.contains("\"error\""));
    }

    /// 工具调用一旦开始就续不了（参数没法 prefill）：以断流的错误收尾，
    /// 一次都不再问（Go TestStreamCutAfterToolCallEndsAsBefore）。
    #[test]
    fn cut_after_tool_call_ends_as_before() {
        let (a, _) = scripted_fake(vec![Call::Cut(
            [ev_text("hel"), ev_tool_call()].concat(),
        )]);
        let (gw, _sb) = sandbox(&json!({"providers": [{"id": "pa", "name": "A", "key": "k", "chat": format!("{}/v1", a.base)}]}).to_string());
        let text = text_of(ask(&gw, "pa/kimi-k3"));
        assert_eq!(a.hits.load(Ordering::SeqCst), 1, "有调用开始的回复不再问");
        assert!(text.contains("\"error\""), "以错误收尾: {text}");
        assert!(text.contains("connection lost mid-reply"), "Go cutMidReply 的口径: {text}");
        assert!(text.ends_with("data: [DONE]\n\n"));
    }

    /// 每一问都被掐：上限（1+streamRetries）用尽后以错误收尾（Go
    /// TestStreamCutContinuesOnlySoLong）。
    #[test]
    fn continuation_only_so_long() {
        let cut = Call::Cut(ev_text("hel"));
        let (a, _) = scripted_fake(vec![cut.clone(), cut.clone(), cut.clone()]);
        let (gw, _sb) = sandbox(&json!({"providers": [{"id": "pa", "name": "A", "key": "k", "chat": format!("{}/v1", a.base)}]}).to_string());
        let text = text_of(ask(&gw, "pa/kimi-k3"));
        assert_eq!(
            a.hits.load(Ordering::SeqCst),
            1 + STREAM_RETRIES as usize,
            "上限之内不再多问"
        );
        assert!(text.contains("\"error\""));
        assert_eq!(relay_text(&text), "hel", "客户端只有第一段正文: {text}");
    }

    /// 无 prefill 模式的上游会从头重答——拼接是改写过的重答，不续（Go
    /// TestStreamCutOnPlainChatEndsAsBefore）。
    #[test]
    fn plain_chat_cut_ends_as_before() {
        let (a, _) = scripted_fake(vec![Call::Cut(ev_text(
            "Go's garbage collector is a concurrent, tri-color",
        ))]);
        let (gw, _sb) = sandbox(&json!({"providers": [{"id": "pa", "name": "A", "key": "k", "chat": format!("{}/v1", a.base)}]}).to_string());
        let text = text_of(ask(&gw, "pa/gpt-x"));
        assert_eq!(a.hits.load(Ordering::SeqCst), 1, "从头重答的上游不被问续答");
        assert!(text.contains("\"error\""));
        assert_eq!(relay_text(&text), "Go's garbage collector is a concurrent, tri-color");
    }

    /// 断在纯思考里也续：prefill 带思考回放（空正文），续答的思考不重放
    ///（Go TestStreamCutMidThinkingContinues）。
    #[test]
    fn cut_mid_thinking_continues() {
        let (a, _) = scripted_fake(vec![
            Call::Cut(ev_think("think-1")),
            Call::Whole([ev_think("rethink"), ev_text("done")].concat()),
        ]);
        let (gw, _sb) = sandbox(&json!({"providers": [{"id": "pa", "name": "A", "key": "k", "chat": format!("{}/v1", a.base)}]}).to_string());
        let text = text_of(ask(&gw, "pa/kimi-k3"));
        assert_eq!(a.hits.load(Ordering::SeqCst), 2);
        assert_eq!(relay_think(&text), "think-1", "rethink 不重放: {text}");
        assert_eq!(relay_text(&text), "done");
        assert!(!text.contains("\"error\""));
        let bodies = a.bodies();
        let last = bodies[1]["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["reasoning_content"], "think-1");
        assert_eq!(last["partial"], true);
        assert_eq!(last["content"], "", "纯思考断流的 prefill 是空正文 + 思考回放");
    }

    /// DeepSeek 的 prefix 模式走 /beta 路径、消息带 prefix 标记（Go
    /// TestStreamCutContinuesWithDeepSeekPrefix + gateway.go:8664）。
    #[test]
    fn deepseek_prefix_continuation_uses_beta_path() {
        let (a, paths) = scripted_fake(vec![
            Call::Cut(ev_text("hel")),
            Call::Whole(ev_text("lo")),
        ]);
        let (gw, _sb) = sandbox(&json!({"providers": [{"id": "pa", "name": "A", "key": "k", "chat": format!("{}/v1", a.base)}]}).to_string());
        let text = text_of(ask(&gw, "pa/deepseek-chat"));
        assert_eq!(a.hits.load(Ordering::SeqCst), 2);
        assert_eq!(paths.lock().unwrap()[1], "/beta/chat/completions", "prefix 模式的续答走 /beta");
        assert_eq!(relay_text(&text), "hello");
        let bodies = a.bodies();
        let last = bodies[1]["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["prefix"], true);
        assert_eq!(last["content"], "hel");
    }

    /// 拒 prefill 的模型：以断流自己的错误收尾（不是客户端从没发起过的
    /// 续答的拒答），并且记住——下一个同模型的断流不再问（Go
    /// TestStreamCutPrefillRefusedEndsAsBefore + prefill.go 的 unfit 记忆）。
    /// 同一网关两发（记忆是进程级的，第二个沙箱会清掉它）。
    #[test]
    fn prefill_refusal_ends_as_cut_and_is_remembered() {
        let refusal = "{\"error\":{\"message\":\"This model does not support assistant message prefill. The conversation must end with a user message.\"}}";
        let (a, _) = scripted_fake(vec![
            Call::Cut(ev_text("hel")),
            Call::Fail(400, refusal),
            Call::Cut(ev_text("hel")),
        ]);
        let (gw, _sb) = sandbox(&json!({"providers": [{"id": "pa", "name": "A", "key": "k", "chat": format!("{}/v1", a.base)}]}).to_string());
        let text = text_of(ask(&gw, "pa/kimi-k3"));
        assert_eq!(a.hits.load(Ordering::SeqCst), 2, "拒答前续答问过一次");
        assert!(text.contains("connection lost mid-reply"), "收尾说的是断流: {text}");
        assert!(!text.contains("does not support"), "拒答正文不进客户端的流: {text}");
        assert_eq!(relay_text(&text), "hel");
        // 记住了：下一个断流不再问（第二发一次出站调用就到头）。
        let text = text_of(ask(&gw, "pa/kimi-k3"));
        assert_eq!(a.hits.load(Ordering::SeqCst), 3, "拒过的模型不再被问续答（2+1）");
        assert!(text.contains("\"error\""));
        assert_eq!(relay_text(&text), "hel");
    }

    /// 续答计入台账：断掉的 try 的桶与续答 try 的桶都在账上（Go 把两次
    /// 折进一行 billed；Rust 按出站调用分行——tap 行 = 最后一次上报，断
    /// try 对被重新上报的桶补一行，总额相同），客户端看到的 usage 帧
    /// 原样（最后一次 try 自己的）。
    #[test]
    fn continuation_usage_ledger_records_both_tries() {
        let (a, _) = scripted_fake(vec![
            Call::Cut([ev_text("hel"), ev_usage(500, 20, 150)].concat()),
            Call::Whole([ev_text("lo"), ev_usage(700, 5, 200)].concat()),
        ]);
        let (gw, _sb) = sandbox(&json!({"providers": [{"id": "pa", "name": "A", "key": "k", "chat": format!("{}/v1", a.base)}]}).to_string());
        let text = text_of(ask(&gw, "pa/kimi-k3"));
        assert!(text.contains("\"error\"") == false);
        // 客户端看到的 usage 帧原样：最后一次 try 自己的（Go：客户端读
        // 最后一次的 prompt 当会话满度，不是和）。
        let u = relay_last_usage(&text).expect("usage frame relayed");
        assert_eq!(u["prompt_tokens"], 700);
        assert_eq!(u["completion_tokens"], 5);
        // 台账：两行（断 try 补一行 + tap 行），四桶总额 = 两次上报之和。
        let rows = gw.usage.records_since(0);
        assert_eq!(rows.len(), 2, "断 try 一行补账 + tap 行: {rows:?}");
        let total = |pick: fn(&super::super::usage_ledger::UsageRecord) -> Option<u64>| {
            rows.iter().filter_map(|r| pick(r)).sum::<u64>()
        };
        assert_eq!(total(|r| r.in_tokens), 850, "500−150 + 700−200");
        assert_eq!(total(|r| r.out_tokens), 25, "20 + 5");
        assert_eq!(total(|r| r.cache_read), 350, "150 + 200");
        assert!(rows.iter().all(|r| r.provider == "pa" && r.kind == "turn"));
    }

    /// 断流的 try 没说过 usage：不补空行（tap 行只有续答 try 的桶）。
    #[test]
    fn cut_try_without_usage_adds_no_extra_row() {
        let (a, _) = scripted_fake(vec![
            Call::Cut(ev_text("hel")),
            Call::Whole([ev_text("lo"), ev_usage(700, 5, 0)].concat()),
        ]);
        let (gw, _sb) = sandbox(&json!({"providers": [{"id": "pa", "name": "A", "key": "k", "chat": format!("{}/v1", a.base)}]}).to_string());
        let text = text_of(ask(&gw, "pa/kimi-k3"));
        assert_eq!(relay_text(&text), "hello");
        let rows = gw.usage.records_since(0);
        assert_eq!(rows.len(), 1, "断 try 没报过桶就不补行: {rows:?}");
        assert_eq!(rows[0].in_tokens, Some(700));
    }

    /// 流首之前断：静默换人重发整轮，续答机制不沾手（既有换人路径）。
    #[test]
    fn cut_before_first_line_still_fails_over_not_continues() {
        // 单来源（没有下一个候选可换）→ 502 可判定错误，续答不触发
        //（流首之前客户端一个字节都没拿到，没有可 prefill 的东西）。
        let a = cut_fake(CutAt::Before);
        let (gw, _sb) = sandbox(&json!({"providers": [{"id": "pa", "name": "A", "chat": format!("{}/v1", a.base)}]}).to_string());
        let reply = ask(&gw, "pa/kimi-k3");
        assert_eq!(status_of(&reply), 502, "流首之前死仍是换人/502 语义");
        assert!(text_of(reply).contains("before its first line"));
    }

    /// chat_prefill 对照 Go TestChatPrefill 的表（host_of 已剥端口，端口
    /// 形态的用剥过的主机名断言）。
    #[test]
    fn chat_prefill_matches_go_table() {
        for (host, model, want) in [
            ("api.deepseek.com", "deepseek-chat", "prefix"),
            ("api.moonshot.cn", "kimi-k3", "partial"),
            ("api.moonshot.ai", "kimi-k3", "partial"),
            ("api.kimi.ai", "kimi-for-coding", "partial"),
            ("api.openai.com", "gpt-5", ""),
            ("openrouter.ai", "moonshotai/kimi-k3", ""), // 别处的中转不被认定直连
            ("relay.example.com", "kimi-k3", ""),
            ("127.0.0.1", "kimi-k3", "partial"),
            ("127.0.0.1", "deepseek-chat", "prefix"),
            ("localhost", "kimi-for-coding", "partial"),
            ("192.168.1.5", "kimi-k3", "partial"),
            ("127.0.0.1", "gpt-5", ""),
        ] {
            assert_eq!(chat_prefill(host, model), want, "chat_prefill({host}, {model})");
        }
    }

    /// unecho 对照 Go TestContinuationUnecho 的表（1:1）。
    #[test]
    fn unecho_matches_go_table() {
        for (echo, chunks, want) in [
            ("hel", vec!["lo"], "lo"),                 // 普通续答
            ("hel", vec!["hel", "lo"], "lo"),          // 先回声再续
            ("hel", vec!["h", "e", "llo"], "lo"),      // 回声分片
            ("hel", vec!["he", "lp"], "p"),            // 前缀即回声
            ("hel", vec!["x", "yz"], "xyz"),           // 不像的整段放行
            ("hel", vec!["hel"], ""),                  // 只有回声
            ("hel", vec!["he"], ""),                   // 可能还是回声的持住
            ("", vec!["hel"], "hel"),                  // 没预填就不剔
        ] {
            let mut u = Unecho { echo: echo.to_string(), seen: String::new() };
            let mut out = String::new();
            for ch in &chunks {
                out.push_str(&u.strip(ch));
            }
            assert_eq!(out, want, "unecho({echo}, {chunks:?})");
        }
    }

    /// prefill 拒答词对照 Go prefill.go:37 prefillRefusal 的三条字面形态
    ///（badRequest 门 = 400/422）。
    #[test]
    fn refuses_prefill_matches_go_words() {
        assert!(refuses_prefill(400, b"This model does not support assistant message prefill."));
        assert!(refuses_prefill(422, b"The last message cannot be from the assistant for a model that does not support prefill"));
        assert!(refuses_prefill(400, b"Requests ending with a model turn are not supported"));
        assert!(!refuses_prefill(400, b"unrelated bad request"));
        assert!(!refuses_prefill(500, b"does not support prefill"), "只有 400/422 算拒 prefill");
        assert!(!refuses_prefill(404, b"requests ending with a model turn are not supported"));
    }
}
