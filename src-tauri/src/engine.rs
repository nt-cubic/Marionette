//! L2 翻译引擎 — 内嵌 magpie 无头子进程（docs/cross-agent-bridge.md §4.5，S1 处方）。
//!
//! 订阅型跨用（Grok / Claude / ChatGPT 登录态）不经 Marionette 自己翻译，而是
//! 按需下载 [magpie](https://github.com/yetone/magpie) 的 CLI 二进制，以
//! `magpie serve` 无头跑在 127.0.0.1 上，把私有端点翻成标准 OpenAI / Anthropic
//! 协议。S1 spike 的源码级结论（本模块照抄的处方）：
//!
//! - **安装**：`~/.marionette/engine/`——`magpie.exe`（GitHub releases 的
//!   `magpie-cli-windows-amd64.exe` 资产，~48MB）+ 空 `.portable` 标记文件
//!   （全部状态自动进 `engine/data/`，零 env 围栏）+ `manifest.json`
//!   `{version, sha256, installedAt}`。SHA256SUMS 校验失败删除重下一次；
//!   临时文件 + rename 原子落位；下载尊重 `proxy.rs` 的代理配置。
//! - **拉起**：`magpie serve`，env `MAGPIE_ADDR=127.0.0.1:<port>` +
//!   `MAGPIE_NO_STATS=1`；**不设** `MAGPIE_NO_ACCOUNTS`（订阅 discovery 要开）、
//!   **保留** `USERPROFILE`/`APPDATA`（discovery 要读 `~/.grok` 等真实凭证）。
//! - **端口**：先探测 3425 —— `GET /` 的 body 含 `"magpie"` 即用户自装实例在
//!   跑，直接复用（`reused=true`，绝不强杀别人的进程）；否则从 3425 起找空闲
//!   端口拉自己的。
//! - **健康**：`GET /v1/models` 200 且 body 是 JSON 目录形状（不能只看状态码，
//!   附录 B4.4——200 + HTML 是 Cloudflare 挑战，不是引擎）。
//! - **退出**：child 句柄存全局状态，app 退出路径 TerminateProcess 直杀
//!   （magpie 的 JSON 全部 tmp+rename 原子写，强杀安全）。
//! - **provider add**：引擎健康后对本机凭据库里的 key-plan entry 跑短命 CLI
//!   进程注册（先 `provider key` 更新、不存在再 `provider add`——直接 add 对
//!   已存在的 provider 会开出 `deepseek-2` 副本）。订阅账号由引擎自己的
//!   discovery 发现，Marionette 不碰。
//!
//! 安全底线（§5）：key 只进子进程命令行（短命、不落日志、不落 Marionette 磁盘）；
//! 日志只记 provider 名与事件。引擎令牌是字面量 `magpie`（S1：网关回环任意
//! token 放行），所以它也 不是机密。

use serde::{Deserialize, Serialize};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// model_bridge 侧的引擎伪来源 id（绑定 entry_id / preset 都是它）。
pub const ENGINE_ENTRY_ID: &str = "engine";
/// grok owned 表里的 preset id 变体（指到引擎的 base）。
pub const ENGINE_PRESET_ID: &str = "engine";
/// 注入给 agent 的网关令牌（字面量；回环任意 token 放行，S1 实测）。
pub const ENGINE_TOKEN: &str = "magpie";

const DEFAULT_PORT: u16 = 3425;
const GITHUB_REPO: &str = "yetone/magpie";
const ASSET_NAME: &str = "magpie-cli-windows-amd64.exe";
const USER_AGENT: &str = "Marionette/0.1 (engine)";
/// 48MB 下载的整体预算（慢网络 + 断点重来一次）。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
/// serve 起来后等 /v1/models 变健康的窗口（discovery 首启可能慢）。
const HEALTH_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTH_POLL: Duration = Duration::from_millis(300);
/// 环回请求（探测 / 健康 / 目录）的超时——引擎在本机，慢即异常。
const LOOP_HTTP_TIMEOUT: Duration = Duration::from_secs(4);
const REUSE_PROBE_TIMEOUT: Duration = Duration::from_millis(1500);
/// 引擎目录缓存的 TTL；失败负缓存短一些（订阅登录进来后目录会变）。
const CATALOG_TTL: Duration = Duration::from_secs(60);
const CATALOG_FAIL_TTL: Duration = Duration::from_secs(15);
/// 子进程 stdout/stderr 进 debug_log 的单行上限（防刷屏）。
const MAX_LOG_LINE: usize = 200;
/// 从默认端口向上找空闲端口的尝试次数。
const PORT_SCAN_ATTEMPTS: u32 = 64;

// ─── 状态机 ─────────────────────────────────────────────────────────────────

/// 引擎的对外状态（`engine_status` 的返回 + `engine-state-changed` 事件负载）。
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase", tag = "state")]
pub enum EngineStatus {
    /// 未安装也未尝试。UI 引导「启用」。
    Absent,
    /// 后台下载中（进度同时走 `engine-download-progress` 事件）。
    Downloading { downloaded: u64, total: u64 },
    /// 已装好，正在拉 serve + 等健康。
    Starting,
    /// 网关在 127.0.0.1 上可用。`reused = true` 表示复用了用户自装实例
    /// （3425 上探测到 magpie——那种进程不属于我们，退出时不杀）。
    Running {
        port: u16,
        version: Option<String>,
        reused: bool,
    },
    Failed { error: String },
}

/// serve 子进程句柄。测试用 Fake 变体（不起真进程）。
enum ChildHandle {
    Real(std::process::Child),
    #[allow(dead_code)]
    Fake,
}

impl ChildHandle {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        match self {
            ChildHandle::Real(child) => child.try_wait(),
            ChildHandle::Fake => Ok(None),
        }
    }
    fn kill(&mut self) {
        if let ChildHandle::Real(child) = self {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

struct EngineState {
    status: EngineStatus,
    /// 我们自己拉起的 serve 进程（reused 实例为 None）。
    child: Option<ChildHandle>,
}

static STATE: Mutex<EngineState> = Mutex::new(EngineState {
    status: EngineStatus::Absent,
    child: None,
});

fn state_lock() -> &'static Mutex<EngineState> {
    &STATE
}

fn lock_state() -> std::sync::MutexGuard<'static, EngineState> {
    state_lock().lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ─── 事件（进度驱动 UI；无 AppHandle 的测试环境静默跳过）─────────────────────

static APP_HANDLE: OnceLock<Mutex<Option<tauri::AppHandle>>> = OnceLock::new();

/// main.rs setup 里登记，供后台线程发 tauri 事件。
pub fn set_app_handle(app: tauri::AppHandle) {
    let slot = APP_HANDLE.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(app);
    }
}

