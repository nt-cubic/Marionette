//! Claude 订阅账号（Pro / Max）的**核心对话回路**——Go 版
//! `internal/gateway/claude_subscription.go`（8224 行）的最小移植。
//!
//! 为什么是桥：Claude 订阅的流量必须走本机 `claude` CLI 子进程——
//! Anthropic 对订阅资格的判定看的是**请求内容**而不只是凭据，Pi / OpenCode
//! 等第三方 harness 生成的 prompt 直连 api.anthropic.com 会被路由成
//! Extra Usage 计费（Go claude_subscription.go:3-7 文件头）。所以本模块
//! **绝不拿 OAuth token 直连 api.anthropic.com**，一切流量都让真的
//! Claude Code 进程自己去发；Marionette 只做「渲染 prompt → spawn →
//! stream-json 翻译回 anthropic SSE」。
//!
//! 范围（必做核心）：
//! - 凭据发现：`$CLAUDE_CONFIG_DIR|~/.claude/.credentials.json` 的
//!   `claudeAiOauth.accessToken`（Go `parseClaudeCredentials`，
//!   provider/account.go:488；只读，绝不写/刷新），加引擎 logins.json 的
//!   `agent=="claude"` 条目兜底；`claude` CLI 在 PATH 上的存在性
//!   （`where.exe`）。发现不到 → 本模块整体 inactive（`intercept`
//!   返回 None 让位）。
//! - 桥调用：入站 anthropic `/v1/messages` → 提取 system+messages 渲染成
//!   文本 prompt（Go `renderClaudePrompt` :5193 / `renderCachedParts`
//!   :5741 的简化版）→ `claude -p --output-format stream-json
//!   --input-format stream-json --verbose …`（Go `claudeCLIArgs` :3957
//!   1:1 抄，去掉 MCP helper 相关）→ stdin 一行
//!   `{"type":"user","message":{role,content:[text 块]}}`（Go :1481
//!   `startRun` 的喂法）→ stdout 逐行 stream-json（`stream_event.event`
//!   本来就是 anthropic SSE 事件，直接透传成 SSE 帧；`result` 行收口，
//!   Go `readOutput` :4310 的口径）→ 非流式客户端本地聚合。
//! - 模型映射：入站模型名剥 `claude/` 前缀后 `--model` 直传（Go
//!   `serveClaudeSubscription` :7262「by Anthropic's id」）。
//! - 额度行：`rate_limit_event` 的窗口（Go `claudeLimits` :5087——
//!   Anthropic 告诉 Claude Code 的，最诚实）优先；没有跑过对话时兜底
//!   `claude -p /usage`（Go `claudeUsageArgs` claude_warmup.go:516——
//!   Go 的额度也**全是 CLI 子进程**，从不拿 token 打 HTTP）。
//! - `intercept`：与 codex_account / grok_account 同形。
//!
//! 安全红线：**绝不写/刷新凭据文件；绝不碰 api.anthropic.com（桥的全部
//! 意义）；不加依赖；子进程走 std::process**。令牌值只读不存（Account
//! 连 token 字段都没有——对话路径和额度路径都用不到它，CLI 自己认账）。
//!
//! ## 差距清单（Go 有、本核心路径不做，后续批次可领）
//! 每条注明 Go 文件/函数，接手时按图索骥：
//! 1. **会话持久化 / replay / --resume**：Go `savedSession` / `unshelve` /
//!    `branch` / `cutSession*` / `replaySession`（claude_subscription.go
//!    :2753-3373、:933 `start` 的 from 分支）。每回合新建进程 = 每次把
//!    整段对话重新写缓存（Go 文件头 :15-29 解释的正是这个）；核心路径
//!    先保证正确，缓存经济性后补。
//! 2. **跨回合驻留进程**（conversation→run 匹配、idle 队列、2h 淘汰）：
//!    Go `keepIdle` / `match` / `resume`（:2623、:5851、:2033）。同上。
//! 3. **调用方工具（MCP helper）**：Go 把 caller 的工具经隐藏 stdio
//!    helper 挂给 Claude Code（`bridgeTools` :8137、`mcpCall` :6479、
//!    tools/call 阻塞-回填协议）。本路径 `--tools ""`——Claude Code
//!    自带工具全关，caller 的工具定义整个丢弃（请求里的 tools 不透传）。
//! 4. **warmup / 探活**：Go `claude_warmup.go`（askClaude / warmClaude）。
//! 5. **图片**：入站 image 块渲染成 `[image omitted]` 文本（Go
//!    `imageBlocks` :5798 真的会传 base64/url 图）；lost media、URL 图
//!    抓取（`fetchURLImages` :5525）全不做。
//! 6. **上下文编辑 / 客户端缓存标记**（clientCache、`cacheMark`、
//!    历史分块 `historyChunk`）：Go :5303-5319 那套 cache_control 编排，
//!    本路径单文本块、不带 cache 标记。
//! 7. **多账号**（`claude-accounts/` 目录、`inClaudeDir` :3984、logins
//!    侧账号切换）：只认本机当前登录的那份。
//! 8. **interactive TUI 模式**（`claudeTUI` :1568、claudetui shim）、
//!    `--resume` 分支会话切割、499 rewind（`rewind` :3630）。
//! 9. **精细节流 / 失败语义**：refusal fallback、cut-short 重试
//!    （`refused`/`cut` 状态机，readOutput :4325-4360）、text-written
//!    tool calls（`textCalls`，claude_text_calls.go）、StructuredOutput
//!    schema（`cliSchema` :6829）、safeguards（:1706）。
//! 10. **`GET /v1/magpie/claude-runs`** 观测端点、`limitHeaders`
//!     （:7273 anthropic-ratelimit-unified-* 回写）。
//! 11. **`claudeRunName` 的 `[1m]` 后缀**（:3939，未知 gen≥5 模型撑 1M
//!     窗口）与 lean prompt / append-system-prompt（:5413）。
//! 12. **chat / responses 入站协议**：只接 anthropic（Marionette 的
//!     Claude Code agent 打 /v1/messages）；`/v1/chat/completions`、
//!     `/v1/responses` 的 claude/* 模型不接（返回 None 走原路径 404）。
//!     translate.rs 没有 responses→anthropic 请求翻译，接了也是半吊子。
//! 13. **`.claude.json` live 模型表**（Go `claude_code_models.go` 读
//!     additionalModelOptionsCache / modelAccessCache）：模型行用静态表。
//! 14. **/usage 的 Extra Usage 行**（Go `claudeExtraUsage`
//!     claude_warmup.go:243，usage_report.rate_limits.extra_usage）。
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use super::catalog::VendorRow;
use super::effort::Proto;
use super::httpd::{Reply, Request};
use super::store::Provider;

pub const CLAUDE_ID: &str = "claude";
const QUOTA_NAME: &str = "Claude (Pro/Max)";
/// 桥进程整回合硬上限（Go `runTimeout` 同量级：run.timer，一个回合等
/// caller 工具结果最长 10 分钟）。
const BRIDGE_TIMEOUT: Duration = Duration::from_secs(600);
/// /usage 读的硬上限（Go runClaudeTree 的 ctx 截断同量级）。
const USAGE_TIMEOUT: Duration = Duration::from_secs(60);
/// rate_limit_event 额度读数的新鲜期（窗口 5h/7d，半小时前的读数足够新）。
const LIMITS_FRESH: Duration = Duration::from_secs(30 * 60);
/// /usage 兜底的最短间隔（CLI 一次要几秒，别被轮询打爆）。
const USAGE_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// CLI 在 PATH 上的查找结果缓存（where.exe 也要 fork）。
const BINARY_TTL: Duration = Duration::from_secs(30);
/// stderr 只留尾巴用于错误消息（Go endedBefore 的 clipTail 300 字符）。
const STDERR_TAIL: usize = 4 * 1024;

/// Go claudeRunSettings（claude_warmup.go:514）去掉引号问题的取舍：
/// 我们不传 `--settings`（值里带 `"`，.cmd 形态的 claude 会被 cmd 的引号
/// 规则弄坏；这两个设置只是 deep link 注册与思考摘要显示，CLI 默认即可）。
/// 参数表因此**全数组、无引号字符**——.cmd/.bat 走 `cmd /C` 也安全。

// =====================================================================
// 测试缝（cfg(test) 才存在；生产路径恒走真实位置）
// =====================================================================

/// 一次桥调用的可执行形态：program + 前置参数（测试里是
/// `node fake-claude.js`；生产是 `claude` 或 `CLAUDE_BINARY`，无前置参数）。
#[derive(Clone)]
pub(crate) struct Program {
    pub program: String,
    pub prefix: Vec<String>,
}

#[cfg(test)]
#[derive(Clone)]
pub(crate) struct Seam {
    /// Claude 的配置目录（.credentials.json 的家）。
    pub home: PathBuf,
    /// 引擎 logins.json 所在目录（None = 测试里不读 logins）。
    pub logins: Option<PathBuf>,
    /// 假 CLI（None = 模拟「PATH 上没有 claude」）。
    pub cli: Option<Program>,
}

/// 测试缝：未设置时 **没有任何凭据来源、没有 CLI**（测试默认读不到真实账号）。
#[cfg(test)]
pub(crate) static TEST_SEAM: Mutex<Option<Seam>> = Mutex::new(None);

#[cfg(test)]
fn seam() -> Option<Seam> {
    TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

// =====================================================================
// 凭据发现（只读）
// =====================================================================

/// 发现到的一份 Claude 登录。**没有 token 字段**：对话与额度全走 CLI
/// 子进程，token 只用来判断「签没签过到」，不需要带在身上。
#[derive(Clone)]
pub(crate) struct Account {
    /// claudeAiOauth.subscriptionType（"pro"/"max"/…，展示用）。
    plan: String,
    /// accessToken 的 expiresAt（unix 秒；ms→s）。不做硬拦截——CLI 会
    /// 自己刷新，文件里过期不等于账号不可用（Go 也只看非空，
    /// provider/account.go:488-504）。
    #[allow(dead_code)]
    exp: Option<i64>,
}

fn now_secs() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Go appdir.ClaudeDir（appdir/claude.go:18）：`$CLAUDE_CONFIG_DIR` 优先
/// （绝对路径才认），否则 `~/.claude`。
#[cfg(not(test))]
fn claude_config_dir() -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        let p = PathBuf::from(&d);
        if p.is_absolute() {
            return Some(p);
        }
    }
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(|h| PathBuf::from(h).join(".claude"))
}
#[cfg(test)]
fn claude_config_dir() -> Option<PathBuf> {
    seam().map(|s| s.home)
}

/// 引擎数据目录里的 logins.json（Go readLogins 的 `agent=="claude"` 条目，
/// provider/claude_dirs.go：logins.json 记的是保存账号，auth 是整份
/// credentials JSON）。
#[cfg(not(test))]
fn logins_dir() -> Option<PathBuf> {
    super::config_dir()
}
#[cfg(test)]
fn logins_dir() -> Option<PathBuf> {
    seam().and_then(|s| s.logins)
}

/// Go parseClaudeCredentials（provider/account.go:488）：`.credentials.json`
/// 的 `claudeAiOauth.accessToken` 非空才算签着到（cleared = 空 access +
/// 空 refresh 是 CLI 主动登出，读作没有）。
fn oauth_of(raw: &Value) -> Option<&Value> {
    raw.get("claudeAiOauth").filter(|o| o.is_object())
}

fn account_of(oauth: &Value) -> Option<Account> {
    let access = oauth.get("accessToken").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
    if access.is_empty() {
        return None;
    }
    let refresh = oauth.get("refreshToken").and_then(|t| t.as_str()).unwrap_or("").trim();
    if refresh.is_empty() && access.is_empty() {
        return None; // cleared（Go :53-58 的口径；access 已空时到不了这）
    }
    let exp = oauth
        .get("expiresAt")
        .and_then(|e| e.as_i64())
        .filter(|e| *e > 0)
        .map(|ms| ms / 1000);
    Some(Account { plan: oauth.get("subscriptionType").and_then(|s| s.as_str()).unwrap_or("").to_string(), exp })
}

