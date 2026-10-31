//! Council 模型 worker:与引擎(Magpie)的 completion 通道。
//!
//! 架构审计决策 #1:模型 worker 直连引擎 HTTP,不经 ACP(避免事件进
//! transcript 管线、绕开一会话一 turn 限制、无需 spawn 进程)。调用
//! 模板 = `memory.rs::distill_with_deepseek`(后端唯一的直调先例):
//! ureq + 手工 JSON + 非流式 + `/choices/0/message/content`。
//!
//! 端口永不缓存:`engine::running_base()` 每次现查(引擎端口会漂移,
//! 见 engine.rs:22 的注释)。

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct WorkerRequest {
    pub model: String,
    pub system: String,
    pub user: String,
    pub max_tokens: Option<u64>,
}

#[derive(Clone, Debug, Default)]
pub struct WorkerOutput {
    pub content: String,
    /// prompt+output 字符数,预算估算用。
    pub prompt_chars: usize,
    pub output_chars: usize,
}

/// 模型客户端抽象。生产 = `EngineClient`(引擎 /v1/chat/completions);
/// 测试 = `MockClient`(脚本化响应,按 model id 区分,验收 #1/#14 的
/// mock/live 区分点)。
pub trait ModelClient: Send + Sync {
    fn name(&self) -> &'static str;
    fn complete(&self, req: &WorkerRequest) -> Result<WorkerOutput, String>;
}

/// 引擎客户端。注意:引擎唤醒(`ensure_bridge_ready_if_installed`)是
/// orchestrator 的职责——启动 run 时做一次,不在每次请求里重复(一次
/// 最多等 20s,重复等待没有意义)。
pub struct EngineClient {
    pub timeout_secs: u64,
}

impl Default for EngineClient {
    fn default() -> Self {
        // 审议回复偏长,比 memory 蒸馏的 60s 放宽。
        EngineClient { timeout_secs: 120 }
    }
}

impl ModelClient for EngineClient {
    fn name(&self) -> &'static str {
        "engine"
    }

    fn complete(&self, req: &WorkerRequest) -> Result<WorkerOutput, String> {
        let (port, _version) = crate::engine::running_base()
            .ok_or_else(|| "模型共享引擎未运行(无法发起 Council 模型调用)".to_string())?;
        let prompt_chars = req.system.chars().count() + req.user.chars().count();
        let mut body = serde_json::json!({
            "model": req.model,
            "messages": [
                { "role": "system", "content": req.system },
                { "role": "user", "content": req.user },
            ],
            "stream": false,
        });
        if let Some(max) = req.max_tokens {
            body["max_tokens"] = serde_json::json!(max);
        }
        let body = serde_json::to_string(&body).map_err(|e| format!("序列化请求失败:{e}"))?;
        let agent = crate::http_client::agent()?;
        // 错误前缀是契约:orchestrator 的瞬时错误分类(worker::is_transient_error)
        // 按「引擎请求失败」(传输层,可退避重试)与「引擎返回 <code>」
        // (HTTP 状态,按码分类)区分,改动前缀前先看那边。
        let resp = agent
            .post(&format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .set("Authorization", &format!("Bearer {}", crate::engine::ENGINE_TOKEN))
            .set("Content-Type", "application/json")
            .timeout(Duration::from_secs(self.timeout_secs))
            .send_string(&body)
            .map_err(|e| match e {
                // ureq 2.x:非 2xx 是 Err(Status)——规整成「引擎返回 <code>」
                // 前缀,别让它混进传输层错误。
                ureq::Error::Status(code, r) => {
                    let detail = r.into_string().map(|s| shorten(&s, 300)).unwrap_or_default();
                    format!("引擎返回 {code}({}):{detail}", req.model)
                }
                _ => format!("引擎请求失败({}):{}", req.model, shorten(&e.to_string(), 300)),
            })?;
        let status = resp.status();
        let text = resp.into_string().map_err(|e| format!("读取引擎应答失败:{e}"))?;
        if !(200..300).contains(&status) {
            return Err(format!("引擎返回 {status}({}):{}", req.model, shorten(&text, 300)));
        }
        let parsed: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("解析引擎应答失败:{e}"))?;
        let content = parsed
            .pointer("/choices/0/message/content")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if content.is_empty() {
            return Err(format!("引擎返回空内容({})", req.model));
        }
        let output_chars = content.chars().count();
        Ok(WorkerOutput { content, prompt_chars, output_chars })
    }
}