fn emit_event<T: Serialize>(event: &str, payload: &T) {
    let Some(slot) = APP_HANDLE.get() else { return };
    let Ok(guard) = slot.lock() else { return };
    let Some(app) = guard.as_ref() else { return };
    use tauri::Emitter;
    let _ = app.emit(event, payload);
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadProgress {
    downloaded: u64,
    total: u64,
}

/// 状态迁移的唯一入口：写状态 + 发 `engine-state-changed` + 记 dev.log。
/// Downloading 的刷屏态只发进度事件（不下日志），终态/阶段态走完整通知。
fn set_status(status: EngineStatus) {
    {
        let mut state = lock_state();
        state.status = status.clone();
    }
    if let EngineStatus::Downloading { downloaded, total } = &status {
        emit_download_progress(*downloaded, *total);
        return;
    }
    emit_event("engine-state-changed", &status);
    let summary: String = match &status {
        EngineStatus::Absent => "engine state → absent".to_string(),
        EngineStatus::Downloading { .. } => unreachable!("handled above"),
        EngineStatus::Starting => "engine state → starting".to_string(),
        EngineStatus::Running { port, version, reused } => format!(
            "engine state → running port={port} version={} reused={reused}",
            version.as_deref().unwrap_or("?")
        ),
        EngineStatus::Failed { error } => format!("engine state → failed: {error}"),
    };
    let level = if matches!(status, EngineStatus::Failed { .. }) { "warn" } else { "info" };
    crate::debug_log::append("engine", level, "", &summary, None);
}

fn emit_download_progress(downloaded: u64, total: u64) {
    let progress = DownloadProgress { downloaded, total };
    emit_event("engine-download-progress", &progress);
}

// ─── 路径与 manifest ─────────────────────────────────────────────────────────

pub fn engine_dir() -> Result<PathBuf, String> {
    #[cfg(test)]
    if let Some(dir) = test_engine_dir() {
        return Ok(dir);
    }
    crate::app_paths::global_dir()
        .map(|dir| dir.join("engine"))
        .map_err(|error| format!("无法定位 ~/.marionette: {error}"))
}

fn exe_path(dir: &Path) -> PathBuf {
    dir.join("magpie.exe")
}

fn manifest_path(dir: &Path) -> PathBuf {
    dir.join("manifest.json")
}

/// 安装清单：版本 + 期望的 sha256（exe 完整性的本地记录）。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EngineManifest {
    pub version: String,
    pub sha256: String,
    pub installed_at: String,
}

/// 读 manifest；损坏（坏 JSON / 缺字段）→ 改名留档后视为未安装（下次启用
/// 重新下载），绝不静默当作有效。
pub(crate) fn load_manifest_at(dir: &Path) -> Option<EngineManifest> {
    let path = manifest_path(dir);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return None;
    };
    match serde_json::from_str::<EngineManifest>(&text) {
        Ok(manifest)
            if !manifest.version.trim().is_empty()
                && !manifest.sha256.trim().is_empty() =>
        {
            // sha256 归一成小写（SHA256SUMS 与我们自己的计算都是小写；规范
            // 化后 roundtrip 与比较不用惦记大小写）。
            let manifest = EngineManifest {
                sha256: manifest.sha256.trim().to_ascii_lowercase(),
                ..manifest
            };
            Some(manifest)
        }
        _ => {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let archived = path.with_file_name(format!("manifest.corrupt-{ts}.json"));
            let _ = std::fs::rename(&path, &archived);
            crate::debug_log::append(
                "engine",
                "warn",
                "",
                "manifest.json corrupted; archived, engine will re-download",
                Some(&archived.display().to_string()),
            );
            None
        }
    }
}

pub(crate) fn save_manifest_at(dir: &Path, manifest: &EngineManifest) -> Result<(), String> {
    let body = serde_json::to_string_pretty(manifest)
        .map_err(|e| format!("序列化 manifest.json 失败: {e}"))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("创建 engine 目录失败: {e}"))?;
    let temp = dir.join(format!("manifest.json.{}.tmp", std::process::id()));
    std::fs::write(&temp, format!("{body}\n"))
        .map_err(|e| format!("写 manifest.json 失败: {e}"))?;
    if let Err(error) = std::fs::rename(&temp, manifest_path(dir)) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("manifest.json 原子落位失败: {error}"));
    }
    Ok(())
}

/// exe 是否在位且与 manifest 记录的 sha256 一致（决定要不要重新下载）。
fn is_installed_at(dir: &Path) -> bool {
    let Some(manifest) = load_manifest_at(dir) else {
        return false;
    };
    let exe = exe_path(dir);
    if !exe.is_file() {
        return false;
    }
    match sha256_of_file(&exe) {
        Ok(actual) => actual.eq_ignore_ascii_case(&manifest.sha256),
        Err(_) => false,
    }
}