/// 凭据发现（每次现读、只读）：`~/.claude/.credentials.json` 优先，没有再
/// 看 logins.json 的 claude 条目（取 expiresAt 更晚的）。发现不到 = None。
pub(crate) fn discover() -> Option<Account> {
    let mut found: Vec<Account> = Vec::new();
    if let Some(dir) = claude_config_dir() {
        if let Ok(b) = std::fs::read(dir.join(".credentials.json")) {
            if let Ok(raw) = serde_json::from_slice::<Value>(&b) {
                if let Some(a) = oauth_of(&raw).and_then(account_of) {
                    found.push(a);
                }
            }
        }
    }
    if let Some(dir) = logins_dir() {
        if let Some(list) = std::fs::read(dir.join("logins.json"))
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        {
            for entry in list.as_array().into_iter().flatten() {
                if entry.get("agent").and_then(|a| a.as_str()) != Some("claude") {
                    continue;
                }
                let Some(auth) = entry.get("auth").filter(|a| a.is_object()) else { continue };
                if let Some(a) = oauth_of(auth).and_then(account_of) {
                    found.push(a);
                }
            }
        }
    }
    // 到期更晚的赢；平局先出现的赢（.credentials.json 在前——Go
    // readClaudeCredential 同序）。
    let mut best: Option<Account> = None;
    for a in found {
        let better = match &best {
            None => true,
            Some(b) => a.exp.unwrap_or(0) > b.exp.unwrap_or(0),
        };
        if better {
            best = Some(a);
        }
    }
    best
}

// =====================================================================
// CLI 存在性
// =====================================================================

/// PATH 上找 `claude`（Go claudeBinary :768 的存在性判断；`CLAUDE_BINARY`
/// 环境变量可钉死路径）。结果缓存 30s。Windows 上 npm 装的 claude 是
/// .cmd——返回路径本身，spawn 时再决定走不走 cmd /C。
#[cfg(not(test))]
fn claude_binary() -> Option<Program> {
    static CACHE: Mutex<Option<(Instant, Option<Program>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, hit)) = cache.as_ref() {
        if at.elapsed() < BINARY_TTL {
            return hit.clone();
        }
    }
    let found = find_claude_on_path().map(|p| Program { program: p, prefix: Vec::new() });
    *cache = Some((Instant::now(), found.clone()));
    found
}

#[cfg(not(test))]
fn find_claude_on_path() -> Option<String> {
    if let Some(p) = std::env::var_os("CLAUDE_BINARY").filter(|p| !p.is_empty()) {
        return Some(p.to_string_lossy().into_owned());
    }
    let mut cmd = Command::new("where.exe");
    cmd.arg("claude");
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let line = String::from_utf8_lossy(&out.stdout).lines().next()?.trim().to_string();
    if line.is_empty() {
        None
    } else {
        Some(line)
    }
}

/// 测试：seam.cli 说了算（None = 模拟没有 CLI；测试绝不真找 PATH）。
#[cfg(test)]
fn claude_binary() -> Option<Program> {
    seam().and_then(|s| s.cli)
}

/// 本模块整体是否激活：凭据 + CLI 都在。
fn available() -> bool {
    discover().is_some() && claude_binary().is_some()
}

// =====================================================================
// 合成来源与模型目录
// =====================================================================

/// 合成来源 `claude`（只在内存，**绝不写进 providers.json**）。anthropic
/// 字段是不可达占位——真流量由 intercept 在路由层截走，永不发 HTTP。
pub fn synthetic_provider() -> Option<Provider> {
    discover()?;
    Some(Provider {
        id: CLAUDE_ID.to_string(),
        name: "Claude".to_string(),
        anthropic: "http://127.0.0.1:9/claude-bridge".to_string(),
        ..Default::default()
    })
}

/// Go claudeKnownLong（claude_subscription.go:3919）：Claude Code (2.1.316)
/// 认识的、按自身 id 跑 1M 窗口的模型。静态表——live 表是差距清单第 13 条。
const MODELS: [(&str, u64); 9] = [
    ("claude-fable-5", 1_000_000),
    ("claude-fable-5-1", 1_000_000),
    ("claude-mythos-5", 1_000_000),
    ("claude-mythos-5-1", 1_000_000),
    ("claude-opus-5", 1_000_000),
    ("claude-opus-5-5", 1_000_000),
    ("claude-sonnet-5", 1_000_000),
    ("claude-sonnet-5-5", 1_000_000),
    ("claude-haiku-5-5", 1_000_000),
];

/// `/v1/models` 的 claude 行（有凭据 + CLI + 用户没自定义 claude 来源才有）。
pub fn listed_rows(providers: &[Provider]) -> Vec<VendorRow> {
    if providers.iter().any(|p| p.id == CLAUDE_ID) || !available() {
        return Vec::new();
    }
    MODELS
        .iter()
        .map(|(id, ctx)| VendorRow {
            id: id.to_string(),
            context_window: *ctx,
            max_output: 0,
            images: false,
            raw: json!({ "id": id, "display_name": id }),
        })
        .collect()
}

/// 整串 `claude/<id>` 是不是本来源能接的。放行任意非空 id——CLI 的
/// `--model` 对未知名字自己给默认/报错（Go `p.OwnModelID` 直传同语义）。
fn knows(whole: &str) -> bool {
    match whole.strip_prefix("claude/") {
        Some(rest) => !rest.is_empty() && !rest.contains('/'),
        None => false,
    }
}

// =====================================================================
// prompt 渲染（Go renderClaudePrompt :5193 的简化：单文本块、无缓存标记）
// =====================================================================

/// Go historyOpen/historyClose（:5440-5444）。
const HISTORY_OPEN: &str = "<conversation_history>\nThe conversation so far, each turn of it answered already. Images and files in it were sent with those earlier messages: none of them is new, and none needs answering again.\n\n";
const HISTORY_CLOSE: &str = "</conversation_history>\n\n<current_turn>\nThe turn to answer now:\n\n";
/// Go pastCallsNote（claude_text_calls.go:28）。
const PAST_CALLS_NOTE: &str = "(The \"[tool call …]\" and \"[tool result …]\" lines above record calls made earlier. Writing such a line calls nothing: call a tool only with the tool itself.)";

