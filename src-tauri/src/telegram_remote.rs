//! Telegram 远程桥（docs/plans/telegram-bridge.md · MVP）。
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
//!   裸发文字的去向一目了然；按钮 = 切换绑定 + 📄 会话详情 + 远程 ⏹
//!   （`AcpService::cancel`，与 UI 停止键同路）+ 刷新。`/status` 拉全量清单。
//!   心跳线程在忙时每 60s 重画一次保鲜时长行。
//! - **回复正文**：只要「最后一条 agent 消息」（工具调用是消息边界，中途
//!   解说不进推送），优先官方 Rich Messages（Bot API 10.1 `sendRichMessage`，
//!   `rich_message.markdown` 收原生 markdown——标题/表格/代码块由客户端渲染，
//!   单条 32K 字符不截断），失败退 MarkdownV2 分片、再退纯文本；计划卡片
//!   同走 MarkdownV2 + 纯文本兜底。
//! - **回复带图**（2026-11-23）：回合内 agent 读到 / 产生的图——image 块
//!   （含只有 base64 的截图）、工具 `locations`/`rawInput` 指向的图片文件、
//!   末段正文里的 markdown 图片——在正文之后逐张发出（≤4 张取最近）。
//!   收集口径对齐前端工具卡预览（`extract_update_images`），发送见
//!   `send_image`；同样受「内容预览」偏好门控。
//! - **会话详情视图**：置顶面板同一个 message_id 上的「第二页」——点 📄
//!   原地切换成该会话的状态 + 最近一条回复（markdown 渲染），⬅ 返回总控；
//!   正文受「内容预览」偏好门控（关 = 内容不出本机）。
//! - **HTTP**：复用 `http_client` 的 ureq + native-tls agent，跟随应用代理设置；阻塞调用跑在
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
use std::sync::mpsc::SyncSender;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

/// Telegram Bot API base。用户在日本直连可达（调研 2026-10-17）。
const API_BASE: &str = "https://api.telegram.org";

/// Bot API base（诊断钩子，2026-11-15）：`MARIONETTE_TELEGRAM_API_BASE`
/// 可把整条线路指到**本地**假 Bot API 服务器，做线路段端到端 fixture
/// （poller → 配对门 → 路由 → 管线 → 出站队列全部走真代码，只有云端
/// 中继是 fixture）。只放行 loopback，未设 = 官方云、行为与历史一致。
fn api_base() -> String {
    std::env::var("MARIONETTE_TELEGRAM_API_BASE")
        .ok()
        .filter(|v| {
            let v = v.trim_end_matches('/');
            v.starts_with("http://127.0.0.1") || v.starts_with("http://localhost")
        })
        .unwrap_or_else(|| API_BASE.to_string())
}
/// 长轮询挂起秒数；HTTP 超时留 15s 余量。
const POLL_TIMEOUT_SECS: u64 = 50;
const POLL_HTTP_TIMEOUT_SECS: u64 = 65;
const SEND_HTTP_TIMEOUT_SECS: u64 = 15;
/// sendMessage 正文上限 4096 字符；留安全边界。
const MAX_MESSAGE_CHARS: usize = 3800;
/// 回合末条回复在推送里保留的长度已改为按消息分片（见 REPLY_CHUNK_CHARS）。
/// 回合内 agent 文本滚动缓冲上限（超了丢头部，保尾部）——32K 约等于全文。
const TURN_BUFFER_CHARS: usize = 32000;
/// 「会话详情」视图里最近一条回复的缓存上限（正文展示用）。
const LAST_REPLY_CHARS: usize = 8000;
/// `sendRichMessage` 的 markdown 单条上限是 32768 字符（Bot API 10.1+）；
/// 留一点给 header/footer 再裁。
const RICH_MARKDOWN_LIMIT: usize = 32700;
/// 推送正文每条消息的预算（4096 上限给头部/页脚/MarkdownV2 转义膨胀留余量）。
const REPLY_CHUNK_CHARS: usize = 3300;
/// 单片转成 MarkdownV2 后的长度上限：转义最坏约 2×，超了就在原文上对半
/// 重切（`fit_markdown_chunk`），给头/尾与 4096 硬顶留余量。
const MD_CHUNK_ESCAPED_LIMIT: usize = 3400;
/// 正文最多拆几条消息（主消息 + 续片），再长就截断提示回电脑看。
const REPLY_MAX_MESSAGES: usize = 4;
/// 回合推送随附的图片上限（保本轮最近的几张，更早的丢）。
const MAX_TURN_IMAGES: usize = 4;
/// 用户消息里自带的图（键）记几个：回合末的随附图要把它们排除——
/// 用户刚附的截图不必再推回他手机。
const MAX_USER_IMAGE_KEYS: usize = 16;
/// 单张图的上传上限（Telegram `sendPhoto` 的 10 MB 硬顶；超了不发）。
const IMAGE_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// 图片上传超时：multipart 上传 + 服务端转码比发一条文字慢得多。
const IMAGE_HTTP_TIMEOUT_SECS: u64 = 60;
/// caption 上限 1024 字符（按 UTF-16 码元计），留安全边界。
const CAPTION_MAX_UNITS: usize = 1000;
/// `send_image` 的「文件不存在」哨兵：这种图连电脑上也看不着，静默跳过。
const IMAGE_NOT_FOUND: &str = "image file not found";
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
/// 出站队列容量：正常时近空；长故障期满了就丢新消息（见 try_enqueue）。
const OUTBOUND_QUEUE_CAP: usize = 64;
/// 队列满以来丢弃的消息总数（留痕用）。
static OUTBOUND_DROPS: AtomicU64 = AtomicU64::new(0);

/// 前端收文本路由的事件名（App.tsx 监听 → performSend → ack）。
pub const INBOUND_EVENT: &str = "telegram-remote-inbound";

/// 前端收远程命令的事件名（/new 建会话、/model 切模型 → App.tsx 执行 →
/// `telegram_remote_result` 回执）。
pub const COMMAND_EVENT: &str = "telegram-remote-command";

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

/// update_id 游标持久化（2026-11-15，chief M0 探针 5）。poller 线程是
/// 唯一写者、独立文件——避免与 telegram.json 的 UI 保存互相踩（设置
/// 整档覆盖会回滚游标，游标回写会冲掉并发保存的 token）。重启后不再
/// 重复处理旧消息；Telegram 24h update 保留不能代替本地 offset。
fn cursor_path() -> Result<PathBuf, String> {
    Ok(crate::app_paths::global_dir()?.join("telegram-cursor.json"))
}

/// Bot 编号 = token 冒号前的数字（公开信息，不含密钥）。游标按 bot 记账：
/// update_id 是每个 bot 各自的序列，换了 bot（换 token）还沿用旧 offset，
/// 新 bot 比它小的 update 会被 Telegram 判为「已确认」而永远收不到。
fn bot_id_of(token: &str) -> String {
    token.split(':').next().unwrap_or("").to_string()
}

fn load_cursor(bot_id: &str) -> i64 {
    let path = match cursor_path() {
        Ok(path) => path,
        Err(_) => return 0,
    };
    let Some(value) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
    else {
        return 0;
    };
    // 旧格式（无 botId）或 bot 不符 = 不可信，从 0 起（Telegram 只会重投
    // 未确认的 update，最坏是重放一小段，不会漏）。
    if value.get("botId").and_then(Value::as_str) != Some(bot_id) {
        return 0;
    }
    value
        .get("updateOffset")
        .and_then(Value::as_i64)
        .unwrap_or(0)
}

fn save_cursor(bot_id: &str, offset: i64) {
    let Ok(path) = cursor_path() else { return };
    let body = format!(
        "{}\n",
        serde_json::json!({ "updateOffset": offset, "botId": bot_id })
    );
    if let Err(error) = crate::storage::write_atomic(&path, &body) {
        log("warn", "persist update cursor failed", Some(&error));
    }
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
        std::fs::create_dir_all(dir).map_err(|e| {
            crate::ts!(
                "创建 .marionette 目录失败: {e}",
                "Failed to create the .marionette directory: {e}"
            )
        })?;
    }
    let body = serde_json::to_string_pretty(settings).map_err(|e| {
        crate::ts!(
            "序列化 telegram.json 失败: {e}",
            "Failed to serialize telegram.json: {e}"
        )
    })?;
    // 临时兄弟文件 + rename 原子写（同凭据库模式）。
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, format!("{body}\n")).map_err(|e| {
        crate::ts!(
            "写入临时文件失败: {e}",
            "Failed to write the temp file: {e}"
        )
    })?;
    if let Err(error) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(crate::ts!(
            "重命名临时文件失败: {error}",
            "Failed to rename the temp file: {error}"
        ));
    }
    Ok(())
}

fn load_settings() -> TelegramSettings {
    match settings_path() {
        Ok(path) => load_settings_at(&path),
        Err(_) => TelegramSettings::default(),
    }
}

/// The one chat id currently paired with this installation. Staff binding is
/// intentionally limited to it in E1 so a typo cannot send employee traffic to
/// an unrelated Telegram chat.
pub fn paired_chat_id() -> Option<i64> {
    load_settings().chat_id
}

fn save_settings(settings: &TelegramSettings) -> Result<(), String> {
    save_settings_at(&settings_path()?, settings)
}

// ─── 运行时状态 ────────────────────────────────────────────────────────────

struct SessionTrack {
    /// 本回合开始时刻（见到 user_message 回显时钉住）。
    turn_started: Option<Instant>,
    last_activity: Instant,
    /// 本回合 agent 文本（只要最后一段：工具调用是消息边界）。
    text: TurnText,
    /// 本回合 agent 读到 / 产生的图（去重，保最近 MAX_TURN_IMAGES 张）。
    images: VecDeque<TurnImage>,
    /// 本回合用户消息里自带的图（键）：回合末的随附图要排除它们。
    user_image_keys: Vec<String>,
    /// 最近一条已完成回合的正文缓存（面板「会话详情」视图用）。
    last_reply: String,
    /// 挂起未解的权限/ask/plan 请求数（>0 = 会话在等你）。
    waiting: u32,
    /// 本回合是本地探针（额度面板刷新发的 `/status` / `/usage`）：完成时
    /// 不推送、正文不进 last_reply——它不是用户发起的对话。
    probe_turn: bool,
}

/// 回合内 agent 文本的「末段」追踪。agent 常在工具调用之间插一段解说
/// 再继续（「我先看看…」→ 工具 → 最终答复），推送只要最后一条消息；
/// 上一段留作兜底——回合以工具收尾、末段为空时用它，免得推空正文。
#[derive(Default)]
struct TurnText {
    current: String,
    previous: String,
}

impl TurnText {
    fn reset(&mut self) {
        self.current.clear();
        self.previous.clear();
    }

    fn append(&mut self, text: &str) {
        append_tail(&mut self.current, text, TURN_BUFFER_CHARS);
    }

    /// 工具调用 = 边界：当前段结业为 previous；空段不覆盖（连续工具调用）。
    fn tool_boundary(&mut self) {
        if !self.current.is_empty() {
            self.previous.clear();
            std::mem::swap(&mut self.current, &mut self.previous);
        }
    }

    /// 取最终正文（末段优先，空则退上一段），取完清空。
    fn take_final(&mut self) -> String {
        let current = std::mem::take(&mut self.current);
        if current.trim().is_empty() {
            std::mem::take(&mut self.previous)
        } else {
            self.previous.clear();
            current
        }
    }
}

/// 一张要随回合推送发出去的图（来源见 `extract_update_images`）。
#[derive(Clone, PartialEq)]
enum TurnImage {
    /// 本地文件（可能是相对路径——发送前按会话 cwd 补全）。
    Local(PathBuf),
    /// 远程图片（http/https）：交给 Telegram 服务端自己拉。
    Remote(String),
    /// 内联 base64：image 块只有 `data` 没有 uri（截图 / Read 图片的
    /// 常见形态）。存 base64 原文——解码放到发送线程，事件收口只搬字符串。
    Bytes { data: String, mime: String },
}

impl std::fmt::Debug for TurnImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            // 图片本体不进 Debug：一张截图的 base64 能把日志刷满。
            TurnImage::Local(path) => f.debug_tuple("Local").field(path).finish(),
            TurnImage::Remote(url) => f.debug_tuple("Remote").field(url).finish(),
            TurnImage::Bytes { data, mime } => f
                .debug_struct("Bytes")
                .field("data", &format_args!("{} b64 chars", data.len()))
                .field("mime", mime)
                .finish(),
        }
    }
}

impl TurnImage {
    /// 去重键。本地路径与前端 `imagePathKey` 同口径（分隔符归一 + 小写）。
    fn key(&self) -> String {
        match self {
            TurnImage::Local(path) => {
                format!("local:{}", path_key(&path.to_string_lossy()))
            }
            TurnImage::Remote(url) => format!("remote:{}", url.trim().to_lowercase()),
            TurnImage::Bytes { data, .. } => {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                data.trim().hash(&mut hasher);
                format!("bytes:{}", hasher.finish())
            }
        }
    }

    /// 相对路径按会话 cwd 补全（与前端 `resolveToolImagePath` 同口径）。
    fn resolve(self, cwd: Option<&str>) -> Self {
        match self {
            TurnImage::Local(path) => TurnImage::Local(PathBuf::from(resolve_tool_image_path(
                &path.to_string_lossy(),
                cwd,
            ))),
            other => other,
        }
    }

    /// 日志 / 失败提示里的一行描述（不含图片本体）。进 dev.log，所以不本地化。
    fn describe(&self) -> String {
        match self {
            TurnImage::Local(path) => path.to_string_lossy().into_owned(),
            TurnImage::Remote(url) => url.clone(),
            TurnImage::Bytes { data, mime } => {
                format!("inline {mime} · {} KB", data.len() / 1024)
            }
        }
    }

    /// 首选的发送方法（Bot API 方法名，multipart 字段名）。
    fn send_as(&self) -> (&'static str, &'static str) {
        let kind = match self {
            TurnImage::Local(path) => path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(str::to_ascii_lowercase),
            TurnImage::Remote(url) => image_extension(url),
            TurnImage::Bytes { mime, .. } => Some(mime_subtype(mime)),
        };
        send_method_for(kind.as_deref())
    }
}

impl SessionTrack {
    fn new() -> Self {
        Self {
            turn_started: None,
            last_activity: Instant::now(),
            text: TurnText::default(),
            images: VecDeque::new(),
            user_image_keys: Vec::new(),
            last_reply: String::new(),
            waiting: 0,
            probe_turn: false,
        }
    }

    /// 新回合开始：清掉上一轮的随附图，记下用户这次自带的图。
    /// `fresh=false`（同一条用户消息的后续 chunk）：只并入新图键，
    /// 不清——用户附件可能分散在多个 chunk 里。
    fn begin_turn(&mut self, user_images: &[TurnImage], fresh: bool) {
        if fresh {
            self.images.clear();
            self.user_image_keys.clear();
        }
        for image in user_images {
            let key = image.key();
            if self.user_image_keys.len() < MAX_USER_IMAGE_KEYS
                && !self.user_image_keys.contains(&key)
            {
                self.user_image_keys.push(key);
            }
        }
    }

    /// 收进本轮的随附图（同一张只留一次；满了丢最旧的）。
    fn push_images(&mut self, images: Vec<TurnImage>) {
        for image in images {
            let key = image.key();
            if self.images.iter().any(|existing| existing.key() == key) {
                continue;
            }
            self.images.push_back(image);
            while self.images.len() > MAX_TURN_IMAGES {
                self.images.pop_front();
            }
        }
    }

    /// 取本轮随附图（用户自带的那几张不算），取完清空。用户图键留到下个
    /// 回合才清——同一回合的取图可能不止一次（agent re-entry 的封段），
    /// 排除口径必须全程有效。
    fn take_images(&mut self) -> Vec<TurnImage> {
        let taken = std::mem::take(&mut self.images);
        let user_keys = &self.user_image_keys;
        taken
            .into_iter()
            .filter(|image| !user_keys.contains(&image.key()))
            .collect()
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
    BindSession { session_id: String, label: String },
    /// 总控面板：打断该会话当前回合（与 UI 停止键同一条 cancel 路径）。
    StopTurn { session_id: String, label: String },
    /// 总控面板：置顶消息原地切换到该会话的详情视图。
    ViewSession { session_id: String, label: String },
    /// 详情视图：返回总控清单。
    PanelBack,
    /// 总控面板：手动刷新面板文本。
    RefreshDashboard,
    // ─── /new 向导（每个步骤的回答；向导本体存在 Inner.wizard）──────────
    /// /new 第 1 步：建在聊天区。
    NewDestChat,
    /// /new 第 1 步：建在某个项目下。
    NewDestProject {
        project_id: String,
        project_name: String,
    },
    /// /new 第 2 步：选 agent。
    NewAgent {
        agent_id: String,
        agent_label: String,
    },
    /// /new 第 3 步：选模型（model_id 空 = 用默认/最近一次）。
    NewModel {
        model_id: String,
        model_label: String,
    },
    /// /new 第 4 步：选编辑模式（mode_id 空 = 不指定）。
    NewMode { mode_id: String, mode_label: String },
    /// /new 任意步骤：取消。
    NewCancel,
    // ─── 模型菜单（/model 与 /new 第 3 步共用）──────────────────────────
    /// /new 第 3 步模型清单翻页（菜单原地重画）。
    WizardModelPage { offset: usize },
    /// /model 菜单翻页（菜单原地重画）。
    ModelPage { session_id: String, offset: usize },
    /// /model：把这个会话切到该模型。
    SetModel {
        session_id: String,
        model_id: String,
        model_label: String,
    },
    /// /resume：把这个会话设为裸文本绑定，继续聊。
    ResumeSession { session_id: String, label: String },
    /// /resume 清单翻页（菜单原地重画）。
    ResumePage { offset: usize },
    /// 菜单类消息的「关闭」：只摘按钮，无事发生。
    DismissMenu,
}

impl CallbackAction {
    fn session_id(&self) -> &str {
        match self {
            CallbackAction::Permission { session_id, .. }
            | CallbackAction::Question { session_id, .. }
            | CallbackAction::QuestionDecline { session_id, .. }
            | CallbackAction::Plan { session_id, .. }
            | CallbackAction::BindSession { session_id, .. }
            | CallbackAction::StopTurn { session_id, .. }
            | CallbackAction::ViewSession { session_id, .. } => session_id,
            // 无会话上下文的动作：日志里给个可读名。
            CallbackAction::PanelBack | CallbackAction::RefreshDashboard => "dashboard",
            CallbackAction::NewDestChat
            | CallbackAction::NewDestProject { .. }
            | CallbackAction::NewAgent { .. }
            | CallbackAction::NewModel { .. }
            | CallbackAction::NewMode { .. }
            | CallbackAction::NewCancel
            | CallbackAction::WizardModelPage { .. } => "new-session",
            CallbackAction::ModelPage { session_id, .. }
            | CallbackAction::SetModel { session_id, .. } => session_id,
            CallbackAction::ResumeSession { session_id, .. } => session_id,
            CallbackAction::ResumePage { .. } | CallbackAction::DismissMenu => "menu",
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
    /// 置顶面板当前展示的视图：总控清单，或某个会话的详情页。
    view: PanelView,
    pending_pairing: Option<PairingRequest>,
    last_error: Option<String>,
    /// 进行中的 /new 向导。重新 /new 直接覆盖旧的（旧按钮全部失效）。
    wizard: Option<NewSessionWizard>,
}

/// /new 向导的累积选择。步骤线性推进：目的地 → agent → 模型 → 模式 → 创建。
#[derive(Clone, Debug)]
struct NewSessionWizard {
    dest: Option<WizardDest>,
    agent_id: String,
    agent_label: String,
    /// None = 模型步骤还没答（也没法答 → 跳过）；Some(None) = 用户选「默认」；
    /// Some(Some((id, label))) = 指定了模型。
    model: Option<Option<(String, String)>>,
}

#[derive(Clone, Debug)]
enum WizardDest {
    Chat,
    Project { id: String, name: String },
}

/// 置顶面板的当前视图。
#[derive(Clone, Debug, PartialEq)]
enum PanelView {
    Dashboard,
    Session(String),
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
            view: PanelView::Dashboard,
            pending_pairing: None,
            last_error: None,
            wizard: None,
        }
    }

    fn track(&mut self, session_id: &str) -> &mut SessionTrack {
        self.sessions
            .entry(session_id.to_string())
            .or_insert_with(SessionTrack::new)
    }

