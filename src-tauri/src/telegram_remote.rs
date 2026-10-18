//! Telegram 远程桥（docs/telegram-bridge.md · MVP）。
//!
//! 一个纯出站的 Telegram 通知/回复通道：
//!
//! - **发送侧**：钩在 `acp.rs` 的 UI 事件收口 `emit_now` 上（`observe_ui_event`），
//!   只认五类事件——回合结束 / 等权限 / 等 ask / 等 plan 审批 / 进程死亡——
//!   组装成文本（+内联按钮）投进 mpsc 队列，由发送线程串行发出。
//!   事件收口里绝不做网络 IO。
//! - **接收侧**：`getUpdates` 长轮询（timeout=50s）。文本消息路由到会话后
//!   `app.emit("telegram-remote-inbound")` 交给前端 `performSend` 管线
//!   （挂起重连、历史注入、排队、配置回推全部复用），前端完成后
//!   `telegram_remote_ack` 回执。权限/ask/plan 按钮直接在 Rust 侧走
//!   `AcpService::respond_*`——与 UI 是同一批 waiter map，先答先得。
//! - **总控面板**：一条置顶消息（`pinChatMessage`），会话状态一变（开跑/
//!   跑完/等你/等待解除）就 `editMessageText` 原地重画——谁在跑、谁在等你、
//!   裸发文字的去向一目了然；按钮 = 切换绑定 + 远程 ⏹（`AcpService::cancel`，
//!   与 UI 停止键同路）+ 刷新。`/status` 拉全量清单。心跳线程在忙时每 60s
//!   重画一次保鲜时长行。
//! - **HTTP**：复用 `http_client` 的 ureq + native-tls agent；阻塞调用跑在
//!   两条 std 线程上（发送 + 轮询），零新依赖，体积不变。
//! - **安全**：bot token 经 DPAPI 落 `~/.marionette/telegram.json`（与凭据库
//!   同一套前缀格式），明文只在内存；只处理配对过的 chat id；未配对时
//!   第一条私聊消息挂起为「待配对」，在设置浮层里确认后才固化。
//!
//! 未启用时 `TELEGRAM` 未初始化或 `enabled=false`，`observe_ui_event` 第一行
//! 就返回——对主流程零开销、零行为变化。轮询线程只要有 token 就会跑
//! （token 无论是进程启动时就有、还是运行中才保存，`ensure_threads`
//! 两条路都会拉起；否则永远看不到第一条 `/start`，配对会死锁）；
//! 发送与通知都额外要求已配对且开关打开。

use crate::acp::AcpService;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// Telegram Bot API base。用户在日本直连可达（调研 2026-10-17）。
const API_BASE: &str = "https://api.telegram.org";
/// 长轮询挂起秒数；HTTP 超时留 15s 余量。
const POLL_TIMEOUT_SECS: u64 = 50;
const POLL_HTTP_TIMEOUT_SECS: u64 = 65;
const SEND_HTTP_TIMEOUT_SECS: u64 = 15;
/// sendMessage 正文上限 4096 字符；留安全边界。
const MAX_MESSAGE_CHARS: usize = 3800;
/// 回合末条回复在推送里保留的长度已改为按消息分片（见 REPLY_CHUNK_CHARS）。
/// 回合内 agent 文本滚动缓冲上限（超了丢头部，保尾部）——32K 约等于全文。
const TURN_BUFFER_CHARS: usize = 32000;
/// 推送正文每条消息的预算（4096 上限给头部/页脚/MarkdownV2 转义膨胀留余量）。
const REPLY_CHUNK_CHARS: usize = 3300;
/// 单片转成 MarkdownV2 后的长度上限：转义最坏约 2×，超了就在原文上对半
/// 重切（`fit_markdown_chunk`），给头/尾与 4096 硬顶留余量。
const MD_CHUNK_ESCAPED_LIMIT: usize = 3400;
/// 正文最多拆几条消息（主消息 + 续片），再长就截断提示回电脑看。
const REPLY_MAX_MESSAGES: usize = 4;
/// bot 发出的消息 id → 会话 的路由表上限（FIFO 淘汰）。
const MAX_ROUTES: usize = 256;
/// 内联按钮回调上下文上限（过期按钮按下会得到「已处理/已过期」）。
const MAX_CALLBACKS: usize = 128;
/// 权限卡片 120s 自动拒绝（acp.rs `TIMEOUT_PERMISSION_SECS`）；
/// 提前 30s 提醒 = 挂起 90s 后仍未解答才推。
const PERMISSION_REMINDER_AFTER_SECS: u64 = 90;
/// 待配对请求的存活时间。
const PAIRING_TTL: Duration = Duration::from_secs(300);
/// getUpdates 断线退避：1s 起步、60s 封顶；409（同 token 第二个 poller）15s 起。
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(60);
const BACKOFF_409_START: Duration = Duration::from_secs(15);

/// 前端收文本路由的事件名（App.tsx 监听 → performSend → ack）。
pub const INBOUND_EVENT: &str = "telegram-remote-inbound";

// ─── 设置（~/.marionette/telegram.json）────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TelegramPrefs {
    /// 回合结束推送。
    pub notify_turn: bool,
    /// 等待推送（权限 / ask / plan）。
    pub notify_wait: bool,
    /// 进程死亡 / 回合报错推送。
    pub notify_error: bool,
    /// 推送里带末条回复摘要（关 = 只推一行标题，内容不出本机）。
    pub content_preview: bool,
}

impl Default for TelegramPrefs {
    fn default() -> Self {
        Self {
            notify_turn: true,
            notify_wait: true,
            notify_error: true,
            content_preview: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TelegramSettings {
    pub version: u32,
    /// 总开关。要求 token + 已配对才有实际效果（保存时校验）。
    pub enabled: bool,
    /// DPAPI 密文（`dpapi:<base64>`），绝不落明文。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_cipher: Option<String>,
    /// 配对白名单：唯一的 chat id。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paired_username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bot_username: Option<String>,
    pub prefs: TelegramPrefs,
}

impl Default for TelegramSettings {
    fn default() -> Self {
        Self {
            version: 1,
            enabled: false,
            token_cipher: None,
            chat_id: None,
            paired_username: None,
            bot_username: None,
            prefs: TelegramPrefs::default(),
        }
    }
}

const SETTINGS_VERSION: u32 = 1;

fn settings_path() -> Result<PathBuf, String> {
    Ok(crate::app_paths::global_dir()?.join("telegram.json"))
}

fn load_settings_at(path: &Path) -> TelegramSettings {
    let Ok(text) = std::fs::read_to_string(path) else {
        return TelegramSettings::default();
    };
    if text.trim().is_empty() {
        return TelegramSettings::default();
    }
    match serde_json::from_str::<TelegramSettings>(&text) {
        Ok(settings) if settings.version == SETTINGS_VERSION => settings,
        Ok(_newer) => {
            // 未来版本：留档后从默认重来（立场与凭据库一致：不静默清空）。
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let _ = std::fs::rename(
                path,
                path.with_file_name(format!("telegram.corrupt-{ts}.json")),
            );
            crate::debug_log::append(
                "telegram",
                "warn",
                "",
                "telegram.json from a newer Marionette — archived and reset",
                None,
            );
            TelegramSettings::default()
        }
        Err(_parse) => {
            // 解析失败（截断 / 并发写瞬态 / 外部编辑）：文件原地保留供排查，
            // 绝不归档清空——归档会把 token 和配对一起带走（2026-10-18
            // 实测日志里出现成串的 unreadable 归档告警）。警告限频：设置
            // 浮层开着时状态轮询 1Hz，不限频会刷爆 dev.log。
            static LAST_WARN_MS: AtomicU64 = AtomicU64::new(0);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let previous = LAST_WARN_MS.swap(now, Ordering::Relaxed);
            if now.saturating_sub(previous) >= 60_000 {
                crate::debug_log::append(
                    "telegram",
                    "warn",
                    "",
                    "telegram.json unreadable — kept on disk, using defaults",
                    None,
                );
            }
            TelegramSettings::default()
        }
    }
}

fn save_settings_at(path: &Path, settings: &TelegramSettings) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建 .marionette 目录失败: {e}"))?;
    }
    let body = serde_json::to_string_pretty(settings)
        .map_err(|e| format!("序列化 telegram.json 失败: {e}"))?;
    // 临时兄弟文件 + rename 原子写（同凭据库模式）。
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, format!("{body}\n")).map_err(|e| format!("写入临时文件失败: {e}"))?;
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("重命名临时文件失败: {error}"));
    }
    Ok(())
}

fn load_settings() -> TelegramSettings {
    match settings_path() {
        Ok(path) => load_settings_at(&path),
        Err(_) => TelegramSettings::default(),
    }
}

fn save_settings(settings: &TelegramSettings) -> Result<(), String> {
    save_settings_at(&settings_path()?, settings)
}

// ─── 运行时状态 ────────────────────────────────────────────────────────────

struct SessionTrack {
    /// 本回合开始时刻（见到 user_message 回显时钉住）。
    turn_started: Option<Instant>,
    last_activity: Instant,
    /// 本回合 agent 文本的尾部滚动缓冲（推送摘要用）。
    reply_tail: String,
    /// 挂起未解的权限/ask/plan 请求数（>0 = 会话在等你）。
    waiting: u32,
}

impl SessionTrack {
    fn new() -> Self {
        Self {
            turn_started: None,
            last_activity: Instant::now(),
            reply_tail: String::new(),
            waiting: 0,
        }
    }

    fn state(&self) -> DashState {
        if self.waiting > 0 {
            DashState::Waiting
        } else if self.turn_started.is_some() {
            DashState::Running
        } else {
            DashState::Idle
        }
    }
}

/// 总控面板/状态清单里一个会话的态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DashState {
    Running,
    Waiting,
    Idle,
}

impl DashState {
    fn icon(self) -> &'static str {
        match self {
            DashState::Running => "▶",
            DashState::Waiting => "⏸",
            DashState::Idle => "💤",
        }
    }
}

/// 内联按钮按下后要做的事。存在 Rust 侧，callback_data 只带短 id。
#[derive(Clone, Debug)]
enum CallbackAction {
    Permission {
        session_id: String,
        request_id: String,
        option_id: String,
        option_label: String,
    },
    Question {
        session_id: String,
        request_id: String,
        question: String,
        selected: String,
    },
    QuestionDecline {
        session_id: String,
        request_id: String,
    },
    Plan {
        session_id: String,
        request_id: String,
        decision: String,
        decision_label: String,
    },
    /// 总控面板：把裸文本的默认去向切到这个会话。
    BindSession {
        session_id: String,
        label: String,
    },
    /// 总控面板：打断该会话当前回合（与 UI 停止键同一条 cancel 路径）。
    StopTurn {
        session_id: String,
        label: String,
    },
    /// 总控面板：手动刷新面板文本。
    RefreshDashboard,
}

impl CallbackAction {
    fn session_id(&self) -> &str {
        match self {
            CallbackAction::Permission { session_id, .. }
            | CallbackAction::Question { session_id, .. }
            | CallbackAction::QuestionDecline { session_id, .. }
            | CallbackAction::Plan { session_id, .. }
            | CallbackAction::BindSession { session_id, .. }
            | CallbackAction::StopTurn { session_id, .. } => session_id,
            // 无会话上下文的动作：日志里给个可读名。
            CallbackAction::RefreshDashboard => "dashboard",
        }
    }
}

#[derive(Clone, Debug)]
struct PairingRequest {
    chat_id: i64,
    username: String,
    first_name: String,
    received_at: Instant,
}

/// 需要串行访问的可变状态（observe 钩子 / 轮询 / 发送三处共享）。
struct Inner {
    sessions: HashMap<String, SessionTrack>,
    /// 最近一次收到 user_message 的会话 = 裸文本的默认去向。
    last_user_session: Option<String>,
    /// bot 发出的 tg message_id → session_id（引用回复路由）。
    routes: HashMap<i64, String>,
    route_order: VecDeque<i64>,
    /// callback 短 id → 按下后要做的事。
    callbacks: HashMap<u64, CallbackAction>,
    callback_order: VecDeque<u64>,
    /// 已推送、还在等人的权限请求（提醒去重 + 已答清理）。
    pending_permissions: HashMap<String, PendingPermissionPush>,
    /// 挂起中的 ask / plan 请求（request_id → session_id；清理 waiting 计数用）。
    pending_questions: HashMap<String, String>,
    pending_plans: HashMap<String, String>,
    /// 总控面板消息（None = 还没发过；用户删了会在下次 edit 失败时重建）。
    dashboard: Option<DashboardHandle>,
    pending_pairing: Option<PairingRequest>,
    last_error: Option<String>,
}

/// 已发出的总控面板消息。
#[derive(Clone, Copy, Debug)]
struct DashboardHandle {
    message_id: i64,
}

#[derive(Clone)]
struct PendingPermissionPush {
    session_id: String,
}

impl Inner {
    fn new() -> Self {
        Self {
            sessions: HashMap::new(),
            last_user_session: None,
            routes: HashMap::new(),
            route_order: VecDeque::new(),
            callbacks: HashMap::new(),
            callback_order: VecDeque::new(),
            pending_permissions: HashMap::new(),
            pending_questions: HashMap::new(),
            pending_plans: HashMap::new(),
            dashboard: None,
            pending_pairing: None,
            last_error: None,
        }
    }

