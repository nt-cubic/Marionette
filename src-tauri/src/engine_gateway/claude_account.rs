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
//!   --input-format stream-json --verbose …`（Go `claudeCLIArgs` :3957，
//!   caller 带 tools 时加 `--mcp-config`，见下文 MCP helper 节）→ stdin 一行
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
//! ## 跨回合驻留（2026-11-18，差距清单第 2 条交付）
//!
//! Go `keepIdle`（:2623）/ `resume`（:2033）/ `nextTurn`（:2171）/
//! `turnKey`（:3790）/ `hashMessages`（:3870，`messageParts` 在
//! claude_lost_media.go）的移植。回合正常收口（`result` 行）后**进程不杀**：
//! 按会话键进 idle 池等下一回合；同会话的下一回合（客户端把上一条回复
//! 原样复述在 messages 里）直接命中，**只把增量消息写给驻留进程的
//! stdin**（Go renderClaudeTurn :5700），不再每回合重写整段对话缓存。
//!
//! - **会话键**：`turn_key` = sha256(owner·model·effort·tool_choice·
//!   system·tools·消息词哈希)。消息按 Go `messageParts` 的口径归一——
//!   文本取去空白词、tool_use 按「名字+第 N 个调用」（不是 id）、
//!   tool_result 按「应答的第 N 个调用」、thinking 跳过、**相邻同角色
//!   消息合并**——所以客户端怎么切 text 块 / 怎么复述都能对上驻留键。
//! - **idle 淘汰**：`IDLE_LONGEST` = **1 小时**（Go `idleLongest =
//!   claudeCacheTTL = time.Hour`，:151-152——任务简报里写的「2h」与
//!   Go 源不符，按源实现）+ `IDLE_MOST` = 6（:153，最久未用的先让位）。
//!   Go 用每 run 一个 timer；这里**惰性清扫**（每次碰池先扫过期项），
//!   无请求时多驻留到引擎空闲回收，语义等价、少一个线程。
//! - **one-off 询问不驻留**（Go `ended` :2567：无工具且对话里没有任何
//!   assistant 回复——标题生成、路由分类器这类一问一答）回合结束即杀。
//! - **降级**：键不中 / 档位变了（Go 用 control_request 热改 effort
//!   `setEffort`，我们不会改——档位不一致就不复用）/ 进程已死 /
//!   stdin 写失败 → 全部落回「新起进程 + 全量 prompt」的旧路径。
//!
//! ## 调用方工具 MCP helper（2026-11-18，差距清单第 3 条交付）
//!
//! Go `bridgeTools`（:8137）/ `mcpCall`（:6479）/ `continueWith`
//! （:6354）/ claudebridge/mcp.go（隐藏 stdio helper）的移植。caller
//! 请求里带 tools 时：`--mcp-config` 挂一个 MCP 服务，把 caller 的工具
//! **原样宣告**给 Claude Code（名字经 `bridge_tool_name` 净化，Go
//! `bridgeToolName` :5616——`[A-Za-z0-9_-]{1,51}` 之外的名字换成
//! `_` + sha256 前 6 字节 hex 后缀，出站的 tool_use 块再映射回原名）。
//! tools/call 是**阻塞-回填**：Claude Code 调 caller 的工具 → 应答流里
//! 出现 `stop_reason:"tool_use"` 的 tool_use 块（映射回原名后照常发给
//! caller，SSE 在 message_stop 处收口、进程驻留等结果）→ caller 的下一
//! 个请求带 tool_result 块 → `match`（:5851 byExactID 口径）按
//! tool_use_id 找到驻留 run → 结果经 MCP 回填 → 拿续答（一回合内完成，
//! `BRIDGE_TIMEOUT` 600s 就是这段等待的硬上限，Go `runTimeout`）。
//!
//! - **helper 形态的偏差**：Go 的 helper 是「本进程 + 隐藏子命令」被
//!   Claude Code 当 **stdio** MCP server 拉起，tools/call 经环回 HTTP
//!   回调网关（claude_subscription.go:1057 的 mcp-config +
//!   claudebridge/mcp.go）。我们没有可用的隐藏子命令入口（main.rs 不在
//!   本模块手里），改为**进程内环回 HTTP 的 MCP server**（std::net，
//!   127.0.0.1 随机端口，mcp-config 里 `{"type":"http","url":…}`），
//!   JSON-RPC 协议（initialize/tools/list/tools/call/notification）与
//!   Go helper 逐字段同形。Claude Code 不支持 http 型 mcp-config 的
//!   旧版本上表现为「没有工具」，不影响对话本身。
//! - **注册时机偏差**：Go 在 tools/call 回调到达时才登记 `b.calls`
//!   （match 侧因此有 20×25ms 重试，:5928）；我们在泵线程看到
//!   tool_use 块的 `content_block_start` 就登记——同一泵步骤里先登记
//!   后放帧，客户端不可能在登记前应答，**无需重试**。结果先到、
//!   tools/call 后到的窄竞态用 Go 的 `early` 表（:6377）同样兜住。
//! - caller 不带 tools 的请求**零变化**：不写 mcp-config、不挂 helper、
//!   `--tools ""` 照旧（见测试 no-MCP 断言）。
//! - 安全：工具定义/调用参数是 caller 数据，helper 只做宣告与转发，
//!   **不执行任何东西**；helper 死了当回合失败（410→rpc error），网关
//!   不崩；`tool_choice:"none"` 时 mcpCall 直接回「No tool can be
//!   used…」（Go :6539）；`name:` 选择只宣告那个工具（Go bridgeTools）。
//!
//! ## 会话持久化 / --resume 续命（2026-11-19，差距清单第 1 条的最小可用集）
//!
//! Go `shelve`（:2753）/ `savedSession`（:2695）/ `unshelve`（:2820）/
//! `start` 的 from 分支（:933、:1104-1201）的移植。驻留池淘汰（IDLE_MOST
//! 容量 / IDLE_LONGEST 过期）的 run 不再只是杀进程：**淘汰即入架**——run
//! 的 Claude Code 会话 id（泵线程从 system/init 收，Go :4714）连同会话键
//! 落盘；下一回合驻留池不中时按键查架，`--resume <id>` 续跑，只喂增量
//! （Go renderClaudeTurn 的喂法），缓存前缀原样命中——不再全量重写
//! （Go 文件头 :15-29 / savedSession :2686-2694 解释的正是这个代价）。
//!
//! - **会话文件本体是 Claude Code 自己写的**（Go claudeSessions :3448 的
//!   形状：`$CLAUDE_CONFIG_DIR/projects/<工作目录编码>/<sid>.jsonl`）。为
//!   此 `bridge_args` **摘掉 `--no-session-persistence`**（Go :1116 的
//!   DeleteFunc 同款），并把 `CLAUDE_CONFIG_DIR` 显式传给子进程（与本模块
//!   的目录判定同源）。孤儿文件由 `sweep_sessions_once`（每进程第一次
//!   spawn，Go sweepSessions :3523 的口径）按 mtime 收走。
//! - **落盘位置**：架是引擎数据目录的 `claude-shelf.json`（与
//!   providers.json / logins.json 同层，不另立目录），形状
//!   `{ <turn_key>: {session, effort, at} }`，容量 64（Go shelfMost :2684），
//!   寿命 = IDLE_LONGEST（Go expires :2794）。Go 的架在内存里（进程重启即
//!   失忆），这里落盘是**有意的加码**：会话文件本来就在磁盘上，键又是
//!   内容哈希，跨进程重启照样能续。
//! - **失败回退**（Go start :943-947）：续跑起不来（会话文件不在 / CLI
//!   加载不了会话直接退 / 报错 result）→ 落回现行全量新起路径。Go 用
//!   `--resume` 前的 initialize 控制握手探这条线（:1426-1432），我们没有
//!   control 通道（差距第 2/8 条），改为**首帧偷看**：spawn + tell 之后先
//!   读一步，Event = 续上了（帧存回 reader 照常供流）；Failed/收口 =
//!   续不上，落回全量。回退只消费架条目，**不删会话文件**（Go discard
//!   会删，我们保守一档交给 sweep——偷看比握手探得宽，误删真会话的代价
//!   是一整段缓存重写，不值得）。
//! - **淘汰顺序不变**：容量淘汰仍按最久未用先让位（keep_idle_locked 的
//!   min_by_key 没动），victim 先入架后杀（Go keepIdle :2664-2676 的顺序）。
//!
//! ## 差距清单（Go 有、本路径不做，后续批次可领）
//! 每条注明 Go 文件/函数，接手时按图索骥：
//! 1. ~~会话持久化 / --resume~~ **最小集已交付**（见上节）；仍未搬：
//!    `branch` / `retryBranch` / `cutSession*`（:2954/:3099/:3217——
//!    「另一请求先答了工具结果」与重发回合的**会话切割分叉**，没有它，
//!    这两类回合仍落全量新起）、`replaySession`（transcript 回放）、
//!    `rewritten` / `looseTurn`（:3107/:2315 客户端改写历史的续聊）、
//!    `behindKey` side-question 回退（:2044）、convKey 跨模型续聊与
//!    `retire` 清架（:2413，我们的键已含 model，同键必同模型）、
//!    `cachePrefix.accepts` 校验（:2863）、`keepForReset` 的 `until`
//!    （:2716）、进程退出时的 `letGoIdle` 全量入架（:8198，引擎正常退出
//!    时架子上的会话随进程失忆——会话文件留给下一次启动的 sweep）。
//! 2. ~~跨回合驻留进程~~ **已交付**（见上节）；仍未搬：side question 的
//!    `behindKey` 回退（:2044-2076）、客户端改写历史的 `rewritten` /
//!    looseTurn（:3107）、conversationKeys 跨模型续聊（conv 键只做
//!    匹配主键的原料，shelf 才消费）、`oversized` 上下文体检（:2595）、
//!    `setEffort` 热改档位（:2143，档位不一致时我们不复用而是新起）。
//! 3. ~~调用方工具（MCP helper）~~ **已交付**（见上节）；仍未搬：
//!    客户端用自家 id 复述调用的 `remap` 匹配（:5909——我们只认
//!    byExactID，AmpCode 式改写 id 的 caller 会话不走驻留）、结果随行
//!    用户文字的 queue-ack 排序（`queue` :6413 等 Claude Code 确认排队
//!    再喂结果，#1878——我们直接写行，不确认）、`follows` 改写检测
//!    （:3094）、mid-run 增补工具的 retool（`more`/`listChanged`，
//!    :8113）、websearch/advisor/StructuredOutput 侧工具。
//! 4. **warmup / 探活**：Go `claude_warmup.go`（askClaude / warmClaude）。
//! 5. **图片**：入站 base64/URL image 块按原位置转发；Go `imageBlocks`
//!    :5798 的 2000px/5MiB 预缩放仍未搬，超大图由上游 API 自行裁定。
//!    lost media 恢复、URL 图抓取（`fetchURLImages` :5525）不做。
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
//!    schema（`cliSchema` :6829）、safeguards（:1706）、`parkedMost`
//!    停靠上限（:3764——我们用回合 deadline 一并覆盖）。
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
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::catalog::VendorRow;
use super::effort::Proto;
use super::httpd::{Reply, Request};
use super::store::Provider;

pub const CLAUDE_ID: &str = "claude";
const QUOTA_NAME: &str = "Claude (Pro/Max)";
/// 桥进程整回合硬上限（Go `runTimeout` 同量级：run.timer，一个回合等
/// caller 工具结果最长 10 分钟；`continueWith` 会重置，Go :6357）。
const BRIDGE_TIMEOUT: Duration = Duration::from_secs(600);
/// 驻留 run 的 idle 上限（Go `idleLongest = claudeCacheTTL = time.Hour`，
/// claude_subscription.go:151-152——一小时后 prompt 缓存反正已失效，
/// 留着只占进程）。
const IDLE_LONGEST: Duration = Duration::from_secs(60 * 60);
/// idle 池容量（Go `idleMost` :153）：最久未用的先让位。
const IDLE_MOST: usize = 6;
/// /usage 读的硬上限（Go runClaudeTree 的 ctx 截断同量级）。
const USAGE_TIMEOUT: Duration = Duration::from_secs(60);
/// rate_limit_event 额度读数的新鲜期（窗口 5h/7d，半小时前的读数足够新）。
const LIMITS_FRESH: Duration = Duration::from_secs(30 * 60);
/// /usage 兜底的最短间隔（CLI 一次要几秒，别被轮询打爆）。
const USAGE_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// 额度面板最多陪 /usage 等多久：壳侧读缓存只等 4s，等超了整张面板都空。
/// 没等到的读数由后台线程写进缓存，下一次拉取就有。
const USAGE_WAIT: Duration = Duration::from_millis(2500);
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
    /// 会话架 claude-shelf.json 的家目录（None = 不读写架）。
    pub shelf: Option<PathBuf>,
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

/// 毫秒钟（架条目的 at 用它——秒级分辨率下，测试缝把 idleLongest 压到
/// 亚秒时会整批秒过期）。
fn now_millis() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
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