// ─── SHA256 ─────────────────────────────────────────────────────────────────

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn sha256_of_file(path: &Path) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let mut file =
        std::fs::File::open(path).map_err(|e| format!("打开 {} 失败: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 256 * 1024];
    loop {
        let n = file
            .read(&mut buffer)
            .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

/// 解析 SHA256SUMS 里目标资产的条目（`<sha>  <name>` / `<sha> *<name>`）。
fn parse_sums_entry(text: &str, asset: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (sha, name) = match line.split_once(' ').or_else(|| line.split_once('\t')) {
            Some(pair) => pair,
            None => continue,
        };
        let name = name.trim().trim_start_matches('*');
        if name.eq_ignore_ascii_case(asset) {
            let sha = sha.trim();
            if sha.len() == 64 && sha.chars().all(|c| c.is_ascii_hexdigit()) {
                return Some(sha.to_ascii_lowercase());
            }
        }
    }
    None
}

// ─── 网络（下载走代理配置；环回请求绝不走代理）───────────────────────────────

/// 下载用 agent：native-tls + 可选代理（尊重 proxy.rs）。
fn download_agent() -> Result<ureq::Agent, String> {
    #[cfg(test)]
    if test_direct_http() {
        // 测试直连（绕过用户本机代理，本地假 HTTP 服务才可达）。
        return loopback_agent();
    }
    let connector = ureq::native_tls::TlsConnector::new()
        .map_err(|e| format!("TLS 初始化失败 (schannel): {e}"))?;
    let mut builder = ureq::AgentBuilder::new();
    builder = builder.tls_connector(std::sync::Arc::new(connector));
    let config = crate::proxy::load_proxy_config();
    if config.enabled && !config.url.trim().is_empty() {
        match ureq::Proxy::new(config.url.trim()) {
            Ok(proxy) => builder = builder.proxy(proxy),
            Err(error) => {
                crate::debug_log::append(
                    "engine",
                    "warn",
                    "",
                    "proxy config invalid; downloading directly",
                    Some(&format!("{error}")),
                );
            }
        }
    }
    Ok(builder.build())
}

/// 环回 agent：显式不带代理（系统代理劫持 127.0.0.1 的坑，B2 观察名单）。
fn loopback_agent() -> Result<ureq::Agent, String> {
    crate::http_client::agent()
}

// ─── release 解析与下载 ─────────────────────────────────────────────────────

#[derive(Clone)]
struct ReleasePlan {
    version: String,
    asset_url: String,
    sha256: String,
}

fn strip_v(tag: &str) -> &str {
    tag.trim().trim_start_matches(['v', 'V'])
}

fn resolve_latest_release() -> Result<ReleasePlan, String> {
    #[cfg(test)]
    if let Some(mock) = test_release_plan() {
        return mock;
    }
    let url = format!("https://api.github.com/repos/{GITHUB_REPO}/releases/latest");
    let agent = download_agent()?;
    let response = agent
        .get(&url)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", USER_AGENT)
        .timeout(Duration::from_secs(20))
        .call()
        .map_err(|e| format!("查询 GitHub Releases 失败（无网络或被墙？）: {e}"))?;
    let value: serde_json::Value = response
        .into_json()
        .map_err(|e| format!("解析 GitHub Releases 应答失败: {e}"))?;
    let tag = value
        .get("tag_name")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    if tag.is_empty() {
        return Err("GitHub Releases 应答里没有 tag_name".to_string());
    }
    let assets = value
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let asset_url = assets
        .iter()
        .find(|asset| {
            asset
                .get("name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| name.eq_ignore_ascii_case(ASSET_NAME))
        })
        .and_then(|asset| asset.get("browser_download_url"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("latest release 没有 `{ASSET_NAME}` 资产"))?
        .to_string();
    let sums_url = assets
        .iter()
        .find(|asset| {
            asset
                .get("name")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|name| name.to_ascii_lowercase().contains("sha256sums"))
        })
        .and_then(|asset| asset.get("browser_download_url"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "latest release 没有校验文件（SHA256SUMS）— 拒绝无校验下载".to_string())?
        .to_string();
    let sums_text = agent
        .get(&sums_url)
        .set("User-Agent", USER_AGENT)
        .timeout(Duration::from_secs(20))
        .call()
        .map_err(|e| format!("下载 SHA256SUMS 失败: {e}"))?
        .into_string()
        .map_err(|e| format!("读取 SHA256SUMS 失败: {e}"))?;
    let sha256 = parse_sums_entry(&sums_text, ASSET_NAME)
        .ok_or_else(|| format!("SHA256SUMS 里没有 `{ASSET_NAME}` 的条目"))?;
    Ok(ReleasePlan {
        version: strip_v(&tag).to_string(),
        asset_url,
        sha256,
    })
}

/// 流式下载资产到临时文件，边收边报进度；sha 校验失败 → 删除并 **重试一次**
/// （仍是坏的就放弃）。成功才 rename 落位 + 写 `.portable` + manifest。
fn download_and_install(plan: &ReleasePlan, dir: &Path) -> Result<(), String> {
    let mut last_error = String::new();
    for attempt in 0..2 {
        match download_once(plan, dir) {
            Ok(()) => return Ok(()),
            Err(error) => {
                if attempt == 0 {
                    crate::debug_log::append(
                        "engine",
                        "warn",
                        "",
                        "engine download attempt 1 failed; retrying",
                        Some(&error),
                    );
                }
                last_error = error;
            }
        }
    }
    Err(last_error)
}

fn download_once(plan: &ReleasePlan, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建 engine 目录失败: {e}"))?;
    let temp = dir.join(format!("magpie.exe.{}.download", std::process::id()));
    let response = download_agent()?
        .get(&plan.asset_url)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/octet-stream")
        .timeout(DOWNLOAD_TIMEOUT)
        .call()
        .map_err(|e| format!("下载 magpie 失败: {e}"))?;
    let total: u64 = response
        .header("Content-Length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let mut reader = response.into_reader();
    let mut file = std::fs::File::create(&temp)
        .map_err(|e| format!("创建临时文件失败（{}）: {e}", temp.display()))?;
    let mut buffer = vec![0u8; 64 * 1024];
    let mut downloaded: u64 = 0;
    let mut last_event = Instant::now();
    loop {
        let n = reader
            .read(&mut buffer)
            .map_err(|e| format!("读取下载流失败: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buffer[..n])
            .map_err(|e| format!("写入下载流失败: {e}"))?;
        downloaded += n as u64;
        // 进度节流：状态机每 250ms 刷一次，事件同频（UI 不需要更密）。
        if last_event.elapsed() >= Duration::from_millis(250) {
            set_status(EngineStatus::Downloading { downloaded, total });
            last_event = Instant::now();
        }
    }
    file.flush().map_err(|e| format!("落盘失败: {e}"))?;
    drop(file);
    if downloaded < 1024 * 1024 {
        let _ = std::fs::remove_file(&temp);
        return Err(format!(
            "下载的文件只有 {} 字节 — 不像 magpie.exe（~48MB）",
            downloaded
        ));
    }
    let actual = sha256_of_file(&temp)?;
    if !actual.eq_ignore_ascii_case(&plan.sha256) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!(
            "SHA256 校验失败（期望 {}…，实际 {}…）— 已删除，重试下载",
            &plan.sha256[..8.min(plan.sha256.len())],
            &actual[..8.min(actual.len())]
        ));
    }
    set_status(EngineStatus::Downloading { downloaded, total: downloaded });
    // 原子落位 + 围栏标记 + manifest。
    std::fs::rename(&temp, exe_path(dir))
        .map_err(|e| format!("magpie.exe 原子落位失败: {e}"))?;
    std::fs::write(dir.join(".portable"), b"")
        .map_err(|e| format!("写 .portable 标记失败: {e}"))?;
    let manifest = EngineManifest {
        version: plan.version.clone(),
        sha256: plan.sha256.clone(),
        installed_at: now_epoch_millis(),
    };
    save_manifest_at(dir, &manifest)?;
    crate::debug_log::append(
        "engine",
        "info",
        "",
        &format!("engine installed version={}", plan.version),
        Some(&format!("sha256={}…", &plan.sha256[..12.min(plan.sha256.len())])),
    );
    Ok(())
}

fn now_epoch_millis() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
        .to_string()
}

// ─── 端口探测 / 拉起 / 健康 ────────────────────────────────────────────────

/// `GET http://127.0.0.1:<port>/` 的 body 含 "magpie"（网关 info 端点返回
/// `{"name":"magpie",...}`）→ 认为用户自装实例在跑。顺带抠 version。
fn probe_reuse_at(port: u16) -> Option<Option<String>> {
    #[cfg(test)]
    if let Some(hook) = test_reuse_hook() {
        return hook(port);
    }
    let Ok(agent) = loopback_agent() else {
        return None;
    };
    let Ok(response) = agent
        .get(&format!("http://127.0.0.1:{port}/"))
        .timeout(REUSE_PROBE_TIMEOUT)
        .call()
    else {
        return None;
    };
    let Ok(body) = response.into_string() else {
        return None;
    };
    if !body.contains("magpie") {
        return None;
    }
    let version = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| {
            value
                .get("version")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        });
    Some(version)
}

/// 从 `start` 起找一个能 bind 的环回端口（绑定后立刻释放；serve 马上会占上）。
fn find_free_port_from(start: u16) -> Option<u16> {
    for offset in 0..PORT_SCAN_ATTEMPTS {
        let candidate = u32::from(start) + offset;
        if candidate > u32::from(u16::MAX) {
            break;
        }
        let port = candidate as u16;
        match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(listener) => {
                drop(listener);
                return Some(port);
            }
            Err(_) => continue,
        }
    }
    None
}

/// `magpie serve` 拉起（S1 处方的 env 全集）。stdout/stderr 逐行进 debug_log
/// （截断防刷屏）。
fn spawn_serve(exe: &Path, port: u16) -> Result<ChildHandle, String> {
    #[cfg(test)]
    if let Some(hook) = test_launch_hook() {
        return hook(port).map(|()| ChildHandle::Fake);
    }
    let mut command = std::process::Command::new(exe);
    command
        .arg("serve")
        // S1：显式钉住监听地址；关每日遥测。
        .env("MAGPIE_ADDR", format!("127.0.0.1:{port}"))
        .env("MAGPIE_NO_STATS", "1")
        // 刻意不设 MAGPIE_NO_ACCOUNTS（订阅 discovery 要开）、保留
        // USERPROFILE/APPDATA（discovery 读 ~/.grok 等真实凭证）——
        // std Command 默认继承父 env，正是我们要的。
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .current_dir(exe.parent().unwrap_or(Path::new(".")));
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("拉起 magpie serve 失败（{}）: {e}", exe.display()))?;
    if let Some(stdout) = child.stdout.take() {
        spawn_log_thread(stdout);
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_log_thread(stderr);
    }
    Ok(ChildHandle::Real(child))
}

fn spawn_log_thread<R: Read + Send + 'static>(reader: R) {
    let _ = std::thread::Builder::new().name("engine-serve-log".into()).spawn(move || {
        let mut reader = std::io::BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    let trimmed = line.trim_end_matches(['\r', '\n']);
                    if trimmed.is_empty() {
                        continue;
                    }
                    crate::debug_log::append(
                        "engine",
                        "info",
                        "",
                        "serve:",
                        Some(cap_at_char_boundary(trimmed, MAX_LOG_LINE)),
                    );
                }
            }
        }
    });
}