    fn track(&mut self, session_id: &str) -> &mut SessionTrack {
        self.sessions
            .entry(session_id.to_string())
            .or_insert_with(SessionTrack::new)
    }

    fn remember_route(&mut self, message_id: i64, session_id: &str) {
        if self.routes.insert(message_id, session_id.to_string()).is_none() {
            self.route_order.push_back(message_id);
        }
        while self.routes.len() > MAX_ROUTES {
            match self.route_order.pop_front() {
                Some(oldest) => {
                    self.routes.remove(&oldest);
                }
                None => break,
            }
        }
    }

    fn remember_callback(&mut self, id: u64, action: CallbackAction) {
        if self.callbacks.insert(id, action).is_none() {
            self.callback_order.push_back(id);
        }
        while self.callbacks.len() > MAX_CALLBACKS {
            match self.callback_order.pop_front() {
                Some(oldest) => {
                    self.callbacks.remove(&oldest);
                }
                None => break,
            }
        }
    }
}

/// 发送线程的任务。
enum Outbound {
    Send {
        text: String,
        buttons: Vec<TgButton>,
        /// 发出后把 tg message_id 记到这个会话名下（引用回复路由）。
        route_for_session: Option<String>,
        /// 发到手机时 reply_to 这条消息（用户发来的那条）。
        reply_to: Option<i64>,
        /// true = 以 MarkdownV2 发送（parse 失败自动回退纯文本）。
        md: bool,
    },
    /// 前端 performSend 的回执。
    Ack {
        ok: bool,
        error: Option<String>,
        session_label: String,
        reply_to: Option<i64>,
    },
    /// 总控面板更新：有 message_id 就 edit，没有就发新 + 置顶。
    Dashboard {
        text: String,
        /// inline_keyboard 的行分组（会话行 = [名字, ⏹]，末行 = [🔄]）。
        keyboard: Vec<Vec<TgButton>>,
    },
}

#[derive(Clone, Debug)]
struct TgButton {
    text: String,
    callback_id: u64,
}

/// 线程共用的活配置（保存命令原地刷新，线程下一轮跟随）。
#[derive(Clone, Debug, Default)]
struct LiveConfig {
    /// 用户开关（要求 token 存在）。
    enabled: bool,
    token: Option<String>,
    chat_id: Option<i64>,
    prefs: TelegramPrefs,
}

impl LiveConfig {
    /// 通知/发送的生效条件：开关开 + 已配对。
    fn active(&self) -> bool {
        self.enabled && self.chat_id.is_some()
    }
}

/// 进程级单例。未启动（从未 start）时为 None，钩子零开销。
struct Telegram {
    app: AppHandle,
    acp: AcpService,
    outbound: Sender<Outbound>,
    /// 发送线程的接收端。惰性取走：线程第一次拉起时 move 进去。
    inbound_rx: Mutex<Option<std::sync::mpsc::Receiver<Outbound>>>,
    inner: Mutex<Inner>,
    live: Mutex<LiveConfig>,
    callback_seq: AtomicU64,
    /// 线程只拉一次（start 与 apply_live 两条路都可能触发）。
    threads_started: AtomicBool,
}

static TELEGRAM: OnceLock<Telegram> = OnceLock::new();

fn log(level: &str, summary: &str, detail: Option<&str>) {
    crate::debug_log::append("telegram", level, "", summary, detail);
}

/// 进程内初始化（main.rs setup 调一次）。幂等。
///
/// 只要 token 存在就把两条线程拉起来：轮询线程在未配对时也要能收到
/// 第一条消息，否则配对流程死锁。线程在开关关闭时自动休眠。
pub fn start(app: AppHandle, acp: AcpService) {
    let settings = load_settings();
    let token = settings.token_cipher.as_deref().and_then(|cipher| {
        match crate::credentials::decrypt_secret(cipher) {
            Ok(token) => Some(token),
            Err(error) => {
                log(
                    "warn",
                    "bot token decrypt failed — telegram remote stays off",
                    Some(&error),
                );
                None
            }
        }
    });
    let live = LiveConfig {
        enabled: settings.enabled && token.is_some(),
        token,
        chat_id: settings.chat_id,
        prefs: settings.prefs.clone(),
    };

    let (tx, rx) = std::sync::mpsc::channel::<Outbound>();
    let telegram = Telegram {
        app,
        acp,
        outbound: tx,
        inbound_rx: Mutex::new(Some(rx)),
        inner: Mutex::new(Inner::new()),
        live: Mutex::new(live),
        callback_seq: AtomicU64::new(1),
        threads_started: AtomicBool::new(false),
    };
    if TELEGRAM.set(telegram).is_ok() {
        if let Some(slot) = TELEGRAM.get() {
            ensure_threads(slot);
        }
    }
}

/// 幂等拉起发送 + 轮询线程（进程内只发生一次）。
///
/// 两条触发路：`start`（进程启动时已有 token）与 `apply_live`（运行中
/// 首次保存 token）。漏掉第二条的话，首次使用流程「先开 app、后粘
/// token」会永远等不到第一条消息——配对死锁（2026-10-18 实测踩中）。
fn ensure_threads(slot: &'static Telegram) {
    if slot.threads_started.swap(true, Ordering::SeqCst) {
        return;
    }
    let Some(rx) = slot
        .inbound_rx
        .lock()
        .ok()
        .and_then(|mut receiver| receiver.take())
    else {
        // 已被拉走过（不应发生）：标记回退，别把状态卡死。
        slot.threads_started.store(false, Ordering::SeqCst);
        return;
    };
    let sender_slot = slot;
    thread::spawn(move || sender_loop(sender_slot, rx));
    let poller_slot = slot;
    thread::spawn(move || poller_loop(poller_slot));
    // 面板心跳：有会话在跑/在等时每 60s 重画一次，让「跑了 N 分钟」保鲜。
    let ticker_slot = slot;
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(DASHBOARD_TICK_SECS));
        let live = ticker_slot.live.lock().ok().map(|l| l.clone());
        let Some(live) = live else { continue };
        if !live.active() {
            continue;
        }
        let busy = ticker_slot
            .inner
            .lock()
            .map(|inner| {
                inner
                    .sessions
                    .values()
                    .any(|track| track.waiting > 0 || track.turn_started.is_some())
            })
            .unwrap_or(false);
        if busy {
            schedule_dashboard(ticker_slot);
        }
    });
    log("info", "telegram threads started", None);
}

// ─── UI 事件钩子（acp.rs emit_now 调用）────────────────────────────────────

/// UI 事件的旁路观察点。必须便宜：未初始化 / 未生效时前两行就返回。
pub fn observe_ui_event(session_id: &str, kind: &str, method: Option<&str>, data: &Value) {
    let Some(slot) = TELEGRAM.get() else { return };
    let live = slot.live.lock().ok().map(|l| l.clone());
    let Some(live) = live else { return };
    if !live.active() {
        return;
    }

    match (kind, method) {
        ("notification", Some("session/update")) => observe_session_update(slot, session_id, data),
        ("response" | "error", Some("turn/complete")) => {
            observe_turn_complete(slot, session_id, kind, data, &live)
        }
        ("request", Some("permission/prompt")) => {
            observe_permission_prompt(slot, session_id, data, &live)
        }
        ("request", Some("question/prompt")) => {
            observe_question_prompt(slot, session_id, data, &live)
        }
        ("request", Some("plan/approval")) => observe_plan_prompt(slot, session_id, data, &live),
        ("error", Some("process/ended")) => {
            // acp 的 drop_ui_pending_for_session 不发事件——这里就地清掉
            // 该会话的等待/回合追踪，否则面板留一个永远不消的 ⏸。
            {
                if let Ok(mut inner) = slot.inner.lock() {
                    inner
                        .pending_permissions
                        .retain(|_, pending| pending.session_id != session_id);
                    inner
                        .pending_questions
                        .retain(|_, session| session != session_id);
                    inner
                        .pending_plans
                        .retain(|_, session| session != session_id);
                    let track = inner.track(session_id);
                    track.waiting = 0;
                    track.turn_started = None;
                }
            }
            if live.prefs.notify_error {
                let message = data
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("agent 进程退出");
                push_simple(&live, session_id, "💀", "agent 进程退出", Some(message));
            }
            schedule_dashboard(slot);
        }
        // 已解答/已超时：清理等待追踪 + 刷新总控面板（resolved 来自 respond
        // 命令与 telegram 按钮两条路；timeout 来自 acp.rs 的自动拒绝线程）。
        (_, Some(method)) if is_wait_cleanup(method) => {
            if let Some(request_id) = data.get("requestId").and_then(Value::as_str) {
                observe_wait_cleanup(slot, method, request_id);
            }
        }
        _ => {}
    }
}

/// 六类「等待解除」事件：permission/question/plan × resolved/timeout。
fn is_wait_cleanup(method: &str) -> bool {
    matches!(
        method,
        "permission/timeout"
            | "permission/resolved"
            | "question/timeout"
            | "question/resolved"
            | "plan/timeout"
            | "plan/resolved"
    )
}

/// 摘掉一条挂起请求：pending 表 + 该会话的 waiting 计数，然后刷面板。
fn observe_wait_cleanup(slot: &'static Telegram, method: &str, request_id: &str) {
    let owner_session = {
        let Ok(mut inner) = slot.inner.lock() else { return };
        let session_id = match method.split('/').next() {
            Some("permission") => inner
                .pending_permissions
                .remove(request_id)
                .map(|p| p.session_id),
            Some("question") => inner.pending_questions.remove(request_id),
            Some("plan") => inner.pending_plans.remove(request_id),
            _ => None,
        };
        if let Some(session_id) = session_id {
            let track = inner.track(&session_id);
            track.waiting = track.waiting.saturating_sub(1);
            Some(session_id)
        } else {
            None
        }
    };
    let _ = owner_session;
    schedule_dashboard(slot);
}

/// `session/update`：维护回合起点、末条回复尾部缓冲、最近用户会话。
fn observe_session_update(slot: &'static Telegram, session_id: &str, data: &Value) {
    let Some(update) = data.pointer("/params/update") else {
        return;
    };
    let kind = update_kind(update);
    if kind.is_empty() {
        return;
    }
    // 新回合开始（或等待解除后继续跑）是面板可见的状态变化。
    let mut turn_started_now = false;
    {
        let Ok(mut inner) = slot.inner.lock() else { return };

        if kind.contains("user_message") {
            inner.last_user_session = Some(session_id.to_string());
            let track = inner.track(session_id);
            turn_started_now = track.turn_started.is_none() || track.waiting == 0;
            track.turn_started = Some(Instant::now());
            track.last_activity = Instant::now();
            track.reply_tail.clear();
        } else {
            // 回合起点锚定在「首个活动」上：Grok 实测从不回显
            // user_message_chunk（2026-10-18 探针），只认用户回显的话这些
            // 会话永远没有时长、也算不上 Running；首个活动同时赢得裸文本
            // 路由（等于「你刚碰过它」）。
            let is_agent_text = kind.contains("agent_message")
                || kind.contains("assistant")
                || kind.contains("message_chunk")
                || kind == "message";
            let is_activity = is_agent_text
                || kind.contains("tool_call")
                || kind.contains("thought")
                || kind.contains("plan");
            if is_agent_text {
                let text = extract_update_text(update);
                if !text.is_empty() {
                    let mut started = false;
                    {
                        let track = inner.track(session_id);
                        if track.turn_started.is_none() {
                            track.turn_started = Some(Instant::now());
                            started = true;
                        }
                        track.last_activity = Instant::now();
                        append_tail(&mut track.reply_tail, &text, TURN_BUFFER_CHARS);
                    }
                    if started {
                        turn_started_now = true;
                        inner.last_user_session = Some(session_id.to_string());
                    }
                }
            } else if is_activity {
                let mut started = false;
                {
                    let track = inner.track(session_id);
                    if track.turn_started.is_none() {
                        track.turn_started = Some(Instant::now());
                        started = true;
                    }
                    track.last_activity = Instant::now();
                }
                if started {
                    turn_started_now = true;
                    inner.last_user_session = Some(session_id.to_string());
                }
            }
        }
    }
    if turn_started_now {
        schedule_dashboard(slot);
    }
}

