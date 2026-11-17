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

pub mod bodycap;
pub mod catalog;
pub mod chat;
pub mod cli;
pub mod groups;
pub mod httpd;
pub mod messages;
pub mod presets;
pub mod quotas;
pub mod responses;
pub mod routes;
pub mod store;
pub mod translate;
pub mod vendor_presets;

use std::path::PathBuf;
use std::sync::Mutex;

/// 引擎自己的版本串（`/v1/models` 之外的自我标识，语义同 magpie 的 tag）。
pub const ENGINE_VERSION: &str = concat!("rust-", env!("CARGO_PKG_VERSION"));

/// magpie 同款落盘布局（appdir）：exe 旁有 `.portable` → 便携（数据 beside
/// exe）；否则 `XDG_CONFIG_HOME/magpie`（绝对路径才认）；否则
/// `%USERPROFILE%\.config\magpie`。providers.json 与 magpie 完全互通——
/// 用户已配置的来源原样迁移。
pub fn config_dir() -> Option<PathBuf> {
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

/// 网关共享状态：provider/group 存储 + 模型目录缓存 + 字节上限学习表
/// （provider id → 已学 cap 字节数）。
pub struct Gateway {
    pub store: Mutex<store::Store>,
    pub catalog: catalog::Catalog,
    pub caps: Mutex<std::collections::HashMap<String, usize>>,
}

impl Gateway {
    pub fn open() -> Result<Gateway, String> {
        Ok(Gateway {
            store: Mutex::new(store::Store::load()?),
            catalog: catalog::Catalog::default(),
            caps: Mutex::new(std::collections::HashMap::new()),
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
    let mut builder = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(5));
    if let Ok(connector) = ureq::native_tls::TlsConnector::new() {
        builder = builder.tls_connector(std::sync::Arc::new(connector));
    }
    if let Some(url) = outbound_proxy().filter(|u| !u.trim().is_empty()) {
        if let Ok(proxy) = ureq::Proxy::new(url.trim()) {
            builder = builder.proxy(proxy);
        }
    }
    builder.build()
}

/// 出站代理（engine_gateway 不 import 主程序模块——dev bin 的 crate 根
/// 不是 marionette，crate::proxy 在那边不存在）：主进程在拉起网关线程
/// 前用 set_outbound_proxy 注入 Marionette 的代理设置；没注入时退环境
/// 变量（ureq 不自动读代理 env）。
static OUTBOUND_PROXY: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

pub fn set_outbound_proxy(url: Option<String>) {
    let _ = OUTBOUND_PROXY.set(url);
}

fn outbound_proxy() -> Option<String> {
    if let Some(set) = OUTBOUND_PROXY.get() {
        return set.clone();
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