/// 按字符边界截断（PTY drain 断言的教训：绝不在多字节字符中间切）。
fn cap_at_char_boundary(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// 健康检查：`GET /v1/models` 200 且 body 是 JSON 目录形状（`data[]` /
/// `models[]`）——只看状态码会把「200 + HTML 挑战页」当健康（B4.4）。
fn engine_healthy_at(port: u16) -> bool {
    #[cfg(test)]
    if let Some(hook) = test_health_hook() {
        return hook(port);
    }
    let Ok(agent) = loopback_agent() else {
        return false;
    };
    let Ok(response) = agent
        .get(&format!("http://127.0.0.1:{port}/v1/models"))
        .set("Accept", "application/json")
        .timeout(LOOP_HTTP_TIMEOUT)
        .call()
    else {
        return false;
    };
    if response.status() != 200 {
        return false;
    }
    let Ok(body) = response.into_string() else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) else {
        return false;
    };
    value
        .get("data")
        .and_then(serde_json::Value::as_array)
        .or_else(|| value.get("models").and_then(serde_json::Value::as_array))
        .is_some()
}

// ─── key 型 provider 注册（订阅账号由引擎 discovery 自己发现）────────────────

/// 我们的 preset id → magpie 的 provider 注册参数（preset 名, add 的 k=v 附带）。
/// 名字以 magpie 源码 internal/provider/presets.go 为准（2026-10-16 核实）：
/// deepseek→`deepseek`、kimi→`moonshot`、glm→`zhipu`（我们的 chat 端点是
/// Coding Plan 端点，add 时要带 `region=coding`，否则落到按量端点报「无余额」）、
/// openrouter→`openrouter`、xai→`xai`、opencode-go→`opencode-go`。
fn magpie_provider_for_preset(preset: &str) -> Option<(&'static str, &'static str)> {
    match preset {
        "deepseek" => Some(("deepseek", "")),
        "kimi" => Some(("moonshot", "")),
        "glm" => Some(("zhipu", "region=coding")),
        "openrouter" => Some(("openrouter", "")),
        "xai" => Some(("xai", "")),
        "opencode-go" => Some(("opencode-go", "")),
        _ => None,
    }
}

/// 对凭据库里的每个 key-plan entry 注册进引擎。幂等策略：先试
/// `provider key <id> <key>`（已存在 → 换 key），失败再 `provider add`
/// （不存在 → 新增；对已存在会开出 `-2` 副本，所以顺序是 key 在前）。
/// 任何一条失败只记 warn，不阻塞引擎进入 Running。
fn register_key_providers(exe: &Path) {
    #[cfg(test)]
    if test_register_hook().is_some() {
        return;
    }
    let Some(home) = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
    else {
        return;
    };
    let credentials = home.join(".marionette").join("credentials.json");
    let Ok(store) = crate::credentials::load_store_at(&credentials) else {
        return;
    };
    for entry in store
        .entries
        .iter()
        .filter(|entry| entry.kind == crate::credentials::CredentialKind::KeyPlan)
    {
        let Some(preset) = entry.preset.as_deref() else { continue };
        let Some((provider_name, extra)) = magpie_provider_for_preset(preset) else {
            continue;
        };
        let Ok(key) = crate::credentials::peek_secret_at(&credentials, &entry.id) else {
            crate::debug_log::append(
                "engine",
                "warn",
                "",
                "engine provider register: key undecryptable",
                Some(&format!("entry={}", entry.id)),
            );
            continue;
        };
        let Some(key) = key else { continue };
        // key 绝不进日志/事件——命令行参数只存在于这个短命子进程里。
        let updated = run_magpie_cli(
            exe,
            &["provider", "key", provider_name, key.as_str()],
        );
        let result = match updated {
            Ok(()) => Ok(()),
            Err(_) => {
                let mut args: Vec<&str> = vec!["provider", "add", provider_name, key.as_str()];
                if !extra.is_empty() {
                    args.push(extra);
                }
                run_magpie_cli(exe, &args)
            }
        };
        match result {
            Ok(()) => crate::debug_log::append(
                "engine",
                "info",
                "",
                "engine provider registered",
                Some(provider_name),
            ),
            Err(error) => crate::debug_log::append(
                "engine",
                "warn",
                "",
                "engine provider register failed",
                Some(&format!("provider={provider_name}: {error}")),
            ),
        }
    }
}

