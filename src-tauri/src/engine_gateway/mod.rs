//! 进程内引擎网关——magpie 种子的 Rust 替身（docs/plans/plan-engine-rust.md）。
//!
//! 目标形态：编译进 marionette.exe 的一个模块，`engine.rs` 直接拉起监听线程，
//! **不再有内嵌子 exe（20.6MB UPX 载荷）和独立 magpie 进程（100–160MB 内存）**。
//! 合同面只有一张（docs/plans/plan-engine-contract.md）：/v1/models、组宣告、
//! quotas、chat/responses/messages 三组协议、provider/group CLI 动词。
//! 验收 = scripts/verify-engine-contract.mts 全绿 + 真机对账。
//!
//! 体积红线：**不引入 tokio/hyper/axum**——std TcpListener + 线程足够
//! （loopback-only HTTP/1.1）；出站复用 crate 既有的 ureq（ring-less）。
//! 本模块当前只被开发/测试二进制 `marionette-engine`（engine_gateway_bin.rs）
//! 编译；接进 main.rs 的 serve 链是账本里的任务，别提前焊死。
#![allow(dead_code)]

pub mod affinity;
pub mod antigravity_account;
pub mod antigravity_codeassist;
pub mod antigravity_stream;
pub mod bodycap;
pub mod bodyprep;
pub mod catalog;
pub mod chat;
pub mod chattidy;
pub mod claude_account;
pub mod cli;
pub mod codex_account;
pub mod codex_catalog;
pub mod cors;
pub mod effort;
pub mod grok_account;
pub mod groups;
pub mod httpd;
pub mod integral;
pub mod key_balances;
pub mod messages;
pub mod opencode_client;
pub mod plan_quotas;
pub mod presets;
pub mod prev_response;
pub mod quota_extra;
pub mod quota_kit;
pub mod quotas;
pub mod redact;
pub mod responses;
pub mod routes;
pub mod store;
pub mod textcalls;
pub mod translate;
pub mod usage_ledger;
pub mod vendor_presets;
pub mod ws;
pub mod zcode_account;

use std::path::PathBuf;
use std::sync::Mutex;

/// 引擎自己的版本串（`/v1/models` 之外的自我标识，语义同 magpie 的 tag）。
pub const ENGINE_VERSION: &str = concat!("rust-", env!("CARGO_PKG_VERSION"));

/// Shared modules compile in two crate roots: the GUI (`engine-rust`) and the
/// standalone contract-test binary. Only the GUI owns the locale state; the
/// dev binary deliberately picks the English diagnostic.
pub(crate) fn localized(zh: String, en: String) -> String {
    #[cfg(engine_rust_active)]
    {
        if crate::loc::is_en() {
            en
        } else {
            zh
        }
    }
    #[cfg(not(engine_rust_active))]
    {
        let _ = zh;
        en
    }
}

/// magpie 同款落盘布局（appdir）：exe 旁有 `.portable` → 便携（数据 beside
/// exe）；否则 `XDG_CONFIG_HOME/magpie`（绝对路径才认）；否则
/// `%USERPROFILE%\.config\magpie`。providers.json 与 magpie 完全互通——
/// 用户已配置的来源原样迁移。
pub fn config_dir() -> Option<PathBuf> {
    if let Some(dir) = CONFIG_DIR_OVERRIDE.get() {
        return Some(dir.clone());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.join(".portable").exists() {
                return Some(dir.to_path_buf());
            }
        }
    }
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        let p = PathBuf::from(&xdg);
        if p.is_absolute() {
            return Some(p.join("magpie"));
        }
    }
    std::env::var("USERPROFILE")
        .ok()
        .map(|h| PathBuf::from(h).join(".config").join("magpie"))
}

/// 主程序在拉起网关前指定数据目录：Marionette 的 magpie 一直跑在
/// `.portable` 模式（`~/.marionette/engine/` 旁放 `.portable`，数据在
/// `engine/data/`）——用户已有的来源、钥匙、跨用别名组、登录都在那里。
/// 进程内引擎若按 `~/.config/magpie` 找，会看到一张空表：别名组全失效、
/// codex 会话 404。所以 engine-rust 形态必须沿用同一个目录。
static CONFIG_DIR_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