/// system 字段（字符串或 text 块数组）→ 纯文本。
fn system_text(system: &Value) -> String {
    match system {
        Value::String(s) => s.trim().to_string(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string(),
        _ => String::new(),
    }
}

/// 一条消息的 content 块里有没有 tool_use（Go callsTool :2358）。
fn calls_tool(content: &Value) -> bool {
    content
        .as_array()
        .is_some_and(|blocks| blocks.iter().any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_use")))
}

/// Go historyEnd（:5578）：最后一条「不带工具调用的 assistant 回复」之后是
/// 本回合要答的；全是 user 或没有这样的回复 → 0（整段都是当前回合）。
fn history_end(messages: &[Value]) -> usize {
    for j in (0..messages.len()).rev() {
        let m = &messages[j];
        if m.get("role").and_then(|r| r.as_str()) == Some("assistant") && !calls_tool(m.get("content").unwrap_or(&Value::Null)) {
            if j == messages.len() - 1 {
                return 0;
            }
            return j + 1;
        }
    }
    0
}

/// content 块数组 → 文本片段（Go renderCachedParts :5741 的核心路径：
/// text 直写；thinking 跳过——「thinking 永远不当话说」（:5760 注释）；
/// tool_use / tool_result 写成 `[tool call …]` / `[tool result …]` 行；
/// tool_result 后同消息的 text 是用户的（:5750 的 [user message] 分隔）；
/// image 只留占位（差距清单第 5 条））。
fn render_blocks(blocks: &[Value], text: &mut String, open_result: &mut Option<String>) {
    let close_result = |text: &mut String, open: &mut Option<String>, as_user: bool| {
        if let Some(id) = open.take() {
            if as_user {
                text.push_str(&format!("\n[end of tool result id={id}]\n\n[user message]\n"));
            } else {
                text.push_str(&format!("\n[end of tool result id={id}]\n\n"));
            }
        }
    };
    for b in blocks {
        let ty = b.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match ty {
            "text" => {
                let t = b.get("text").and_then(|x| x.as_str()).unwrap_or("");
                if !t.is_empty() {
                    // tool_result 之后的第一段文字是用户说的，不是工具输出
                    close_result(text, open_result, true);
                    text.push_str(t);
                }
            }
            "thinking" | "redacted_thinking" => {}
            "tool_use" => {
                let name = b.get("name").and_then(|x| x.as_str()).unwrap_or("");
                let id = b.get("id").and_then(|x| x.as_str()).unwrap_or("");
                let args = b.get("input").cloned().unwrap_or(json!({}));
                close_result(text, open_result, false);
                text.push_str(&format!("\n[tool call {name} id={id} args={args}]"));
            }
            "tool_result" => {
                let id = b.get("tool_use_id").and_then(|x| x.as_str()).unwrap_or("");
                let is_error = b.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false);
                let err = if is_error { " error" } else { "" };
                close_result(text, open_result, false);
                let body = match b.get("content").unwrap_or(&Value::Null) {
                    Value::String(s) => s.clone(),
                    Value::Array(parts) => parts
                        .iter()
                        .map(|p| match p.get("type").and_then(|t| t.as_str()) {
                            Some("text") => p.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                            Some("image") => "[image omitted]".to_string(),
                            _ => String::new(),
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                    _ => String::new(),
                };
                text.push_str(&format!("\n[tool result id={id}{err}]\n{body}"));
                *open_result = Some(id.to_string());
            }
            "image" => {
                text.push_str("[image omitted]");
            }
            _ => {}
        }
    }
}

/// 一条消息（content 可为字符串）→ 文本。
fn render_message(m: &Value, text: &mut String, open_result: &mut Option<String>) {
    match m.get("content").unwrap_or(&Value::Null) {
        Value::String(s) => {
            if !s.is_empty() {
                if let Some(id) = open_result.take() {
                    text.push_str(&format!("\n[end of tool result id={id}]\n\n[user message]\n"));
                }
                text.push_str(s);
            }
        }
        Value::Array(blocks) => render_blocks(blocks, text, open_result),
        _ => {}
    }
}

/// 入站 anthropic 请求 → CLI stdin 的 content 文本块（Go renderClaudePrompt
/// 的简化：system 一块 + 对话一块，历史/当前回合用 Go 原样的标记包裹）。
fn render_prompt(body: &Value) -> Vec<Value> {
    let messages = body.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default();
    let mut blocks: Vec<Value> = Vec::new();
    // caller 的指令自成一块（Go :5208-5240；cache_control 是差距第 6 条）
    let system = system_text(body.get("system").unwrap_or(&Value::Null));
    if !system.is_empty() {
        blocks.push(json!({
            "type": "text",
            "text": format!("<external_system_instructions>\n{system}\n</external_system_instructions>\n\n")
        }));
    }
    let split = history_end(&messages);
    let mut text = String::new();
    let mut open_result: Option<String> = None;
    if split > 0 {
        text.push_str(HISTORY_OPEN);
    }
    for (i, m) in messages.iter().enumerate() {
        if split > 0 && i == split {
            // 历史里没收尾的 tool_result 在回合边界收掉
            if let Some(id) = open_result.take() {
                text.push_str(&format!("\n[end of tool result id={id}]\n\n"));
            }
            text.push_str(HISTORY_CLOSE);
        }
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        let label = if role == "assistant" { "Assistant" } else { "Human" };
        if split > 0 || i > 0 {
            text.push_str("\n\n");
        }
        text.push_str(&format!("{label}: "));
        render_message(m, &mut text, &mut open_result);
    }
    // Go :5286 / :5721：对话里**任何**一条消息带工具调用 → pastCallsNote
    // （模型会把 `[tool call …]` 文本当调用方式，得告诉它那只是记录）
    if messages.iter().any(|m| calls_tool(m.get("content").unwrap_or(&Value::Null))) {
        text.push_str("\n\n");
        text.push_str(PAST_CALLS_NOTE);
    }
    if split > 0 {
        text.push_str("\n\n</current_turn>");
    }
    // Go closeBlocks :5813：空 prompt 兜底 [continue]
    if !text.trim().is_empty() {
        blocks.push(json!({ "type": "text", "text": text }));
    } else if blocks.is_empty() {
        blocks.push(json!({ "type": "text", "text": "[continue]" }));
    }
    blocks
}

// =====================================================================
// 桥子进程
// =====================================================================

/// Go claudeCLIArgs（:3957）：`--mcp-config`（helper）与 `--settings` 不带
/// （本路径没有 MCP helper；settings 值带引号，见文件头说明）。
fn bridge_args(model: &str, effort: Option<&str>) -> Vec<String> {
    let mut args: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--input-format",
        "stream-json",
        "--include-partial-messages",
        "--verbose",
        "--model",
        model,
        "--tools",
        "",
        "--strict-mcp-config",
        "--setting-sources",
        "",
        "--dangerously-skip-permissions",
        "--no-session-persistence",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(e) = effort {
        args.push("--effort".into());
        args.push(e.to_string());
        args.push("--thinking-display".into());
        args.push("summarized".into());
    }
    args
}

/// Go claudeUsageArgs（claude_warmup.go:516，--settings 同上不带）。
fn usage_args() -> Vec<String> {
    ["-p", "/usage", "--output-format", "json", "--verbose", "--tools", "", "--strict-mcp-config", "--setting-sources", "", "--no-session-persistence"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Go cleanClaudeEnvTTL（:4116）要摘掉的环境变量——**尤其 ANTHROPIC_BASE_URL**：
/// agent 的 env 里它指着本引擎，不摘 claude CLI 会把请求打回我们自己（死循环）。
const BLOCKED_ENV: [&str; 11] = [
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_AUTH_TOKEN",
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SSE_PORT",
    "CLAUDE_CODE_OAUTH_TOKEN",
    "CLAUDE_CODE_PROMPT_CACHE_TTL",
    "FORCE_PROMPT_CACHING_5M",
    "CLAUDE_CODE_EXTRA_BODY",
    "ANTHROPIC_BETAS",
];

/// Go claudeWorkDir（:552）的对应物：temp 下一个稳定的共享工作目录（不进
/// git、每回合同一个——Claude Code 把 cwd 写进 system prompt，目录漂移会
/// 打碎 prompt 缓存）。建不出来就退当前目录（能跑，只是缓存经济性差）。
fn work_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("marionette-claude-work");
    if std::fs::create_dir_all(&dir).is_err() {
        return std::env::temp_dir();
    }
    dir
}

fn build_command(program: &Program, args: &[String]) -> Command {
    let mut cmd = if program.prefix.is_empty() {
        let mut c = Command::new(&program.program);
        c.args(args);
        c
    } else {
        let mut c = Command::new(&program.program);
        c.args(&program.prefix);
        c.args(args);
        c
    };
    // npm 装的 claude 是 .cmd：Rust std 直 spawn .bat/.cmd 的参数转义有
    // BatBadBut 约束（含 `"` 即 InvalidInput）——我们参数表无引号，但统一
    // 走 cmd /C 更稳（cmd 自己处理 .cmd 文件）。
    let lower = program.program.to_ascii_lowercase();
    if lower.ends_with(".cmd") || lower.ends_with(".bat") {
        let mut c = Command::new("cmd.exe");
        c.arg("/C").arg(&program.program).args(args);
        cmd = c;
    }
    for k in BLOCKED_ENV {
        cmd.env_remove(k);
    }
    // Go cleanClaudeEnvTTL 追加的固定项（TTL 固定 1h——用户偏好读取是差距）
    cmd.env("ENABLE_CLAUDEAI_MCP_SERVERS", "0");
    cmd.env("DISABLE_AUTO_COMPACT", "1");
    cmd.env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1");
    cmd.env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1");
    cmd.env("CLAUDE_CODE_DISABLE_GIT_INSTRUCTIONS", "1");
    cmd.env("CLAUDE_CODE_PROMPT_CACHE_TTL", "1h");
    cmd.current_dir(work_dir());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// 一条 stdout envelope 经翻译后的产物。
enum Step {
    /// 一条 anthropic 事件（stream_event 透传，或整消息合成）。
    Event(Value),
    /// 终局错误（result.is_error / 桥提前退场 / 超时）。
    Failed(u16, String),
    /// rate_limit_event 的额度窗口（喂 quota 缓存后继续）。
    Limits(Vec<Value>),
}

/// 桥进程 + stream-json → anthropic SSE 的流式翻译器。
/// 流式客户端直接当 Read 用；非流式客户端用 [`BridgeStream::aggregate`]。
pub(crate) struct BridgeStream {
    child: Arc<Mutex<Child>>,
    stdin: Option<std::process::ChildStdin>,
    done: Arc<AtomicBool>,
    stdout: BufReader<std::process::ChildStdout>,
    stderr_tail: Arc<Mutex<String>>,
    /// 待写出的 SSE 字节（pos 之前已消费）。
    out: Vec<u8>,
    pos: usize,
    finished: bool,
    /// 整条 assistant 消息合成的帧族，还没吐完（pump 先弹这里再读新行）。
    pending: Vec<Value>,
    /// message_start 已见（整消息兜底只在没流起来时启用）。
    started: bool,
    /// 已经流式输出过的消息 id（Go streamed 表，重复的 whole 行跳过）。
    streamed: Vec<String>,
    /// message_stop 已到。
    got_stop: bool,
    /// result 行已到（回合终局）。
    ended: bool,
    /// 回显给客户端的模型名（message_start 没给时兜底）。
    model: String,
}

/// spawn 桥进程并喂入本回合 prompt（Go startRun :1004 的单回合形态）。
/// `extra_env`：测试给假 CLI 传模式开关。
fn spawn_bridge(
    program: &Program,
    model: &str,
    effort: Option<&str>,
    prompt: &[Value],
    extra_env: &[(String, String)],
) -> Result<BridgeStream, (u16, String)> {
    let args = bridge_args(model, effort);
    let mut cmd = build_command(program, &args);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| (502, format!("claude bridge spawn failed: {e}")))?;
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (stdin, stdout, stderr) = match (stdin, stdout, stderr) {
        (Some(i), Some(o), Some(e)) => (i, o, e),
        _ => {
            let _ = child.kill();
            return Err((502, "claude bridge pipes unavailable".into()));
        }
    };
    // stderr 只留尾巴（错误消息用；Go lockedWriter + endedBefore 的 clipTail）
    let tail = Arc::new(Mutex::new(String::new()));
    {
        let tail = tail.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 1024];
            let mut reader = stderr;
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut g = tail.lock().unwrap_or_else(|p| p.into_inner());
                        g.push_str(&String::from_utf8_lossy(&buf[..n]));
                        let len = g.len();
                        if len > STDERR_TAIL {
                            let cut = len - STDERR_TAIL;
                            let mut boundary = cut;
                            while boundary < g.len() && !g.is_char_boundary(boundary) {
                                boundary += 1;
                            }
                            *g = g[boundary..].to_string();
                        }
                    }
                }
            }
        });
    }
    let child = Arc::new(Mutex::new(child));
    let done = Arc::new(AtomicBool::new(false));
    // 看门狗：回合硬上限到了就杀（Drop 也杀，这里兜「客户端不再读」的挂死）
    {
        let (child, done) = (child.clone(), done.clone());
        std::thread::spawn(move || {
            let deadline = Instant::now() + BRIDGE_TIMEOUT;
            while Instant::now() < deadline {
                if done.load(Ordering::Acquire) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            if !done.load(Ordering::Acquire) {
                if let Ok(mut c) = child.lock() {
                    let _ = c.kill();
                }
            }
        });
    }
    let mut stream = BridgeStream {
        child,
        stdin: Some(stdin),
        done,
        stdout: BufReader::new(stdout),
        stderr_tail: tail,
        out: Vec::new(),
        pos: 0,
        finished: false,
        pending: Vec::new(),
        started: false,
        streamed: Vec::new(),
        got_stop: false,
        ended: false,
        model: model.to_string(),
    };
    // 喂本回合（Go :1481 的 user 行；startRun 先 initialize 控制请求的路径
    // 只有带 MCP helper / safeguards 才需要，这里不用）
    let line = json!({ "type": "user", "message": { "role": "user", "content": prompt } });
    let mut bytes = serde_json::to_vec(&line).unwrap_or_default();
    bytes.push(b'\n');
    if let Some(s) = stream.stdin.as_mut() {
        if s.write_all(&bytes).is_err() {
            return Err((502, "claude bridge closed its input before the turn".into()));
        }
        let _ = s.flush();
    }
    Ok(stream)
}

impl BridgeStream {
    fn close_input(&mut self) {
        self.stdin.take(); // drop = 关管道，CLI 读到 EOF 自行退出
    }

    fn stderr_line(&self) -> String {
        self.stderr_tail.lock().unwrap_or_else(|p| p.into_inner()).trim().to_string()
    }

