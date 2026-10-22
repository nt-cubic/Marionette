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
//!   **embedded-engine 构建**（产品定稿 2026-10-16「单 exe 内置引擎」）：
//!   nogui magpie 从 `third_party/magpie`（`tools\import-magpie.ps1` 吸纳）
//!   编出后 **UPX 压缩直嵌**进 exe（47.9MB → ~13.6MB，自解压零解码），
//!   **出厂种子**：本机 `~/.marionette/engine/` 尚无完整安装时离线落位。
//!   用户经 UI「更新引擎」从 GitHub 装上的版本成为本机权威，启动不再用内嵌
//!   盖回；「恢复出厂」才再解内嵌。无内嵌的开发构建仍走下载路径。
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
//! - **provider add**：引擎健康后把「凭据库 key-plan + opencode 扫描引用的
//!   api key」注册进引擎（2026-10-16 产品定稿「无脑打开软件就能用」；先
//!   `provider key` 更新、不存在再 `provider add`——直接 add 对已存在的
//!   provider 会开出 `deepseek-2` 副本；同一 provider 的 key-plan 优先于
//!   扫描引用）。订阅账号由引擎自己的 discovery 发现，Marionette 不碰。
//!   引擎随应用启动自动拉起（main.rs setup → `enable()`）；凭据变化 /
//!   扫描刷新时经 `reregister_key_providers()` 后台补注册（仅自拉实例）。
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
/// 用量面板点刷新时 magpie 会去问各家订阅，比目录查询慢一截。
const QUOTA_LIVE_TIMEOUT: Duration = Duration::from_secs(20);
/// 用量面板短缓存：回合结束和点刷新都会问，不该每次都打网关。
const QUOTA_CACHE_TTL: Duration = Duration::from_secs(30);
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
    /// 最近一次「有人需要引擎」的时刻（目录查询 / 别名同步 / 注入解析 /
    /// 注册）。空闲回收的计时基准。
    last_needed_at: Option<Instant>,
}

static STATE: Mutex<EngineState> = Mutex::new(EngineState {
    status: EngineStatus::Absent,
    child: None,
    last_needed_at: None,
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

#[cfg(test)]
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

// ─── 内嵌引擎（embedded-engine feature：UPX 压缩进 exe，离线解出安装）──────
//
// 产品定稿（2026-10-16）：带引擎版把 nogui magpie（third_party/magpie，
// `tools\import-magpie.ps1` 吸纳的上游子集）压进二进制。压缩层 2026-10-17
// 从 gzip 换成 **UPX**（实测 47.9MB → ~13.6MB，比 gzip 的 18.5MB 小 5MB，
// 且免掉运行时解码——UPX exe 启动时自解压）。
//
// 本机权威 = `~/.marionette/engine/`（manifest + exe）。内嵌只作出厂种子：
// 未安装/损坏时落位；用户「更新引擎」装上的版本粘住，启动不再按内嵌版本
// 强行覆盖。打包升内嵌 = import-magpie（默认同 latest 稳定 tag）+ 重编。

/// 内嵌引擎版本（下载路径存的是去 v 形态，这里同规矩归一后比较）。
#[cfg(feature = "embedded-engine")]
const EMBEDDED_ENGINE_VERSION: &str = include_str!("../engine-embedded/VERSION");
/// UPX 压缩后的 magpie exe 字节（自解压，无需解码层）。
#[cfg(feature = "embedded-engine")]
const EMBEDDED_MAGPIE: &[u8] = include_bytes!("../engine-embedded/magpie-windows-amd64.exe");

fn embedded_version() -> Option<&'static str> {
    #[cfg(feature = "embedded-engine")]
    {
        Some(strip_v(EMBEDDED_ENGINE_VERSION))
    }
    #[cfg(not(feature = "embedded-engine"))]
    {
        None
    }
}

fn embedded_artifact() -> (&'static [u8], &'static str) {
    #[cfg(feature = "embedded-engine")]
    {
        (EMBEDDED_MAGPIE, strip_v(EMBEDDED_ENGINE_VERSION))
    }
    #[cfg(not(feature = "embedded-engine"))]
    {
        (&[], "")
    }
}

/// 需要用内嵌种子落位：有内嵌构建，且本机还没有完整可用的引擎。
/// 已装好的用户版（含主动从 GitHub 更新的）一律保留。
fn needs_embedded_seed(dir: &Path) -> bool {
    embedded_version().is_some() && !is_installed_at(dir)
}

/// 把内嵌的（UPX 自解压）magpie 字节落位安装：临时文件 + rename 原子写 +
/// `.portable` 围栏 + manifest 记真实 sha——与下载路径共用 `is_installed_at`
/// 的校验语义。纯输入参数，测试直接喂小夹具。
fn install_embedded_engine_at(bytes: &[u8], version: &str, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("创建 engine 目录失败: {e}"))?;
    let temp = dir.join(format!("magpie.exe.{}.extract", std::process::id()));
    std::fs::write(&temp, bytes)
        .map_err(|e| format!("写解出临时文件失败（{}）: {e}", temp.display()))?;
    let sha = sha256_of_file(&temp)?;
    std::fs::rename(&temp, exe_path(dir))
        .map_err(|e| format!("magpie.exe 原子落位失败: {e}"))?;
    std::fs::write(dir.join(".portable"), b"")
        .map_err(|e| format!("写 .portable 标记失败: {e}"))?;
    let manifest = EngineManifest {
        version: version.to_string(),
        sha256: sha,
        installed_at: now_epoch_millis(),
    };
    save_manifest_at(dir, &manifest)?;
    crate::debug_log::append(
        "engine",
        "info",
        "",
        &format!("engine embedded artifact installed version={version}"),
        Some(&format!("bytes={}", bytes.len())),
    );
    Ok(())
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

/// 本次要注册的 (magpie provider, key, add 附带 k=v)。key-plan 优先：同一
/// provider 已有 key-plan 时跳过它的扫描引用（否则两边各注册一次会开出 `-2`
/// 副本 / 互相覆盖 key）。
fn registration_plan(
    key_plans: &[(String, String)],
    references: &[(String, String)],
) -> Vec<(&'static str, String, &'static str)> {
    let mut plan: Vec<(&'static str, String, &'static str)> = Vec::new();
    for (preset, key) in key_plans.iter().chain(references.iter()) {
        let Some((name, extra)) = magpie_provider_for_preset(preset) else {
            continue;
        };
        if plan.iter().any(|(n, _, _)| *n == name) {
            continue;
        }
        plan.push((name, key.clone(), extra));
    }
    plan
}

/// 注册对账：账本里有、本次计划没有的 provider → 应从引擎回收。
/// （凭据被删 / opencode auth.json 变更后，注册器只加不减就会留下旧 key。）
fn reconcile_removals(ledger: &[String], plan_names: &[&str]) -> Vec<String> {
    ledger
        .iter()
        .filter(|name| !plan_names.contains(&name.as_str()))
        .cloned()
        .collect()
}

/// 我们注册进引擎的 provider 账本（engine/registered-providers.json）。
/// 只记名字——对账用；回收只碰账本里的名字，绝不动用户在 magpie 里自加的。
fn load_registered_ledger(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn save_registered_ledger(path: &Path, names: &[String]) {
    if let Ok(text) = serde_json::to_string(names) {
        let temp = path.with_extension("json.tmp");
        if std::fs::write(&temp, text).and_then(|_| std::fs::rename(&temp, path)).is_err() {
            crate::debug_log::append(
                "engine",
                "warn",
                "",
                "engine provider ledger write failed",
                Some(&path.display().to_string()),
            );
        }
    }
}

/// 把「凭据库 key-plan + opencode 扫描引用的 api key」注册进引擎。
///
/// 2026-10-16 产品定稿「无脑打开软件就能用」：引用（opencode auth.json）的
/// key 只在注册瞬间读出、只进 magpie CLI 的命令行（除此之外扫描路径仍然
/// 绝不复制/存储 key，`scan_entry` 的 secret 恒为 None 不变）。幂等策略：
/// 先试 `provider key <id> <key>`（已存在 → 换 key），失败再 `provider add`
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
    let mut key_plans: Vec<(String, String)> = Vec::new();
    if let Ok(store) = crate::credentials::load_store_at(&credentials) {
        for entry in store
            .entries
            .iter()
            .filter(|entry| entry.kind == crate::credentials::CredentialKind::KeyPlan)
        {
            let Some(preset) = entry.preset.as_deref() else { continue };
            if magpie_provider_for_preset(preset).is_none() {
                continue;
            }
            match crate::credentials::peek_secret_at(&credentials, &entry.id) {
                Ok(Some(key)) => key_plans.push((preset.to_string(), key)),
                Ok(None) => {}
                Err(_) => {
                    crate::debug_log::append(
                        "engine",
                        "warn",
                        "",
                        "engine provider register: key undecryptable",
                        Some(&format!("entry={}", entry.id)),
                    );
                }
            }
        }
    }
    // 引用侧是 opencode 的原始 provider id（zhipuai-coding-plan / moonshot…），
    // 先归一化成 preset id —— registration_plan 只认 preset（同名巧合
    // deepseek/opencode-go 之外，原名直查会整体漏注册）。
    let references = crate::model_bridge::reference_keys_as_presets(
        crate::credentials::peek_opencode_api_keys(),
    );
    let plan = registration_plan(&key_plans, &references);
    let ledger_path = home.join(".marionette").join("engine").join("registered-providers.json");
    let ledger = load_registered_ledger(&ledger_path);
    let plan_names: Vec<&str> = plan.iter().map(|(name, _, _)| *name).collect();
    // 对账：账本里有、本次计划没有 → 引擎侧回收（只碰我们自己注册过的名字）。
    let mut kept_ledger: Vec<String> = ledger
        .iter()
        .filter(|name| plan_names.contains(&name.as_str()))
        .cloned()
        .collect();
    for name in reconcile_removals(&ledger, &plan_names) {
        match run_magpie_cli(exe, &["provider", "rm", name.as_str()]) {
            Ok(()) => {
                crate::debug_log::append(
                    "engine",
                    "info",
                    "",
                    "engine provider reclaimed",
                    Some(&name),
                );
            }
            Err(error) => {
                // 回收失败留在账本里，下次注册时机重试。
                crate::debug_log::append(
                    "engine",
                    "warn",
                    "",
                    "engine provider reclaim failed",
                    Some(&format!("provider={name}: {error}")),
                );
                kept_ledger.push(name);
            }
        }
    }
    if plan.is_empty() && kept_ledger.is_empty() && ledger.is_empty() {
        return;
    }
    for (provider_name, key, extra) in plan {
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
            Ok(()) => {
                crate::debug_log::append(
                    "engine",
                    "info",
                    "",
                    "engine provider registered",
                    Some(provider_name),
                );
                kept_ledger.push(provider_name.to_string());
            }
            Err(error) => crate::debug_log::append(
                "engine",
                "warn",
                "",
                "engine provider register failed",
                Some(&format!("provider={provider_name}: {error}")),
            ),
        }
    }
    save_registered_ledger(&ledger_path, &kept_ledger);
}