pub fn set_config_dir(dir: PathBuf) {
    let _ = CONFIG_DIR_OVERRIDE.set(dir);
}

/// 命令名/路径 → 可以直接 spawn 的 (program, 前置参数)。主程序注入
/// process_util 那一套查找（同 set_outbound_proxy：网关不 import 主程序模块）。
pub type CommandResolver = fn(&str) -> Option<(String, Vec<String>)>;

static COMMAND_RESOLVER: std::sync::OnceLock<CommandResolver> = std::sync::OnceLock::new();

pub fn set_command_resolver(resolver: CommandResolver) {
    let _ = COMMAND_RESOLVER.set(resolver);
}

/// 铸一枚网关 Bearer 令牌（48 hex；tools_mcp::mint_token 同款形状：
/// 计数器 nonce + nanos + pid 经 SHA-256 折叠）。2026-11-22 安全批——
/// engine.rs 落盘到 `<engine_dir>/serve-token.txt`（每安装一枚、引擎回收
/// 再唤醒不变），cli.rs 的 argv serve 也用它（MAGPIE_TOKEN 可钉死以便测试）。
pub fn mint_gateway_token() -> String {
    use sha2::{Digest, Sha256};
    static NONCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nonce = NONCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut hasher = Sha256::new();
    hasher.update(b"marionette-gateway-token");
    hasher.update(nonce.to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..48].to_string()
}

// ─── 订阅共享执行闸（2026-11-22 产品定案：四族默认不共享）─────────────────────
//
// claude / codex / grok / gemini 四族的模型**默认不进网关目录、拦截器不接**
// （关 = 直连 HTTP 也 404，与未安装无异）。主程序把「用户已开启的族」推进来
// （引擎启动时 + 面板开关命令时）。**未推送 = 全开**：standalone dev bin 与
// 合同套件维持旧形状；engine.rs 的偏好过滤（catalog_direct*）才是菜单/注入
// 层的唯一过滤点，这层只是执行闸。额度行不受此闸影响。

static SUBSCRIPTION_ENABLED: std::sync::RwLock<Option<Vec<String>>> = std::sync::RwLock::new(None);

/// 主程序推送「已开启的族」。测试构建 no-op（同进程的网关单测依赖
/// 「未推送 = 全开」默认；闸测试用 set_subscription_enabled_for_tests）。
pub fn set_subscription_enabled(families: Vec<String>) {
    #[cfg(test)]
    {
        let _ = families;
    }
    #[cfg(not(test))]
    {
        if let Ok(mut guard) = SUBSCRIPTION_ENABLED.write() {
            *guard = Some(families);
        }
    }
}

/// 某订阅族现在放不放行（未推送 = 全开）。
pub fn family_enabled(family: &str) -> bool {
    match SUBSCRIPTION_ENABLED.read() {
        Ok(guard) => match guard.as_ref() {
            Some(list) => list.iter().any(|f| f == family),
            None => true,
        },
        Err(poisoned) => poisoned
            .into_inner()
            .as_ref()
            .map_or(true, |list| list.iter().any(|f| f == family)),
    }
}

/// 测试专用：直接布置执行闸（Some(list) = 已推送名单；None = 复位回「全开」
/// 默认）。闸测试必须取 TEST_SANDBOX_LOCK（world() 已代取）并保证 Drop 复位。
#[cfg(test)]
pub(crate) fn set_subscription_enabled_for_tests(families: Option<Vec<String>>) {
    if let Ok(mut guard) = SUBSCRIPTION_ENABLED.write() {
        *guard = families;
    }
}

// ─── 本机的订阅登录态（模型共享面板「订阅共享」四族的已登录 / 未登录）──────────
//
// 面板以前拿发现链的账号去重清单（`accounts`）当登录信号，而那份清单只产出
// ChatGPT 一家——Claude / Grok / Gemini 三行不管登没登录都写「未检测到本机
// 登录」（2026-11-26 用户实锤：登着 Grok 和 Antigravity 也这么说）。现在直接问
// 各账号模块自己的只读发现：**与合成来源 / 额度行同一份判断**——这里说已登录，
// 引擎侧就一定拿得到这个账号。不看过期：access token 到期由各家 CLI 自己续，
// 用户眼里仍是「登录着」；过期是开启共享之后由引擎如实报的事。

