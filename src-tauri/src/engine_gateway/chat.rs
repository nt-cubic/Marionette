//! `/v1/chat/completions` 转发（合同 C5 的 openai→openai 直通形态）。
//! 模型 id 剥 `provider/` 前缀后按来源出站；SSE 流式透传（边读边 flush）。
//! 字节上限 refit（丢最老图重发）在 bodycap.rs，由这里在 4xx 应答后调用。
//!
//! 组路由与失败转移（groups.rs）：asked 解析成有序候选表，[`attempt`] 打一个
//! 成员——连接失败 / `groups::retryable` 口径的状态时返回 `Attempt::Retry`
//! 让 `groups::drive` 换下一个；**只在应答头阶段换**，流式 2xx 一旦交出
//! `Stream` 就是终局。responses/messages 的翻译路径复用同一个 [`attempt`]。
//!
//! 出站前的 chat 体「合身」（Go gateway.go `case provider.Chat` 那一段的
//! 子集，[`fit_for_vendor`]）：`developer`→`system`、纯文本 parts 折成字符串、
//! `max_tokens`↔`max_completion_tokens`（OpenAI 自家域名主动用后者；其它厂商
//! 400 点名另一个字段时改名重发一次并记住）、DeepSeek 系回放 `reasoning_content`
//! （translate.rs 的 responses→chat 把 reasoning item 暂存在 [`THINK_KEY`]，
//! 这里按厂商决定是回放还是丢弃）。
use std::io::Read;

use serde_json::Value;

use super::bodycap;
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

/// POST /v1/chat/completions 入口。
pub fn forward(gw: &super::Gateway, body: &[u8]) -> Reply {
    let model = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(String::from))
        .unwrap_or_default();
    let cands = groups::plan_for(gw, &model);
    if cands.is_empty() {
        return Reply::error(404, &format!("no provider for model: {model}"));
    }
    groups::drive(&cands, |cand| {
        let body = groups::body_for(gw, super::effort::Proto::Chat, cand, body);
        attempt(gw, cand, &body)
    })
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
        let reply = forward(&gw, &chat_request("g", false));
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
        let reply = forward(&gw, &chat_request("g", false));
        assert_eq!(status_of(&reply), 200);
        assert_eq!(b.hits.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn request_errors_stay_put_and_total_outage_returns_the_last_error() {
        let a = fake(Box::new(|_| (400, "application/json", b"{\"error\":{\"message\":\"bad parameter\"}}".to_vec())));
        let b = fake(Box::new(|_| (200, "application/json", ok_chat("never"))));
        let (gw, sb) = sandbox(&two_member_world(&a, &b, "order"));
        let reply = forward(&gw, &chat_request("g", false));
        assert_eq!(status_of(&reply), 400);
        assert_eq!(b.hits.load(Ordering::SeqCst), 0, "request errors must not move to the next member");
        drop(sb);
        let a = fake(Box::new(|_| (503, "application/json", b"{\"error\":\"a down\"}".to_vec())));
        let b = fake(Box::new(|_| (502, "application/json", b"{\"error\":\"b down\"}".to_vec())));
        let (gw, _sb) = sandbox(&two_member_world(&a, &b, "order"));
        let reply = forward(&gw, &chat_request("g", false));
        assert_eq!(status_of(&reply), 502);
        assert!(text_of(reply).contains("b down"));
    }

    #[test]
    fn streaming_failover_only_before_bytes_and_stream_is_final() {
        // A 在应答头阶段就 503：换成员，B 的流照常透传。
        let a = fake(Box::new(|_| (503, "application/json", b"{\"error\":\"down\"}".to_vec())));
        let b = fake(Box::new(|_| (200, "text/event-stream", SSE.as_bytes().to_vec())));
        let (gw, sb) = sandbox(&two_member_world(&a, &b, "order"));
        let reply = forward(&gw, &chat_request("g", true));
        assert!(matches!(reply, Reply::Stream { status: 200, .. }));
        assert!(text_of(reply).contains("[DONE]"));
        drop(sb);
        // A 已经开流（2xx）：流就是终局，B 一次都不碰。
        let a = fake(Box::new(|_| (200, "text/event-stream", SSE.as_bytes().to_vec())));
        let b = fake(Box::new(|_| (200, "application/json", ok_chat("never"))));
        let (gw, _sb) = sandbox(&two_member_world(&a, &b, "order"));
        let reply = forward(&gw, &chat_request("g", true));
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
        let reply = forward(&gw, &body);
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
        assert_eq!(status_of(&forward(&gw, &body)), 200);
        assert_eq!(v.hits.load(Ordering::SeqCst), 2);
        assert_eq!(v.bodies()[1]["max_completion_tokens"], 5);
        // 记住了：下一次直接用对的字段，不再吃 400。
        assert_eq!(status_of(&forward(&gw, &body)), 200);
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
            assert_eq!(status_of(&forward(&gw, &chat_request("w-route", false))), 200);
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
        assert_eq!(status_of(&forward(&gw, &body)), 200);
        assert_eq!(a.bodies()[0]["reasoning_effort"], "low");
        assert_eq!(b.bodies()[0]["reasoning_effort"], "max");
        assert_eq!(b.bodies()[0]["model"], "mb");
    }
}