    /// 读一行 stdout 并翻译。None = 输出结束（EOF 收尾逻辑已发过 Failed 或
    /// 流已正常收口）。整条 assistant 消息合成的帧族先进 `pending`，本函数
    /// 先弹 pending 再读新行。
    fn pump(&mut self) -> Option<Step> {
        if let Some(first) = self.pending.first().cloned() {
            self.pending.remove(0);
            return Some(Step::Event(first));
        }
        if self.ended && self.finished {
            return None;
        }
        let mut line = Vec::new();
        match self.stdout.read_until(b'\n', &mut line) {
            Ok(0) => {
                // EOF：result 没来但回复已正常收口（message_stop 到过）→ 干净
                // 结束；没收口 → 桥死了/被看门狗杀了，如实报错（Go
                // replyUnended :5054 的口径）
                self.finished = true;
                if !self.ended && !self.got_stop {
                    let why = if self.stderr_line().is_empty() {
                        "Claude Code ended before it answered".to_string()
                    } else {
                        format!("Claude Code ended before it answered: {}", clip(&self.stderr_line(), 300))
                    };
                    return Some(Step::Failed(502, why));
                }
                return None;
            }
            Ok(_) => {}
            Err(e) => {
                self.finished = true;
                return Some(Step::Failed(502, format!("claude bridge output: {e}")));
            }
        }
        let Ok(v) = serde_json::from_slice::<Value>(&line) else {
            return Some(Step::Event(Value::Null)); // 杂行安静跳过（pump 循环里过滤）
        };
        if v.is_null() {
            return Some(Step::Event(Value::Null));
        }
        match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "stream_event" => {
                let event = v.get("event").cloned().unwrap_or(Value::Null);
                let ty = event.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match ty {
                    "message_start" => {
                        self.started = true;
                        if let Some(id) = event.pointer("/message/id").and_then(|i| i.as_str()) {
                            self.streamed.push(id.to_string());
                        }
                        if let Some(m) = event.pointer("/message/model").and_then(|m| m.as_str()) {
                            self.model = m.to_string();
                        }
                        Some(Step::Event(event))
                    }
                    "message_stop" => {
                        self.got_stop = true;
                        Some(Step::Event(event))
                    }
                    "message_delta" | "content_block_start" | "content_block_delta" | "content_block_stop" | "ping" => {
                        Some(Step::Event(event))
                    }
                    _ => Some(Step::Event(Value::Null)), // 未知事件安静跳过
                }
            }
            // 整条 assistant 消息：流式路径下是已流过内容的重复（Go 的
            // streamed/unstreamed 语义 :4697）；没流起来（CLI 没给
            // stream_event）时由它合成帧族——回复内容不能丢
            "assistant" => {
                let msg = v.get("message").cloned().unwrap_or(Value::Null);
                let id = msg.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                if self.started || self.streamed.iter().any(|s| *s == id) {
                    return Some(Step::Event(Value::Null));
                }
                let model = msg.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
                let model = if model.is_empty() { self.model.clone() } else { model };
                let usage = msg.get("usage").cloned().unwrap_or_else(|| json!({}));
                let mut frames = vec![json!({
                    "type": "message_start",
                    "message": {
                        "id": if id.is_empty() { "msg_bridge".to_string() } else { id.clone() },
                        "type": "message", "role": "assistant", "model": model,
                        "content": [], "usage": usage.clone(),
                    }
                })];
                let blocks = msg.get("content").and_then(|c| c.as_array()).cloned().unwrap_or_default();
                for (i, b) in blocks.iter().enumerate() {
                    // anthropic 流式规矩：content_block_start 的块体不带正文，
                    // 正文走 delta——直接把整块塞 start 会让拼接正文的客户端
                    // （含本模块 aggregate）读到两遍
                    let mut bare = b.clone();
                    match b.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                        "text" => {
                            bare["text"] = Value::String(String::new());
                        }
                        "thinking" => {
                            bare["thinking"] = Value::String(String::new());
                        }
                        _ => {}
                    }
                    frames.push(json!({"type": "content_block_start", "index": i, "content_block": bare}));
                    match b.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                        "text" => {
                            let t = b.get("text").and_then(|x| x.as_str()).unwrap_or("");
                            if !t.is_empty() {
                                frames.push(json!({"type": "content_block_delta", "index": i,
                                    "delta": {"type": "text_delta", "text": t}}));
                            }
                        }
                        "thinking" => {
                            let t = b.get("thinking").and_then(|x| x.as_str()).unwrap_or("");
                            if !t.is_empty() {
                                frames.push(json!({"type": "content_block_delta", "index": i,
                                    "delta": {"type": "thinking_delta", "thinking": t}}));
                            }
                        }
                        _ => {}
                    }
                    frames.push(json!({"type": "content_block_stop", "index": i}));
                }
                frames.push(json!({"type": "message_delta",
                    "delta": {"stop_reason": msg.get("stop_reason").cloned().unwrap_or_else(|| json!("end_turn")), "stop_sequence": null},
                    "usage": usage}));
                frames.push(json!({"type": "message_stop"}));
                self.pending = frames;
                self.started = true;
                self.got_stop = true;
                if !id.is_empty() {
                    self.streamed.push(id);
                }
                Some(Step::Event(Value::Null))
            }
            // system/init：记 session 的地方（本路径不 --resume，纯记账，跳过）
            "system" => Some(Step::Event(Value::Null)),
            "rate_limit_event" => {
                let info = v.get("rate_limit_info").cloned().unwrap_or(Value::Null);
                let windows = claude_limits_of(&info);
                if !windows.is_empty() {
                    note_limits(&windows);
                }
                Some(Step::Limits(windows))
            }
            // user/tool_use_result、control_response、command_lifecycle：不关心
            "user" | "control_response" | "command_lifecycle" => Some(Step::Event(Value::Null)),
            "result" => {
                self.ended = true;
                self.close_input();
                let is_error = v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
                if is_error {
                    let text = v.get("result").and_then(|r| r.as_str()).unwrap_or("Claude Code error");
                    let status = v.get("api_error_status").and_then(|s| s.as_u64()).unwrap_or(0) as u16;
                    let status = if status == 0 { 502 } else { status };
                    return Some(Step::Failed(status, clip(text, 500)));
                }
                // 正常 result 但流没收口（CLI 没发 message_stop 的怪流）：
                // 补齐收尾帧，客户端不至于挂等（message_stop 由 result 兜底）
                if !self.got_stop {
                    self.got_stop = true;
                    self.pending = vec![
                        json!({"type": "message_delta",
                            "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                            "usage": {}}),
                        json!({"type": "message_stop"}),
                    ];
                }
                Some(Step::Event(Value::Null))
            }
            _ => Some(Step::Event(Value::Null)),
        }
    }

    /// 非流式：消费全部事件，聚合成最终 anthropic message。
    fn aggregate(mut self, asked_model: &str) -> Result<Value, (u16, String)> {
        let mut id = String::new();
        let mut model = String::new();
        let mut usage: Option<Value> = None;
        let mut stop: Option<String> = None;
        let mut blocks: Vec<(usize, Value)> = Vec::new(); // (index, block)
        let mut json_inputs: HashMap<usize, String> = HashMap::new();
        loop {
            match self.pump() {
                Some(Step::Event(v)) => {
                    if v.is_null() {
                        continue;
                    }
                    match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                        "message_start" => {
                            id = v.pointer("/message/id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            model = v.pointer("/message/model").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            if let Some(u) = v.pointer("/message/usage").filter(|u| u.is_object()) {
                                usage = Some(u.clone());
                            }
                        }
                        "content_block_start" => {
                            let index = v.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                            let block = v.get("content_block").cloned().unwrap_or(Value::Null);
                            blocks.push((index, block));
                        }
                        "content_block_delta" => {
                            let index = v.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                            let delta = v.get("delta").cloned().unwrap_or(Value::Null);
                            let Some((_, block)) = blocks.iter_mut().rev().find(|(i, _)| *i == index) else { continue };
                            match delta.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                                "text_delta" => {
                                    let t = delta.get("text").and_then(|x| x.as_str()).unwrap_or("");
                                    let cur = block.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                    block["text"] = Value::String(cur + t);
                                }
                                "thinking_delta" => {
                                    let t = delta.get("thinking").and_then(|x| x.as_str()).unwrap_or("");
                                    let cur = block.get("thinking").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                    block["thinking"] = Value::String(cur + t);
                                }
                                "signature_delta" => {
                                    let t = delta.get("signature").and_then(|x| x.as_str()).unwrap_or("");
                                    let cur = block.get("signature").and_then(|x| x.as_str()).unwrap_or("").to_string();
                                    block["signature"] = Value::String(cur + t);
                                }
                                "input_json_delta" => {
                                    let t = delta.get("partial_json").and_then(|x| x.as_str()).unwrap_or("");
                                    *json_inputs.entry(index).or_default() += t;
                                }
                                _ => {}
                            }
                        }
                        "message_delta" => {
                            if let Some(s) = v.pointer("/delta/stop_reason").and_then(|x| x.as_str()).filter(|s| !s.is_empty()) {
                                stop = Some(s.to_string());
                            }
                            if let Some(u) = v.get("usage").filter(|u| u.is_object()) {
                                usage = Some(u.clone());
                            }
                        }
                        _ => {}
                    }
                }
                Some(Step::Failed(status, msg)) => return Err((status, msg)),
                Some(Step::Limits(_)) => continue,
                None => break,
            }
        }
        // input_json_delta 的分片拼装成对象（拼不出就 {}，别让客户端解析炸）
        for (index, block) in blocks.iter_mut() {
            if let Some(raw) = json_inputs.remove(index) {
                let parsed = serde_json::from_str::<Value>(raw.trim()).unwrap_or_else(|_| json!({}));
                block["input"] = if parsed.is_object() { parsed } else { json!({}) };
            } else if block.get("type").and_then(|t| t.as_str()) == Some("tool_use") && block.get("input").is_none() {
                block["input"] = json!({});
            }
        }
        blocks.sort_by_key(|(i, _)| *i);
        let content: Vec<Value> = blocks.into_iter().map(|(_, b)| b).collect();
        Ok(json!({
            "id": if id.is_empty() { "msg_bridge".to_string() } else { id },
            "type": "message",
            "role": "assistant",
            "model": if model.is_empty() { asked_model.to_string() } else { model },
            "content": content,
            "stop_reason": stop.unwrap_or_else(|| "end_turn".to_string()),
            "stop_sequence": Value::Null,
            "usage": usage.unwrap_or_else(|| json!({"input_tokens": 0, "output_tokens": 0})),
        }))
    }
}

impl Read for BridgeStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        // 整帧字节按读方缓冲的粒度吐；切点退到 UTF-8 字符边界——帧里有
        // 中文正文，read_to_string 这类消费方对半字符敏感
        let serve = |out: &mut Vec<u8>, pos: &mut usize, buf: &mut [u8]| -> usize {
            let mut n = (out.len() - *pos).min(buf.len());
            // UTF-8 边界：切点不在续字节（0b10xxxxxx）上
            while n > 0 && out.get(*pos + n).is_some_and(|b| b & 0xC0 == 0x80) {
                n -= 1;
            }
            buf[..n].copy_from_slice(&out[*pos..*pos + n]);
            *pos += n;
            if *pos >= out.len() {
                out.clear();
                *pos = 0;
            }
            n
        };
        if self.pos < self.out.len() {
            let n = serve(&mut self.out, &mut self.pos, buf);
            return Ok(n);
        }
        if self.finished {
            return Ok(0);
        }
        // 出缓冲空了再泵
        loop {
            match self.pump() {
                Some(Step::Event(v)) => {
                    if v.is_null() {
                        continue;
                    }
                    self.out = sse_frame(&v).into_bytes();
                    self.pos = 0;
                    let n = serve(&mut self.out, &mut self.pos, buf);
                    return Ok(n);
                }
                Some(Step::Failed(status, msg)) => {
                    self.finished = true;
                    self.close_input();
                    self.out = sse_frame(&json!({"type": "error",
                        "error": {"type": error_kind(status), "message": msg}}))
                    .into_bytes();
                    self.pos = 0;
                    let n = serve(&mut self.out, &mut self.pos, buf);
                    return Ok(n);
                }
                Some(Step::Limits(_)) => continue,
                None => {
                    self.finished = true;
                    return Ok(0);
                }
            }
        }
    }
}

impl Drop for BridgeStream {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        self.close_input();
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn clip(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let cut: String = s.chars().take(n).collect();
        format!("{cut}…")
    }
}

/// anthropic SSE 帧（messages.rs sse_frame 同形状；那边是私有 fn，这里是
/// 同款本地实现——事件 JSON 本来就是 anthropic 形状，直接序列化）。
fn sse_frame(event: &Value) -> String {
    let ty = event.get("type").and_then(|t| t.as_str()).unwrap_or("ping");
    let data = serde_json::to_string(event).unwrap_or_else(|_| "{}".into());
    format!("event: {ty}\ndata: {data}\n\n")
}

fn error_kind(status: u16) -> &'static str {
    match status {
        400 | 422 => "invalid_request_error",
        401 => "authentication_error",
        403 => "permission_error",
        404 => "not_found_error",
        429 => "rate_limit_error",
        s if s >= 500 => "api_error",
        _ => "invalid_request_error",
    }
}

/// anthropic 错误信封（grok_account 同款形状）。
fn anthropic_error(status: u16, message: &str) -> Reply {
    Reply::json(status, &json!({ "type": "error", "error": { "type": error_kind(status), "message": message } }))
}

// =====================================================================
// 额度
// =====================================================================