/// 跑一个短命 magpie CLI 命令（.portable 围栏由 exe 位置决定，无需 env）。
fn run_magpie_cli(exe: &Path, args: &[&str]) -> Result<(), String> {
    let mut command = std::process::Command::new(exe);
    command
        .args(args)
        .env("MAGPIE_NO_STATS", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .current_dir(exe.parent().unwrap_or(Path::new(".")));
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let output = command
        .output()
        .map_err(|e| format!("拉起 magpie CLI 失败: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(cap_at_char_boundary(stderr.trim(), MAX_LOG_LINE).to_string())
    }
}

// ─── enable 流水线（后台线程）───────────────────────────────────────────────

/// 幂等启用：Running / Downloading / Starting → 直接返回当前态；否则登记
/// Downloading 并在后台线程跑完「（复用探测 →）下载校验 → serve → 健康 →
/// provider 注册 → Running」。失败落 `Failed{error}`（事件 + 日志留档）。
pub fn enable() -> Result<EngineStatus, String> {
    let dir = engine_dir()?;
    let claimed = {
        let mut state = lock_state();
        match state.status {
            EngineStatus::Running { .. }
            | EngineStatus::Downloading { .. }
            | EngineStatus::Starting => None,
            EngineStatus::Absent | EngineStatus::Failed { .. } => {
                state.status = EngineStatus::Downloading { downloaded: 0, total: 0 };
                Some(EngineStatus::Downloading { downloaded: 0, total: 0 })
            }
        }
    };
    let Some(initial) = claimed else {
        return Ok(status());
    };
    emit_event("engine-state-changed", &initial);
    std::thread::Builder::new()
        .name("engine-enable".into())
        .spawn(move || {
            if let Err(error) = enable_pipeline(&dir) {
                set_status(EngineStatus::Failed { error: error.clone() });
            }
        })
        .map_err(|e| format!("启动引擎后台线程失败: {e}"))?;
    Ok(initial)
}

fn enable_pipeline(dir: &Path) -> Result<(), String> {
    // 1. 用户自装实例在默认端口上跑着 → 直接复用（不是我们的进程，不杀）。
    if let Some(version) = probe_reuse_at(DEFAULT_PORT) {
        crate::debug_log::append(
            "engine",
            "info",
            "",
            "reusing a user-installed magpie on the default port",
            Some(&format!("port={DEFAULT_PORT}")),
        );
        set_status(EngineStatus::Running {
            port: DEFAULT_PORT,
            version,
            reused: true,
        });
        return Ok(());
    }
    // 2. 安装（已在位且 sha 对得上 → 跳过下载）。
    if !is_installed_at(dir) {
        let plan = resolve_latest_release()?;
        download_and_install(&plan, dir)?;
    }
    // 3. 拉起 + 等健康。
    set_status(EngineStatus::Starting);
    let exe = exe_path(dir);
    let port = find_free_port_from(DEFAULT_PORT)
        .ok_or_else(|| format!("从 {DEFAULT_PORT} 起找不到空闲环回端口"))?;
    let mut child = spawn_serve(&exe, port)?;
    let deadline = Instant::now() + HEALTH_TIMEOUT;
    loop {
        if engine_healthy_at(port) {
            break;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(format!("引擎进程提前退出（code={}）— 详见 dev.log 的 serve 输出", status));
            }
            Ok(None) => {}
            Err(error) => {
                child.kill();
                return Err(format!("引擎进程状态查询失败: {error}"));
            }
        }
        if Instant::now() >= deadline {
            child.kill();
            return Err(format!(
                "引擎 {HEALTH_TIMEOUT:?} 内未通过健康检查（GET /v1/models）"
            ));
        }
        std::thread::sleep(HEALTH_POLL);
    }
    let version = load_manifest_at(dir).map(|manifest| manifest.version);
    {
        let mut state = lock_state();
        state.child = Some(child);
    }
    // 4. key 型 provider 注册（失败不阻塞；订阅由引擎 discovery 自己发现）。
    register_key_providers(&exe);
    set_status(EngineStatus::Running {
        port,
        version,
        reused: false,
    });
    Ok(())
}

// ─── 对外查询 / 生命周期 ────────────────────────────────────────────────────

/// 状态快照（顺带做子进程存活检查：serve 死了就地转 Failed）。
pub fn status() -> EngineStatus {
    let mut state = lock_state();
    if let EngineStatus::Running { port, .. } = state.status.clone() {
        let dead = state
            .child
            .as_mut()
            .is_some_and(|child| matches!(child.try_wait(), Ok(Some(_))));
        if dead {
            state.child = None;
            state.status = EngineStatus::Failed {
                error: format!("引擎进程已退出（端口 {port} 不再服务）"),
            };
            let error = state.status.clone();
            drop(state);
            emit_event("engine-state-changed", &error);
            return error;
        }
    }
    state.status.clone()
}

/// Running 才有值：(port, version)。model_bridge / 注入解析用。
pub fn running_base() -> Option<(u16, Option<String>)> {
    match status() {
        EngineStatus::Running { port, version, .. } => Some((port, version)),
        _ => None,
    }
}

/// 引擎的 `/v1` 地址（grok owned 表的 `base_url`——chat_completions 后端向
/// base_url 拼 `/chat/completions`）。
pub fn engine_v1_url() -> Option<String> {
    running_base().map(|(port, _)| format!("http://127.0.0.1:{port}/v1"))
}

/// model_bridge 侧「引擎就绪」的统一判定（伪来源可见性 / 绑定校验 / 注入
/// 解析共用）。生产 = 引擎 Running。测试构建默认 **false**：引擎状态是全局
/// 的，并行测试不能让不相干的桥测试看到彼此的引擎——要测引擎接线的测试用
/// [`test_running`] 显式打开。
pub fn bridge_ready() -> bool {
    #[cfg(test)]
    {
        return seams()
            .map(|guard| guard.as_ref().is_some_and(|seams| seams.bridge_visible))
            .unwrap_or(false);
    }
    #[cfg(not(test))]
    {
        running_base().is_some()
    }
}

/// app 退出路径杀引擎。只杀**我们自己拉起**的 serve（reused 实例是用户的，
/// 不碰）。magpie 全部 tmp+rename 原子写，TerminateProcess 直杀安全（S1）。
pub fn kill_engine() {
    let mut state = lock_state();
    if let Some(mut child) = state.child.take() {
        child.kill();
        crate::debug_log::append("engine", "info", "", "engine serve killed on shutdown", None);
        state.status = EngineStatus::Absent;
    }
}

// ─── 模型目录（引擎 id 是 `provider/model` 形态，如 grok/grok-4.7）──────────

struct CatalogEntry {
    fetched_at: Instant,
    models: Vec<String>,
}

static CATALOG: OnceLock<Mutex<Option<CatalogEntry>>> = OnceLock::new();

/// 引擎 `/v1/models` 全量（带 TTL 缓存）。引擎不在 Running → 空。
pub fn catalog() -> Vec<String> {
    #[cfg(test)]
    if let Some(models) = test_catalog() {
        return models;
    }
    let cache = CATALOG.get_or_init(|| Mutex::new(None));
    if let Ok(guard) = cache.lock() {
        if let Some(entry) = guard.as_ref() {
            let ttl = if entry.models.is_empty() {
                CATALOG_FAIL_TTL
            } else {
                CATALOG_TTL
            };
            if entry.fetched_at.elapsed() < ttl {
                return entry.models.clone();
            }
        }
    }
    let models = fetch_catalog();
    if let Ok(mut guard) = cache.lock() {
        *guard = Some(CatalogEntry {
            fetched_at: Instant::now(),
            models: models.clone(),
        });
    }
    models
}

fn fetch_catalog() -> Vec<String> {
    let Some(port) = running_base().map(|(port, _)| port) else {
        return Vec::new();
    };
    let Ok(agent) = loopback_agent() else {
        return Vec::new();
    };
    let Ok(response) = agent
        .get(&format!("http://127.0.0.1:{port}/v1/models"))
        .set("Accept", "application/json")
        .timeout(LOOP_HTTP_TIMEOUT)
        .call()
    else {
        return Vec::new();
    };
    let Ok(body) = response.into_string() else {
        return Vec::new();
    };
    crate::model_bridge::parse_models_catalog(&body)
}

// ─── 测试缝（全部 #[cfg(test)]；全局状态 → 用串行锁防并行测试互踩）──────────

#[cfg(test)]
type BoxLaunch = std::sync::Arc<dyn Fn(u16) -> Result<(), String> + Send + Sync>;
#[cfg(test)]
type BoxHealth = std::sync::Arc<dyn Fn(u16) -> bool + Send + Sync>;
#[cfg(test)]
type BoxReuse = std::sync::Arc<dyn Fn(u16) -> Option<Option<String>> + Send + Sync>;
#[cfg(test)]
type BoxVoid = std::sync::Arc<dyn Fn() + Send + Sync>;

#[cfg(test)]
struct Seams {
    dir: Option<PathBuf>,
    launch: Option<BoxLaunch>,
    health: Option<BoxHealth>,
    reuse: Option<BoxReuse>,
    register: Option<BoxVoid>,
    release: Option<Result<ReleasePlan, String>>,
    catalog: Option<Vec<String>>,
    /// 测试直连下载源（绕过用户本机代理，本地假 HTTP 服务才可达）。
    direct_http: bool,
    /// model_bridge 的引擎伪来源在测试里是否可见（见 engine_source_visible）。
    bridge_visible: bool,
}

#[cfg(test)]
impl Default for Seams {
    fn default() -> Self {
        Self {
            dir: None,
            launch: None,
            health: None,
            reuse: None,
            register: None,
            release: None,
            catalog: None,
            direct_http: false,
            bridge_visible: false,
        }
    }
}

#[cfg(test)]
static SEAMS: OnceLock<Mutex<Option<Seams>>> = OnceLock::new();

#[cfg(test)]
static TEST_SERIAL: Mutex<()> = Mutex::new(());

/// 测试句柄：Drop 时清缝 + 状态归零，保证全局状态不泄漏到下一个测试。
#[cfg(test)]
pub(crate) struct TestHandle {
    _serial: std::sync::MutexGuard<'static, ()>,
}

#[cfg(test)]
impl Drop for TestHandle {
    fn drop(&mut self) {
        if let Some(slot) = SEAMS.get() {
            if let Ok(mut guard) = slot.lock() {
                *guard = None;
            }
        }
        let mut state = lock_state();
        state.status = EngineStatus::Absent;
        state.child = None;
        if let Some(cache) = CATALOG.get() {
            if let Ok(mut guard) = cache.lock() {
                *guard = None;
            }
        }
    }
}

/// 安装测试缝（拿到串行锁直到句柄 drop）。仅本模块测试用；跨模块的引擎
/// 接线测试走 [`test_running`]。
#[cfg(test)]
fn install_test_seams(seams: Seams) -> TestHandle {
    let serial = TEST_SERIAL.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let slot = SEAMS.get_or_init(|| Mutex::new(None));
    if let Ok(mut guard) = slot.lock() {
        *guard = Some(seams);
    }
    TestHandle { _serial: serial }
}

/// 便捷入口：把引擎摆成 Running + 指定目录缓存 + 对 model_bridge 可见
/// （model_bridge 的引擎接线测试用）。
#[cfg(test)]
pub(crate) fn test_running(catalog: Vec<String>) -> TestHandle {
    let handle = install_test_seams(Seams {
        catalog: Some(catalog),
        bridge_visible: true,
        ..Seams::default()
    });
    let mut state = lock_state();
    state.status = EngineStatus::Running {
        port: 39999,
        version: Some("test".to_string()),
        reused: false,
    };
    state.child = None;
    handle
}

#[cfg(test)]
fn seams() -> Option<std::sync::MutexGuard<'static, Option<Seams>>> {
    let slot = SEAMS.get()?;
    slot.lock().ok()
}