/// 本机已登录的订阅族（`claude` / `codex` / `grok` / `gemini`；gemini =
/// Antigravity 的 Google 登录）。每次现读文件：不缓存、不联网、不写任何凭据。
pub fn signed_in_families() -> Vec<&'static str> {
    let mut families = Vec::new();
    if claude_account::discover().is_some() {
        families.push("claude");
    }
    if codex_account::discover().is_some() {
        families.push("codex");
    }
    if grok_account::discover().is_some() {
        families.push("grok");
    }
    if antigravity_account::signed_in() {
        families.push("gemini");
    }
    families
}

/// 网关共享状态：provider/group 存储 + 模型目录缓存 + 字节上限学习表
/// （provider id → 已学 cap 字节数）。
pub struct Gateway {
    pub store: Mutex<store::Store>,
    pub catalog: catalog::Catalog,
    pub caps: Mutex<std::collections::HashMap<String, usize>>,
    /// 会话粘性（affinity.rs）：同会话粘住曾答账号——厂商缓存命中 =
    /// 速度与成本（Go affinity.go 的最小版）。
    pub affinity: affinity::Affinity,
    /// usage 台账（usage_ledger.rs）：每轮一行 JSONL，M2 对账数据源。
    pub usage: usage_ledger::UsageLedger,
    /// previous_response_id 的服务端保存表（prev_response.rs）：responses
    /// 翻译路径的存回合状态。Arc 是因为流式应答的编码器（ResponsesSse）
    /// 要把它带进 Reply::Stream 的 reader 里在收尾时落账。
    pub prev: std::sync::Arc<prev_response::PrevStore>,
}

impl Gateway {
    pub fn open() -> Result<Gateway, String> {
        Ok(Gateway {
            store: Mutex::new(store::Store::load()?),
            catalog: catalog::Catalog::default(),
            caps: Mutex::new(std::collections::HashMap::new()),
            affinity: affinity::Affinity::load(),
            usage: usage_ledger::UsageLedger::open(),
            prev: std::sync::Arc::new(prev_response::PrevStore::default()),
        })
    }
}

/// 出站 HTTP agent（ureq，ring-less）：连接 5s。**TLS 必须手工接
/// native-tls 连接器**——ureq 在 default-features=false 下 native-tls
/// feature 不点亮 tls 总闸，裸 AgentBuilder 的 https 全部
/// 'Unknown Scheme: no TLS backend'（2026-11-18 真机实锤，engine.rs
/// download_agent 同款做法）。主程序构建（feature engine-rust）带
/// Marionette 的代理设置；独立 dev bin 走环境变量兜底。
pub fn outbound_agent() -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new().timeout_connect(std::time::Duration::from_secs(5));
    if let Ok(connector) = ureq::native_tls::TlsConnector::new() {
        builder = builder.tls_connector(std::sync::Arc::new(connector));
    }
    if let Some(url) = outbound_proxy().filter(|u| !u.trim().is_empty()) {
        builder = builder.proxy(ureq_proxy(url.trim()));
    }
    builder.build()
}

/// 代理地址 → ureq `Proxy`（主程序侧的同义件是 `proxy::ureq_proxy`；网关不
/// import 主程序模块，各留一份）：
/// - scheme 大小写不敏感；`socks5h://` 改写成 `socks5://`——ureq 2 只认后者，
///   而它的 socks5 本来就把域名原样交给代理解析，两者同义；
/// - 仍解析不了的地址（https 代理等）**不回落直连**：指向本机不可达的丢弃
///   端口，请求以连接失败收场。用户配了代理就是不想让流量裸奔（主程序
///   http_client.rs 同款纪律；旧写法 `if let Ok(..)` 在这里是 fail-open）。
pub(crate) fn ureq_proxy(url: &str) -> ureq::Proxy {
    let normalized = match url.split_once("://") {
        Some((scheme, rest)) if scheme.eq_ignore_ascii_case("socks5h") => {
            format!("socks5://{rest}")
        }
        Some((scheme, rest)) => format!("{}://{rest}", scheme.to_ascii_lowercase()),
        None => url.to_string(),
    };
    ureq::Proxy::new(normalized).unwrap_or_else(|_| {
        ureq::Proxy::new("http://127.0.0.1:9").expect("static blackhole proxy parses")
    })
}