/// Go claudeLimits（:5087）：rate_limit_event 的 unifiedWindows + 顶层
/// rateLimitType（没在 windows 里时）→ 窗口表。utilization 是 0-1。
fn claude_limits_of(info: &Value) -> Vec<Value> {
    if !info.is_object() {
        return Vec::new();
    }
    let mut out: Vec<Value> = Vec::new();
    let mut push = |kind: &str, used: f64, resets_at: i64| {
        let name = match kind {
            "five_hour" => "5 hours",
            "seven_day" => "7 days",
            "seven_day_overage_included" => "7 days · Fable",
            other => return other_check(&mut out, other, used, resets_at),
        };
        out.push(json!({
            "kind": kind,
            "name": name,
            "used": (used * 100.0).round().min(100.0),
            "remaining": ((1.0 - used) * 100.0).round().max(0.0),
            "resetsAt": rfc3339(resets_at),
        }));
    };
    fn other_check(out: &mut Vec<Value>, kind: &str, used: f64, resets_at: i64) {
        // 其它窗口（seven_day_opus / seven_day_sonnet / 未来的）按 kind 原名展示
        let shown = match kind {
            "seven_day_opus" => "7 days · Opus",
            "seven_day_sonnet" => "7 days · Sonnet",
            other => other,
        };
        out.push(json!({
            "kind": kind,
            "name": shown,
            "used": (used * 100.0).round().min(100.0),
            "remaining": ((1.0 - used) * 100.0).round().max(0.0),
            "resetsAt": rfc3339(resets_at),
        }));
    }
    if let Some(windows) = info.get("unifiedWindows").and_then(|w| w.as_object()) {
        for (kind, w) in windows {
            let used = w.get("utilization").and_then(|u| u.as_f64()).unwrap_or(0.0);
            let resets = w.get("resetsAt").and_then(|r| r.as_i64()).unwrap_or(0);
            push(kind, used, resets);
        }
    }
    let told = info.get("rateLimitType").and_then(|t| t.as_str()).unwrap_or("");
    if !told.is_empty() {
        let already = info
            .get("unifiedWindows")
            .and_then(|w| w.as_object())
            .is_some_and(|w| w.contains_key(told));
        if !already {
            match info.get("utilization").and_then(|u| u.as_f64()) {
                Some(used) => push(told, used, info.get("resetsAt").and_then(|r| r.as_i64()).unwrap_or(0)),
                None => {
                    if info.get("status").and_then(|s| s.as_str()) == Some("rejected") {
                        push(told, 1.0, info.get("resetsAt").and_then(|r| r.as_i64()).unwrap_or(0));
                    }
                }
            }
        }
    }
    out
}

fn rfc3339(secs: i64) -> Option<String> {
    if secs <= 0 {
        return None;
    }
    chrono::DateTime::from_timestamp(secs, 0).map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

/// 桥跑动时 Anthropic 告诉 Claude Code 的窗口（最新一次读数，30 分钟内新鲜）。
static LIMITS_CACHE: Mutex<Option<(Instant, Vec<Value>)>> = Mutex::new(None);

fn note_limits(windows: &[Value]) {
    if windows.is_empty() {
        return;
    }
    let mut g = LIMITS_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    *g = Some((Instant::now(), windows.to_vec()));
}

fn cached_limits() -> Option<Vec<Value>> {
    let g = LIMITS_CACHE.lock().unwrap_or_else(|p| p.into_inner());
    g.as_ref().filter(|(at, _)| at.elapsed() < LIMITS_FRESH).map(|(_, w)| w.clone())
}

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    *LIMITS_CACHE.lock().unwrap_or_else(|p| p.into_inner()) = None;
    *USAGE_STATE.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

/// /usage 兜底的状态：`(at, Option<文本>)`——Some(None) = 刚失败过（退避）。
static USAGE_STATE: Mutex<Option<(Instant, Option<String>)>> = Mutex::new(None);

/// 跑 `claude -p /usage`（Go claudeUsage claude_warmup.go:198：**额度也是 CLI
/// 子进程**，从不拿 token 打 HTTP）。返回 /usage 的文本。
fn run_usage_cli() -> Option<String> {
    let program = claude_binary()?;
    let args = usage_args();
    let mut cmd = build_command(&program, &args);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let child = Arc::new(Mutex::new(child));
    // 看门狗（/usage 上限 60s；正常路径读完 stdout 后由 done 提前退出）
    let done = Arc::new(AtomicBool::new(false));
    {
        let child = child.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + USAGE_TIMEOUT;
            while Instant::now() < deadline {
                if done.load(Ordering::Acquire) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            if !done.load(Ordering::Acquire) {
                if let Ok(mut c) = child.lock() {
                    let _ = c.kill();
                }
            }
        });
    }
    let mut buf = Vec::new();
    let mut reader = BufReader::new(stdout);
    let _ = reader.read_to_end(&mut buf);
    done.store(true, Ordering::Release);
    if let Ok(mut c) = child.lock() {
        let _ = c.kill();
        let _ = c.wait();
    }
    parse_usage_output(&buf)
}

/// Go claudeResult（claude_warmup.go:480）：verbose json 是行/数组混合——
/// 数组形态取最后一个 type=="result" 的 result 文本。
fn parse_usage_output(bytes: &[u8]) -> Option<String> {
    let trimmed: &[u8] = {
        let t = bytes;
        let start = t.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(0);
        &t[start..]
    };
    if trimmed.first() == Some(&b'[') {
        let all: Vec<Value> = serde_json::from_slice(trimmed).ok()?;
        for v in all.iter().rev() {
            if v.get("type").and_then(|t| t.as_str()) == Some("result") {
                if v.get("is_error").and_then(|e| e.as_bool()) == Some(true) {
                    return None;
                }
                return v.get("result").and_then(|r| r.as_str()).map(String::from);
            }
        }
        return None;
    }
    // 行分隔 JSONL 兜底
    for line in bytes.split(|b| *b == b'\n').rev() {
        if let Ok(v) = serde_json::from_slice::<Value>(line) {
            if v.get("type").and_then(|t| t.as_str()) == Some("result")
                && v.get("is_error").and_then(|e| e.as_bool()) != Some(true)
            {
                return v.get("result").and_then(|r| r.as_str()).map(String::from);
            }
        }
    }
    None
}

/// 月份表（/usage 的 "Oct 1" 风格）。
fn month_num(s: &str) -> Option<u32> {
    let m = s.trim().trim_end_matches(',').to_ascii_lowercase();
    Some(match m.as_str() {
        "jan" | "january" => 1,
        "feb" | "february" => 2,
        "mar" | "march" => 3,
        "apr" | "april" => 4,
        "may" => 5,
        "jun" | "june" => 6,
        "jul" | "july" => 7,
        "aug" | "august" => 8,
        "sep" | "sept" | "september" => 9,
        "oct" | "october" => 10,
        "nov" | "november" => 11,
        "dec" | "december" => 12,
        _ => return None,
    })
}

/// "3:30pm" / "3pm" / "12:05am" → (hour, minute)（12 小时制）。
fn parse_clock(s: &str) -> Option<(u32, u32)> {
    let s = s.trim().trim_end_matches(',');
    let lower = s.to_ascii_lowercase();
    let (body, pm) = if let Some(b) = lower.strip_suffix("pm") {
        (b, true)
    } else if let Some(b) = lower.strip_suffix("am") {
        (b, false)
    } else {
        return None;
    };
    let (h, m) = match body.split_once(':') {
        Some((h, m)) => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        None => (body.parse::<u32>().ok()?, 0),
    };
    if h > 12 || m > 59 {
        return None;
    }
    let hour = match (h, pm) {
        (12, false) => 0,
        (12, true) => 12,
        (h, true) => h + 12,
        (h, false) => h,
    };
    Some((hour, m))
}

/// /usage 的 reset 文本 → unix 秒。Go claudeResetTime（claude_usage_cli.go:288）
/// 的简化：丢弃时区名（chrono-tz 不在依赖里），按**本机时区**读；无年份的
/// 日期在 [now-24h, now+span+2h] 窗口内取年（Go claudeResetSlack 的窗口）。
fn parse_reset(s: &str, span_secs: i64, now: i64) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // 丢 "(Asia/Shanghai)" 时区段
    let s = match (s.find('('), s.ends_with(')')) {
        (Some(i), true) => s[..i].trim(),
        _ => s,
    };
    let mut month: Option<u32> = None;
    let mut day: Option<u32> = None;
    let mut year: Option<i32> = None;
    let mut clock: Option<(u32, u32)> = None;
    for token in s.split_whitespace() {
        let token = token.trim_matches(',');
        if token.eq_ignore_ascii_case("at") || token.is_empty() {
            continue;
        }
        if let Some(m) = month_num(token) {
            month = Some(m);
        } else if let Some(c) = parse_clock(token) {
            clock = Some(c);
        } else if let Ok(n) = token.parse::<u32>() {
            if n > 31 {
                year = Some(n as i32); // 4 位年份（"Jan 2, 2006 at 3:04pm"）
            } else if n >= 1 && day.is_none() {
                day = Some(n);
            }
        }
    }
    let (hour, minute) = clock?;
    use chrono::Datelike;
    let now_date = chrono::Local::now();
    let to_unix = |n: chrono::NaiveDateTime| -> Option<i64> {
        use chrono::TimeZone;
        chrono::Local.from_local_datetime(&n).single().map(|t| t.timestamp())
    };
    // 只有钟点（"3:30pm"）：今天的这个钟点，过了就明天（Go :327 的裸钟点
    // 布局；窗口内取，过去最近兜底）
    let Some(day) = day else {
        let base = chrono::NaiveDate::from_ymd_opt(now_date.year(), now_date.month(), now_date.day())?
            .and_hms_opt(hour, minute, 0)?;
        for offset in [0, 1, -1] {
            if let Some(t) = base.checked_add_signed(chrono::Duration::days(offset)).and_then(|n| to_unix(n)) {
                let low = now - 24 * 3600;
                let high = now + span_secs + 2 * 3600;
                if t > low && t <= high {
                    return Some(t);
                }
            }
        }
        return None;
    };
    let month = month?;
    let naive = |y: i32| chrono::NaiveDate::from_ymd_opt(y, month, day)?.and_hms_opt(hour, minute, 0);
    if let Some(y) = year {
        return to_unix(naive(y)?);
    }
    // 无年份：在窗口内挑年（今年 → 明年 → 去年）
    let y = now_date.year();
    let low = now - 24 * 3600;
    let high = now + span_secs + 2 * 3600;
    for candidate in [y, y + 1, y - 1] {
        if let Some(t) = naive(candidate).and_then(|n| to_unix(n)) {
            if t > low && t <= high {
                return Some(t);
            }
        }
    }
    // 都不在窗口：取过去最近的一次（Go past 分支——读作已重置）
    for candidate in [y, y - 1, y + 1] {
        if let Some(t) = naive(candidate).and_then(|n| to_unix(n)) {
            if t <= now {
                return Some(t);
            }
        }
    }
    None
}

/// /usage 文本 → 窗口表（Go parseClaudeUsage claude_usage_cli.go:174 的核心行）。
/// "Current session: 13% used · resets Oct 1 at 3:30pm (Asia/Shanghai)" /
/// "Current week (all models): 4% used" / "Current week (Fable): 0% used"。
fn parse_usage_windows(text: &str, now: i64) -> Vec<Value> {
    const WEEK: i64 = 7 * 24 * 3600;
    let mut out: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    let emit = |kind: &str, name: &str, span: i64, rest: &str, out: &mut Vec<Value>, seen: &mut Vec<String>| {
        if seen.iter().any(|s| s == name) {
            return;
        }
        if let Some((used, reset)) = used_and_reset(rest, span, now) {
            seen.push(name.to_string());
            out.push(json!({
                "kind": kind,
                "name": name,
                "used": used,
                "remaining": (100.0 - used).max(0.0),
                "resetsAt": reset,
            }));
        }
    };
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Current session:") {
            emit("five_hour", "5 hours", 5 * 3600, rest, &mut out, &mut seen);
            continue;
        }
        if let Some(r) = line.strip_prefix("Current week") {
            // "(all models)" 是整周；其它 scope 是模型周（Go :204-211）
            let (scope, rest) = match r.strip_prefix(" (") {
                Some(after) => match after.split_once(')') {
                    Some((scope, rest)) => (scope.trim().trim_end_matches(',').to_string(), rest),
                    None => (String::new(), r),
                },
                None => (String::new(), r),
            };
            let scope = scope.strip_suffix(" only").unwrap_or(&scope).trim().to_string();
            if scope.is_empty() || scope.eq_ignore_ascii_case("all models") {
                emit("seven_day", "7 days", WEEK, rest, &mut out, &mut seen);
            } else {
                emit("seven_day_scoped", &format!("7 days · {scope}"), WEEK, rest, &mut out, &mut seen);
            }
        }
    }
    out
}