fn observe_turn_complete(
    slot: &'static Telegram,
    session_id: &str,
    kind: &str,
    data: &Value,
    live: &LiveConfig,
) {
    let stop_reason = data
        .get("stopReason")
        .or_else(|| data.get("stop_reason"))
        .and_then(Value::as_str)
        .unwrap_or("end_turn");

    // 无论推不推，回合状态都就地清掉（缓冲不跨回合泄漏）。
    // tail 与 started 解耦：agent 不回显 user_message 时（Grok 实测）也照样带正文。
    let (elapsed, tail) = slot
        .inner
        .lock()
        .ok()
        .and_then(|mut inner| {
            let track = inner.track(session_id);
            let started = track.turn_started.take();
            let tail = std::mem::take(&mut track.reply_tail);
            Some((started.map(|t| t.elapsed()), tail))
        })
        .unwrap_or((None, String::new()));
    // 回合结束 = 总控面板的可见状态变化（含 cancelled：推送降噪，面板照刷）。
    schedule_dashboard(slot);

    if kind == "error" {
        if live.prefs.notify_error {
            let message = data
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    data.get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("回合报错")
                        .to_string()
                });
            push_simple(live, session_id, "⚠️", "回合出错", Some(&clip(&message, 600)));
        }
        return;
    }

    // 正常结束。cancelled 多半是本地手动打断，不推（降噪）。
    if !live.prefs.notify_turn || stop_reason == "cancelled" {
        return;
    }
    let (icon, head) = stop_reason_head(stop_reason);
    let label = session_label(session_id);
    let header = match elapsed {
        Some(elapsed) => format!("[{label}] {icon} {head} · {}", format_duration(elapsed)),
        None => format!("[{label}] {icon} {head}"),
    };
    let footer = "💬 直接回复这条消息 = 继续这个会话";
    let body = tail.trim();
    if live.prefs.content_preview && !body.is_empty() {
        enqueue_reply_push(live, &header, body, footer, session_id);
    } else {
        enqueue_send(
            live,
            format!("{header}\n\n{footer}"),
            Vec::new(),
            Some(session_id.to_string()),
            None,
        );
    }
}

/// 回合回复推送：MarkdownV2 全文分片。主消息带头部，末条带页脚与路由提示；
/// 每片独立成文（先切原文再转换），parse 失败由发送层回退纯文本兜底。
fn enqueue_reply_push(
    live: &LiveConfig,
    header: &str,
    body: &str,
    footer: &str,
    session_id: &str,
) {
    let mut chunks: Vec<String> = split_markdown_paragraphs(body, REPLY_CHUNK_CHARS)
        .into_iter()
        .flat_map(|chunk| fit_markdown_chunk(&chunk))
        .collect();
    let mut truncated = false;
    if chunks.len() > REPLY_MAX_MESSAGES {
        chunks.truncate(REPLY_MAX_MESSAGES);
        truncated = true;
    }
    let last = chunks.len().saturating_sub(1);
    for (index, chunk) in chunks.iter().enumerate() {
        let mut md = String::new();
        if index == 0 {
            md.push_str(&md_escape(header));
            md.push_str("\n\n");
        }
        md.push_str(&md_to_markdownv2(chunk));
        md.push('\n');
        if index == last {
            md.push_str("\n");
            md.push_str(&md_escape(footer));
            if truncated {
                md.push_str(&md_escape("\n…（正文太长已截断，回电脑看全文）"));
            }
        } else {
            md.push_str(&md_escape("…（接下条）"));
        }
        enqueue_send_markdown(live, md, Some(session_id.to_string()), None);
    }
}

fn observe_permission_prompt(
    slot: &'static Telegram,
    session_id: &str,
    data: &Value,
    live: &LiveConfig,
) {
    let Some(request_id) = data.get("requestId").and_then(Value::as_str) else {
        return;
    };
    // 状态追踪与推送偏好解耦：面板上的 ⏸ 不受 notify_wait 影响。
    {
        let Ok(mut inner) = slot.inner.lock() else { return };
        let track = inner.track(session_id);
        track.waiting += 1;
        inner.pending_permissions.insert(
            request_id.to_string(),
            PendingPermissionPush {
                session_id: session_id.to_string(),
            },
        );
    }
    schedule_dashboard(slot);
    if !live.prefs.notify_wait {
        return;
    }
    let title = data
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("需要批准");
    let detail = data.get("detail").and_then(Value::as_str);

    let label = session_label(session_id);
    let mut text = format!("[{label}] ⏸ 等你批准：{title}");
    if let Some(detail) = detail.filter(|d| !d.trim().is_empty()) {
        text.push_str("\n");
        text.push_str(&clip(&render_tool_detail(detail), 800));
    }

    // 选项 → 按钮（allow once / always / deny…）。太多或没有就退化为纯文本。
    let mut buttons = Vec::new();
    if let Some(options) = data.get("options").and_then(Value::as_array) {
        for option in options.iter().take(8) {
            let Some(option_id) = option
                .get("optionId")
                .or_else(|| option.get("option_id"))
                .and_then(Value::as_str)
            else {
                continue;
            };
            let option_label = option
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(option_id);
            let (button, callback_id, action) = make_button(
                slot,
                CallbackAction::Permission {
                    session_id: session_id.to_string(),
                    request_id: request_id.to_string(),
                    option_id: option_id.to_string(),
                    option_label: option_label.to_string(),
                },
                option_label,
            );
            buttons.push(button);
            if let Ok(mut inner) = slot.inner.lock() {
                inner.remember_callback(callback_id, action);
            }
        }
    }
    enqueue_send(live, text, buttons, Some(session_id.to_string()), None);

    // 到期前 30s 提醒（仍在 pending 才发）。
    let req = request_id.to_string();
    thread::spawn(move || {
        thread::sleep(Duration::from_secs(PERMISSION_REMINDER_AFTER_SECS));
        let Some(slot) = TELEGRAM.get() else { return };
        let live = slot.live.lock().ok().map(|l| l.clone());
        let Some(live) = live else { return };
        if !live.active() || !live.prefs.notify_wait {
            return;
        }
        let pending = slot
            .inner
            .lock()
            .ok()
            .and_then(|inner| inner.pending_permissions.get(&req).cloned());
        if let Some(pending) = pending {
            let label = session_label(&pending.session_id);
            enqueue_send(
                &live,
                format!("[{label}] ⏳ 这条批准再不回，30 秒后将自动拒绝"),
                Vec::new(),
                None,
                None,
            );
        }
    });
}

fn observe_question_prompt(
    slot: &'static Telegram,
    session_id: &str,
    data: &Value,
    live: &LiveConfig,
) {
    let Some(request_id) = data.get("requestId").and_then(Value::as_str) else {
        return;
    };
    {
        let Ok(mut inner) = slot.inner.lock() else { return };
        inner.track(session_id).waiting += 1;
        inner
            .pending_questions
            .insert(request_id.to_string(), session_id.to_string());
    }
    schedule_dashboard(slot);
    if !live.prefs.notify_wait {
        return;
    }
    let label = session_label(session_id);
    let questions = data.get("questions").and_then(Value::as_array).cloned();

    match question_push_plan(questions.as_ref()) {
        QuestionPush::Buttons { question, options } => {
            let text = format!("[{label}] ❓ agent 问你：{question}");
            let mut buttons = Vec::new();
            for option_label in options.iter().take(8) {
                let (button, callback_id, action) = make_button(
                    slot,
                    CallbackAction::Question {
                        session_id: session_id.to_string(),
                        request_id: request_id.to_string(),
                        // 应答按问题原文匹配（build_ask_user_question_result
                        // 用它当 key），绝不截断。
                        question: question.clone(),
                        selected: option_label.clone(),
                    },
                    option_label,
                );
                buttons.push(button);
                if let Ok(mut inner) = slot.inner.lock() {
                    inner.remember_callback(callback_id, action);
                }
            }
            // 「跳过」= declined（与 UI 的 Skip 同一条 wire）。
            let (button, callback_id, action) = make_button(
                slot,
                CallbackAction::QuestionDecline {
                    session_id: session_id.to_string(),
                    request_id: request_id.to_string(),
                },
                "跳过",
            );
            buttons.push(button);
            if let Ok(mut inner) = slot.inner.lock() {
                inner.remember_callback(callback_id, action);
            }
            enqueue_send(live, text, buttons, Some(session_id.to_string()), None);
        }
        QuestionPush::TextOnly { body } => {
            let text = format!("[{label}] ❓ agent 问你（多问题/多选，回电脑答）：\n{body}");
            enqueue_send(live, text, Vec::new(), Some(session_id.to_string()), None);
        }
    }
}

fn observe_plan_prompt(slot: &'static Telegram, session_id: &str, data: &Value, live: &LiveConfig) {
    let Some(request_id) = data.get("requestId").and_then(Value::as_str) else {
        return;
    };
    {
        let Ok(mut inner) = slot.inner.lock() else { return };
        inner.track(session_id).waiting += 1;
        inner
            .pending_plans
            .insert(request_id.to_string(), session_id.to_string());
    }
    schedule_dashboard(slot);
    if !live.prefs.notify_wait {
        return;
    }
    let plan = data
        .get("planMarkdown")
        .or_else(|| data.get("plan_markdown"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let label = session_label(session_id);
    let mut text = format!("[{label}] 📋 计划待审");
    if live.prefs.content_preview && !plan.trim().is_empty() {
        text.push_str("\n\n");
        text.push_str(&clip(plan.trim(), 1500));
    }
    let mut buttons = Vec::new();
    for (button_text, decision) in [("✅ 批准", "approve"), ("✏️ 继续改", "request_changes")] {
        let (button, callback_id, action) = make_button(
            slot,
            CallbackAction::Plan {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                decision: decision.to_string(),
                decision_label: button_text.to_string(),
            },
            button_text,
        );
        buttons.push(button);
        if let Ok(mut inner) = slot.inner.lock() {
            inner.remember_callback(callback_id, action);
        }
    }
    enqueue_send(live, text, buttons, Some(session_id.to_string()), None);
}

// ─── 总控面板（置顶 dashboard）─────────────────────────────────────────────

/// 面板最多列出的会话行数（防按钮/文本爆长；多余的并进「其余 N 空闲」）。
const DASHBOARD_MAX_ROWS: usize = 8;
/// /status 全量清单的行数上限（按最近动静排序截断）。
const STATUS_MAX_ROWS: usize = 20;
/// 面板心跳：有会话在跑时每 60s 重画一次（「跑了 N 分钟」保鲜）。
const DASHBOARD_TICK_SECS: u64 = 60;

/// 面板/清单里的一行。
#[derive(Clone, Debug)]
struct DashRow {
    session_id: String,
    label: String,
    agent: String,
    state: DashState,
    /// 回合已跑秒数（Running 才有意义）。
    elapsed_secs: Option<u64>,
    /// 最后一次动静距今秒数。
    idle_secs: u64,
    bound: bool,
}

/// 纯渲染：面板文本。行序 = Running → Waiting → 绑定。
fn render_dashboard(rows: &[DashRow], idle_count: usize) -> String {
    let running = rows.iter().filter(|r| r.state == DashState::Running).count();
    let waiting = rows.iter().filter(|r| r.state == DashState::Waiting).count();
    let mut head = String::from("📌 Marionette 总控");
    if running + waiting > 0 {
        head.push_str(&format!(" · ▶{running} ⏸{waiting}"));
    }
    let mut out = String::new();
    out.push_str(&head);
    out.push_str("\n\n");
    let mut lines = Vec::new();
    for row in rows {
        let marker = if row.bound { "⏎ " } else { "" };
        let agent = if row.agent.is_empty() {
            String::new()
        } else {
            format!(" · {}", row.agent)
        };
        let line = match row.state {
            DashState::Running => {
                let dur = row
                    .elapsed_secs
                    .map(format_duration_secs)
                    .unwrap_or_else(|| "…".to_string());
                format!("{marker}▶ [{}]{agent} · {dur}", row.label)
            }
            DashState::Waiting => format!("{marker}⏸ [{}] · 等你", row.label),
            DashState::Idle => format!(
                "{marker}💤 [{}] · {}前有动静",
                row.label,
                format_age_short(row.idle_secs)
            ),
        };
        lines.push(line);
    }
    if lines.is_empty() {
        out.push_str("💤 没有会话在跑\n");
    } else {
        out.push_str(&lines.join("\n"));
        out.push('\n');
    }
    if idle_count > 0 {
        out.push_str(&format!("\n💤 其余 {idle_count} 个会话空闲"));
    }
    out.push_str("\n💡 点会话名 = 切换裸发文字的去向 · /status 看全部");
    out
}

/// 纯渲染：/status 全量清单文本。
fn render_status(rows: &[DashRow], total: usize) -> String {
    let mut out = format!("📋 会话清单（{}/{}）\n\n", rows.len(), total);
    let lines: Vec<String> = rows
        .iter()
        .map(|row| {
            let marker = if row.bound { "⏎ " } else { "" };
            let tail = match row.state {
                DashState::Running => format!(
                    "跑了 {}",
                    row.elapsed_secs.map(format_duration_secs).unwrap_or_default()
                ),
                DashState::Waiting => "等你批准/回答".to_string(),
                DashState::Idle => format!("{}前有动静", format_age_short(row.idle_secs)),
            };
            let agent = if row.agent.is_empty() {
                String::new()
            } else {
                format!(" · {}", row.agent)
            };
            format!("{}{} [{}]{agent} · {tail}", marker, row.state.icon(), row.label)
        })
        .collect();
    out.push_str(&lines.join("\n"));
    out.push_str("\n\n💬 回复某条通知 = 定向送达；裸发文字 → ⏎ 标记的会话（总控面板上可点选）。");
    out
}

/// 快照 + 取名 + 组装 + 投递。绝不在持有 inner 锁时碰 storage/acp（锁序红线）。
fn schedule_dashboard(slot: &'static Telegram) {
    let live = slot.live.lock().ok().map(|l| l.clone());
    let Some(live) = live else { return };
    if !live.active() {
        return;
    }
    let (rows, idle_count) = dashboard_snapshot(slot, DASHBOARD_MAX_ROWS, false);
    let text = render_dashboard(&rows, idle_count);

    let mut keyboard: Vec<Vec<TgButton>> = Vec::new();
    let mut actions: Vec<(TgButton, u64, CallbackAction)> = Vec::new();
    for row in &rows {
        let (bind_btn, bind_id, bind_action) = make_button(
            slot,
            CallbackAction::BindSession {
                session_id: row.session_id.clone(),
                label: row.label.clone(),
            },
            &clip(&row.label, 26),
        );
        actions.push((bind_btn.clone(), bind_id, bind_action));
        let mut line = vec![bind_btn];
        if row.state != DashState::Idle {
            let (stop_btn, stop_id, stop_action) = make_button(
                slot,
                CallbackAction::StopTurn {
                    session_id: row.session_id.clone(),
                    label: row.label.clone(),
                },
                "⏹",
            );
            actions.push((stop_btn.clone(), stop_id, stop_action));
            line.push(stop_btn);
        }
        keyboard.push(line);
    }
    let (refresh_btn, refresh_id, refresh_action) =
        make_button(slot, CallbackAction::RefreshDashboard, "🔄 刷新");
    actions.push((refresh_btn.clone(), refresh_id, refresh_action));
    keyboard.push(vec![refresh_btn]);

    if let Ok(mut inner) = slot.inner.lock() {
        for (button, id, action) in actions {
            let _ = button;
            inner.remember_callback(id, action);
        }
    }
    let _ = slot.outbound.send(Outbound::Dashboard { text, keyboard });
}

/// inner 快照 → 行数据（标签/agent 事后另行查询，避免锁序纠缠）。
/// `include_idle=false` 只留非空闲 + 绑定（总控面板用）；
/// `true` 全列（/status 用）。返回 (行, 未列出的空闲会话数)。
fn dashboard_snapshot(
    slot: &'static Telegram,
    max_rows: usize,
    include_idle: bool,
) -> (Vec<DashRow>, usize) {
    let raw = slot.inner.lock().ok().map(|inner| {
        (
            inner
                .sessions
                .iter()
                .map(|(id, track)| {
                    (
                        id.clone(),
                        track.state(),
                        track.turn_started.map(|t| t.elapsed().as_secs()),
                        track.last_activity.elapsed().as_secs(),
                    )
                })
                .collect::<Vec<_>>(),
            inner.last_user_session.clone(),
        )
    });
    let Some((raw_rows, bound)) = raw else { return (Vec::new(), 0) };

    let idle_count = raw_rows
        .iter()
        .filter(|(_, state, _, _)| *state == DashState::Idle)
        .count();
    let mut rows: Vec<DashRow> = Vec::new();
    for (session_id, state, elapsed_secs, idle_secs) in raw_rows {
        let is_bound = bound.as_deref() == Some(session_id.as_str());
        if state == DashState::Idle && !is_bound && !include_idle {
            continue;
        }
        rows.push(DashRow {
            session_id,
            label: String::new(),
            agent: String::new(),
            state,
            elapsed_secs,
            idle_secs,
            bound: is_bound,
        });
    }
    // 排序：Running → Waiting → 绑定；同类按最近动静在前。
    rows.sort_by(|a, b| {
        state_rank(a.state)
            .cmp(&state_rank(b.state))
            .then(b.idle_secs.cmp(&a.idle_secs))
    });
    rows.truncate(max_rows);
    for row in &mut rows {
        let meta = session_meta(&row.session_id);
        row.label = meta.label;
        row.agent = meta.agent;
    }
    (rows, idle_count)
}

fn state_rank(state: DashState) -> u8 {
    match state {
        DashState::Running => 0,
        DashState::Waiting => 1,
        DashState::Idle => 2,
    }
}

fn format_duration_secs(secs: u64) -> String {
    format_duration(Duration::from_secs(secs))
}

/// 闲置年龄（「N前有动静」）：分钟/小时取整即可，秒级精确反而啰嗦。
fn format_age_short(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}秒")
    } else if secs < 3600 {
        format!("{}分钟", secs / 60)
    } else if secs < 86400 {
        format!("{}小时{}分", secs / 3600, secs % 3600 / 60)
    } else {
        format!("{}天", secs / 86400)
    }
}