/// 出站代理（engine_gateway 不 import 主程序模块——dev bin 的 crate 根
/// 不是 marionette，crate::proxy 在那边不存在）：主进程在拉起网关线程
/// 前用 set_outbound_proxy 注入 Marionette 的代理设置；没注入时退环境
/// 变量（ureq 不自动读代理 env）。
///
/// `None` = 尚未注入；`Some(None)` = 用户明确关闭代理。不能用 OnceLock：
/// 用户修改代理后停止/重启引擎时，第二次注入必须覆盖第一次。
static OUTBOUND_PROXY: Mutex<Option<Option<String>>> = Mutex::new(None);

pub fn set_outbound_proxy(url: Option<String>) {
    let normalized = url
        .map(|url| url.trim().to_string())
        .filter(|url| !url.is_empty());
    let mut slot = OUTBOUND_PROXY.lock().unwrap_or_else(|p| p.into_inner());
    *slot = Some(normalized);
}

fn outbound_proxy() -> Option<String> {
    if let Some(set) = OUTBOUND_PROXY
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .clone()
    {
        return set;
    }
    for key in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        if let Ok(url) = std::env::var(key) {
            if !url.trim().is_empty() {
                return Some(url);
            }
        }
    }
    None
}

/// 测试沙箱共享串行锁：quotas/presets（以及后续沙箱模块）的测试都靠
/// 进程级 `XDG_CONFIG_HOME` 重定向 config_dir，各持私锁时并行测试仍互踩
/// env——所有沙箱模块的测试统一取这一把（council store 沙箱同款做法）。
#[cfg(test)]
pub(crate) static TEST_SANDBOX_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    static PROXY_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn outbound_proxy_can_be_replaced_between_engine_restarts() {
        let _guard = PROXY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        set_outbound_proxy(Some(" http://127.0.0.1:7890 ".into()));
        assert_eq!(outbound_proxy().as_deref(), Some("http://127.0.0.1:7890"));
        set_outbound_proxy(Some("http://127.0.0.1:7891".into()));
        assert_eq!(outbound_proxy().as_deref(), Some("http://127.0.0.1:7891"));
        set_outbound_proxy(Some("   ".into()));
        assert_eq!(
            outbound_proxy(),
            None,
            "an explicit empty setting disables the proxy"
        );
        set_outbound_proxy(None);
        assert_eq!(outbound_proxy(), None);
    }

    /// `socks5h` / 大写 scheme 必须真的走 SOCKS5；解析不了的地址不许静默直连
    /// （旧写法 `if let Ok(proxy) = Proxy::new(..)` 失败即裸奔）。
    #[test]
    fn ureq_proxy_normalizes_socks_spellings_and_never_goes_direct() {
        use std::io::Read;
        use std::time::Duration;
        let agent_for = |proxy: ureq::Proxy| {
            ureq::AgentBuilder::new()
                .proxy(proxy)
                .timeout(Duration::from_secs(8))
                .build()
        };
        for url in ["socks5h://127.0.0.1:PORT", "SOCKS5://127.0.0.1:PORT"] {
            // 假 SOCKS5：只收协商头就断开——足够证明请求交到了 SOCKS 代理手里。
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut greeting = [0u8; 3];
                let _ = stream.read_exact(&mut greeting);
                let _ = tx.send(greeting.to_vec());
            });
            let url = url.replace("PORT", &port.to_string());
            let outcome = agent_for(ureq_proxy(&url))
                .get("http://example.invalid/v1/models")
                .call();
            assert!(
                outcome.is_err(),
                "{url}: the mock never completes the handshake"
            );
            let greeting = rx
                .recv_timeout(Duration::from_secs(8))
                .unwrap_or_else(|e| panic!("{url}: request never reached the SOCKS proxy: {e}"));
            assert_eq!(
                greeting,
                vec![0x05, 0x01, 0x00],
                "{url}: a SOCKS5 no-auth greeting"
            );
        }

        // https 代理 ureq 2 说不来：请求不得落到这个「代理」端口上（直连更不行）。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let _ = tx.send(());
                drop(stream);
            }
        });
        let outcome = agent_for(ureq_proxy(&format!("https://127.0.0.1:{port}")))
            .get("http://example.invalid/v1/models")
            .call();
        assert!(
            outcome.is_err(),
            "an unparseable proxy must not be silently bypassed"
        );
        assert!(
            rx.recv_timeout(Duration::from_millis(300)).is_err(),
            "traffic reached the address we could not parse as a proxy"
        );
    }

    /// 「订阅共享」四族的已登录直接问各账号模块（2026-11-26 修：以前只有 ChatGPT
    /// 会被认出来）。没布置凭据 = 一家都不认；布置后按各自的凭据文件认；Claude 主动
    /// 登出后的形状（access / refresh 都清空、只剩订阅类型元数据）不算登录——用户
    /// 机器上就是这个形状；API key 形态的 Codex 不是订阅登录。
    #[test]
    fn signed_in_families_come_from_each_accounts_own_discovery() {
        let _guard = TEST_SANDBOX_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                // 四个缝类型各异，逐个复位。
                *codex_account::TEST_SEAM
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = None;
                *grok_account::TEST_SEAM
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = None;
                *claude_account::TEST_SEAM
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = None;
                *antigravity_account::TEST_SEAM
                    .lock()
                    .unwrap_or_else(|p| p.into_inner()) = None;
            }
        }
        let _reset = Reset;
        assert!(
            signed_in_families().is_empty(),
            "no credentials laid down → nothing is signed in"
        );

        let dir = std::env::temp_dir().join(format!(
            "marionette-engine-signed-in-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let write = |rel: &str, body: &str| {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        };
        // Grok：auth.json 里有一条带 key 的账号。
        write(
            "grok/auth.json",
            r#"{"me": {"key": "k-fixture", "email": "a@b", "user_id": "u"}}"#,
        );
        // Antigravity：五个字段齐。
        write(
            ".gemini/antigravity-acp/acp_token.json",
            r#"{"client_id":"c","client_secret":"s","refresh_token":"r","token_uri":"https://oauth2.googleapis.com/token","project_id":"p"}"#,
        );
        // Claude：CLI 主动登出后的形状。
        write(
            "claude/.credentials.json",
            r#"{"claudeAiOauth":{"accessToken":"","refreshToken":"","subscriptionType":"team","rateLimitTier":"x","scopes":"a b","refreshTokenExpiresAt":1}}"#,
        );
        *grok_account::TEST_SEAM
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(grok_account::Seam {
            base: String::new(),
            home: dir.join("grok"),
        });
        *antigravity_account::TEST_SEAM
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(antigravity_account::Seam {
            base: String::new(),
            home: dir.clone(),
        });
        *claude_account::TEST_SEAM
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(claude_account::Seam {
            home: dir.join("claude"),
            logins: None,
            shelf: None,
            cli: None,
        });
        assert_eq!(
            signed_in_families(),
            vec!["grok", "gemini"],
            "a logged-out Claude is not signed in"
        );

        // Codex：auth.json 的 ChatGPT 登录（发现只看 access token 在不在，不要求是 JWT）。
        write(
            "codex/auth.json",
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"tok-fixture","account_id":"acct"}}"#,
        );
        *codex_account::TEST_SEAM
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(codex_account::Seam {
            base: String::new(),
            logins_dir: dir.join("engine-data"),
            codex_home: dir.join("codex"),
        });
        // Claude 重新登录。
        write(
            "claude/.credentials.json",
            r#"{"claudeAiOauth":{"accessToken":"at-fixture","refreshToken":"rt","subscriptionType":"pro","expiresAt":4102444800000}}"#,
        );
        assert_eq!(
            signed_in_families(),
            vec!["claude", "codex", "grok", "gemini"]
        );
        // API key 形态的 Codex 不是订阅登录。
        write(
            "codex/auth.json",
            r#"{"auth_mode":"apikey","OPENAI_API_KEY":"sk-fixture"}"#,
        );
        assert!(!signed_in_families().contains(&"codex"));
        drop(_reset);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