#[cfg(test)]
fn test_engine_dir() -> Option<PathBuf> {
    seams().and_then(|guard| guard.as_ref().and_then(|s| s.dir.clone()))
}

#[cfg(test)]
fn test_launch_hook() -> Option<BoxLaunch> {
    seams().and_then(|guard| guard.as_ref().and_then(|s| s.launch.clone()))
}

#[cfg(test)]
fn test_health_hook() -> Option<BoxHealth> {
    seams().and_then(|guard| guard.as_ref().and_then(|s| s.health.clone()))
}

#[cfg(test)]
fn test_reuse_hook() -> Option<BoxReuse> {
    seams().and_then(|guard| guard.as_ref().and_then(|s| s.reuse.clone()))
}

#[cfg(test)]
fn test_register_hook() -> Option<BoxVoid> {
    seams().and_then(|guard| guard.as_ref().and_then(|s| s.register.clone()))
}

#[cfg(test)]
fn test_direct_http() -> bool {
    seams().map(|guard| guard.as_ref().is_some_and(|s| s.direct_http)).unwrap_or(false)
}

#[cfg(test)]
fn test_release_plan() -> Option<Result<ReleasePlan, String>> {
    seams().and_then(|guard| {
        guard.as_ref().and_then(|s| {
            s.release.as_ref().map(|r| match r {
                Ok(plan) => Ok(plan.clone()),
                Err(error) => Err(error.clone()),
            })
        })
    })
}