/// 凭据侧有变化（新增/删除 key-plan、opencode auth.json 更新）后的补注册。
/// 引擎的 enable 流水线只在拉起时注册一次；这个入口让已 Running 的自拉实例
/// 即时补上。复用实例（用户自装 magpie）照旧跳过——不碰别人的引擎状态。
pub fn reregister_key_providers() {
    if !matches!(
        status(),
        EngineStatus::Running { reused: false, .. }
    ) {
        return;
    }
    note_engine_needed();
    let Ok(dir) = engine_dir() else { return };
    let exe = exe_path(&dir);
    if !exe.is_file() {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("engine-reregister".into())
        .spawn(move || register_key_providers(&exe));
}

// ─── 订阅配额（`magpie quota --json` → 用量面板；30s 缓存）─────────────────

/// 一条订阅配额（序列化给前端；字段全部宽松——magpie 输出里缺什么都容忍）。
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EngineQuotaWindow {
    pub name: String,
    pub used: Option<f64>,
    pub remaining: Option<f64>,
    pub resets_at: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EngineQuotaEntry {
    pub provider: String,
    pub name: String,
    pub user: Option<String>,
    pub windows: Vec<EngineQuotaWindow>,
    /// key 余额（DeepSeek 等没有窗口、只有一笔钱）。
    pub balance: Option<String>,
}

fn quota_items(value: &serde_json::Value) -> Option<&Vec<serde_json::Value>> {
    value
        .as_array()
        .or_else(|| value.get("data").and_then(serde_json::Value::as_array))
}

fn json_f64(value: &serde_json::Value, key: &str) -> Option<f64> {
    let n = value.get(key)?;
    n.as_f64()
        .or_else(|| n.as_i64().map(|i| i as f64))
        .or_else(|| n.as_u64().map(|i| i as f64))
}

/// 解析 `magpie quota --json` 或 `GET /v1/magpie/quotas` 的输出
/// （CLI 是裸数组，网关是 `{object:"list",data:[…]}`；坏形状 → 空表，绝不 panic）。
fn parse_quota_json(text: &str) -> Vec<EngineQuotaEntry> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let Some(items) = quota_items(&value) else {
        return Vec::new();
    };
    let string = |v: &serde_json::Value, key: &str| {
        v.get(key).and_then(serde_json::Value::as_str).map(str::to_string)
    };
    let mut out = Vec::new();
    for item in items {
        let Some(provider) = string(item, "provider") else { continue };
        let windows = item
            .get("windows")
            .and_then(serde_json::Value::as_array)
            .map(|rows| {
                rows.iter()
                    .filter_map(|row| {
                        let name = string(row, "name")?;
                        Some(EngineQuotaWindow {
                            name,
                            used: json_f64(row, "used"),
                            remaining: json_f64(row, "remaining"),
                            resets_at: string(row, "resetsAt"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(EngineQuotaEntry {
            provider,
            name: string(item, "name").unwrap_or_default(),
            user: string(item, "user"),
            windows,
            balance: string(item, "balance"),
        });
    }
    out
}

static QUOTA_CACHE: OnceLock<Mutex<Option<(Instant, Vec<EngineQuotaEntry>)>>> = OnceLock::new();

fn quota_cache() -> &'static Mutex<Option<(Instant, Vec<EngineQuotaEntry>)>> {
    QUOTA_CACHE.get_or_init(|| Mutex::new(None))
}

fn fetch_quota_http(port: u16, force: bool) -> Result<String, String> {
    #[cfg(test)]
    if test_cli_capture_hook().is_some() {
        return Err("test seam uses CLI".into());
    }
    let agent = loopback_agent()?;
    let url = if force {
        format!("http://127.0.0.1:{port}/v1/magpie/quotas")
    } else {
        format!("http://127.0.0.1:{port}/v1/magpie/quotas?kept=1")
    };
    let timeout = if force {
        QUOTA_LIVE_TIMEOUT
    } else {
        LOOP_HTTP_TIMEOUT
    };
    let response = agent
        .get(&url)
        .set("Accept", "application/json")
        .set("Authorization", &format!("Bearer {ENGINE_TOKEN}"))
        .timeout(timeout)
        .call()
        .map_err(|e| format!("GET quotas: {e}"))?;
    if response.status() != 200 {
        return Err(format!("GET quotas HTTP {}", response.status()));
    }
    response
        .into_string()
        .map_err(|e| format!("read quotas: {e}"))
}

fn fetch_quota_cli() -> Result<String, String> {
    let dir = engine_dir()?;
    let exe = exe_path(&dir);
    if !exe.is_file() {
        return Err("magpie.exe 不在引擎目录".into());
    }
    run_magpie_cli_capture(&exe, &["quota", "--json"])
}

/// 拉订阅配额。自拉 / 复用实例都走正在跑的网关
/// `GET /v1/magpie/quotas`（`force` 时现场问供应商，否则 `?kept=1` 读缓存）。
/// HTTP 失败才退回 `magpie quota --json`。点刷新传 `force=true` 跳过 30s 缓存。
pub fn quota(force: bool) -> Vec<EngineQuotaEntry> {
    let EngineStatus::Running { port, .. } = status() else {
        return Vec::new();
    };
    if !force {
        if let Ok(guard) = quota_cache().lock() {
            if let Some((at, cached)) = guard.as_ref() {
                if at.elapsed() < QUOTA_CACHE_TTL {
                    return cached.clone();
                }
            }
        }
    }
    let (source, text) = match fetch_quota_http(port, force) {
        Ok(text) => ("http", text),
        Err(http_err) => match fetch_quota_cli() {
            Ok(text) => ("cli", text),
            Err(cli_err) => {
                crate::debug_log::append(
                    "engine",
                    "warn",
                    "",
                    "engine quota fetch failed",
                    Some(&format!("http={http_err}; cli={cli_err}")),
                );
                if let Ok(guard) = quota_cache().lock() {
                    if let Some((_, cached)) = guard.as_ref() {
                        return cached.clone();
                    }
                }
                return Vec::new();
            }
        },
    };
    let entries = parse_quota_json(&text);
    crate::debug_log::append(
        "engine",
        "info",
        "",
        &format!("engine quota {source} n={}", entries.len()),
        Some(
            &entries
                .iter()
                .map(|e| e.provider.as_str())
                .collect::<Vec<_>>()
                .join(","),
        ),
    );
    if let Ok(mut guard) = quota_cache().lock() {
        *guard = Some((Instant::now(), entries.clone()));
    }
    entries
}

// ─── 空闲回收（长时间无需求且无引擎绑定 → 停 serve；任何跨用入口自动唤醒）──
//
// 与「常开不可见」的折中：回收条件极保守 —— ①没有任何会话绑着引擎模型，
// ②30 分钟没人查目录 / 同步别名 / 解析注入 / 动过凭据。唤醒是全自动的：
// 模型菜单打开（已安装才唤醒，绝不惊喜下载）、绑定、网关按钮、下次启动。

/// 空闲多久后回收自拉 serve（无绑定 + 无需求）。
const IDLE_RECLAIM_AFTER: Duration = Duration::from_secs(30 * 60);
/// 回收巡检间隔。
const IDLE_CHECK_EVERY: Duration = Duration::from_secs(60);

/// 「有人需要引擎」——目录查询 / 别名同步 / 注入解析 / 注册时机都会打点。
pub fn note_engine_needed() {
    lock_state().last_needed_at = Some(Instant::now());
}

/// 绑定着引擎模型的会话数（只读 model_bridge 的存储文件——直接解析避免
/// engine ↔ model_bridge 循环依赖；形状见 BindingFile）。
fn engine_binding_count_at(path: &Path) -> usize {
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return 0;
    };
    value
        .get("bindings")
        .and_then(serde_json::Value::as_object)
        .map(|bindings| {
            bindings
                .values()
                .filter(|selection| {
                    selection.get("entryId").and_then(serde_json::Value::as_str)
                        == Some(crate::engine::ENGINE_ENTRY_ID)
                })
                .count()
        })
        .unwrap_or(0)
}

fn engine_binding_count() -> usize {
    let Some(home) = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
    else {
        return 0;
    };
    engine_binding_count_at(&home.join(".marionette").join("bridge-bindings.json"))
}

/// 引擎已安装但处于 Absent（典型：空闲回收后）→ 后台唤醒。幂等；未安装
/// 不动（绝不因菜单打开触发 48MB 下载）。
pub fn wake_if_installed() {
    #[cfg(test)]
    {
        // 测试构建：绝不在单测里真拉引擎（enable 会触碰真实 ~/.marionette）。
        return;
    }
    #[cfg(not(test))]
    {
        if !matches!(status(), EngineStatus::Absent) {
            return;
        }
        let Ok(dir) = engine_dir() else { return };
        if !is_installed_at(&dir) {
            return;
        }
        let _ = enable();
    }
}

/// 菜单级短暂唤醒：已安装才同步等待（未安装立即 false，绝不触发下载）。
pub fn ensure_bridge_ready_if_installed(timeout: Duration) -> bool {
    if bridge_ready() {
        return true;
    }
    #[cfg(test)]
    {
        let _ = timeout;
        return false;
    }
    #[cfg(not(test))]
    {
        let installed = engine_dir().map(|dir| is_installed_at(&dir)).unwrap_or(false);
        installed && ensure_bridge_ready(timeout)
    }
}

/// 手动停止（网关浮层的「停止网关」，2026-10-17）：只停**我们拉起的**
/// serve——复用实例不归 Marionette 管（warn 留痕、状态不动）。绑定不清：
/// 引擎再起（按钮/菜单/启动）时一切照旧；正在用跨用模型的会话会随引擎
/// 一起断流，这是用户显式停止的预期语义。
pub fn stop() {
    let mut state = lock_state();
    match state.status {
        EngineStatus::Running { reused: true, .. } => {
            drop(state);
            crate::debug_log::append(
                "engine",
                "warn",
                "",
                "engine stop skipped: reused user install",
                None,
            );
            return;
        }
        EngineStatus::Running { .. } => {}
        _ => return,
    }
    if let Some(child) = state.child.as_mut() {
        child.kill();
    }
    state.child = None;
    state.status = EngineStatus::Absent;
    drop(state);
    if let Some(cache) = CATALOG.get() {
        if let Ok(mut guard) = cache.lock() {
            *guard = None;
        }
    }
    crate::debug_log::append("engine", "info", "", "engine stopped by user", None);
    emit_event("engine-state-changed", &EngineStatus::Absent);
}

/// 引擎更新检查结果（浮层「检查更新」）。
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EngineUpdateInfo {
    pub current_version: Option<String>,
    pub embedded_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_available: bool,
    /// 有内嵌种子，且与本机当前版本不同（或尚未安装）时可恢复出厂。
    pub can_restore_factory: bool,
    pub note: Option<String>,
}

fn installed_version_at(dir: &Path) -> Option<String> {
    load_manifest_at(dir).map(|manifest| manifest.version)
}

fn clear_catalog_cache() {
    if let Some(cache) = CATALOG.get() {
        if let Ok(mut guard) = cache.lock() {
            *guard = None;
        }
    }
}

/// 停掉我们拉起的 serve，好替换磁盘上的 magpie.exe。复用实例 / 安装中拒绝。
fn stop_owned_for_replace() -> Result<(), String> {
    let mut state = lock_state();
    match &state.status {
        EngineStatus::Running { reused: true, .. } => {
            return Err(
                "当前复用本机自装的 magpie，无法替换。请先关掉它，再更新本应用引擎。"
                    .to_string(),
            );
        }
        EngineStatus::Downloading { .. } | EngineStatus::Starting => {
            return Err("引擎正在安装或启动，请稍后再试".to_string());
        }
        EngineStatus::Running { .. } => {
            if let Some(child) = state.child.as_mut() {
                child.kill();
            }
            state.child = None;
            state.status = EngineStatus::Absent;
            drop(state);
            clear_catalog_cache();
            emit_event("engine-state-changed", &EngineStatus::Absent);
            crate::debug_log::append(
                "engine",
                "info",
                "",
                "engine stopped for binary replace",
                None,
            );
            Ok(())
        }
        EngineStatus::Absent | EngineStatus::Failed { .. } => Ok(()),
    }
}

/// 查 GitHub latest（稳定版）与本机 / 出厂版本对比。网络失败时仍返回本机信息，`note` 带原因。
pub fn check_update() -> EngineUpdateInfo {
    let dir = engine_dir().ok();
    let current_version = dir.as_ref().and_then(|d| installed_version_at(d));
    let embedded = embedded_version().map(|v| v.to_string());
    let can_restore_factory = match (&embedded, &current_version) {
        (Some(seed), Some(cur)) => seed != cur,
        (Some(_), None) => true,
        (None, _) => false,
    };
    match resolve_latest_release() {
        Ok(plan) => {
            let latest_version = plan.version;
            let update_available = match &current_version {
                Some(cur) => crate::agent_update::is_newer(&latest_version, cur),
                None => true,
            };
            EngineUpdateInfo {
                current_version,
                embedded_version: embedded,
                latest_version: Some(latest_version),
                update_available,
                can_restore_factory,
                note: None,
            }
        }
        Err(error) => EngineUpdateInfo {
            current_version,
            embedded_version: embedded,
            latest_version: None,
            update_available: false,
            can_restore_factory,
            note: Some(error),
        },
    }
}

/// 显式更新：停自拉实例 → 下 latest 稳定 CLI → 落位为新的本机权威 → 再拉起。
/// 后台线程执行；返回 kick-off 快照，UI 跟 `engine-download-progress` / `engine-state-changed`。
pub fn update() -> Result<EngineStatus, String> {
    stop_owned_for_replace()?;
    let dir = engine_dir()?;
    let initial = {
        let mut state = lock_state();
        match state.status {
            EngineStatus::Downloading { .. } | EngineStatus::Starting => {
                return Err("引擎正在安装或启动，请稍后再试".to_string());
            }
            _ => {
                let initial = EngineStatus::Downloading {
                    downloaded: 0,
                    total: 0,
                };
                state.status = initial.clone();
                initial
            }
        }
    };
    emit_event("engine-state-changed", &initial);
    std::thread::Builder::new()
        .name("engine-update".into())
        .spawn(move || {
            if let Err(error) = update_pipeline(&dir) {
                set_status(EngineStatus::Failed { error });
            }
        })
        .map_err(|e| format!("启动引擎更新线程失败: {e}"))?;
    Ok(initial)
}

fn update_pipeline(dir: &Path) -> Result<(), String> {
    let plan = resolve_latest_release()?;
    download_and_install(&plan, dir)?;
    crate::debug_log::append(
        "engine",
        "info",
        "",
        &format!("engine updated to local authority version={}", plan.version),
        None,
    );
    enable_pipeline(dir)
}

/// 恢复出厂：用 exe 内嵌种子覆盖本机引擎，再拉起。无内嵌构建不可用。
pub fn restore_factory() -> Result<EngineStatus, String> {
    let (bytes, version) = embedded_artifact();
    if bytes.is_empty() || version.is_empty() {
        return Err("当前构建没有内嵌引擎，无法恢复出厂".to_string());
    }
    let version = version.to_string();
    stop_owned_for_replace()?;
    let dir = engine_dir()?;
    let initial = {
        let mut state = lock_state();
        match state.status {
            EngineStatus::Downloading { .. } | EngineStatus::Starting => {
                return Err("引擎正在安装或启动，请稍后再试".to_string());
            }
            _ => {
                let initial = EngineStatus::Starting;
                state.status = initial.clone();
                initial
            }
        }
    };
    emit_event("engine-state-changed", &initial);
    std::thread::Builder::new()
        .name("engine-restore".into())
        .spawn(move || {
            if let Err(error) = restore_factory_pipeline(bytes, &version, &dir) {
                set_status(EngineStatus::Failed { error });
            }
        })
        .map_err(|e| format!("启动引擎恢复线程失败: {e}"))?;
    Ok(initial)
}

fn restore_factory_pipeline(bytes: &[u8], version: &str, dir: &Path) -> Result<(), String> {
    install_embedded_engine_at(bytes, version, dir)?;
    crate::debug_log::append(
        "engine",
        "info",
        "",
        &format!("engine restored to factory seed version={version}"),
        None,
    );
    enable_pipeline(dir)
}

/// 等引擎就绪（跨用绑定的一键优雅路径，2026-10-17 用户反馈）：就绪立即真；
/// 否则幂等 `enable` 后在窗口内轮询——已装引擎通常 2-5s（内嵌版首启含解压），
/// 超时返回 false 由调用方走原有指引错误。
pub fn ensure_bridge_ready(timeout: Duration) -> bool {
    if bridge_ready() {
        return true;
    }
    #[cfg(test)]
    {
        // 测试构建：seam 没摆出 Running 就直接失败——绝不在单测里真拉引擎
        //（enable 会触碰真实 ~/.marionette）。要测等待行为就先摆 seam。
        let _ = timeout;
        return false;
    }
    #[cfg(not(test))]
    {
        let _ = enable();
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if bridge_ready() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        false
    }
}

/// 回收巡检线程：Running（自拉）期间每分钟看一眼；不满足回收条件就继续，
/// 状态不再是自拉 Running 就退出。在 enable_pipeline 成功后启动。
fn idle_watchdog() {
    loop {
        std::thread::sleep(IDLE_CHECK_EVERY);
        let mut state = lock_state();
        if !matches!(state.status, EngineStatus::Running { reused: false, .. }) {
            return;
        }
        let idle_for = state
            .last_needed_at
            .map(|at| at.elapsed())
            .unwrap_or(Duration::ZERO);
        if idle_for < IDLE_RECLAIM_AFTER || engine_binding_count() > 0 {
            continue;
        }
        if let Some(child) = state.child.as_mut() {
            child.kill();
        }
        state.child = None;
        state.status = EngineStatus::Absent;
        drop(state);
        crate::debug_log::append(
            "engine",
            "info",
            "",
            &format!("engine idle-reclaimed after {}min idle", IDLE_RECLAIM_AFTER.as_secs() / 60),
            Some("bindings=0; any cross-use entry auto-wakes it"),
        );
        emit_event("engine-state-changed", &EngineStatus::Absent);
        return;
    }
}

/// 跑一个短命 magpie CLI 命令并捕获 stdout（quota 用；测试缝同 run_magpie_cli）。
fn run_magpie_cli_capture(exe: &Path, args: &[&str]) -> Result<String, String> {
    #[cfg(test)]
    if let Some(hook) = test_cli_capture_hook() {
        return hook(args);
    }
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
        .map_err(|e| format!("启动 magpie CLI 失败: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "magpie {} 退出码 {:?}",
            args.first().unwrap_or(&""),
            output.status.code()
        ));
    }
    String::from_utf8(output.stdout).map_err(|e| format!("magpie 输出不是 UTF-8: {e}"))
}

/// 跑一个短命 magpie CLI 命令（.portable 围栏由 exe 位置决定，无需 env）。
/// 测试构建下先查 CLI 缝（别名组测试记录参数用），有缝就不真的拉子进程。
fn run_magpie_cli(exe: &Path, args: &[&str]) -> Result<(), String> {
    #[cfg(test)]
    if let Some(hook) = test_cli_hook() {
        return hook(args);
    }
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

// ─── codex 内建名 → 引擎模型的路由组别名（M2 收官）────────────────────────
//
// codex 的 `providers/set` 只换端点（Responses 协议），模型 id 必须保持
// codex 内建名（gpt-5.2-codex…）——引擎的 `provider/model` id 进不去。magpie
// 的路由组补上这一环（2026-10-16 源码核实，internal/provider/group.go）：
// `GroupFor` 对无斜杠的裸名先按组 id 原文小写命中，组 id 即 codex 内建名；
// `pick=` 同时把 routing 钉成 manual，全部流量走那一个成员。`group add` 按
// id 幂等 upsert（同名 id 整组替换），所以会话启动 / 切模型时重跑一遍即可。
// 组名空间全局共享：两个 codex 会话绑同一个内建名时后写者胜（记 warn），
// 旧组只在没有任何会话引用时删除。

/// Marionette 所建路由组的名字前缀（magpie Routing 视图里的可读标识）。
const CODEX_GROUP_PREFIX: &str = "Marionette ";

/// 每个绑了引擎的 codex 会话当前持有的别名：session_id → (组 id, 引擎模型)。
static CODEX_ALIASES: OnceLock<Mutex<std::collections::HashMap<String, (String, String)>>> =
    OnceLock::new();

fn alias_map() -> &'static Mutex<std::collections::HashMap<String, (String, String)>> {
    CODEX_ALIASES.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

/// 组 id 规则对齐 magpie CLI 的校验（小写字母 / 数字 / 点 / 横线）；codex
/// 内建名本身全小写，这里只做防御性归一，不合法的名字直接拒（留痕）。
fn codex_group_id(builtin: &str) -> Option<String> {
    let id = builtin.trim().to_lowercase();
    if id.is_empty() {
        return None;
    }
    id.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '-')
        .then_some(id)
}

/// 别名组（以及一切 CLI 状态写入）只对**我们自己拉起**的 serve 有效：
/// reused 实例的状态不在我们的 `engine/data/` 里，CLI 写进去它也读不到。
/// Running 且非 reused → 我们那份 magpie.exe 的路径。
fn owned_serve_exe() -> Option<PathBuf> {
    if !matches!(status(), EngineStatus::Running { reused: false, .. }) {
        return None;
    }
    engine_dir().ok().map(|dir| exe_path(&dir))
}

/// 把 codex 会话当前的内建模型名别名到所选引擎模型。
///
/// - 内容有变 → `group add "Marionette <名>" models=<m> id=<内建名> pick=<m>`；
/// - 会话之前别名过别的内建名（codex 原生切模型）→ 旧组无人引用时 `group rm`；
/// - 引擎没在自拉运行 / 复用实例 → 留痕跳过（绑定侧已保证前者在 spawn 时
///   不发生；复用实例是支持态，用户可在自己的 magpie 里手工建组）。
///
/// CLI 失败一律只记 warn：流量照进引擎，未别名的内建名会在引擎侧 404，
/// dev.log 里有原因可查。
pub fn codex_alias_sync(session_id: &str, builtin: &str, engine_model: &str) {
    note_engine_needed();
    let Some(group_id) = codex_group_id(builtin) else {
        crate::debug_log::append(
            "engine",
            "warn",
            session_id,
            "codex alias skipped: built-in name is not a valid group id",
            Some(builtin),
        );
        return;
    };
    // 幂等快速路径：同会话、同内建名、同引擎模型 → 不再拉 CLI。
    {
        let map = alias_map().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if map
            .get(session_id)
            .is_some_and(|(id, model)| id == &group_id && model == engine_model)
        {
            return;
        }
    }
    let Some(exe) = owned_serve_exe() else {
        let reused = matches!(status(), EngineStatus::Running { reused: true, .. });
        crate::debug_log::append(
            "engine",
            if reused { "warn" } else { "info" },
            session_id,
            if reused {
                "codex alias skipped: engine is a reused user install (create the group in your own magpie if needed)"
            } else {
                "codex alias skipped: engine not running"
            },
            None,
        );
        return;
    };
    let previous = alias_map()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(session_id)
        .cloned();
    let name = format!("{CODEX_GROUP_PREFIX}{group_id}");
    let result = run_magpie_cli(
        &exe,
        &[
            "group",
            "add",
            &name,
            &format!("models={engine_model}"),
            &format!("id={group_id}"),
            &format!("pick={engine_model}"),
        ],
    );
    match result {
        Ok(()) => {
            let mut map = alias_map().lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            map.insert(
                session_id.to_string(),
                (group_id.clone(), engine_model.to_string()),
            );
            if map.values().filter(|(id, _)| id == &group_id).count() > 1 {
                crate::debug_log::append(
                    "engine",
                    "warn",
                    session_id,
                    "codex alias group shared by multiple sessions; last bind wins",
                    Some(&group_id),
                );
            }
            crate::debug_log::append(
                "engine",
                "info",
                session_id,
                "codex alias group updated",
                Some(&format!("group={group_id} model={engine_model}")),
            );
        }
        Err(error) => crate::debug_log::append(
            "engine",
            "warn",
            session_id,
            "codex alias group add failed",
            Some(&error),
        ),
    }
    // 旧别名清理：别的会话还在用就不删（共享名空间）。
    if let Some((old_id, _)) = previous {
        if old_id != group_id {
            drop_codex_alias_group(&old_id);
        }
    }
}

/// 会话解绑 / 会话侧清理入口：移除该会话登记的别名，组在无人引用时删掉。
pub fn codex_alias_clear(session_id: &str) {
    let previous = alias_map()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(session_id);
    let Some((old_id, _)) = previous else {
        return;
    };
    drop_codex_alias_group(&old_id);
}

/// 删一个别名组（先查引用计数；引擎侧删不掉只留痕）。
fn drop_codex_alias_group(group_id: &str) {
    let still_used = alias_map()
        .lock()
        .map(|map| map.values().any(|(id, _)| id == group_id))
        .unwrap_or(true); // 锁坏了宁可漏删也不误删
    if still_used {
        return;
    }
    let Some(exe) = owned_serve_exe() else {
        return;
    };
    match run_magpie_cli(&exe, &["group", "rm", group_id]) {
        Ok(()) => crate::debug_log::append(
            "engine",
            "info",
            "",
            "codex alias group removed",
            Some(group_id),
        ),
        Err(error) => crate::debug_log::append(
            "engine",
            "warn",
            "",
            "codex alias group remove failed",
            Some(&format!("group={group_id}: {error}")),
        ),
    }
}

// ─── enable 流水线（后台线程）───────────────────────────────────────────────

/// 幂等启用：Running / Downloading / Starting → 直接返回当前态；否则登记
/// 初始态并在后台线程跑完「（复用探测 →）内嵌落位或下载 → serve → 健康 →
/// provider 注册 → Running」。失败落 `Failed{error}`（事件 + 日志留档）。
///
/// 便携版（`embedded-engine`）走 Starting：字节已在 exe 里，打开即拉起，
/// 不经过下载态、也不打 GitHub。无内嵌的开发构建仍走 Downloading。
pub fn enable() -> Result<EngineStatus, String> {
    let dir = engine_dir()?;
    let claimed = {
        let mut state = lock_state();
        match state.status {
            EngineStatus::Running { .. }
            | EngineStatus::Downloading { .. }
            | EngineStatus::Starting => None,
            EngineStatus::Absent | EngineStatus::Failed { .. } => {
                let initial = if embedded_version().is_some() {
                    EngineStatus::Starting
                } else {
                    EngineStatus::Downloading {
                        downloaded: 0,
                        total: 0,
                    }
                };
                state.status = initial.clone();
                Some(initial)
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
    // 2. 安装：本机权威在 `engine/`。仅当尚未完整安装时用内嵌种子离线落位；
    //    用户已更新过的版本粘住。便携版种子落位失败即停（不偷偷上网）。
    //    无内嵌的开发构建才走 GitHub 下载。
    if needs_embedded_seed(dir) {
        let (bytes, version) = embedded_artifact();
        install_embedded_engine_at(bytes, version, dir)?;
    }
    if !is_installed_at(dir) {
        if embedded_version().is_some() {
            return Err("内嵌引擎落位后仍不完整".to_string());
        }
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
    note_engine_needed();
    set_status(EngineStatus::Running {
        port,
        version,
        reused: false,
    });
    // 5. 空闲回收巡检（无绑定 + 30 分钟无需求 → 停 serve；见 idle_watchdog）。
    let _ = std::thread::Builder::new()
        .name("engine-idle-watchdog".into())
        .spawn(idle_watchdog);
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
    // 注入解析 = 有会话真要用引擎 → 空闲回收的打点（测试构建除外：测试态
    // 没有 watchdog，打了也无妨，但保持纯净）。
    let base = running_base();
    #[cfg(not(test))]
    if base.is_some() {
        note_engine_needed();
    }
    base.map(|(port, _)| format!("http://127.0.0.1:{port}/v1"))
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
    models: Vec<crate::model_bridge::CatalogModelRow>,
}

static CATALOG: OnceLock<Mutex<Option<CatalogEntry>>> = OnceLock::new();

/// 引擎 `/v1/models` 全量（带 TTL 缓存，含每模型窗口/输出上限）。
/// 引擎不在 Running → 空。
pub fn catalog_entries() -> Vec<crate::model_bridge::CatalogModelRow> {
    #[cfg(test)]
    if let Some(models) = test_catalog() {
        return models
            .into_iter()
            .map(|id| crate::model_bridge::CatalogModelRow {
                id,
                context_window: 0,
                max_output: 0,
                images: false,
            })
            .collect();
    }
    note_engine_needed();
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

/// 引擎 `/v1/models` 全量（带 TTL 缓存），只要 id。引擎不在 Running → 空。
pub fn catalog() -> Vec<String> {
    catalog_entries()
        .into_iter()
        .map(|row| row.id)
        .collect()
}

/// 面向用户/agent 的引擎目录：剥掉 `group/` 自动路由组（magpie 的跨来源
/// 路由入口，目录里自标 "routing group"）。2026-10-20 产品决定：菜单只列
/// 具体来源的模型，人直接选 `grok/grok-4.7` 这类；路由组是给程序用的抽象，
/// 进菜单只有困惑（「auto-grok-4-7 完全不懂」）。想要时仍可用自定义模型
/// 输入 `group/…` id。桥的所有消费点（跨用菜单 / opencode 配置 / dsh 路由 /
/// pi models.json）统一走这里。
pub fn catalog_direct() -> Vec<String> {
    catalog()
        .into_iter()
        .filter(|id| !id.starts_with("group/"))
        .collect()
}

/// 同 `catalog_direct`，但带窗口/输出上限——dsh 桥写 `contextWindow`/
/// `maxTokens` 用（llm-pi-ai 对缺声明的模型落 256K 兜底，见
/// model_bridge::CatalogModelRow）。
pub fn catalog_direct_entries() -> Vec<crate::model_bridge::CatalogModelRow> {
    catalog_entries()
        .into_iter()
        .filter(|row| !row.id.starts_with("group/"))
        .collect()
}

fn fetch_catalog() -> Vec<crate::model_bridge::CatalogModelRow> {
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
    crate::model_bridge::parse_models_catalog_rows(&body)
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
pub(crate) type BoxCli = std::sync::Arc<dyn Fn(&[&str]) -> Result<(), String> + Send + Sync>;
#[cfg(test)]
pub(crate) type BoxCliCapture =
    std::sync::Arc<dyn Fn(&[&str]) -> Result<String, String> + Send + Sync>;

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
    /// magpie CLI 捕获缝（run_magpie_cli 的替代；别名组测试记录参数用）。
    cli: Option<BoxCli>,
    /// 带输出捕获的 CLI 缝（quota 用）。
    cli_capture: Option<BoxCliCapture>,
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
            cli: None,
            cli_capture: None,
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
        if let Some(cache) = QUOTA_CACHE.get() {
            if let Ok(mut guard) = cache.lock() {
                *guard = None;
            }
        }
        if let Some(map) = CODEX_ALIASES.get() {
            if let Ok(mut guard) = map.lock() {
                guard.clear();
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
    install_test_running(Seams {
        catalog: Some(catalog),
        bridge_visible: true,
        ..Seams::default()
    })
}

/// [`test_running`] + CLI 捕获缝：别名组接线测试记录 magpie CLI 参数用。
#[cfg(test)]
pub(crate) fn test_running_with_cli(catalog: Vec<String>, cli: BoxCli) -> TestHandle {
    install_test_running(Seams {
        catalog: Some(catalog),
        bridge_visible: true,
        cli: Some(cli),
        ..Seams::default()
    })
}

/// test_running* 的共同落地：装缝 + 把状态摆成自拉 Running。
#[cfg(test)]
fn install_test_running(seams: Seams) -> TestHandle {
    let handle = install_test_seams(seams);
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
fn test_cli_hook() -> Option<BoxCli> {
    seams().and_then(|guard| guard.as_ref().and_then(|s| s.cli.clone()))
}

#[cfg(test)]
fn test_cli_capture_hook() -> Option<BoxCliCapture> {
    seams().and_then(|guard| guard.as_ref().and_then(|s| s.cli_capture.clone()))
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

    /// 内嵌引擎落位安装：字节原样写盘 + `.portable` + manifest 记真实 sha
    /// （与下载路径共用 `is_installed_at` 的校验语义）；篡改即失效。
    #[test]
    fn embedded_install_roundtrips_and_verifies() {
        let dir = temp_dir("embedded");
        let payload = b"fake embedded magpie exe payload 0123456789";

        install_embedded_engine_at(payload, "0.1.9999", &dir).unwrap();
        assert_eq!(fs::read(exe_path(&dir)).unwrap(), payload);
        assert!(dir.join(".portable").exists(), "围栏标记必须一并落位");
        let manifest = load_manifest_at(&dir).unwrap();
        assert_eq!(manifest.version, "0.1.9999");
        assert_eq!(manifest.sha256, sha256_hex(payload));
        assert!(is_installed_at(&dir), "sha 对上 → 已安装（与下载路径同语义）");

        // 种子判定：已完整安装 → 不重落位（即使用户版版本号 ≠ 内嵌）。
        assert!(
            !needs_embedded_seed(&dir),
            "已装好的本机引擎是权威，内嵌不得盖回"
        );

        // 篡改一个字节 → 校验失败 → 需要种子（有内嵌时）或下载路径。
        let mut tampered = payload.to_vec();
        tampered[0] ^= 0xff;
        fs::write(exe_path(&dir), tampered).unwrap();
        assert!(!is_installed_at(&dir));
        #[cfg(feature = "embedded-engine")]
        assert!(needs_embedded_seed(&dir), "损坏后才允许内嵌种子重落位");
        #[cfg(not(feature = "embedded-engine"))]
        assert!(!needs_embedded_seed(&dir), "无内嵌构建不走种子");

        fs::remove_dir_all(&dir).ok();
    }

    /// 默认构建不带内嵌引擎：解压分支永不触发，下载路径保持原样。
    #[cfg(not(feature = "embedded-engine"))]
    #[test]
    fn default_build_has_no_embedded_engine() {
        assert_eq!(embedded_version(), None);
        let dir = temp_dir("noembed");
        assert!(!needs_embedded_seed(&dir), "无内嵌构建恒不触发解压");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn needs_embedded_seed_only_when_missing_or_corrupt() {
        let dir = temp_dir("seed-policy");
        #[cfg(feature = "embedded-engine")]
        assert!(needs_embedded_seed(&dir), "空目录需要种子");
        #[cfg(not(feature = "embedded-engine"))]
        assert!(!needs_embedded_seed(&dir));

        let payload = b"sticky user-updated magpie binary";
        install_embedded_engine_at(payload, "9.9.9", &dir).unwrap();
        assert!(!needs_embedded_seed(&dir), "用户版粘住");
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

    /// 注册计划：key-plan 优先于同 provider 的扫描引用；未映射的引用跳过；
    /// glm 携带 region=coding 附加参数。
    #[test]
    fn registration_plan_prefers_key_plans_and_skips_unmapped() {
        let plan = registration_plan(
            &[("opencode-go".to_string(), "store-key".to_string())],
            &[
                ("opencode-go".to_string(), "ref-key".to_string()), // 同名：引用让位
                ("deepseek".to_string(), "ds-key".to_string()),     // 独立引用：保留
                ("anthropic".to_string(), "oauth-key".to_string()), // OAuth 引用：不注册
            ],
        );
        let names: Vec<&str> = plan.iter().map(|(n, _, _)| *n).collect();
        assert_eq!(names, vec!["opencode-go", "deepseek"]);
        assert_eq!(plan[0].1, "store-key");
        assert_eq!(plan[1].1, "ds-key");

        let glm = registration_plan(
            &[("glm".to_string(), "g".to_string())],
            &[],
        );
        assert_eq!(glm[0].0, "zhipu");
        assert_eq!(glm[0].2, "region=coding");
    }

    /// 注册对账：账本减去本次计划 = 引擎侧应回收的名字；计划里的不动。
    #[test]
    fn reconcile_removals_only_reclaims_ledger_names_missing_from_the_plan() {
        let ledger = vec![
            "opencode-go".to_string(),
            "deepseek".to_string(),
            "moonshot".to_string(),
        ];
        let plan = vec!["opencode-go", "xai"];
        assert_eq!(reconcile_removals(&ledger, &plan), vec!["deepseek", "moonshot"]);
        // 空计划 → 全回收（用户删光了 key）。
        assert_eq!(reconcile_removals(&ledger, &[]), ledger);
        // 空账本 → 无事可做。
        assert!(reconcile_removals(&[], &plan).is_empty());
    }

    /// 账本读写 roundtrip；损坏文件按空账本处理（绝不动引擎状态）。
    #[test]
    fn registered_ledger_roundtrips_and_tolerates_corruption() {
        let dir = temp_dir("ledger");
        let path = dir.join("registered-providers.json");
        assert!(load_registered_ledger(&path).is_empty(), "缺文件 → 空账本");
        save_registered_ledger(&path, &["opencode-go".to_string(), "deepseek".to_string()]);
        assert_eq!(
            load_registered_ledger(&path),
            vec!["opencode-go".to_string(), "deepseek".to_string()]
        );
        std::fs::write(&path, "{not json").unwrap();
        assert!(load_registered_ledger(&path).is_empty(), "坏文件 → 空账本");
        assert!(!path.with_extension("json.tmp").exists(), "临时文件已 rename 走");
        fs::remove_dir_all(&dir).ok();
    }

    /// `magpie quota --json` 解析：实机形状（2026-10-17 实测）→ 条目与窗口；
    /// 缺字段容忍、坏形状 → 空表。
    #[test]
    fn quota_json_parses_the_measured_shape_and_tolerates_junk() {
        let json = r#"[
  {"provider":"grok","name":"Grok (SuperGrok)","kind":"subscription",
   "user":"ntcubic@yahoo.com",
   "windows":[{"name":"7 days","used":68,"remaining":32,
               "resetsAt":"2026-10-17T12:09:03.3069Z","holdsWhy":"log"}],
   "readAt":"2026-10-17T00:10:27.7158835+09:00"},
  {"provider":"codex","name":"Codex","plan":"free",
   "windows":[{"name":"30 days","used":0,"remaining":100}]}
]"#;
        let entries = parse_quota_json(json);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].provider, "grok");
        assert_eq!(entries[0].user.as_deref(), Some("ntcubic@yahoo.com"));
        assert_eq!(entries[0].windows[0].used, Some(68.0));
        assert_eq!(entries[0].windows[0].remaining, Some(32.0));
        assert!(entries[0].windows[0].resets_at.is_some());
        // 第二条无 user、无 resetsAt —— 全部容忍。
        assert_eq!(entries[1].user, None);
        assert_eq!(entries[1].windows[0].resets_at, None);

        assert!(parse_quota_json("not json").is_empty());
        assert!(parse_quota_json("{}").is_empty());
        assert!(parse_quota_json(r#"[{"windows":[]}]"#).is_empty(), "无 provider 的条目跳过");

        // 网关 GET /v1/magpie/quotas 包一层 {object,data}（2026-10-18 实测）。
        let wrapped = r#"{"object":"list","data":[
          {"provider":"grok","name":"Grok (SuperGrok)","kind":"subscription",
           "user":"ntcubic@yahoo.com",
           "windows":[{"name":"7 days","used":0,"remaining":100,
                       "resetsAt":"2026-10-24T12:09:03.306Z"}]},
          {"provider":"deepseek","name":"DeepSeek","kind":"balance","balance":"12.50",
           "windows":[]}
        ]}"#;
        let wrapped_entries = parse_quota_json(wrapped);
        assert_eq!(wrapped_entries.len(), 2);
        assert_eq!(wrapped_entries[0].windows[0].used, Some(0.0));
        assert_eq!(wrapped_entries[1].balance.as_deref(), Some("12.50"));
        assert!(wrapped_entries[1].windows.is_empty());
    }

    /// 引擎绑定计数（空闲回收的「有人在用」判据）：只数 entryId=engine 的；
    /// 坏文件/缺文件按 0（宁可不回收也不误杀）。
    #[test]
    fn engine_binding_count_reads_only_engine_selections() {
        let dir = temp_dir("bindcount");
        let path = dir.join("bridge-bindings.json");
        assert_eq!(engine_binding_count_at(&path), 0, "缺文件 → 0");

        std::fs::write(
            &path,
            r#"{"version":1,"bindings":{
                 "s1":{"entryId":"engine","modelId":"grok/grok-4.7"},
                 "s2":{"entryId":"engine","modelId":"opencode-go/glm-5.3"},
                 "s3":{"entryId":"opencode-go","modelId":"glm-5.3"},
                 "s4":{"entryId":"opencode/deepseek","modelId":"deepseek-v4-pro"}
               }}"#,
        )
        .unwrap();
        assert_eq!(engine_binding_count_at(&path), 2);

        std::fs::write(&path, "{corrupt").unwrap();
        assert_eq!(engine_binding_count_at(&path), 0, "坏文件 → 0（不误判有人在用）");

        fs::remove_dir_all(&dir).ok();
    }

    /// 手动停止：自拉 Running → Absent；非 Running 时幂等 no-op。
    #[test]
    fn manual_stop_clears_running_state() {
        let _engine = crate::engine::test_running(vec!["grok/grok-4.7".to_string()]);
        assert!(matches!(crate::engine::status(), EngineStatus::Running { .. }));
        stop();
        assert!(matches!(crate::engine::status(), EngineStatus::Absent));
        stop();
        assert!(matches!(crate::engine::status(), EngineStatus::Absent));
    }

    // ─── codex 内建名 → 路由组别名（M2 收官；CLI 由测试缝记录）──────────────

    /// CLI 录制器：每次 magpie CLI 调用的参数 + 一个可选的应答注入。
    struct CliRecorder {
        calls: std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>>,
        _engine: TestHandle,
    }

    impl CliRecorder {
        /// 引擎摆成自拉 Running + CLI 缝记录所有调用（默认全部成功）。
        fn install(catalog: Vec<String>) -> Self {
            let calls: std::sync::Arc<std::sync::Mutex<Vec<Vec<String>>>> =
                std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let recorder = calls.clone();
            let engine = test_running_with_cli(
                catalog,
                std::sync::Arc::new(move |args| {
                    recorder
                        .lock()
                        .unwrap()
                        .push(args.iter().map(|s| s.to_string()).collect());
                    Ok(())
                }),
            );
            Self { calls, _engine: engine }
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    /// 组处方逐字段锚定（magpie groups_cli.go 2026-10-16 核实）：
    /// `group add "Marionette <内建名>" models=<引擎模型> id=<内建名> pick=<引擎模型>`
    /// —— id 命中 GroupFor 的裸名首查，pick 自带 manual 路由；幂等重放零 CLI。
    #[test]
    fn codex_alias_group_add_uses_the_prescribed_shape() {
        let cli = CliRecorder::install(Vec::new());
        codex_alias_sync("s1", "gpt-5.2-codex", "grok/grok-4.7");
        assert_eq!(
            cli.calls(),
            vec![vec![
                "group",
                "add",
                "Marionette gpt-5.2-codex",
                "models=grok/grok-4.7",
                "id=gpt-5.2-codex",
                "pick=grok/grok-4.7",
            ]]
        );
        // 同会话同内建名同模型：快速路径，不再拉 CLI。
        codex_alias_sync("s1", "gpt-5.2-codex", "grok/grok-4.7");
        assert_eq!(cli.calls().len(), 1);
        // 同名不同引擎模型（换绑）：重跑 add（pick 换人）。
        codex_alias_sync("s1", "gpt-5.2-codex", "kimi/k2.6");
        assert_eq!(cli.calls().len(), 2);
    }

    /// codex 原生切模型：新内建名建组，旧组无人引用即删（模型名不再 404）。
    #[test]
    fn codex_alias_switch_removes_the_old_group() {
        let cli = CliRecorder::install(Vec::new());
        codex_alias_sync("s1", "gpt-5.2-codex", "grok/grok-4.7");
        codex_alias_sync("s1", "gpt-5.1-codex", "grok/grok-4.7");
        let calls = cli.calls();
        assert_eq!(calls[0][..2], ["group".to_string(), "add".to_string()]);
        assert_eq!(calls[1][..2], ["group".to_string(), "add".to_string()]);
        assert_eq!(
            calls[2],
            vec!["group".to_string(), "rm".to_string(), "gpt-5.2-codex".to_string()]
        );
    }

    /// 共享名空间：两个会话别名同一个内建名 → 先解绑的保留组（后者还在
    /// 用），最后一个解绑才删。
    #[test]
    fn codex_alias_clear_respects_shared_groups() {
        let cli = CliRecorder::install(Vec::new());
        codex_alias_sync("s1", "gpt-5.2-codex", "grok/grok-4.7");
        codex_alias_sync("s2", "gpt-5.2-codex", "kimi/k2.6");
        assert_eq!(cli.calls().len(), 2, "两次 add，无删除");
        codex_alias_clear("s1");
        assert_eq!(
            cli.calls().len(),
            2,
            "s2 还在用同名组，s1 解绑不能删"
        );
        codex_alias_clear("s2");
        let calls = cli.calls();
        assert_eq!(
            calls[2],
            vec!["group".to_string(), "rm".to_string(), "gpt-5.2-codex".to_string()],
            "最后一个引用者解绑时删组"
        );
        // 无登记的会话清别名：no-op。
        codex_alias_clear("s-unknown");
        assert_eq!(cli.calls().len(), 3);
    }

    /// 复用实例 / 引擎没跑：别名静默跳过（组写不进别人的状态；留痕走
    /// debug_log，不进 CLI）。
    #[test]
    fn codex_alias_skipped_on_reused_or_stopped_engine() {
        let cli = CliRecorder::install(Vec::new());
        // 引擎自拉 Running（CLIRecorder 装的）→ 正常；然后改成 reused。
        codex_alias_sync("s1", "gpt-5.2-codex", "grok/grok-4.7");
        assert_eq!(cli.calls().len(), 1);
        {
            let mut state = lock_state();
            state.status = EngineStatus::Running {
                port: 3425,
                version: None,
                reused: true,
            };
        }
        codex_alias_sync("s2", "gpt-5.1-codex", "grok/grok-4.7");
        assert_eq!(cli.calls().len(), 1, "reused 实例不能写组");
        {
            let mut state = lock_state();
            state.status = EngineStatus::Absent;
        }
        codex_alias_sync("s3", "gpt-5.1-codex", "grok/grok-4.7");
        assert_eq!(cli.calls().len(), 1, "引擎没跑同样跳过");
    }

    /// 组 id 规则对齐 magpie CLI 校验：大写防御性归一，空名 / 非法字符拒绝
    /// （不拉 CLI、不进表）。
    #[test]
    fn codex_alias_validates_group_ids() {
        let cli = CliRecorder::install(Vec::new());
        // 大写内建名：归一成小写后建组（codex 实际全小写，防御路径）。
        codex_alias_sync("s1", "GPT-5.2-CODEX", "grok/grok-4.7");
        assert!(cli.calls()[0].contains(&"id=gpt-5.2-codex".to_string()));
        // 非法字符（空格 / 斜杠）与空名：拒绝。
        codex_alias_sync("s2", "gpt 5", "m");
        codex_alias_sync("s3", "provider/model", "m");
        codex_alias_sync("s4", "  ", "m");
        assert_eq!(cli.calls().len(), 1);
    }
}