/// " 13% used · resets Oct 1 at 3:30pm (TZ)" / "（scope 后）: 4% used" →
/// (used%, resetsAt RFC3339)。scope 行冒号后的百分比在这里剥。
fn used_and_reset(rest: &str, span: i64, now: i64) -> Option<(f64, Option<String>)> {
    let rest = rest.trim().trim_start_matches(':').trim();
    let pct = rest.split("% used").next()?.trim();
    let used: f64 = pct.parse().ok()?;
    let used = used.clamp(0.0, 100.0);
    let reset = rest
        .split_once("· resets ")
        .map(|(_, r)| parse_reset(r, span, now))
        .flatten()
        .and_then(rfc3339);
    Some((used, reset))
}

/// 订阅额度行（C3）：桥跑过的 rate_limit_event 读数优先，否则跑一次
/// `/usage` CLI（5 分钟退避）。没凭据 / 没 CLI / 用户自有 `claude` 来源 /
/// 什么都读不到 → None（展示面安静跳过）。
pub fn quota_row(providers: &[Provider]) -> Option<Value> {
    if providers.iter().any(|p| p.id == CLAUDE_ID) {
        return None;
    }
    let account = discover()?;
    claude_binary()?;
    let windows = match cached_limits() {
        Some(w) if !w.is_empty() => w,
        _ => {
            // /usage 兜底（带退避）：读缓存/退避在锁内，**跑 CLI 在锁外**
            // （一次最多 60s，别把后来者挂住）
            let cached = {
                let state = USAGE_STATE.lock().unwrap_or_else(|p| p.into_inner());
                match state.as_ref() {
                    Some((at, Some(t))) if at.elapsed() < USAGE_BACKOFF => Some(parse_usage_windows(t, now_secs())),
                    Some((at, None)) if at.elapsed() < USAGE_BACKOFF => Some(Vec::new()), // 刚失败过：退避
                    _ => None,
                }
            };
            if let Some(windows) = cached {
                return (!windows.is_empty()).then(|| row_of(windows, &account));
            }
            let text = run_usage_cli();
            let windows = text.as_deref().map(|t| parse_usage_windows(t, now_secs())).unwrap_or_default();
            *USAGE_STATE.lock().unwrap_or_else(|p| p.into_inner()) = Some((Instant::now(), text));
            if windows.is_empty() {
                return None;
            }
            windows
        }
    };
    Some(row_of(windows, &account))
}

fn row_of(windows: Vec<Value>, account: &Account) -> Value {
    let mut row = json!({
        "provider": CLAUDE_ID,
        "name": QUOTA_NAME,
        "kind": "subscription",
        "windows": windows,
    });
    if !account.plan.is_empty() {
        row["plan"] = json!(account.plan);
    }
    row
}

// =====================================================================
// intercept（与 grok_account / codex_account 同形）
// =====================================================================

/// 路由层钩子：请求的模型若只能由合成 `claude` 来源接住，且本机有登录 +
/// CLI，就在这里走桥；否则 None（走原有路径）。**只接 anthropic 入站**
/// （差距清单第 12 条）。
pub fn intercept(gateway: &super::Gateway, request: &Request, proto: Proto) -> Option<Reply> {
    if proto != Proto::Anthropic {
        return None;
    }
    let parsed: Value = serde_json::from_slice(&request.body).ok()?;
    let asked = parsed.get("model")?.as_str()?.to_string();
    let (groups, providers) = {
        let store = gateway.store.lock().ok()?;
        (store.groups().to_vec(), store.providers().to_vec())
    };
    let known = |id: &str| gateway.catalog.knows(&providers, id);
    // 真实来源（含用户自建的 claude API 来源）能接住的，一律走原路径。
    if super::groups::resolve_with(&groups, &providers, &asked, &known).is_some() {
        return None;
    }
    if providers.iter().any(|p| p.id == CLAUDE_ID) {
        return None;
    }
    let synthetic = synthetic_provider()?;
    let mut with = providers.clone();
    with.push(synthetic);
    let known2 = |id: &str| knows(id) || gateway.catalog.knows(&with, id);
    let (provider, bare, levels) = super::groups::resolve_with(&groups, &with, &asked, &known2)?;
    if provider != CLAUDE_ID {
        return None;
    }
    // inactive：没凭据或没 CLI → 让位（模型也不进目录，这里只是兜底）。
    // 凭据只判存在性——token 本身用不上（见 Account 注释）
    discover()?;
    let program = claude_binary()?;

    // 模型映射：`claude/<id>` → `--model <id>` 直传（Go :7262 OwnModelID）
    let model = bare.trim();
    if model.is_empty() {
        return Some(anthropic_error(400, "claude bridge: empty model name"));
    }
    // 档位：客户端自己要的（output_config.effort / thinking budget）优先，
    // 成员强度后缀（`claude/claude-opus-5:high`）次之（Go claudeCLIArgs 的
    // --effort；多档成员取近似是差距，单档直用）
    let mut effort = super::effort::requested(Proto::Anthropic, &parsed)
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if effort.is_empty() && levels.len() == 1 {
        effort = levels[0].to_ascii_lowercase();
    }
    let effort = match effort.as_str() {
        "low" | "medium" | "high" | "xhigh" | "max" => Some(effort),
        _ => None,
    };
    let prompt = render_prompt(&parsed);
    let wants_stream = parsed.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    let stream = spawn_bridge(&program, model, effort.as_deref(), &prompt, &[]).map_err(|(status, msg)| anthropic_error(status, &msg));
    let stream = match stream {
        Ok(s) => s,
        Err(reply) => return Some(reply),
    };
    if wants_stream {
        Some(Reply::Stream {
            status: 200,
            content_type: "text/event-stream".into(),
            reader: Box::new(stream),
        })
    } else {
        match stream.aggregate(model) {
            Ok(v) => Some(Reply::json(200, &v)),
            Err((status, msg)) => Some(anthropic_error(status, &msg)),
        }
    }
}

// =====================================================================
// 测试
// =====================================================================

#[cfg(test)]
mod tests {
    use super::super::Gateway;
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DIR_SEQ: AtomicUsize = AtomicUsize::new(0);