#[cfg(test)]
fn test_catalog() -> Option<Vec<String>> {
    seams().and_then(|guard| guard.as_ref().and_then(|s| s.catalog.clone()))
}

// ─── 测试 ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::net::TcpListener;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "marionette_test_engine_{tag}_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 一个单请求的假 HTTP 服务（端口探测 / 形状校验测试用）。
    fn serve_once(body: &'static str, content_type: &'static str) -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body.as_bytes());
            }
        });
        port
    }

    // ─── manifest ───────────────────────────────────────────────────────────

    #[test]
    fn manifest_roundtrips_and_corrupt_archives_instead_of_trusting() {
        let dir = temp_dir("manifest");
        assert_eq!(load_manifest_at(&dir), None, "没有文件 → 未安装");

        let manifest = EngineManifest {
            version: "0.1.1267".into(),
            sha256: sha256_hex(b"fake-exe").to_ascii_uppercase(),
            installed_at: "1".into(),
        };
        save_manifest_at(&dir, &manifest).unwrap();
        assert_eq!(load_manifest_at(&dir).unwrap().version, "0.1.1267");
        // sha 大小写不敏感（SHA256SUMS 可能是大写）。
        assert_eq!(load_manifest_at(&dir).unwrap().sha256, sha256_hex(b"fake-exe"));

        // 损坏 → 留档 + None（绝不当有效）。
        fs::write(manifest_path(&dir), "{\"version\":\"x\",\"sha256\":\"").unwrap();
        assert_eq!(load_manifest_at(&dir), None, "坏 JSON → 未安装");
        let archives: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with("manifest.corrupt-"))
            .collect();
        assert_eq!(archives.len(), 1, "应恰好留档一份: {archives:?}");

        // 缺字段的半截 manifest 同样不算数。
        fs::write(
            manifest_path(&dir),
            serde_json::to_string(&serde_json::json!({"version": "1", "sha256": ""})).unwrap(),
        )
        .unwrap();
        assert_eq!(load_manifest_at(&dir), None);

        fs::remove_dir_all(&dir).ok();
    }

    /// 在位校验：exe 字节与 manifest 的 sha 对得上才算安装（篡改即重下信号）。
    #[test]
    fn installed_check_verifies_exe_bytes_against_manifest() {
        let dir = temp_dir("installed");
        let bytes = b"this is definitely a magpie exe (fake)";
        fs::write(exe_path(&dir), bytes).unwrap();
        assert!(!is_installed_at(&dir), "没有 manifest → 不算安装");

        save_manifest_at(
            &dir,
            &EngineManifest {
                version: "v-test".into(),
                sha256: sha256_hex(bytes),
                installed_at: "1".into(),
            },
        )
        .unwrap();
        assert!(is_installed_at(&dir), "sha 对上 → 已安装");

        // 篡改一个字节 → 校验失败 → 视为未安装（下次启用重新下载）。
        fs::write(exe_path(&dir), b"this is definitely a magpie exE (fake)").unwrap();
        assert!(!is_installed_at(&dir));

        fs::remove_dir_all(&dir).ok();
    }

    // ─── SHA256SUMS / 日志截断 ───────────────────────────────────────────────

    #[test]
    fn parses_sha256sums_entry_in_both_separator_forms() {
        let sha = "a".repeat(64);
        let text = format!(
            "{}  other-asset.tar.gz\n{} *{}\n{}  {}\n",
            "b".repeat(64),
            sha,
            ASSET_NAME,
            "c".repeat(64),
            ASSET_NAME
        );
        assert_eq!(parse_sums_entry(&text, ASSET_NAME).as_deref(), Some(sha.as_str()));
        // 大小写不敏感；缺资产 / 非 hex → None。
        assert_eq!(parse_sums_entry(&text, "magpie-cli-darwin-arm64"), None);
        assert_eq!(parse_sums_entry(&format!("zzz  {ASSET_NAME}"), ASSET_NAME), None);
        assert_eq!(parse_sums_entry("", ASSET_NAME), None);
    }

    #[test]
    fn log_lines_cap_at_char_boundary() {
        let cjk = "喜".repeat(80); // 3 bytes/char
        let capped = cap_at_char_boundary(&cjk, MAX_LOG_LINE);
        assert!(capped.len() <= MAX_LOG_LINE);
        assert!(capped.chars().all(|c| c == '喜'), "不能切出半个字符");
        let short = "abc";
        assert_eq!(cap_at_char_boundary(short, MAX_LOG_LINE), short);
    }

    // ─── 端口探测（真环回 + 假 HTTP 服务）────────────────────────────────────

    #[test]
    fn port_probe_detects_magpie_and_ignores_foreign_bodies() {
        let _seams = install_test_seams(Seams::default());
        // body 含 "magpie"（真网关的 info 端点形状）→ 复用 + 抠到 version。
        let port = serve_once(r#"{"name":"magpie","version":"0.1.1267"}"#, "application/json");
        let version = probe_reuse_at(port).expect("应判定为 magpie 实例");
        assert_eq!(version.as_deref(), Some("0.1.1267"));

        // body 不含 "magpie"（3425 被别的服务占了）→ 不复用。
        let foreign = serve_once("<html>hello from some other app</html>", "text/html");
        assert!(probe_reuse_at(foreign).is_none());

        // 没人听 → 不复用。
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let closed = listener.local_addr().unwrap().port();
        drop(listener);
        assert!(probe_reuse_at(closed).is_none());
    }

    #[test]
    fn free_port_scan_skips_bound_ports() {
        let _seams = install_test_seams(Seams::default());
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let bound = listener.local_addr().unwrap().port();
        // 从被占端口起扫 → 必须跳过它。
        let found = find_free_port_from(bound).expect("应找到空闲端口");
        assert_ne!(found, bound);
        // 找到的端口真的可以 bind。
        assert!(TcpListener::bind(("127.0.0.1", found)).is_ok());
    }

    // ─── 状态机（fake 拉起，不起真进程）─────────────────────────────────────

    /// 等状态进入终态（Running / Failed），超时 panic（防挂死测试）。
    fn wait_for_terminal(deadline_secs: u64) -> EngineStatus {
        let deadline = Instant::now() + Duration::from_secs(deadline_secs);
        loop {
            let snapshot = status();
            if matches!(
                snapshot,
                EngineStatus::Running { .. } | EngineStatus::Failed { .. }
            ) {
                return snapshot;
            }
            assert!(Instant::now() < deadline, "引擎状态卡在 {snapshot:?}");
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// 已安装（假 exe + 对得上的 sha）+ fake 拉起 + fake 健康 → enable 幂等地
    /// 到达 Running；重复 enable 不再拉起（launch 计数恒 1）。
    #[test]
    fn enable_reaches_running_and_is_idempotent() {
        let dir = temp_dir("idempotent");
        let bytes = b"fake magpie exe bytes for state machine test";
        fs::write(exe_path(&dir), bytes).unwrap();
        fs::write(dir.join(".portable"), b"").unwrap();
        save_manifest_at(
            &dir,
            &EngineManifest {
                version: "9.9.9".into(),
                sha256: sha256_hex(bytes),
                installed_at: "1".into(),
            },
        )
        .unwrap();
        let launches = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let launches_for_hook = launches.clone();
        let _seams = install_test_seams(Seams {
            dir: Some(dir.clone()),
            // 复用探测 mock 成「3425 上没有 magpie」，测试不依赖本机环境。
            reuse: Some(std::sync::Arc::new(|_| None)),
            launch: Some(std::sync::Arc::new(move |_port| {
                launches_for_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })),
            health: Some(std::sync::Arc::new(|_| true)),
            register: Some(std::sync::Arc::new(|| {})),
            ..Seams::default()
        });

        let initial = enable().unwrap();
        assert!(matches!(initial, EngineStatus::Downloading { .. }));
        let reached = wait_for_terminal(5);
        let EngineStatus::Running { port, version, reused } = reached else {
            panic!("应到达 Running，实际 {reached:?}");
        };
        assert!(!reused);
        assert_eq!(version.as_deref(), Some("9.9.9"));
        assert!(port >= DEFAULT_PORT);

        // 幂等：Running 时再 enable 不重复拉起。
        let again = enable().unwrap();
        assert!(matches!(again, EngineStatus::Running { .. }));
        assert_eq!(
            launches.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "重复 enable 不能再拉起 serve"
        );

        fs::remove_dir_all(&dir).ok();
    }

    /// 无网络（release 解析被 mock 成失败）→ 干净落 Failed 并留档，无残留
    /// 临时文件、状态不再 Downloading。
    #[test]
    fn enable_fails_cleanly_when_release_unreachable() {
        let dir = temp_dir("offline");
        let _seams = install_test_seams(Seams {
            dir: Some(dir.clone()),
            reuse: Some(std::sync::Arc::new(|_| None)),
            release: Some(Err("mock: 网络不可达".to_string())),
            ..Seams::default()
        });
        let initial = enable().unwrap();
        assert!(matches!(initial, EngineStatus::Downloading { .. }));
        let failed = wait_for_terminal(5);
        let EngineStatus::Failed { error } = &failed else {
            panic!("应 Failed，实际 {failed:?}");
        };
        assert!(error.contains("网络不可达"), "unexpected: {error}");
        // 干净失败：目录里没有下载残留。
        assert!(
            fs::read_dir(&dir).unwrap().next().is_none(),
            "失败路径不能留下载残留: {:?}",
            fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect::<Vec<_>>()
        );

        // Failed → 再 enable 可以重试（重新 claim）。
        let retry = enable().unwrap();
        assert!(matches!(retry, EngineStatus::Downloading { .. }));
        wait_for_terminal(5);
        fs::remove_dir_all(&dir).ok();
    }

    /// 下载校验失败路径（假 exe 字节 + 期望错 sha）：校验失败 → 删除重来一次
    /// → 仍失败 → Err。用本地 HTTP 服务当下载源 + 正确的 SHA256SUMS 语义。
    #[test]
    fn sha_mismatch_deletes_and_retries_then_fails() {
        let dir = temp_dir("shafail");
        // 下载走真实代码路径（含代理 agent）→ 测试缝切直连。
        let _seams = install_test_seams(Seams {
            direct_http: true,
            ..Seams::default()
        });
        let payload: &'static [u8] = Box::leak(vec![0x4d; 2 * 1024 * 1024].into_boxed_slice());
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            // 两次下载尝试各服务一次（Content-Length 让 total 有值）。
            for _ in 0..2 {
                if let Ok((mut stream, _)) = listener.accept() {
                    let mut buffer = [0u8; 1024];
                    let _ = stream.read(&mut buffer);
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        payload.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(payload);
                }
            }
        });
        let plan = ReleasePlan {
            version: "0.0.0-test".into(),
            asset_url: format!("http://127.0.0.1:{port}/magpie.exe"),
            // 故意错的期望值 → 每次 sha 校验必失败。
            sha256: sha256_hex(b"not-the-real-bytes"),
        };
        let error = download_and_install(&plan, &dir).unwrap_err();
        assert!(error.contains("SHA256"), "unexpected: {error}");
        // 删除重来：目录里没有 magpie.exe / 残留临时文件。
        assert!(!exe_path(&dir).exists(), "校验失败的 exe 不能落位");
        let leftovers: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(leftovers.is_empty(), "失败路径要删干净: {leftovers:?}");
        fs::remove_dir_all(&dir).ok();
    }

    /// 健康检查的形状校验：200 + 合法目录 JSON → 健康；200 + HTML / 非 JSON →
    /// 不健康（B4.4：不能只看状态码）。
    #[test]
    fn health_check_requires_catalog_shape_not_just_200() {
        let _seams = install_test_seams(Seams::default());
        let good = serve_once(r#"{"object":"list","data":[{"id":"grok/grok-4.7"}]}"#, "application/json");
        assert!(engine_healthy_at(good));
        let empty = serve_once(r#"{"object":"list","data":[]}"#, "application/json");
        assert!(engine_healthy_at(empty), "空目录（无订阅）也是健康");
        let html = serve_once("<html><body>Cloudflare challenge</body></html>", "text/html");
        assert!(!engine_healthy_at(html), "200 + HTML 不是引擎");
        let envelope = serve_once(r#"{"code":1000,"message":"denied"}"#, "application/json");
        assert!(!engine_healthy_at(envelope), "200 + 信封不是目录");
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let closed = listener.local_addr().unwrap().port();
        drop(listener);
        assert!(!engine_healthy_at(closed));
    }

    /// provider 名映射锚定（magpie presets.go 2026-10-16 核实）。
    #[test]
    fn magpie_provider_names_are_pinned() {
        assert_eq!(magpie_provider_for_preset("deepseek"), Some(("deepseek", "")));
        assert_eq!(magpie_provider_for_preset("kimi"), Some(("moonshot", "")));
        // glm 的 key 是 Coding Plan key：不带 region=coding 会落到按量端点。
        assert_eq!(magpie_provider_for_preset("glm"), Some(("zhipu", "region=coding")));
        assert_eq!(magpie_provider_for_preset("openrouter"), Some(("openrouter", "")));
        assert_eq!(magpie_provider_for_preset("xai"), Some(("xai", "")));
        assert_eq!(magpie_provider_for_preset("opencode-go"), Some(("opencode-go", "")));
        assert_eq!(magpie_provider_for_preset("unknown"), None);
    }
}