/// 测试用脚本化客户端:按 model id 排队弹出响应;队列为空 → 错误
/// (模拟失败注入)。记录全部调用供断言(哪些模型收到了什么)。
pub struct MockClient {
    /// model id → 响应队列(循环弹出)。
    scripts: Mutex<std::collections::HashMap<String, VecDeque<String>>>,
    /// model id → 一次性错误注入(先进先出,耗尽后回落响应脚本)。
    failures: Mutex<std::collections::HashMap<String, VecDeque<String>>>,
    /// (model, system+user 摘要) 调用记录。
    pub calls: Mutex<Vec<MockCall>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MockCall {
    pub model: String,
    pub prompt: String,
}

impl MockClient {
    pub fn new() -> Self {
        MockClient {
            scripts: Mutex::new(std::collections::HashMap::new()),
            failures: Mutex::new(std::collections::HashMap::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// 给某模型入队一组响应(循环使用,永不耗尽)。
    pub fn enqueue(&self, model: &str, responses: Vec<String>) {
        self.scripts.lock().unwrap().insert(model.to_string(), responses.into());
    }

    /// 给某模型入队一次性错误(先进先出;耗尽后回落响应脚本)。
    /// 错误文本建议按 EngineClient 的前缀约定书写,配合
    /// `is_transient_error` 测退避重试。
    pub fn enqueue_failures(&self, model: &str, errors: Vec<String>) {
        self.failures.lock().unwrap().insert(model.to_string(), errors.into());
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    pub fn models_called(&self) -> Vec<String> {
        self.calls.lock().unwrap().iter().map(|c| c.model.clone()).collect()
    }
}

impl Default for MockClient {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelClient for MockClient {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn complete(&self, req: &WorkerRequest) -> Result<WorkerOutput, String> {
        self.calls
            .lock()
            .unwrap()
            .push(MockCall { model: req.model.clone(), prompt: format!("{}||{}", req.system, req.user) });
        if let Some(queue) = self.failures.lock().unwrap().get_mut(&req.model) {
            if !queue.is_empty() {
                return Err(queue.pop_front().unwrap());
            }
        }
        let mut scripts = self.scripts.lock().unwrap();
        match scripts.get_mut(&req.model) {
            Some(queue) if !queue.is_empty() => {
                let content = queue.pop_front().unwrap();
                // 循环:重新入队,长 fan-out 测试不耗尽。
                queue.push_back(content.clone());
                let prompt_chars = req.system.chars().count() + req.user.chars().count();
                let output_chars = content.chars().count();
                Ok(WorkerOutput { content, prompt_chars, output_chars })
            }
            _ => Err(format!("mock 队列无响应({})", req.model)),
        }
    }
}

fn shorten(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}…")
    }
}

/// 瞬时错误判定(DSH 式:瞬时与结构性分开,只有前者值得退避重试):
/// - 「引擎请求失败」= 传输层(超时/连接拒绝/重置/DNS)→ 瞬时;
/// - 「引擎返回 <code>」按状态码:408/429/5xx 瞬时,其余结构性;
/// - 解析失败/空内容/引擎未运行等是结构性错误,重试无意义。
/// EngineClient 的错误前缀是本文件拼的契约;Mock 注入的错误按同样
/// 约定书写。
pub fn is_transient_error(msg: &str) -> bool {
    if msg.starts_with("引擎请求失败") {
        return true;
    }
    if let Some(rest) = msg.strip_prefix("引擎返回 ") {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(code) = digits.parse::<u16>() {
            return code == 408 || code == 429 || (500..600).contains(&code);
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_client_distinguishes_models() {
        let mock = MockClient::new();
        mock.enqueue("glm/x", vec!["GLM 提案".into()]);
        mock.enqueue("gpt/y", vec!["GPT 提案".into()]);
        let a = mock.complete(&WorkerRequest { model: "glm/x".into(), system: "s".into(), user: "u".into(), max_tokens: None }).unwrap();
        let b = mock.complete(&WorkerRequest { model: "gpt/y".into(), system: "s".into(), user: "u".into(), max_tokens: None }).unwrap();
        assert_ne!(a.content, b.content);
        // 未编排的模型 → 失败注入。
        assert!(mock.complete(&WorkerRequest { model: "other/z".into(), system: "s".into(), user: "u".into(), max_tokens: None }).is_err());
        assert_eq!(mock.call_count(), 3);
    }

    #[test]
    fn engine_client_reports_missing_engine() {
        // 本机恰有引擎在跑时跳过(测试不依赖真实引擎,验收 #14 的
        // mock/live 区分原则);否则必须给人话错误而不是 panic。
        if crate::engine::running_base().is_some() {
            return;
        }
        let client = EngineClient { timeout_secs: 1 };
        let err = client
            .complete(&WorkerRequest { model: "m".into(), system: "s".into(), user: "u".into(), max_tokens: None })
            .err()
            .expect("应当失败");
        assert!(err.contains("引擎"), "错误信息应指向引擎:{err}");
    }

    #[test]
    fn transient_error_classification() {
        // 传输层 → 瞬时(超时/连接问题都走「引擎请求失败」前缀)。
        assert!(is_transient_error("引擎请求失败(glm/x): connection refused"));
        assert!(is_transient_error("引擎请求失败(glm/x): network timed out"));
        // HTTP 状态:408/429/5xx 瞬时。
        assert!(is_transient_error("引擎返回 429(glm/x):rate limited"));
        assert!(is_transient_error("引擎返回 503(glm/x):overloaded"));
        assert!(is_transient_error("引擎返回 408(glm/x):timeout"));
        // 结构性:其它状态码与本地错误。
        assert!(!is_transient_error("引擎返回 400(glm/x):bad request"));
        assert!(!is_transient_error("引擎返回 401(glm/x):unauthorized"));
        assert!(!is_transient_error("引擎返回空内容(glm/x)"));
        assert!(!is_transient_error("解析引擎应答失败:expected value"));
        assert!(!is_transient_error("模型共享引擎未运行(无法发起 Council 模型调用)"));
        assert!(!is_transient_error("mock 队列无响应(glm/x)"));
        assert!(!is_transient_error(""));
    }

    #[test]
    fn mock_failure_injection_then_fallback() {
        let mock = MockClient::new();
        mock.enqueue("glm/x", vec!["恢复".into()]);
        mock.enqueue_failures("glm/x", vec![
            "引擎返回 503(glm/x):overloaded".into(),
            "引擎返回 429(glm/x):rate limited".into(),
        ]);
        let req = WorkerRequest { model: "glm/x".into(), system: "s".into(), user: "u".into(), max_tokens: None };
        assert!(mock.complete(&req).is_err());
        assert!(mock.complete(&req).is_err());
        let ok = mock.complete(&req).unwrap();
        assert_eq!(ok.content, "恢复");
        assert_eq!(mock.call_count(), 3);
    }
}