    struct World {
        dir: PathBuf,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl Drop for World {
        fn drop(&mut self) {
            *TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()) = None;
            reset_for_tests();
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// 沙箱世界：临时目录 + TEST_SEAM + XDG 重定向（grok/codex 测试同款，
    /// 全部取 TEST_SANDBOX_LOCK 串行）。
    fn world(seam_cli: Option<Program>, with_logins: bool) -> (Gateway, World) {
        let lock = crate::engine_gateway::TEST_SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_for_tests();
        let dir = std::env::temp_dir().join(format!(
            "marionette-engine-claude-{}-{}",
            std::process::id(),
            DIR_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(dir.join("magpie")).unwrap();
        std::fs::create_dir_all(dir.join("home")).unwrap();
        std::fs::create_dir_all(dir.join("logins")).unwrap();
        std::fs::write(dir.join("magpie").join("providers.json"), r#"{"providers":[]}"#).unwrap();
        *TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()) = Some(Seam {
            home: dir.join("home"),
            logins: if with_logins { Some(dir.join("logins")) } else { None },
            cli: seam_cli,
        });
        let prior = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let gateway = Gateway::open().expect("gateway");
        match &prior {
            Some(p) => std::env::set_var("XDG_CONFIG_HOME", p),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        (gateway, World { dir, _lock: lock })
    }

    /// 假 claude CLI（node 脚本咳 stream-json 行；模式走
    /// MARIONETTE_FAKE_CLAUDE_MODE 环境变量：chat（默认）/ error / noresult /
    /// usage）。dump 文件记录 argv/stdin/env 供断言。
    fn fake_cli(world_dir: &std::path::Path) -> Program {
        let node = find_node().expect("node on PATH (fake claude CLI needs it)");
        let script = world_dir.join("fake-claude.js");
        std::fs::write(&script, FAKE_CLI_JS).unwrap();
        Program { program: node, prefix: vec![script.to_string_lossy().into_owned()] }
    }

    fn find_node() -> Option<String> {
        let mut cmd = Command::new("where.exe");
        cmd.arg("node");
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let out = cmd.output().ok()?;
        if !out.status.success() {
            return None;
        }
        String::from_utf8_lossy(&out.stdout).lines().next().map(|l| l.trim().to_string()).filter(|l| !l.is_empty())
    }

    const FAKE_CLI_JS: &str = r#"
const fs = require("fs");
const mode = process.env.MARIONETTE_FAKE_CLAUDE_MODE || "chat";
const dump = process.env.MARIONETTE_FAKE_CLAUDE_DUMP;
const argv = process.argv.slice(2);
let stdinText = "";
process.stdin.setEncoding("utf8");
process.stdin.on("data", (d) => (stdinText += d));
process.stdin.on("end", () => {
  try {
    if (dump) fs.writeFileSync(dump, JSON.stringify({
      argv,
      stdin: stdinText,
      baseUrl: process.env.ANTHROPIC_BASE_URL || null,
      cwd: process.cwd(),
    }));
  } catch {}
  process.exit(0);
});
process.stdin.resume();
const w = (o) => process.stdout.write(JSON.stringify(o) + "\n");
if (mode === "usage") {
  process.stdout.write(JSON.stringify([
    { type: "system", subtype: "init", session_id: "s-usage" },
    { type: "result", subtype: "success", is_error: false,
      result: "Current session: 13% used · resets " + (process.env.MARIONETTE_FAKE_RESET || "Oct 1 at 3:30pm (Asia/Shanghai)") + "\nCurrent week (all models): 4% used\n" }
  ]) + "\n");
} else if (mode === "error") {
  w({ type: "result", subtype: "error_overloaded", is_error: true, api_error_status: 429, result: "rate limited: too many requests" });
} else if (mode === "noresult") {
  const model = argv[argv.indexOf("--model") + 1] || "claude-sonnet-5";
  w({ type: "stream_event", event: { type: "message_start", message: { id: "msg_cut", type: "message", role: "assistant", model, content: [], usage: { input_tokens: 5, output_tokens: 1 } } } });
  w({ type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } } });
  w({ type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "partial" } } });
  w({ type: "stream_event", event: { type: "message_stop" } });
  setTimeout(() => process.exit(0), 300);
} else if (mode === "whole") {
  w({ type: "assistant", message: { id: "msg_whole", type: "message", role: "assistant", model: "claude-opus-5",
    content: [{ type: "text", text: "whole message reply" }], stop_reason: "end_turn",
    usage: { input_tokens: 7, output_tokens: 3 } } });
  w({ type: "result", subtype: "success", is_error: false, result: "done" });
} else {
  const model = argv[argv.indexOf("--model") + 1] || "claude-sonnet-5";
  const effort = argv.indexOf("--effort") >= 0 ? argv[argv.indexOf("--effort") + 1] : null;
  w({ type: "system", subtype: "init", session_id: "sess-fake" });
  w({ type: "stream_event", event: { type: "message_start", message: { id: "msg_f1", type: "message", role: "assistant", model, content: [], usage: { input_tokens: 10, output_tokens: 1 } } } });
  if (effort) w({ type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "thinking", thinking: "" } } });
  const textIndex = effort ? 1 : 0;
  if (effort) w({ type: "stream_event", event: { type: "content_block_stop", index: 0 } });
  w({ type: "stream_event", event: { type: "content_block_start", index: textIndex, content_block: { type: "text", text: "" } } });
  w({ type: "stream_event", event: { type: "content_block_delta", index: textIndex, delta: { type: "text_delta", text: "你好，世界" } } });
  w({ type: "stream_event", event: { type: "content_block_stop", index: textIndex } });
  w({ type: "stream_event", event: { type: "message_delta", delta: { stop_reason: "end_turn", stop_sequence: null }, usage: { input_tokens: 10, output_tokens: 2 } } });
  w({ type: "stream_event", event: { type: "message_stop" } });
  w({ type: "rate_limit_event", rate_limit_info: { status: "ok", rateLimitType: "five_hour",
    unifiedWindows: { five_hour: { utilization: 0.42, resetsAt: 4102444800 }, seven_day: { utilization: 0.07, resetsAt: 4102963200 } } } });
  w({ type: "result", subtype: "success", is_error: false, result: "done" });
}
"#;

    fn write_credentials(world: &World, plan: &str) {
        let creds = json!({
            "claudeAiOauth": {
                "accessToken": "test-access-token",
                "refreshToken": "test-refresh-token",
                "expiresAt": (now_secs() + 3600) * 1000,
                "scopes": ["user:profile"],
                "subscriptionType": plan,
            }
        });
        std::fs::write(world.dir.join("home").join(".credentials.json"), serde_json::to_vec(&creds).unwrap()).unwrap();
    }

    fn write_login(world: &World, plan: &str) {
        let logins = json!([{
            "id": "login-1", "agent": "claude", "user": "a@b.c", "plan": plan,
            "auth": { "claudeAiOauth": {
                "accessToken": "test-login-token", "refreshToken": "test-login-refresh",
                "expiresAt": (now_secs() + 7200) * 1000, "subscriptionType": plan } }
        }]);
        std::fs::write(world.dir.join("logins").join("logins.json"), serde_json::to_vec(&logins).unwrap()).unwrap();
    }

    fn messages_body(model: &str, stream: bool) -> Value {
        json!({
            "model": model,
            "max_tokens": 1024,
            "stream": stream,
            "system": "You are a helpful assistant.",
            "messages": [{ "role": "user", "content": "你好" }],
        })
    }

    fn post(body: &Value) -> Request {
        Request {
            method: "POST".into(),
            path: "/v1/messages".into(),
            headers: Vec::new(),
            body: serde_json::to_vec(body).unwrap(),
        }
    }

    fn read_stream(reply: Reply) -> String {
        match reply {
            Reply::Stream { status, mut reader, .. } => {
                assert_eq!(status, 200);
                let mut buf = String::new();
                reader.read_to_string(&mut buf).unwrap();
                buf
            }
            Reply::Json { status, body } => panic!("expected stream, got json {status}: {}", String::from_utf8_lossy(&body)),
        }
    }

    fn sse_events(sse: &str) -> Vec<Value> {
        sse.split("\n\n")
            .filter(|b| !b.trim().is_empty())
            .filter_map(|b| b.lines().find(|l| l.starts_with("data: ")).map(|l| &l[6..]))
            .filter_map(|d| serde_json::from_str(d).ok())
            .collect()
    }

    fn dump_of(world: &World) -> Value {
        let b = std::fs::read(world.dir.join("dump.json")).unwrap();
        serde_json::from_slice(&b).unwrap()
    }

    // ---------------------------------------------------------------
    // 凭据发现
    // ---------------------------------------------------------------

    #[test]
    fn no_credentials_means_inactive() {
        let (gateway, _w) = world(None, false);
        assert!(discover().is_none());
        assert!(synthetic_provider().is_none());
        assert!(listed_rows(&[]).is_empty());
        assert!(quota_row(&[]).is_none());
        // intercept 返回 None（走原路径 → routes 404）
        let reply = super::super::routes::route(&post(&messages_body("claude/claude-sonnet-5", false)), &gateway);
        match reply {
            Reply::Json { status, .. } => assert_eq!(status, 404),
            _ => panic!("expected 404"),
        }
    }

    #[test]
    fn cli_missing_means_inactive_even_with_credentials() {
        let (_gateway, w) = world(None, false);
        write_credentials(&w, "max");
        assert!(discover().is_some()); // 凭据在
        assert!(!available()); // 但 CLI 不在 → 整体 inactive
        assert!(listed_rows(&[]).is_empty());
    }

    #[test]
    fn credential_file_is_read_only() {
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let before = std::fs::read(w.dir.join("home").join(".credentials.json")).unwrap();
        drop(discover());
        let reply = super::super::routes::route(&post(&messages_body("claude/claude-sonnet-5", false)), &gateway);
        match reply {
            Reply::Json { status, .. } => assert_eq!(status, 404), // 没 CLI 仍 404
            _ => panic!("expected 404"),
        }
        let after = std::fs::read(w.dir.join("home").join(".credentials.json")).unwrap();
        assert_eq!(before, after, "credential file must never be written");
    }

    #[test]
    fn logins_json_is_the_fallback_credential() {
        let (_gateway, w) = world(None, true);
        write_login(&w, "pro");
        let account = discover().expect("login discovered");
        assert_eq!(account.plan, "pro");
        assert!(account.exp.is_some());
    }

    #[test]
    fn signed_out_credentials_do_not_count() {
        let (_gateway, w) = world(None, false);
        let creds = json!({ "claudeAiOauth": { "accessToken": "", "refreshToken": "" } });
        std::fs::write(w.dir.join("home").join(".credentials.json"), serde_json::to_vec(&creds).unwrap()).unwrap();
        assert!(discover().is_none());
    }

    // ---------------------------------------------------------------
    // 桥往返
    // ---------------------------------------------------------------

    #[test]
    fn bridge_roundtrip_streams_anthropic_frames() {
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        std::env::set_var("MARIONETTE_FAKE_CLAUDE_DUMP", w.dir.join("dump.json"));
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        let reply = intercept(&gateway, &post(&messages_body("claude/claude-sonnet-5", true)), Proto::Anthropic).expect("intercepted");
        let sse = read_stream(reply);
        let events = sse_events(&sse);
        let types: Vec<&str> = events.iter().map(|e| e.get("type").and_then(|t| t.as_str()).unwrap_or("")).collect();
        // 帧族断言：message_start 第一帧 → 块开/delta/停 → message_delta(带
        // stop_reason+usage) → message_stop（anthropic SDK 的硬顺序）
        assert_eq!(types.first(), Some(&"message_start"));
        assert_eq!(types.last(), Some(&"message_stop"));
        assert!(types.contains(&"content_block_delta"));
        let delta = events.iter().find(|e| e.get("type").and_then(|t| t.as_str()) == Some("content_block_delta")).unwrap();
        assert_eq!(delta.pointer("/delta/text").and_then(|t| t.as_str()), Some("你好，世界"));
        let md = events.iter().find(|e| e.get("type").and_then(|t| t.as_str()) == Some("message_delta")).unwrap();
        assert_eq!(md.pointer("/delta/stop_reason").and_then(|s| s.as_str()), Some("end_turn"));
        assert_eq!(md.pointer("/usage/output_tokens").and_then(|u| u.as_i64()), Some(2));
        let start = events.first().unwrap();
        assert_eq!(start.pointer("/message/model").and_then(|m| m.as_str()), Some("claude-sonnet-5"));
        // 假 CLI 记录的 argv：--model 直传 + 桥参数族（Go claudeCLIArgs 同款）
        let dump = dump_of(&w);
        let argv: Vec<String> = dump.get("argv").unwrap().as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        let joined = argv.join(" ");
        for flag in ["-p", "--output-format stream-json", "--input-format stream-json", "--include-partial-messages", "--verbose", "--model claude-sonnet-5", "--tools ", "--strict-mcp-config", "--dangerously-skip-permissions", "--no-session-persistence"] {
            assert!(joined.contains(flag), "argv missing `{flag}`: {joined}");
        }
        assert!(!joined.contains("--mcp-config"), "no MCP helper in the core path");
        // stdin：一行 user 消息，content 是渲染后的 text 块（system 一块 +
        // 对话一块，Human: 标注）
        let stdin = dump.get("stdin").unwrap().as_str().unwrap();
        let line: Value = serde_json::from_str(stdin.trim_end()).unwrap();
        assert_eq!(line.pointer("/type").and_then(|t| t.as_str()), Some("user"));
        assert_eq!(line.pointer("/message/role").and_then(|r| r.as_str()), Some("user"));
        let sys_block = line.pointer("/message/content/0/text").and_then(|t| t.as_str()).unwrap();
        assert!(sys_block.contains("<external_system_instructions>"), "system wrapped: {sys_block}");
        assert!(sys_block.contains("You are a helpful assistant."));
        let convo_block = line.pointer("/message/content/1/text").and_then(|t| t.as_str()).unwrap();
        assert!(convo_block.contains("Human: 你好"), "conversation labeled: {convo_block}");
        // ANTHROPIC_BASE_URL 必须被摘掉（否则桥会打回引擎自己）
        assert_eq!(dump.get("baseUrl"), Some(&Value::Null));
        std::env::remove_var("MARIONETTE_FAKE_CLAUDE_DUMP");
    }

    #[test]
    fn bridge_aggregates_non_stream_reply() {
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        let reply = intercept(&gateway, &post(&messages_body("claude/claude-opus-5-5", false)), Proto::Anthropic).expect("intercepted");
        match reply {
            Reply::Json { status, body } => {
                assert_eq!(status, 200);
                let v: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(v.get("type").and_then(|t| t.as_str()), Some("message"));
                assert_eq!(v.get("role").and_then(|r| r.as_str()), Some("assistant"));
                assert_eq!(v.get("model").and_then(|m| m.as_str()), Some("claude-opus-5-5"));
                assert_eq!(v.pointer("/content/0/text").and_then(|t| t.as_str()), Some("你好，世界"));
                assert_eq!(v.get("stop_reason").and_then(|s| s.as_str()), Some("end_turn"));
                assert_eq!(v.pointer("/usage/output_tokens").and_then(|u| u.as_i64()), Some(2));
            }
            _ => panic!("expected json"),
        }
    }

    #[test]
    fn bridge_error_result_maps_to_error_reply() {
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        std::env::set_var("MARIONETTE_FAKE_CLAUDE_MODE", "error");
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        let reply = intercept(&gateway, &post(&messages_body("claude/claude-sonnet-5", false)), Proto::Anthropic).expect("intercepted");
        match reply {
            Reply::Json { status, body } => {
                assert_eq!(status, 429);
                let v: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(v.pointer("/error/type").and_then(|t| t.as_str()), Some("rate_limit_error"));
                assert!(v.pointer("/error/message").unwrap().as_str().unwrap().contains("rate limited"));
            }
            _ => panic!("expected json"),
        }
        std::env::remove_var("MARIONETTE_FAKE_CLAUDE_MODE");
    }

    #[test]
    fn stream_error_result_emits_error_frame() {
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        std::env::set_var("MARIONETTE_FAKE_CLAUDE_MODE", "error");
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        let reply = intercept(&gateway, &post(&messages_body("claude/claude-sonnet-5", true)), Proto::Anthropic).expect("intercepted");
        let sse = read_stream(reply);
        let events = sse_events(&sse);
        let err = events.iter().find(|e| e.get("type").and_then(|t| t.as_str()) == Some("error")).expect("error frame");
        assert_eq!(err.pointer("/error/type").and_then(|t| t.as_str()), Some("rate_limit_error"));
        std::env::remove_var("MARIONETTE_FAKE_CLAUDE_MODE");
    }

    #[test]
    fn cli_exit_without_result_after_reply_is_a_clean_end() {
        // noresult：message_stop 到了但 result 没来、进程自己退了 → 流正常
        // 收口（Go readOutput 的 stopped 路径），不报错也不挂死
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        std::env::set_var("MARIONETTE_FAKE_CLAUDE_MODE", "noresult");
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        let reply = intercept(&gateway, &post(&messages_body("claude/claude-sonnet-5", true)), Proto::Anthropic).expect("intercepted");
        let sse = read_stream(reply);
        let events = sse_events(&sse);
        let types: Vec<&str> = events.iter().map(|e| e.get("type").and_then(|t| t.as_str()).unwrap_or("")).collect();
        assert_eq!(types.last(), Some(&"message_stop"));
        assert!(!types.contains(&"error"), "clean end, no error: {types:?}");
        std::env::remove_var("MARIONETTE_FAKE_CLAUDE_MODE");
    }

    #[test]
    fn whole_assistant_message_without_stream_events_is_synthesized() {
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        std::env::set_var("MARIONETTE_FAKE_CLAUDE_MODE", "whole");
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        let reply = intercept(&gateway, &post(&messages_body("claude/claude-sonnet-5", false)), Proto::Anthropic).expect("intercepted");
        match reply {
            Reply::Json { status: _, body } => {
                let v: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(v.pointer("/content/0/text").and_then(|t| t.as_str()), Some("whole message reply"));
                assert_eq!(v.get("id").and_then(|i| i.as_str()), Some("msg_whole"));
            }
            Reply::Stream { status, .. } => panic!("expected json, got stream {status}"),
        }
        std::env::remove_var("MARIONETTE_FAKE_CLAUDE_MODE");
    }

    #[test]
    fn requested_effort_maps_to_cli_flag() {
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        std::env::set_var("MARIONETTE_FAKE_CLAUDE_DUMP", w.dir.join("dump-effort.json"));
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        let mut body = messages_body("claude/claude-opus-5", false);
        body["thinking"] = json!({ "type": "enabled", "budget_tokens": 24000 });
        let reply = intercept(&gateway, &post(&body), Proto::Anthropic).expect("intercepted");
        assert!(matches!(reply, Reply::Json { status: 200, .. }));
        let dump: Value = serde_json::from_slice(&std::fs::read(w.dir.join("dump-effort.json")).unwrap()).unwrap();
        let argv: Vec<String> = dump.get("argv").unwrap().as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        let joined = argv.join(" ");
        assert!(joined.contains("--effort high"), "effort flag: {joined}");
        assert!(joined.contains("--thinking-display summarized"), "thinking display: {joined}");
        std::env::remove_var("MARIONETTE_FAKE_CLAUDE_DUMP");
    }

    // ---------------------------------------------------------------
    // 渲染
    // ---------------------------------------------------------------

    #[test]
    fn multi_turn_history_is_wrapped_and_tool_calls_rendered() {
        // historyEnd（Go :5578）：最后一条「无工具调用的 assistant 回复」之后
        // 才是当前回合——纯文本回复（msgs[1]）封历史，工具循环属当前回合
        let body = json!({
            "model": "claude/claude-sonnet-5",
            "system": "sys",
            "messages": [
                { "role": "user", "content": "先看这个文件" },
                { "role": "assistant", "content": [{ "type": "text", "text": "我看过了" }] },
                { "role": "user", "content": "再查一下配置" },
                { "role": "assistant", "content": [
                    { "type": "text", "text": "我来看" },
                    { "type": "tool_use", "id": "toolu_1", "name": "read_file", "input": { "path": "a.txt" } }
                ]},
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_1", "content": "文件内容 ABC" },
                    { "type": "text", "text": "现在总结一下" }
                ]}
            ]
        });
        let blocks = render_prompt(&body);
        assert_eq!(blocks.len(), 2); // system + 对话
        let text = blocks[1].get("text").and_then(|t| t.as_str()).unwrap();
        assert!(text.starts_with("<conversation_history>"), "history wrapper: {text}");
        assert!(text.contains("</conversation_history>\n\n<current_turn>"));
        assert!(text.contains("Human: 先看这个文件"));
        assert!(text.contains("Assistant: 我看过了"));
        assert!(text.contains("[tool call read_file id=toolu_1 args={\"path\":\"a.txt\"}]"));
        assert!(text.contains("[tool result id=toolu_1]\n文件内容 ABC"));
        // tool_result 之后的 text 是用户的话（Go :5750）
        assert!(text.contains("[end of tool result id=toolu_1]\n\n[user message]\n现在总结一下"));
        // 对话里有调用 → pastCallsNote（Go :5286 对整段消息判，不只在历史里）
        assert!(text.contains("record calls made earlier"));
        assert!(text.contains("</current_turn>"));
    }

    #[test]
    fn tool_loop_without_text_reply_has_no_history_wrapper() {
        // 没有任何「无工具调用的 assistant 回复」→ historyEnd=0，整段都是
        // 当前回合（Go :5578 的空表兜底；子代理式工具循环正是这个形状）
        let body = json!({
            "model": "m",
            "messages": [
                { "role": "user", "content": "查" },
                { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "t1", "name": "search", "input": {"q": "x"} } ]},
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "t1", "content": "结果" } ]}
            ]
        });
        let blocks = render_prompt(&body);
        let text = blocks[0].get("text").and_then(|t| t.as_str()).unwrap();
        assert!(!text.contains("<conversation_history>"));
        assert!(!text.contains("<current_turn>"));
        assert!(text.contains("[tool result id=t1]\n结果"));
    }

    #[test]
    fn single_turn_is_not_wrapped() {
        let body = json!({ "model": "m", "messages": [ { "role": "user", "content": "hi" } ] });
        let blocks = render_prompt(&body);
        let text = blocks[0].get("text").and_then(|t| t.as_str()).unwrap();
        assert!(text.contains("Human: hi"));
        assert!(!text.contains("<conversation_history>"));
    }

    #[test]
    fn thinking_blocks_are_never_told_and_images_get_a_note() {
        let body = json!({
            "model": "m",
            "messages": [
                { "role": "user", "content": "q" },
                { "role": "assistant", "content": [
                    { "type": "thinking", "thinking": "secret reasoning", "signature": "sig==" },
                    { "type": "text", "text": "answer" }
                ]},
                { "role": "user", "content": [
                    { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "AAAA" } },
                    { "type": "text", "text": "这是什么" }
                ]}
            ]
        });
        let blocks = render_prompt(&body);
        let text = blocks[0].get("text").and_then(|t| t.as_str()).unwrap();
        assert!(!text.contains("secret reasoning"), "thinking is never told as words");
        assert!(!text.contains("sig=="));
        assert!(text.contains("Assistant: answer"));
        // 图片占位直接落在正文流里（真图走 base64 块是差距清单第 5 条）
        assert!(text.contains("Human: [image omitted]这是什么"));
    }

    // ---------------------------------------------------------------
    // 路由让位
    // ---------------------------------------------------------------

    #[test]
    fn user_defined_claude_provider_yields() {
        // 用户自有 claude 来源（API key 直连）→ 本模块让位
        let lock = crate::engine_gateway::TEST_SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        reset_for_tests();
        let dir = std::env::temp_dir().join(format!("marionette-engine-claude-user-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("magpie")).unwrap();
        std::fs::create_dir_all(dir.join("home")).unwrap();
        std::fs::write(
            dir.join("magpie").join("providers.json"),
            r#"{"providers":[{"id":"claude","name":"My Claude","anthropic":"https://example.com"}]}"#,
        )
        .unwrap();
        *TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()) = Some(Seam { home: dir.join("home"), logins: None, cli: None });
        let prior = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let gateway = Gateway::open().unwrap();
        match &prior {
            Some(p) => std::env::set_var("XDG_CONFIG_HOME", p),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let providers = vec![Provider { id: "claude".into(), name: "My Claude".into(), anthropic: "https://example.com".into(), ..Default::default() }];
        assert!(listed_rows(&providers).is_empty());
        assert!(quota_row(&providers).is_none());
        assert!(intercept(&gateway, &post(&messages_body("claude/claude-sonnet-5", false)), Proto::Anthropic).is_none());
        drop(lock);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_anthropic_protos_are_not_intercepted() {
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        let request = Request { method: "POST".into(), path: "/v1/chat/completions".into(), headers: Vec::new(), body: serde_json::to_vec(&json!({"model":"claude/claude-sonnet-5","messages":[]})).unwrap() };
        assert!(intercept(&gateway, &request, Proto::Chat).is_none());
        assert!(intercept(&gateway, &request, Proto::Responses).is_none());
    }

    // ---------------------------------------------------------------
    // 额度
    // ---------------------------------------------------------------

    #[test]
    fn rate_limit_event_feeds_quota_row() {
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        let reply = intercept(&gateway, &post(&messages_body("claude/claude-sonnet-5", false)), Proto::Anthropic).expect("intercepted");
        assert!(matches!(reply, Reply::Json { status: 200, .. }));
        let row = quota_row(&[]).expect("quota row from rate_limit_event");
        assert_eq!(row.get("provider").and_then(|p| p.as_str()), Some("claude"));
        let windows = row.get("windows").unwrap().as_array().unwrap();
        let five = windows.iter().find(|x| x.get("name").and_then(|n| n.as_str()) == Some("5 hours")).expect("5h window");
        assert_eq!(five.get("used").and_then(|u| u.as_f64()), Some(42.0));
        assert_eq!(five.get("remaining").and_then(|u| u.as_f64()), Some(58.0));
        assert!(five.get("resetsAt").is_some());
        let week = windows.iter().find(|x| x.get("name").and_then(|n| n.as_str()) == Some("7 days")).expect("7d window");
        assert_eq!(week.get("used").and_then(|u| u.as_f64()), Some(7.0));
        assert_eq!(row.get("plan").and_then(|p| p.as_str()), Some("max"));
    }

    #[test]
    fn usage_cli_is_the_quota_fallback() {
        let (gateway, w) = world(None, false);
        write_credentials(&w, "pro");
        let cli = fake_cli(&w.dir);
        std::env::set_var("MARIONETTE_FAKE_CLAUDE_MODE", "usage");
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        let _ = &gateway;
        let row = quota_row(&[]).expect("quota row from /usage");
        let windows = row.get("windows").unwrap().as_array().unwrap();
        assert_eq!(windows.len(), 2);
        let five = windows.iter().find(|x| x.get("name").and_then(|n| n.as_str()) == Some("5 hours")).expect("5h");
        assert_eq!(five.get("used").and_then(|u| u.as_f64()), Some(13.0));
        assert!(five.get("resetsAt").is_some(), "reset parsed");
        let week = windows.iter().find(|x| x.get("name").and_then(|n| n.as_str()) == Some("7 days")).expect("7d");
        assert_eq!(week.get("used").and_then(|u| u.as_f64()), Some(4.0));
        assert_eq!(row.get("plan").and_then(|p| p.as_str()), Some("pro"));
        std::env::remove_var("MARIONETTE_FAKE_CLAUDE_MODE");
    }

    #[test]
    fn usage_window_parsing_units() {
        // 带时区名 / 带作用域周 / 无 reset 三种行
        let now = now_secs();
        let windows = parse_usage_windows(
            "Current session: 13% used · resets Oct 1 at 3:30pm (Asia/Shanghai)\nCurrent week (all models): 4% used · resets Oct 3 at 2pm (UTC)\nCurrent week (Fable): 0% used\nLast 7d · 403 requests · 388 sessions",
            now,
        );
        assert_eq!(windows.len(), 3);
        assert!(windows.iter().any(|w| w.get("name").and_then(|n| n.as_str()) == Some("5 hours")));
        assert!(windows.iter().any(|w| w.get("name").and_then(|n| n.as_str()) == Some("7 days")));
        assert!(windows.iter().any(|w| w.get("name").and_then(|n| n.as_str()) == Some("7 days · Fable")));
        for w in &windows {
            let used = w.get("used").and_then(|u| u.as_f64()).unwrap();
            assert!((0.0..=100.0).contains(&used));
        }
    }

    #[test]
    fn reset_parser_handles_12h_clock_and_year_inference() {
        let now = now_secs();
        // 时间只给钟点：取下一个不超窗的（5h 窗）
        let t = parse_reset("3:30pm", 5 * 3600, now).expect("clock-only reset");
        let dt = chrono::DateTime::from_timestamp(t, 0).unwrap();
        let within = t > now - 24 * 3600 && t <= now + 5 * 3600 + 2 * 3600;
        assert!(within, "clock-only within window: {dt}");
        // 月日 + at 钟点
        let t2 = parse_reset("Oct 1 at 3:30pm (Asia/Shanghai)", 7 * 24 * 3600, now);
        assert!(t2.is_some());
        // 12am → 0 点（按本机时区显示——reset 本来就是本地钟点）
        let t3 = parse_reset("12:05am", 5 * 3600, now).expect("12am");
        use chrono::TimeZone;
        let dt3 = chrono::Local.timestamp_opt(t3, 0).unwrap();
        assert_eq!(dt3.format("%H:%M").to_string(), "00:05");
        // 空串
        assert!(parse_reset("", 3600, now).is_none());
    }
}