// ─── 组装按钮 / 发送 ───────────────────────────────────────────────────────

/// 分配 callback 短 id，返回 (按钮, id, 完整动作)。id 与动作成对登记。
fn make_button(
    slot: &'static Telegram,
    action: CallbackAction,
    label: &str,
) -> (TgButton, u64, CallbackAction) {
    let id = slot.callback_seq.fetch_add(1, Ordering::Relaxed);
    (
        TgButton {
            text: clip(label, 48),
            callback_id: id,
        },
        id,
        action,
    )
}

fn push_simple(live: &LiveConfig, session_id: &str, icon: &str, head: &str, detail: Option<&str>) {
    let label = session_label(session_id);
    let mut text = format!("[{label}] {icon} {head}");
    if let Some(detail) = detail.filter(|d| !d.trim().is_empty()) {
        text.push('：');
        text.push_str(&clip(detail.trim(), 600));
    }
    enqueue_send(live, text, Vec::new(), Some(session_id.to_string()), None);
}

/// 投递到发送线程。fire-and-forget：通道断了（进程退出）就直接丢，
/// 绝不反压事件收口。
fn enqueue_send(
    live: &LiveConfig,
    text: String,
    buttons: Vec<TgButton>,
    route_for_session: Option<String>,
    reply_to: Option<i64>,
) {
    let _ = live;
    let Some(slot) = TELEGRAM.get() else { return };
    let _ = slot.outbound.send(Outbound::Send {
        text,
        buttons,
        route_for_session,
        reply_to,
        md: false,
    });
}

/// 同上，但以 MarkdownV2 发送（回合回复正文用）。
fn enqueue_send_markdown(
    live: &LiveConfig,
    text: String,
    route_for_session: Option<String>,
    reply_to: Option<i64>,
) {
    let _ = live;
    let Some(slot) = TELEGRAM.get() else { return };
    let _ = slot.outbound.send(Outbound::Send {
        text,
        buttons: Vec::new(),
        route_for_session,
        reply_to,
        md: true,
    });
}

// ─── 发送线程 ──────────────────────────────────────────────────────────────