    fn remember_route(&mut self, message_id: i64, session_id: &str) {
        if self
            .routes
            .insert(message_id, session_id.to_string())
            .is_none()
        {
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
        /// MarkdownV2 被拒时的纯文本回退体（None = 用 text 本身）。
        plain: Option<String>,
    },
    /// 回合结束推送：整段正文交给发送线程，优先 Rich Messages 直投，
    /// 失败再退 MarkdownV2 分片（见 `deliver_turn_reply`）。
    TurnReply {
        header: String,
        body: String,
        footer: String,
        session_id: String,
    },
    /// 回合推送的随附图片：排在同回合的 TurnReply 之后，逐张发出。
    Photos {
        session_id: String,
        images: Vec<TurnImage>,
        /// 首张（发出成功的那张）的 caption。
        caption: String,
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
        /// inline_keyboard 的行分组（会话行 = [名字, 📄, ⏹]，末行 = [🔄]）。
        keyboard: Vec<Vec<TgButton>>,
        /// true = 会话详情视图（正文是原生 markdown，走 rich 编辑投递）。
        rich: bool,
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
    outbound: SyncSender<Outbound>,
    /// 发送线程的接收端。惰性取走：线程第一次拉起时 move 进去。
    inbound_rx: Mutex<Option<std::sync::mpsc::Receiver<Outbound>>>,
    inner: Mutex<Inner>,
    live: Mutex<LiveConfig>,
    callback_seq: AtomicU64,
    /// 线程只拉一次（start 与 apply_live 两条路都可能触发）。
    threads_started: AtomicBool,
    /// setMyCommands 只需成功一次（进程生命周期内）。
    commands_synced: AtomicBool,
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
    let token =
        settings.token_cipher.as_deref().and_then(
            |cipher| match crate::credentials::decrypt_secret(cipher) {
                Ok(token) => Some(token),
                Err(error) => {
                    log(
                        "warn",
                        "bot token decrypt failed — telegram remote stays off",
                        Some(&error),
                    );
                    None
                }
            },
        );
    let live = LiveConfig {
        enabled: settings.enabled && token.is_some(),
        token,
        chat_id: settings.chat_id,
        prefs: settings.prefs.clone(),
    };

    let (tx, rx) = std::sync::mpsc::sync_channel::<Outbound>(OUTBOUND_QUEUE_CAP);
    let telegram = Telegram {
        app,
        acp,
        outbound: tx,
        inbound_rx: Mutex::new(Some(rx)),
        inner: Mutex::new(Inner::new()),
        live: Mutex::new(live),
        callback_seq: AtomicU64::new(1),
        threads_started: AtomicBool::new(false),
        commands_synced: AtomicBool::new(false),
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

/// 标记该会话刚发出的回合是本地探针（额度面板刷新发的 `/status` /
/// `/usage`，发送方在 `send_prompt` 成功后调用）。回合完成时
/// `observe_turn_complete` 据此跳过推送——否则每次刷新额度都会给
/// Telegram 发一条「✅ 跑完 · 0秒」+ 整张状态卡。
/// Chief 任务卡（broker 推送的唯一出口；relay 模式的逐会话推送不走这里）。
/// 只有 chief 模式才推——relay 模式下 worker 通知走 observe_turn_complete
/// 老路，两条路互斥，不会双推。路由：任务卡登记 chief 会话路由，
/// 回复任务卡 = 对 Chief 说话。
fn staff_task_notification_allowed(task: &crate::chief::types::Task, kind: &str) -> bool {
    crate::chief::staff_commands::telegram_task_notification_allowed(task, kind)
}

pub fn chief_task_notify(task: &crate::chief::types::Task, kind: &str, summary: Option<&str>) {
    crate::debug_log::append(
        "chief",
        "info",
        &task.task_id,
        &format!("telegram notify {kind}"),
        summary,
    );
    let Some(slot) = TELEGRAM.get() else { return };
    let live = slot.live.lock().ok().map(|l| l.clone());
    let Some(live) = live else { return };
    if !live.active() {
        return;
    }
    if !staff_task_notification_allowed(task, kind) {
        crate::debug_log::append(
            "chief",
            "info",
            &task.task_id,
            "telegram notify suppressed by staff policy",
            Some(kind),
        );
        return;
    }
    let (icon, head) = match kind {
        "task.created" => ("🫡", "已接单"),
        "task.started" => ("▶️", "任务开始"),
        "task.completed" => ("✅", "任务完成"),
        "task.failed" => ("❌", "任务失败"),
        "task.cancelled" => ("🛑", "任务已停止"),
        "task.interrupted" => ("⏸", "任务中断（可恢复）"),
        _ => ("•", kind),
    };
    let mut text = format!("{icon} {} — {head}", task.title);
    if let Some(body) = summary.filter(|s| !s.trim().is_empty()) {
        text.push('\n');
        text.push_str(&clip(body.trim(), 800));
    }
    let route_session = task
        .staff_id
        .as_deref()
        .and_then(crate::chief::staff_commands::profile_for_staff_id)
        .map(|profile| profile.session_id)
        .unwrap_or_else(|| task.chief_session_id.clone());
    enqueue_send(&live, text, Vec::new(), Some(route_session), None);
}

pub fn mark_probe_turn(session_id: &str) {
    let Some(slot) = TELEGRAM.get() else { return };
    // 未生效时不留标记：残留标记会误吞启用后第一条真实回合的推送。
    let live_on = slot
        .live
        .lock()
        .ok()
        .map(|live| live.active())
        .unwrap_or(false);
    if !live_on {
        return;
    }
    if let Ok(mut inner) = slot.inner.lock() {
        inner.track(session_id).probe_turn = true;
    }
}

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
                    .map(str::to_string)
                    .unwrap_or_else(|| crate::ts!("agent 进程退出", "agent process exited"));
                push_simple(
                    &live,
                    session_id,
                    "💀",
                    &crate::ts!("agent 进程退出", "agent process exited"),
                    Some(&message),
                );
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
        let Ok(mut inner) = slot.inner.lock() else {
            return;
        };
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
        let Ok(mut inner) = slot.inner.lock() else {
            return;
        };

        if kind.contains("user_message") {
            inner.last_user_session = Some(session_id.to_string());
            let track = inner.track(session_id);
            let fresh = track.turn_started.is_none() || track.waiting == 0;
            turn_started_now = fresh;
            track.turn_started = Some(Instant::now());
            track.last_activity = Instant::now();
            track.text.reset();
            track.begin_turn(&extract_update_images(update), fresh);
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
                let images = extract_update_images(update);
                if !text.is_empty() || !images.is_empty() {
                    let mut started = false;
                    {
                        let track = inner.track(session_id);
                        if track.turn_started.is_none() {
                            track.turn_started = Some(Instant::now());
                            started = true;
                        }
                        track.last_activity = Instant::now();
                        if !text.is_empty() {
                            // Web bridges can revise the complete HTML/Markdown reply.
                            // Keep one current reply for forwarding.
                            if update.pointer("/_meta/marionette.webReply").is_some()
                                || update
                                    .pointer("/_meta/marionette.textSnapshot")
                                    .and_then(Value::as_bool)
                                    == Some(true)
                            {
                                track.text.current.clear();
                            }
                            track.text.append(&text);
                        }
                        track.push_images(images);
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
                    // 工具调用 = agent 消息边界：之前的解说段结业为 previous，
                    // 末段重新累计（推送只要最后一条消息）。
                    if kind.contains("tool_call") {
                        track.text.tool_boundary();
                    }
                    if track.turn_started.is_none() {
                        track.turn_started = Some(Instant::now());
                        started = true;
                    }
                    track.last_activity = Instant::now();
                    track.push_images(extract_update_images(update));
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
    let (elapsed, tail, turn_images, probe) = slot
        .inner
        .lock()
        .ok()
        .and_then(|mut inner| {
            let track = inner.track(session_id);
            let started = track.turn_started.take();
            let probe = std::mem::take(&mut track.probe_turn);
            let tail = track.text.take_final();
            let turn_images = track.take_images();
            // 详情视图的正文缓存：不受通知偏好影响，只在门控下出本机。
            // 探针（/status 卡片）不是回复，别拿它顶掉真实正文。
            if !probe && !tail.trim().is_empty() {
                track.last_reply = clip(tail.trim(), LAST_REPLY_CHARS);
            }
            Some((started.map(|t| t.elapsed()), tail, turn_images, probe))
        })
        .unwrap_or((None, String::new(), Vec::new(), false));
    // 回合结束 = 总控面板的可见状态变化（含 cancelled：推送降噪，面板照刷）。
    schedule_dashboard(slot);
    // 探针回合（额度刷新的 /status、/usage）：面板照刷，推送全免。
    if probe {
        return;
    }
    // Chief 模式聚合（02-architecture.md §8.1）：worker 的逐条回合推送被
    // 任务卡取代（chief_task_notify 负责），Chief 自身的回复照推——那是
    // 给用户的交付面。worker 权限卡不受影响（走 permission prompt 路）。
    // 必须先于下面的员工额度闸：被聚合掉的 worker 回合不能占掉冷却/小时额度，
    // 否则随后的 task.completed 任务卡会被自己的 worker 回合挡掉。
    if crate::chief::broker::is_chief_worker(session_id) {
        return;
    }
    // Probe turns must not consume a staff cooldown/hourly slot.
    let gate_kind = if kind == "error" {
        "task.failed"
    } else {
        "completed"
    };
    if !crate::chief::staff_commands::telegram_notification_allowed(session_id, gate_kind) {
        return;
    }

    if kind == "error" {
        if live.prefs.notify_error {
            let message = data
                .pointer("/error/message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| {
                    data.get("message")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                        .unwrap_or_else(|| crate::ts!("回合报错", "Turn failed with an error"))
                });
            push_simple(
                live,
                session_id,
                "⚠️",
                &crate::ts!("回合出错", "Turn error"),
                Some(&clip(&message, 600)),
            );
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
    let footer = &crate::ts!(
        "💬 直接回复这条消息 = 继续这个会话",
        "💬 Reply directly to this message = continue this session"
    );
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
    // 随附图跟在正文后面。「内容预览」关 = 内容不出本机，图同理。
    if live.prefs.content_preview {
        enqueue_turn_photos(session_id, &label, turn_images, body);
    }
}

/// 回合随附图入队：本轮收集到的图 + 末段正文里引用的 markdown 图片，
/// 保最近 MAX_TURN_IMAGES 张。排在同回合的正文之后（队列保序）。
fn enqueue_turn_photos(session_id: &str, label: &str, mut images: Vec<TurnImage>, body: &str) {
    for src in markdown_image_srcs(body) {
        // 正文里的引用只是路径 / 链接：按前端同一张扩展名表认图。
        if let Some(image) = turn_image_from_src(&src, true) {
            let key = image.key();
            if !images.iter().any(|existing| existing.key() == key) {
                images.push(image);
            }
        }
    }
    let keep_from = images.len().saturating_sub(MAX_TURN_IMAGES);
    let images = images.split_off(keep_from);
    if images.is_empty() {
        return;
    }
    try_enqueue(Outbound::Photos {
        session_id: session_id.to_string(),
        images,
        caption: format!("[{label}] 📷"),
    });
}

/// 回合回复推送：整段正文交给发送线程——优先官方 Rich Messages 直投
/// （`sendRichMessage`，单条 32K 字符、全文不截断），失败再退 MarkdownV2
/// 分片老路。见 `deliver_turn_reply`。
fn enqueue_reply_push(live: &LiveConfig, header: &str, body: &str, footer: &str, session_id: &str) {
    let _ = live;
    let Some(slot) = TELEGRAM.get() else { return };
    let _ = slot.outbound.send(Outbound::TurnReply {
        header: header.to_string(),
        body: body.to_string(),
        footer: footer.to_string(),
        session_id: session_id.to_string(),
    });
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
        let Ok(mut inner) = slot.inner.lock() else {
            return;
        };
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
    if !crate::chief::staff_commands::telegram_notification_allowed(session_id, "permission") {
        return;
    }
    if !live.prefs.notify_wait {
        return;
    }
    let title = data
        .get("title")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| crate::ts!("需要批准", "Needs approval"));
    let detail = data.get("detail").and_then(Value::as_str);

    let label = session_label(session_id);
    let mut text = crate::ts!(
        "[{label}] ⏸ 等你批准：{title}",
        "[{label}] ⏸ Waiting on your approval: {title}"
    );
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
            if !crate::chief::staff_commands::telegram_notification_allowed(
                &pending.session_id,
                "permission",
            ) {
                return;
            }
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
        let Ok(mut inner) = slot.inner.lock() else {
            return;
        };
        inner.track(session_id).waiting += 1;
        inner
            .pending_questions
            .insert(request_id.to_string(), session_id.to_string());
    }
    schedule_dashboard(slot);
    if !crate::chief::staff_commands::telegram_notification_allowed(session_id, "question") {
        return;
    }
    if !live.prefs.notify_wait {
        return;
    }
    let label = session_label(session_id);
    let questions = data.get("questions").and_then(Value::as_array).cloned();

    match question_push_plan(questions.as_ref()) {
        QuestionPush::Buttons { question, options } => {
            let text = crate::ts!(
                "[{label}] ❓ agent 问你：{question}",
                "[{label}] ❓ The agent asks you: {question}"
            );
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
                &crate::ts!("跳过", "Skip"),
            );
            buttons.push(button);
            if let Ok(mut inner) = slot.inner.lock() {
                inner.remember_callback(callback_id, action);
            }
            enqueue_send(live, text, buttons, Some(session_id.to_string()), None);
        }
        QuestionPush::TextOnly { body } => {
            let text = crate::ts!("[{label}] ❓ agent 问你（多问题/多选，回电脑答）：\n{body}", "[{label}] ❓ The agent asks you (multi-question/multi-select — answer on the computer):\n{body}");
            enqueue_send(live, text, Vec::new(), Some(session_id.to_string()), None);
        }
    }
}

fn observe_plan_prompt(slot: &'static Telegram, session_id: &str, data: &Value, live: &LiveConfig) {
    let Some(request_id) = data.get("requestId").and_then(Value::as_str) else {
        return;
    };
    {
        let Ok(mut inner) = slot.inner.lock() else {
            return;
        };
        inner.track(session_id).waiting += 1;
        inner
            .pending_plans
            .insert(request_id.to_string(), session_id.to_string());
    }
    schedule_dashboard(slot);
    if !crate::chief::staff_commands::telegram_notification_allowed(session_id, "plan") {
        return;
    }
    if !live.prefs.notify_wait {
        return;
    }
    let plan = data
        .get("planMarkdown")
        .or_else(|| data.get("plan_markdown"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let label = session_label(session_id);
    // MarkdownV2 优先渲染（计划正文就是 markdown），纯文本回退体带原文。
    let trimmed = plan.trim();
    let mut md = crate::ts!("[{label}] 📋 计划待审", "[{label}] 📋 Plan awaiting review");
    let mut plain = md.clone();
    if live.prefs.content_preview && !trimmed.is_empty() {
        let body = clip(trimmed, 1500);
        md.push_str("\n\n");
        md.push_str(&md_to_markdownv2(&body));
        plain.push_str("\n\n");
        plain.push_str(&body);
    }
    let mut buttons = Vec::new();
    for (button_text, decision) in [
        (crate::ts!("✅ 批准", "✅ Approve"), "approve"),
        (
            crate::ts!("✏️ 继续改", "✏️ Request changes"),
            "request_changes",
        ),
    ] {
        let (button, callback_id, action) = make_button(
            slot,
            CallbackAction::Plan {
                session_id: session_id.to_string(),
                request_id: request_id.to_string(),
                decision: decision.to_string(),
                decision_label: button_text.to_string(),
            },
            &button_text,
        );
        buttons.push(button);
        if let Ok(mut inner) = slot.inner.lock() {
            inner.remember_callback(callback_id, action);
        }
    }
    enqueue_send_md_with_plain(live, md, plain, buttons, Some(session_id.to_string()), None);
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
    let running = rows
        .iter()
        .filter(|r| r.state == DashState::Running)
        .count();
    let waiting = rows
        .iter()
        .filter(|r| r.state == DashState::Waiting)
        .count();
    let mut head = crate::ts!("📌 Marionette 总控", "📌 Marionette Dashboard");
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
            DashState::Waiting => crate::ts!(
                "{marker}⏸ [{}] · 等你",
                "{marker}⏸ [{}] · Waiting on you",
                row.label
            ),
            DashState::Idle => crate::ts!(
                "{marker}💤 [{}] · {}前有动静",
                "{marker}💤 [{}] · Active {} ago",
                row.label,
                format_age_short(row.idle_secs)
            ),
        };
        lines.push(line);
    }
    if lines.is_empty() {
        out.push_str(&crate::ts!("💤 没有会话在跑\n", "💤 No sessions running\n"));
    } else {
        out.push_str(&lines.join("\n"));
        out.push('\n');
    }
    if idle_count > 0 {
        out.push_str(&crate::ts!(
            "\n💤 其余 {idle_count} 个会话空闲",
            "\n💤 {idle_count} other sessions idle"
        ));
    }
    out.push_str(&crate::ts!(
        "\n💡 点会话名 = 切换绑定 · 📄 = 会话详情 · /status 看全部",
        "\n💡 Tap a session name = rebind · 📄 = session details · /status for all"
    ));
    out
}

/// 纯渲染：/status 全量清单文本。
fn render_status(rows: &[DashRow], total: usize) -> String {
    let mut out = crate::ts!(
        "📋 会话清单（{}/{}）\n\n",
        "📋 Sessions ({}/{})\n\n",
        rows.len(),
        total
    );
    let lines: Vec<String> = rows
        .iter()
        .map(|row| {
            let marker = if row.bound { "⏎ " } else { "" };
            let tail = match row.state {
                DashState::Running => crate::ts!(
                    "跑了 {}",
                    "Running for {}",
                    row.elapsed_secs
                        .map(format_duration_secs)
                        .unwrap_or_default()
                ),
                DashState::Waiting => {
                    crate::ts!("等你批准/回答", "Waiting on you to approve/answer")
                }
                DashState::Idle => crate::ts!(
                    "{}前有动静",
                    "Active {} ago",
                    format_age_short(row.idle_secs)
                ),
            };
            let agent = if row.agent.is_empty() {
                String::new()
            } else {
                format!(" · {}", row.agent)
            };
            format!(
                "{}{} [{}]{agent} · {tail}",
                marker,
                row.state.icon(),
                row.label
            )
        })
        .collect();
    out.push_str(&lines.join("\n"));
    out.push_str(&crate::ts!("\n\n💬 回复某条通知 = 定向送达；裸发文字 → ⏎ 标记的会话（总控面板上可点选）。", "\n\n💬 Reply to a notice = deliver to that session; plain text → the ⏎-marked session (tap to choose on the dashboard)."));
    out
}

/// 快照 + 取名 + 组装 + 投递。绝不在持有 inner 锁时碰 storage/acp（锁序红线）。
/// 置顶面板只有一个 message_id，当前视图（总控 / 某会话详情）由 `Inner.view`
/// 决定——点 📄 原地切到详情，点 ⬅ 切回总控；状态变化照常触发重画（画的是
/// 当前视图）。
fn schedule_dashboard(slot: &'static Telegram) {
    let live = slot.live.lock().ok().map(|l| l.clone());
    let Some(live) = live else { return };
    if !live.active() {
        return;
    }
    let view = slot.inner.lock().ok().map(|inner| inner.view.clone());
    let (text, keyboard, rich) = match view {
        Some(PanelView::Session(session_id)) => {
            match session_detail_parts(slot, &session_id, live.prefs.content_preview) {
                Some(parts) => parts,
                // 会话查不到（没有 track）：退回总控视图。
                None => dashboard_parts(slot),
            }
        }
        _ => dashboard_parts(slot),
    };

    let _ = slot.outbound.send(Outbound::Dashboard {
        text,
        keyboard,
        rich,
    });
}

/// 总控面板的文本与键盘（会话行 = [名字, 📄, ⏹]，末行 = [🔄]）。
fn dashboard_parts(slot: &'static Telegram) -> (String, Vec<Vec<TgButton>>, bool) {
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
        let (view_btn, view_id, view_action) = make_button(
            slot,
            CallbackAction::ViewSession {
                session_id: row.session_id.clone(),
                label: row.label.clone(),
            },
            "📄",
        );
        actions.push((view_btn.clone(), view_id, view_action));
        let mut line = vec![bind_btn, view_btn];
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
    let (refresh_btn, refresh_id, refresh_action) = make_button(
        slot,
        CallbackAction::RefreshDashboard,
        &crate::ts!("🔄 刷新", "🔄 Refresh"),
    );
    actions.push((refresh_btn.clone(), refresh_id, refresh_action));
    keyboard.push(vec![refresh_btn]);
    if let Ok(mut inner) = slot.inner.lock() {
        for (button, id, action) in actions {
            let _ = button;
            inner.remember_callback(id, action);
        }
    }
    (text, keyboard, false)
}

/// 会话详情视图的文本与键盘。正文受 `content_preview` 偏好门控
/// （关 = 内容不出本机，只给状态）。
fn session_detail_parts(
    slot: &'static Telegram,
    session_id: &str,
    include_body: bool,
) -> Option<(String, Vec<Vec<TgButton>>, bool)> {
    let row = session_row(slot, session_id)?;
    let last_reply = slot
        .inner
        .lock()
        .ok()
        .and_then(|inner| {
            inner
                .sessions
                .get(session_id)
                .map(|track| track.last_reply.clone())
        })
        .unwrap_or_default();
    let text = render_session_detail(
        &row,
        if include_body {
            last_reply.as_str()
        } else {
            ""
        },
        include_body,
    );

    let mut actions: Vec<(TgButton, u64, CallbackAction)> = Vec::new();
    let mut keyboard: Vec<Vec<TgButton>> = Vec::new();
    let (back_btn, back_id, back_action) = make_button(
        slot,
        CallbackAction::PanelBack,
        &crate::ts!("⬅ 返回总控", "⬅ Back to dashboard"),
    );
    actions.push((back_btn.clone(), back_id, back_action));
    let mut line = vec![back_btn];
    if row.state != DashState::Idle {
        let (stop_btn, stop_id, stop_action) = make_button(
            slot,
            CallbackAction::StopTurn {
                session_id: session_id.to_string(),
                label: row.label.clone(),
            },
            &crate::ts!("⏹ 停止", "⏹ Stop"),
        );
        actions.push((stop_btn.clone(), stop_id, stop_action));
        line.push(stop_btn);
    }
    keyboard.push(line);
    let (bind_btn, bind_id, bind_action) = make_button(
        slot,
        CallbackAction::BindSession {
            session_id: session_id.to_string(),
            label: row.label.clone(),
        },
        &crate::ts!("⏎ 设为绑定", "⏎ Set as bound"),
    );
    actions.push((bind_btn.clone(), bind_id, bind_action));
    keyboard.push(vec![bind_btn]);
    if let Ok(mut inner) = slot.inner.lock() {
        for (button, id, action) in actions {
            let _ = button;
            inner.remember_callback(id, action);
        }
    }
    Some((text, keyboard, true))
}

/// 单会话的行数据（详情视图用）。没有 track（从未见动静）返回 None。
fn session_row(slot: &'static Telegram, session_id: &str) -> Option<DashRow> {
    let raw = slot.inner.lock().ok().and_then(|inner| {
        inner.sessions.get(session_id).map(|track| {
            (
                track.state(),
                track.turn_started.map(|t| t.elapsed().as_secs()),
                track.last_activity.elapsed().as_secs(),
                inner.last_user_session.as_deref() == Some(session_id),
            )
        })
    });
    let Some((state, elapsed_secs, idle_secs, bound)) = raw else {
        return None;
    };
    let meta = session_meta(session_id);
    Some(DashRow {
        session_id: session_id.to_string(),
        label: meta.label,
        agent: meta.agent,
        state,
        elapsed_secs,
        idle_secs,
        bound,
    })
}

/// 纯渲染：会话详情视图文本。正文是原生 markdown，由 `editMessageText` 的
/// `rich_message` 参数渲染（客户端原生表格/代码块/标题）。
fn render_session_detail(row: &DashRow, body: &str, body_allowed: bool) -> String {
    let marker = if row.bound { "⏎ " } else { "" };
    let agent = if row.agent.is_empty() {
        String::new()
    } else {
        format!(" · {}", row.agent)
    };
    let state_line = match row.state {
        DashState::Running => crate::ts!(
            "▶ 跑了 {}",
            "▶ Running for {}",
            row.elapsed_secs
                .map(format_duration_secs)
                .unwrap_or_else(|| "…".to_string())
        ),
        DashState::Waiting => crate::ts!("⏸ 等你批准/回答", "⏸ Waiting on you to approve/answer"),
        DashState::Idle => crate::ts!(
            "💤 {}前有动静",
            "💤 Active {} ago",
            format_age_short(row.idle_secs)
        ),
    };
    let mut out = format!("📄 {marker}[{}]{agent} · {state_line}\n\n", row.label);
    if !body_allowed {
        out.push_str(&crate::ts!(
            "🔒 内容预览已关闭——正文不出本机（设置里可开）",
            "🔒 Content preview is off — message bodies stay on this computer (enable in settings)"
        ));
    } else if body.trim().is_empty() {
        out.push_str(&crate::ts!(
            "（最近一条回复为空——回合还没跑完，或 agent 没有输出文本）",
            "(last reply is empty — the turn isn't finished, or the agent produced no text)"
        ));
    } else {
        out.push_str(body.trim());
    }
    out.push_str(&crate::ts!(
        "\n\n💡 回复任意一条通知 = 定向继续这个会话",
        "\n\n💡 Reply to any notice = continue that session directly"
    ));
    out
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
    let Some((raw_rows, bound)) = raw else {
        return (Vec::new(), 0);
    };

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
        crate::ts!("{secs}秒", "{secs}s")
    } else if secs < 3600 {
        crate::ts!("{}分钟", "{}m", secs / 60)
    } else if secs < 86400 {
        crate::ts!("{}小时{}分", "{}h{}m", secs / 3600, secs % 3600 / 60)
    } else {
        crate::ts!("{}天", "{}d", secs / 86400)
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
        text.push_str(&crate::ts!("：", ":"));
        text.push_str(&clip(detail.trim(), 600));
    }
    enqueue_send(live, text, Vec::new(), Some(session_id.to_string()), None);
}

/// 投递到发送线程。fire-and-forget：通道断了（进程退出）就直接丢，
/// 绝不反压事件收口。队列满时同样丢新消息——长故障期宁可丢通知，
/// 也不能让 poller/事件线程堵在 send 上。
fn enqueue_send(
    live: &LiveConfig,
    text: String,
    buttons: Vec<TgButton>,
    route_for_session: Option<String>,
    reply_to: Option<i64>,
) {
    let _ = live;
    try_enqueue(Outbound::Send {
        text,
        buttons,
        route_for_session,
        reply_to,
        md: false,
        plain: None,
    });
}

/// MarkdownV2 优先、带按钮、并可指定纯文本回退体（回退体不带转义反斜杠，
/// 读起来比 MarkdownV2 原文干净——计划卡片用）。
fn enqueue_send_md_with_plain(
    live: &LiveConfig,
    md: String,
    plain: String,
    buttons: Vec<TgButton>,
    route_for_session: Option<String>,
    reply_to: Option<i64>,
) {
    let _ = live;
    try_enqueue(Outbound::Send {
        text: md,
        buttons,
        route_for_session,
        reply_to,
        md: true,
        plain: Some(plain),
    });
}

/// 有界队列投递：满则丢 + 按 2 的幂次低频留痕（1,2,4,8…次时各记一条）。
fn try_enqueue(job: Outbound) {
    let Some(slot) = TELEGRAM.get() else { return };
    if slot.outbound.try_send(job).is_err() {
        let n = OUTBOUND_DROPS.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_power_of_two() {
            log(
                "warn",
                &format!("telegram outbound queue full — dropped {n} messages so far"),
                None,
            );
        }
    }
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
        let mut plain_fallback: Option<String> = None;
        let mut is_dashboard = false;
        let mut dashboard_rich = false;
        let mut turn: Option<(String, String, String, String)> = None;
        let mut photos: Option<(String, Vec<TurnImage>, String)> = None;
        match job {
            Outbound::Send {
                text,
                buttons,
                route_for_session: route,
                reply_to: reply,
                md,
                plain,
            } => {
                job_text = text;
                job_keyboard = if buttons.is_empty() {
                    Vec::new()
                } else {
                    vec![buttons]
                };
                route_for_session = route;
                reply_to = reply;
                plain_fallback = plain;
                if md {
                    parse_mode = Some("MarkdownV2");
                }
            }
            Outbound::TurnReply {
                header,
                body,
                footer,
                session_id,
            } => {
                turn = Some((header, body, footer, session_id));
                job_text = String::new();
                job_keyboard = Vec::new();
            }
            Outbound::Photos {
                session_id,
                images,
                caption,
            } => {
                photos = Some((session_id, images, caption));
                job_text = String::new();
                job_keyboard = Vec::new();
            }
            Outbound::Ack {
                ok,
                error,
                session_label,
                reply_to: reply,
            } => {
                job_text = if ok {
                    crate::ts!(
                        "✓ 已送进 [{session_label}]",
                        "✓ Delivered to [{session_label}]"
                    )
                } else {
                    crate::ts!(
                        "⚠️ 没送进去（{session_label}）：{}",
                        "⚠️ Not delivered ({session_label}): {}",
                        clip(&error.unwrap_or_default(), 400)
                    )
                };
                job_keyboard = Vec::new();
                reply_to = reply;
            }
            Outbound::Dashboard {
                text,
                keyboard,
                rich,
            } => {
                job_text = text;
                job_keyboard = keyboard;
                is_dashboard = true;
                dashboard_rich = rich;
            }
        };

        if let Some((header, body, footer, session_id)) = turn {
            deliver_turn_reply(slot, &live, &header, &body, &footer, &session_id);
            continue;
        }

        if let Some((session_id, images, caption)) = photos {
            deliver_turn_photos(slot, &live, &session_id, images, &caption);
            continue;
        }

        if is_dashboard {
            deliver_dashboard(&live, &job_text, &job_keyboard, dashboard_rich);
            continue;
        }

        match send_message(
            &live,
            &job_text,
            &job_keyboard,
            reply_to,
            parse_mode,
            plain_fallback.as_deref(),
        ) {
            Ok(message_id) => {
                backoff = BACKOFF_MIN;
                if let (Some(session_id), Some(message_id)) = (route_for_session, message_id) {
                    if let Ok(mut inner) = slot.inner.lock() {
                        inner.remember_route(message_id, &session_id);
                    }
                }
            }
            Err(error) => {
                log(
                    "warn",
                    "sendMessage failed — queue keeps draining",
                    Some(&error),
                );
                if let Ok(mut inner) = slot.inner.lock() {
                    inner.last_error = Some(error);
                }
                thread::sleep(backoff);
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

/// 回合回复投递：优先官方 Rich Messages（Bot API 10.1 `sendRichMessage`，
/// `rich_message.markdown` 收原生 markdown——不转义、标题/表格/代码块/公式
/// 由客户端渲染，单条 32K 字符所以全文不截断）；任何失败退回 MarkdownV2
/// 分片老路（每片 parse 失败再退纯文本）。投递是底线，富格式是增益。
fn deliver_turn_reply(
    slot: &'static Telegram,
    live: &LiveConfig,
    header: &str,
    body: &str,
    footer: &str,
    session_id: &str,
) {
    let rich = format!("{header}\n\n{body}\n\n{footer}");
    match send_rich_message(live, &rich) {
        Ok(message_id) => {
            if let Some(message_id) = message_id {
                if let Ok(mut inner) = slot.inner.lock() {
                    inner.remember_route(message_id, session_id);
                }
            }
            return;
        }
        Err(error) => {
            log(
                "warn",
                "sendRichMessage failed — falling back to MarkdownV2 chunks",
                Some(&error),
            );
        }
    }

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
        let mut plain = String::new();
        if index == 0 {
            md.push_str(&md_escape(header));
            md.push_str("\n\n");
            plain.push_str(header);
            plain.push_str("\n\n");
        }
        md.push_str(&md_to_markdownv2(chunk));
        md.push('\n');
        plain.push_str(chunk);
        if index == last {
            md.push_str("\n");
            md.push_str(&md_escape(footer));
            plain.push_str("\n\n");
            plain.push_str(footer);
            if truncated {
                let note = crate::ts!(
                    "\n…（正文太长已截断，回电脑看全文）",
                    "\n…(body truncated — see the full text on the computer)"
                );
                md.push_str(&md_escape(&note));
                plain.push_str(&note);
            }
        } else {
            md.push_str(&md_escape(&crate::ts!(
                "…（接下条）",
                "…(continued next message)"
            )));
            plain.push_str(&crate::ts!("\n…（接下条）", "\n…(continued next message)"));
        }
        match send_message(live, &md, &[], None, Some("MarkdownV2"), Some(&plain)) {
            Ok(Some(message_id)) => {
                if let Ok(mut inner) = slot.inner.lock() {
                    inner.remember_route(message_id, session_id);
                }
            }
            Ok(None) => {}
            Err(error) => log("warn", "turn reply chunk failed", Some(&error)),
        }
    }
}

/// 回合随附图的投递：逐张发出，每张的 message_id 记进路由表（在 Telegram
/// 里回复这张图 = 路由到该会话）。正文已经在前面落地，所以图片失败不重试
/// ——投递的底线是文字，图是增益；发不出去的几张合并成一条提示。
fn deliver_turn_photos(
    slot: &'static Telegram,
    live: &LiveConfig,
    session_id: &str,
    images: Vec<TurnImage>,
    caption: &str,
) {
    // 相对路径按会话 cwd 补全；只在真有相对路径时才去读一次 storage
    // （锁序红线：这里没有持有 inner 锁）。
    let is_relative =
        |image: &TurnImage| matches!(image, TurnImage::Local(path) if path.is_relative());
    let cwd = if images.iter().any(is_relative) {
        session_cwd(session_id)
    } else {
        None
    };
    let mut caption_pending = true;
    let mut failed: Vec<(String, String)> = Vec::new();
    for image in images {
        if is_relative(&image) && cwd.is_none() {
            // 不知道相对谁，别拿进程自己的 cwd 去碰运气。
            log(
                "info",
                "turn image skipped — relative path without a session cwd",
                Some(&image.describe()),
            );
            continue;
        }
        let image = image.resolve(cwd.as_deref());
        let this_caption = if caption_pending { caption } else { "" };
        match send_image(live, &image, this_caption) {
            Ok(message_id) => {
                caption_pending = false;
                if let Some(message_id) = message_id {
                    if let Ok(mut inner) = slot.inner.lock() {
                        inner.remember_route(message_id, session_id);
                    }
                }
            }
            Err(error) if error == IMAGE_NOT_FOUND => {
                log(
                    "info",
                    "turn image skipped — file not found",
                    Some(&image.describe()),
                );
            }
            Err(error) => {
                log("warn", "turn image send failed", Some(&error));
                // 传输层失败（断网 / 超时）：后面的图和提示大概率同样发不
                // 出去，别让每张都干等一个超时、把发送线程拖住。
                if tg_error_status(&error).is_none() {
                    return;
                }
                failed.push((image.describe(), clip(&error, 120)));
            }
        }
    }
    if failed.is_empty() {
        return;
    }
    let mut note = crate::ts!(
        "📷 {} 张图没能发到 Telegram：\n",
        "📷 {} image(s) could not be sent to Telegram:\n",
        failed.len()
    );
    for (desc, reason) in &failed {
        note.push_str("• ");
        note.push_str(&clip(desc, 200));
        note.push_str(&crate::ts!("（", " ("));
        note.push_str(reason);
        note.push_str(&crate::ts!("）\n", ")\n"));
    }
    enqueue_send(live, note, Vec::new(), Some(session_id.to_string()), None);
}

/// Bot API 方法与对应的 multipart / 请求字段名。
const SEND_PHOTO: (&str, &str) = ("sendPhoto", "photo");
const SEND_ANIMATION: (&str, &str) = ("sendAnimation", "animation");
const SEND_DOCUMENT: (&str, &str) = ("sendDocument", "document");

/// 发一张图。照片走 `sendPhoto`、GIF 走 `sendAnimation`、其余按文件
/// `sendDocument`（见 `TurnImage::send_as`）；Telegram 拒收时退成文件再试
/// 一次——它只认 jpg / png / webp 是照片，bmp / svg / 超大尺寸的走这条路。
fn send_image(live: &LiveConfig, image: &TurnImage, caption: &str) -> Result<Option<i64>, String> {
    let Some(token) = live.token.as_deref() else {
        return Err("no token".to_string());
    };
    let Some(chat_id) = live.chat_id else {
        return Err("no paired chat".to_string());
    };
    let caption = clip_utf16(caption.trim(), CAPTION_MAX_UNITS);
    let timeout = Duration::from_secs(IMAGE_HTTP_TIMEOUT_SECS);
    let attempts: Vec<(&str, &str)> = match image.send_as() {
        SEND_DOCUMENT => vec![SEND_DOCUMENT],
        primary => vec![primary, SEND_DOCUMENT],
    };
    match image {
        TurnImage::Remote(url) => {
            let mut last_error = String::new();
            for &(method, field) in &attempts {
                let mut body = json!({ "chat_id": chat_id });
                body[field] = json!(url);
                if !caption.is_empty() {
                    body["caption"] = json!(caption);
                }
                match tg_call(token, method, &body, timeout) {
                    Ok(value) => return Ok(message_id_of(&value)),
                    Err(error) if rejected(&error) => last_error = error,
                    Err(error) => return Err(error),
                }
            }
            Err(last_error)
        }
        TurnImage::Local(path) => {
            let metadata = std::fs::metadata(path).map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    IMAGE_NOT_FOUND.to_string()
                } else {
                    format!("{}: {error}", path.display())
                }
            })?;
            if metadata.len() > IMAGE_MAX_BYTES {
                return Err(too_large(metadata.len()));
            }
            let data =
                std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
            upload_image(
                token,
                chat_id,
                &Upload {
                    data: &data,
                    filename: upload_filename(path),
                    mime: mime_for_path(path),
                },
                &caption,
                &attempts,
                timeout,
            )
        }
        TurnImage::Bytes { data, mime } => {
            let bytes = decode_base64_image(data)?;
            upload_image(
                token,
                chat_id,
                &Upload {
                    data: &bytes,
                    filename: format!("image.{}", mime_extension(mime)),
                    mime: mime.trim().to_string(),
                },
                &caption,
                &attempts,
                timeout,
            )
        }
    }
}

/// 要上传的一份字节。
struct Upload<'a> {
    data: &'a [u8],
    filename: String,
    mime: String,
}

/// 同一份字节按 attempts 依次上传（被拒就换下一种发法）。
fn upload_image(
    token: &str,
    chat_id: i64,
    upload: &Upload<'_>,
    caption: &str,
    attempts: &[(&str, &str)],
    timeout: Duration,
) -> Result<Option<i64>, String> {
    let mut fields: Vec<(&str, String)> = vec![("chat_id", chat_id.to_string())];
    if !caption.is_empty() {
        fields.push(("caption", caption.to_string()));
    }
    let mut last_error = String::new();
    for &(method, field) in attempts {
        let file = UploadFile {
            field,
            filename: upload.filename.clone(),
            mime: upload.mime.clone(),
            data: upload.data,
        };
        match tg_call_multipart(token, method, &fields, &file, timeout) {
            Ok(value) => return Ok(message_id_of(&value)),
            Err(error) if rejected(&error) => last_error = error,
            Err(error) => return Err(error),
        }
    }
    Err(last_error)
}

/// image 块的 base64 → 字节。先按长度估解码后大小，超限的不解码
/// （别为一张发不出去的图白占内存）。
fn decode_base64_image(data: &str) -> Result<Vec<u8>, String> {
    use base64::{
        engine::general_purpose::STANDARD as B64,
        engine::general_purpose::STANDARD_NO_PAD as B64_NO_PAD, Engine,
    };
    let compact: String = data.chars().filter(|c| !c.is_whitespace()).collect();
    let estimated = compact.len().saturating_mul(3) / 4;
    if estimated as u64 > IMAGE_MAX_BYTES {
        return Err(too_large(estimated as u64));
    }
    B64.decode(compact.as_bytes())
        .or_else(|_| B64_NO_PAD.decode(compact.as_bytes()))
        .map_err(|error| format!("invalid base64 image: {error}"))
}

fn too_large(bytes: u64) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    format!(
        "image is {mb:.1} MB, over the {} MB limit",
        IMAGE_MAX_BYTES / 1024 / 1024
    )
}

/// 上传用的文件名：只取文件名部分，去掉会让 Content-Disposition 头断行的字符。
/// Telegram 靠扩展名辅助识别类型，所以后缀要留住。
fn upload_filename(path: &Path) -> String {
    let name: String = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|ch| {
            if ch == '"' || ch == '\\' || ch.is_control() {
                '_'
            } else {
                ch
            }
        })
        .collect();
    if name.is_empty() {
        "image".to_string()
    } else {
        name
    }
}