/// `/v1/models` 的 claude 行（有凭据 + CLI + 用户没自定义 claude 来源 +
/// 订阅共享已开启才有；2026-11-22 四族默认不共享）。
pub fn listed_rows(providers: &[Provider]) -> Vec<VendorRow> {
    if providers.iter().any(|p| p.id == CLAUDE_ID) || !super::family_enabled(CLAUDE_ID) || !available() {
        return Vec::new();
    }
    MODELS
        .iter()
        .map(|(id, ctx)| VendorRow {
            id: id.to_string(),
            context_window: *ctx,
            max_output: 0,
            // stream-json 的 user content 接受 base64/URL image 块；桥现在
            // 按原位置转发，目录必须如实宣告，否则发送端会提前禁用附件。
            images: true,
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

/// 模型名进命令行前的字符集闸（见 intercept 的注入批注释）：首字符必须
/// 字母/数字（拒绝 `-x` 旗标伪装与 `&whoami` 这类首字符元命令），其余只许
/// `._:+-` 与字母数字。CLI 实名表（claude-opus-5-5 / sonnet / opus …）全过。
fn model_name_safe(model: &str) -> bool {
    let mut chars = model.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '+' | '-'))
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

/// Prompt 的 content 数组。文本先攒着；遇到图片时先把文本 flush 成 text
/// 块，再放原始 image 块，保持图片在对话中的位置。
struct PromptBuilder {
    blocks: Vec<Value>,
    text: String,
    open_result: Option<String>,
}

impl PromptBuilder {
    fn new() -> Self {
        Self { blocks: Vec::new(), text: String::new(), open_result: None }
    }

    fn push_str(&mut self, text: &str) {
        self.text.push_str(text);
    }

    fn flush_text(&mut self) {
        if !self.text.is_empty() {
            self.blocks.push(json!({ "type": "text", "text": std::mem::take(&mut self.text) }));
        }
    }

    fn push_image(&mut self, block: &Value) {
        match claude_image_block(block) {
            Some(image) => {
                self.flush_text();
                self.blocks.push(image);
            }
            None => self.text.push_str("[image omitted: unsupported image source]"),
        }
    }

    fn close_result(&mut self, as_user: bool) {
        if let Some(id) = self.open_result.take() {
            if as_user {
                self.push_str(&format!("\n[end of tool result id={id}]\n\n[user message]\n"));
            } else {
                self.push_str(&format!("\n[end of tool result id={id}]\n\n"));
            }
        }
    }

    fn finish(mut self) -> Vec<Value> {
        self.flush_text();
        if self.blocks.is_empty() {
            self.blocks.push(json!({ "type": "text", "text": "[continue]" }));
        }
        self.blocks
    }
}

fn claude_image_block(block: &Value) -> Option<Value> {
    let source = block.get("source")?;
    let kind = source.get("type").and_then(Value::as_str)?;
    let source = match kind {
        "base64" => {
            let media = source.get("media_type").and_then(Value::as_str)?;
            let data = source.get("data").and_then(Value::as_str)?;
            if !media.starts_with("image/") || data.is_empty() {
                return None;
            }
            json!({ "type": "base64", "media_type": media, "data": data })
        }
        "url" => {
            let url = source.get("url").and_then(Value::as_str)?;
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return None;
            }
            json!({ "type": "url", "url": url })
        }
        _ => return None,
    };
    Some(json!({ "type": "image", "source": source }))
}

/// content 块数组 → Claude Code content。text 直写；thinking 跳过——
/// 「thinking 永远不当话说」（Go :5760）；tool_use / tool_result 写成
/// `[tool call …]` / `[tool result …]` 行；image 原样放进 content 数组。
fn render_blocks(builder: &mut PromptBuilder, blocks: &[Value], offered: Option<&ToolNames>) {
    for b in blocks {
        let ty = b.get("type").and_then(|t| t.as_str()).unwrap_or("");
        match ty {
            "text" => {
                let t = b.get("text").and_then(|x| x.as_str()).unwrap_or("");
                if !t.is_empty() {
                    builder.close_result(true);
                    builder.push_str(t);
                }
            }
            "thinking" | "redacted_thinking" => {}
            "tool_use" => {
                let name = b.get("name").and_then(|x| x.as_str()).unwrap_or("");
                let name = offered.and_then(|n| n.bridged_of(name)).unwrap_or_else(|| name.to_string());
                let id = b.get("id").and_then(|x| x.as_str()).unwrap_or("");
                let args = b.get("input").cloned().unwrap_or(json!({}));
                builder.close_result(false);
                builder.push_str(&format!("\n[tool call {name} id={id} args={args}]"));
            }
            "tool_result" => {
                let id = b.get("tool_use_id").and_then(|x| x.as_str()).unwrap_or("");
                let is_error = b.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false);
                let err = if is_error { " error" } else { "" };
                builder.close_result(false);
                builder.push_str(&format!("\n[tool result id={id}{err}]\n"));
                match b.get("content").unwrap_or(&Value::Null) {
                    Value::String(s) => builder.push_str(s),
                    Value::Array(parts) => {
                        for p in parts {
                            match p.get("type").and_then(|t| t.as_str()) {
                                Some("text") => {
                                    builder.push_str(p.get("text").and_then(|x| x.as_str()).unwrap_or(""));
                                }
                                Some("image") => builder.push_image(p),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
                builder.open_result = Some(id.to_string());
            }
            "image" => builder.push_image(b),
            _ => {}
        }
    }
}

fn render_message(builder: &mut PromptBuilder, m: &Value, offered: Option<&ToolNames>) {
    match m.get("content").unwrap_or(&Value::Null) {
        Value::String(s) => {
            if !s.is_empty() {
                builder.close_result(true);
                builder.push_str(s);
            }
        }
        Value::Array(blocks) => render_blocks(builder, blocks, offered),
        _ => {}
    }
}

/// 入站 anthropic 请求 → CLI stdin 的 content 数组（Go renderClaudePrompt
/// 的简化：system 一块 + 对话块，历史/当前回合用 Go 原样的标记包裹）。
fn render_prompt(body: &Value, offered: Option<&ToolNames>) -> Vec<Value> {
    let messages = body.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default();
    let mut out = PromptBuilder::new();
    let system = system_text(body.get("system").unwrap_or(&Value::Null));
    if !system.is_empty() {
        out.blocks.push(json!({
            "type": "text",
            "text": format!("<external_system_instructions>\n{system}\n</external_system_instructions>\n\n")
        }));
    }
    let split = history_end(&messages);
    if split > 0 {
        out.push_str(HISTORY_OPEN);
    }
    for (i, m) in messages.iter().enumerate() {
        if split > 0 && i == split {
            out.close_result(false);
            out.push_str(HISTORY_CLOSE);
        }
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        let label = if role == "assistant" { "Assistant" } else { "Human" };
        if split > 0 || i > 0 {
            out.push_str("\n\n");
        }
        out.push_str(&format!("{label}: "));
        render_message(&mut out, m, offered);
    }
    if messages.iter().any(|m| calls_tool(m.get("content").unwrap_or(&Value::Null))) {
        out.push_str("\n\n");
        out.push_str(PAST_CALLS_NOTE);
    }
    if split > 0 {
        out.push_str("\n\n</current_turn>");
    }
    out.finish()
}

/// 对话里有没有 assistant 回复（Go hasReply；one-off 询问判定与
/// renderClaudeTurn 的「要不要 Human:/Assistant: 标注」共用口径）。
fn has_reply(messages: &[Value]) -> bool {
    messages.iter().any(|m| m.get("role").and_then(|r| r.as_str()) == Some("assistant"))
}

/// 驻留 run 下一回合的增量喂法（Go renderClaudeTurn :5700）：只渲染
/// `since`（上一条无工具回复之后的消息），带 assistant 回复才加
/// Human:/Assistant: 标注；宣告过的工具的 tool_use 行写桥名（renderParts
/// 同款）；有调用过工具就附 pastCallsNote；closeBlocks 的空兜底同款。
fn render_turn(msgs: &[Value], offered: Option<&ToolNames>) -> Vec<Value> {
    let labeled = has_reply(msgs);
    let mut out = PromptBuilder::new();
    for (i, m) in msgs.iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n");
        }
        if labeled {
            let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
            let label = if role == "assistant" { "Assistant" } else { "Human" };
            out.push_str(&format!("{label}: "));
        }
        render_message(&mut out, m, offered);
    }
    if msgs.iter().any(|m| calls_tool(m.get("content").unwrap_or(&Value::Null))) {
        out.push_str("\n\n");
        out.push_str(PAST_CALLS_NOTE);
    }
    out.finish()
}

// =====================================================================
// 桥子进程
// =====================================================================

/// Go claudeCLIArgs（:3957）：`--settings` 不带（值里带 `"`，见文件头）；
/// caller 带 tools 时 `--mcp-config` 挂 helper（Go :1057 写 mcp-config 的
/// 对应物，插在 `--strict-mcp-config` 后，与 Go 参数序一致）。
fn bridge_args(model: &str, effort: Option<&str>, mcp_config: Option<&str>) -> Vec<String> {
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
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(path) = mcp_config {
        args.push("--mcp-config".into());
        args.push(path.to_string());
    }
    // --no-session-persistence **不带**（Go :1116 的 DeleteFunc 同款——
    // claudeCLIArgs 本来带、startRun 在非 WSL 的 run 上摘掉）：会话落盘是
    // 续命（会话架）的原料——run 被淘汰后，下一回合 `--resume <id>` 从
    // Claude Code 自己写的会话文件续，不再全量重写缓存。孤儿会话文件由
    // sweep_sessions_once 按 mtime 收走。usage_args 的那条保留（/usage
    // 一次性询问，Go claudeUsageArgs :516 也保留）。
    args.extend(["--setting-sources", "", "--dangerously-skip-permissions"].iter().map(|s| s.to_string()));
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
    // 2026-11-22 注入批：不再有 `.cmd/.bat → cmd.exe /C` 重拼接——resolver 已
    // 把 npm 垫片解包成 exe / node+cli.js（prefix 形态）；真正兜底的 .cmd 交 std
    // 直接 spawn（BatBadBut 修复后的行为，/usage 路径同款、有带空格中文路径的
    // 通过测试）。cmd /C 重解析正是两个坑的来源：>2 引号时 cmd 剥首尾引号
    // （空格路径打碎），未加引号的参数按 cmd 语法切分（模型名注入）。
    let cmd = if program.prefix.is_empty() {
        let mut c = Command::new(&program.program);
        c.args(args);
        c
    } else {
        let mut c = Command::new(&program.program);
        c.args(&program.prefix);
        c.args(args);
        c
    };
    prepare_command(cmd)
}

/// /usage 专用：**不经 `cmd /C` 拼接**——程序路径带空格时，空参数 `""` 会让
/// cmd 把路径拆开。.cmd/.bat 交给 std 自己转义（BatBadBut 修复后的行为）。
fn usage_command(program: &Program) -> Command {
    let mut cmd = Command::new(&program.program);
    cmd.args(&program.prefix).args(usage_args());
    prepare_command(cmd)
}

/// 所有 claude 子进程共同的环境、工作目录与窗口标志。
fn prepare_command(mut cmd: Command) -> Command {
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

// =====================================================================
// 工具名映射（Go bridgeToolName / bridgeNames / declaredName，
// claude_subscription.go:5596-5655）
// =====================================================================

/// mcp-config 里的 server 名（模型看到的工具名前缀 `mcp__marionette__`；
/// Go 用 "magpie"，本桥是 Marionette 自己）。
const MCP_SERVER: &str = "marionette";
const MCP_TOOL_PREFIX: &str = "mcp__marionette__";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Go bridgeToolNameOK `^[A-Za-z0-9_-]{1,51}$`（:5632）。
fn bridge_name_ok(s: &str) -> bool {
    let n = s.chars().count();
    (1..=51).contains(&n) && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Go bridgeToolNameHashed `_[0-9a-f]{12}$`（:5634）——已是净化名的不再净化。
fn bridge_name_hashed(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 13 && b[b.len() - 13] == b'_' && b[b.len() - 12..].iter().all(|c| c.is_ascii_hexdigit())
}

/// Go bridgeToolName（:5616）：Claude Code 只认 `[A-Za-z0-9_-]` 的 MCP 工具
/// 名（会自作主张改写 `spawn.agent` 这类名字，:2649 事故）——不合规的名字
/// 换成 `_` 净化 + sha256 前 6 字节 hex 后缀（原名恒同结果），出站时再
/// 映射回原名。已带 `mcp__marionette__` 前缀的（caller 本身就是 Claude 系）
/// 只取余下部分。
fn bridge_tool_name(name: &str) -> String {
    if let Some(rest) = name.strip_prefix(MCP_TOOL_PREFIX) {
        if bridge_name_ok(rest) && !bridge_name_hashed(rest) {
            return rest.to_string();
        }
    }
    if bridge_name_ok(name) && !bridge_name_hashed(name) {
        return name.to_string();
    }
    let sum = Sha256::digest(name.as_bytes());
    let short: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' })
        .take(38)
        .collect();
    format!("{short}_{}", hex(&sum[..6]))
}

/// 双向名表：Go bridgeNames（:5639，桥名→原名）+ renderParts 用的
/// 原名→桥全名（`mcp__marionette__<桥名>`，Go bridgeName :5594）。
#[derive(Clone, Default)]
struct ToolNames {
    declared: HashMap<String, String>,
    bridged_full: HashMap<String, String>,
}

impl ToolNames {
    /// 模型见到的名字 → caller 原名（Go declaredName :5651；查不到时带前缀
    /// 的退余下部分、不带的保持原名——延迟加载工具的口径 :5601-5613）。
    fn declared_of(&self, model_name: &str) -> Option<String> {
        match model_name.strip_prefix(MCP_TOOL_PREFIX) {
            Some(rest) => Some(self.declared.get(rest).cloned().unwrap_or_else(|| rest.to_string())),
            None => None,
        }
    }

    /// caller 原名 → 模型见到的全名；caller 没宣告过的工具 None（原名直传）。
    fn bridged_of(&self, caller_name: &str) -> Option<String> {
        self.bridged_full.get(caller_name).cloned()
    }
}

/// 入站请求的 tools →（helper 宣告表, 名表, 本回合是否许调工具）。
/// Go bridgeTools（:8137）：`name:` 选择只宣告那个工具；`none` 保留全部
/// 宣告（Anthropic 按 tools→system→messages 的前缀缓存，撤了宣告就整段
/// 重写缓存）但调用时拒绝（`allowed_all=false`，mcpCall :6539 直答）。
/// schema 缺失/不是对象 → `{"type":"object","properties":{}}`。
fn bridge_tools(tools: &[Value], tool_choice: &str) -> (Vec<Value>, ToolNames, bool) {
    let want = tool_choice.strip_prefix("name:");
    let mut out = Vec::new();
    let mut names = ToolNames::default();
    for t in tools {
        let Some(name) = t.get("name").and_then(|n| n.as_str()) else { continue };
        if let Some(w) = want {
            if name != w {
                continue;
            }
        }
        let schema = match t.get("input_schema").or_else(|| t.get("schema")) {
            Some(s) if s.is_object() => s.clone(),
            _ => json!({ "type": "object", "properties": {} }),
        };
        let bridged = bridge_tool_name(name);
        let mut row = json!({ "name": bridged, "inputSchema": schema });
        if let Some(d) = t.get("description").and_then(|d| d.as_str()) {
            row["description"] = json!(d);
        }
        out.push(row);
        names.declared.insert(bridged.clone(), name.to_string());
        names.bridged_full.insert(name.to_string(), format!("{MCP_TOOL_PREFIX}{bridged}"));
    }
    let allowed_all = tool_choice != "none";
    (out, names, allowed_all)
}

/// 入站 tool_choice（anthropic 的对象/字符串两形态）→ Go 的字符串口径
/// （""/none/required/name:<n>；`auto` 视同没选）。
fn tool_choice_of(body: &Value) -> String {
    match body.get("tool_choice") {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Object(o)) => match o.get("type").and_then(|t| t.as_str()) {
            Some("auto") => String::new(),
            Some("any") | Some("required") => "required".into(),
            Some("tool") => o
                .get("name")
                .and_then(|n| n.as_str())
                .map(|n| format!("name:{n}"))
                .unwrap_or_default(),
            Some("none") => "none".into(),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

// =====================================================================
// 会话键（Go turnKey :3790 / conversationKeys :3803 / hashMessages :3870 /
// messageParts·messageWords——claude_lost_media.go）
// =====================================================================

/// 去空白词（Go messageWords：字段直连、无任何空白——客户端怎么切 text
/// 块、怎么复述都不影响键）。
fn strip_words(s: &str) -> String {
    s.split_whitespace().collect::<String>()
}

fn image_fingerprint(block: &Value) -> String {
    let source = block.get("source").unwrap_or(&Value::Null);
    let mut h = Sha256::new();
    for field in ["type", "media_type", "data", "url"] {
        h.update(field.as_bytes());
        h.update(b"\0");
        h.update(source.get(field).and_then(Value::as_str).unwrap_or("").as_bytes());
        h.update(b"\0");
    }
    hex(&h.finalize()[..8])
}

/// tool_result 块的文本体（字符串或块数组里的 text/image 拼接——与
/// render_blocks 的提取同口径；图片用内容指纹，不用固定占位，避免不同
/// 图片得到相同的会话键）。
fn result_text_of(b: &Value) -> String {
    match b.get("content").unwrap_or(&Value::Null) {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|p| match p.get("type").and_then(|t| t.as_str()) {
                Some("text") => p.get("text").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                Some("image") => format!("[image {}]", image_fingerprint(p)),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// 每条消息的规范词（Go messageParts）：text 取词；tool_use 记
/// `\x01call <名> #<第 N 个调用>`（**不是 id**——客户端可能用自家 id 复述，
/// 调用的次序才是身份）；tool_result 记 `\x01result #<应答的第 N 个>`；
/// thinking 跳过；image 记 `\x01image <内容指纹>`。content 为纯字符串的消息
/// （anthropic 允许）按一整段 text 计。
fn message_words(msgs: &[Value]) -> Vec<String> {
    let mut calls: HashMap<String, usize> = HashMap::new();
    let mut n = 0usize;
    msgs.iter()
        .map(|m| {
            let mut b = String::new();
            match m.get("content") {
                Some(Value::String(s)) => b.push_str(&strip_words(s)),
                Some(Value::Array(blocks)) => {
                    for p in blocks {
                        let ty = p.get("type").and_then(|t| t.as_str()).unwrap_or("");
                        let words = match ty {
                            "text" => strip_words(p.get("text").and_then(|x| x.as_str()).unwrap_or("")),
                            "tool_use" => {
                                n += 1;
                                if let Some(id) = p.get("id").and_then(|x| x.as_str()) {
                                    calls.insert(id.to_string(), n);
                                }
                                let name = p.get("name").and_then(|x| x.as_str()).unwrap_or("");
                                strip_words(&format!("\x01call {name} #{n}"))
                            }
                            "tool_result" => {
                                let k = p
                                    .get("tool_use_id")
                                    .and_then(|x| x.as_str())
                                    .and_then(|id| calls.get(id).map(|v| v.to_string()))
                                    .unwrap_or_else(|| "?".into());
                                strip_words(&format!("\x01result #{k} {}", result_text_of(p)))
                            }
                            "image" => {
                                strip_words(&format!("\x01image {}", image_fingerprint(p)))
                            }
                            _ => String::new(),
                        };
                        b.push_str(&words);
                    }
                }
                _ => {}
            }
            b
        })
        .collect()
}

/// Go hashMessages（:3870）：相邻同角色消息合并（角色标记只在角色变化时
/// 写）——客户端把一条回复拆成 N 条 assistant 消息与 run 自己记的一条
/// 是同一个键（#5705）。
fn hash_messages(h: &mut Sha256, msgs: &[Value]) {
    let mut role = String::new();
    for (i, words) in message_words(msgs).iter().enumerate() {
        if !words.is_empty() {
            let r = msgs[i].get("role").and_then(|x| x.as_str()).unwrap_or("user");
            if r != role {
                role = r.to_string();
                h.update(format!("\x00{r}:").as_bytes());
            }
            h.update(words.as_bytes());
        }
    }
}

/// 一个回合请求的身份（Go turnKey 的材料）。msgs 是完整消息表——
/// keepIdle 计算「请求消息 + 刚给出的回复」的键、nextTurn 计算
/// 「到上一条无工具回复为止」的键都用它。
#[derive(Clone)]
struct RequestIdentity {
    owner: String,
    model: String,
    effort: Option<String>,
    tool_choice: String,
    system: String,
    tools_json: String,
    tools_empty: bool,
    msgs: Vec<Value>,
}

/// Go turnKey（:3790）的子集：owner·model·effort 有无·tool_choice·system·
/// tools + 消息词哈希（webSearch/format/thinkOff/billing 头不在本桥面里）。
fn turn_key(id: &RequestIdentity, msgs: &[Value]) -> String {
    let mut h = Sha256::new();
    for part in [&id.owner, &id.model, &id.tool_choice, &id.system, &id.tools_json] {
        h.update(part.as_bytes());
        h.update(&[0]);
    }
    h.update(&[id.effort.is_some() as u8]);
    hash_messages(&mut h, msgs);
    hex(&h.finalize())
}

/// Go nextTurn（:2171）：这是不是「上一条无工具回复之后的新回合」，是则
/// 给 (会话键, 增量消息的起始下标)。末条是 assistant / 没有这样的回复 /
/// 增量里有应答**更早**调用的 tool_result（工具循环的续轮，该走 match
/// 而不是 resume）→ None。
fn next_turn(id: &RequestIdentity) -> Option<(String, usize)> {
    let msgs = &id.msgs;
    if msgs.is_empty() || msgs.last().and_then(|m| m.get("role")).and_then(|r| r.as_str()) == Some("assistant") {
        return None;
    }
    let mut j = msgs.len() as isize - 1;
    while j >= 0 {
        let m = &msgs[j as usize];
        let role = m.get("role").and_then(|r| r.as_str()).unwrap_or("user");
        if role == "assistant" && !calls_tool(m.get("content").unwrap_or(&Value::Null)) {
            break;
        }
        j -= 1;
    }
    if j < 0 {
        return None;
    }
    let split = j as usize + 1;
    // Go :2188——since 里的 tool_result 必须应答 since 之内的调用
    let mut calls: Vec<&str> = Vec::new();
    for m in &msgs[split..] {
        for b in m.get("content").and_then(|c| c.as_array()).into_iter().flatten() {
            match b.get("type").and_then(|t| t.as_str()) {
                Some("tool_use") => {
                    if let Some(cid) = b.get("id").and_then(|x| x.as_str()) {
                        calls.push(cid);
                    }
                }
                Some("tool_result") => {
                    let id = b.get("tool_use_id").and_then(|x| x.as_str()).unwrap_or("");
                    if !calls.contains(&id) {
                        return None;
                    }
                }
                _ => {}
            }
        }
    }
    Some((turn_key(id, &msgs[..split]), split))
}

// =====================================================================
// 工具结果提取（Go match 的 fresh 提取 :5857 + saidWithResults）
// =====================================================================

/// 尾部非 assistant 消息里的一个 tool_result（Go match 的 fresh）。
struct FreshResult {
    id: String,
    content: String,
    is_error: bool,
}

/// 尾部有没有 tool_result、都是谁（Go match :5857 的提取口径）。None =
/// 尾部没有 tool_result（普通新回合，match 让位）。
fn fresh_results(messages: &[Value]) -> Option<Vec<FreshResult>> {
    let mut i = messages.len();
    while i > 0 && messages[i - 1].get("role").and_then(|r| r.as_str()) != Some("assistant") {
        i -= 1;
    }
    let mut out = Vec::new();
    for m in &messages[i..] {
        for b in m.get("content").and_then(|c| c.as_array()).into_iter().flatten() {
            if b.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                out.push(FreshResult {
                    id: b.get("tool_use_id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
                    content: result_text_of(b),
                    is_error: b.get("is_error").and_then(|x| x.as_bool()).unwrap_or(false),
                });
            }
        }
    }
    (!out.is_empty()).then_some(out)
}

/// 尾部随工具结果一起来的用户文字（Go saidWithResults；magpie 自家通知的
/// agentNotice/midTurnHead 甄别不搬——那是它们 harness 的形状）。没有
/// tool_result 就没有 said。
fn said_with_results(messages: &[Value]) -> String {
    let mut i = messages.len();
    while i > 0 && messages[i - 1].get("role").and_then(|r| r.as_str()) != Some("assistant") {
        i -= 1;
    }
    let mut said: Vec<String> = Vec::new();
    let mut results = false;
    for m in &messages[i..] {
        for b in m.get("content").and_then(|c| c.as_array()).into_iter().flatten() {
            match b.get("type").and_then(|t| t.as_str()) {
                Some("tool_result") => results = true,
                Some("text") => {
                    let t = b.get("text").and_then(|x| x.as_str()).unwrap_or("");
                    if results && !t.trim().is_empty() {
                        said.push(t.to_string());
                    }
                }
                _ => {}
            }
        }
    }
    if !results {
        return String::new();
    }
    said.join("\n\n")
}

// =====================================================================
// 驻留 run：进程跨回合存活，泵线程独占 stdout 翻译成 anthropic 帧压进
// 「当前段」；回合 = attach（挂段）→ tell（喂 user 行）→ 段收口
// （result / tool_use 挂起 / 失败）。
// =====================================================================

/// 段内待消费的一条产物。
enum Queued {
    Frame(Value),
    Failed(u16, String),
}

/// 一条 stdout envelope 经翻译后的产物（段读者视角）。
enum Step {
    Event(Value),
    Failed(u16, String),
}

/// 等待 caller 结果的一次 tools/call（Go run.pending 的 waiter；投递 =
/// 置 arrived + 唤醒，Go 走 channel，这里走 Condvar）。
struct PendingCall {
    arrived: Option<Value>,
}

struct RunState {
    /// 进程终局（EOF / 被杀 / 失败回合）。置位后段读者吐完队列即 EOF、
    /// tools/call 等待者拿 rpc error、池清理完成。
    closed: bool,
    /// 看门狗杀点：回合中 = 600s 预算（continueWith 重置，Go endIn(runTimeout)
    /// :6357）；idle = now+IDLE_LONGEST（Go endIn(idleLongest)）。
    deadline: Instant,
    // —— 当前段（attach 起、segment_done 止）——
    queue: VecDeque<Queued>,
    segment_done: bool,
    /// 段在 tool_use 的 message_stop 收口，进程驻留等 caller 结果。
    parked: bool,
    started: bool,
    got_stop: bool,
    streamed: Vec<String>,
    /// 本段回复的词（Go ended 的 said——keepIdle 键的回复部分）。
    said_words: String,
    /// 当前 assistant 消息里的 tool_use id（park 判定）。
    msg_tool_ids: Vec<String>,
    msg_stop_reason: Option<String>,
    /// 最近 message_start 报的模型名（whole 合成的兜底）。
    model_echo: String,
    /// Claude Code 的会话 id（Go run.sessionID :303/:4716）：system/init 上
    /// 收的；resume 起跑的 run 预填被续的 id（Go :1478）。会话架的原料——
    /// 淘汰入架时它就是落盘的 session。
    session_id: String,
    // —— 回合身份（段的活动期有效；keepIdle 消费）——
    identity: Option<RequestIdentity>,
    /// 本 run 起跑档位（resume 一致性检查；Go setEffort 热改的降级替代）。
    effort: Option<String>,
    /// idle 池里的键（Go r.idleKey；keepIdle 置、take 清）。
    idle_key: Option<String>,
    // —— caller 工具 ——
    names: ToolNames,
    helper_tools: Vec<Value>,
    allowed_all: bool,
    pending: HashMap<String, PendingCall>,
    /// 先到的 tool_result（Go early :6377——客户端比 tools/call 快一步）。
    early: HashMap<String, Value>,
}

/// 一个驻留的 Claude Code 进程。
struct RunCore {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<Option<std::process::ChildStdin>>>,
    stderr_tail: Arc<Mutex<String>>,
    /// helper 路径 token（没挂 helper 的 run 为 None，不进 runs 表）。
    token: Option<String>,
    /// helper 的 mcp.json 家目录（终局时删）。
    cfg_dir: Option<PathBuf>,
    state: Arc<(Mutex<RunState>, Condvar)>,
}

type RunLock<'a> = std::sync::MutexGuard<'a, RunState>;

fn lock_run(core: &RunCore) -> RunLock<'_> {
    core.state.0.lock().unwrap_or_else(|p| p.into_inner())
}

fn hex_token() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    let t = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{t:x}{n:x}")
}

// =====================================================================
// 池（Go subscriptionBridge 的 idle / calls / runs 三表，:81-89）
// =====================================================================

struct Pool {
    /// 会话键 → (run, 入池时刻)（Go b.idle）。
    idle: HashMap<String, (Arc<RunCore>, Instant)>,
    /// tool_use id → 等待结果的 run（Go b.calls）。
    calls: HashMap<String, Arc<RunCore>>,
    /// helper token → run（Go b.runs）。
    runs: HashMap<String, Arc<RunCore>>,
}

static POOL: std::sync::LazyLock<Mutex<Pool>> =
    std::sync::LazyLock::new(|| Mutex::new(Pool { idle: HashMap::new(), calls: HashMap::new(), runs: HashMap::new() }));

/// 测试缝：idle 上限/容量的覆盖点（生产恒常量）。
#[cfg(test)]
static TEST_IDLE_LONGEST: Mutex<Option<Duration>> = Mutex::new(None);
#[cfg(test)]
static TEST_IDLE_MOST: Mutex<Option<usize>> = Mutex::new(None);

fn idle_longest() -> Duration {
    #[cfg(test)]
    {
        TEST_IDLE_LONGEST.lock().unwrap_or_else(|p| p.into_inner()).unwrap_or(IDLE_LONGEST)
    }
    #[cfg(not(test))]
    {
        IDLE_LONGEST
    }
}

fn idle_most() -> usize {
    #[cfg(test)]
    {
        TEST_IDLE_MOST.lock().unwrap_or_else(|p| p.into_inner()).unwrap_or(IDLE_MOST)
    }
    #[cfg(not(test))]
    {
        IDLE_MOST
    }
}

/// 池内惰性清扫（Go 的每 run timer 换成碰池即扫）：过期 idle 项出池，
/// 返回 (会话键, run) 名单——**杀进程在池锁外做**（持锁不做子进程操作），
/// 出池的键跟着走（入架要用）。
fn sweep_pool(p: &mut Pool) -> Vec<(String, Arc<RunCore>)> {
    let mut victims = Vec::new();
    let expired: Vec<String> = p
        .idle
        .iter()
        .filter(|(_, (_, at))| at.elapsed() >= idle_longest())
        .map(|(k, _)| k.clone())
        .collect();
    for k in expired {
        if let Some((run, _)) = p.idle.remove(&k) {
            victims.push((k, run));
        }
    }
    victims
}

/// Go keepIdle（:2615）：回合收口后 run 按会话键进 idle 池。键冲突逐旧、
/// 同 run 双键去重（:2649 的防膨胀）、容量超限淘汰最久（:2656）。
/// **在 state 锁内调用**（嵌套池锁是本模块唯一允许的锁向，见 pump_thread
/// 注释）；返回 (会话键, run) 待逐名单——调用方**出锁后**交给
/// [`release_victims`]（入架 + 杀进程）。
fn keep_idle_locked(run: &Arc<RunCore>, g: &mut RunLock<'_>, key: &str) -> Vec<(String, Arc<RunCore>)> {
    if g.closed {
        return Vec::new(); // 回答途中死了（Go #1667）
    }
    g.deadline = Instant::now() + idle_longest();
    g.idle_key = Some(key.to_string());
    let mut p = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let mut victims = sweep_pool(&mut p);
    // 同 run 双键去重：只留新键（Go :2649）
    let stale: Vec<String> = p
        .idle
        .iter()
        .filter(|(k, (r, _))| *k != key && Arc::ptr_eq(r, run))
        .map(|(k, _)| k.clone())
        .collect();
    for k in stale {
        p.idle.remove(&k);
    }
    if let Some((old, _)) = p.idle.get(key) {
        if !Arc::ptr_eq(old, run) {
            victims.push((key.to_string(), old.clone()));
            p.idle.remove(key);
        }
    }
    p.idle.insert(key.to_string(), (run.clone(), Instant::now()));
    while p.idle.len() > idle_most() {
        let oldest = p
            .idle
            .iter()
            .min_by_key(|(_, (_, at))| *at)
            .map(|(k, _)| k.clone());
        match oldest {
            Some(k) => {
                if let Some((v, _)) = p.idle.remove(&k) {
                    victims.push((k, v));
                }
            }
            None => break,
        }
    }
    victims
}

/// Go resume 的查找臂（:2034-2063 的主键命中）：按会话键取出 idle run。
/// 可用性检查在池锁外（closed / 档位不一致的不动它——档位不一致的继续
/// 等自己的回合，Go 是 setEffort 热改后继续用，我们不会热改就让位）。
fn take_idle_run(key: &str, effort: Option<&str>) -> Option<Arc<RunCore>> {
    let run = {
        let mut p = POOL.lock().unwrap_or_else(|e| e.into_inner());
        let victims = sweep_pool(&mut p);
        drop(p);
        release_victims(victims);
        let p = POOL.lock().unwrap_or_else(|e| e.into_inner());
        p.idle.get(key).map(|(r, _)| r.clone())
    }?;
    let usable = {
        let g = lock_run(&run);
        !g.closed && g.effort.as_deref() == effort
    };
    if !usable {
        return None;
    }
    // 双检：可用才出池（并发同键的另一个请求抢不到就算了，落新起）
    {
        let mut p = POOL.lock().unwrap_or_else(|e| e.into_inner());
        match p.idle.get(key) {
            Some((r, _)) if Arc::ptr_eq(r, &run) => {
                p.idle.remove(key);
            }
            _ => return None,
        }
    }
    lock_run(&run).idle_key = None;
    Some(run)
}

/// Go match（:5851）的 byExactID 口径：尾部 tool_result 的 id 全部指向
/// **同一个**驻留 run 才命中（两个不同 run = ambiguous，谁也不给）。
/// remap/20×25ms 重试不搬——我们在 tool_use 块下发前就登记 calls（泵线程
/// 同一步骤先登记后放帧），不存在 Go :5928 的回调注册竞态。
fn match_run(messages: &[Value]) -> Option<(Arc<RunCore>, Vec<FreshResult>)> {
    let fresh = fresh_results(messages)?;
    let mut p = POOL.lock().unwrap_or_else(|e| e.into_inner());
    let victims = sweep_pool(&mut p);
    let mut found: Option<Arc<RunCore>> = None;
    for r in &fresh {
        let Some(run) = p.calls.get(&r.id) else { continue };
        match &found {
            Some(f) if Arc::ptr_eq(f, run) => {}
            Some(_) => {
                drop(p);
                release_victims(victims);
                return None; // ambiguousCalls（Go :5892）
            }
            None => found = Some(run.clone()),
        }
    }
    drop(p);
    release_victims(victims);
    found.map(|run| (run, fresh))
}

// =====================================================================
// 会话架（Go b.shelf / savedSession 的落盘对应物，:2695-2815）：池淘汰的
// run 不再只是杀进程——它的 Claude Code 会话 id 连同会话键落盘，下一回合
// `--resume <id>` 从 Claude Code 自己写的会话文件续（Go claudeSessions
// :3448 的形状：`$CLAUDE_CONFIG_DIR/projects/<工作目录编码>/<sid>.jsonl`，
// bridge_args 摘掉 --no-session-persistence 之后由 CLI 自己落盘）。
// =====================================================================

/// 架容量（Go shelfMost :2684 = 64）：每个条目一条会话文件。
const SHELF_MOST: usize = 64;

/// 架的一个条目（Go savedSession 的最小集）。
#[derive(Clone, Serialize, Deserialize)]
struct ShelfEntry {
    /// Claude Code session id（泵线程从 system/init 收，Go :4714）。
    session: String,
    /// 起跑档位：续命请求的档位必须一致（take_idle_run 同款原则——Go 是
    /// setEffort 热改后接着用，我们不搬，档位不一致就不复用）。
    effort: Option<String>,
    /// 入架时刻（unix 毫秒；Go s.at）。寿命 = IDLE_LONGEST（Go expires
    /// :2794——一小时后 prompt 缓存反正已失效）。
    at: i64,
}

/// 架文件锁：读-改-写全程串行。**绝不持 state/pool 锁拿它**（里面有文件
/// IO），所有调用点都在锁外。
static SHELF_LOCK: Mutex<()> = Mutex::new(());

/// 架的家：引擎数据目录（providers.json / logins.json 同层，不另立目录）。
#[cfg(not(test))]
fn shelf_dir() -> Option<PathBuf> {
    super::config_dir()
}
/// 测试缝：随 World 的临时目录走（绝不碰真实 ~/.marionette）。
#[cfg(test)]
fn shelf_dir() -> Option<PathBuf> {
    seam().and_then(|s| s.shelf)
}

fn shelf_path() -> Option<PathBuf> {
    shelf_dir().map(|d| d.join("claude-shelf.json"))
}

/// Go claudeProjectName（:482）：Claude Code 给工作目录的会话文件夹名——
/// 路径里 ASCII 字母数字之外的字符全换 `-`。
fn claude_project_name(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// Go claudeSessions（:3448）的单目录版：本桥工作目录（work_dir，Go
/// claudeWorkDir 的共享目录同款）的会话文件夹。Go 还查 evalSymlinks 的
/// 第二个目录（macOS 的 /var），Windows temp 不 symlink，不搬。
fn claude_sessions() -> Option<PathBuf> {
    let dir = claude_config_dir()?;
    Some(dir.join("projects").join(claude_project_name(&work_dir().to_string_lossy())))
}

/// Go removeSession（:3465）的 sid 护栏再加一档：只认 `[A-Za-z0-9-]`
/// （uuid 的字符域），拼路径前挡住穿越。
fn sid_ok(sid: &str) -> bool {
    !sid.is_empty() && sid.len() <= 128 && sid.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Go savedSession.saved（:3405）：会话文件还在不在。不在 = resume 必死，
/// 条目作废。
fn session_saved(sessions: &std::path::Path, sid: &str) -> bool {
    sid_ok(sid) && sessions.join(format!("{sid}.jsonl")).is_file()
}

/// 读架（fail-open：读不了/解析不了 = 空架——memos index.json 同款口径）。
fn shelf_read() -> HashMap<String, ShelfEntry> {
    let Some(path) = shelf_path() else { return HashMap::new() };
    let Ok(bytes) = std::fs::read(&path) else { return HashMap::new() };
    serde_json::from_slice(&bytes).unwrap_or_default()
}

/// 写架（调用方持 SHELF_LOCK）。失败安静放过：架只是缓存经济的优化，
/// 写不进去 = 下一回合全量重写，不致命。
fn shelf_write(entries: &HashMap<String, ShelfEntry>) {
    let Some(path) = shelf_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let body = serde_json::to_vec(entries).unwrap_or_default();
    let _ = std::fs::write(&path, body);
}

/// 修剪：过期的出架（Go shelfTimer :2780 的惰性对应——at + idleLongest），
/// 超容量的最久先出（Go trimShelf :2803）。会话文件**不删**：孤儿文件由
/// sweep_sessions_once 按 mtime 收走（Go discard 即时删，我们保守一档，
/// 见模块头偏差记录）。
fn shelf_prune(entries: &mut HashMap<String, ShelfEntry>) {
    let now = now_millis();
    let ttl = idle_longest().as_millis() as i64;
    entries.retain(|_, e| now - e.at < ttl);
    while entries.len() > SHELF_MOST {
        let Some(oldest) = entries.iter().min_by_key(|(_, e)| e.at).map(|(k, _)| k.clone()) else {
            break;
        };
        entries.remove(&oldest);
    }
}

/// 淘汰名单的处置（Go keepIdle :2664 shelve → :2676 abortFor 的顺序）。
/// **锁外调用**：shelve_run 只拿 victim 自己的 state 锁（无嵌套），架文件
/// 在 SHELF_LOCK 下读-改-写。
fn release_victims(victims: Vec<(String, Arc<RunCore>)>) {
    for (key, run) in &victims {
        shelve_run(key, run);
    }
    for (_, run) in &victims {
        abort_run(run);
    }
}

/// 条目入架（shelve_run / pump_eof 共用的写侧）：SHELF_LOCK 下读-改-写。
/// 会话文件不在场的（CLI 没落盘 / 已被清）不收——收了也续不了（Go :2757
/// 的 sid == "" 臂同款保守）。**锁外调用**（state/pool 锁都不许持）。
fn shelve_entry(key: &str, session: &str, effort: Option<String>) {
    if !sid_ok(session) {
        return;
    }
    let Some(sessions) = claude_sessions() else { return };
    if !session_saved(&sessions, session) {
        return;
    }
    let _guard = SHELF_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut entries = shelf_read();
    shelf_prune(&mut entries);
    entries.insert(key.to_string(), ShelfEntry { session: session.to_string(), effort, at: now_millis() });
    shelf_write(&entries);
}

/// 入架（Go shelve :2753 的落盘侧）：victim 等在 key 上、被淘汰时把
/// （key → session id）记进架。
fn shelve_run(key: &str, run: &Arc<RunCore>) {
    let (sid, effort) = {
        let g = lock_run(run);
        (g.session_id.clone(), g.effort.clone())
    };
    shelve_entry(key, &sid, effort);
}

/// 出架（Go unshelve :2820 的主键命中臂 + :2857 delete + startRun :1132 的
/// saved() 检查）：下一回合的键在架上、会话文件还在、档位一致 → 交出
/// session id 并出架（Go unshelve :2857 的 delete，认领语义）。条目过期 /
/// 文件没了 → 顺手清条目（架上的键是内容哈希，死条目永远死）。档位不一致
/// → 条目留着（同键同档位的后续回合还能用，1 小时内自然过期）。
fn unshelve(key: &str, effort: Option<&str>) -> Option<String> {
    let _guard = SHELF_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let mut entries = shelf_read();
    let hit = entries.get(key).cloned();
    shelf_prune(&mut entries); // 过期条目顺手清（含刚查到的这个）
    let hit = hit.filter(|_| entries.contains_key(key));
    let mut taken = None;
    if let Some(e) = hit {
        let resumable = claude_sessions().is_some_and(|d| session_saved(&d, &e.session));
        match resumable.then(|| e.effort == effort.map(|s| s.to_string())) {
            Some(true) => {
                entries.remove(key);
                taken = Some(e.session);
            }
            Some(false) => {} // 档位不一致：条目留着，本回合落全量新起
            None => {
                entries.remove(key); // 会话文件没了：死条目（Go :1132 from = nil）
            }
        }
    }
    shelf_write(&entries);
    taken
}

/// Go sweepSessions（:3523，每进程一次）：桥自己项目文件夹里、上一次进程
/// / 中途死掉的 run 留下的孤儿会话文件——IDLE_LONGEST+1 分钟没动过的收走。
/// 活会话的文件一直在被追加（mtime 新鲜），不会中。**只收本桥工作目录的
/// 项目文件夹**，用户自己的会话目录一个字节不碰。
fn sweep_sessions_once() {
    static SWEPT: AtomicBool = AtomicBool::new(false);
    if SWEPT.swap(true, Ordering::Relaxed) {
        return;
    }
    let Some(dir) = claude_sessions() else { return };
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    let cutoff = idle_longest() + Duration::from_secs(60);
    for e in rd.flatten() {
        let path = e.path();
        if path.extension().and_then(|x| x.to_str()) != Some("jsonl") {
            continue;
        }
        let stale = e.metadata().ok().and_then(|md| md.modified().ok()).is_some_and(|m| m.elapsed().unwrap_or_default() > cutoff);
        if stale {
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// run 终局：标 closed、杀进程、撤出池、删 helper 配置目录。Go abortFor +
/// removeRun 的合并。**绝不持 state/pool 锁调用**（幂等，重复调用无害）。
fn abort_run(core: &Arc<RunCore>) {
    {
        let mut g = lock_run(core);
        if g.closed && core.token.is_none() {
            // 已终局且已清池——仍走一遍杀进程兜底（便宜）
        }
        g.closed = true;
        g.pending.clear();
        core.state.1.notify_all();
    }
    *core.stdin.lock().unwrap_or_else(|p| p.into_inner()) = None; // EOF
    if let Ok(mut c) = core.child.lock() {
        let _ = c.kill();
        let _ = c.wait();
    }
    {
        let mut p = POOL.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = &core.token {
            p.runs.remove(t);
        }
        p.calls.retain(|_, r| !Arc::ptr_eq(r, core));
        p.idle.retain(|_, (r, _)| !Arc::ptr_eq(r, core));
    }
    if let Some(d) = &core.cfg_dir {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// 泵线程：登记一个 tool_use（calls 表，match 的路标）。**必须在对应帧
/// 入队之前调用**（客户端见到块才可能应答，见 match_run 注释）。
fn register_call(run: &Arc<RunCore>, id: &str) {
    if id.is_empty() {
        return;
    }
    let mut p = POOL.lock().unwrap_or_else(|e| e.into_inner());
    p.calls.insert(id.to_string(), run.clone());
}

/// 工具回合挂起（Go readOutput 的 tool_use 收口 + park :3686——段在
/// message_stop 后 EOF，进程驻留等结果；我们不做 parkLongest 分档，
/// 回合 deadline（600s）一并覆盖）。
fn park_segment(run: &RunCore, g: &mut RunLock<'_>) {
    if g.segment_done || g.msg_tool_ids.is_empty() {
        return;
    }
    if g.msg_stop_reason.as_deref() == Some("tool_use") {
        g.parked = true;
        g.segment_done = true;
        run.state.1.notify_all();
    }
}

/// stdout 泵线程（Go readOutput :4310 的移植）：独占 stdout，逐行翻译成
/// anthropic 帧压进当前段；段外（idle/挂起后）只记账不消费。锁序红线：
/// **只允许 state 锁内嵌套池锁**（register_call/keep_idle 材料读取），
/// 反向嵌套全模块不存在。
fn pump_thread(run: Arc<RunCore>, mut stdout: BufReader<std::process::ChildStdout>) {
    loop {
        let mut line = Vec::new();
        match stdout.read_until(b'\n', &mut line) {
            Ok(0) => {
                pump_eof(&run, None);
                return;
            }
            Ok(_) => {}
            Err(e) => {
                pump_eof(&run, Some(format!("claude bridge output: {e}")));
                return;
            }
        }
        let Ok(v) = serde_json::from_slice::<Value>(&line) else { continue };
        if v.is_null() {
            continue;
        }
        match v.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "stream_event" => {
                let mut event = v.get("event").cloned().unwrap_or(Value::Null);
                let ty = event.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
                match ty.as_str() {
                    "message_start" => {
                        let mut g = lock_run(&run);
                        g.msg_tool_ids.clear();
                        g.msg_stop_reason = None;
                        g.started = true;
                        if let Some(id) = event.pointer("/message/id").and_then(|i| i.as_str()) {
                            g.streamed.push(id.to_string());
                        }
                        if let Some(m) = event.pointer("/message/model").and_then(|m| m.as_str()) {
                            g.model_echo = m.to_string();
                        }
                        g.queue.push_back(Queued::Frame(event));
                        run.state.1.notify_all();
                    }
                    "message_delta" => {
                        let mut g = lock_run(&run);
                        if let Some(sr) = event.pointer("/delta/stop_reason").and_then(|x| x.as_str()) {
                            g.msg_stop_reason = Some(sr.to_string());
                        }
                        g.queue.push_back(Queued::Frame(event));
                        run.state.1.notify_all();
                    }
                    "message_stop" => {
                        let mut g = lock_run(&run);
                        g.got_stop = true;
                        g.queue.push_back(Queued::Frame(event));
                        park_segment(&run, &mut g);
                        run.state.1.notify_all();
                    }
                    "content_block_start" => {
                        let is_tool = event.pointer("/content_block/type").and_then(|t| t.as_str()) == Some("tool_use");
                        if is_tool {
                            let id = event.pointer("/content_block/id").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let name = event.pointer("/content_block/name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            let mut g = lock_run(&run);
                            let mapped = g.names.declared_of(&name);
                            if let Some(m) = mapped {
                                if let Some(cb) = event.get_mut("content_block").and_then(|c| c.as_object_mut()) {
                                    cb.insert("name".into(), json!(m));
                                }
                            }
                            register_call(&run, &id); // 先登记后放帧（match 竞态Free 的保证）
                            g.msg_tool_ids.push(id);
                            g.queue.push_back(Queued::Frame(event));
                            run.state.1.notify_all();
                        } else {
                            let mut g = lock_run(&run);
                            g.queue.push_back(Queued::Frame(event));
                            run.state.1.notify_all();
                        }
                    }
                    "content_block_delta" => {
                        let mut g = lock_run(&run);
                        if event.pointer("/delta/type").and_then(|t| t.as_str()) == Some("text_delta") {
                            let t = event.pointer("/delta/text").and_then(|x| x.as_str()).unwrap_or("");
                            g.said_words.push_str(&strip_words(t));
                        }
                        g.queue.push_back(Queued::Frame(event));
                        run.state.1.notify_all();
                    }
                    "content_block_stop" | "ping" => {
                        let mut g = lock_run(&run);
                        g.queue.push_back(Queued::Frame(event));
                        run.state.1.notify_all();
                    }
                    _ => {} // 未知事件安静跳过
                }
            }
            // 整条 assistant 消息：流式路径下是已流过内容的重复（Go 的
            // streamed/unstreamed 语义 :4697）；没流起来（CLI 没给
            // stream_event）时由它合成帧族——回复内容不能丢。
            "assistant" => {
                let msg = v.get("message").cloned().unwrap_or(Value::Null);
                let id = msg.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
                let mut g = lock_run(&run);
                if g.started || g.streamed.iter().any(|s| *s == id) {
                    continue;
                }
                let model = msg.get("model").and_then(|m| m.as_str()).unwrap_or("").to_string();
                let model = if model.is_empty() { g.model_echo.clone() } else { model };
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
                let mut tool_ids: Vec<String> = Vec::new();
                for (i, b) in blocks.iter().enumerate() {
                    // anthropic 流式规矩：content_block_start 的块体不带正文，
                    // 正文走 delta——直接把整块塞 start 会让拼接正文的客户端
                    // （含本模块 aggregate）读到两遍
                    let mut bare = b.clone();
                    match b.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                        "text" => {
                            g.said_words.push_str(&strip_words(b.get("text").and_then(|x| x.as_str()).unwrap_or("")));
                            bare["text"] = Value::String(String::new());
                        }
                        "thinking" => {
                            bare["thinking"] = Value::String(String::new());
                        }
                        "tool_use" => {
                            let name = b.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string();
                            if let Some(m) = g.names.declared_of(&name) {
                                bare["name"] = json!(m);
                            }
                            if let Some(tid) = b.get("id").and_then(|x| x.as_str()) {
                                tool_ids.push(tid.to_string());
                            }
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
                let stop_reason = msg.get("stop_reason").cloned().unwrap_or_else(|| json!("end_turn"));
                frames.push(json!({"type": "message_delta",
                    "delta": {"stop_reason": stop_reason, "stop_sequence": null},
                    "usage": usage}));
                frames.push(json!({"type": "message_stop"}));
                // 先登记后放帧（与 stream 路径同一保证）
                for tid in &tool_ids {
                    register_call(&run, tid);
                }
                g.msg_tool_ids = tool_ids;
                g.msg_stop_reason = stop_reason.as_str().map(|s| s.to_string());
                for f in frames {
                    g.queue.push_back(Queued::Frame(f));
                }
                g.started = true;
                g.got_stop = true;
                if !id.is_empty() {
                    g.streamed.push(id);
                }
                park_segment(&run, &mut g);
                run.state.1.notify_all();
            }
            // system/init：Claude Code 的会话 id（Go readOutput :4714-4718），
            // 会话架（savedSession）的原料
            "system" => {
                if v.get("subtype").and_then(|s| s.as_str()) == Some("init") {
                    if let Some(sid) = v.get("session_id").and_then(|s| s.as_str()).filter(|s| !s.is_empty()) {
                        lock_run(&run).session_id = sid.to_string();
                    }
                }
            }
            "rate_limit_event" => {
                let info = v.get("rate_limit_info").cloned().unwrap_or(Value::Null);
                let windows = claude_limits_of(&info);
                if !windows.is_empty() {
                    note_limits(&windows);
                }
            }
            // user/tool_use_result、control_response、command_lifecycle：不关心
            "user" | "control_response" | "command_lifecycle" => {}
            "result" => {
                let is_error = v.get("is_error").and_then(|e| e.as_bool()).unwrap_or(false);
                if is_error {
                    // Go ended 的 !ok 臂（:2559）：失败回合不留驻留
                    let text = v.get("result").and_then(|r| r.as_str()).unwrap_or("Claude Code error");
                    let status = v.get("api_error_status").and_then(|s| s.as_u64()).unwrap_or(0) as u16;
                    let status = if status == 0 { 502 } else { status };
                    {
                        let mut g = lock_run(&run);
                        g.queue.push_back(Queued::Failed(status, clip(text, 500)));
                        g.segment_done = true;
                        run.state.1.notify_all();
                    }
                    abort_run(&run);
                    continue;
                }
                // 正常收口（Go ended → keepIdle，:2567-2612）。keepIdle 的池
                // 操作在放段（segment_done）**之前**完成、淘汰名单的入架在
                // 唤醒**之前**完成——读者醒来返回时，run 已可被下一回合复用、
                // 架上已写好，下一回合的 unshelve 不与入架赛跑。
                let mut victims: Vec<(String, Arc<RunCore>)> = Vec::new();
                let mut retire = false;
                {
                    let mut g = lock_run(&run);
                    if !g.segment_done {
                        // CLI 没发 message_stop 的怪流：补齐收尾帧（result 兜底）
                        if !g.got_stop {
                            g.got_stop = true;
                            g.queue.push_back(Queued::Frame(json!({"type": "message_delta",
                                "delta": {"stop_reason": "end_turn", "stop_sequence": null},
                                "usage": {}})));
                            g.queue.push_back(Queued::Frame(json!({"type": "message_stop"})));
                        }
                        let id = g.identity.take();
                        let said = std::mem::take(&mut g.said_words);
                        if let Some(id) = id {
                            if id.tools_empty && !has_reply(&id.msgs) {
                                // one-off 询问不驻留（Go :2567：无工具且对话没有
                                // assistant 回复——标题生成/分类器这类一问一答）
                                retire = true;
                            } else {
                                // 键 = 请求消息 + 刚给出的回复（Go :2577-2585——
                                // 客户端下回原样复述这条回复，词归一后同一个键）
                                let mut msgs = id.msgs.clone();
                                msgs.push(json!({ "role": "assistant", "content": [{ "type": "text", "text": said }] }));
                                let key = turn_key(&id, &msgs);
                                victims = keep_idle_locked(&run, &mut g, &key);
                            }
                        }
                        g.segment_done = true;
                    }
                }
                release_victims(victims);
                if retire {
                    abort_run(&run);
                }
                run.state.1.notify_all();
            }
            _ => {}
        }
    }
}

/// stdout EOF / 读错：段没收口按 Go replyUnended（:5054）口径收尾（吐过
/// message_stop 的干净结束、没吐的报 502），随后终局清理。**空闲中的 run
/// 死亡（看门狗到点 / 进程自己退）先入架再走**——任务口径「1h idle 杀进程
/// 前保存」：看门狗的 deadline 正是 idle 杀点，比下一次碰池的惰性清扫先到；
/// 会话文件是完整的（这一轮已经答完），下一回合 `--resume` 续上。Go 的
/// idle timer 直接 abort 不入架，这是本移植有意的加码。mid-turn 死亡
/// （idle_key 空）与已被 abort 的（不在池里）不入架——失败回合的会话不能
/// 当作对话的续点（Go abortFor 同款不出架）。
fn pump_eof(run: &Arc<RunCore>, err: Option<String>) {
    let tail = run.stderr_tail.lock().unwrap_or_else(|p| p.into_inner()).trim().to_string();
    let mut idle_key: Option<(String, String, Option<String>)> = None;
    {
        let mut g = lock_run(run);
        if !g.segment_done {
            if !g.got_stop {
                let why = match err {
                    Some(e) => e,
                    None if tail.is_empty() => "Claude Code ended before it answered".to_string(),
                    None => format!("Claude Code ended before it answered: {}", clip(&tail, 300)),
                };
                g.queue.push_back(Queued::Failed(502, why));
            }
            g.segment_done = true;
        }
        g.closed = true;
        g.pending.clear();
        if let Some(key) = g.idle_key.clone() {
            idle_key = Some((key, g.session_id.clone(), g.effort.clone()));
        }
        run.state.1.notify_all();
    }
    if let Some((key, sid, effort)) = idle_key {
        // 池成员资格复核（abort 过的已出池，不重复入架）；锁外做文件 IO
        let in_pool = {
            let p = POOL.lock().unwrap_or_else(|e| e.into_inner());
            p.idle.get(&key).is_some_and(|(r, _)| Arc::ptr_eq(r, run))
        };
        if in_pool {
            shelve_entry(&key, &sid, effort);
        }
    }
    // 池清理 + 收尸（锁外）
    *run.stdin.lock().unwrap_or_else(|p| p.into_inner()) = None;
    if let Ok(mut c) = run.child.lock() {
        let _ = c.kill();
        let _ = c.wait();
    }
    {
        let mut p = POOL.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = &run.token {
            p.runs.remove(t);
        }
        p.calls.retain(|_, r| !Arc::ptr_eq(r, run));
        p.idle.retain(|_, (r, _)| !Arc::ptr_eq(r, run));
    }
    if let Some(d) = &run.cfg_dir {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// 看门狗：deadline（回合预算 / idle 上限）到了杀进程（EOF 由泵线程收）。
fn watchdog_thread(run: Arc<RunCore>) {
    loop {
        std::thread::sleep(Duration::from_millis(500));
        let (closed, deadline) = {
            let g = lock_run(&run);
            (g.closed, g.deadline)
        };
        if closed {
            return;
        }
        if Instant::now() >= deadline {
            if let Ok(mut c) = run.child.lock() {
                let _ = c.kill();
            }
            // 泵线程收 EOF 后置 closed；再等一轮确认，杀不透就再杀
        }
    }
}

// =====================================================================
// MCP helper（Go claudebridge/mcp.go 的进程内对应物）：环回 HTTP 上的
// JSON-RPC MCP server，Claude Code 经 `--mcp-config` 的 http 型条目连入。
// =====================================================================

static HELPER_PORT: Mutex<Option<u16>> = Mutex::new(None);

/// 首次需要时拉起 helper 单例（bind 127.0.0.1:0），返回端口。失败 = None
/// （caller 带 tools 的回合降级为不带 helper 新起——由调用方决定）。
fn ensure_helper() -> Option<u16> {
    let mut g = HELPER_PORT.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(p) = *g {
        return Some(p);
    }
    let listener = TcpListener::bind("127.0.0.1:0").ok()?;
    let port = listener.local_addr().ok()?.port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(s) => {
                    std::thread::spawn(move || helper_connection(s));
                }
                Err(_) => continue,
            }
        }
    });
    *g = Some(port);
    Some(port)
}

/// 单连接单请求（Connection: close；MCP streamable HTTP 允许每请求一连接）。
fn helper_connection(mut stream: TcpStream) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(120)));
    // 头 ≤16KB、体 ≤16MB（Go readBoundedRequestBody requestLimits{body:16<<20}）
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match stream.read(&mut byte) {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                head.push(byte[0]);
                if head.len() > 16 * 1024 {
                    return;
                }
                if head.len() >= 4 && &head[head.len() - 4..] == b"\r\n\r\n" {
                    break;
                }
            }
        }
    }
    let head_text = String::from_utf8_lossy(&head);
    let mut lines = head_text.split("\r\n");
    let request_line = lines.next().unwrap_or("");
    let mut parts = request_line.split_whitespace();
    let _method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let mut content_length = 0usize;
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            if k.trim().eq_ignore_ascii_case("content-length") {
                content_length = v.trim().parse().unwrap_or(0);
            }
        }
    }
    if content_length > 16 << 20 {
        return;
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 && stream.read_exact(&mut body).is_err() {
        return;
    }
    let token = path.trim_matches('/');
    let run = {
        let p = POOL.lock().unwrap_or_else(|e| e.into_inner());
        p.runs.get(token).cloned()
    };
    let Some(run) = run else {
        let msg = json!({"error": "unknown or expired Claude run"});
        let _ = helper_write(&mut stream, 404, &msg);
        return;
    };
    let Ok(req) = serde_json::from_slice::<Value>(&body) else {
        let _ = helper_write(&mut stream, 400, &json!({"error": "invalid JSON"}));
        return;
    };
    let id = req.get("id").cloned();
    let Some(id) = id else {
        // 通知（initialized 等）不回体（MCP 规矩：无 id 无响应）
        let _ = helper_write_status(&mut stream, 202);
        return;
    };
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let answer: Result<Value, (i64, String)> = match method.as_str() {
        "initialize" => Ok(json!({
            "protocolVersion": "2025-06-18",
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": { "name": MCP_SERVER, "version": "1" }
        })),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let g = lock_run(&run);
            Ok(json!({ "tools": g.helper_tools }))
        }
        "tools/call" => helper_call(&run, &params),
        _ => Err((-32601, "method not found".into())),
    };
    let reply = match answer {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, msg)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": msg } }),
    };
    let _ = helper_write(&mut stream, 200, &reply);
}

fn helper_write(stream: &mut TcpStream, status: u16, body: &Value) -> std::io::Result<()> {
    let payload = serde_json::to_vec(body).unwrap_or_default();
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        if status == 200 { "OK" } else if status == 202 { "Accepted" } else if status == 404 { "Not Found" } else { "Bad Request" },
        payload.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(&payload)?;
    stream.flush()
}

fn helper_write_status(stream: &mut TcpStream, status: u16) -> std::io::Result<()> {
    let head = format!("HTTP/1.1 {status} Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    stream.write_all(head.as_bytes())?;
    stream.flush()
}

/// tools/call 的阻塞-回填（Go mcpCall :6479 主路径 + claudebridge/mcp.go
/// 的 tools/call）：把调用挂进 pending 等 caller 的 tool_result（经
/// continue_with 投递），直到结果 / run 结束 / 回合 deadline。
fn helper_call(run: &Arc<RunCore>, params: &Value) -> Result<Value, (i64, String)> {
    // Claude Code 在 _meta 里用**整键** "claudecode/toolUseId" 报它正在做的
    // 调用的 id（Go claudebridge/mcp.go 的 p.Meta["claudecode/toolUseId"]）——
    // 键名自带斜杠，JSON pointer 要转义（~1），别拆成嵌套路径。
    let id = params
        .get("_meta")
        .and_then(|m| m.get("claudecode/toolUseId"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let id = if id.is_empty() { format!("call_{}", hex_token()) } else { id };
    {
        let mut g = lock_run(run);
        if g.closed {
            return Err((-32000, "the agent's run ended".into()));
        }
        // tool_choice none：本回合一个工具都不许调（Go mcpCall :6539——直接
        // 答错误结果，模型改用文字回答）
        if !g.allowed_all {
            return Ok(json!({
                "content": [{ "type": "text", "text": "No tool can be used for this reply. Answer without tools." }],
                "isError": true
            }));
        }
        // 先到的结果（Go early :6529）
        if let Some(early) = g.early.remove(&id) {
            return Ok(early);
        }
        g.pending.entry(id.clone()).or_insert_with(|| PendingCall { arrived: None });
    }
    let mut g = lock_run(run);
    loop {
        if let Some(w) = g.pending.get(&id) {
            if let Some(v) = w.arrived.clone() {
                g.pending.remove(&id);
                return Ok(v);
            }
        } else if g.closed {
            return Err((-32000, "the agent's run ended".into()));
        }
        if g.closed {
            g.pending.remove(&id);
            return Err((-32000, "the agent's run ended".into()));
        }
        if Instant::now() >= g.deadline {
            g.pending.remove(&id);
            return Err((-32000, "claude bridge turn timed out".into()));
        }
        let (g2, _) = run.state.1.wait_timeout(g, Duration::from_millis(500)).unwrap_or_else(|p| p.into_inner());
        g = g2;
    }
}

// =====================================================================
// 回合编排：spawn / resume / continueWith（Go start :933 / resume :2033 /
// continueWith :6354）
// =====================================================================

/// 挂新段（Go attach :4222）：重置段状态、装回合身份、开 600s 预算。
fn attach_segment(run: &Arc<RunCore>, identity: RequestIdentity) -> SegmentReader {
    {
        let mut g = lock_run(run);
        g.queue.clear();
        g.segment_done = false;
        g.parked = false;
        g.started = false;
        g.got_stop = false;
        g.streamed.clear();
        g.said_words.clear();
        g.msg_tool_ids.clear();
        g.msg_stop_reason = None;
        g.identity = Some(identity);
        g.deadline = Instant::now() + BRIDGE_TIMEOUT;
        g.idle_key = None;
        run.state.1.notify_all();
    }
    SegmentReader { core: run.clone(), natural: false, pre: VecDeque::new(), out: Vec::new(), pos: 0 }
}

/// 给驻留进程的 stdin 写一行 user 消息（Go run.tell）。
fn tell_line(run: &Arc<RunCore>, content: &[Value]) -> Result<(), String> {
    let line = json!({ "type": "user", "message": { "role": "user", "content": content } });
    let mut bytes = serde_json::to_vec(&line).unwrap_or_default();
    bytes.push(b'\n');
    let mut g = run.stdin.lock().unwrap_or_else(|p| p.into_inner());
    match g.as_mut() {
        Some(w) => w.write_all(&bytes).and_then(|_| w.flush()).map_err(|e| e.to_string()),
        None => Err("claude bridge input is closed".into()),
    }
}

/// 新起一个驻留 run 并喂首回合（Go startRun :1004）。caller 带 tools 时
/// 挂 helper（写 mcp-config、进 runs 表）；`resume_sid` 非 None 时以
/// `--resume <id>` 起跑（Go :1193 的 from 分支，只喂增量的前提）。
#[allow(clippy::too_many_arguments)]
fn spawn_run(
    program: &Program,
    model: &str,
    effort: Option<&str>,
    prompt: &[Value],
    helper_tools: &[Value],
    names: ToolNames,
    allowed_all: bool,
    identity: RequestIdentity,
    extra_env: &[(String, String)],
    resume_sid: Option<&str>,
) -> Result<SegmentReader, (u16, String)> {
    sweep_sessions_once(); // Go :1119-1121 的起跑清扫（每进程一次）
    let mut mcp_arg: Option<String> = None;
    let mut token: Option<String> = None;
    let mut cfg_dir: Option<PathBuf> = None;
    if !helper_tools.is_empty() {
        // Go :1038-1059：tools.json + callback token 都在 run 自己的临时目录
        // 里；我们的 helper 在进程内，配置只剩 URL（token 在路径上）。
        let port = ensure_helper().ok_or_else(|| (502u16, "claude bridge helper unavailable".to_string()))?;
        let t = hex_token();
        let dir = std::env::temp_dir().join(format!("marionette-claude-{}-{t}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| (502, format!("claude bridge workdir: {e}")))?;
        let cfg = json!({ "mcpServers": { MCP_SERVER: { "type": "http", "url": format!("http://127.0.0.1:{port}/{t}") } } });
        std::fs::write(dir.join("mcp.json"), serde_json::to_vec(&cfg).unwrap_or_default())
            .map_err(|e| (502, format!("claude bridge mcp config: {e}")))?;
        mcp_arg = Some(dir.join("mcp.json").to_string_lossy().into_owned());
        token = Some(t);
        cfg_dir = Some(dir);
    }
    let mut args = bridge_args(model, effort, mcp_arg.as_deref());
    if let Some(sid) = resume_sid {
        // Go :1193：`--resume <sid>`——续命回合从这里起步
        args.push("--resume".into());
        args.push(sid.to_string());
    }
    let mut cmd = build_command(program, &args);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    // 会话落盘的目录必须与本模块 claude_sessions() 的判定同源：显式传
    // CLAUDE_CONFIG_DIR（生产 = 用户现状同值；测试缝下假 CLI 因此把会话
    // 文件写进 seam 的 home，Rust 侧的 saved() 检查才找得到）
    if let Some(dir) = claude_config_dir() {
        cmd.env("CLAUDE_CONFIG_DIR", dir);
    }
    cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| {
        if let Some(d) = &cfg_dir {
            let _ = std::fs::remove_dir_all(d);
        }
        (502, format!("claude bridge spawn failed: {e}"))
    })?;
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
    let core = Arc::new(RunCore {
        child: Arc::new(Mutex::new(child)),
        stdin: Arc::new(Mutex::new(Some(stdin))),
        stderr_tail: tail,
        token: token.clone(),
        cfg_dir,
        state: Arc::new((
            Mutex::new(RunState {
                closed: false,
                deadline: Instant::now() + BRIDGE_TIMEOUT,
                queue: VecDeque::new(),
                segment_done: false,
                parked: false,
                started: false,
                got_stop: false,
                streamed: Vec::new(),
                said_words: String::new(),
                msg_tool_ids: Vec::new(),
                msg_stop_reason: None,
                model_echo: model.to_string(),
                session_id: resume_sid.unwrap_or_default().to_string(),
                identity: None,
                effort: effort.map(|e| e.to_string()),
                idle_key: None,
                names,
                helper_tools: helper_tools.to_vec(),
                allowed_all,
                pending: HashMap::new(),
                early: HashMap::new(),
            }),
            Condvar::new(),
        )),
    });
    {
        let pump_core = core.clone();
        std::thread::spawn(move || pump_thread(pump_core, BufReader::new(stdout)));
    }
    {
        let watch_core = core.clone();
        std::thread::spawn(move || watchdog_thread(watch_core));
    }
    if let Some(t) = &token {
        POOL.lock().unwrap_or_else(|e| e.into_inner()).runs.insert(t.clone(), core.clone());
    }
    let reader = attach_segment(&core, identity);
    // 喂本回合（Go :1481 的 user 行）
    if tell_line(&core, prompt).is_err() {
        abort_run(&core);
        return Err((502, "claude bridge closed its input before the turn".into()));
    }
    Ok(reader)
}

/// Go continueWith（:6354）的移植：结果交给等待中的 tools/call（早到的存
/// early），挂新段等续答。None = 交回 intercept 落「新起」（runExpired /
/// answeredElsewhere / agent 不在等这些结果，Go :6389-6398）。
fn continue_with(run: &Arc<RunCore>, fresh: &[FreshResult], said: &str, identity: RequestIdentity) -> Option<SegmentReader> {
    {
        let g = lock_run(run);
        if g.closed {
            return None; // runExpired
        }
        if g.identity.is_some() && !g.segment_done {
            return None; // answeredElsewhere（Go claimResume :2006 的对应物）
        }
    }
    let reader = attach_segment(run, identity);
    // 用户随结果说的话先写（Go queue(said) :6413，#1878——文字不能混进
    // 工具结果；排队确认不等，见差距清单第 3 条）
    if !said.trim().is_empty() {
        if tell_line(run, &[json!({ "type": "text", "text": said })]).is_err() {
            abort_run(run);
            return None;
        }
    }
    let mut delivered = 0;
    let mut stored_early = 0;
    {
        let mut g = lock_run(run);
        for r in fresh {
            // Go mcpResult（:6449）：anthropic tool_result → MCP 结果
            let mcp = json!({
                "content": [{ "type": "text", "text": r.content }],
                "isError": r.is_error
            });
            if let Some(w) = g.pending.get_mut(&r.id) {
                if w.arrived.is_none() {
                    w.arrived = Some(mcp);
                    delivered += 1;
                }
            } else {
                g.early.insert(r.id.clone(), mcp);
                stored_early += 1;
            }
        }
        run.state.1.notify_all();
    }
    if delivered > 0 {
        // 已交付的调用撤路标（Go :6390 delete b.calls）
        let mut p = POOL.lock().unwrap_or_else(|e| e.into_inner());
        for r in fresh {
            p.calls.remove(&r.id);
        }
    }
    if delivered == 0 && stored_early == 0 {
        // Go :6398——agent 没在等这些结果：当回合失败交回新起。
        // （全 early 时不算失败：我们的 calls 登记早于 tools/call，宽限
        // 等它来取——Go 靠 b.calls 只在回调时登记所以不会走到这。）
        abort_run(run);
        return None;
    }
    Some(reader)
}

/// Go resume（:2033）的主路径：把下一回合的增量消息写给驻留 run。
/// tell 失败 → 杀 run 交回新起（Go abortFor 后 start 全量新起同款）。
fn resume_run(run: &Arc<RunCore>, identity: &RequestIdentity, split: usize) -> Option<SegmentReader> {
    let reader = attach_segment(run, identity.clone());
    let offered = {
        let g = lock_run(run);
        if g.helper_tools.is_empty() {
            None
        } else {
            Some(g.names.clone())
        }
    };
    let blocks = render_turn(&identity.msgs[split..], offered.as_ref());
    if tell_line(run, &blocks).is_err() {
        abort_run(run);
        return None;
    }
    Some(reader)
}

/// 从架上的会话续跑（Go startRun :1192-1194 的 `--resume` + :1232 的只喂
/// `since`）。**首帧偷看**当 Go :1426-1432 的 initialize 握手用：续不上的
/// Claude Code（会话加载不了直接退 / 报错 result）在第一帧前就现形——
/// 失败 = 落回全量新起（Go start :943-947），架条目在 unshelve 里已出；
/// 不删会话文件（Go discard 会删，我们保守一档交给 sweep——偷看比握手
/// 探得宽，误删真会话的代价是一整段缓存重写）。Err = 调用方落 ③。
#[allow(clippy::too_many_arguments)]
fn resume_spawn(
    program: &Program,
    model: &str,
    effort: Option<&str>,
    identity: &RequestIdentity,
    split: usize,
    tools: &[Value],
    tool_choice: &str,
    sid: &str,
) -> Result<SegmentReader, ()> {
    let (mut helper_tools, mut names, mut allowed_all) = bridge_tools(tools, tool_choice);
    if !helper_tools.is_empty() && ensure_helper().is_none() {
        helper_tools = Vec::new();
        names = ToolNames::default();
        allowed_all = true;
    }
    let offered = if helper_tools.is_empty() { None } else { Some(&names) };
    let blocks = render_turn(&identity.msgs[split..], offered);
    let mut reader =
        spawn_run(program, model, effort, &blocks, &helper_tools, names, allowed_all, identity.clone(), &[], Some(sid))
            .map_err(|_| ())?;
    // 偷看第一步：Event = 会话续上了（帧存回 reader，客户端不丢内容）；
    // Failed / 段收口 = 续不上（Go :1428-1432：加载不了会话的 Claude Code
    // 直接退，先喂回合它就一声不吭地结束）
    match reader.next_step() {
        Some(Step::Event(v)) => {
            reader.pre.push_back(v);
            Ok(reader)
        }
        _ => {
            abort_run(&reader.core);
            Err(())
        }
    }
}

// =====================================================================
// 段读者：SSE 流（Read）/ 非流聚合（aggregate）
// =====================================================================

/// 一个回合的客户端读面。流式客户端直接当 Read 用；非流式用
/// [`SegmentReader::aggregate`]。Drop 时未自然收口 = 客户端断开 →
/// 停掉 agent（Go #780：回复没送完的 caller 不会带工具结果来）。
struct SegmentReader {
    core: Arc<RunCore>,
    natural: bool,
    /// 偷看步预读出的帧（resume_spawn 的首帧探活）：先于队列供出。
    pre: VecDeque<Value>,
    /// 待写出的 SSE 字节（pos 之前已消费）——大帧按读方缓冲粒度分多次吐。
    out: Vec<u8>,
    pos: usize,
}

impl SegmentReader {
    /// 取下一条产物；None = 段收口（EOF）。
    fn next_step(&mut self) -> Option<Step> {
        if let Some(v) = self.pre.pop_front() {
            return Some(Step::Event(v));
        }
        let core = self.core.clone();
        let mut g = lock_run(&core);
        loop {
            if let Some(q) = g.queue.pop_front() {
                return match q {
                    Queued::Frame(v) => Some(Step::Event(v)),
                    Queued::Failed(s, m) => Some(Step::Failed(s, m)),
                };
            }
            if g.segment_done {
                self.natural = true;
                return None;
            }
            if g.closed {
                // 泵 EOF 路径总会先补 segment_done；这里兜异常时序
                self.natural = true;
                return None;
            }
            if Instant::now() >= g.deadline {
                // 看门狗会杀进程；读者先以超时收口（一次性——段就此关死），
                // 别让客户端挂满 600s、也别重复吐超时帧
                g.segment_done = true;
                self.natural = true;
                return Some(Step::Failed(504, "claude bridge turn exceeded its budget".into()));
            }
            let (g2, _) = core.state.1.wait_timeout(g, Duration::from_millis(500)).unwrap_or_else(|p| p.into_inner());
            g = g2;
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
            match self.next_step() {
                Some(Step::Event(v)) => {
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

impl Read for SegmentReader {
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
        loop {
            match self.next_step() {
                Some(Step::Event(v)) => {
                    self.out = sse_frame(&v).into_bytes();
                    self.pos = 0;
                    let n = serve(&mut self.out, &mut self.pos, buf);
                    return Ok(n);
                }
                Some(Step::Failed(status, msg)) => {
                    self.out = sse_frame(&json!({"type": "error",
                        "error": {"type": error_kind(status), "message": msg}}))
                    .into_bytes();
                    self.pos = 0;
                    let n = serve(&mut self.out, &mut self.pos, buf);
                    return Ok(n);
                }
                None => return Ok(0),
            }
        }
    }
}

impl Drop for SegmentReader {
    fn drop(&mut self) {
        if !self.natural {
            // 客户端断开 / 回复没送完就被丢弃（httpd 写错误丢 reader、聚合
            // 失败丢弃）——停掉 agent，别让进程白等不会来的工具结果
            abort_run(&self.core);
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
    // 上一个测试没等完的 /usage 不能把读数写进下一个测试
    let deadline = Instant::now() + Duration::from_secs(15);
    while USAGE_BUSY.load(Ordering::Acquire) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    *LIMITS_CACHE.lock().unwrap_or_else(|p| p.into_inner()) = None;
    *USAGE_STATE.lock().unwrap_or_else(|p| p.into_inner()) = None;
    // 驻留池清空 + 全部 run 收尸（测试世界结束时别让 node 假 CLI 活过夜）
    let runs: Vec<Arc<RunCore>> = {
        let mut p = POOL.lock().unwrap_or_else(|e| e.into_inner());
        let mut all: Vec<Arc<RunCore>> = p.idle.values().map(|(r, _)| r.clone()).collect();
        all.extend(p.runs.values().cloned());
        p.idle.clear();
        p.calls.clear();
        p.runs.clear();
        all
    };
    for r in runs {
        abort_run(&r);
    }
    *TEST_IDLE_LONGEST.lock().unwrap_or_else(|p| p.into_inner()) = None;
    *TEST_IDLE_MOST.lock().unwrap_or_else(|p| p.into_inner()) = None;
}

/// /usage 兜底的状态：`(at, Option<文本>)`——Some(None) = 刚失败过（退避）。
static USAGE_STATE: Mutex<Option<(Instant, Option<String>)>> = Mutex::new(None);

/// 跑 `claude -p /usage`（Go claudeUsage claude_warmup.go:198：**额度也是 CLI
/// 子进程**，从不拿 token 打 HTTP）。返回 /usage 的文本。
fn run_usage_cli() -> Option<String> {
    let program = resolved_program()?;
    let mut cmd = usage_command(&program);
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

/// 对话桥与 /usage 共用的 CLI 查找。`where.exe` 的第一行在 npm 安装上是 sh
/// 脚本（os error 193），中文路径还会被解成乱码，所以走主程序注入的查找
/// （process_util 那套：候选排序 .exe>.cmd>.ps1>无扩展名 + npm 垫片解包）。
/// 2026-11-22 注入批评审：对话桥从「只给 /usage 用」切换过来——桥的参数表
/// 含请求方可控的模型名，搭配 intercept 的字符集校验与 build_command 的
/// 不经 cmd /C 直起，三个坑（垫片选错 / 空格中文路径 / 模型名注入）一次关掉。
/// 结果缓存 30s（resolver 每次要跑 where.exe，桥是逐请求调用）。
#[cfg(not(test))]
fn resolved_program() -> Option<Program> {
    static CACHE: Mutex<Option<(Instant, Option<Program>)>> = Mutex::new(None);
    let mut cache = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, hit)) = cache.as_ref() {
        if at.elapsed() < BINARY_TTL {
            return hit.clone();
        }
    }
    let found = match super::COMMAND_RESOLVER.get() {
        Some(resolve) => {
            let wanted = std::env::var("CLAUDE_BINARY")
                .ok()
                .filter(|path| !path.is_empty())
                .unwrap_or_else(|| "claude".to_string());
            resolve(&wanted).map(|(program, prefix)| Program { program, prefix })
        }
        // dev bin 没有主程序可注入：退 where.exe 第一行的旧查找（连带其缓存）。
        None => claude_binary(),
    };
    *cache = Some((Instant::now(), found.clone()));
    found
}

#[cfg(test)]
fn resolved_program() -> Option<Program> {
    claude_binary()
}

/// 已有一次 /usage 在跑（面板连点刷新不重复起进程）。
static USAGE_BUSY: AtomicBool = AtomicBool::new(false);

/// 后台跑一次 /usage，最多等 `USAGE_WAIT`。
fn refresh_usage() {
    if !USAGE_BUSY.swap(true, Ordering::AcqRel) {
        let spawned = std::thread::Builder::new().name("claude-usage".into()).spawn(|| {
            let text = run_usage_cli();
            *USAGE_STATE.lock().unwrap_or_else(|p| p.into_inner()) = Some((Instant::now(), text));
            USAGE_BUSY.store(false, Ordering::Release);
        });
        if spawned.is_err() {
            USAGE_BUSY.store(false, Ordering::Release);
        }
    }
    let deadline = Instant::now() + USAGE_WAIT;
    while USAGE_BUSY.load(Ordering::Acquire) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
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
    let windows = match cached_limits() {
        Some(w) if !w.is_empty() => w,
        _ => {
            // /usage 兜底（带退避，成功失败都算）：**跑 CLI 在锁外、在后台**
            // （一次最多 60s）。等不到新读数时先给上一次的。
            let due = USAGE_STATE
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
                .map_or(true, |(at, _)| at.elapsed() >= USAGE_BACKOFF);
            if due {
                refresh_usage();
            }
            let text = USAGE_STATE
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .as_ref()
                .and_then(|(_, text)| text.clone())?;
            let windows = parse_usage_windows(&text, now_secs());
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
    // 订阅共享执行闸（2026-11-22）：claude 族没开 → 让位（listed_rows 也不列，
    // 直连请求与未安装无异）。真实 providers 能接住的本来就走原路径，不受影响。
    if !super::family_enabled(CLAUDE_ID) {
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
    // 凭据只判存在性——token 本身用不上（见 Account 注释）；plan 兼任
    // 会话键的 owner（Go 的账号身份，单账号世界取订阅类型足矣）。
    let account = discover()?;
    let program = resolved_program()?;

    // 模型映射：`claude/<id>` → `--model <id>` 直传（Go :7262 OwnModelID）
    let model = bare.trim();
    if model.is_empty() {
        return Some(anthropic_error(400, "claude bridge: empty model name"));
    }
    // 2026-11-22 注入批：模型名是请求方可控、最终进 Claude Code 命令行的值。
    // 只放行字母/数字开头、其余为字母数字或 `._:+-` 的名字（`:` 是档位后缀
    // `claude-x:high`）；空格、引号、`&|<>^%;,` 等 cmd 元字符一律 400。防纵深：
    // spawn 已不经 cmd /C，这里再拦一道，两道同时失效也注入不进来。
    if !model_name_safe(model) {
        return Some(anthropic_error(400, "claude bridge: invalid model name"));
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
    let wants_stream = parsed.get("stream").and_then(|s| s.as_bool()).unwrap_or(false);
    let tools: Vec<Value> = parsed.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default();
    let tool_choice = tool_choice_of(&parsed);
    let identity = RequestIdentity {
        owner: account.plan.clone(),
        model: model.to_string(),
        effort: effort.clone(),
        tool_choice: tool_choice.clone(),
        system: system_text(parsed.get("system").unwrap_or(&Value::Null)),
        tools_json: serde_json::to_string(&tools).unwrap_or_default(),
        tools_empty: tools.is_empty(),
        msgs: parsed.get("messages").and_then(|m| m.as_array()).cloned().unwrap_or_default(),
    };

    // ① 工具结果回填（Go serveClaudeSubscription 的 match → continueWith，
    // :7454-7528）：尾部的 tool_result 命中驻留 run 就喂回去拿续答；
    // 不命中/不可用 → 落到新起（Go runExpired/answeredElsewhere 同款）。
    if let Some((run, fresh)) = match_run(&identity.msgs) {
        let said = said_with_results(&identity.msgs);
        if let Some(reader) = continue_with(&run, &fresh, &said, identity.clone()) {
            return Some(reply_of(reader, wants_stream, model));
        }
    }
    // ② 驻留复用（Go resume :2033）：同会话的下一回合只喂增量消息。
    let mut cold: Option<(String, usize)> = None;
    if let Some((key, split)) = next_turn(&identity) {
        if let Some(run) = take_idle_run(&key, effort.as_deref()) {
            if let Some(reader) = resume_run(&run, &identity, split) {
                return Some(reply_of(reader, wants_stream, model));
            }
        }
        cold = Some((key, split));
    }
    // ②' 冷启动续命（Go unshelve :2820 + start :933 的 from 分支）：驻留池
    // 不中，但架上有这段对话的会话（曾被淘汰的 run 落盘的 session id）→
    // `--resume` 续跑，只喂增量；续不上（会话文件没了 / CLI 拒续）→ 落回
    // ③ 全量新起（Go start :943-947 同款回退）。
    if let Some((key, split)) = cold {
        if let Some(sid) = unshelve(&key, effort.as_deref()) {
            if let Ok(reader) = resume_spawn(&program, model, effort.as_deref(), &identity, split, &tools, &tool_choice, &sid) {
                return Some(reply_of(reader, wants_stream, model));
            }
        }
    }
    // ③ 新起（Go start :933 / startRun :1004）：全量 prompt；带 tools 时挂
    // helper。helper 起不来（环回 bind 失败）→ 不带 helper 硬跑（caller 的
    // 工具宣告不出去，对话仍可用——Go 没有的降级，见模块头偏差记录）。
    let (mut helper_tools, mut names, mut allowed_all) = bridge_tools(&tools, &tool_choice);
    if !helper_tools.is_empty() && ensure_helper().is_none() {
        helper_tools = Vec::new();
        names = ToolNames::default();
        allowed_all = true;
    }
    let offered = if helper_tools.is_empty() { None } else { Some(&names) };
    let prompt = render_prompt(&parsed, offered);
    match spawn_run(&program, model, effort.as_deref(), &prompt, &helper_tools, names, allowed_all, identity, &[], None) {
        Ok(reader) => Some(reply_of(reader, wants_stream, model)),
        Err((status, msg)) => Some(anthropic_error(status, &msg)),
    }
}

/// 段读者 → Reply（流式透传 / 非流聚合，与旧路径同形）。
fn reply_of(reader: SegmentReader, wants_stream: bool, model: &str) -> Reply {
    if wants_stream {
        Reply::Stream {
            status: 200,
            content_type: "text/event-stream".into(),
            reader: Box::new(reader),
        }
    } else {
        match reader.aggregate(model) {
            Ok(v) => Reply::json(200, &v),
            Err((status, msg)) => anthropic_error(status, &msg),
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
        std::fs::create_dir_all(dir.join("shelf")).unwrap();
        std::fs::write(dir.join("magpie").join("providers.json"), r#"{"providers":[]}"#).unwrap();
        *TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()) = Some(Seam {
            home: dir.join("home"),
            logins: if with_logins { Some(dir.join("logins")) } else { None },
            shelf: Some(dir.join("shelf")),
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
    /// MARIONETTE_FAKE_CLAUDE_MODE 环境变量：chat（默认，**每收到一行
    /// stdin 答一整轮**——驻留语义）/ error / noresult / usage / whole /
    /// toolround（走 --mcp-config 里的 URL 真打 MCP：initialize →
    /// tools/list → tools/call 阻塞等结果后续答））。dump 文件记录首回合
    /// argv/stdin/env 供断言；MARIONETTE_FAKE_SPAWNS / _STDIN_LOG / _MCP_LOG
    /// 各是 JSONL 流水。
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
const http = require("http");
const mode = process.env.MARIONETTE_FAKE_CLAUDE_MODE || "chat";
const dump = process.env.MARIONETTE_FAKE_CLAUDE_DUMP;
const spawns = process.env.MARIONETTE_FAKE_SPAWNS;
const stdinLog = process.env.MARIONETTE_FAKE_STDIN_LOG;
const mcpLog = process.env.MARIONETTE_FAKE_MCP_LOG;
const argv = process.argv.slice(2);
const resumeSid = (() => { const i = argv.indexOf("--resume"); return i >= 0 && i + 1 < argv.length ? argv[i + 1] : null; })();
const SID = resumeSid || "sess-fake";
if (spawns) { try { fs.appendFileSync(spawns, JSON.stringify({ pid: process.pid, argv }) + "\n"); } catch {} }
// 会话落盘（Claude Code 自己写会话文件的对应物，Go claudeSessions :3448 的
// 形状）：$CLAUDE_CONFIG_DIR/projects/<工作目录编码>/<sid>.jsonl——Rust 侧
// saved() 检查的就是这个文件
try {
  const cc = process.env.CLAUDE_CONFIG_DIR;
  if (cc) {
    const path = require("path");
    const dir = path.join(cc, "projects", process.cwd().replace(/[^A-Za-z0-9]/g, "-"));
    fs.mkdirSync(dir, { recursive: true });
    fs.writeFileSync(path.join(dir, SID + ".jsonl"), JSON.stringify({ type: "session", sessionId: SID }) + "\n");
  }
} catch {}
const w = (o) => process.stdout.write(JSON.stringify(o) + "\n");
const modelOf = () => argv[argv.indexOf("--model") + 1] || "claude-sonnet-5";
const effortOn = () => argv.indexOf("--effort") >= 0;
function chatTurn(n) {
  const model = modelOf();
  w({ type: "system", subtype: "init", session_id: SID });
  w({ type: "stream_event", event: { type: "message_start", message: { id: "msg_f" + n, type: "message", role: "assistant", model, content: [], usage: { input_tokens: 10, output_tokens: 1 } } } });
  if (effortOn()) w({ type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "thinking", thinking: "" } } });
  const textIndex = effortOn() ? 1 : 0;
  if (effortOn()) w({ type: "stream_event", event: { type: "content_block_stop", index: 0 } });
  w({ type: "stream_event", event: { type: "content_block_start", index: textIndex, content_block: { type: "text", text: "" } } });
  w({ type: "stream_event", event: { type: "content_block_delta", index: textIndex, delta: { type: "text_delta", text: "你好，世界" } } });
  w({ type: "stream_event", event: { type: "content_block_stop", index: textIndex } });
  w({ type: "stream_event", event: { type: "message_delta", delta: { stop_reason: "end_turn", stop_sequence: null }, usage: { input_tokens: 10, output_tokens: 2 } } });
  w({ type: "stream_event", event: { type: "message_stop" } });
  if (n === 1) w({ type: "rate_limit_event", rate_limit_info: { status: "ok", rateLimitType: "five_hour",
    unifiedWindows: { five_hour: { utilization: 0.42, resetsAt: 4102444800 }, seven_day: { utilization: 0.07, resetsAt: 4102963200 } } } });
  w({ type: "result", subtype: "success", is_error: false, result: "done" });
}
function mcpUrl() {
  const i = argv.indexOf("--mcp-config");
  if (i < 0 || i + 1 >= argv.length) return null;
  try {
    const cfg = JSON.parse(fs.readFileSync(argv[i + 1], "utf8"));
    return cfg.mcpServers.marionette.url;
  } catch { return null; }
}
function rpc(url, obj, cb) {
  if (mcpLog) { try { fs.appendFileSync(mcpLog, JSON.stringify({ tag: "req", method: obj.method, url }) + "\n"); } catch {} }
  const u = new URL(url);
  const body = JSON.stringify(obj);
  const req = http.request({ hostname: u.hostname, port: u.port, path: u.pathname, method: "POST", headers: { "Content-Type": "application/json" } }, (res) => {
    let b = ""; res.on("data", (d) => (b += d)); res.on("end", () => {
      if (mcpLog) { try { fs.appendFileSync(mcpLog, JSON.stringify({ method: obj.method, status: res.statusCode, body: b }) + "\n"); } catch {} }
      cb(res.statusCode, b);
    });
  });
  req.on("error", (e) => {
    if (mcpLog) { try { fs.appendFileSync(mcpLog, JSON.stringify({ tag: "err", method: obj.method, error: String(e && e.message || e) }) + "\n"); } catch {} }
    cb(0, "");
  });
  req.end(body);
}
function toolTurn(n) {
  if (n !== 1) { chatTurn(n); return; }
  const model = modelOf();
  w({ type: "system", subtype: "init", session_id: "sess-tool" });
  w({ type: "stream_event", event: { type: "message_start", message: { id: "msg_t1", type: "message", role: "assistant", model, content: [], usage: { input_tokens: 8, output_tokens: 1 } } } });
  w({ type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "tool_use", id: "toolu_01", name: "mcp__marionette__get_weather", input: {} } } });
  w({ type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "input_json_delta", partial_json: "{\"city\":\"北京\"}" } } });
  w({ type: "stream_event", event: { type: "content_block_stop", index: 0 } });
  w({ type: "stream_event", event: { type: "message_delta", delta: { stop_reason: "tool_use", stop_sequence: null }, usage: { input_tokens: 8, output_tokens: 1 } } });
  w({ type: "stream_event", event: { type: "message_stop" } });
  const url = mcpUrl();
  if (mcpLog) { try { fs.appendFileSync(mcpLog, JSON.stringify({ tag: "url", url: url || null, cfg: argv.indexOf("--mcp-config") >= 0 ? argv[argv.indexOf("--mcp-config") + 1] : null }) + "\n"); } catch {} }
  if (!url) { w({ type: "result", subtype: "success", is_error: false, result: "no mcp config" }); return; }
  rpc(url, { jsonrpc: "2.0", id: 1, method: "initialize", params: {} }, () => {
    rpc(url, { jsonrpc: "2.0", id: 2, method: "tools/list" }, () => {
      rpc(url, { jsonrpc: "2.0", id: 3, method: "tools/call", params: { name: "get_weather", arguments: { city: "北京" }, _meta: { "claudecode/toolUseId": "toolu_01" } } }, (s, b) => {
        let text = "";
        try { text = JSON.parse(b).result.content[0].text; } catch {}
        w({ type: "stream_event", event: { type: "message_start", message: { id: "msg_t2", type: "message", role: "assistant", model: modelOf(), content: [], usage: { input_tokens: 9, output_tokens: 2 } } } });
        w({ type: "stream_event", event: { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } } });
        w({ type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "天气结论：" + text } } });
        w({ type: "stream_event", event: { type: "content_block_stop", index: 0 } });
        w({ type: "stream_event", event: { type: "message_delta", delta: { stop_reason: "end_turn", stop_sequence: null }, usage: { input_tokens: 9, output_tokens: 3 } } });
        w({ type: "stream_event", event: { type: "message_stop" } });
        w({ type: "result", subtype: "success", is_error: false, result: "done" });
      });
    });
  });
}
let stdinText = "";
let dumped = false;
let turn = 0;
process.stdin.setEncoding("utf8");
process.stdin.on("data", (d) => {
  stdinText += d;
  let i;
  while ((i = stdinText.indexOf("\n")) >= 0) {
    const line = stdinText.slice(0, i);
    stdinText = stdinText.slice(i + 1);
    if (!dumped && dump) {
      dumped = true;
      try { fs.writeFileSync(dump, JSON.stringify({ argv, stdin: line + "\n", baseUrl: process.env.ANTHROPIC_BASE_URL || null, cwd: process.cwd() })); } catch {}
    }
    if (stdinLog) { try { fs.appendFileSync(stdinLog, JSON.stringify({ line }) + "\n"); } catch {} }
    if (mode === "chat" || mode === "toolround" || mode === "resumeerror") {
      turn += 1;
      if (mode === "toolround") toolTurn(turn);
      else if (mode === "resumeerror" && resumeSid) {
        // --resume 的会话续不上：一声不吭地退（真实 CLI 对不存在的会话
        // 就是退出，Go :1428-1432 的口径）——Rust 侧的偷看步当场现形
        process.exit(3);
      }
      else chatTurn(turn);
    }
  }
});
process.stdin.on("end", () => process.exit(0));
process.stdin.resume();
// 即答模式（error / noresult / whole / usage）：启动即写，不等 stdin（老行为）
if (mode === "usage") {
  process.stdout.write(JSON.stringify([
    { type: "system", subtype: "init", session_id: "s-usage" },
    { type: "result", subtype: "success", is_error: false,
      result: "Current session: 13% used · resets " + (process.env.MARIONETTE_FAKE_RESET || "Oct 1 at 3:30pm (Asia/Shanghai)") + "\nCurrent week (all models): 4% used\n" }
  ]) + "\n");
} else if (mode === "error") {
  w({ type: "result", subtype: "error_overloaded", is_error: true, api_error_status: 429, result: "rate limited: too many requests" });
} else if (mode === "noresult") {
  const model = modelOf();
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
        for flag in ["-p", "--output-format stream-json", "--input-format stream-json", "--include-partial-messages", "--verbose", "--model claude-sonnet-5", "--tools ", "--strict-mcp-config", "--dangerously-skip-permissions"] {
            assert!(joined.contains(flag), "argv missing `{flag}`: {joined}");
        }
        assert!(!joined.contains("--mcp-config"), "no MCP helper in the core path");
        // 会话落盘开着（Go :1116 摘掉 --no-session-persistence）——续命的前提
        assert!(!joined.contains("--no-session-persistence"), "sessions persist for the shelf: {joined}");
        assert!(!joined.contains("--resume"), "a fresh run never resumes: {joined}");
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
        let blocks = render_prompt(&body, None);
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
        let blocks = render_prompt(&body, None);
        let text = blocks[0].get("text").and_then(|t| t.as_str()).unwrap();
        assert!(!text.contains("<conversation_history>"));
        assert!(!text.contains("<current_turn>"));
        assert!(text.contains("[tool result id=t1]\n结果"));
    }

    #[test]
    fn single_turn_is_not_wrapped() {
        let body = json!({ "model": "m", "messages": [ { "role": "user", "content": "hi" } ] });
        let blocks = render_prompt(&body, None);
        let text = blocks[0].get("text").and_then(|t| t.as_str()).unwrap();
        assert!(text.contains("Human: hi"));
        assert!(!text.contains("<conversation_history>"));
    }

    #[test]
    fn thinking_is_not_replayed_and_images_keep_their_anthropic_shape() {
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
        let blocks = render_prompt(&body, None);
        let text = blocks
            .iter()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
        assert!(!text.contains("secret reasoning"), "thinking is never told as words");
        assert!(!text.contains("sig=="));
        assert!(text.contains("Assistant: answer"));
        assert!(text.contains("Human: "));
        assert!(text.contains("这是什么"));
        let image = blocks
            .iter()
            .find(|b| b.get("type").and_then(Value::as_str) == Some("image"))
            .expect("the image must be sent as an image block, not omitted text");
        assert_eq!(image.pointer("/source/type").and_then(Value::as_str), Some("base64"));
        assert_eq!(image.pointer("/source/media_type").and_then(Value::as_str), Some("image/png"));
        assert_eq!(image.pointer("/source/data").and_then(Value::as_str), Some("AAAA"));
    }

    #[test]
    fn url_images_and_tool_result_images_are_forwarded() {
        let body = json!({
            "model": "m",
            "messages": [
                { "role": "user", "content": [
                    { "type": "image", "source": { "type": "url", "url": "https://example.test/a.png" } },
                    { "type": "text", "text": "look" }
                ]},
                { "role": "assistant", "content": [
                    { "type": "tool_use", "id": "tool-1", "name": "screenshot", "input": {} }
                ]},
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "tool-1", "content": [
                        { "type": "text", "text": "capture" },
                        { "type": "image", "source": { "type": "base64", "media_type": "image/jpeg", "data": "BBBB" } }
                    ]}
                ]}
            ]
        });
        let blocks = render_prompt(&body, None);
        let images = blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("image")).collect::<Vec<_>>();
        assert_eq!(images.len(), 2, "both user and tool images must survive");
        assert_eq!(images[0].pointer("/source/type").and_then(Value::as_str), Some("url"));
        assert_eq!(images[1].pointer("/source/type").and_then(Value::as_str), Some("base64"));
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
        *TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()) = Some(Seam { home: dir.join("home"), logins: None, shelf: Some(dir.join("shelf")), cli: None });
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

    /// 2026-11-22 订阅共享执行闸：claude 族未推送进开启名单 → intercept 让位
    /// （直连请求与未安装无异）、listed_rows 为空；推送进名单 → 照常桥接。
    /// Drop 复位闸（其它测试依赖「未推送 = 全开」默认；world() 已取共享锁）。
    #[test]
    fn subscription_gate_blocks_claude_family_when_not_enabled() {
        struct GateReset;
        impl Drop for GateReset {
            fn drop(&mut self) {
                super::super::set_subscription_enabled_for_tests(None);
            }
        }
        let _gate = GateReset;
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        let request = post(&messages_body("claude/claude-sonnet-5", false));
        super::super::set_subscription_enabled_for_tests(Some(Vec::new()));
        assert!(intercept(&gateway, &request, Proto::Anthropic).is_none(), "gated family must yield");
        assert!(listed_rows(&[]).is_empty(), "gated family must not list");
        super::super::set_subscription_enabled_for_tests(Some(vec!["claude".to_string()]));
        let reply = intercept(&gateway, &request, Proto::Anthropic).expect("intercepted when enabled");
        assert!(matches!(reply, Reply::Json { status: 200, .. }));
        assert!(!listed_rows(&[]).is_empty(), "enabled family lists its rows");
    }

    #[test]
    fn dangerous_model_names_are_rejected_before_spawn() {
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        // 夹带命令 / 空格 / 引号 / cmd 元字符 / 旗标伪装：一律 400，CLI 不被拉起。
        for name in ["x&calc", "a b", "a\"b", "&whoami", "x|y", "a;b", "-flag", "%TEMP%", "a^b", "x>y"] {
            let reply = intercept(&gateway, &post(&messages_body(&format!("claude/{name}"), false)), Proto::Anthropic)
                .unwrap_or_else(|| panic!("{name} should be intercepted"));
            match reply {
                Reply::Json { status, body } => {
                    assert_eq!(status, 400, "{name}");
                    let text = String::from_utf8_lossy(&body).into_owned();
                    assert!(text.contains("invalid model name"), "{name}: {text}");
                }
                Reply::Stream { .. } => panic!("{name}: expected a 400 json error, got a stream"),
            }
        }
        // 合法名字（含档位后缀形态）不受影响：正常 200 桥接。
        for name in ["claude-sonnet-5", "sonnet", "claude-opus-5:high"] {
            let reply = intercept(&gateway, &post(&messages_body(&format!("claude/{name}"), false)), Proto::Anthropic)
                .unwrap_or_else(|| panic!("{name} should be intercepted"));
            assert!(matches!(reply, Reply::Json { status: 200, .. }), "{name}");
        }
    }

    #[test]
    fn build_command_never_rewraps_through_cmd() {
        // 2026-11-22 注入批：无论解析结果是什么形态，都不允许再经
        // `cmd.exe /C` 重拼接（那是空格路径打碎与模型名注入两个坑的来源）。
        let args = bridge_args("claude-opus-5", None, None);
        let cmd = build_command(
            &Program { program: r"C:\John Smith 张三\claude.cmd".to_string(), prefix: Vec::new() },
            &args,
        );
        let rendered = format!("{cmd:?}");
        // Debug 渲染会把 `\` 转义成 `\\`，断言用不含反斜杠的子串。
        assert!(rendered.contains("claude.cmd") && rendered.contains("张三"), "{rendered}");
        assert!(!rendered.contains("\"cmd.exe\""), "{rendered}");
        // node+cli.js 前缀形态照旧支持。
        let cmd = build_command(
            &Program { program: "node.exe".to_string(), prefix: vec![r"C:\cli.js".to_string()] },
            &args,
        );
        let rendered = format!("{cmd:?}");
        assert!(rendered.contains("node.exe") && rendered.contains("cli.js"), "{rendered}");
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

    #[cfg(target_os = "windows")]
    #[test]
    fn usage_survives_a_script_in_a_spaced_chinese_path() {
        let (_gateway, w) = world(None, false);
        write_credentials(&w, "pro");
        let dir = w.dir.join("John Smith 张三");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("out.json"),
            r#"[{"type":"result","is_error":false,"result":"Current session: 27% used\nCurrent week (all models): 3% used"}]"#,
        )
        .unwrap();
        let script = dir.join("claude.cmd");
        std::fs::write(&script, "@type \"%~dp0out.json\"\r\n").unwrap();
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli =
            Some(Program { program: script.to_string_lossy().into_owned(), prefix: Vec::new() });

        let row = quota_row(&[]).expect("quota row from a spaced, non-ASCII path");
        assert_eq!(row["windows"][0]["used"], 27.0);
        assert_eq!(row["windows"][1]["used"], 3.0);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn slow_usage_does_not_hold_the_panel_and_lands_later() {
        let (_gateway, w) = world(None, false);
        write_credentials(&w, "pro");
        let script = w.dir.join("slow.js");
        std::fs::write(
            &script,
            r#"setTimeout(() => console.log(JSON.stringify([{type:"result",is_error:false,result:"Current session: 42% used"}])), 4500);"#,
        )
        .unwrap();
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(Program {
            program: find_node().expect("node on PATH"),
            prefix: vec![script.to_string_lossy().into_owned()],
        });

        let started = Instant::now();
        assert!(quota_row(&[]).is_none(), "还没读到");
        assert!(started.elapsed() < Duration::from_secs(4), "不能超过壳侧 4s 的超时");

        let deadline = Instant::now() + Duration::from_secs(20);
        while USAGE_BUSY.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(quota_row(&[]).expect("后台读数已落缓存")["windows"][0]["used"], 42.0);
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

    // ---------------------------------------------------------------
    // 跨回合驻留（差距第 2 条：Go keepIdle :2615 / resume :2033）
    // ---------------------------------------------------------------

    fn jsonl_lines(path: &std::path::Path) -> Vec<Value> {
        let b = std::fs::read_to_string(path).unwrap_or_default();
        b.lines().filter(|l| !l.trim().is_empty()).filter_map(|l| serde_json::from_str(l).ok()).collect()
    }

    fn spawn_count(world: &World) -> usize {
        jsonl_lines(&world.dir.join("spawns.jsonl")).len()
    }

    #[test]
    fn resident_run_is_reused_for_next_turn() {
        // 同会话的续回合：不起新进程、stdin 只喂增量（Go resume :2033 /
        // renderClaudeTurn :5700——客户端复述的回复经词归一后与驻留键相认）。
        // 注意 Go 的 one-off 臂（ended :2567）：**无工具对话的第一回合**请求里
        // 没有任何 assistant 消息，不驻留——第二回合才起「会被留下」的 run，
        // 第三回合验证复用。
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        std::env::set_var("MARIONETTE_FAKE_SPAWNS", w.dir.join("spawns.jsonl"));
        std::env::set_var("MARIONETTE_FAKE_STDIN_LOG", w.dir.join("stdinlog.jsonl"));
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        let turn = |msgs: Value| {
            let mut body = messages_body("claude/claude-sonnet-5", false);
            body["messages"] = msgs;
            intercept(&gateway, &post(&body), Proto::Anthropic).expect("intercepted")
        };
        // 第一回合（one-off：不驻留）
        assert!(matches!(turn(json!([{ "role": "user", "content": "你好" }])), Reply::Json { status: 200, .. }));
        // 第二回合（起 run，收口后驻留）
        assert!(matches!(turn(json!([
            { "role": "user", "content": "你好" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "第二问" }
        ])), Reply::Json { status: 200, .. }));
        // 第三回合：复用驻留 run
        let reply = turn(json!([
            { "role": "user", "content": "你好" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "第二问" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "继续说点别的" }
        ]));
        match reply {
            Reply::Json { status, body } => {
                assert_eq!(status, 200);
                let v: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(v.pointer("/content/0/text").and_then(|t| t.as_str()), Some("你好，世界"));
            }
            Reply::Stream { status, .. } => panic!("expected json, got stream {status}"),
        }
        // 第一回合 + 第二回合各起一个进程，第三回合复用第二个（Go keepIdle
        // 的意义所在）
        assert_eq!(spawn_count(&w), 2, "resident run must be reused");
        // 复用回合只喂增量：没有 system 包裹、没有历史回放，只有新问题
        let stdin = jsonl_lines(&w.dir.join("stdinlog.jsonl"));
        assert_eq!(stdin.len(), 3);
        let line3 = stdin[2].get("line").and_then(|l| l.as_str()).unwrap();
        assert!(line3.contains("继续说点别的"), "turn fed to resident run: {line3}");
        assert!(!line3.contains("external_system_instructions"), "no system re-send: {line3}");
        assert!(!line3.contains("conversation_history"), "no whole-conversation replay: {line3}");
        std::env::remove_var("MARIONETTE_FAKE_SPAWNS");
        std::env::remove_var("MARIONETTE_FAKE_STDIN_LOG");
    }

    #[test]
    fn resident_run_expires_after_idle_longest() {
        // idle 过期（Go idleLongest，测试缝压到 300ms）：驻留过的 run 过期
        // 出池——**入架续命**（差距第 1 条）：下一回合 `--resume` 续跑，只
        // 喂增量，不再全量重写缓存。
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        std::env::set_var("MARIONETTE_FAKE_SPAWNS", w.dir.join("spawns.jsonl"));
        std::env::set_var("MARIONETTE_FAKE_STDIN_LOG", w.dir.join("stdinlog.jsonl"));
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        *TEST_IDLE_LONGEST.lock().unwrap_or_else(|p| p.into_inner()) = Some(Duration::from_millis(300));
        let turn = |msgs: Value| {
            let mut body = messages_body("claude/claude-sonnet-5", false);
            body["messages"] = msgs;
            intercept(&gateway, &post(&body), Proto::Anthropic).expect("intercepted")
        };
        // 两回合把 run 驻留进池（第一回合 one-off 不驻留）
        assert!(matches!(turn(json!([{ "role": "user", "content": "你好" }])), Reply::Json { status: 200, .. }));
        assert!(matches!(turn(json!([
            { "role": "user", "content": "你好" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "第二问" }
        ])), Reply::Json { status: 200, .. }));
        let spawns_before = spawn_count(&w);
        std::thread::sleep(Duration::from_millis(500)); // 过期 + 看门狗收尸
        let reply = turn(json!([
            { "role": "user", "content": "你好" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "第二问" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "过期之后" }
        ]));
        assert!(matches!(reply, Reply::Json { status: 200, .. }));
        assert_eq!(spawn_count(&w), spawns_before + 1, "expired idle run gives way to one resume spawn");
        // 过期的 run 入了架：这次起跑带 --resume（架条目已消费）
        let spawns = jsonl_lines(&w.dir.join("spawns.jsonl"));
        let argv = spawns.last().unwrap().get("argv").and_then(|a| a.as_array()).unwrap();
        let joined = argv.iter().map(|v| v.as_str().unwrap_or("")).collect::<Vec<_>>().join(" ");
        assert!(joined.contains("--resume sess-fake"), "resumed from the shelved session: {joined}");
        // 只喂增量：没有 system 包裹、没有历史回放，只有新问题
        let stdin = jsonl_lines(&w.dir.join("stdinlog.jsonl"));
        let last = stdin.last().unwrap().get("line").and_then(|l| l.as_str()).unwrap();
        assert!(last.contains("过期之后"), "increment fed to the resumed run: {last}");
        assert!(!last.contains("conversation_history"), "no whole-conversation replay after resume: {last}");
        assert!(!last.contains("external_system_instructions"));
        assert!(shelf_read().is_empty(), "shelf entry consumed by the resume");
        std::env::remove_var("MARIONETTE_FAKE_SPAWNS");
        std::env::remove_var("MARIONETTE_FAKE_STDIN_LOG");
    }

    #[test]
    fn idle_capacity_evicts_the_oldest() {
        // 容量淘汰（Go idleMost :2656，测试缝压到 1）：第二个驻留会话把第一
        // 个挤出池——被挤的入架（淘汰即保存），它的续回合 `--resume` 续跑。
        // 最久先出的顺序不变（Go :2660 的 min_by_key 没动）。
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        std::env::set_var("MARIONETTE_FAKE_SPAWNS", w.dir.join("spawns.jsonl"));
        std::env::set_var("MARIONETTE_FAKE_STDIN_LOG", w.dir.join("stdinlog.jsonl"));
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        *TEST_IDLE_MOST.lock().unwrap_or_else(|p| p.into_inner()) = Some(1);
        let turn = |msgs: Value| intercept(&gateway, &post(&json!({
            "model": "claude/claude-sonnet-5", "max_tokens": 1024, "stream": false,
            "system": "You are a helpful assistant.", "messages": msgs,
        })), Proto::Anthropic).expect("intercepted");
        // 会话 A 两回合（第二回合起 run 并驻留）
        assert!(matches!(turn(json!([{ "role": "user", "content": "会话A第一轮" }])), Reply::Json { status: 200, .. }));
        assert!(matches!(turn(json!([
            { "role": "user", "content": "会话A第一轮" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "会话A第二轮" }
        ])), Reply::Json { status: 200, .. }));
        let after_a = spawn_count(&w);
        // 会话 B 两回合：B 的驻留把 A 挤出池（容量 1），A 的 run 入架
        assert!(matches!(turn(json!([{ "role": "user", "content": "会话B第一轮" }])), Reply::Json { status: 200, .. }));
        assert!(matches!(turn(json!([
            { "role": "user", "content": "会话B第一轮" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "会话B第二轮" }
        ])), Reply::Json { status: 200, .. }));
        // A 的续回合：A 已被挤出池但架上有它的会话 → --resume 续跑
        let reply = turn(json!([
            { "role": "user", "content": "会话A第一轮" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "会话A第二轮" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "会话A第三轮" }
        ]));
        assert!(matches!(reply, Reply::Json { status: 200, .. }));
        assert_eq!(spawn_count(&w), after_a + 3, "B1(one-off) + B2(kept, evicts A) + A3 resumed");
        let spawns = jsonl_lines(&w.dir.join("spawns.jsonl"));
        let argv = spawns.last().unwrap().get("argv").and_then(|a| a.as_array()).unwrap();
        let joined = argv.iter().map(|v| v.as_str().unwrap_or("")).collect::<Vec<_>>().join(" ");
        assert!(joined.contains("--resume sess-fake"), "the evicted session resumes: {joined}");
        let stdin = jsonl_lines(&w.dir.join("stdinlog.jsonl"));
        let last = stdin.last().unwrap().get("line").and_then(|l| l.as_str()).unwrap();
        assert!(last.contains("会话A第三轮"), "only the new turn is fed: {last}");
        assert!(!last.contains("conversation_history"), "no whole-conversation replay: {last}");
        std::env::remove_var("MARIONETTE_FAKE_SPAWNS");
        std::env::remove_var("MARIONETTE_FAKE_STDIN_LOG");
    }

    #[test]
    fn one_off_ask_is_not_kept_idle() {
        // Go ended 的 one-off 臂（:2567）：无工具且对话没有任何 assistant
        // 回复的一问一答（标题/分类器）不留驻留进程
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        std::env::set_var("MARIONETTE_FAKE_SPAWNS", w.dir.join("spawns.jsonl"));
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        let reply = intercept(&gateway, &post(&messages_body("claude/claude-sonnet-5", false)), Proto::Anthropic).expect("answered");
        assert!(matches!(reply, Reply::Json { status: 200, .. }));
        std::thread::sleep(Duration::from_millis(200));
        let p = POOL.lock().unwrap_or_else(|e| e.into_inner());
        assert!(p.idle.is_empty(), "one-off ask must not be kept idle");
        drop(p);
        std::env::remove_var("MARIONETTE_FAKE_SPAWNS");
    }

    // ---------------------------------------------------------------
    // 会话架 / --resume 续命（差距第 1 条：Go shelve :2753 / unshelve :2820 /
    // start 的 from 分支 :933）
    // ---------------------------------------------------------------

    /// 架文件路径（测试世界里的）。
    fn shelf_file(world: &World) -> PathBuf {
        world.dir.join("shelf").join("claude-shelf.json")
    }

    fn write_shelf_entry(_world: &World, key: &str, entry: ShelfEntry) {
        let _g = SHELF_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let mut entries = shelf_read();
        entries.insert(key.to_string(), entry);
        shelf_write(&entries);
    }

    #[test]
    fn evicted_session_resumes_from_the_shelf_and_stays_resident() {
        // 完整链（任务验收三条）：B2 的驻留把 A2 挤出池 → A2 入架（淘汰即
        // 保存）→ A3 池不中、架命中 → `--resume` 续跑、stdin 只含增量 →
        // A3 收口进池（把 B2 挤出并入架）→ A4 驻留复用，不再起进程。
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        std::env::set_var("MARIONETTE_FAKE_SPAWNS", w.dir.join("spawns.jsonl"));
        std::env::set_var("MARIONETTE_FAKE_STDIN_LOG", w.dir.join("stdinlog.jsonl"));
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        *TEST_IDLE_MOST.lock().unwrap_or_else(|p| p.into_inner()) = Some(1);
        let turn = |msgs: Value| intercept(&gateway, &post(&json!({
            "model": "claude/claude-sonnet-5", "max_tokens": 1024, "stream": false,
            "system": "You are a helpful assistant.", "messages": msgs,
        })), Proto::Anthropic).expect("intercepted");
        // A1（one-off）+ A2（起 run 并驻留）
        assert!(matches!(turn(json!([{ "role": "user", "content": "会话A第一轮" }])), Reply::Json { status: 200, .. }));
        assert!(matches!(turn(json!([
            { "role": "user", "content": "会话A第一轮" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "会话A第二轮" }
        ])), Reply::Json { status: 200, .. }));
        // B1 + B2：B2 的驻留把 A2 挤出池，A2 的会话入架（假 CLI 已落盘
        // sess-fake.jsonl，saved() 才肯收）
        assert!(matches!(turn(json!([{ "role": "user", "content": "会话B第一轮" }])), Reply::Json { status: 200, .. }));
        assert!(matches!(turn(json!([
            { "role": "user", "content": "会话B第一轮" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "会话B第二轮" }
        ])), Reply::Json { status: 200, .. }));
        let shelf: Value = serde_json::from_slice(&std::fs::read(shelf_file(&w)).unwrap()).unwrap();
        assert_eq!(shelf.as_object().unwrap().len(), 1, "the evicted run's session is shelved");
        // A3：池不中、架命中 → resume（1 个新进程），stdin 只含增量
        assert!(matches!(turn(json!([
            { "role": "user", "content": "会话A第一轮" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "会话A第二轮" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "会话A第三轮" }
        ])), Reply::Json { status: 200, .. }));
        assert_eq!(spawn_count(&w), 5, "A1 + A2 + B1 + B2 + A3(resume)");
        let spawns = jsonl_lines(&w.dir.join("spawns.jsonl"));
        let argv5 = spawns[4].get("argv").and_then(|a| a.as_array()).unwrap();
        let joined5 = argv5.iter().map(|v| v.as_str().unwrap_or("")).collect::<Vec<_>>().join(" ");
        assert!(joined5.contains("--resume sess-fake"), "A3 resumes the shelved session: {joined5}");
        let stdin = jsonl_lines(&w.dir.join("stdinlog.jsonl"));
        assert_eq!(stdin.len(), 5);
        let a3 = stdin[4].get("line").and_then(|l| l.as_str()).unwrap();
        assert!(a3.contains("会话A第三轮"), "only the new turn: {a3}");
        assert!(!a3.contains("conversation_history"), "no replay: {a3}");
        assert!(!a3.contains("external_system_instructions"), "no system re-send: {a3}");
        // A3 收口：它消费掉的架条目不复存在，A3 自己的 run 进池（容量 1
        // 挤出 B2 的 run → B2 入架）——架上仍是 1 条，但已是 A3 的新键
        let shelf: Value = serde_json::from_slice(&std::fs::read(shelf_file(&w)).unwrap()).unwrap();
        assert_eq!(shelf.as_object().unwrap().len(), 1, "B2's evicted run is shelved; A3's own entry was consumed by its resume");
        // A4：驻留复用，不再起进程
        assert!(matches!(turn(json!([
            { "role": "user", "content": "会话A第一轮" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "会话A第二轮" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "会话A第三轮" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "会话A第四轮" }
        ])), Reply::Json { status: 200, .. }));
        assert_eq!(spawn_count(&w), 5, "A4 reuses the resident run");
        let stdin = jsonl_lines(&w.dir.join("stdinlog.jsonl"));
        assert_eq!(stdin.len(), 6);
        let a4 = stdin[5].get("line").and_then(|l| l.as_str()).unwrap();
        assert!(a4.contains("会话A第四轮"), "resident increment: {a4}");
        assert!(!a4.contains("conversation_history"));
        for var in ["MARIONETTE_FAKE_SPAWNS", "MARIONETTE_FAKE_STDIN_LOG"] {
            std::env::remove_var(var);
        }
        *TEST_IDLE_MOST.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    #[test]
    fn stale_shelf_entry_falls_back_to_a_fresh_spawn() {
        // 架条目指向的会话文件不在了（如被上一次进程的清扫收走）→ 条目清
        // 掉，落回现行全量新起路径（Go startRun :1132 的 from = nil 臂）
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        std::env::set_var("MARIONETTE_FAKE_SPAWNS", w.dir.join("spawns.jsonl"));
        std::env::set_var("MARIONETTE_FAKE_STDIN_LOG", w.dir.join("stdinlog.jsonl"));
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        // 直接写一条死条目（sess-ghost.jsonl 不存在）
        let msgs: Vec<Value> = vec![
            json!({ "role": "user", "content": "你好" }),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] }),
            json!({ "role": "user", "content": "接着说" }),
        ];
        let identity = RequestIdentity {
            owner: "max".into(),
            model: "claude-sonnet-5".into(),
            effort: None,
            tool_choice: String::new(),
            system: "You are a helpful assistant.".into(),
            tools_json: "[]".into(),
            tools_empty: true,
            msgs: msgs.clone(),
        };
        let (key, _) = next_turn(&identity).expect("a next-turn key");
        write_shelf_entry(&w, &key, ShelfEntry { session: "sess-ghost".into(), effort: None, at: now_secs() });
        let mut body = messages_body("claude/claude-sonnet-5", false);
        body["messages"] = Value::Array(msgs);
        let reply = intercept(&gateway, &post(&body), Proto::Anthropic).expect("intercepted");
        assert!(matches!(reply, Reply::Json { status: 200, .. }));
        assert_eq!(spawn_count(&w), 1, "one fresh spawn, no doomed resume attempt");
        let spawns = jsonl_lines(&w.dir.join("spawns.jsonl"));
        let argv = spawns[0].get("argv").and_then(|a| a.as_array()).unwrap();
        let joined = argv.iter().map(|v| v.as_str().unwrap_or("")).collect::<Vec<_>>().join(" ");
        assert!(!joined.contains("--resume"), "dead entry is never resumed: {joined}");
        // 全量新起：整段对话（history 包裹 + system）
        let stdin = jsonl_lines(&w.dir.join("stdinlog.jsonl"));
        let last = stdin.last().unwrap().get("line").and_then(|l| l.as_str()).unwrap();
        assert!(last.contains("conversation_history"), "fresh spawn replays history: {last}");
        assert!(last.contains("external_system_instructions"));
        assert!(shelf_read().is_empty(), "the dead entry was pruned");
        for var in ["MARIONETTE_FAKE_SPAWNS", "MARIONETTE_FAKE_STDIN_LOG"] {
            std::env::remove_var(var);
        }
    }

    #[test]
    fn rejected_resume_falls_back_to_a_fresh_spawn() {
        // CLI 拒续（会话文件在、加载不了直接退，真实 CLI 对坏会话的形状，
        // resumeerror 模式模拟）→ 首帧偷看当场现形 → 落回全量新起，客户端
        // 照常拿到回复（Go start :943-947 的回退）。会话文件**不删**（交给
        // sweep，见模块头偏差记录）。
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        std::env::set_var("MARIONETTE_FAKE_CLAUDE_MODE", "resumeerror");
        std::env::set_var("MARIONETTE_FAKE_SPAWNS", w.dir.join("spawns.jsonl"));
        std::env::set_var("MARIONETTE_FAKE_STDIN_LOG", w.dir.join("stdinlog.jsonl"));
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        *TEST_IDLE_MOST.lock().unwrap_or_else(|p| p.into_inner()) = Some(1);
        let turn = |msgs: Value| intercept(&gateway, &post(&json!({
            "model": "claude/claude-sonnet-5", "max_tokens": 1024, "stream": false,
            "system": "You are a helpful assistant.", "messages": msgs,
        })), Proto::Anthropic).expect("intercepted");
        assert!(matches!(turn(json!([{ "role": "user", "content": "会话A第一轮" }])), Reply::Json { status: 200, .. }));
        assert!(matches!(turn(json!([
            { "role": "user", "content": "会话A第一轮" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "会话A第二轮" }
        ])), Reply::Json { status: 200, .. }));
        assert!(matches!(turn(json!([{ "role": "user", "content": "会话B第一轮" }])), Reply::Json { status: 200, .. }));
        assert!(matches!(turn(json!([
            { "role": "user", "content": "会话B第一轮" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "会话B第二轮" }
        ])), Reply::Json { status: 200, .. }));
        // A3：resume 起跑 → CLI 退场 → 回退全量新起 → 200
        let a3_msgs = json!([
            { "role": "user", "content": "会话A第一轮" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "会话A第二轮" },
            { "role": "assistant", "content": [{ "type": "text", "text": "你好，世界" }] },
            { "role": "user", "content": "会话A第三轮" }
        ]);
        // A3 的 next-turn 键 = A2 入架的那个键（断言用）
        let identity = RequestIdentity {
            owner: "max".into(),
            model: "claude-sonnet-5".into(),
            effort: None,
            tool_choice: String::new(),
            system: "You are a helpful assistant.".into(),
            tools_json: "[]".into(),
            tools_empty: true,
            msgs: a3_msgs.as_array().unwrap().clone(),
        };
        let (shelved_key, _) = next_turn(&identity).expect("the A3 turn is a next turn");
        let reply = turn(a3_msgs);
        match reply {
            Reply::Json { status, body } => {
                assert_eq!(status, 200);
                let v: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(v.pointer("/content/0/text").and_then(|t| t.as_str()), Some("你好，世界"));
            }
            Reply::Stream { status, .. } => panic!("expected json, got stream {status}"),
        }
        assert_eq!(spawn_count(&w), 6, "A1 + A2 + B1 + B2 + resume attempt + fresh fallback");
        let spawns = jsonl_lines(&w.dir.join("spawns.jsonl"));
        let argv_of = |i: usize| {
            spawns[i].get("argv").and_then(|a| a.as_array()).unwrap().iter().map(|v| v.as_str().unwrap_or("")).collect::<Vec<_>>().join(" ")
        };
        assert!(argv_of(4).contains("--resume sess-fake"), "the resume attempt was made first");
        assert!(!argv_of(5).contains("--resume"), "the fallback spawns fresh");
        let stdin = jsonl_lines(&w.dir.join("stdinlog.jsonl"));
        let last = stdin.last().unwrap().get("line").and_then(|l| l.as_str()).unwrap();
        assert!(last.contains("conversation_history"), "fallback replays the whole conversation: {last}");
        // 失败 resume 消费的条目不在了；回退的 run 收口时把 B2 的 run 挤出
        // 池入架（容量 1）——架上 1 条，但已是别的键
        let shelf = shelf_read();
        assert!(!shelf.contains_key(&shelved_key), "the failed resume's entry was consumed");
        assert_eq!(shelf.len(), 1, "the fallback run's completion shelved B2's evicted run");
        let sessions = claude_sessions().expect("sessions dir");
        assert!(sessions.join("sess-fake.jsonl").is_file(), "the session file is left for the sweep, not deleted");
        for var in ["MARIONETTE_FAKE_CLAUDE_MODE", "MARIONETTE_FAKE_SPAWNS", "MARIONETTE_FAKE_STDIN_LOG"] {
            std::env::remove_var(var);
        }
        *TEST_IDLE_MOST.lock().unwrap_or_else(|p| p.into_inner()) = None;
    }

    #[test]
    fn shelf_entries_expire_and_dead_ones_are_pruned() {
        // 过期条目（Go shelfTimer :2780 的惰性对应）查不到且被清掉；没过期
        // 但会话文件不在的（死条目）也清掉。条目内容是内容哈希的键——死
        // 了永远死，留着只占容量。
        let (_gateway, w) = world(None, false);
        write_shelf_entry(&w, "k-old", ShelfEntry { session: "sess-old".into(), effort: None, at: now_millis() - 2 * 3600 * 1000 });
        write_shelf_entry(&w, "k-fresh", ShelfEntry { session: "sess-new".into(), effort: None, at: now_millis() });
        assert_eq!(unshelve("k-old", None), None, "expired entries are not served");
        let entries = shelf_read();
        assert!(!entries.contains_key("k-old"), "expired entry pruned");
        assert!(entries.contains_key("k-fresh"), "fresh entry survives the prune");
        assert_eq!(unshelve("k-fresh", None), None, "missing session file = dead entry");
        assert!(shelf_read().is_empty(), "dead entry pruned");
        // 档位不一致：条目留着（同键同档位的后续回合还能用，1 小时内自然
        // 过期）；同档位的下一查认领（出架交 session id）
        let sessions = claude_sessions().expect("sessions dir");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join("sess-eff.jsonl"), "{}\n").unwrap();
        write_shelf_entry(&w, "k-eff", ShelfEntry { session: "sess-eff".into(), effort: Some("high".into()), at: now_millis() });
        assert_eq!(unshelve("k-eff", Some("low")), None, "effort mismatch does not resume");
        assert!(shelf_read().contains_key("k-eff"), "entry kept for a same-effort turn");
        assert_eq!(unshelve("k-eff", Some("high")).as_deref(), Some("sess-eff"), "matching effort claims the entry");
        assert!(shelf_read().is_empty(), "claimed entry leaves the shelf");
        let _ = &w;
    }

    #[test]
    fn session_paths_follow_the_go_encoding() {
        // claudeProjectName（Go :482）：字母数字之外的字符全换 `-`；sid
        // 护栏（Go removeSession :3465 加一档字符域）
        assert_eq!(claude_project_name(r"C:\Users\a b\AppData\Local\Temp\marionette-claude-work"), "C--Users-a-b-AppData-Local-Temp-marionette-claude-work");
        assert_eq!(claude_project_name("/tmp/marionette-claude-work"), "-tmp-marionette-claude-work");
        assert!(sid_ok("3f9c2b1e-4d5a-4f6b-8c7d-2e1f0a9b8c7d"));
        assert!(!sid_ok(""));
        assert!(!sid_ok("../escape"));
        assert!(!sid_ok(r"a\b"));
        assert!(!sid_ok("a/b"));
        assert!(!sid_ok("sess dot"));
    }

    // ---------------------------------------------------------------
    // MCP helper（差距第 3 条：Go bridgeTools :8137 / mcpCall :6479 /
    // claudebridge/mcp.go 的阻塞-回填协议）
    // ---------------------------------------------------------------

    fn weather_tools() -> Vec<Value> {
        vec![json!({
            "name": "get_weather",
            "description": "查天气",
            "input_schema": { "type": "object", "properties": { "city": { "type": "string" } } }
        })]
    }

    #[test]
    fn tool_round_trips_through_the_helper() {
        // tool_use 下发（名字映射回原名、SSE 在 message_stop 收口）→
        // tools/call 阻塞 → caller 回 tool_result → helper 回填 → 续答，
        // 一回合内完成且不起新进程
        let (gateway, w) = world(None, false);
        write_credentials(&w, "max");
        let cli = fake_cli(&w.dir);
        std::env::set_var("MARIONETTE_FAKE_CLAUDE_MODE", "toolround");
        std::env::set_var("MARIONETTE_FAKE_SPAWNS", w.dir.join("spawns.jsonl"));
        std::env::set_var("MARIONETTE_FAKE_STDIN_LOG", w.dir.join("stdinlog.jsonl"));
        std::env::set_var("MARIONETTE_FAKE_MCP_LOG", w.dir.join("mcplog.jsonl"));
        TEST_SEAM.lock().unwrap_or_else(|p| p.into_inner()).as_mut().unwrap().cli = Some(cli);
        // 第一回合：带 tools 的请求 → 回 tool_use 块
        let mut body = messages_body("claude/claude-sonnet-5", true);
        body["tools"] = json!(weather_tools());
        body["messages"] = json!([{ "role": "user", "content": "北京天气怎么样" }]);
        let reply = intercept(&gateway, &post(&body), Proto::Anthropic).expect("turn 1");
        let sse = read_stream(reply);
        let events = sse_events(&sse);
        let types: Vec<&str> = events.iter().map(|e| e.get("type").and_then(|t| t.as_str()).unwrap_or("")).collect();
        assert_eq!(types.first(), Some(&"message_start"));
        assert_eq!(types.last(), Some(&"message_stop"), "stream closes at the tool_use message (park)");
        let block = events.iter().find(|e| e.get("type").and_then(|t| t.as_str()) == Some("content_block_start")).expect("tool block");
        // 名字映射回 caller 原名（Go declaredName）——不是 mcp__marionette__ 前缀名
        assert_eq!(block.pointer("/content_block/type").and_then(|t| t.as_str()), Some("tool_use"));
        assert_eq!(block.pointer("/content_block/name").and_then(|t| t.as_str()), Some("get_weather"));
        assert_eq!(block.pointer("/content_block/id").and_then(|t| t.as_str()), Some("toolu_01"));
        let md = events.iter().find(|e| e.get("type").and_then(|t| t.as_str()) == Some("message_delta")).unwrap();
        assert_eq!(md.pointer("/delta/stop_reason").and_then(|s| s.as_str()), Some("tool_use"));
        // 第二回合：caller 回 tool_result → match 命中 → 回填 → 续答
        let mut body2 = messages_body("claude/claude-sonnet-5", false);
        body2["tools"] = json!(weather_tools());
        body2["messages"] = json!([
            { "role": "user", "content": "北京天气怎么样" },
            { "role": "assistant", "content": [
                { "type": "tool_use", "id": "toolu_01", "name": "get_weather", "input": { "city": "北京" } } ]},
            { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "toolu_01", "content": "晴 25°C" } ]}
        ]);
        let reply = intercept(&gateway, &post(&body2), Proto::Anthropic).expect("turn 2");
        match reply {
            Reply::Json { status, body } => {
                assert_eq!(status, 200);
                let v: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(v.pointer("/content/0/text").and_then(|t| t.as_str()), Some("天气结论：晴 25°C"));
                assert_eq!(v.get("stop_reason").and_then(|s| s.as_str()), Some("end_turn"));
            }
            Reply::Stream { status, .. } => panic!("expected json, got stream {status}"),
        }
        // 全程一个进程（helper 回填在同一驻留 run 里完成）
        assert_eq!(spawn_count(&w), 1, "tool round must complete in the resident run");
        // 带 tools 的请求挂 --mcp-config（Go claudeCLIArgs :3965；无 tools
        // 的路径没有它——见 bridge_roundtrip 的断言）
        let spawns = jsonl_lines(&w.dir.join("spawns.jsonl"));
        let argv = spawns[0].get("argv").and_then(|a| a.as_array()).unwrap();
        let joined = argv.iter().map(|v| v.as_str().unwrap_or("")).collect::<Vec<_>>().join(" ");
        assert!(joined.contains("--mcp-config"), "helper attached for a tools-bearing request: {joined}");
        assert!(joined.contains("--tools "), "Claude Code built-ins stay off: {joined}");
        // MCP 流水：initialize 应答、tools/list 宣告、tools/call 拿到 tool_result 的文本
        //（fake 的流水里 "req" 行是请求侧记账，带 body 的是应答行）
        let mcp = jsonl_lines(&w.dir.join("mcplog.jsonl"));
        let answered = |m: &str| mcp.iter().find(|l| l.get("body").is_some() && l.get("method").and_then(|x| x.as_str()) == Some(m));
        let init = answered("initialize").expect("initialize round-trip");
        assert!(init.get("body").and_then(|b| b.as_str()).unwrap_or("").contains("marionette"), "serverInfo");
        let list = answered("tools/list").expect("tools/list round-trip");
        let list_body = list.get("body").and_then(|b| b.as_str()).unwrap_or("");
        assert!(list_body.contains("get_weather"), "helper offers the caller's tool: {list_body}");
        assert!(list_body.contains("inputSchema"), "schema announced: {list_body}");
        let call = answered("tools/call").expect("tools/call round-trip");
        let call_body = call.get("body").and_then(|b| b.as_str()).unwrap_or("");
        assert!(call_body.contains("晴 25°C"), "tool_result refilled into the helper: {call_body}");
        assert!(call_body.contains("\"isError\":false"));
        // tools/call 之后没有新 spawn，第二回合也没再喂全量对话
        let stdin = jsonl_lines(&w.dir.join("stdinlog.jsonl"));
        assert_eq!(stdin.len(), 1, "continuation runs without a new user line (results go through the helper)");
        for var in ["MARIONETTE_FAKE_CLAUDE_MODE", "MARIONETTE_FAKE_SPAWNS", "MARIONETTE_FAKE_STDIN_LOG", "MARIONETTE_FAKE_MCP_LOG"] {
            std::env::remove_var(var);
        }
    }

    #[test]
    fn tool_choice_none_refuses_calls_without_dropping_them() {
        // tool_choice none：宣告保留（缓存前缀稳定，Go bridgeTools :8141 注
        // 释）、调用时直拒（Go mcpCall :6539）。直测 helper_call 的拒绝臂。
        let (_, w) = world(None, false);
        write_credentials(&w, "max");
        let (_tools, names, allowed_all) = bridge_tools(&weather_tools(), "none");
        assert!(!allowed_all, "none refuses every call");
        assert_eq!(names.declared.len(), 1, "announcement keeps all tools");
        let core = RunCore {
            child: Arc::new(Mutex::new(dummy_child())),
            stdin: Arc::new(Mutex::new(None)),
            stderr_tail: Arc::new(Mutex::new(String::new())),
            token: None,
            cfg_dir: None,
            state: Arc::new((
                Mutex::new(RunState {
                    closed: false,
                    deadline: Instant::now() + BRIDGE_TIMEOUT,
                    queue: VecDeque::new(),
                    segment_done: false,
                    parked: false,
                    started: false,
                    got_stop: false,
                    streamed: Vec::new(),
                    said_words: String::new(),
                    msg_tool_ids: Vec::new(),
                    msg_stop_reason: None,
                    model_echo: "claude-sonnet-5".into(),
                    session_id: String::new(),
                    identity: None,
                    effort: None,
                    idle_key: None,
                    names,
                    helper_tools: Vec::new(),
                    allowed_all,
                    pending: HashMap::new(),
                    early: HashMap::new(),
                }),
                Condvar::new(),
            )),
        };
        let core = Arc::new(core);
        let res = helper_call(&core, &json!({ "name": "get_weather", "arguments": {} }));
        let v = res.expect("refusal is a result, not an error");
        assert_eq!(v.pointer("/isError").and_then(|e| e.as_bool()), Some(true));
        assert!(v.pointer("/content/0/text").and_then(|t| t.as_str()).unwrap_or("").contains("No tool can be used"));
        let _ = &w;
    }

    /// 一个不会真跑的哑子进程句柄（helper 直测用；RunCore 只在孩子终局
    /// 清理时碰它）。立即退出的 cmd。
    fn dummy_child() -> Child {
        Command::new("cmd.exe")
            .arg("/C")
            .arg("exit 0")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("dummy child")
    }

    // ---------------------------------------------------------------
    // 名字净化与会话键单元（Go bridgeToolName :5616 / messageWords）
    // ---------------------------------------------------------------

    #[test]
    fn tool_names_are_sanitized_and_mapped_back() {
        // 合规名原样
        assert_eq!(bridge_tool_name("read_file"), "read_file");
        // 非法字符 → 净化 + 12 hex 后缀（原名恒同结果）
        let a = bridge_tool_name("spawn.agent");
        let b = bridge_tool_name("spawn.agent");
        assert_eq!(a, b);
        assert!(a.starts_with("spawn_agent_"), "{a}");
        let suffix = &a[a.len() - 12..];
        assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()), "{a}");
        assert_ne!(bridge_tool_name("spawn.agent"), bridge_tool_name("spawn-agent"));
        // 已带 mcp__marionette__ 前缀的（caller 是 Claude 系）只取余下部分
        assert_eq!(bridge_tool_name("mcp__marionette__read_file"), "read_file");
        // 双向映射：模型名 → 原名、原名 → 模型全名
        let (_tools, names, _) = bridge_tools(&weather_tools(), "");
        assert_eq!(names.declared_of("mcp__marionette__get_weather").as_deref(), Some("get_weather"));
        assert_eq!(names.bridged_of("get_weather").as_deref(), Some("mcp__marionette__get_weather"));
        // 没宣告的工具不映射（延迟加载口径）
        assert_eq!(names.declared_of("WebFetch"), None);
        assert_eq!(names.bridged_of("WebFetch"), None);
    }

    #[test]
    fn turn_keys_merge_block_splits_and_call_positions() {
        // 相邻同角色合并 + 空白归一：一条回复拆两条消息/拆多个 text 块与
        // 整条文本是同一个会话键（Go #5705）
        let id = |msgs: Vec<Value>| RequestIdentity {
            owner: "max".into(),
            model: "claude-sonnet-5".into(),
            effort: None,
            tool_choice: String::new(),
            system: String::new(),
            tools_json: "[]".into(),
            tools_empty: true,
            msgs,
        };
        let one = id(vec![
            json!({ "role": "user", "content": "第一问" }),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "看这里:\n答案" }] }),
        ]);
        let split = id(vec![
            json!({ "role": "user", "content": [{ "type": "text", "text": "第一问" }] }),
            json!({ "role": "assistant", "content": [
                { "type": "thinking", "thinking": "internal reasoning" },
                { "type": "text", "text": "看这里:" },
                { "type": "text", "text": "答案" },
            ]}),
            json!({ "role": "assistant", "content": [] }), // 空消息不计
        ]);
        assert_eq!(turn_key(&one, &one.msgs), turn_key(&split, &split.msgs));
        // tool_use 按调用次序计身份、不看 id（AmpCode 式改写 id 不影响键）
        let with_ids = id(vec![
            json!({ "role": "user", "content": "查" }),
            json!({ "role": "assistant", "content": [
                { "type": "tool_use", "id": "toolu_A", "name": "search", "input": { "q": "x" } } ]}),
            json!({ "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "toolu_A", "content": "结果" } ]}),
        ]);
        let renamed = id(vec![
            json!({ "role": "user", "content": "查" }),
            json!({ "role": "assistant", "content": [
                { "type": "tool_use", "id": "TU-xyz", "name": "search", "input": { "q": "x" } } ]}),
            json!({ "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "TU-xyz", "content": "结果" } ]}),
        ]);
        assert_eq!(turn_key(&with_ids, &with_ids.msgs), turn_key(&renamed, &renamed.msgs));
        // 身份字段不同 → 不同键（model/system/tools）
        let mut other = one.clone();
        other.model = "claude-opus-5".into();
        assert_ne!(turn_key(&one, &one.msgs), turn_key(&other, &other.msgs));
        let mut with_system = one.clone();
        with_system.system = "别的人设".into();
        assert_ne!(turn_key(&one, &one.msgs), turn_key(&with_system, &with_system.msgs));
        // nextTurn：新回合给键、纯工具续轮不给（走 match）
        let fresh = RequestIdentity {
            msgs: vec![
                json!({ "role": "user", "content": "hi" }),
                json!({ "role": "assistant", "content": [{ "type": "text", "text": "答" }] }),
                json!({ "role": "user", "content": "下一问" }),
            ],
            ..one.clone()
        };
        assert!(next_turn(&fresh).is_some());
        let tooling = RequestIdentity {
            msgs: vec![
                json!({ "role": "user", "content": "hi" }),
                json!({ "role": "assistant", "content": [
                    { "type": "text", "text": "我调工具" },
                    { "type": "tool_use", "id": "t1", "name": "search", "input": {} } ]}),
                json!({ "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "t1", "content": "结果" } ]}),
            ],
            ..one.clone()
        };
        assert!(next_turn(&tooling).is_none(), "tool-result continuation goes through match, not resume");
    }

    #[test]
    fn fresh_results_and_said_extraction() {
        let msgs = vec![
            json!({ "role": "user", "content": "跑一下" }),
            json!({ "role": "assistant", "content": [
                { "type": "tool_use", "id": "t1", "name": "run", "input": {} } ]}),
            json!({ "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "t1", "content": [{ "type": "text", "text": "done" }] },
                { "type": "text", "text": "顺便再看看" } ]}),
        ];
        let fresh = fresh_results(&msgs).expect("fresh tool result");
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].id, "t1");
        assert_eq!(fresh[0].content, "done");
        assert!(!fresh[0].is_error);
        // 随结果的用户文字被单独提出（Go saidWithResults）；没有结果就没有 said
        assert_eq!(said_with_results(&msgs), "顺便再看看");
        let plain = vec![json!({ "role": "user", "content": "新问题" })];
        assert!(fresh_results(&plain).is_none());
        assert_eq!(said_with_results(&plain), "");
    }
}