fn sender_loop(slot: &'static Telegram, rx: std::sync::mpsc::Receiver<Outbound>) {
    let mut backoff = BACKOFF_MIN;
    while let Ok(job) = rx.recv() {
        let live = slot.live.lock().ok().map(|l| l.clone());
        let Some(live) = live else { continue };
        if !live.active() {
            continue;
        }
        let job_text;
        let job_keyboard;
        let mut route_for_session: Option<String> = None;
        let mut reply_to: Option<i64> = None;
        let mut parse_mode: Option<&'static str> = None;
        let mut is_dashboard = false;
        match job {
            Outbound::Send {
                text,
                buttons,
                route_for_session: route,
                reply_to: reply,
                md,
            } => {
                job_text = text;
                job_keyboard = if buttons.is_empty() {
                    Vec::new()
                } else {
                    vec![buttons]
                };
                route_for_session = route;
                reply_to = reply;
                if md {
                    parse_mode = Some("MarkdownV2");
                }
            }
            Outbound::Ack {
                ok,
                error,
                session_label,
                reply_to: reply,
            } => {
                job_text = if ok {
                    format!("✓ 已送进 [{session_label}]")
                } else {
                    format!(
                        "⚠️ 没送进去（{session_label}）：{}",
                        clip(&error.unwrap_or_default(), 400)
                    )
                };
                job_keyboard = Vec::new();
                reply_to = reply;
            }
            Outbound::Dashboard { text, keyboard } => {
                job_text = text;
                job_keyboard = keyboard;
                is_dashboard = true;
            }
        };

        if is_dashboard {
            deliver_dashboard(&live, &job_text, &job_keyboard);
            continue;
        }

        match send_message(&live, &job_text, &job_keyboard, reply_to, parse_mode) {
            Ok(message_id) => {
                backoff = BACKOFF_MIN;
                if let (Some(session_id), Some(message_id)) = (route_for_session, message_id) {
                    if let Ok(mut inner) = slot.inner.lock() {
                        inner.remember_route(message_id, &session_id);
                    }
                }
            }
            Err(error) => {
                log("warn", "sendMessage failed — queue keeps draining", Some(&error));
                if let Ok(mut inner) = slot.inner.lock() {
                    inner.last_error = Some(error);
                }
                thread::sleep(backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

/// 面板投递：已有 message_id 就原地 edit；edit 失败（消息被删等）就发新 + 置顶。
fn deliver_dashboard(live: &LiveConfig, text: &str, keyboard: &[Vec<TgButton>]) {
    let Some(slot) = TELEGRAM.get() else { return };
    let existing = slot
        .inner
        .lock()
        .ok()
        .and_then(|mut inner| inner.dashboard.take().map(|handle| handle.message_id));
    if let Some(message_id) = existing {
        match edit_message(live, message_id, text, keyboard) {
            Ok(()) => {
                if let Ok(mut inner) = slot.inner.lock() {
                    inner.dashboard = Some(DashboardHandle { message_id });
                }
                return;
            }
            // 内容没变被 Telegram 拒：同样是成功，别触发重发。
            Err(ref error) if error.contains("message is not modified") => {
                if let Ok(mut inner) = slot.inner.lock() {
                    inner.dashboard = Some(DashboardHandle { message_id });
                }
                return;
            }
            Err(error) => {
                // 面板被用户删了或 edit 被拒：发一条新的顶上。
                log("warn", "dashboard edit failed — resending", Some(&error));
            }
        }
    }
    match send_message(live, text, keyboard, None, None) {
        Ok(Some(message_id)) => {
            if let Ok(mut inner) = slot.inner.lock() {
                inner.dashboard = Some(DashboardHandle { message_id });
            }
            if let Err(error) = pin_message(live, message_id) {
                log("info", "dashboard pin skipped", Some(&error));
            }
        }
        Ok(None) => {}
        Err(error) => {
            log("warn", "dashboard send failed", Some(&error));
            if let Ok(mut inner) = slot.inner.lock() {
                inner.last_error = Some(error);
            }
        }
    }
}

/// 发送一条消息；成功返回 Telegram 分配的 message_id。
/// `keyboard` 为 inline_keyboard 的行分组（空 = 无按钮）。
/// MarkdownV2 发送失败（解析拒绝、转义后超长……）时自动回退纯文本重发
/// 一次——格式化是增益，投递是底线。
fn send_message(
    live: &LiveConfig,
    text: &str,
    keyboard: &[Vec<TgButton>],
    reply_to: Option<i64>,
    parse_mode: Option<&str>,
) -> Result<Option<i64>, String> {
    match send_message_inner(live, text, keyboard, reply_to, parse_mode) {
        Err(ref error) if parse_mode.is_some() => {
            log("warn", "MarkdownV2 send failed — resending as plain text", Some(error));
            send_message_inner(live, text, keyboard, reply_to, None)
        }
        result => result,
    }
}

fn send_message_inner(
    live: &LiveConfig,
    text: &str,
    keyboard: &[Vec<TgButton>],
    reply_to: Option<i64>,
    parse_mode: Option<&str>,
) -> Result<Option<i64>, String> {
    let Some(token) = live.token.as_deref() else {
        return Err("no token".to_string());
    };
    let Some(chat_id) = live.chat_id else {
        return Err("no paired chat".to_string());
    };
    let mut body = json!({
        "chat_id": chat_id,
        // 先按 chars 截再按 UTF-16 码元闸：4096 硬顶按码元计。
        "text": clip_utf16(&clip(text, MAX_MESSAGE_CHARS), 4000),
    });
    if let Some(reply_to) = reply_to {
        body["reply_to_message_id"] = json!(reply_to);
    }
    if let Some(parse_mode) = parse_mode {
        body["parse_mode"] = json!(parse_mode);
    }
    if !keyboard.is_empty() {
        body["reply_markup"] = reply_markup(keyboard);
    }
    let value = tg_call(
        token,
        "sendMessage",
        &body,
        Duration::from_secs(SEND_HTTP_TIMEOUT_SECS),
    )?;
    Ok(value.pointer("/result/message_id").and_then(Value::as_i64))
}

/// 原地编辑面板消息（文本 + 键盘一起换）。
fn edit_message(
    live: &LiveConfig,
    message_id: i64,
    text: &str,
    keyboard: &[Vec<TgButton>],
) -> Result<(), String> {
    let Some(token) = live.token.as_deref() else {
        return Err("no token".to_string());
    };
    let Some(chat_id) = live.chat_id else {
        return Err("no paired chat".to_string());
    };
    let mut body = json!({
        "chat_id": chat_id,
        "message_id": message_id,
        "text": clip(text, MAX_MESSAGE_CHARS),
    });
    if !keyboard.is_empty() {
        body["reply_markup"] = reply_markup(keyboard);
    }
    tg_call(
        token,
        "editMessageText",
        &body,
        Duration::from_secs(SEND_HTTP_TIMEOUT_SECS),
    )?;
    Ok(())
}

/// 置顶面板消息（私聊无需额外权限；静音，不响铃）。
fn pin_message(live: &LiveConfig, message_id: i64) -> Result<(), String> {
    let Some(token) = live.token.as_deref() else {
        return Err("no token".to_string());
    };
    let Some(chat_id) = live.chat_id else {
        return Err("no paired chat".to_string());
    };
    tg_call(
        token,
        "pinChatMessage",
        &json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "disable_notification": true,
        }),
        Duration::from_secs(SEND_HTTP_TIMEOUT_SECS),
    )?;
    Ok(())
}

/// 组 `reply_markup` 的整体值：`{"inline_keyboard": [[…]]}`。
/// Telegram 要求 reply_markup 是对象壳——裸数组直接 400
/// （can't parse reply keyboard markup），面板会一条都发不出去。
fn reply_markup(keyboard: &[Vec<TgButton>]) -> Value {
    json!({
        "inline_keyboard": keyboard
            .iter()
            .map(|row| {
                Value::Array(
                    row.iter()
                        .map(|b| {
                            json!({ "text": b.text, "callback_data": b.callback_id.to_string() })
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>(),
    })
}

// ─── 轮询线程 ──────────────────────────────────────────────────────────────

fn poller_loop(slot: &'static Telegram) {
    let mut offset: i64 = 0;
    let mut backoff = BACKOFF_MIN;
    loop {
        let live = slot.live.lock().ok().map(|l| l.clone());
        let Some(live) = live else {
            thread::sleep(Duration::from_secs(5));
            continue;
        };
        // 有 token 就轮询：未配对时也要能收到第一条消息（配对依赖它）。
        let Some(token) = live.token.clone() else {
            thread::sleep(Duration::from_secs(2));
            continue;
        };

        let url = format!(
            "{API_BASE}/bot{token}/getUpdates?timeout={POLL_TIMEOUT_SECS}&offset={offset}&allowed_updates=%5B%22message%22%2C%22callback_query%22%5D"
        );
        let response = crate::http_client::agent()
            .and_then(|agent| {
                agent
                    .get(&url)
                    .timeout(Duration::from_secs(POLL_HTTP_TIMEOUT_SECS))
                    .call()
                    .map_err(|e| e.to_string())
            })
            .and_then(|resp| {
                resp.into_string()
                    .map_err(|e| format!("read getUpdates body: {e}"))
            })
            .and_then(|text| {
                serde_json::from_str::<Value>(&text)
                    .map_err(|e| format!("parse getUpdates: {e}"))
            });

        match response {
            Ok(value) => {
                backoff = BACKOFF_MIN;
                if let Ok(mut inner) = slot.inner.lock() {
                    inner.last_error = None;
                }
                if value.get("ok").and_then(Value::as_bool) != Some(true) {
                    // API 层错误对象（409 之外的 4xx 也长这样）。
                    let desc = value
                        .pointer("/description")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    log("warn", "getUpdates returned not-ok", Some(desc));
                    thread::sleep(BACKOFF_409_START);
                    continue;
                }
                let updates = value
                    .get("result")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for update in updates {
                    if let Some(update_id) = update.get("update_id").and_then(Value::as_i64) {
                        offset = offset.max(update_id + 1);
                    }
                    handle_update(slot, &live, update);
                }
            }
            Err(error) => {
                let is_conflict = error.contains("409") || error.contains("Conflict");
                if let Ok(mut inner) = slot.inner.lock() {
                    inner.last_error = Some(error.clone());
                }
                log(
                    "warn",
                    if is_conflict {
                        "getUpdates 409 — another poller owns this token; backing off"
                    } else {
                        "getUpdates failed — backing off"
                    },
                    Some(&error),
                );
                let start = if is_conflict {
                    BACKOFF_409_START
                } else {
                    backoff
                };
                thread::sleep(start);
                backoff = (backoff.max(start) * 2).min(BACKOFF_MAX);
            }
        }
    }
}

fn handle_update(slot: &'static Telegram, live: &LiveConfig, update: Value) {
    if let Some(message) = update.get("message").cloned() {
        handle_incoming_message(slot, live, message);
    } else if let Some(callback) = update.get("callback_query").cloned() {
        handle_callback_query(slot, live, callback);
    }
}

fn handle_incoming_message(slot: &'static Telegram, live: &LiveConfig, message: Value) {
    let Some(chat_id) = message.pointer("/chat/id").and_then(Value::as_i64) else {
        return;
    };
    // 只认私聊（群/频道一律忽略）。
    if message.pointer("/chat/type").and_then(Value::as_str) != Some("private") {
        return;
    }
    let username = message
        .pointer("/from/username")
        .and_then(Value::as_str)
        .unwrap_or("");
    let first_name = message
        .pointer("/from/first_name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let text = message
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();

    match live.chat_id {
        Some(paired) if paired == chat_id => {}
        Some(_) => {
            // 配对过的人之外：静默丢弃，记一条 warn（白名单红线）。
            log(
                "warn",
                "dropped message from unpaired chat",
                Some(&format!("@{username} ({first_name})")),
            );
            return;
        }
        None => {
            // 未配对：挂起第一条消息作为配对请求，等设置浮层里确认。
            if text.is_empty() {
                return;
            }
            if let Ok(mut inner) = slot.inner.lock() {
                let fresh = inner
                    .pending_pairing
                    .as_ref()
                    .map_or(true, |p| p.received_at.elapsed() > PAIRING_TTL);
                if fresh {
                    inner.pending_pairing = Some(PairingRequest {
                        chat_id,
                        username: username.to_string(),
                        first_name: first_name.to_string(),
                        received_at: Instant::now(),
                    });
                    log(
                        "info",
                        "pairing request held for UI confirmation",
                        Some(&format!("@{username} ({first_name})")),
                    );
                }
            }
            return;
        }
    }

    if text.is_empty() || !live.active() {
        return;
    }
    let message_id = message.get("message_id").and_then(Value::as_i64);

    // 命令面：/status 拉全量清单；其余命令回用法提示。
    if text.starts_with('/') {
        let command = text
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        match command.as_str() {
            "/status" | "/sessions" => {
                let (rows, _) = dashboard_snapshot(slot, STATUS_MAX_ROWS, true);
                // 全量清单也把空闲会话列出来（snapshot 默认只留非空闲+绑定）。
                let total = slot
                    .inner
                    .lock()
                    .map(|inner| inner.sessions.len())
                    .unwrap_or(rows.len());
                let _ = send_message(
                    live,
                    &render_status(&rows, total),
                    &[],
                    message_id,
                    None,
                );
                schedule_dashboard(slot);
            }
            _ => {
                let _ = send_message(
                    live,
                    "用法：直接发文字 → 送进 ⏎ 当前绑定的会话；回复某条通知 → 定到那个会话；\n/status → 全部会话的状态清单。顶部的「总控」面板实时显示谁在跑、谁在等你，点会话名即可切换绑定。",
                    &[],
                    message_id,
                    None,
                );
            }
        }
        return;
    }

    // 引用回复 → 被引消息所属会话；裸文本 → 最近用户会话。
    let target = message
        .pointer("/reply_to_message/message_id")
        .and_then(Value::as_i64)
        .and_then(|replied| {
            slot.inner
                .lock()
                .ok()
                .and_then(|inner| inner.routes.get(&replied).cloned())
        })
        .or_else(|| {
            slot.inner
                .lock()
                .ok()
                .and_then(|inner| inner.last_user_session.clone())
        });

    let Some(session_id) = target else {
        let _ = send_message(
            live,
            "还没有可以投递的会话——先在电脑上开一个对话框，或回复某条通知来指定。",
            &[],
            message_id,
            None,
        );
        return;
    };

    log(
        "info",
        "inbound text routed to session",
        Some(&format!("{session_id} · {} chars", text.len())),
    );
    let _ = slot.app.emit_to(
        "main",
        INBOUND_EVENT,
        json!({
            "sessionId": session_id,
            "text": text,
            "messageId": message_id,
        }),
    );
}

fn handle_callback_query(slot: &'static Telegram, live: &LiveConfig, callback: Value) {
    let Some(callback_id) = callback.get("id").and_then(Value::as_str) else {
        return;
    };
    let answer = |text: &str| {
        if let Some(token) = live.token.as_deref() {
            let _ = tg_call(
                token,
                "answerCallbackQuery",
                &json!({ "callback_query_id": callback_id, "text": clip(text, 190) }),
                Duration::from_secs(SEND_HTTP_TIMEOUT_SECS),
            );
        }
    };

    let Some(data) = callback.get("data").and_then(Value::as_str) else {
        answer("无法解析按钮");
        return;
    };
    let Ok(id) = data.parse::<u64>() else {
        answer("无法解析按钮");
        return;
    };
    let Some(action) = slot
        .inner
        .lock()
        .ok()
        .and_then(|mut inner| inner.callbacks.remove(&id))
    else {
        answer("这条已处理或已过期");
        return;
    };

    let result = match &action {
        CallbackAction::Permission {
            session_id,
            request_id,
            option_id,
            option_label,
        } => slot
            .acp
            .respond_permission(request_id, option_id)
            .map(|_| {
                emit_resolved(slot, session_id, "permission/resolved", request_id);
                format!("已选：{option_label}")
            }),
        CallbackAction::Question {
            session_id,
            request_id,
            question,
            selected,
        } => {
            let answers = json!([{ "question": question, "selected": [selected] }]);
            slot.acp
                .respond_question(request_id, answers, false)
                .map(|_| {
                    emit_resolved(slot, session_id, "question/resolved", request_id);
                    format!("已选：{selected}")
                })
        }
        CallbackAction::QuestionDecline {
            session_id,
            request_id,
        } => slot
            .acp
            .respond_question(request_id, json!([]), true)
            .map(|_| {
                emit_resolved(slot, session_id, "question/resolved", request_id);
                "已跳过".to_string()
            }),
        CallbackAction::Plan {
            session_id,
            request_id,
            decision,
            decision_label,
        } => slot
            .acp
            .respond_plan_approval(request_id, decision, None)
            .map(|_| {
                emit_resolved(slot, session_id, "plan/resolved", request_id);
                decision_label.clone()
            }),
        CallbackAction::BindSession { session_id, label } => {
            if let Ok(mut inner) = slot.inner.lock() {
                inner.last_user_session = Some(session_id.clone());
            }
            schedule_dashboard(slot);
            Ok(format!("⏎ 裸发文字将送进 [{label}]"))
        }
        CallbackAction::StopTurn { session_id, label } => slot
            .acp
            .cancel(session_id)
            .map(|()| format!("⏹ 已请求停止 [{label}]")),
        CallbackAction::RefreshDashboard => {
            schedule_dashboard(slot);
            Ok("已刷新".to_string())
        }
    };

    match result {
        Ok(label) => {
            answer(&label);
            // 摘掉按钮，防止重复点击。面板消息除外：它的键盘由下一次
            // 面板编辑整体重画，就地摘除会把整块面板按钮抹掉。
            let dashboard_message = slot
                .inner
                .lock()
                .ok()
                .and_then(|inner| inner.dashboard.map(|handle| handle.message_id));
            let pressed_message = callback.pointer("/message/message_id").and_then(Value::as_i64);
            if let Some(message_id) = pressed_message {
                if Some(message_id) != dashboard_message {
                    if let Some(chat_id) = live.chat_id {
                        if let Some(token) = live.token.as_deref() {
                            let _ = tg_call(
                                token,
                                "editMessageReplyMarkup",
                                &json!({
                                    "chat_id": chat_id,
                                    "message_id": message_id,
                                    "reply_markup": { "inline_keyboard": [] },
                                }),
                                Duration::from_secs(SEND_HTTP_TIMEOUT_SECS),
                            );
                        }
                    }
                }
            }
            log(
                "info",
                "remote resolve ok",
                Some(&format!("{} · {label}", action.session_id())),
            );
        }
        Err(error) => {
            answer(&format!("没成：{}", clip(&error, 160)));
            log(
                "warn",
                "remote resolve failed",
                Some(&format!("{} · {error}", action.session_id())),
            );
        }
    }
}

/// 远程解答成功后通知 UI 收卡片（与 timeout 事件同形；前端幂等清理）。
fn emit_resolved(slot: &'static Telegram, session_id: &str, method: &str, request_id: &str) {
    crate::acp::emit_ui_event(
        &slot.app,
        session_id,
        "system",
        Some(method),
        json!({ "requestId": request_id }),
    );
}

// ─── Telegram HTTP ─────────────────────────────────────────────────────────

/// POST JSON；非 ok 一律带上下文报错。token 绝不进错误文本（url 不外泄）。
fn tg_call(token: &str, method: &str, body: &Value, timeout: Duration) -> Result<Value, String> {
    if token.is_empty() {
        return Err("no token".to_string());
    }
    let url = format!("{API_BASE}/bot{token}/{method}");
    let agent = crate::http_client::agent()?;
    let response = agent
        .post(&url)
        .timeout(timeout)
        .send_json(body)
        .map_err(|error| match error {
            ureq::Error::Status(code, response) => {
                // 错误体里的 description 是 400 的唯一线索（字段名、超长、
                // markup 形状……），绝不能丢。
                let desc = response
                    .into_string()
                    .ok()
                    .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                    .and_then(|value| {
                        value
                            .pointer("/description")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    })
                    .unwrap_or_default();
                if desc.is_empty() {
                    format!("HTTP {code} · {method}")
                } else {
                    format!("HTTP {code} · {method}: {desc}")
                }
            }
            other => format!("{method}: {other}"),
        })?;
    let status = response.status();
    let text = response
        .into_string()
        .map_err(|e| format!("read {method} body: {e}"))?;
    let value: Value =
        serde_json::from_str(&text).map_err(|e| format!("parse {method} response: {e}"))?;
    if value.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(value)
    } else {
        let desc = value
            .pointer("/description")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        Err(format!("HTTP {status} · {method}: {desc}"))
    }
}

/// `getMe`（保存设置时校验 token 用）。返回 bot 的 @用户名。
fn verify_token(token: &str) -> Result<String, String> {
    let value = tg_call(token, "getMe", &json!({}), Duration::from_secs(10))?;
    value
        .pointer("/result/username")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| "getMe 响应里没有 username".to_string())
}

// ─── 纯函数（组消息 / 路由决策 / 解析）─────────────────────────────────────

fn stop_reason_head(reason: &str) -> (&'static str, &'static str) {
    match reason {
        "max_tokens" => ("✂️", "到达长度上限"),
        "refusal" => ("⚠️", "模型拒答"),
        "max_turn_requests" => ("⏹", "达到回合上限"),
        _ => ("✅", "跑完"),
    }
}

/// `session/update` 的 kind（`sessionUpdate` | `type` | `kind`，对齐 acpTranscript）。
fn update_kind(update: &Value) -> String {
    update
        .get("sessionUpdate")
        .or_else(|| update.get("type"))
        .or_else(|| update.get("kind"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// 从 update 里提取文本（content / delta / text / message，含 text block 数组）。
fn extract_update_text(update: &Value) -> String {
    for field in ["delta", "content", "text", "message"] {
        if let Some(value) = update.get(field) {
            let text = text_of_value(value);
            if !text.is_empty() {
                return text;
            }
        }
    }
    String::new()
}

fn text_of_value(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        // 单个 content block（{type:"text", text:"…"}；无 type 的宽松形态也收）。
        Value::Object(block) => {
            let is_text = block.get("type").and_then(Value::as_str) != Some("image")
                && block.contains_key("text");
            is_text
                .then(|| block.get("text").and_then(Value::as_str).map(str::to_string))
                .flatten()
                .unwrap_or_default()
        }
        Value::Array(items) => items
            .iter()
            .filter_map(|item| {
                let block = item.as_object()?;
                if block.get("type").and_then(Value::as_str) == Some("text")
                    || !block.contains_key("type")
                {
                    block.get("text").and_then(Value::as_str).map(str::to_string)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// ask 卡片的推送形态：单问题单选 → 按钮；否则 → 纯文本（回电脑答）。
#[derive(Debug)]
enum QuestionPush {
    Buttons {
        question: String,
        options: Vec<String>,
    },
    TextOnly {
        body: String,
    },
}

fn question_push_plan(questions: Option<&Vec<Value>>) -> QuestionPush {
    let Some(questions) = questions else {
        return QuestionPush::TextOnly {
            body: "（问题内容没能解析）".to_string(),
        };
    };
    let mut any_multi = false;
    let mut parsed: Vec<(String, Vec<String>)> = Vec::new();
    for question in questions {
        let text = question
            .get("question")
            .and_then(Value::as_str)
            .unwrap_or("请选择");
        let multi = question.get("multiSelect").and_then(Value::as_bool) == Some(true)
            || question.get("multi_select").and_then(Value::as_bool) == Some(true);
        any_multi = any_multi || multi;
        let options = question
            .get("options")
            .and_then(Value::as_array)
            .map(|options| {
                options
                    .iter()
                    .filter_map(|o| o.get("label").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        parsed.push((text.to_string(), options));
    }
    let Some((question, options)) = parsed.first().cloned() else {
        return QuestionPush::TextOnly {
            body: "（没有问题）".to_string(),
        };
    };
    if parsed.len() == 1 && !any_multi {
        if !options.is_empty() {
            return QuestionPush::Buttons { question, options };
        }
        // 单问题但没有选项：开放式问题，提示用文字回复。
        return QuestionPush::TextOnly {
            body: format!("{question}\n（开放式问题：直接回复文字即可送进会话）"),
        };
    }
    let body = parsed
        .iter()
        .map(|(q, options)| {
            if options.is_empty() {
                q.clone()
            } else {
                format!("{q}\n  选项：{}", options.join(" / "))
            }
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    QuestionPush::TextOnly { body }
}

/// 权限推送的 detail：acp 侧给的是工具入参的原始 JSON 字符串
/// （`permission_summary` 里 `rawInput.to_string()`）。能 parse 就渲染成
/// `key: value` 行，parse 不了就原样返回（plan markdown 等长文走原样）。
fn render_tool_detail(detail: &str) -> String {
    let trimmed = detail.trim();
    if !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
        return trimmed.to_string();
    }
    match serde_json::from_str::<Value>(trimmed) {
        Ok(value) => json_detail_lines(&value),
        Err(_) => trimmed.to_string(),
    }
}

/// JSON 入参 → 紧凑的 `key: value` 行。字符串去引号，数组逗号连接，
/// 嵌套对象保持单行紧凑 JSON（工具入参没有更深的人话可翻）。
fn json_detail_lines(value: &Value) -> String {
    match value {
        Value::Object(map) => map
            .iter()
            .map(|(key, val)| format!("{key}: {}", scalar_detail(val)))
            .collect::<Vec<_>>()
            .join("\n"),
        Value::Array(items) => items
            .iter()
            .map(scalar_detail)
            .collect::<Vec<_>>()
            .join(", "),
        other => scalar_detail(other),
    }
}

fn scalar_detail(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "—".to_string(),
        Value::Array(items) => items
            .iter()
            .map(scalar_detail)
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}

// ─── Markdown → Telegram MarkdownV2 ────────────────────────────────────────

/// MarkdownV2 实体外保留字（含转义符自身）全部加反斜杠。
fn md_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        if matches!(
            ch,
            '_' | '*' | '[' | ']' | '(' | ')' | '~' | '`' | '>' | '#' | '+' | '-' | '=' | '|'
                | '{' | '}' | '.' | '!' | '\\'
        ) {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// 链接 URL 部分只转义 ')' 与 '\'（Telegram 规定；其余原样，转义会进 URL）。
fn md_escape_url(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == ')' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// code/pre 实体内只需转义反引号与反斜杠。
fn md_code_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if ch == '`' || ch == '\\' {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

fn find_char(chars: &[char], from: usize, needle: char) -> Option<usize> {
    (from..chars.len()).find(|&i| chars[i] == needle)
}

/// 找 [text](url)；URL 含空白视为普通文本。
fn find_link(chars: &[char], open: usize) -> Option<(usize, usize, usize)> {
    let close = find_char(chars, open + 1, ']')?;
    let url_start = close + 2;
    if url_start >= chars.len() || chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let url_end = find_char(chars, url_start, ')')?;
    let url: String = chars[url_start..url_end].iter().collect();
    if url.chars().any(char::is_whitespace) {
        return None;
    }
    Some((close, url_start, url_end))
}

/// 行内转换：`code`、**bold**、*italic*/_italic_（内部含空白不认，避免
/// 误伤 file_name_x 与 2*3*4）、[text](url)；其余逐字转义。
fn md_inline_chars(chars: &[char]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '`' {
            if let Some(end) = find_char(chars, i + 1, '`') {
                out.push('`');
                out.push_str(&md_code_escape(
                    &chars[i + 1..end].iter().collect::<String>(),
                ));
                out.push('`');
                i = end + 1;
                continue;
            }
        }
        if ch == '*' && chars.get(i + 1) == Some(&'*') {
            // 闭合也找成对的 **（单个 * 是斜体/字面量，不能吞）。
            if let Some(end) = (i + 2..chars.len().saturating_sub(1)).find(|&p| {
                chars[p] == '*' && chars.get(p + 1) == Some(&'*')
            }) {
                if end > i + 2 {
                    out.push('*');
                    out.push_str(&md_escape(
                        &chars[i + 2..end].iter().collect::<String>(),
                    ));
                    out.push('*');
                    i = end + 2;
                    continue;
                }
            }
        }
        if ch == '*' || ch == '_' {
            if let Some(end) = find_char(chars, i + 1, ch) {
                let inner: String = chars[i + 1..end].iter().collect();
                // CommonMark：词内下划线不作强调（file_name_x 字面量）；
                // 星号允许词内（2*3*4 → 斜体 3）。
                let boundary_ok = ch == '*'
                    || ((i == 0 || !chars[i - 1].is_alphanumeric())
                        && (end + 1 >= chars.len() || !chars[end + 1].is_alphanumeric()));
                if end > i + 1 && !inner.chars().any(char::is_whitespace) && boundary_ok {
                    out.push('_');
                    out.push_str(&md_escape(&inner));
                    out.push('_');
                    i = end + 1;
                    continue;
                }
            }
        }
        if ch == '[' {
            if let Some((text_end, url_start, url_end)) = find_link(chars, i) {
                out.push('[');
                out.push_str(&md_escape(
                    &chars[i + 1..text_end].iter().collect::<String>(),
                ));
                out.push_str("](");
                out.push_str(&md_escape_url(
                    &chars[url_start..url_end].iter().collect::<String>(),
                ));
                out.push(')');
                i = url_end + 1;
                continue;
            }
        }
        out.push_str(&md_escape(&ch.to_string()));
        i += 1;
    }
    out
}

/// 单行转换；标题行（前导 # + 空格）整行加粗。
fn md_inline_line(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut head = 0;
    while head < chars.len() && chars[head] == '#' {
        head += 1;
    }
    if head > 0 && chars.get(head) == Some(&' ') {
        let mut out = String::from("*");
        out.push_str(&md_inline_chars(&chars[head + 1..]));
        out.push('*');
        return out;
    }
    md_inline_chars(&chars)
}

/// 整段转换：``` 围栏 → pre 块（开栏语言标记丢弃，内容仅转义反引号），
/// 未闭合围栏补一个闭合。分片发生在原文上（split_markdown_paragraphs），
/// 每片独立成文，无跨片实体。
fn md_to_markdownv2(text: &str) -> String {
    let mut out = String::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            out.push_str("```\n");
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            out.push_str(&md_code_escape(line));
            out.push('\n');
        } else {
            out.push_str(&md_inline_line(line));
            out.push('\n');
        }
    }
    if in_fence {
        out.push_str("```");
    }
    out.trim_end_matches('\n').to_string()
}

/// 按空行切块并尽量合并到 limit 内；代码围栏块保持原子；超限单元按
/// 换行优先硬切（多字节安全）。
fn split_markdown_paragraphs(text: &str, limit: usize) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_fence = false;
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        }
        if !in_fence && line.trim().is_empty() {
            if !current.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
            continue;
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(line);
    }
    if !current.is_empty() {
        blocks.push(current);
    }

    let mut chunks: Vec<String> = Vec::new();
    for block in blocks {
        if block.chars().count() > limit {
            for piece in hard_split_block(&block, limit) {
                push_chunk(&mut chunks, piece, limit);
            }
        } else {
            push_chunk(&mut chunks, block, limit);
        }
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

/// MarkdownV2 转义膨胀护栏：片转完后若超 `MD_CHUNK_ESCAPED_LIMIT`，把原文
/// 对半重切（优先在换行处下刀）再各自转，直至达标。退化路径（切开代码围栏）
/// 由 `md_to_markdownv2` 的补闭合兜住，投递不受影响。
fn fit_markdown_chunk(raw: &str) -> Vec<String> {
    const FLOOR_CHARS: usize = 500;
    if md_to_markdownv2(raw).chars().count() <= MD_CHUNK_ESCAPED_LIMIT
        || raw.chars().count() <= FLOOR_CHARS
    {
        return vec![raw.to_string()];
    }
    let chars: Vec<char> = raw.chars().collect();
    let mut cut = chars.len() / 2;
    while cut > 0 && chars[cut] != '\n' {
        cut -= 1;
    }
    if cut == 0 {
        cut = chars.len() / 2;
    }
    let mut out = fit_markdown_chunk(&chars[..cut].iter().collect::<String>());
    out.extend(fit_markdown_chunk(&chars[cut..].iter().collect::<String>()));
    out
}

fn push_chunk(chunks: &mut Vec<String>, piece: String, limit: usize) {
    if let Some(last) = chunks.last_mut() {
        if last.chars().count() + 2 + piece.chars().count() <= limit {
            last.push_str("\n\n");
            last.push_str(&piece);
            return;
        }
    }
    chunks.push(piece);
}

fn hard_split_block(block: &str, limit: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut buf = String::new();
    for line in block.lines() {
        if !buf.is_empty() && buf.chars().count() + 1 + line.chars().count() > limit {
            out.push(std::mem::take(&mut buf));
        }
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(line);
        while buf.chars().count() > limit {
            let head: String = buf.chars().take(limit).collect();
            let rest: String = buf.chars().skip(limit).collect();
            out.push(head);
            buf = rest;
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

/// 尾部滚动追加：超上限时从头截，保证 push 拿到的是回合末尾。
/// 截断点对齐 char 边界（PTY mid-codepoint 教训：绝不切在多字节中间）。
fn append_tail(buffer: &mut String, text: &str, cap: usize) {
    buffer.push_str(text);
    if buffer.len() > cap {
        let cut = buffer.len() - cap;
        let boundary = next_char_boundary(buffer, cut);
        buffer.drain(..boundary);
    }
}

/// 找到 >= target 的第一个 char 边界。
fn next_char_boundary(s: &str, target: usize) -> usize {
    let mut i = target.min(s.len());
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

/// 按 chars 截断（多字节安全），截了就加省略号。
fn clip(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if out.chars().count() < text.chars().count() {
        out.push('…');
    }
    out
}

/// Telegram 的 4096 上限按 UTF-16 码元计（astral 字符如 emoji 占 2 个）；
/// `chars` 计数会低估，emoji 密集文本要按码元再闸一次（发送前最后防线）。
fn clip_utf16(text: &str, max_units: usize) -> String {
    if text.encode_utf16().count() <= max_units {
        return text.to_string();
    }
    let mut units = 0usize;
    let mut end = 0usize;
    for (idx, ch) in text.char_indices() {
        let width = ch.len_utf16();
        if units + width > max_units {
            break;
        }
        units += width;
        end = idx + ch.len_utf8();
    }
    text[..end].to_string()
}

fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs < 60 {
        format!("{secs}秒")
    } else if secs < 3600 {
        format!("{}分{:02}秒", secs / 60, secs % 60)
    } else {
        format!("{}时{}分", secs / 3600, (secs % 3600) / 60)
    }
}

/// 会话显示名与 agent 名：storage 一次查询；缺省退回短 id。
/// 只在未持有 telegram inner 锁时调用（锁序红线：inner → storage 禁止）。
struct SessionMeta {
    label: String,
    agent: String,
}

fn session_meta(session_id: &str) -> SessionMeta {
    let fallback = SessionMeta {
        label: clip(session_id, 24),
        agent: String::new(),
    };
    let Some(slot) = TELEGRAM.get() else {
        return fallback;
    };
    let Some(state) = slot.app.try_state::<crate::AppState>() else {
        return fallback;
    };
    let Ok(storage) = state.storage.lock() else {
        return fallback;
    };
    let Ok(Some(session)) = storage.find_session(session_id) else {
        return fallback;
    };
    let label = if session.label.trim().is_empty() {
        clip(session_id, 24)
    } else {
        clip(session.label.trim(), 40)
    };
    let agent = crate::custom_agents::all_agents_merged()
        .iter()
        .find(|agent| agent.id == session.agent_id)
        .map(|agent| clip(agent.label.trim(), 20))
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| clip(&session.agent_id, 20));
    SessionMeta { label, agent }
}

fn session_label(session_id: &str) -> String {
    session_meta(session_id).label
}

// ─── 对 commands.rs 的 API（状态 / 保存 / 配对 / 回执）────────────────────

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingPairingInfo {
    pub username: String,
    pub first_name: String,
    pub chat_id: i64,
    pub received_at_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TelegramStatus {
    pub configured: bool,
    pub enabled: bool,
    pub bot_username: Option<String>,
    pub paired: bool,
    pub paired_username: Option<String>,
    pub pending_pairing: Option<PendingPairingInfo>,
    pub prefs: TelegramPrefs,
    pub last_error: Option<String>,
}

pub fn status() -> TelegramStatus {
    let settings = load_settings();
    let runtime = TELEGRAM.get().map(|slot| {
        let inner = slot.inner.lock().ok();
        let pending = inner.as_ref().and_then(|inner| {
            inner
                .pending_pairing
                .as_ref()
                .filter(|p| p.received_at.elapsed() <= PAIRING_TTL)
                .map(|p| PendingPairingInfo {
                    username: p.username.clone(),
                    first_name: p.first_name.clone(),
                    chat_id: p.chat_id,
                    received_at_ms: p.received_at.elapsed().as_millis() as u64,
                })
        });
        let last_error = inner.and_then(|inner| inner.last_error.clone());
        let live_enabled = slot.live.lock().ok().map(|l| l.enabled).unwrap_or(false);
        (pending, last_error, live_enabled)
    });
    let (pending, last_error, live_enabled) =
        runtime.unwrap_or((None, None, false));

    TelegramStatus {
        configured: settings.token_cipher.is_some(),
        enabled: settings.enabled && live_enabled,
        bot_username: settings.bot_username.clone(),
        paired: settings.chat_id.is_some(),
        paired_username: settings.paired_username,
        pending_pairing: pending,
        prefs: settings.prefs,
        last_error,
    }
}

#[derive(Clone, Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TelegramSave {
    pub enabled: Option<bool>,
    /// Some("") = 清除 token；None = 不动。
    pub token: Option<String>,
    pub prefs: Option<TelegramPrefs>,
}

/// 保存设置。带 token 时先 `getMe` 校验；启用要求 token + 已配对。
pub fn save(patch: TelegramSave) -> Result<TelegramStatus, String> {
    let mut settings = load_settings();

    if let Some(token) = patch.token.as_deref() {
        let token = token.trim();
        if token.is_empty() {
            settings.token_cipher = None;
            settings.bot_username = None;
            settings.enabled = false;
            settings.chat_id = None;
            settings.paired_username = None;
        } else {
            let bot_username = verify_token(token)?;
            settings.token_cipher = Some(crate::credentials::encrypt_secret(token)?);
            settings.bot_username = Some(bot_username);
        }
    }
    if let Some(prefs) = patch.prefs {
        settings.prefs = prefs;
    }
    match patch.enabled {
        Some(true) => {
            if settings.token_cipher.is_none() {
                return Err("还没有填 bot token".to_string());
            }
            if settings.chat_id.is_none() {
                return Err(
                    "还没配对：先在 Telegram 里给 bot 发一条消息，再回到这里确认".to_string(),
                );
            }
            settings.enabled = true;
        }
        Some(false) => settings.enabled = false,
        None => {}
    }

    save_settings(&settings)?;
    apply_live(&settings);
    // 开关刚打开就发首帧面板：别让用户等到第一次状态变化才见到总控。
    if let Some(slot) = TELEGRAM.get() {
        schedule_dashboard(slot);
    }
    log(
        "info",
        "settings saved",
        Some(&format!(
            "enabled={} paired={}",
            settings.enabled,
            settings.chat_id.is_some()
        )),
    );
    Ok(status())
}

/// 接受当前待配对请求：固化 chat 白名单，并给对方回一条确认。
pub fn accept_pairing() -> Result<TelegramStatus, String> {
    let mut settings = load_settings();
    let request = TELEGRAM
        .get()
        .and_then(|slot| slot.inner.lock().ok())
        .and_then(|mut inner| inner.pending_pairing.take())
        .ok_or_else(|| {
            "当前没有待确认的配对请求（给 bot 发条消息，等它出现在这里）".to_string()
        })?;

    settings.chat_id = Some(request.chat_id);
    if !request.username.is_empty() {
        settings.paired_username = Some(request.username.clone());
    } else if !request.first_name.is_empty() {
        settings.paired_username = Some(request.first_name.clone());
    }
    save_settings(&settings)?;
    apply_live(&settings);

    // 配对完成给对方回一条确认（仅在开关已开时；否则等用户自己开）。
    if let Some(slot) = TELEGRAM.get() {
        let live = slot.live.lock().ok().map(|l| l.clone());
        if let Some(live) = live {
            if live.active() && live.chat_id == Some(request.chat_id) {
                enqueue_send(
                    &live,
                    "🎬 配对完成。Marionette 的会话动态会推到这里；直接回复文字即可继续对话，等批准的通知上会有按钮。"
                        .to_string(),
                    Vec::new(),
                    None,
                    None,
                );
                schedule_dashboard(slot);
            }
        }
    }
    log(
        "info",
        "pairing accepted",
        Some(&format!(
            "@{} chat_id={}",
            request.username, request.chat_id
        )),
    );
    Ok(status())
}

/// 忽略当前待配对请求（不动已保存的白名单）。
pub fn dismiss_pairing() -> TelegramStatus {
    if let Some(slot) = TELEGRAM.get() {
        if let Ok(mut inner) = slot.inner.lock() {
            inner.pending_pairing = None;
        }
    }
    status()
}

/// 解除配对（保留 token，清 chat 白名单并停用）。
pub fn reset_pairing() -> Result<TelegramStatus, String> {
    let mut settings = load_settings();
    settings.chat_id = None;
    settings.paired_username = None;
    settings.enabled = false;
    save_settings(&settings)?;
    apply_live(&settings);
    Ok(status())
}

/// 前端 performSend 的回执（ok=false 带错误摘要）。
pub fn ack(ok: bool, error: Option<String>, reply_to: Option<i64>, session_id: Option<String>) {
    let Some(slot) = TELEGRAM.get() else { return };
    let label = session_id
        .as_deref()
        .map(session_label)
        .unwrap_or_else(|| "会话".to_string());
    let _ = slot.outbound.send(Outbound::Ack {
        ok,
        error,
        session_label: label,
        reply_to,
    });
}

/// 把盘上设置刷进活配置（线程下一轮就跟随）。token 存在时确保线程已拉起
/// （首次保存 token 的路径也走这里——见 `ensure_threads`）。
fn apply_live(settings: &TelegramSettings) {
    let Some(slot) = TELEGRAM.get() else { return };
    let token = settings
        .token_cipher
        .as_deref()
        .and_then(|cipher| crate::credentials::decrypt_secret(cipher).ok());
    {
        let Ok(mut live) = slot.live.lock() else { return };
        live.enabled = settings.enabled && token.is_some();
        live.token = token.clone();
        live.chat_id = settings.chat_id;
        live.prefs = settings.prefs.clone();
    }
    if token.is_some() {
        ensure_threads(slot);
    }
}

// ─── 单测（纯函数；绝不碰网络与真实目录）──────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_kind_reads_all_three_aliases() {
        assert_eq!(
            update_kind(&json!({ "sessionUpdate": "user_message_chunk" })),
            "user_message_chunk"
        );
        assert_eq!(update_kind(&json!({ "type": "tool_call" })), "tool_call");
        assert_eq!(update_kind(&json!({ "kind": "plan" })), "plan");
        assert_eq!(update_kind(&json!({})), "");
    }

    #[test]
    fn extract_update_text_handles_blocks_strings_and_arrays() {
        assert_eq!(
            extract_update_text(&json!({ "content": { "type": "text", "text": "hello" } })),
            "hello"
        );
        assert_eq!(
            extract_update_text(&json!({ "delta": { "type": "text", "text": "世界" } })),
            "世界"
        );
        assert_eq!(extract_update_text(&json!({ "content": "plain" })), "plain");
        // block 数组：只收 text block。
        assert_eq!(
            extract_update_text(&json!({ "content": [
                { "type": "text", "text": "a" },
                { "type": "image", "image": "x" },
                { "type": "text", "text": "b" },
            ]})),
            "ab"
        );
        assert_eq!(
            extract_update_text(&json!({ "content": { "type": "image" } })),
            ""
        );
    }

    #[test]
    fn question_plan_single_question_maps_to_buttons() {
        let questions = vec![json!({
            "question": "选一个",
            "options": [ { "label": "A" }, { "label": "B" } ],
        })];
        match question_push_plan(Some(&questions)) {
            QuestionPush::Buttons { question, options } => {
                assert_eq!(question, "选一个");
                assert_eq!(options, vec!["A".to_string(), "B".to_string()]);
            }
            other => panic!("expected buttons, got {other:?}"),
        }
    }

    #[test]
    fn question_plan_multi_question_or_multiselect_falls_back_to_text() {
        let multi_q = vec![
            json!({ "question": "一", "options": [ { "label": "x" } ] }),
            json!({ "question": "二", "options": [ { "label": "y" } ] }),
        ];
        assert!(matches!(
            question_push_plan(Some(&multi_q)),
            QuestionPush::TextOnly { .. }
        ));
        let multi_select = vec![json!({
            "question": "选多个",
            "multiSelect": true,
            "options": [ { "label": "x" } ],
        })];
        assert!(matches!(
            question_push_plan(Some(&multi_select)),
            QuestionPush::TextOnly { .. }
        ));
        let open = vec![json!({ "question": "说说看" })];
        match question_push_plan(Some(&open)) {
            QuestionPush::TextOnly { body } => assert!(body.contains("直接回复文字")),
            other => panic!("expected text, got {other:?}"),
        }
    }

    #[test]
    fn append_tail_keeps_the_end_and_respects_char_boundaries() {
        let mut buffer = String::new();
        append_tail(&mut buffer, "abcdef", 10);
        append_tail(&mut buffer, "ghij", 10);
        assert_eq!(buffer, "abcdefghij");
        // 中间字符绝不切开；缓冲保尾部（cap 按 bytes 计，断言按 chars）。
        let mut cjk = String::new();
        append_tail(&mut cjk, "你好世界测试", 12); // 6 字 18 bytes → 留 4 字
        assert_eq!(cjk.chars().count(), 4);
        append_tail(&mut cjk, "尾", 12);
        assert!(cjk.ends_with('尾'));
        // 恰好边界值不会抖动。
        let mut exact = String::new();
        append_tail(&mut exact, "abcd", 4);
        assert_eq!(exact, "abcd");
    }

    #[test]
    fn clip_is_char_safe_and_marks_truncation() {
        assert_eq!(clip("abcdef", 3), "abc…");
        assert_eq!(clip("你好世界", 2), "你好…");
        assert_eq!(clip("短", 10), "短");
    }

    #[test]
    fn format_duration_buckets() {
        assert_eq!(format_duration(Duration::from_secs(7)), "7秒");
        assert_eq!(format_duration(Duration::from_secs(65)), "1分05秒");
        assert_eq!(format_duration(Duration::from_secs(3670)), "1时1分");
    }

    #[test]
    fn stop_reason_head_maps_known_reasons() {
        assert_eq!(stop_reason_head("end_turn"), ("✅", "跑完"));
        assert_eq!(stop_reason_head("max_tokens"), ("✂️", "到达长度上限"));
        assert_eq!(stop_reason_head("refusal"), ("⚠️", "模型拒答"));
        assert_eq!(stop_reason_head("max_turn_requests"), ("⏹", "达到回合上限"));
        assert_eq!(stop_reason_head("anything-else"), ("✅", "跑完"));
    }

    #[test]
    fn settings_roundtrip_encrypts_token_and_never_writes_plaintext() {
        let dir = std::env::temp_dir().join(format!("marionette_tg_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("telegram.json");

        let mut settings = TelegramSettings::default();
        settings.enabled = true;
        settings.token_cipher = Some(crate::credentials::encrypt_secret("123:ABC-token").unwrap());
        settings.chat_id = Some(42);
        settings.prefs.notify_turn = false;
        save_settings_at(&path, &settings).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(!raw.contains("123:ABC-token"), "token 明文绝不能落盘");
        assert!(raw.contains("tokenCipher"));

        let back = load_settings_at(&path);
        assert!(back.enabled);
        assert_eq!(back.chat_id, Some(42));
        assert!(!back.prefs.notify_turn);
        assert_eq!(
            crate::credentials::decrypt_secret(back.token_cipher.as_deref().unwrap()).unwrap(),
            "123:ABC-token"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unreadable_settings_stay_on_disk_and_newer_versions_archive() {
        let dir = std::env::temp_dir().join(format!("marionette_tg_corrupt_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("telegram.json");

        // 解析失败（截断/瞬态）：文件原地保留，绝不归档（归档会把 token 一起带走）。
        std::fs::write(&path, "{ not json").unwrap();
        let settings = load_settings_at(&path);
        assert!(!settings.enabled);
        assert!(path.exists(), "unreadable telegram.json must stay on disk");
        let archived = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("telegram.corrupt-"));
        assert!(!archived, "parse failure must not archive the file");

        // 未来版本：留档后从默认重来。
        std::fs::write(&path, r#"{"version": 99, "enabled": true}"#).unwrap();
        let settings = load_settings_at(&path);
        assert!(!settings.enabled);
        let archived = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| e.file_name().to_string_lossy().starts_with("telegram.corrupt-"));
        assert!(archived, "newer-version telegram.json must be archived, not wiped");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_config_active_requires_pairing_and_switch() {
        let mut live = LiveConfig::default();
        assert!(!live.active());
        live.enabled = true;
        assert!(!live.active(), "开关开了但没配对 → 不生效");
        live.chat_id = Some(1);
        assert!(live.active());
    }

    fn dash_row(
        label: &str,
        agent: &str,
        state: DashState,
        elapsed_secs: Option<u64>,
        idle_secs: u64,
        bound: bool,
    ) -> DashRow {
        DashRow {
            session_id: format!("s-{label}"),
            label: label.to_string(),
            agent: agent.to_string(),
            state,
            elapsed_secs,
            idle_secs,
            bound,
        }
    }

    #[test]
    fn session_track_state_prefers_waiting_then_running() {
        let mut track = SessionTrack::new();
        assert_eq!(track.state(), DashState::Idle);
        track.turn_started = Some(Instant::now());
        assert_eq!(track.state(), DashState::Running);
        // 等待中的回合仍然是 ⏸：agent 挂在权限/ask/plan 上。
        track.waiting = 1;
        assert_eq!(track.state(), DashState::Waiting);
    }

    #[test]
    fn render_dashboard_lists_counts_bound_and_idle_overflow() {
        let rows = vec![
            dash_row("主线", "Claude Code", DashState::Running, Some(125), 0, false),
            dash_row("NAS 脚本", "DSH", DashState::Waiting, None, 30, false),
            dash_row("调研", "", DashState::Idle, None, 240, true),
        ];
        let text = render_dashboard(&rows, 3);
        assert!(text.contains("▶1 ⏸1"), "头部计数要出现：{text}");
        assert!(text.contains("▶ [主线] · Claude Code · 2分05秒"), "{text}");
        assert!(text.contains("⏸ [NAS 脚本] · 等你"), "{text}");
        assert!(text.contains("⏎ 💤 [调研] · 4分钟前有动静"), "{text}");
        assert!(text.contains("其余 3 个会话空闲"), "{text}");
        assert!(text.contains("/status"), "{text}");
    }

    #[test]
    fn render_dashboard_empty_shows_idle_line() {
        let text = render_dashboard(&[], 0);
        assert!(text.contains("没有会话在跑"), "{text}");
        assert!(text.contains("📌 Marionette 总控"), "{text}");
    }

    #[test]
    fn render_status_covers_three_states_and_hints() {
        let rows = vec![
            dash_row("跑着的", "Grok", DashState::Running, Some(60), 0, true),
            dash_row("等着的", "", DashState::Waiting, None, 5, false),
            dash_row("闲着的", "Codex", DashState::Idle, None, 900, false),
        ];
        let text = render_status(&rows, 5);
        assert!(text.contains("会话清单（3/5）"), "{text}");
        assert!(text.contains("⏎ ▶ [跑着的] · Grok · 跑了 1分00秒"), "{text}");
        assert!(text.contains("⏸ [等着的] · 等你批准/回答"), "{text}");
        assert!(text.contains("💤 [闲着的] · Codex · 15分钟前有动静"), "{text}");
        assert!(text.contains("⏎ 标记的会话"), "{text}");
    }

    #[test]
    fn render_tool_detail_turns_json_into_key_value_lines() {
        // acp 的 rawInput 是 serde 序列化串：反序列化后键序不保证，逐键断言。
        let rendered = render_tool_detail(r#"{"file_path":"C:\\Users\\NT\\plans\\x.md"}"#);
        assert_eq!(rendered, "file_path: C:\\Users\\NT\\plans\\x.md");
        let rendered = render_tool_detail(r#"{"command":"cargo test","args":["--all"]}"#);
        assert!(rendered.contains("command: cargo test"), "{rendered}");
        assert!(rendered.contains("args: --all"), "{rendered}");
        // 数组顶层：逗号连接。
        assert_eq!(render_tool_detail(r#"["a","b"]"#), "a, b");
        // 非 JSON（plan markdown 等）与坏 JSON 原样保留。
        assert_eq!(render_tool_detail("# 计划\n正文"), "# 计划\n正文");
        assert_eq!(render_tool_detail("{oops"), "{oops");
    }

    #[test]
    fn wait_cleanup_matches_all_six_methods() {
        for method in [
            "permission/timeout",
            "permission/resolved",
            "question/timeout",
            "question/resolved",
            "plan/timeout",
            "plan/resolved",
        ] {
            assert!(is_wait_cleanup(method), "{method}");
        }
        assert!(!is_wait_cleanup("permission/prompt"));
        assert!(!is_wait_cleanup("session/update"));
    }

    #[test]
    fn md_escape_covers_reserved_chars_and_backslash() {
        assert_eq!(md_escape("[a].(b)!"), r"\[a\]\.\(b\)\!");
        assert_eq!(md_escape(r"C:\x\y"), r"C:\\x\\y");
        assert_eq!(md_escape("中文·不变"), "中文·不变");
    }

    #[test]
    fn markdownv2_converts_bold_italic_code_link_heading() {
        assert_eq!(
            md_to_markdownv2("**跑完了**，见 `cargo test`"),
            r"*跑完了*，见 `cargo test`"
        );
        assert_eq!(md_to_markdownv2("*重点*内容"), r"_重点_内容");
        // 标题整行加粗。
        assert_eq!(md_to_markdownv2("## 结论"), "*结论*");
        // 链接：文本部分全转义，URL 只转义 ) 与 \。
        assert_eq!(
            md_to_markdownv2("[文档](https://x.io/a.b)"),
            r"[文档](https://x.io/a.b)"
        );
        // 未配对的标记按字面转义，绝不产生悬空实体。
        assert_eq!(md_to_markdownv2("a * b"), r"a \* b");
    }

    #[test]
    fn markdownv2_keeps_intraword_underscores_literal() {
        // 词内下划线按字面量转义（CommonMark 规则）；词内 * 的斜体语义保留。
        assert_eq!(
            md_to_markdownv2("改了 file_name_x 和 2*3*4"),
            r"改了 file\_name\_x 和 2_3_4"
        );
    }

    #[test]
    fn markdownv2_fences_drop_language_and_keep_content_raw() {
        assert_eq!(
            md_to_markdownv2("```rust\nfn main() {}\n```"),
            "```\nfn main() {}\n```"
        );
        // 未闭合围栏补闭合。
        assert_eq!(md_to_markdownv2("```\nplain"), "```\nplain\n```");
    }

    #[test]
    fn reply_chunks_merge_paragraphs_and_keep_fences_atomic() {
        let text = "第一段。\n\n第二段，稍长一点。";
        assert_eq!(split_markdown_paragraphs(text, 100), vec![text]);
        // 各自超不过 limit 时合并；超了就分开。
        assert_eq!(
            split_markdown_paragraphs(text, 10),
            vec!["第一段。".to_string(), "第二段，稍长一点。".to_string()]
        );
        // 围栏块内空行不切块。
        let fenced = "```\na\n\nb\n```";
        assert_eq!(split_markdown_paragraphs(fenced, 100), vec![fenced.to_string()]);
        // 单块超限硬切（换行优先、多字节安全）。
        let long = "很长的段落".repeat(50);
        for chunk in split_markdown_paragraphs(&long, 40) {
            assert!(chunk.chars().count() <= 40);
        }
    }

    #[test]
    fn reply_markup_must_be_inline_keyboard_object() {
        // 回归：reply_markup 必须是 {"inline_keyboard": …} 对象壳。
        // 裸数组会被 Telegram 400（can't parse reply keyboard markup），
        // 曾导致总控面板一条都发不出去。
        let keyboard = vec![
            vec![
                TgButton { text: "⏹ [NAS 脚本]".into(), callback_id: 42 },
                TgButton { text: "名".into(), callback_id: 7 },
            ],
            vec![TgButton { text: "🔄 刷新".into(), callback_id: 9 }],
        ];
        let value = reply_markup(&keyboard);
        assert!(
            value.get("inline_keyboard").is_some(),
            "reply_markup 缺 inline_keyboard 壳: {value}"
        );
        assert_eq!(value["inline_keyboard"][0][0]["callback_data"], "42");
        assert_eq!(value["inline_keyboard"][0][1]["text"], "名");
        assert_eq!(value["inline_keyboard"][1][0]["callback_data"], "9");
    }

    #[test]
    fn fit_markdown_chunk_splits_escape_dense_text() {
        // '.' 转义后翻倍：4000 字 → 8000，超过 3400 必须在原文上重切。
        let raw = ".".repeat(4000);
        let pieces = fit_markdown_chunk(&raw);
        assert!(pieces.len() >= 2, "应切成多片: {}", pieces.len());
        for piece in &pieces {
            assert!(md_to_markdownv2(piece).chars().count() <= MD_CHUNK_ESCAPED_LIMIT);
        }
        assert_eq!(pieces.concat(), raw, "重切不得丢字");
        // 达标文本原样返回。
        assert_eq!(fit_markdown_chunk("普通正文"), vec!["普通正文".to_string()]);
    }

    #[test]
    fn clip_utf16_counts_astral_pairs() {
        assert_eq!(clip_utf16("abc", 10), "abc");
        // 😀 是 astral，占 2 个 UTF-16 码元：5 个 = 10 码元。
        let emoji = "😀".repeat(5);
        assert_eq!(clip_utf16(&emoji, 10), emoji);
        let clipped = clip_utf16(&emoji, 7);
        assert_eq!(clipped.encode_utf16().count(), 6, "按码元切在字符边界上");
        assert!(clipped.chars().all(|c| c == '😀'));
    }
}