fn mime_for_path(path: &Path) -> String {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// MIME 子型 → 文件扩展名（内联字节没有文件名，按类型补一个）。
fn mime_extension(mime: &str) -> &'static str {
    match mime_subtype(mime).as_str() {
        "png" => "png",
        "jpg" | "jpeg" => "jpg",
        "gif" => "gif",
        "webp" => "webp",
        "bmp" => "bmp",
        "svg" | "svg+xml" => "svg",
        _ => "bin",
    }
}

/// Telegram 明确拒收了这个文件（4xx；429 是限流，换发法也没用）。
fn rejected(error: &str) -> bool {
    matches!(tg_error_status(error), Some(code) if (400..500).contains(&code) && code != 429)
}

/// `HTTP 400 · …` 形态错误里的状态码；传输层失败 / 本地错误没有。
fn tg_error_status(error: &str) -> Option<u16> {
    let rest = error.strip_prefix("HTTP ")?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn message_id_of(value: &Value) -> Option<i64> {
    value.pointer("/result/message_id").and_then(Value::as_i64)
}

/// Rich Messages 直投：`rich_message.markdown` 收原生 markdown。返回
/// Telegram 分配的 message_id。
fn send_rich_message(live: &LiveConfig, markdown: &str) -> Result<Option<i64>, String> {
    let Some(token) = live.token.as_deref() else {
        return Err("no token".to_string());
    };
    let Some(chat_id) = live.chat_id else {
        return Err("no paired chat".to_string());
    };
    let body = rich_request_body(chat_id, markdown);
    let value = tg_call(
        token,
        "sendRichMessage",
        &body,
        Duration::from_secs(SEND_HTTP_TIMEOUT_SECS),
    )?;
    Ok(value.pointer("/result/message_id").and_then(Value::as_i64))
}

/// `sendRichMessage` 请求体（纯函数，形状由单测钉住）。
fn rich_request_body(chat_id: i64, markdown: &str) -> Value {
    json!({
        "chat_id": chat_id,
        "rich_message": { "markdown": clip_utf16(markdown, RICH_MARKDOWN_LIMIT) },
    })
}

/// 面板投递：已有 message_id 就原地 edit；edit 失败（消息被删等）就发新 + 置顶。
/// `rich=true`（会话详情视图）先按 `rich_message` 参数编辑（原生 markdown 渲染），
/// 被拒再退普通 text 编辑——按钮永远保住。
fn deliver_dashboard(live: &LiveConfig, text: &str, keyboard: &[Vec<TgButton>], rich: bool) {
    let Some(slot) = TELEGRAM.get() else { return };
    let existing = slot
        .inner
        .lock()
        .ok()
        .and_then(|mut inner| inner.dashboard.take().map(|handle| handle.message_id));
    if let Some(message_id) = existing {
        let edited = if rich {
            edit_message(live, message_id, text, keyboard, true)
                .or_else(|_| edit_message(live, message_id, text, keyboard, false))
        } else {
            edit_message(live, message_id, text, keyboard, false)
        };
        match edited {
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
    // 重建走普通 sendMessage：sendRichMessage 不带普通内联键盘，按钮优先。
    match send_message(live, text, keyboard, None, None, None) {
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
/// `plain_fallback`：MarkdownV2 发送失败（解析拒绝、转义后超长……）时改发
/// 这段纯文本（None = 用 text 本身）——格式化是增益，投递是底线。
fn send_message(
    live: &LiveConfig,
    text: &str,
    keyboard: &[Vec<TgButton>],
    reply_to: Option<i64>,
    parse_mode: Option<&str>,
    plain_fallback: Option<&str>,
) -> Result<Option<i64>, String> {
    match send_message_inner(live, text, keyboard, reply_to, parse_mode) {
        Err(ref error) if parse_mode.is_some() => {
            log(
                "warn",
                "MarkdownV2 send failed — resending as plain text",
                Some(error),
            );
            let fallback = plain_fallback.unwrap_or(text);
            send_message_inner(live, fallback, keyboard, reply_to, None)
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

/// 原地编辑面板消息（文本 + 键盘一起换）。`rich=true` 时正文走
/// `rich_message` 参数（Bot API 10.1 给 editMessageText 加的，原生 markdown
/// 渲染）。
fn edit_message(
    live: &LiveConfig,
    message_id: i64,
    text: &str,
    keyboard: &[Vec<TgButton>],
    rich: bool,
) -> Result<(), String> {
    let Some(token) = live.token.as_deref() else {
        return Err("no token".to_string());
    };
    let Some(chat_id) = live.chat_id else {
        return Err("no paired chat".to_string());
    };
    let mut body = if rich {
        json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "rich_message": { "markdown": clip_utf16(text, RICH_MARKDOWN_LIMIT) },
        })
    } else {
        json!({
            "chat_id": chat_id,
            "message_id": message_id,
            "text": clip(text, MAX_MESSAGE_CHARS),
        })
    };
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
    // 重启恢复：从游标文件续读（首次运行为 0 = 全量）。游标按 bot 编号
    // 绑定，等拿到 token 后在循环里装载 / 换 bot 时清零。
    let mut offset: i64 = 0;
    let mut cursor_bot: Option<String> = None;
    let mut backoff = BACKOFF_MIN;
    // 连续 not-ok 应答计数：跟 Err 分支同款指数退避，别固定 15s 敲到天荒地老。
    let mut not_ok_streak: u32 = 0;
    // 401/403 期间只记一条警告（last_error 持续可见），不随退避节奏刷屏。
    let mut auth_parked = false;
    let mut parked_token: Option<String> = None;
    loop {
        let live = slot.live.lock().ok().map(|l| l.clone());
        let Some(live) = live else {
            thread::sleep(Duration::from_secs(5));
            continue;
        };
        // token 换新 = 用户对 401 停用的回应，恢复轮询。
        let Some(token) = live.token.clone() else {
            thread::sleep(Duration::from_secs(2));
            continue;
        };
        if auth_parked && parked_token.as_deref() != Some(token.as_str()) {
            auth_parked = false;
            log("info", "telegram token replaced — resuming polling", None);
        }

        let bot_id = bot_id_of(&token);
        if cursor_bot.as_deref() != Some(bot_id.as_str()) {
            offset = if cursor_bot.is_none() {
                load_cursor(&bot_id)
            } else {
                0
            };
            cursor_bot = Some(bot_id.clone());
        }
        let base = api_base();
        let url = format!(
            "{base}/bot{token}/getUpdates?timeout={POLL_TIMEOUT_SECS}&offset={offset}&allowed_updates=%5B%22message%22%2C%22callback_query%22%5D"
        );
        let response = crate::http_client::agent_with_proxy()
            .and_then(|agent| {
                agent
                    .get(&url)
                    .timeout(Duration::from_secs(POLL_HTTP_TIMEOUT_SECS))
                    .call()
                    .map_err(|e| sanitize_error("getUpdates", &e.to_string()))
            })
            .and_then(|resp| {
                resp.into_string()
                    .map_err(|e| format!("read getUpdates body: {e}"))
            })
            .and_then(|text| {
                serde_json::from_str::<Value>(&text).map_err(|e| format!("parse getUpdates: {e}"))
            });

        match response {
            Ok(value) => {
                backoff = BACKOFF_MIN;
                if let Ok(mut inner) = slot.inner.lock() {
                    inner.last_error = None;
                }
                if value.get("ok").and_then(Value::as_bool) != Some(true) {
                    // API 层错误对象（409 之外的 4xx 也长这样）。
                    not_ok_streak = not_ok_streak.saturating_add(1);
                    let desc = value
                        .pointer("/description")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    log("warn", "getUpdates returned not-ok", Some(desc));
                    // 15s 起、翻倍、60s 封顶（streak 1→15s, 2→30s, ≥3→60s）。
                    let wait = BACKOFF_409_START
                        .checked_mul(1u32 << not_ok_streak.saturating_sub(1).min(3))
                        .unwrap_or(BACKOFF_MAX)
                        .min(BACKOFF_MAX);
                    thread::sleep(wait);
                    continue;
                }
                not_ok_streak = 0;
                let updates = value
                    .get("result")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let offset_before = offset;
                for update in updates {
                    if let Some(update_id) = update.get("update_id").and_then(Value::as_i64) {
                        offset = offset.max(update_id + 1);
                    }
                    handle_update(slot, &live, update);
                }
                // 游标落盘（每批一次；poller 是唯一写者）。收到即推进：
                // 崩溃窗口内丢的是「处理中」的那一批，不产生重复任务
                // （broker requestId / callback 只接受一次兜底）。
                // 空批（50s 长轮询超时）不重复写盘。
                if offset != offset_before {
                    save_cursor(&bot_id, offset);
                }
            }
            Err(error) => {
                let is_conflict = error.contains("409") || error.contains("Conflict");
                // token 被吊销/无权限：再试一万次也是 401。停用轮询等待换 token
                // （上面 token 变化检测恢复），不随退避节奏刷屏。
                let is_auth_reject =
                    !is_conflict && (error.contains("401") || error.contains("403"));
                if is_auth_reject && !auth_parked {
                    auth_parked = true;
                    parked_token = Some(token.clone());
                    log(
                        "warn",
                        "telegram rejected the bot token (401/403) — polling parked; replace the token in settings to resume",
                        Some(&error),
                    );
                }
                if auth_parked {
                    if let Ok(mut inner) = slot.inner.lock() {
                        inner.last_error = Some(error.clone());
                    }
                    thread::sleep(BACKOFF_MAX);
                    continue;
                }
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

/// 处理一条 update。**在独立线程上跑**：命令回复 / 回调应答 / 菜单翻页全是
/// 同步 HTTP（sendMessage、answerCallbackQuery、editMessageText），在 poller
/// 线程里跑的话每条交互都会把 getUpdates 长轮询整段卡住（audit P1-B）。
/// `live` 是每轮循环里克隆的快照（derive Clone），move 进线程安全。
fn handle_update(slot: &'static Telegram, live: &LiveConfig, update: Value) {
    let live = live.clone();
    thread::spawn(move || {
        if let Some(message) = update.get("message").cloned() {
            handle_incoming_message(slot, &live, message);
        } else if let Some(callback) = update.get("callback_query").cloned() {
            handle_callback_query(slot, &live, callback);
        }
    });
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

    // 命令面：/status 全量清单、/new 向导、/model 切模型；其余命令回用法提示。
    if text.starts_with('/') {
        let command = text
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        // BotFather 注册的命令可能带 @botname 后缀。
        let command = command.split('@').next().unwrap_or("").to_string();
        if crate::chief::staff_commands::telegram_chat_routes_to_staff(chat_id) {
            if command == "/stop" {
                let text = match crate::chief::broker::chief_stop_all(&slot.app) {
                    Ok(stopped) => format!("🛑 已持久暂停员工：停止 {stopped} 个进行中任务。发送 /resume 才恢复新任务。"),
                    Err(error) => format!("⚠️ 急停状态未能持久化，未执行任务取消：{error}"),
                };
                enqueue_send(live, text, Vec::new(), None, message_id);
                return;
            }
            if command == "/resume" {
                match crate::chief::staff_commands::chief_staff_set_paused(slot.app.clone(), false)
                {
                    Ok(count) => {
                        enqueue_send(
                            live,
                            format!("▶️ 已恢复 {count} 位员工的新任务能力。"),
                            Vec::new(),
                            None,
                            message_id,
                        );
                    }
                    Err(error) => {
                        enqueue_send(
                            live,
                            format!("⚠️ 恢复失败：{error}"),
                            Vec::new(),
                            None,
                            message_id,
                        );
                    }
                }
                return;
            }
        }
        match command.as_str() {
            "/status" | "/sessions" => {
                let (rows, _) = dashboard_snapshot(slot, STATUS_MAX_ROWS, true);
                // 全量清单也把空闲会话列出来（snapshot 默认只留非空闲+绑定）。
                let total = slot
                    .inner
                    .lock()
                    .map(|inner| inner.sessions.len())
                    .unwrap_or(rows.len());
                // 回复走发送线程：poller 里同步 HTTP 会把长轮询整个卡住。
                enqueue_send(
                    live,
                    render_status(&rows, total),
                    Vec::new(),
                    None,
                    message_id,
                );
                schedule_dashboard(slot);
            }
            "/new" => {
                start_new_session_wizard(slot, live, message_id);
            }
            "/model" => {
                // 目标：引用回复的通知所属会话，否则裸文本绑定的会话。
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
                    enqueue_send(
                        live,
                        crate::ts!("还不知道要切哪个会话——回复它的某条通知再发 /model，或先在总控面板点它的名字绑定。", "Not sure which session to switch — reply to one of its notices then /model, or tap its name on the dashboard to bind it first."),
                        Vec::new(),
                        None,
                        message_id,
                    );
                    return;
                };
                send_model_menu(slot, live, &session_id, 0, message_id, None);
            }
            "/resume" | "/sessions-resume" | "/open" => {
                send_resume_menu(slot, live, 0, message_id, None);
            }
            "/help" | "/start" => {
                enqueue_send(live, help_text(), Vec::new(), None, message_id);
            }
            _ => {
                enqueue_send(live, help_text(), Vec::new(), None, message_id);
            }
        }
        return;
    }

    // ── Chief 模式（2026-11-15，docs/research/telegram-chief/）────────────
    // 裸文本固定进 Chief 会话（后端直送，主窗隐藏也活着）；回复 Chief 路由
    // 的消息同样后端直送。旧 relay 的「最近会话」不再抢裸文本；引用 worker
    // 的旧通知仍走下方前端管线（M1 允许，主窗活着才工作）。
    if crate::chief::staff_commands::telegram_chat_routes_to_staff(chat_id) {
        let bound_staff = crate::chief::staff_commands::profile_for_chat(chat_id);
        let chief_sid = bound_staff
            .as_ref()
            .map(|profile| profile.session_id.clone())
            .or_else(|| crate::chief::store::load_config().chief_session_id.clone());
        let reply_route_id = message
            .pointer("/reply_to_message/message_id")
            .and_then(Value::as_i64);
        let reply_target = reply_route_id.and_then(|id| {
            slot.inner
                .lock()
                .ok()
                .and_then(|inner| inner.routes.get(&id).cloned())
        });
        let goes_chief = match reply_target {
            Some(target) => chief_sid.as_deref() == Some(target.as_str()),
            None => true,
        };
        if goes_chief {
            let trimmed = text.trim();
            // 急停逃生口（03-security-permissions.md §13）：固定短命令，
            // 不依赖 Chief 模型自己服从「停止」。
            if trimmed == "急停" || trimmed.eq_ignore_ascii_case("/stop") {
                let text = match crate::chief::broker::chief_stop_all(&slot.app) {
                    Ok(stopped) => format!("🛑 Chief 急停：已停止 {stopped} 个进行中任务。"),
                    Err(error) => format!("⚠️ 急停状态未能持久化，未执行任务取消：{error}"),
                };
                enqueue_send(live, text, Vec::new(), None, message_id);
                return;
            }
            let target_staff_session = chief_sid.clone();
            match crate::chief::broker::chief_inbound_text_for_chat(
                &slot.app,
                chat_id,
                text.as_str(),
            ) {
                Ok(outcome) => {
                    let note = match outcome {
                        crate::chief::broker::InboundOutcome::Sent => "🫡 已交给 Chief。",
                        crate::chief::broker::InboundOutcome::Queued => {
                            "⏳ Chief 正在处理上一条，你的消息已排队。"
                        }
                    };
                    enqueue_send(
                        live,
                        note.to_string(),
                        Vec::new(),
                        target_staff_session,
                        message_id,
                    );
                }
                Err(error) => {
                    enqueue_send(
                        live,
                        format!("⚠️ Chief 处理失败：{error}"),
                        Vec::new(),
                        None,
                        message_id,
                    );
                }
            }
            return;
        }
        // 引用 worker 旧通知 → 落到下方 relay 管线。
    }

    // 引用回复 → 被引消息所属会话；裸文本 → 最近用户会话。
    // 失路（Marionette 重启后 routes 是内存态、或超过 256 条被逐出）时，
    // 引用回复明确报错而不是静默送进当前绑定会话——那正是「送错对话」事故。
    let reply_route = message
        .pointer("/reply_to_message/message_id")
        .and_then(Value::as_i64);
    let target = match reply_route {
        Some(replied) => match slot
            .inner
            .lock()
            .ok()
            .and_then(|inner| inner.routes.get(&replied).cloned())
        {
            Some(session_id) => Some(session_id),
            None => {
                log(
                    "warn",
                    "reply target message not in route map",
                    Some(&format!("replied msg {replied}")),
                );
                enqueue_send(
                    live,
                    crate::ts!("这条被回复的消息太旧（或 Marionette 重启过），认不出它属于哪个会话——改用 /resume 选一个会话再发。", "This replied-to message is too old (or Marionette restarted), so its session can't be identified — use /resume to pick a session instead."),
                    Vec::new(),
                    None,
                    message_id,
                );
                return;
            }
        },
        None => slot
            .inner
            .lock()
            .ok()
            .and_then(|inner| inner.last_user_session.clone()),
    };

    let Some(session_id) = target else {
        enqueue_send(
            live,
            crate::ts!("还没有可以投递的会话——先在电脑上开一个对话框，或回复某条通知来指定。", "No session to deliver to yet — open a conversation on the computer first, or reply to a notice to target one."),
            Vec::new(),
            None,
            message_id,
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

// ─── /new 向导 + 模型菜单 ──────────────────────────────────────────────────

/// 模型菜单每页条数（Telegram 按钮行太多会把聊天顶飞）。
const MODEL_PAGE_SIZE: usize = 8;
/// /new 第 1 步最多列出的项目数（storage 顺序 = 用户在侧栏的顺序）。
const WIZARD_MAX_PROJECTS: usize = 7;
/// /new 第 2 步最多列出的 agent 数。
const WIZARD_MAX_AGENTS: usize = 10;

fn help_text() -> String {
    crate::ts!(
        "命令：
/new — 新建对话（在哪建 → agent → 模型 → 编辑模式，一步步选）
/resume — 继续过往会话（列出全部历史对话，点一个直接接着聊）
/model — 切换模型（回复某条通知 = 切那个会话；裸发 = 切 ⏎ 绑定的会话）
/status — 全部会话的状态清单
/help — 本说明

直接发文字 → 送进 ⏎ 当前绑定的会话（总控面板上点会话名切换绑定）；
回复某条通知 → 定向送达那个会话；点 📄 看会话详情、⏹ 远程停止。",
        "Commands:
/new — new conversation (where → agent → model → edit mode, step by step)
/resume — resume a past session (lists every historical conversation; tap one to keep chatting)
/model — switch model (reply to a notice = that session; plain send = the ⏎-bound session)
/status — status list of all sessions
/help — this help

Plain text → goes to the ⏎ currently bound session (tap a session name on the dashboard to rebind);
Reply to a notice → delivered to that session; tap 📄 for session details, ⏹ to stop remotely."
    )
}

/// storage 的项目清单（侧栏顺序）。绝不能持有 telegram inner 锁时调用。
fn live_projects(slot: &'static Telegram) -> Vec<(String, String)> {
    let Some(state) = slot.app.try_state::<crate::AppState>() else {
        return Vec::new();
    };
    let Ok(storage) = state.storage.lock() else {
        return Vec::new();
    };
    storage
        .list_projects()
        .unwrap_or_default()
        .into_iter()
        .filter(|project| project.id != crate::app_paths::CHAT_PROJECT_ID)
        .map(|project| (project.id, project.name))
        .collect()
}

/// 组键盘：分配按钮 id、拼行、登记回调。返回 (键盘, 待登记动作)。
fn assemble_keyboard(
    slot: &'static Telegram,
    rows: Vec<Vec<(CallbackAction, String)>>,
) -> (Vec<Vec<TgButton>>, Vec<(u64, CallbackAction)>) {
    let mut keyboard = Vec::new();
    let mut pending = Vec::new();
    for row in rows {
        let mut line = Vec::new();
        for (action, label) in row {
            let (button, id, action) = make_button(slot, action, &label);
            line.push(button);
            pending.push((id, action));
        }
        keyboard.push(line);
    }
    (keyboard, pending)
}

fn register_callbacks(slot: &'static Telegram, pending: Vec<(u64, CallbackAction)>) {
    if let Ok(mut inner) = slot.inner.lock() {
        for (id, action) in pending {
            inner.remember_callback(id, action);
        }
    }
}

/// /new 第 1 步：在哪建。
fn start_new_session_wizard(slot: &'static Telegram, live: &LiveConfig, reply_to: Option<i64>) {
    let projects = live_projects(slot);
    let total = projects.len();
    let projects: Vec<_> = projects.into_iter().take(WIZARD_MAX_PROJECTS).collect();

    let mut rows: Vec<Vec<(CallbackAction, String)>> = vec![vec![(
        CallbackAction::NewDestChat,
        crate::ts!("💬 聊天", "💬 Chat"),
    )]];
    for (id, name) in &projects {
        rows.push(vec![(
            CallbackAction::NewDestProject {
                project_id: id.clone(),
                project_name: name.clone(),
            },
            format!("📁 {}", clip(name, 40)),
        )]);
    }
    rows.push(vec![(
        CallbackAction::NewCancel,
        crate::ts!("✖ 取消", "✖ Cancel"),
    )]);
    let (keyboard, pending) = assemble_keyboard(slot, rows);

    // 新 /new 覆盖旧向导（旧按钮按下时按过期处理）。
    if let Ok(mut inner) = slot.inner.lock() {
        inner.wizard = Some(NewSessionWizard {
            dest: None,
            agent_id: String::new(),
            agent_label: String::new(),
            model: None,
        });
        for (id, action) in pending {
            inner.remember_callback(id, action);
        }
    }

    let overflow = total.saturating_sub(projects.len());
    let note = if overflow > 0 {
        crate::ts!("\n（还有 {overflow} 个项目没列出——先在电脑上切过去再 /new，或在聊天区建）", "\n({overflow} more projects not listed — switch to one on the computer before /new, or create in chat)")
    } else {
        String::new()
    };
    let text = crate::ts!(
        "🆕 新建对话 · 1/4 在哪建？{note}",
        "🆕 New conversation · 1/4 Where? {note}"
    );
    let _ = send_message(live, &text, &keyboard, reply_to, None, None);
}

/// /new 第 2 步：agent 清单（只列启用的 ACP agent）。
fn send_wizard_agent_step(slot: &'static Telegram, live: &LiveConfig, reply_to: Option<i64>) {
    let agents: Vec<(String, String)> = crate::custom_agents::all_agents_merged()
        .into_iter()
        .filter(|agent| agent.enabled && agent.transport == "acp")
        .map(|agent| (agent.id, agent.label))
        .take(WIZARD_MAX_AGENTS)
        .collect();
    let mut rows: Vec<Vec<(CallbackAction, String)>> = Vec::new();
    for (id, label) in &agents {
        rows.push(vec![(
            CallbackAction::NewAgent {
                agent_id: id.clone(),
                agent_label: label.clone(),
            },
            clip(label, 48),
        )]);
    }
    rows.push(vec![(
        CallbackAction::NewCancel,
        crate::ts!("✖ 取消", "✖ Cancel"),
    )]);
    let (keyboard, pending) = assemble_keyboard(slot, rows);
    register_callbacks(slot, pending);
    let _ = send_message(
        live,
        &crate::ts!(
            "🆕 新建对话 · 2/4 用哪个 agent？",
            "🆕 New conversation · 2/4 Which agent?"
        ),
        &keyboard,
        reply_to,
        None,
        None,
    );
}

/// 当前向导快照（dest + agent 都已就绪才返回）。
fn wizard_ready(slot: &'static Telegram) -> Option<(WizardDest, String, String)> {
    let wizard = slot
        .inner
        .lock()
        .ok()
        .and_then(|inner| inner.wizard.clone())?;
    let dest = wizard.dest?;
    if wizard.agent_id.is_empty() {
        return None;
    }
    Some((dest, wizard.agent_id, wizard.agent_label))
}

/// /new 第 3 步：模型清单（agent 任一活跃会话的 caps；没有就跳过）。
fn send_wizard_model_step(slot: &'static Telegram, live: &LiveConfig, reply_to: Option<i64>) {
    let Some((_dest, agent_id, agent_label)) = wizard_ready(slot) else {
        let _ = send_message(
            live,
            &crate::ts!(
                "向导已过期——重新发 /new 开始。",
                "The wizard expired — send /new to start over."
            ),
            &[],
            reply_to,
            None,
            None,
        );
        return;
    };
    let caps = slot.acp.agent_capability_snapshot(&agent_id);
    let Some(caps) = caps.filter(|caps| !caps.models.is_empty()) else {
        // 无活跃会话拿不到目录：跳过模型与模式，直接创建（前端继承该 agent 最近的选择）。
        let _ = send_message(
            live,
            &crate::ts!(
                "🆕 {agent_label}：现在没有活跃会话，模型清单拿不到——先沿用默认，建好后可用 /model 切。",
                "🆕 {agent_label}: no active session right now, so the model list is unavailable — the default is used for now; switch with /model once created."
            ),
            &[],
            reply_to,
            None,
            None,
        );
        wizard_finalize(slot, live, reply_to, None);
        return;
    };
    send_wizard_model_page(slot, live, &agent_label, &caps, 0, reply_to, None);
}

/// 向导模型菜单的一页。host_message 有值 = 原地 edit（翻页），否则发新消息。
fn send_wizard_model_page(
    slot: &'static Telegram,
    live: &LiveConfig,
    agent_label: &str,
    caps: &crate::acp::CapabilitySnapshot,
    offset: usize,
    reply_to: Option<i64>,
    host_message: Option<i64>,
) {
    let (text, range) = model_page_parts(
        &crate::ts!(
            "🆕 新建对话 · 3/4 {agent_label} 用哪个模型？",
            "🆕 New conversation · 3/4 Which model for {agent_label}?"
        ),
        &caps.models,
        offset,
    );
    let mut rows: Vec<Vec<(CallbackAction, String)>> = Vec::new();
    let in_range = &caps.models[range.clone()];
    for model in in_range {
        rows.push(vec![(
            CallbackAction::NewModel {
                model_id: model.id.clone(),
                model_label: model.label.clone(),
            },
            format!(
                "{}{}",
                if caps.current_model.as_deref() == Some(model.id.as_str()) {
                    "● "
                } else {
                    ""
                },
                clip(&model.label, 48)
            ),
        )]);
    }
    let (prev, next) = page_nav(
        range.end < caps.models.len(),
        offset > 0,
        CallbackAction::WizardModelPage {
            offset: offset.saturating_sub(MODEL_PAGE_SIZE),
        },
        CallbackAction::WizardModelPage { offset: range.end },
    );
    match (prev, next) {
        (Some(prev), Some(next)) => rows.push(vec![(prev.0, prev.1), (next.0, next.1)]),
        (Some(nav), None) | (None, Some(nav)) => rows.push(vec![nav]),
        (None, None) => {}
    }
    rows.push(vec![
        (
            CallbackAction::NewModel {
                model_id: String::new(),
                model_label: crate::ts!("默认", "Default"),
            },
            crate::ts!("↩ 默认（沿用最近）", "↩ Default (keep recent)"),
        ),
        (CallbackAction::NewCancel, crate::ts!("✖ 取消", "✖ Cancel")),
    ]);
    let (keyboard, pending) = assemble_keyboard(slot, rows);
    register_callbacks(slot, pending);
    deliver_menu(live, &text, &keyboard, reply_to, host_message);
}

/// /new 第 4 步：编辑模式（caps 无模式就直接创建）。
fn send_wizard_mode_step(
    slot: &'static Telegram,
    live: &LiveConfig,
    reply_to: Option<i64>,
    model: Option<(String, String)>,
) {
    let Some((_, agent_id, agent_label)) = wizard_ready(slot) else {
        let _ = send_message(
            live,
            &crate::ts!(
                "向导已过期——重新发 /new 开始。",
                "The wizard expired — send /new to start over."
            ),
            &[],
            reply_to,
            None,
            None,
        );
        return;
    };
    // 把模型选择记进向导。
    if let Ok(mut inner) = slot.inner.lock() {
        if let Some(wizard) = inner.wizard.as_mut() {
            wizard.model = Some(model);
        }
    }
    let caps = slot.acp.agent_capability_snapshot(&agent_id);
    let modes = caps.map(|caps| caps.modes).unwrap_or_default();
    if modes.is_empty() {
        wizard_finalize(slot, live, reply_to, None);
        return;
    }
    let mut rows: Vec<Vec<(CallbackAction, String)>> = Vec::new();
    for mode in &modes {
        rows.push(vec![(
            CallbackAction::NewMode {
                mode_id: mode.id.clone(),
                mode_label: mode.label.clone(),
            },
            clip(&mode.label, 48),
        )]);
    }
    rows.push(vec![
        (
            CallbackAction::NewMode {
                mode_id: String::new(),
                mode_label: crate::ts!("默认", "Default"),
            },
            crate::ts!("↩ 不指定", "↩ Unspecified"),
        ),
        (CallbackAction::NewCancel, crate::ts!("✖ 取消", "✖ Cancel")),
    ]);
    let (keyboard, pending) = assemble_keyboard(slot, rows);
    register_callbacks(slot, pending);
    let _ = send_message(
        live,
        &crate::ts!(
            "🆕 新建对话 · 4/4 {agent_label} 用什么编辑模式？",
            "🆕 New conversation · 4/4 Which edit mode for {agent_label}?"
        ),
        &keyboard,
        reply_to,
        None,
        None,
    );
}

/// 向导收尾：把完整选择交给前端建会话（前端经 telegram_remote_result 回执）。
fn wizard_finalize(
    slot: &'static Telegram,
    live: &LiveConfig,
    reply_to: Option<i64>,
    mode: Option<(String, String)>,
) {
    let wizard = slot
        .inner
        .lock()
        .ok()
        .and_then(|mut inner| inner.wizard.take());
    let Some(wizard) = wizard else {
        let _ = send_message(
            live,
            &crate::ts!(
                "向导已过期——重新发 /new 开始。",
                "The wizard expired — send /new to start over."
            ),
            &[],
            reply_to,
            None,
            None,
        );
        return;
    };
    let Some(dest) = wizard.dest else {
        let _ = send_message(
            live,
            &crate::ts!(
                "向导状态不完整——重新发 /new 开始。",
                "The wizard state is incomplete — send /new to start over."
            ),
            &[],
            reply_to,
            None,
            None,
        );
        return;
    };
    if wizard.agent_id.is_empty() {
        let _ = send_message(
            live,
            &crate::ts!(
                "向导状态不完整——重新发 /new 开始。",
                "The wizard state is incomplete — send /new to start over."
            ),
            &[],
            reply_to,
            None,
            None,
        );
        return;
    }
    let (chat, project_id) = match &dest {
        WizardDest::Chat => (true, None),
        WizardDest::Project { id, .. } => (false, Some(id.clone())),
    };
    let model = wizard.model.flatten();
    let summary = crate::ts!(
        "🛠 正在创建：{} · {} · 模型 {} · 模式 {}",
        "🛠 Creating: {} · {} · model {} · mode {}",
        match &dest {
            WizardDest::Chat => crate::ts!("聊天", "chat"),
            WizardDest::Project { name, .. } => name.clone(),
        },
        wizard.agent_label,
        model
            .as_ref()
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| crate::ts!("默认", "Default")),
        mode.as_ref()
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| crate::ts!("默认", "Default")),
    );
    let _ = send_message(live, &summary, &[], reply_to, None, None);
    let payload_model_id = model.as_ref().map(|(id, _)| id.clone());
    let payload_model_label = model.map(|(_, label)| label);
    let payload_mode_id = mode.as_ref().map(|(id, _)| id.clone());
    let payload_mode_label = mode.map(|(_, label)| label);
    let _ = slot.app.emit_to(
        "main",
        COMMAND_EVENT,
        json!({
            "kind": "newSession",
            "chat": chat,
            "projectId": project_id,
            "agentId": wizard.agent_id,
            "modelId": payload_model_id,
            "modelLabel": payload_model_label,
            "modeId": payload_mode_id,
            "modeLabel": payload_mode_label,
        }),
    );
}

/// 纯分页：标题行 + 本页范围（起始会对齐到页边界）。
fn model_page_parts(
    title: &str,
    models: &[crate::acp::ModelDef],
    offset: usize,
) -> (String, std::ops::Range<usize>) {
    let total = models.len();
    let start = offset.min(total.saturating_sub(1));
    let start = start - (start % MODEL_PAGE_SIZE);
    let end = (start + MODEL_PAGE_SIZE).min(total);
    let page_no = start / MODEL_PAGE_SIZE + 1;
    let pages = total.div_ceil(MODEL_PAGE_SIZE);
    let text = crate::ts!(
        "{title}\n第 {page_no}/{pages} 页 · 共 {total} 个",
        "{title}\nPage {page_no}/{pages} · {total} total"
    );
    (text, start..end)
}

/// 翻页行的两个按钮（到边就没有）。返回 (上一页, 下一页)。
fn page_nav(
    has_next: bool,
    has_prev: bool,
    prev_action: CallbackAction,
    next_action: CallbackAction,
) -> (
    Option<(CallbackAction, String)>,
    Option<(CallbackAction, String)>,
) {
    (
        has_prev.then(|| (prev_action, crate::ts!("◀ 上一页", "◀ Prev"))),
        has_next.then(|| (next_action, crate::ts!("下一页 ▶", "Next ▶"))),
    )
}

/// /model：会话的模型菜单（优先本会话 caps，退到同 agent 任一活跃会话）。
fn send_model_menu(
    slot: &'static Telegram,
    live: &LiveConfig,
    session_id: &str,
    offset: usize,
    reply_to: Option<i64>,
    host_message: Option<i64>,
) {
    let meta = session_meta(session_id);
    let caps = slot.acp.get_capabilities(session_id).or_else(|| {
        let agent_id = session_agent_id(slot, session_id)?;
        slot.acp.agent_capability_snapshot(&agent_id)
    });
    let Some(caps) = caps.filter(|caps| !caps.models.is_empty()) else {
        let _ = send_message(
            live,
            &crate::ts!(
                "[{}] 这个 agent 现在没有活跃会话，拿不到模型清单——先发一条消息把它唤醒，再 /model。",
                "[{}] has no active session right now, so the model list is unavailable — send it a message to wake it up, then /model.",
                meta.label
            ),
            &[],
            reply_to,
            None,
            None,
        );
        return;
    };
    let (text, range) = model_page_parts(
        &crate::ts!(
            "🔀 [{}] 切到哪个模型？",
            "🔀 Which model for [{}]?",
            meta.label
        ),
        &caps.models,
        offset,
    );
    let mut rows: Vec<Vec<(CallbackAction, String)>> = Vec::new();
    let in_range = &caps.models[range.clone()];
    for model in in_range {
        rows.push(vec![(
            CallbackAction::SetModel {
                session_id: session_id.to_string(),
                model_id: model.id.clone(),
                model_label: model.label.clone(),
            },
            format!(
                "{}{}",
                if caps.current_model.as_deref() == Some(model.id.as_str()) {
                    "● "
                } else {
                    ""
                },
                clip(&model.label, 48)
            ),
        )]);
    }
    let (prev, next) = page_nav(
        range.end < caps.models.len(),
        offset > 0,
        CallbackAction::ModelPage {
            session_id: session_id.to_string(),
            offset: offset.saturating_sub(MODEL_PAGE_SIZE),
        },
        CallbackAction::ModelPage {
            session_id: session_id.to_string(),
            offset: range.end,
        },
    );
    if let (Some(prev), Some(next)) = (&prev, &next) {
        rows.push(vec![prev.clone(), next.clone()]);
    } else if let Some(nav) = prev.or(next) {
        rows.push(vec![nav]);
    }
    rows.push(vec![(
        CallbackAction::DismissMenu,
        crate::ts!("✖ 关闭", "✖ Close"),
    )]);
    let (keyboard, pending) = assemble_keyboard(slot, rows);
    register_callbacks(slot, pending);
    deliver_menu(live, &text, &keyboard, reply_to, host_message);
}

/// 会话的 agent id（storage 直查；Telegram 侧不持有 inner 锁时调用）。
fn session_agent_id(slot: &'static Telegram, session_id: &str) -> Option<String> {
    let state = slot.app.try_state::<crate::AppState>()?;
    let storage = state.storage.lock().ok()?;
    storage.find_session(session_id).ok()??.agent_id.into()
}

/// 菜单投递：有 host_message 就原地 edit（翻页），否则新发。
fn deliver_menu(
    live: &LiveConfig,
    text: &str,
    keyboard: &[Vec<TgButton>],
    reply_to: Option<i64>,
    host_message: Option<i64>,
) {
    if let (Some(message_id), Some(chat_id)) = (host_message, live.chat_id) {
        if let Some(token) = live.token.as_deref() {
            let markup = reply_markup(keyboard);
            let ok = tg_call(
                token,
                "editMessageText",
                &json!({
                    "chat_id": chat_id,
                    "message_id": message_id,
                    "text": clip(text, 3800),
                    "reply_markup": markup,
                }),
                Duration::from_secs(SEND_HTTP_TIMEOUT_SECS),
            )
            .is_ok();
            if ok {
                return;
            }
        }
    }
    let _ = send_message(live, text, keyboard, reply_to, None, None);
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
        answer(&crate::ts!("无法解析按钮", "Could not parse this button"));
        return;
    };
    let Ok(id) = data.parse::<u64>() else {
        answer(&crate::ts!("无法解析按钮", "Could not parse this button"));
        return;
    };
    let Some(action) = slot
        .inner
        .lock()
        .ok()
        .and_then(|mut inner| inner.callbacks.remove(&id))
    else {
        answer(&crate::ts!(
            "这条已处理或已过期",
            "Already handled or expired"
        ));
        return;
    };

    let result = match &action {
        CallbackAction::Permission {
            session_id,
            request_id,
            option_id,
            option_label,
        } => slot.acp.respond_permission(request_id, option_id).map(|_| {
            emit_resolved(slot, session_id, "permission/resolved", request_id);
            crate::ts!("已选：{option_label}", "Chosen: {option_label}")
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
                    crate::ts!("已选：{selected}", "Chosen: {selected}")
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
                crate::ts!("已跳过", "Skipped")
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
            Ok(crate::ts!(
                "⏎ 裸发文字将送进 [{label}]",
                "⏎ Plain text will go to [{label}]"
            ))
        }
        CallbackAction::StopTurn { session_id, label } => slot
            .acp
            .cancel(session_id)
            .map(|()| crate::ts!("⏹ 已请求停止 [{label}]", "⏹ Stop requested for [{label}]")),
        CallbackAction::ViewSession { session_id, label } => {
            if let Ok(mut inner) = slot.inner.lock() {
                inner.view = PanelView::Session(session_id.clone());
            }
            schedule_dashboard(slot);
            Ok(crate::ts!("📄 [{label}] 详情", "📄 [{label}] details"))
        }
        CallbackAction::PanelBack => {
            if let Ok(mut inner) = slot.inner.lock() {
                inner.view = PanelView::Dashboard;
            }
            schedule_dashboard(slot);
            Ok(crate::ts!("已返回总控", "Back to the dashboard"))
        }
        CallbackAction::RefreshDashboard => {
            schedule_dashboard(slot);
            Ok(crate::ts!("已刷新", "Refreshed"))
        }
        // ─── /new 向导 ───────────────────────────────────────────────
        CallbackAction::NewDestChat => {
            let fresh = slot
                .inner
                .lock()
                .map(|mut inner| match inner.wizard.as_mut() {
                    Some(wizard) if wizard.dest.is_none() => {
                        wizard.dest = Some(WizardDest::Chat);
                        true
                    }
                    _ => false,
                });
            match fresh {
                Ok(true) => {
                    send_wizard_agent_step(slot, live, None);
                    Ok(crate::ts!("目的地：聊天", "Destination: chat"))
                }
                _ => Err(crate::ts!(
                    "已过期——重新发 /new 开始",
                    "Expired — send /new to start over"
                )
                .into()),
            }
        }
        CallbackAction::NewDestProject {
            project_id,
            project_name,
        } => {
            let fresh = slot
                .inner
                .lock()
                .map(|mut inner| match inner.wizard.as_mut() {
                    Some(wizard) if wizard.dest.is_none() => {
                        wizard.dest = Some(WizardDest::Project {
                            id: project_id.clone(),
                            name: project_name.clone(),
                        });
                        true
                    }
                    _ => false,
                });
            match fresh {
                Ok(true) => {
                    send_wizard_agent_step(slot, live, None);
                    Ok(crate::ts!(
                        "目的地：{project_name}",
                        "Destination: {project_name}"
                    ))
                }
                _ => Err(crate::ts!(
                    "已过期——重新发 /new 开始",
                    "Expired — send /new to start over"
                )
                .into()),
            }
        }
        CallbackAction::NewAgent {
            agent_id,
            agent_label,
        } => {
            let fresh = slot
                .inner
                .lock()
                .map(|mut inner| match inner.wizard.as_mut() {
                    Some(wizard) if wizard.dest.is_some() && wizard.agent_id.is_empty() => {
                        wizard.agent_id = agent_id.clone();
                        wizard.agent_label = agent_label.clone();
                        wizard.model = None;
                        true
                    }
                    _ => false,
                });
            match fresh {
                Ok(true) => {
                    send_wizard_model_step(slot, live, None);
                    Ok(crate::ts!("agent：{agent_label}", "agent: {agent_label}"))
                }
                _ => Err(crate::ts!(
                    "已过期——重新发 /new 开始",
                    "Expired — send /new to start over"
                )
                .into()),
            }
        }
        CallbackAction::NewModel {
            model_id,
            model_label,
        } => {
            let fresh = slot.inner.lock().map(|inner| {
                matches!(
                    inner.wizard.as_ref(),
                    Some(wizard) if !wizard.agent_id.is_empty()
                )
            });
            match fresh {
                Ok(true) => {
                    let pick = if model_id.is_empty() {
                        None
                    } else {
                        Some((model_id.clone(), model_label.clone()))
                    };
                    send_wizard_mode_step(slot, live, None, pick);
                    if model_id.is_empty() {
                        Ok(crate::ts!("模型：默认", "Model: default"))
                    } else {
                        Ok(crate::ts!("模型：{model_label}", "Model: {model_label}"))
                    }
                }
                _ => Err(crate::ts!(
                    "已过期——重新发 /new 开始",
                    "Expired — send /new to start over"
                )
                .into()),
            }
        }
        CallbackAction::NewMode {
            mode_id,
            mode_label,
        } => {
            let fresh = slot.inner.lock().map(|inner| inner.wizard.is_some());
            match fresh {
                Ok(true) => {
                    let pick = if mode_id.is_empty() {
                        None
                    } else {
                        Some((mode_id.clone(), mode_label.clone()))
                    };
                    wizard_finalize(slot, live, None, pick);
                    if mode_id.is_empty() {
                        Ok(crate::ts!(
                            "模式：默认 · 正在创建…",
                            "Mode: default · creating…"
                        ))
                    } else {
                        Ok(crate::ts!(
                            "模式：{mode_label} · 正在创建…",
                            "Mode: {mode_label} · creating…"
                        ))
                    }
                }
                _ => Err(crate::ts!(
                    "已过期——重新发 /new 开始",
                    "Expired — send /new to start over"
                )
                .into()),
            }
        }
        CallbackAction::NewCancel => {
            if let Ok(mut inner) = slot.inner.lock() {
                inner.wizard = None;
            }
            Ok(crate::ts!("已取消", "Cancelled"))
        }
        CallbackAction::WizardModelPage { offset } => {
            // 翻页 = 原地重画按下的那条菜单消息。
            match wizard_ready(slot) {
                Some((_, agent_id, agent_label)) => {
                    let caps = slot
                        .acp
                        .agent_capability_snapshot(&agent_id)
                        .filter(|caps| !caps.models.is_empty());
                    match caps {
                        Some(caps) => {
                            let host = callback
                                .pointer("/message/message_id")
                                .and_then(Value::as_i64);
                            send_wizard_model_page(
                                slot,
                                live,
                                &agent_label,
                                &caps,
                                *offset,
                                None,
                                host,
                            );
                            Ok(crate::ts!("第 {} 项起", "From item {}", offset + 1))
                        }
                        None => Err(crate::ts!(
                            "模型清单已不可用——重新发 /new 开始",
                            "Model list is no longer available — send /new to start over"
                        )
                        .into()),
                    }
                }
                None => Err(crate::ts!(
                    "已过期——重新发 /new 开始",
                    "Expired — send /new to start over"
                )
                .into()),
            }
        }
        CallbackAction::ModelPage { session_id, offset } => {
            let host = callback
                .pointer("/message/message_id")
                .and_then(Value::as_i64);
            send_model_menu(slot, live, &session_id, *offset, None, host);
            Ok(crate::ts!("第 {} 项起", "From item {}", offset + 1))
        }
        CallbackAction::ResumeSession { session_id, label } => {
            if let Ok(mut inner) = slot.inner.lock() {
                inner.last_user_session = Some(session_id.clone());
            }
            schedule_dashboard(slot);
            Ok(crate::ts!(
                "⏎ 已接上 [{label}]，直接发文字继续聊",
                "⏎ Attached to [{label}] — send text to keep chatting"
            ))
        }
        CallbackAction::ResumePage { offset } => {
            let host = callback
                .pointer("/message/message_id")
                .and_then(Value::as_i64);
            send_resume_menu(slot, live, *offset, None, host);
            Ok(crate::ts!("第 {} 项起", "From item {}", offset + 1))
        }
        CallbackAction::SetModel {
            session_id,
            model_id,
            model_label,
        } => {
            let _ = slot.app.emit_to(
                "main",
                COMMAND_EVENT,
                json!({
                    "kind": "setModel",
                    "sessionId": session_id,
                    "modelId": model_id,
                    "modelLabel": model_label,
                }),
            );
            Ok(crate::ts!(
                "已请求切换 → {model_label}",
                "Switch requested → {model_label}"
            ))
        }
        CallbackAction::DismissMenu => Ok(crate::ts!("已关闭", "Closed")),
    };

    match result {
        Ok(label) => {
            answer(&label);
            // 摘掉按钮，防止重复点击。两类例外：面板消息的键盘由下一次
            // 面板编辑整体重画；翻页菜单刚被 edit 成下一页，就地摘除会把
            // 新一页的按钮抹掉。
            let is_page_action = matches!(
                action,
                CallbackAction::WizardModelPage { .. }
                    | CallbackAction::ModelPage { .. }
                    | CallbackAction::ResumePage { .. }
            );
            let dashboard_message = slot
                .inner
                .lock()
                .ok()
                .and_then(|inner| inner.dashboard.map(|handle| handle.message_id));
            let pressed_message = callback
                .pointer("/message/message_id")
                .and_then(Value::as_i64);
            if !is_page_action {
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
            }
            log(
                "info",
                "remote resolve ok",
                Some(&format!("{} · {label}", action.session_id())),
            );
        }
        Err(error) => {
            answer(&crate::ts!(
                "没成：{}",
                "Didn't work: {}",
                clip(&error, 160)
            ));
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

/// ureq 2.12 的 `Error` Display 会把完整请求 URL 拼进错误文本（Status 打
/// `response.get_url()`、Transport 打 `transport.url`，重定向历史还会再带
/// 一个），URL 形如 `…/bot<TOKEN>/method`。任何可能来自 ureq 的错误串进
/// 日志/`last_error` 前必须过这里把 token 段换成 `<redacted>`（token 字符集
/// 不含 `/`，下一个 `/` 即方法名边界）。
fn sanitize_error(method: &str, error: &str) -> String {
    let mut out = error.to_string();
    let mut scanned = 0usize;
    while let Some(rel) = out[scanned..].find("/bot") {
        let at = scanned + rel + 4;
        let end = out[at..].find('/').map(|off| at + off).unwrap_or(out.len());
        out.replace_range(scanned + rel..end, "/bot<redacted>");
        scanned = scanned + rel + "/bot<redacted>".len();
    }
    format!("{method}: {out}")
}

/// POST JSON；非 ok 一律带上下文报错。token 绝不进错误文本（url 不外泄）。
fn tg_call(token: &str, method: &str, body: &Value, timeout: Duration) -> Result<Value, String> {
    if token.is_empty() {
        return Err("no token".to_string());
    }
    let url = format!("{}/bot{token}/{method}", api_base());
    let agent = crate::http_client::agent_with_proxy()?;
    let response = agent.post(&url).timeout(timeout).send_json(body);
    read_tg_response(method, response)
}

/// 统一解读 Bot API 应答：HTTP 层错误体里的 `description`、JSON 体里的
/// `ok` 都在这一处变成 `HTTP <code> · <method>: <desc>` 形态的错误串。
/// JSON 与 multipart 两条路共用——形状必须一致，`rejected` /
/// `tg_error_status` 靠 `HTTP ` 前缀识别。
fn read_tg_response(
    method: &str,
    response: Result<ureq::Response, ureq::Error>,
) -> Result<Value, String> {
    let response = response.map_err(|error| match error {
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
        other => sanitize_error(method, &other.to_string()),
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

/// multipart/form-data 里要上传的文件段（`field` 随发法变：photo /
/// animation / document）。
struct UploadFile<'a> {
    field: &'a str,
    filename: String,
    mime: String,
    data: &'a [u8],
}

/// multipart 上传（`sendPhoto` / `sendAnimation` / `sendDocument` 的文件体）。
/// ureq 的 json 构建器发不了二进制，这里手拼 RFC 7578 的体——零新依赖。
fn tg_call_multipart(
    token: &str,
    method: &str,
    fields: &[(&str, String)],
    file: &UploadFile<'_>,
    timeout: Duration,
) -> Result<Value, String> {
    let agent = crate::http_client::agent_with_proxy()?;
    tg_call_multipart_via(&agent, &api_base(), token, method, fields, file, timeout)
}

/// 同上，但 agent 与 API base 由调用方给：单测用直连 agent 打回环假服务器，
/// 不读用户的代理设置（`agent_with_proxy` 每次都读盘）。
fn tg_call_multipart_via(
    agent: &ureq::Agent,
    base: &str,
    token: &str,
    method: &str,
    fields: &[(&str, String)],
    file: &UploadFile<'_>,
    timeout: Duration,
) -> Result<Value, String> {
    if token.is_empty() {
        return Err("no token".to_string());
    }
    let boundary = multipart_boundary();
    let body = multipart_body(&boundary, fields, file);
    let url = format!("{base}/bot{token}/{method}");
    let response = agent
        .post(&url)
        .set(
            "Content-Type",
            &format!("multipart/form-data; boundary={boundary}"),
        )
        .timeout(timeout)
        .send_bytes(&body);
    read_tg_response(method, response)
}

/// 边界串：进程号 + 纳秒时间 + 调用序号。图片数据里撞上这一串的概率
/// 当作零（Telegram 只按边界切，不校验唯一性）。
fn multipart_boundary() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    format!(
        "marionette-{:x}-{:x}{:08x}",
        std::process::id(),
        (nanos >> 32) as u64,
        SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

/// 拼一段 multipart/form-data（RFC 7578）：普通字段在前、文件段在后、
/// 结尾边界收口。每段之间靠 `\r\n--边界\r\n` 分隔。
fn multipart_body(boundary: &str, fields: &[(&str, String)], file: &UploadFile<'_>) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\nContent-Type: {}\r\n\r\n",
            file.field, file.filename, file.mime
        )
        .as_bytes(),
    );
    body.extend_from_slice(file.data);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    body
}

/// `getMe`（保存设置时校验 token 用）。返回 bot 的 @用户名。
fn verify_token(token: &str) -> Result<String, String> {
    let value = tg_call(token, "getMe", &json!({}), Duration::from_secs(10))?;
    value
        .pointer("/result/username")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            crate::ts!(
                "getMe 响应里没有 username",
                "getMe response has no username"
            )
        })
}

// ─── 纯函数（组消息 / 路由决策 / 解析）─────────────────────────────────────

fn stop_reason_head(reason: &str) -> (&'static str, String) {
    match reason {
        "max_tokens" => ("✂️", crate::ts!("到达长度上限", "Length limit reached")),
        "refusal" => ("⚠️", crate::ts!("模型拒答", "Model refused")),
        "max_turn_requests" => ("⏹", crate::ts!("达到回合上限", "Turn limit reached")),
        _ => ("✅", crate::ts!("跑完", "Done")),
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
                .then(|| {
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
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
                    block
                        .get("text")
                        .and_then(Value::as_str)
                        .map(str::to_string)
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
            body: crate::ts!(
                "（问题内容没能解析）",
                "(could not parse the question content)"
            ),
        };
    };
    let mut any_multi = false;
    let mut parsed: Vec<(String, Vec<String>)> = Vec::new();
    for question in questions {
        let text = question
            .get("question")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| crate::ts!("请选择", "Choose"));
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
            body: crate::ts!("（没有问题）", "(no question)"),
        };
    };
    if parsed.len() == 1 && !any_multi {
        if !options.is_empty() {
            return QuestionPush::Buttons { question, options };
        }
        // 单问题但没有选项：开放式问题，提示用文字回复。
        return QuestionPush::TextOnly {
            body: crate::ts!(
                "{question}\n（开放式问题：直接回复文字即可送进会话）",
                "{question}\n(open question: reply with text to send it into the session)"
            ),
        };
    }
    let body = parsed
        .iter()
        .map(|(q, options)| {
            if options.is_empty() {
                q.clone()
            } else {
                crate::ts!("{q}\n  选项：{}", "{q}\n  Options: {}", options.join(" / "))
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

// ─── 图片（回合随附）───────────────────────────────────────────────────────

/// 路径比较键（对齐前端 `samePath` / `imagePathKey`）：JSON 里转义过的双
/// 反斜杠归一成单反斜杠，分隔符统一成 `/`，去首尾空白与尾部分隔符，小写。
fn path_key(path: &str) -> String {
    path.replace("\\\\", "\\")
        .replace('\\', "/")
        .trim()
        .trim_end_matches('/')
        .to_lowercase()
}

/// 路径（或 URL）的扩展名，小写；查询串 / 片段不算。
fn image_extension(path: &str) -> Option<String> {
    let bare = path.split(['?', '#']).next().unwrap_or("");
    let base = bare.rsplit(['/', '\\']).next().unwrap_or("");
    let dot = base.rfind('.')?;
    let ext = base[dot + 1..].to_ascii_lowercase();
    (!ext.is_empty() && ext.len() <= 8).then_some(ext)
}

/// 前端 `isImagePath` 同一张表：UI 会把哪些扩展名当图显示。
fn is_image_path(path: &str) -> bool {
    matches!(
        image_extension(path).as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg")
    )
}

/// MIME 的子型（`image/svg+xml` → `svg+xml`），小写。
fn mime_subtype(mime: &str) -> String {
    let full = mime.split(';').next().unwrap_or("").trim();
    match full.rsplit_once('/') {
        Some((_, subtype)) => subtype,
        None => full,
    }
    .to_ascii_lowercase()
}

/// 按扩展名 / MIME 子型挑首选发法。没认出来的先按照片试——反正被拒会
/// 退成文件；只有明确「不是照片」的类型才直接走文件。
fn send_method_for(kind: Option<&str>) -> (&'static str, &'static str) {
    match kind {
        Some("gif") => SEND_ANIMATION,
        // Telegram 照片只收 jpg/png/webp：其余图片类型按文件发更稳。
        Some("bmp" | "svg" | "svg+xml" | "tiff" | "tif" | "avif" | "heic" | "ico" | "bin") => {
            SEND_DOCUMENT
        }
        Some(_) | None => SEND_PHOTO,
    }
}

/// 盘符路径前多出来的一个斜杠（`/D:/x` → `D:/x`）。
fn normalize_windows_drive_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let slashed_drive = bytes.len() >= 3
        && bytes[0] == b'/'
        && bytes[1].is_ascii_alphabetic()
        && (bytes[2] == b':' || bytes[2] == b'\\');
    if slashed_drive {
        path[1..].to_string()
    } else {
        path.to_string()
    }
}

/// 百分号解码（`a%20b.png` → `a b.png`）。解不出合法 UTF-8 的字节用替换
/// 字符顶替，不报错——路径只是用来打开发送，不值得为它失败。
fn percent_decode(text: &str) -> String {
    if !text.contains('%') {
        return text.to_string();
    }
    let bytes = text.as_bytes();
    let hex = |byte: u8| (byte as char).to_digit(16);
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let mut decoded = None;
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                decoded = Some((high * 16 + low) as u8);
            }
        }
        match decoded {
            Some(byte) => {
                out.push(byte);
                index += 3;
            }
            None => {
                out.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn is_drive_authority(authority: &str) -> bool {
    let bytes = authority.as_bytes();
    bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

/// `file:///D:/x.png` / `file://localhost/C:/x` / `file://server/share/x`
/// → 文件系统路径。对齐前端 `imageAttachments.ts::stripFileUri`：百分号
/// 解码、盘符前的多余斜杠去掉、UNC 主机保留成 `//host/…`。
fn strip_file_uri(raw: &str) -> String {
    let trimmed = raw.trim();
    let is_file_uri = trimmed
        .get(..5)
        .is_some_and(|head| head.eq_ignore_ascii_case("file:"));
    if !is_file_uri {
        return normalize_windows_drive_path(trimmed);
    }
    let rest = &trimmed[5..];
    let mut path = match rest.strip_prefix("//") {
        Some(after_slashes) => match after_slashes.find('/') {
            // `//host/rest`：主机是空的、localhost 或盘符 → 只是路径；其余是 UNC。
            Some(at) => {
                let (authority, tail) = (&after_slashes[..at], &after_slashes[at..]);
                if is_drive_authority(authority) {
                    format!("{authority}{tail}")
                } else if authority.is_empty() || authority.eq_ignore_ascii_case("localhost") {
                    tail.to_string()
                } else {
                    format!("//{authority}{tail}")
                }
            }
            None => String::new(),
        },
        None => rest.to_string(),
    };
    path = percent_decode(&path);
    // `file:///C|/x`：竖线写盘符的旧形态（`C|` → `C:`，后面的 `/` 保留）。
    if let Some(drive) = path.strip_prefix('/') {
        let bytes = drive.as_bytes();
        if bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b'|' {
            path = format!("{}:{}", &drive[..1], &drive[2..]);
        }
    }
    normalize_windows_drive_path(&path)
}

/// 相对路径按会话 cwd 补全（对齐前端 `resolveToolImagePath`）；绝对路径
/// （盘符 / UNC / 前导 `/`）与拿不到 cwd 的原样返回。
fn resolve_tool_image_path(path: &str, cwd: Option<&str>) -> String {
    let value = normalize_windows_drive_path(path.trim());
    let bytes = value.as_bytes();
    let on_drive = bytes.len() >= 2
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && bytes
            .get(2)
            .is_some_and(|next| *next == b'/' || *next == b'\\');
    if value.is_empty() || on_drive || value.starts_with("\\\\") || value.starts_with('/') {
        return value;
    }
    let Some(base) = cwd.map(str::trim).filter(|base| !base.is_empty()) else {
        return value;
    };
    let separator = if base.contains('\\') { '\\' } else { '/' };
    // 只吃掉开头的 `./` / `.\`；`../` 要留着，交给文件系统解析。
    let rest = value
        .strip_prefix("./")
        .or_else(|| value.strip_prefix(".\\"))
        .unwrap_or(&value);
    let rest = rest.trim_start_matches(['/', '\\']);
    let rest = if separator == '\\' {
        rest.replace('/', "\\")
    } else {
        rest.replace('\\', "/")
    };
    format!("{}{separator}{rest}", base.trim_end_matches(['\\', '/']))
}

/// 一个图片引用 → 可发送的图。`require_image_ext`：本地路径必须带图片
/// 扩展名——image 块自述是图可以免检，`locations` / 正文里的 markdown
/// 引用只是一个路径，得按扩展名认。
fn turn_image_from_src(raw: &str, require_image_ext: bool) -> Option<TurnImage> {
    let trimmed = raw.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return Some(TurnImage::Remote(trimmed.to_string()));
    }
    if lower.starts_with("data:") {
        return image_from_data_url(&trimmed[5..]);
    }
    let path = strip_file_uri(trimmed);
    if path.is_empty() || (require_image_ext && !is_image_path(&path)) {
        return None;
    }
    Some(TurnImage::Local(PathBuf::from(path)))
}

/// `data:image/png;base64,…` → 内联字节。只认 base64——百分号编码的
/// 那种形态没见过，不猜。
fn image_from_data_url(url: &str) -> Option<TurnImage> {
    let (header, payload) = url.split_once(',')?;
    let is_base64 = header
        .split(';')
        .any(|part| part.trim().eq_ignore_ascii_case("base64"));
    if !is_base64 {
        return None;
    }
    let mime = header.split(';').next().unwrap_or("").trim();
    let mime = if mime.is_empty() { "image/png" } else { mime };
    image_from_base64(payload.trim(), mime)
}

/// base64 → 内联图，带体量闸：超限的图连 base64 都不留（十几 MB 的
/// base64 很占内存，而它注定发不出去）。
fn image_from_base64(data: &str, mime: &str) -> Option<TurnImage> {
    let oversized = data.len().saturating_mul(3) / 4 > IMAGE_MAX_BYTES as usize;
    (!data.is_empty() && !oversized).then(|| TurnImage::Bytes {
        data: data.to_string(),
        mime: mime.to_string(),
    })
}

/// 一个 `session/update` 里的图，与前端工具卡预览同口径
/// （`acpTranscript.ts` 的 `extractToolImageSrcs` / `collectToolImageSrcs`）：
///
/// - `content` 里嵌套的 image 块：`uri`→`url`→`path`→`filePath`→`file_path`
///   取首个非空；都没有就取 `data`（base64）——截图 / Read 图片最常见的形态；
/// - content 里没有图时，才收工具的 `locations[].path`（或单数 `location`）
///   与 `rawInput` 的路径字段——只是路径，必须带图片扩展名才算。
fn extract_update_images(update: &Value) -> Vec<TurnImage> {
    let mut images: Vec<TurnImage> = Vec::new();
    for field in ["content", "delta"] {
        if let Some(node) = update.get(field) {
            collect_image_blocks(node, &mut images);
        }
    }
    // 工具路径只是兜底：Read 一张图会同时给出 base64 块和 file_path，
    // 那是同一张，content 里已经有图就别重复收。
    if images.is_empty() {
        if let Some(path) = tool_image_path(update) {
            if let Some(image) = turn_image_from_src(&path, true) {
                images.push(image);
            }
        }
    }
    images
}

/// 递归找 image 块：数组逐项看；对象要么自己是 image 块，要么往它的
/// `content` 里找（tool_result 包着内容数组）。
fn collect_image_blocks(node: &Value, images: &mut Vec<TurnImage>) {
    match node {
        Value::Array(items) => {
            for item in items {
                collect_image_blocks(item, images);
            }
        }
        Value::Object(block) => {
            let is_image = block
                .get("type")
                .and_then(Value::as_str)
                .is_some_and(|kind| kind.eq_ignore_ascii_case("image"));
            if is_image {
                if let Some(image) = image_block_to_turn_image(block) {
                    let key = image.key();
                    if !images.iter().any(|existing| existing.key() == key) {
                        images.push(image);
                    }
                }
            } else if let Some(content) = block.get("content") {
                collect_image_blocks(content, images);
            }
        }
        _ => {}
    }
}

fn image_block_to_turn_image(block: &serde_json::Map<String, Value>) -> Option<TurnImage> {
    for key in ["uri", "url", "path", "filePath", "file_path"] {
        let Some(raw) = block
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|raw| !raw.is_empty())
        else {
            continue;
        };
        if let Some(image) = turn_image_from_src(raw, false) {
            return Some(image);
        }
    }
    let data = block
        .get("data")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("");
    let mime = block
        .get("mimeType")
        .or_else(|| block.get("mime_type"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|mime| !mime.is_empty())
        .unwrap_or("image/png");
    image_from_base64(data, mime)
}

/// 工具在读写哪个文件：`locations[]`（或单数 `location`）优先，没有就
/// 从 `rawInput` 里认路径（agent 没给 locations 时的兜底）。
fn tool_image_path(update: &Value) -> Option<String> {
    let mut first = None;
    match update.get("locations").or_else(|| update.get("location")) {
        Some(Value::Array(items)) => {
            for item in items {
                if let Some(path) = location_path(item) {
                    first = Some(path);
                    break;
                }
            }
        }
        Some(single) => first = location_path(single),
        None => {}
    }
    first.or_else(|| {
        update
            .get("rawInput")
            .or_else(|| update.get("raw_input"))
            .and_then(path_from_raw_input)
    })
}

fn location_path(entry: &Value) -> Option<String> {
    let path = match entry {
        Value::String(text) => text.trim(),
        Value::Object(record) => record.get("path").and_then(Value::as_str)?.trim(),
        _ => return None,
    };
    (!path.is_empty()).then(|| path.to_string())
}

/// `rawInput` 里的路径：JSON 字符串认 `path` / `filePath` / `file_path` /
/// `uri` 键；裸字符串得长得像路径才算——绝不把一条 shell 命令当成路径。
fn path_from_raw_input(raw: &Value) -> Option<String> {
    match raw {
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.starts_with('{') || trimmed.starts_with('[') {
                let parsed = serde_json::from_str::<Value>(trimmed).ok()?;
                return path_from_raw_input(&parsed);
            }
            let looks_like_path = trimmed.contains('/')
                || trimmed.contains('\\')
                || (!trimmed.contains(char::is_whitespace) && has_file_extension(trimmed));
            looks_like_path.then(|| trimmed.to_string())
        }
        Value::Object(record) => {
            for key in ["path", "filePath", "file_path", "uri"] {
                if let Some(value) = record
                    .get(key)
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                {
                    return Some(value.to_string());
                }
            }
            None
        }
        _ => None,
    }
}

fn has_file_extension(text: &str) -> bool {
    let Some(dot) = text.rfind('.') else {
        return false;
    };
    let ext = &text[dot + 1..];
    !ext.is_empty() && ext.len() <= 8 && ext.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
}

/// 回复正文里 markdown 图片的地址（`![alt](src)`），按出现顺序、不查重。
/// 跳过围栏代码块与行内代码——教用户「图片语法这样写」的回复，不该真的
/// 去发那张图。
fn markdown_image_srcs(text: &str) -> Vec<String> {
    let mut srcs = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for line in text.lines() {
        match (fence_marker(line), fence) {
            // 开栏。
            (Some((marker, run)), None) => fence = Some((marker, run)),
            // 闭合：同一种字符、不短于开栏、后面不带 info string。
            (Some((marker, run)), Some((open_char, open_len)))
                if marker == open_char
                    && run >= open_len
                    && line.trim_start()[run..].trim().is_empty() =>
            {
                fence = None;
            }
            // 栏内的一行（或长得像栏但不是的那行）：不看。
            (Some(_), Some(_)) => {}
            (None, None) => scan_inline_images(&mask_inline_code(line), &mut srcs),
            (None, Some(_)) => {}
        }
    }
    srcs
}

/// 行首的围栏（``` 或 ~~~，至少三个）：(字符, 个数)。
fn fence_marker(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start();
    let first = trimmed.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let run = trimmed.chars().take_while(|ch| *ch == first).count();
    (run >= 3).then_some((first, run))
}

/// 把行内代码段（`…`）涂成空格——长度不变，只为了不扫到里面的 `![…]()`。
fn mask_inline_code(line: &str) -> String {
    if !line.contains('`') {
        return line.to_string();
    }
    let chars: Vec<char> = line.chars().collect();
    let mut masked = chars.clone();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] != '`' {
            index += 1;
            continue;
        }
        let open_start = index;
        while index < chars.len() && chars[index] == '`' {
            index += 1;
        }
        let ticks = index - open_start;
        // 找下一个等长的反引号串；开串、内容、闭串一起涂掉。
        let mut scan = index;
        let mut close_end = None;
        while scan < chars.len() {
            if chars[scan] == '`' {
                let run_start = scan;
                while scan < chars.len() && chars[scan] == '`' {
                    scan += 1;
                }
                if scan - run_start == ticks {
                    close_end = Some(scan);
                    break;
                }
            } else {
                scan += 1;
            }
        }
        match close_end {
            Some(close_end) => {
                masked[open_start..close_end].fill(' ');
                index = close_end;
            }
            None => (),
        }
    }
    masked.into_iter().collect()
}

/// 一行里的 `![alt](src)`：`alt` 里的方括号可以嵌套，`src` 可以用 `<…>`
/// 包起来（带空格的路径）或后面跟一个 title。
fn scan_inline_images(line: &str, srcs: &mut Vec<String>) {
    let chars: Vec<char> = line.chars().collect();
    let mut index = 0;
    while index + 1 < chars.len() {
        if chars[index] != '!' || chars[index + 1] != '[' {
            index += 1;
            continue;
        }
        let Some(alt_end) = closing_bracket(&chars, index + 2) else {
            return;
        };
        if chars.get(alt_end + 1) == Some(&'(') {
            if let Some((src, end)) = link_destination(&chars, alt_end + 2) {
                srcs.push(src);
                index = end;
                continue;
            }
        }
        index = alt_end + 1;
    }
}

/// `from` 起第一个没被转义、且不闭合内层 `[` 的 `]`。
fn closing_bracket(chars: &[char], from: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut index = from;
    while index < chars.len() {
        match chars[index] {
            '\\' => index += 1,
            '[' => depth += 1,
            ']' => {
                if depth == 0 {
                    return Some(index);
                }
                depth -= 1;
            }
            _ => {}
        }
        index += 1;
    }
    None
}

/// `(` 之后的链接目的地：返回 (地址, 收尾 `)` 之后的位置)。
fn link_destination(chars: &[char], from: usize) -> Option<(String, usize)> {
    let mut index = from;
    while index < chars.len() && chars[index].is_whitespace() {
        index += 1;
    }
    let src: String = if chars.get(index) == Some(&'<') {
        index += 1;
        let start = index;
        while index < chars.len() && chars[index] != '>' {
            if chars[index] == '\\' {
                index += 1;
            }
            index += 1;
        }
        if chars.get(index) != Some(&'>') {
            return None;
        }
        let src: String = chars[start..index.min(chars.len())]
            .iter()
            .filter(|ch| **ch != '\\')
            .collect();
        index += 1;
        src
    } else {
        let start = index;
        let mut depth = 0usize;
        while index < chars.len() {
            let ch = chars[index];
            if ch == '\\' {
                index += 2;
                continue;
            }
            if ch.is_whitespace() {
                break;
            }
            if ch == '(' {
                depth += 1;
            }
            if ch == ')' {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            index += 1;
        }
        chars[start..index.min(chars.len())].iter().collect()
    };
    if src.is_empty() {
        return None;
    }
    // 之后是可选的 title 与收尾的 `)`：找到行内下一个不配对的 `)`。
    let mut depth = 0usize;
    while index < chars.len() {
        match chars[index] {
            '(' => depth += 1,
            ')' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
        index += 1;
    }
    Some((src, (index + 1).min(chars.len())))
}

/// MarkdownV2 实体外保留字（含转义符自身）全部加反斜杠。
fn md_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for ch in text.chars() {
        if matches!(
            ch,
            '_' | '*'
                | '['
                | ']'
                | '('
                | ')'
                | '~'
                | '`'
                | '>'
                | '#'
                | '+'
                | '-'
                | '='
                | '|'
                | '{'
                | '}'
                | '.'
                | '!'
                | '\\'
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
            if let Some(end) = (i + 2..chars.len().saturating_sub(1))
                .find(|&p| chars[p] == '*' && chars.get(p + 1) == Some(&'*'))
            {
                if end > i + 2 {
                    out.push('*');
                    out.push_str(&md_escape(&chars[i + 2..end].iter().collect::<String>()));
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
        crate::ts!("{secs}秒", "{secs}s")
    } else if secs < 3600 {
        crate::ts!("{}分{:02}秒", "{}m{:02}s", secs / 60, secs % 60)
    } else {
        crate::ts!("{}时{}分", "{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// 会话显示名与 agent 名：storage 一次查询；缺省退回短 id。
/// 只在未持有 telegram inner 锁时调用（锁序红线：inner → storage 禁止）。
struct SessionMeta {
    label: String,
    agent: String,
    /// 会话工作目录（相对路径的随附图按它补全）；空串当没有。
    cwd: Option<String>,
}

fn session_meta(session_id: &str) -> SessionMeta {
    let fallback = SessionMeta {
        label: clip(session_id, 24),
        agent: String::new(),
        cwd: None,
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
    SessionMeta {
        label,
        agent,
        cwd: (!session.cwd.trim().is_empty()).then(|| session.cwd.clone()),
    }
}

fn session_label(session_id: &str) -> String {
    session_meta(session_id).label
}

fn session_cwd(session_id: &str) -> Option<String> {
    session_meta(session_id).cwd
}

/// /resume 清单的一行（label 已截好，agent 标签独立保留）。
struct ResumeRow {
    session_id: String,
    label: String,
    /// agent 标签（显示用，已截好）。
    agent: String,
    last_active_at: String,
}

/// 全部历史会话，按 last_active_at 倒序。只在未持有 telegram inner 锁时调用
/// （锁序红线：inner → storage 禁止）。
fn historical_sessions(slot: &'static Telegram) -> Vec<ResumeRow> {
    let Some(state) = slot.app.try_state::<crate::AppState>() else {
        return Vec::new();
    };
    let Ok(storage) = state.storage.lock() else {
        return Vec::new();
    };
    let mut rows: Vec<ResumeRow> = Vec::new();
    for project in storage.list_projects().unwrap_or_default() {
        let sessions = storage.list_sessions(&project.id).unwrap_or_default();
        for session in sessions {
            rows.push(ResumeRow {
                session_id: session.id.clone(),
                label: session
                    .label
                    .trim()
                    .is_empty()
                    .then(|| clip(&session.id, 24))
                    .unwrap_or_else(|| clip(session.label.trim(), 40)),
                agent: crate::custom_agents::all_agents_merged()
                    .iter()
                    .find(|agent| agent.id == session.agent_id)
                    .map(|agent| clip(agent.label.trim(), 20))
                    .filter(|label| !label.is_empty())
                    .unwrap_or_else(|| clip(&session.agent_id, 20)),
                last_active_at: session.last_active_at.clone(),
            });
        }
    }
    // 最近活跃在前；时间戳是 ISO 字符串，字典序即时间序。
    rows.sort_by(|a, b| b.last_active_at.cmp(&a.last_active_at));
    rows
}

/// /resume 清单每页条数。
const RESUME_PAGE_SIZE: usize = 8;

/// /resume：全部历史会话菜单，点选即绑定并继续聊。
fn send_resume_menu(
    slot: &'static Telegram,
    live: &LiveConfig,
    offset: usize,
    reply_to: Option<i64>,
    host_message: Option<i64>,
) {
    let sessions = historical_sessions(slot);
    if sessions.is_empty() {
        let _ = send_message(
            live,
            &crate::ts!("还没有任何历史会话。", "No past sessions yet."),
            &[],
            reply_to,
            None,
            None,
        );
        return;
    }
    let offset = offset.min(sessions.len().saturating_sub(1) / RESUME_PAGE_SIZE * RESUME_PAGE_SIZE);
    let end = (offset + RESUME_PAGE_SIZE).min(sessions.len());
    let range = offset..end;
    let total = sessions.len();
    let mut header = crate::ts!(
        "📂 继续哪个会话？({} 个，按最近活跃排序)",
        "📂 Which session to continue? ({}, most recently active first)",
        total
    );
    if total > RESUME_PAGE_SIZE {
        header.push_str(&crate::ts!(
            "\n第 {}–{} 项",
            "\nItems {}–{}",
            offset + 1,
            end
        ));
    }
    let mut rows: Vec<Vec<(CallbackAction, String)>> = Vec::new();
    for row in &sessions[range.clone()] {
        rows.push(vec![(
            CallbackAction::ResumeSession {
                session_id: row.session_id.clone(),
                label: row.label.clone(),
            },
            if row.agent.is_empty() {
                clip(&row.label, 56)
            } else {
                clip(&format!("{} · {}", row.label, row.agent), 56)
            },
        )]);
    }
    let (prev, next) = page_nav(
        range.end < total,
        offset > 0,
        CallbackAction::ResumePage {
            offset: offset.saturating_sub(RESUME_PAGE_SIZE),
        },
        CallbackAction::ResumePage { offset: range.end },
    );
    if let (Some(prev), Some(next)) = (&prev, &next) {
        rows.push(vec![prev.clone(), next.clone()]);
    } else if let Some(nav) = prev.or(next) {
        rows.push(vec![nav]);
    }
    rows.push(vec![(
        CallbackAction::DismissMenu,
        crate::ts!("✖ 关闭", "✖ Close"),
    )]);
    let (keyboard, pending) = assemble_keyboard(slot, rows);
    register_callbacks(slot, pending);
    deliver_menu(live, &header, &keyboard, reply_to, host_message);
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
    let (pending, last_error, live_enabled) = runtime.unwrap_or((None, None, false));

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
                return Err(crate::ts!(
                    "还没有填 bot token",
                    "No bot token filled in yet"
                ));
            }
            if settings.chat_id.is_none() {
                return Err(
                    crate::ts!("还没配对：先在 Telegram 里给 bot 发一条消息，再回到这里确认", "Not paired yet: message the bot on Telegram first, then come back here to confirm"),
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
            crate::ts!(
                "当前没有待确认的配对请求（给 bot 发条消息，等它出现在这里）",
                "No pairing request waiting (message the bot and let it show up here)"
            )
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
                    crate::ts!("🎬 配对完成。Marionette 的会话动态会推到这里；直接回复文字即可继续对话，等批准的通知上会有按钮。", "🎬 Paired. Marionette session updates will arrive here; reply with text to continue the conversation — approval notices come with buttons.")
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
        .unwrap_or_else(|| crate::ts!("会话", "session"));
    let _ = slot.outbound.send(Outbound::Ack {
        ok,
        error,
        session_label: label,
        reply_to,
    });
}

/// 前端执行完远程命令（/new 建会话、/model 切模型）的回执。
/// `bind_session_id` 有值 = 同时把裸文本的默认去向切到这个会话（新建后立即生效）。
pub fn result(text: String, bind_session_id: Option<String>) {
    let Some(slot) = TELEGRAM.get() else { return };
    if let Some(session_id) = bind_session_id.as_deref() {
        if let Ok(mut inner) = slot.inner.lock() {
            inner.last_user_session = Some(session_id.to_string());
        }
        schedule_dashboard(slot);
    }
    let _ = slot.outbound.send(Outbound::Send {
        text,
        buttons: Vec::new(),
        route_for_session: bind_session_id,
        reply_to: None,
        md: false,
        plain: None,
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
        let Ok(mut live) = slot.live.lock() else {
            return;
        };
        live.enabled = settings.enabled && token.is_some();
        live.token = token.clone();
        live.chat_id = settings.chat_id;
        live.prefs = settings.prefs.clone();
    }
    if token.is_some() {
        ensure_threads(slot);
        sync_bot_commands(slot);
    }
}

/// 给 BotFather 注册命令清单（手机输入框 / 自动补全）。成功一次即够；
/// 失败只记日志——补全缺席不影响功能。
fn sync_bot_commands(slot: &'static Telegram) {
    if slot.commands_synced.load(Ordering::Relaxed) {
        return;
    }
    let active = slot
        .live
        .lock()
        .map(|live| live.enabled && live.chat_id.is_some())
        .unwrap_or(false);
    if !active {
        return;
    }
    let Some(token) = slot.live.lock().ok().and_then(|live| live.token.clone()) else {
        return;
    };
    slot.commands_synced.store(true, Ordering::Relaxed);
    std::thread::spawn(move || {
        let commands = json!([
            { "command": "new", "description": crate::ts!("新建对话（项目/聊天 → agent → 模型 → 模式）", "New conversation (project/chat → agent → model → mode)") },
            { "command": "resume", "description": crate::ts!("继续过往会话（选一个直接聊）", "Resume a past session (pick one and chat)") },
            { "command": "model", "description": crate::ts!("切换模型（回复通知=切那个会话）", "Switch model (the reply notice switches that session)") },
            { "command": "status", "description": crate::ts!("全部会话的状态清单", "Status list of all sessions") },
            { "command": "help", "description": crate::ts!("命令说明", "Command help") },
        ]);
        match tg_call(
            &token,
            "setMyCommands",
            &json!({ "commands": commands }),
            Duration::from_secs(SEND_HTTP_TIMEOUT_SECS),
        ) {
            Ok(_) => log("info", "bot command menu registered", None),
            Err(error) => {
                log("warn", "setMyCommands failed", Some(&error));
            }
        }
    });
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

    /// ureq 2.12 的 Display 会把 `bot<TOKEN>` 带进错误文本（Status 打
    /// `url: status code N`、Transport 打 `url: kind: msg`，重定向再带一个）。
    /// 清洗后任何形态都不得残留 token。
    #[test]
    fn sanitize_error_strips_token_from_all_ureq_shapes() {
        let token = "8123456789:AAEhBOweik6ad9r_QXMENQjcrGbqCr4K-3c";
        // Status 形态（getUpdates 经 .map_err(e.to_string()) 直通）。
        let status = format!("https://api.telegram.org/bot{token}/getUpdates: status code 401");
        assert_eq!(
            sanitize_error("getUpdates", &status),
            "getUpdates: https://api.telegram.org/bot<redacted>/getUpdates: status code 401"
        );
        // Transport 形态（tg_call 的 other 分支）。
        let transport =
            format!("https://api.telegram.org/bot{token}/sendMessage: Dns: whois failed");
        assert_eq!(
            sanitize_error("sendMessage", &transport),
            "sendMessage: https://api.telegram.org/bot<redacted>/sendMessage: Dns: whois failed"
        );
        // 重定向历史再带一个 URL 的形态。
        let redirected = format!(
            "https://api.telegram.org/bot{token}/getMe: status code 302 (redirected from https://api.telegram.org/bot{token}/login)"
        );
        let cleaned = sanitize_error("getMe", &redirected);
        assert!(
            !cleaned.contains(token),
            "token must not survive: {cleaned}"
        );
        assert_eq!(cleaned.matches("<redacted>").count(), 2);
        // 无 URL 的错误原样透传（只加方法名前缀）。
        assert_eq!(sanitize_error("getMe", "no token"), "getMe: no token");
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
        assert_eq!(stop_reason_head("end_turn"), ("✅", "跑完".to_string()));
        assert_eq!(
            stop_reason_head("max_tokens"),
            ("✂️", "到达长度上限".to_string())
        );
        assert_eq!(stop_reason_head("refusal"), ("⚠️", "模型拒答".to_string()));
        assert_eq!(
            stop_reason_head("max_turn_requests"),
            ("⏹", "达到回合上限".to_string())
        );
        assert_eq!(
            stop_reason_head("anything-else"),
            ("✅", "跑完".to_string())
        );
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
        let dir =
            std::env::temp_dir().join(format!("marionette_tg_corrupt_{}", std::process::id()));
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
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("telegram.corrupt-")
            });
        assert!(!archived, "parse failure must not archive the file");

        // 未来版本：留档后从默认重来。
        std::fs::write(&path, r#"{"version": 99, "enabled": true}"#).unwrap();
        let settings = load_settings_at(&path);
        assert!(!settings.enabled);
        let archived = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("telegram.corrupt-")
            });
        assert!(
            archived,
            "newer-version telegram.json must be archived, not wiped"
        );
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
            dash_row(
                "主线",
                "Claude Code",
                DashState::Running,
                Some(125),
                0,
                false,
            ),
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
        assert!(
            text.contains("⏎ ▶ [跑着的] · Grok · 跑了 1分00秒"),
            "{text}"
        );
        assert!(text.contains("⏸ [等着的] · 等你批准/回答"), "{text}");
        assert!(
            text.contains("💤 [闲着的] · Codex · 15分钟前有动静"),
            "{text}"
        );
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
        assert_eq!(
            split_markdown_paragraphs(fenced, 100),
            vec![fenced.to_string()]
        );
        // 单块超限硬切（换行优先、多字节安全）。
        let long = "很长的段落".repeat(50);
        for chunk in split_markdown_paragraphs(&long, 40) {
            assert!(chunk.chars().count() <= 40);
        }
    }

    #[test]
    fn model_page_parts_aligns_and_clamps() {
        let models: Vec<crate::acp::ModelDef> = (0..19)
            .map(|i| crate::acp::ModelDef {
                id: format!("m{i}"),
                label: format!("模型 {i}"),
                description: None,
            })
            .collect();
        // offset 越界时夹回最后一页；起始对齐页边界。
        let (text, range) = model_page_parts("标题", &models, 999);
        assert!(text.contains("第 3/3 页"), "{text}");
        assert_eq!(range, 16..19);
        let (text, range) = model_page_parts("标题", &models, 9);
        assert!(text.contains("第 2/3 页"), "{text}");
        assert_eq!(range, 8..16);
    }

    #[test]
    fn page_nav_bounds() {
        let (prev, next) = page_nav(
            true,
            false,
            CallbackAction::NewCancel,
            CallbackAction::NewCancel,
        );
        assert!(prev.is_none());
        assert!(next.is_some());
        let (prev, next) = page_nav(
            false,
            true,
            CallbackAction::NewCancel,
            CallbackAction::NewCancel,
        );
        assert!(prev.is_some());
        assert!(next.is_none());
        let (prev, next) = page_nav(
            false,
            false,
            CallbackAction::NewCancel,
            CallbackAction::NewCancel,
        );
        assert!(prev.is_none() && next.is_none());
    }

    #[test]
    fn reply_markup_must_be_inline_keyboard_object() {
        // 回归：reply_markup 必须是 {"inline_keyboard": …} 对象壳。
        // 裸数组会被 Telegram 400（can't parse reply keyboard markup），
        // 曾导致总控面板一条都发不出去。
        let keyboard = vec![
            vec![
                TgButton {
                    text: "⏹ [NAS 脚本]".into(),
                    callback_id: 42,
                },
                TgButton {
                    text: "名".into(),
                    callback_id: 7,
                },
            ],
            vec![TgButton {
                text: "🔄 刷新".into(),
                callback_id: 9,
            }],
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

    #[test]
    fn turn_text_pushes_only_the_last_segment() {
        // 工具调用之间的解说段不进推送（用户实测「乱七八糟都一起发过来了」）。
        let mut text = TurnText::default();
        text.append("我先看看文件。\n");
        text.tool_boundary();
        text.append("改完了，");
        text.append("测试全绿");
        assert_eq!(text.take_final(), "改完了，测试全绿");
        // 取完清空，不跨回合泄漏。
        assert_eq!(text.take_final(), "");
    }

    #[test]
    fn turn_text_falls_back_to_previous_segment_on_tool_ended_turn() {
        // 回合以工具调用收尾（末段为空）：退回上一段，免得推空正文。
        let mut text = TurnText::default();
        text.append("中途结论……");
        text.tool_boundary();
        assert_eq!(text.take_final(), "中途结论……");
        // 连续工具调用（空段）不覆盖 previous。
        text.append("A");
        text.tool_boundary();
        text.tool_boundary();
        assert_eq!(text.take_final(), "A");
    }

    #[test]
    fn turn_text_reset_clears_both_segments() {
        let mut text = TurnText::default();
        text.append("旧段");
        text.tool_boundary();
        text.append("新段");
        text.reset();
        assert_eq!(text.take_final(), "");
    }

    #[test]
    fn rich_request_body_carries_native_markdown_unescaped() {
        let body = rich_request_body(7, "**完成**\n\n| a | b |\n|---|---|");
        assert_eq!(body["chat_id"], json!(7));
        // 原生 markdown 原样直投，绝不做 MarkdownV2 转义。
        assert_eq!(
            body["rich_message"]["markdown"],
            json!("**完成**\n\n| a | b |\n|---|---|")
        );
        // 超限按 UTF-16 码元裁剪（32768 硬顶留余量）。
        let long = "字".repeat(40_000);
        let clipped = rich_request_body(1, &long);
        assert!(
            clipped["rich_message"]["markdown"]
                .as_str()
                .unwrap()
                .encode_utf16()
                .count()
                <= RICH_MARKDOWN_LIMIT
        );
    }

    #[test]
    fn render_session_detail_shows_state_body_and_privacy_gate() {
        let row = dash_row(
            "主线",
            "Claude Code",
            DashState::Running,
            Some(125),
            0,
            true,
        );
        let text = render_session_detail(&row, "改完了，测试全绿", true);
        assert!(text.contains("📄 ⏎ [主线] · Claude Code"), "{text}");
        assert!(text.contains("跑了 2分05秒"), "{text}");
        assert!(text.contains("改完了，测试全绿"), "{text}");
        assert!(text.contains("定向继续这个会话"), "{text}");
        // 内容预览关闭 → 正文不出本机。
        let locked = render_session_detail(&row, "秘密内容", false);
        assert!(locked.contains("内容预览已关闭"), "{locked}");
        assert!(!locked.contains("秘密内容"), "{locked}");
        // 空正文给提示，不推空块。
        let empty = render_session_detail(&row, "  ", true);
        assert!(empty.contains("最近一条回复为空"), "{empty}");
        // 空闲态显示闲置年龄。
        let idle = dash_row("调研", "", DashState::Idle, None, 240, false);
        let idle_text = render_session_detail(&idle, "", true);
        assert!(idle_text.contains("4分钟前有动静"), "{idle_text}");
    }

    // ─── 回复带图（2026-11-23）────────────────────────────────────────────

    fn local(path: &str) -> TurnImage {
        TurnImage::Local(PathBuf::from(path))
    }

    #[test]
    fn strip_file_uri_mirrors_the_frontend_shapes() {
        assert_eq!(strip_file_uri("file:///D:/a b.png"), "D:/a b.png");
        assert_eq!(strip_file_uri("file://localhost/C:/x.png"), "C:/x.png");
        // UNC 主机保留成 //host/…。
        assert_eq!(
            strip_file_uri("file://server/share/x.png"),
            "//server/share/x.png"
        );
        // `file://D:/x`：两斜杠 + 盘符（非标准但常见）。
        assert_eq!(strip_file_uri("file://D:/x.png"), "D:/x.png");
        // 百分号解码。
        assert_eq!(strip_file_uri("file:///D:/a%20b.png"), "D:/a b.png");
        // 竖线盘符的旧形态。
        assert_eq!(strip_file_uri("file:///C|/x.png"), "C:/x.png");
        // 非 file: 只修盘符前导斜杠；Windows 反斜杠原样保留。
        assert_eq!(strip_file_uri("/D:/x.png"), "D:/x.png");
        assert_eq!(strip_file_uri("D:\\x\\y.png"), "D:\\x\\y.png");
    }

    #[test]
    fn resolve_tool_image_path_matches_the_frontend_resolver() {
        // 相对路径按 cwd 补全，分隔符跟随 cwd。
        assert_eq!(
            resolve_tool_image_path("out/a.png", Some("D:\\proj")),
            "D:\\proj\\out\\a.png"
        );
        assert_eq!(
            resolve_tool_image_path("./screenshot.png", Some("/home/u")),
            "/home/u/screenshot.png"
        );
        // `../` 不吃掉——交给文件系统解析。
        assert_eq!(
            resolve_tool_image_path("../up.png", Some("D:\\proj\\sub")),
            "D:\\proj\\sub\\..\\up.png"
        );
        // 绝对路径与没 cwd 的原样返回。
        assert_eq!(resolve_tool_image_path("D:/abs.png", None), "D:/abs.png");
        assert_eq!(resolve_tool_image_path("rel.png", None), "rel.png");
        assert_eq!(
            resolve_tool_image_path("/nix/abs.png", None),
            "/nix/abs.png"
        );
    }

    #[test]
    fn extract_update_images_collects_blocks_locations_and_raw_input() {
        // image 块：uri（file://）、url、只有 base64 的 data。
        let update = json!({
            "sessionUpdate": "agent_message_chunk",
            "content": [
                { "type": "text", "text": "看这张 " },
                { "type": "image", "uri": "file:///D:/shots/a.png" },
                { "type": "image", "url": "https://example.com/b.png?x=1" },
                { "type": "image", "mimeType": "image/png", "data": "aGVsbG8=" },
                { "type": "image", "data": "d29ybGQ=" },
            ]
        });
        let images = extract_update_images(&update);
        assert_eq!(
            images,
            vec![
                local("D:/shots/a.png"),
                TurnImage::Remote("https://example.com/b.png?x=1".to_string()),
                TurnImage::Bytes {
                    data: "aGVsbG8=".to_string(),
                    mime: "image/png".to_string()
                },
                // 没有 mimeType 时补默认 image/png。
                TurnImage::Bytes {
                    data: "d29ybGQ=".to_string(),
                    mime: "image/png".to_string()
                },
            ]
        );

        // content 里没有图 → 工具 locations 的图片路径兜底（要带图片扩展名）。
        let update = json!({
            "sessionUpdate": "tool_call",
            "content": [{ "type": "tool_call", "title": "write_image" }],
            "locations": [{ "path": "D:/work/out/chart.png" }],
        });
        assert_eq!(
            extract_update_images(&update),
            vec![local("D:/work/out/chart.png")]
        );
        // 非图片扩展名的 location 不收。
        let update = json!({
            "sessionUpdate": "tool_call",
            "content": [{ "type": "tool_call" }],
            "locations": [{ "path": "D:/work/src/main.rs" }],
        });
        assert!(extract_update_images(&update).is_empty());

        // locations 缺席时 rawInput 的 JSON 路径兜底；裸字符串得像路径。
        let update = json!({
            "sessionUpdate": "tool_call",
            "content": [{ "type": "tool_call" }],
            "rawInput": "{ \"path\": \"renders\\\\shot.webp\" }",
        });
        assert_eq!(
            extract_update_images(&update),
            vec![local("renders\\shot.webp")]
        );
        let update = json!({
            "sessionUpdate": "tool_call",
            "content": [{ "type": "tool_call" }],
            "rawInput": "rm -rf /tmp/x",
        });
        assert!(extract_update_images(&update).is_empty());

        // tool_result 的嵌套 content 里也有 image 块。
        let update = json!({
            "sessionUpdate": "tool_call",
            "content": [
                { "type": "tool_result", "content": [
                    { "type": "image", "path": "D:/cache/frame0001.bmp" }
                ]}
            ]
        });
        assert_eq!(
            extract_update_images(&update),
            vec![local("D:/cache/frame0001.bmp")]
        );

        // 同一张图在同一 update 里只收一次（D:\ 与 D:/ 同键）。
        let update = json!({
            "content": [
                { "type": "image", "path": "D:\\dup.png" },
                { "type": "image", "path": "D:/dup.png" },
            ]
        });
        assert_eq!(extract_update_images(&update), vec![local("D:/dup.png")]);

        // content 里已经有图时 locations 的同路径不重复收（Read 一张图：
        // base64 块 + file_path 同时给）。
        let update = json!({
            "content": [
                { "type": "image", "data": "aGVsbG8=", "filePath": "D:/pics/cat.jpg" },
            ],
            "locations": [{ "path": "D:/pics/cat.jpg" }],
        });
        assert_eq!(extract_update_images(&update).len(), 1);
    }

    #[test]
    fn turn_image_from_src_routes_data_urls_and_gate_extensions() {
        // data: URL → 内联字节（mime 保留）。
        assert_eq!(
            turn_image_from_src("data:image/webp;base64,AAAA", true),
            Some(TurnImage::Bytes {
                data: "AAAA".to_string(),
                mime: "image/webp".to_string()
            })
        );
        // base64 之外的分页符编码不认。
        assert_eq!(turn_image_from_src("data:image/png,%%%", true), None);
        // 引用式（require_image_ext=true）：非图片扩展名不收。
        assert_eq!(turn_image_from_src("D:/x/main.rs", true), None);
        assert_eq!(
            turn_image_from_src("D:/x/a.png", true),
            Some(local("D:/x/a.png"))
        );
        // image 块自述是图（false）：没有扩展名也收。
        assert_eq!(
            turn_image_from_src("D:/temp/IMG_2048", false),
            Some(local("D:/temp/IMG_2048"))
        );
        // 体量闸：注定超限的 base64 直接不收（留足余量，别贴着整数除法边界）。
        let huge = "A".repeat(IMAGE_MAX_BYTES as usize * 2);
        assert_eq!(
            turn_image_from_src(&format!("data:image/png;base64,{}", huge), true),
            None
        );
    }

    #[test]
    fn session_track_images_dedupe_cap_and_user_key_exclusion() {
        let user_bytes = TurnImage::Bytes {
            data: "dXNlcg==".to_string(),
            mime: "image/png".to_string(),
        };
        let mut track = SessionTrack::new();
        track.begin_turn(&[local("D:/mine/from-user.png"), user_bytes.clone()], true);
        // 同一张（分隔符差异）只留一次。
        track.push_images(vec![local("D:/gen/shot.png"), local("D:\\gen\\shot.png")]);
        assert_eq!(track.images.len(), 1);
        for name in ["a1", "a2", "a3"] {
            track.push_images(vec![local(&format!("D:/gen/{name}.png"))]);
        }
        // 窗口滚动：第 5 张（a4）把最旧的 shot 挤出去。
        track.push_images(vec![local("D:/gen/a4.png")]);
        assert_eq!(track.images.len(), 4);
        let keys: Vec<String> = track.images.iter().map(TurnImage::key).collect();
        assert!(!keys.contains(&local("D:/gen/shot.png").key()), "{keys:?}");
        assert!(keys.contains(&local("D:/gen/a1.png").key()));
        track.push_images(vec![local("D:/gen/a5.png")]);
        // 取回时用户自带的两张（路径 + base64）不在其中。
        let taken = track.take_images();
        let keys: Vec<String> = taken.iter().map(TurnImage::key).collect();
        assert_eq!(taken.len(), 4, "{keys:?}");
        assert!(
            !keys.contains(&local("D:/gen/a1.png").key()),
            "oldest dropped"
        );
        assert!(keys.contains(&local("D:/gen/a2.png").key()));
        assert!(keys.contains(&local("D:/gen/a5.png").key()));
        // 用户自带的图即使被 agent 原样引用也不推回。
        track.push_images(vec![local("D:/mine/from-user.png"), user_bytes.clone()]);
        let taken = track.take_images();
        let keys: Vec<String> = taken.iter().map(TurnImage::key).collect();
        assert!(
            !keys.contains(&local("D:/mine/from-user.png").key()),
            "{keys:?}"
        );
        assert!(!keys.contains(&user_bytes.key()), "{keys:?}");
        // 取完即清。
        assert!(track.take_images().is_empty());

        // 同一用户消息的后续 chunk：不清之前的图键。
        let mut track = SessionTrack::new();
        track.begin_turn(&[local("D:/mine/one.png")], true);
        track.begin_turn(&[local("D:/mine/two.png")], false);
        track.push_images(vec![local("D:/gen/new.png")]);
        let taken = track.take_images();
        assert_eq!(taken, vec![local("D:/gen/new.png")]);
    }

    #[test]
    fn markdown_image_srcs_skip_code_and_keep_order() {
        let text = "结果：\n\n![a](out/chart.png)\n正文 ![b <x>](https://e.com/b.jpg)\n";
        assert_eq!(
            markdown_image_srcs(text),
            vec![
                "out/chart.png".to_string(),
                "https://e.com/b.jpg".to_string()
            ]
        );
        // 围栏代码块里的图片语法不收；行内代码里的也不收。
        let text =
            "```\n![no](fenced.png)\n```\n行内 `![no](inline.png)` 之后 ![yes](D:/ok.webp)\n";
        assert_eq!(markdown_image_srcs(text), vec!["D:/ok.webp".to_string()]);
        // ~~~ 围栏、info string 的 ```rs 围栏同样跳过。
        let text = "~~~\n![no](t.png)\n~~~\n```rs\nlet x = 1;\n```\n![yes](t2.png)\n";
        assert_eq!(markdown_image_srcs(text), vec!["t2.png".to_string()]);
        // <带空格的路径> 与 title 形态。
        assert_eq!(
            markdown_image_srcs("![x](<my chart v2.png> \"标题\")"),
            vec!["my chart v2.png".to_string()]
        );
        // 引用式图片（![alt][ref]）不收——那不是文件。
        assert!(markdown_image_srcs("![alt][ref]\n\n[ref]: a.png").is_empty());
    }

    #[test]
    fn send_method_picks_photo_animation_or_document() {
        assert_eq!(send_method_for(Some("png")), SEND_PHOTO);
        assert_eq!(send_method_for(Some("jpg")), SEND_PHOTO);
        assert_eq!(send_method_for(Some("gif")), SEND_ANIMATION);
        assert_eq!(send_method_for(Some("bmp")), SEND_DOCUMENT);
        assert_eq!(send_method_for(Some("svg+xml")), SEND_DOCUMENT);
        // 认不出来先按照片试（会被拒，然后退成文件）。
        assert_eq!(send_method_for(None), SEND_PHOTO);
        // 按图本体挑发法。
        assert_eq!(TurnImage::send_as(&local("D:/x/anim.gif")), SEND_ANIMATION);
        assert_eq!(
            TurnImage::send_as(&TurnImage::Remote("https://e.com/a.bogus".to_string())),
            SEND_PHOTO
        );
    }

    #[test]
    fn multipart_body_lays_out_fields_then_file() {
        let file = UploadFile {
            field: "photo",
            filename: "a b.png".to_string(),
            mime: "image/png".to_string(),
            data: &[0x01, 0x02, 0x03, 0xff][..],
        };
        let body = multipart_body(
            "marionette-boundary",
            &[
                ("chat_id", "12345".to_string()),
                ("caption", "你好".to_string()),
            ],
            &file,
        );
        let text = String::from_utf8_lossy(&body);
        assert!(text.starts_with("--marionette-boundary\r\n"), "{text}");
        assert!(text.contains("Content-Disposition: form-data; name=\"chat_id\"\r\n\r\n12345\r\n"));
        assert!(text.contains("Content-Disposition: form-data; name=\"caption\"\r\n\r\n你好\r\n"));
        assert!(text.contains(
            "Content-Disposition: form-data; name=\"photo\"; filename=\"a b.png\"\r\nContent-Type: image/png\r\n\r\n"
        ));
        // 二进制段原样落位（不是 lossy 字符串的一部分，按字节找）。
        assert!(body.windows(4).any(|w| w == [0x01, 0x02, 0x03, 0xff]));
        assert!(text.ends_with("\r\n--marionette-boundary--\r\n"), "{text}");
    }

    #[test]
    fn decode_base64_image_takes_padded_and_unpadded() {
        assert_eq!(decode_base64_image("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode_base64_image("aGVsbG8").unwrap(), b"hello");
        // 空白（chunk 里可能带换行）无碍。
        assert_eq!(decode_base64_image("aGVs\n bG8=").unwrap(), b"hello");
        assert!(decode_base64_image("!!!").is_err());
        // 先估量：注定超限的不解码（返回错误而不是 OOM）。
        let huge = "A".repeat(IMAGE_MAX_BYTES as usize * 2);
        let error = decode_base64_image(&huge).unwrap_err();
        assert!(error.contains("over the"), "{error}");
    }

    #[test]
    fn tg_error_status_and_rejected_recognize_bot_api_rejections() {
        assert_eq!(
            tg_error_status("HTTP 400 · sendPhoto: BAD PHOTO"),
            Some(400)
        );
        assert_eq!(
            tg_error_status("HTTP 429 · sendPhoto: Too Many Requests"),
            Some(429)
        );
        assert!(tg_error_status("sendPhoto: Dns: whois failed").is_none());
        assert!(rejected("HTTP 400 · sendPhoto: BAD PHOTO"));
        // 429 是限流不是拒收；传输层错误两个都不是。
        assert!(!rejected("HTTP 429 · sendPhoto: Too Many Requests"));
        assert!(!rejected("sendPhoto: Dns: whois failed"));
    }

    /// 回环假服务器收一单 multipart：请求行/头部/字段段/文件段全须验。
    /// 直连 agent（不走 `agent_with_proxy`——它每次读用户的代理设置盘）。
    #[test]
    fn multipart_upload_reaches_the_wire() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
            haystack.windows(needle.len()).position(|w| w == needle)
        }

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buf: Vec<u8> = Vec::new();
            let mut chunk = [0u8; 4096];
            let header_end = loop {
                let read = stream.read(&mut chunk).expect("read request");
                if read == 0 {
                    panic!("connection closed before headers finished");
                }
                buf.extend_from_slice(&chunk[..read]);
                if let Some(at) = find_subsequence(&buf, b"\r\n\r\n") {
                    break at + 4;
                }
            };
            let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
            let content_length: usize = headers
                .lines()
                .find(|line| line.to_ascii_lowercase().starts_with("content-length"))
                .and_then(|line| line.split(':').nth(1))
                .and_then(|value| value.trim().parse().ok())
                .expect("content-length header");
            while buf.len() < header_end + content_length {
                let read = stream.read(&mut chunk).expect("read body");
                if read == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..read]);
            }
            let body = buf[header_end..].to_vec();
            let payload = json!({ "ok": true, "result": { "message_id": 4242 } }).to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            stream
                .write_all(response.as_bytes())
                .expect("write response");
            (headers, body)
        });

        let agent = ureq::AgentBuilder::new().build();
        let file = UploadFile {
            field: "photo",
            filename: "chart 绘图.png".to_string(),
            mime: "image/png".to_string(),
            data: &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a][..],
        };
        let value = tg_call_multipart_via(
            &agent,
            &format!("http://127.0.0.1:{port}"),
            "secret-token",
            "sendPhoto",
            &[
                ("chat_id", "12345".to_string()),
                ("caption", "图".to_string()),
            ],
            &file,
            Duration::from_secs(5),
        )
        .expect("multipart upload");
        assert_eq!(message_id_of(&value), Some(4242));

        let (headers, body) = server.join().expect("server thread");
        assert!(
            headers.contains("POST /botsecret-token/sendPhoto"),
            "{headers}"
        );
        assert!(headers
            .to_ascii_lowercase()
            .contains("multipart/form-data; boundary="));
        let body_text = String::from_utf8_lossy(&body);
        assert!(
            body_text.contains("name=\"chat_id\"\r\n\r\n12345\r\n"),
            "{body_text}"
        );
        assert!(
            body_text.contains("name=\"caption\"\r\n\r\n图\r\n"),
            "{body_text}"
        );
        assert!(
            body_text.contains(
                "name=\"photo\"; filename=\"chart 绘图.png\"\r\nContent-Type: image/png\r\n\r\n"
            ),
            "{body_text}"
        );
        // PNG 魔数按字节落位（没被任何编码弄脏）。
        assert!(
            body.windows(8)
                .any(|w| w == [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
            "png magic bytes must ride intact"
        );
        // 结尾边界收口。
        let tail = String::from_utf8_lossy(&body[body.len() - 64..]);
        assert!(tail.contains("--\r\n"), "closing boundary, tail: {tail}");
    }
}
