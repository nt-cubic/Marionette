//! Read existing CLI provider definitions without taking ownership of their files.
//! Secrets are resolved only for engine registration and never serialized to the UI.
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceModel {
    pub id: String,
    pub label: String,
    pub native_ids: BTreeMap<String, Vec<String>>,
}

/// A native provider that resolves to a discovered source's account. The
/// picker folds its rows into that source; `label` is the vendor name the agent
/// itself displays, which reads better than a bare host name.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeAccountRef {
    pub provider: String,
    pub label: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceView {
    pub id: String,
    pub label: String,
    pub endpoint: String,
    pub protocol: String,
    pub origins: Vec<String>,
    pub models: Vec<SourceModel>,
    pub issue: Option<String>,
    pub missing_env: Vec<String>,
    /// agent id → native providers that agent already carries in its own
    /// catalogue and that resolve to this same account (same host, same
    /// credential). The picker folds those native rows into this source
    /// instead of listing the account twice.
    pub native_accounts: BTreeMap<String, Vec<NativeAccountRef>>,
}

pub struct Source {
    pub view: SourceView,
    pub key: String,
    pub headers: BTreeMap<String, String>,
    /// Environment variable names this route authenticates through, resolved or
    /// not. Account identity uses them: two routes naming the same variable for
    /// the same host are the same credential.
    pub env_names: Vec<String>,
}

impl Source {
    /// 手动添加的自定义来源（凭据库条目）→ 它的条目 id。**别用 `view.id` 判断**：
    /// 那是 `source-<端点|协议|身份 的摘要>`，不是条目的 `custom-…`。能认出来的只有
    /// origin——`Marionette · <credentials.json>#<条目 id>`（`discover_at` 写入）；
    /// 从 agent 配置文件里发现的来源 origin 以 agent 名开头（`grok-build · …`）。
    pub fn manual_entry_id(&self) -> Option<&str> {
        self.view.origins.iter().find_map(|origin| {
            origin
                .strip_prefix("Marionette · ")
                .and_then(|rest| rest.rsplit_once('#').map(|(_, id)| id))
        })
    }
}

/// 目录请求共用的 agent（来源检测 / 自定义来源预览）：native-tls、8s、
/// **不跟随重定向**（重定向会把鉴权头带给别的站点）。`proxy` 由调用方给：
/// 生产传应用代理设置，测试传 `None`——不读本机配置，回环假服务才可达。
fn catalog_agent(proxy: Option<&str>) -> Result<ureq::Agent, String> {
    let Ok(connector) = ureq::native_tls::TlsConnector::new() else {
        return Err(crate::ts!("无法初始化 TLS", "Could not initialize TLS"));
    };
    let mut builder = ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(connector))
        .timeout(std::time::Duration::from_secs(8))
        .redirects(0);
    if let Some(url) = proxy {
        // 解析不了的地址不回落直连：如实报「代理地址无效」。
        let Ok(proxy) = crate::proxy::ureq_proxy(url) else {
            return Err(crate::ts!("代理地址无效", "Invalid proxy URL"));
        };
        builder = builder.proxy(proxy);
    }
    Ok(builder.build())
}

/// 应用代理设置（已启用且非空）→ 代理地址。
fn app_proxy() -> Option<String> {
    let config = crate::proxy::load_proxy_config();
    config
        .enabled
        .then(|| config.url.trim().to_string())
        .filter(|url| !url.is_empty())
}

/// 按协议给目录请求带鉴权：Anthropic 用 `x-api-key` + 版本头，其余 Bearer。
fn with_catalog_auth(request: ureq::Request, protocol: &str, key: &str) -> ureq::Request {
    if key.is_empty() {
        return request;
    }
    if protocol == "anthropic" {
        request
            .set("x-api-key", key)
            .set("anthropic-version", "2023-06-01")
    } else {
        request.set("Authorization", &format!("Bearer {key}"))
    }
}

/// 轻量的带鉴权目录请求；绝不发生成请求。
pub fn check_source(id: &str) -> Value {
    let result =
        |state: &str, detail: &str| serde_json::json!({ "state": state, "detail": detail });
    let Some(source) = discover()
        .sources
        .into_iter()
        .find(|source| source.view.id == id)
    else {
        return result(
            &crate::ts!("配置已变化", "Configuration changed"),
            &crate::ts!("请重新扫描", "Re-scan required"),
        );
    };
    if let Some(issue) = &source.view.issue {
        return result(
            &crate::ts!("配置待处理", "Configuration needs attention"),
            issue,
        );
    }
    let agent = match catalog_agent(app_proxy().as_deref()) {
        Ok(agent) => agent,
        Err(error) => {
            let state = if error.contains("代理") || error.contains("proxy") {
                crate::ts!("配置待处理", "Configuration needs attention")
            } else {
                crate::ts!("检测失败", "Check failed")
            };
            return result(&state, &error);
        }
    };
    let url = format!("{}/models", source.view.endpoint.trim_end_matches('/'));
    let mut request = with_catalog_auth(agent.get(&url), &source.view.protocol, &source.key);
    for (name, value) in &source.headers {
        request = request.set(name, value);
    }
    let started = std::time::Instant::now();
    match request.call() {
        Ok(response) => {
            // Bound the body and require a catalogue, since a login HTML page can also be HTTP 200.
            let body: Result<Value, _> =
                serde_json::from_reader(std::io::Read::take(response.into_reader(), 1024 * 1024));
            if body.as_ref().ok().is_some_and(|body| {
                body.get("data").is_some_and(Value::is_array)
                    || body.get("models").is_some_and(Value::is_array)
            }) {
                result(&crate::ts!("已连接 · {ms} ms", "Connected · {ms} ms", ms = started.elapsed().as_millis()), &crate::ts!("模型目录可访问；实际调用还取决于模型权限和余额", "Model catalogue reachable; actual calls still depend on model permissions and balance"))
            } else {
                result(
                    &crate::ts!("待确认", "Needs confirmation"),
                    &crate::ts!(
                        "接口有响应，但未返回模型目录",
                        "Endpoint responded but returned no model catalogue"
                    ),
                )
            }
        }
        Err(ureq::Error::Status(401 | 403, _)) => result(
            &crate::ts!("认证受限", "Auth limited"),
            &crate::ts!(
                "请检查密钥和访问权限",
                "Check the API key and access permissions"
            ),
        ),
        Err(ureq::Error::Status(404 | 405, _)) => result(
            &crate::ts!("待确认", "Needs confirmation"),
            &crate::ts!(
                "服务不支持模型目录检测，可尝试实际调用",
                "Service does not support catalogue probing; try an actual call"
            ),
        ),
        Err(ureq::Error::Status(status, _)) => result(
            &crate::ts!("检测失败", "Check failed"),
            &format!("HTTP {status}"),
        ),
        Err(_) => result(
            &crate::ts!("连接失败", "Connection failed"),
            &crate::ts!(
                "连接超时或网络、TLS 异常",
                "Timed out or a network/TLS error"
            ),
        ),
    }
}

/// 目录里的一条模型。`label` 是服务端给的显示名（Anthropic 目录有）；没有或
/// 与 id 相同时留空。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbedModel {
    pub id: String,
    pub label: String,
}

/// 「添加自定义来源」保存前的目录预览结论。
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomProbe {
    pub ok: bool,
    pub models: Vec<ProbedModel>,
    /// 失败原因，已翻成能照着做的结论；成功时为空。
    pub detail: String,
}

impl CustomProbe {
    fn failed(detail: String) -> Self {
        Self {
            ok: false,
            models: Vec::new(),
            detail,
        }
    }
}

/// 「添加自定义来源」的「获取模型」：按地址 + 协议 + key 拉 `{endpoint}/models`，
/// 不落盘、不注册、不发生成请求（2026-11-26）。地址与保存时同一套校验
/// （`credentials::clean_custom_endpoint`），所以这里能拉到，保存就能过。
/// Anthropic 目录分页（`has_more` + `last_id` → `after_id`），其余协议取一页。
pub fn probe_custom(endpoint: &str, protocol: &str, key: &str) -> CustomProbe {
    probe_custom_with(endpoint, protocol, key, app_proxy().as_deref())
}

fn probe_custom_with(
    endpoint: &str,
    protocol: &str,
    key: &str,
    proxy: Option<&str>,
) -> CustomProbe {
    let endpoint = match crate::credentials::clean_custom_endpoint(endpoint) {
        Ok(endpoint) => endpoint,
        Err(error) => return CustomProbe::failed(error),
    };
    let Ok(agent) = catalog_agent(proxy) else {
        return CustomProbe::failed(crate::ts!("无法初始化 TLS", "Could not initialize TLS"));
    };
    // 目录过大的保护：再多也只取前 2000 条（界面列不下，也没人用得到）。
    const MAX_MODELS: usize = 2000;
    const MAX_PAGES: usize = 10;
    let mut models: Vec<ProbedModel> = Vec::new();
    let mut url = format!("{endpoint}/models");
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let response = match with_catalog_auth(agent.get(&url), protocol, key).call() {
            Ok(response) => response,
            Err(ureq::Error::Status(401 | 403, _)) => {
                return CustomProbe::failed(crate::ts!(
                    "密钥被拒绝（HTTP 401/403），检查 API Key 和访问权限",
                    "The key was rejected (HTTP 401/403) — check the API key and its permissions"
                ));
            }
            Err(ureq::Error::Status(404 | 405, _)) => {
                return CustomProbe::failed(crate::ts!(
                    "这个服务不提供模型目录，请在下方手动填写模型 ID",
                    "This service does not list its models — add model IDs by hand below"
                ));
            }
            Err(ureq::Error::Status(status, _)) => {
                return CustomProbe::failed(format!("HTTP {status}"))
            }
            Err(_) => {
                return CustomProbe::failed(crate::ts!("连接失败或超时，检查接口地址、网络和代理设置", "Could not connect or timed out — check the endpoint, your network and proxy settings"));
            }
        };
        // 1 MiB 上限：登录页/大 HTML 也可能是 200。
        let body: Result<Value, _> =
            serde_json::from_reader(std::io::Read::take(response.into_reader(), 1024 * 1024));
        let rows = body.as_ref().ok().and_then(|body| {
            body.get("data")
                .and_then(Value::as_array)
                .or_else(|| body.get("models").and_then(Value::as_array))
        });
        let Some(rows) = rows else {
            return CustomProbe::failed(crate::ts!("接口有响应，但返回的不是模型目录，请检查接口地址是否以 /v1 结尾", "The endpoint answered but did not return a model catalogue — check that the URL ends with /v1"));
        };
        for row in rows {
            let Some(id) = row
                .get("id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
            else {
                continue;
            };
            if models.iter().any(|m| m.id == id) {
                continue;
            }
            let label = row
                .get("display_name")
                .or_else(|| row.get("name"))
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or("");
            models.push(ProbedModel {
                id: id.to_string(),
                label: if label == id {
                    String::new()
                } else {
                    label.to_string()
                },
            });
            if models.len() >= MAX_MODELS {
                break;
            }
        }
        let last = body.as_ref().ok().and_then(|body| {
            body.get("has_more")
                .and_then(Value::as_bool)
                .filter(|more| *more)?;
            body.get("last_id")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(String::from)
        });
        // 游标没往前走（服务端一直回同一页）就停：别对着一个坏分页空转十次。
        let Some(last) = last.filter(|last| cursor.as_deref() != Some(last.as_str())) else {
            break;
        };
        url = format!("{endpoint}/models?after_id={}", percent_encode(&last));
        cursor = Some(last);
    }
    if models.is_empty() {
        return CustomProbe::failed(crate::ts!("模型目录是空的", "The model catalogue is empty"));
    }
    CustomProbe {
        ok: true,
        models,
        detail: String::new(),
    }
}

/// 查询串里安全的最小百分号编码（`after_id` 用；模型 id 一般是 `a-z0-9.-_`）。
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// One provider an agent already carries in its own catalogue, reduced to the
/// facts that decide whether it is the same account as a discovered source.
pub struct NativeAccount {
    pub agent: String,
    pub provider: String,
    /// Vendor name the agent displays (models.dev `name`).
    pub label: String,
    pub host: String,
    pub env_names: Vec<String>,
    pub key: Option<String>,
}

/// An engine provider that runs on a local sign-in, and an agent catalogue
/// that turns out to be that same account. One sign-in is one menu group.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineAccount {
    /// Engine provider prefix (`codex`).
    pub engine: String,
    pub agent: String,
    /// Native provider the agent lists the account under. `""` means the
    /// agent's whole catalogue is that sign-in (Codex CLI with its own login).
    pub provider: String,
    /// `owner` rows join the engine group; `duplicate` listings of an account
    /// another agent already owns are left out of the picker.
    pub kind: &'static str,
}

#[derive(Default)]
pub struct Discovery {
    pub sources: Vec<Source>,
    pub warnings: Vec<String>,
    pub paths: Vec<ScanPath>,
    pub accounts: Vec<EngineAccount>,
}

#[derive(Serialize)]
pub struct ScanPath {
    pub path: String,
    pub status: String,
    pub detail: String,
}

fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

fn text<'a>(v: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .unwrap_or("")
}

// Only environment references are expanded. Shell commands in third party
// configurations are never executed as part of background discovery.
fn env_reference(raw: &str) -> Option<&str> {
    let raw = raw.trim();
    raw.strip_prefix("{env:")
        .and_then(|s| s.strip_suffix('}'))
        .or_else(|| raw.strip_prefix("${").and_then(|s| s.strip_suffix('}')))
        .or_else(|| raw.strip_prefix('$'))
}

fn resolve(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.starts_with('!') {
        return Err(crate::ts!("认证使用命令，请在原配置中改用环境变量以共享", "Authentication uses a command; switch it to an environment variable in the original config to share"));
    }
    let name = env_reference(raw);
    if let Some(name) = name {
        return std::env::var(name)
            .ok()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                crate::ts!(
                    "环境变量 {name} 不可用，请在下方填写并保存",
                    "Environment variable {name} is unavailable; fill it in below and save"
                )
            });
    }
    Ok(raw.into())
}

impl Discovery {
    fn read(&mut self, path: &Path, toml_file: bool) -> Option<Value> {
        if !path.is_file() {
            self.paths.push(ScanPath {
                path: path.display().to_string(),
                status: "missing".into(),
                detail: crate::ts!("文件不存在", "File not found"),
            });
            return None;
        }
        let result = fs::read_to_string(path).map_err(|_| ()).and_then(|s| {
            if toml_file {
                s.parse::<toml::Value>()
                    .ok()
                    .and_then(|v| serde_json::to_value(v).ok())
                    .ok_or(())
            } else {
                serde_json::from_str(&clean_jsonc(&s)).map_err(|_| ())
            }
        });
        match result {
            Ok(v) => {
                self.paths.push(ScanPath {
                    path: path.display().to_string(),
                    status: "found".into(),
                    detail: crate::ts!("已读取配置", "Config loaded"),
                });
                Some(v)
            }
            Err(()) => {
                self.paths.push(ScanPath {
                    path: path.display().to_string(),
                    status: "error".into(),
                    detail: crate::ts!(
                        "文件存在，但读取或解析失败",
                        "File exists but could not be read or parsed"
                    ),
                });
                self.warnings.push(crate::ts!(
                    "无法读取配置：{path}",
                    "Could not read config: {path}",
                    path = path.display()
                ));
                None
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        path: &Path,
        agent: &str,
        route: &str,
        endpoint: &str,
        protocol: &str,
        auth: &str,
        headers: BTreeMap<String, String>,
        models: Vec<(String, String, String)>,
    ) {
        // Ignore Marionette's own injected routes to prevent discovery loops.
        if route == "marionette" || route.starts_with("marionette/") || endpoint.is_empty() {
            return;
        }
        let endpoint = endpoint.trim_end_matches('/');
        // URLs carrying credentials must not be surfaced or registered.
        let authority = endpoint
            .split("://")
            .nth(1)
            .unwrap_or("")
            .split('/')
            .next()
            .unwrap_or("");
        if authority.contains('@')
            || endpoint.contains('?')
            || !(endpoint.starts_with("http://") || endpoint.starts_with("https://"))
        {
            self.warnings.push(crate::ts!(
                "{path} 的 {route} 接口地址需要使用独立认证字段",
                "The {route} endpoint in {path} must use the separate auth field",
                path = path.display()
            ));
            return;
        }
        let identity = if auth.starts_with('$') || auth.starts_with("{env:") {
            auth.to_string()
        } else {
            format!("{}#{route}", path.display())
        };
        let hash = Sha256::digest(format!("{endpoint}|{protocol}|{identity}|{headers:?}"));
        let id = format!("source-{:x}", hash)[..23].to_string();
        let key_result = if let Some(file) = auth
            .strip_prefix("{file:")
            .and_then(|s| s.strip_suffix('}'))
        {
            let file = if let Some(rest) = file.strip_prefix("~/") {
                home().unwrap_or_default().join(rest)
            } else {
                path.parent().unwrap_or(Path::new(".")).join(file)
            };
            fs::read_to_string(file)
                .map(|s| s.trim().to_string())
                .map_err(|_| {
                    crate::ts!(
                        "无法读取原配置引用的密钥文件",
                        "Could not read the key file referenced by the original config"
                    )
                })
        } else {
            resolve(auth)
        };
        let mut missing_env: Vec<String> = std::iter::once(auth)
            .chain(headers.values().map(String::as_str))
            .filter_map(env_reference)
            .filter(|name| std::env::var(name).map(|v| v.is_empty()).unwrap_or(true))
            .map(str::to_string)
            .collect();
        missing_env.sort();
        missing_env.dedup();
        // Every env name this route can authenticate through, resolved or not.
        let mut env_names: Vec<String> = std::iter::once(auth)
            .chain(headers.values().map(String::as_str))
            .filter_map(env_reference)
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())
            .collect();
        env_names.sort();
        env_names.dedup();
        let mut issue = key_result.as_ref().err().cloned();
        let headers: BTreeMap<String, String> = headers
            .into_iter()
            .map(|(k, v)| {
                let value = resolve(&v).unwrap_or_else(|error| {
                    issue = Some(error);
                    String::new()
                });
                (k, value)
            })
            .collect();
        if auth.is_empty()
            && !authority.starts_with("localhost")
            && !authority.starts_with("127.0.0.1")
            && !authority.starts_with("[::1]")
        {
            issue = Some(crate::ts!(
                "未找到可共享的 API key 或环境变量引用",
                "No shareable API key or environment variable reference found"
            ));
        }
        let origin = format!("{agent} · {}#{route}", path.display());
        let index = if let Some(i) = self.sources.iter().position(|s| {
            s.view.id == id
                || (key_result.as_ref().ok() == Some(&s.key)
                    && s.view.endpoint == endpoint
                    && s.view.protocol == protocol
                    && s.headers == headers)
        }) {
            i
        } else {
            let label = authority
                .trim_start_matches("api.")
                .trim_end_matches(":11434")
                .to_string();
            self.sources.push(Source {
                view: SourceView {
                    id,
                    label,
                    endpoint: endpoint.into(),
                    protocol: protocol.into(),
                    origins: vec![],
                    models: vec![],
                    issue,
                    missing_env: missing_env.clone(),
                    native_accounts: BTreeMap::new(),
                },
                key: key_result.unwrap_or_default(),
                headers,
                env_names: env_names.clone(),
            });
            self.sources.len() - 1
        };
        let Some(source) = self.sources.get_mut(index) else {
            // 逻辑上不可达（push 之后必有元素）；留痕跳过本条目，
            // 别让 last_mut().unwrap() 的隐式不变量在 panic=abort 下杀进程。
            crate::debug_log::append(
                "sources",
                "error",
                "",
                "source registry lost entry right after push",
                None,
            );
            return;
        };
        for name in missing_env {
            if !source.view.missing_env.contains(&name) {
                source.view.missing_env.push(name);
            }
        }
        for name in &env_names {
            if !source.env_names.contains(name) {
                source.env_names.push(name.clone());
            }
        }
        if !source.view.origins.contains(&origin) {
            source.view.origins.push(origin);
        }
        for (model, label, native_id) in models {
            if model.is_empty() {
                continue;
            }
            let i = source
                .view
                .models
                .iter()
                .position(|m| m.id == model)
                .unwrap_or_else(|| {
                    source.view.models.push(SourceModel {
                        id: model.clone(),
                        label,
                        native_ids: BTreeMap::new(),
                    });
                    source.view.models.len() - 1
                });
            let ids = source.view.models[i]
                .native_ids
                .entry(agent.into())
                .or_default();
            if !ids.contains(&native_id) {
                ids.push(native_id);
            }
        }
    }
}

fn string_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.into())))
                .collect()
        })
        .unwrap_or_default()
}

pub fn discover() -> Discovery {
    let Some(home) = home() else {
        return Discovery::default();
    };
    discover_at(&home, true)
}

/// Host of an endpoint, lowercased — the part that identifies the vendor account.
/// Paths differ between an agent's own catalogue (`…/v1`) and a shared route, so
/// they are not part of the identity.
pub fn endpoint_host(endpoint: &str) -> Option<String> {
    let rest = endpoint
        .trim()
        .strip_prefix("https://")
        .or_else(|| endpoint.trim().strip_prefix("http://"))?;
    let host = rest.split(['/', '?', '#']).next().unwrap_or("").trim();
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

/// True when a discovered endpoint and a preset base name the same vendor host.
/// Grok writes `opencode.ai/inference/go/openai/v1` where the preset publishes
/// `opencode.ai/inference/go/v1`; both bill the same account.
pub fn same_vendor_host(
    endpoint: &str,
    chat_base: Option<&str>,
    anthropic_base: Option<&str>,
) -> bool {
    let Some(host) = endpoint_host(endpoint) else {
        return false;
    };
    chat_base
        .and_then(endpoint_host)
        .or_else(|| anthropic_base.and_then(endpoint_host))
        .is_some_and(|base| base == host)
}

/// Providers an agent (OpenCode) already carries in its own catalogue. Their
/// endpoints and env names live in the models.dev cache; keys live in its
/// auth.json or the environment.
pub fn opencode_native_accounts(home: &Path, use_environment: bool) -> Vec<NativeAccount> {
    let Ok(catalog_text) = fs::read_to_string(home.join(".cache/opencode/models.json")) else {
        return Vec::new();
    };
    let Ok(catalog) = serde_json::from_str::<Value>(&catalog_text) else {
        return Vec::new();
    };
    let Some(providers) = catalog.as_object() else {
        return Vec::new();
    };
    let auth_path = home.join(".local/share/opencode/auth.json");
    let auth_keys = crate::credentials::peek_opencode_api_keys_at(&auth_path);
    let mut out = Vec::new();
    for (id, provider) in providers {
        let name = text(provider, &["name"]);
        let label = if name.is_empty() { id.as_str() } else { name };
        let env_names: Vec<String> = provider
            .get("env")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(Value::as_str)
                    .map(|name| name.trim().to_string())
                    .filter(|name| !name.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let host = endpoint_host(text(provider, &["api"]));
        let key = auth_keys
            .iter()
            .find(|(entry, _)| entry == id)
            .map(|(_, key)| key.clone())
            .or_else(|| {
                if !use_environment {
                    return None;
                }
                env_names
                    .iter()
                    .find_map(|name| std::env::var(name).ok().filter(|v| !v.trim().is_empty()))
            });
        if host.is_none() && key.is_none() {
            continue;
        }
        out.push(NativeAccount {
            agent: "opencode".into(),
            provider: id.to_string(),
            label: label.to_string(),
            host: host.unwrap_or_default(),
            env_names,
            key,
        });
    }
    out
}

fn discover_at(home: &Path, use_environment: bool) -> Discovery {
    let mut out = Discovery::default();
    let store_path = home.join(".marionette/credentials.json");
    if let Ok(store) = crate::credentials::load_store_at(&store_path) {
        for entry in &store.entries {
            if let Some(config) = &entry.custom_provider {
                let key = crate::credentials::peek_secret_at(&store_path, &entry.id)
                    .ok()
                    .flatten();
                let models = config
                    .models
                    .iter()
                    .map(|m| (m.clone(), m.clone(), m.clone()))
                    .collect();
                out.add(
                    &store_path,
                    "Marionette",
                    &entry.id,
                    &config.endpoint,
                    &config.protocol,
                    key.as_deref().unwrap_or(""),
                    BTreeMap::new(),
                    models,
                );
                let origin = format!("Marionette · {}#{}", store_path.display(), entry.id);
                if let Some(source) = out
                    .sources
                    .iter_mut()
                    .find(|s| s.view.origins.contains(&origin))
                {
                    source.view.label = entry.label.clone();
                    source.view.issue = if entry.secret.is_some() && key.is_none() {
                        Some(crate::ts!(
                            "无法解密此来源的 API key",
                            "Could not decrypt this source's API key"
                        ))
                    } else {
                        None
                    };
                }
            }
        }
    }
    let grok = std::env::var_os("GROK_HOME")
        .filter(|_| use_environment)
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".grok"))
        .join("config.toml");
    if let Some(dir) = grok.parent() {
        let _ = out.read(&dir.join("auth.json"), false);
    }
    if let Some(v) = out.read(&grok, true) {
        if let Some(models) = v.get("model").and_then(Value::as_object) {
            for (alias, m) in models {
                let env = text(m, &["env_key"]);
                let auth = if env.is_empty() {
                    text(m, &["api_key"]).into()
                } else {
                    format!("${env}")
                };
                let backend = text(m, &["api_backend"]);
                let protocol = match backend {
                    "" | "chat_completions" => "chat",
                    "responses" => "responses",
                    // grok spells the Anthropic wire protocol "messages"
                    // (its api_backend vocabulary), not "anthropic".
                    "anthropic" | "messages" => "anthropic",
                    _ => {
                        out.warnings
                            .push(crate::ts!("Grok 模型 {alias} 的协议 {backend} 暂未接入共享", "Protocol {backend} of Grok model {alias} is not yet supported for sharing"));
                        continue;
                    }
                };
                let model = text(m, &["model"]);
                let label = text(m, &["name"]);
                out.add(
                    &grok,
                    "grok-build",
                    alias,
                    text(m, &["base_url"]),
                    protocol,
                    &auth,
                    string_map(m.get("extra_headers")),
                    vec![(
                        model.into(),
                        if label.is_empty() {
                            model.into()
                        } else {
                            label.into()
                        },
                        alias.clone(),
                    )],
                );
            }
        }
    }
    let pi = std::env::var_os("PI_CODING_AGENT_DIR")
        .filter(|_| use_environment)
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".pi/agent"))
        .join("models.json");
    if let Some(v) = out.read(&pi, false) {
        if let Some(providers) = v.get("providers").and_then(Value::as_object) {
            for (name, p) in providers {
                let rows = p
                    .get("models")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_else(|| vec![Value::Null]);
                for m in rows {
                    let api = text(&m, &["api"]);
                    let api = if api.is_empty() {
                        text(p, &["api"])
                    } else {
                        api
                    };
                    let protocol = match api {
                        "" | "openai-completions" => "chat",
                        "openai-responses" => "responses",
                        "anthropic-messages" => "anthropic",
                        _ => {
                            out.warnings
                                .push(crate::ts!("Pi provider {name} 的协议 {api} 暂未接入共享", "Protocol {api} of Pi provider {name} is not yet supported for sharing"));
                            continue;
                        }
                    };
                    let id = text(&m, &["id"]);
                    let label = text(&m, &["name", "id"]);
                    let endpoint = text(&m, &["baseUrl"]);
                    let endpoint = if endpoint.is_empty() {
                        text(p, &["baseUrl"])
                    } else {
                        endpoint
                    };
                    let mut headers = string_map(p.get("headers"));
                    headers.extend(string_map(m.get("headers")));
                    out.add(
                        &pi,
                        "pi",
                        name,
                        endpoint,
                        protocol,
                        text(p, &["apiKey"]),
                        headers,
                        vec![(id.into(), label.into(), format!("{name}/{id}"))],
                    );
                }
            }
        }
    }
    // OpenCode custom providers: endpoint and model definitions live separately
    // from auth.json. Preserve that relationship instead of guessing a preset.
    let opencode_dir = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|_| use_environment)
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
        .join("opencode");
    let auth_path = if use_environment {
        crate::provider_usage::opencode_auth_path()
    } else {
        None
    }
    .unwrap_or_else(|| home.join(".local/share/opencode/auth.json"));
    let auth = out.read(&auth_path, false).unwrap_or(Value::Null);
    let mut opencode_paths = vec![
        opencode_dir.join("opencode.json"),
        opencode_dir.join("opencode.jsonc"),
    ];
    if use_environment {
        if let Some(path) = std::env::var_os("OPENCODE_CONFIG") {
            let path = PathBuf::from(path);
            if !opencode_paths.contains(&path) {
                opencode_paths.push(path);
            }
        }
    }
    for path in opencode_paths {
        if let Some(v) = out.read(&path, false) {
            if let Some(providers) = v.get("provider").and_then(Value::as_object) {
                for (name, p) in providers {
                    let options = &p["options"];
                    let api_key = text(options, &["apiKey"]);
                    let api_key = if api_key.is_empty() {
                        text(&auth[name], &["key"])
                    } else {
                        api_key
                    };
                    let npm = text(p, &["npm"]);
                    let protocol = match npm {
                        "@ai-sdk/anthropic" => "anthropic",
                        "@ai-sdk/openai" => "responses",
                        "" | "@ai-sdk/openai-compatible" => "chat",
                        _ => {
                            out.warnings.push(crate::ts!(
                                "OpenCode provider {name} 的 SDK {npm} 暂未接入共享",
                                "SDK {npm} of OpenCode provider {name} is not yet supported for sharing"
                            ));
                            continue;
                        }
                    };
                    let models = p
                        .get("models")
                        .and_then(Value::as_object)
                        .map(|ms| {
                            ms.iter()
                                .map(|(id, m)| {
                                    (
                                        id.clone(),
                                        text(m, &["name"]).to_string(),
                                        format!("{name}/{id}"),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    out.add(
                        &path,
                        "opencode",
                        name,
                        text(options, &["baseURL"]),
                        protocol,
                        api_key,
                        string_map(options.get("headers")),
                        models,
                    );
                }
            }
        }
    }
    let codex = std::env::var_os("CODEX_HOME")
        .filter(|_| use_environment)
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"))
        .join("config.toml");
    if let Some(dir) = codex.parent() {
        let _ = out.read(&dir.join("auth.json"), false);
    }
    if let Some(v) = out.read(&codex, true) {
        if let Some(providers) = v.get("model_providers").and_then(Value::as_object) {
            for (name, p) in providers {
                let env = text(p, &["env_key"]);
                let key = if env.is_empty() {
                    text(p, &["experimental_bearer_token"]).into()
                } else {
                    format!("${env}")
                };
                let models = if text(&v, &["model_provider"]) == name {
                    let m = text(&v, &["model"]);
                    vec![(m.into(), m.into(), m.into())]
                } else {
                    vec![]
                };
                out.add(
                    &codex,
                    "codex",
                    name,
                    text(p, &["base_url"]),
                    if text(p, &["wire_api"]) == "chat" {
                        "chat"
                    } else {
                        "responses"
                    },
                    &key,
                    string_map(p.get("http_headers")),
                    models,
                );
            }
        }
    }
    let claude_dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|_| use_environment)
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".claude"));
    let claude = claude_dir.join("settings.json");
    let _ = out.read(&claude_dir.join(".credentials.json"), false);
    if let Some(v) = out.read(&claude, false) {
        let env = &v["env"];
        let key = text(env, &["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"]);
        let key = if key.is_empty() {
            "$ANTHROPIC_API_KEY"
        } else {
            key
        };
        let models = [
            "ANTHROPIC_MODEL",
            "ANTHROPIC_DEFAULT_OPUS_MODEL",
            "ANTHROPIC_DEFAULT_SONNET_MODEL",
            "ANTHROPIC_DEFAULT_HAIKU_MODEL",
        ]
        .iter()
        .filter_map(|k| env[*k].as_str())
        .map(|m| (m.into(), m.into(), m.into()))
        .collect();
        out.add(
            &claude,
            "claude-code",
            "custom",
            text(env, &["ANTHROPIC_BASE_URL"]),
            "anthropic",
            key,
            BTreeMap::new(),
            models,
        );
    }
    mark_native_accounts(&mut out, home, use_environment);
    out.accounts = local_signin_accounts(home, use_environment);
    out
}

fn read_json(path: &Path) -> Option<Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
}

/// ChatGPT account a Codex-style sign-in names. Codex keeps it under `tokens`;
/// OpenCode keeps it under `metadata` beside its own OAuth token.
fn chatgpt_account(value: &Value) -> Option<String> {
    value
        .get("tokens")
        .and_then(|tokens| tokens.get("account_id"))
        .or_else(|| value.get("metadata").and_then(|meta| meta.get("accountId")))
        .or_else(|| {
            value
                .get("metadata")
                .and_then(|meta| meta.get("account_id"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
}

/// 凭据文件里的 ChatGPT 登录在不在（发现链已有的那份判断）。给 magpie 回退 SKU 的
/// 「订阅共享」面板用——Rust SKU 走 `engine_gateway::signed_in_families`（四族都认）。
#[cfg(not(engine_rust_active))]
pub fn codex_signed_in() -> bool {
    let Some(home) = home() else { return false };
    local_signin_accounts(&home, true)
        .iter()
        .any(|account| account.kind == "owner")
}

/// One subscription sign-in is one account. The engine provider that runs it
/// and the agent CLI that owns the same login are the same menu group; a second
/// agent holding a copied sign-in of that account is a duplicate listing. Both
/// facts come from the login files, never from model names.
fn local_signin_accounts(home: &Path, use_environment: bool) -> Vec<EngineAccount> {
    let mut out = Vec::new();
    let codex_dir = std::env::var_os("CODEX_HOME")
        .filter(|_| use_environment)
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    let codex = read_json(&codex_dir.join("auth.json"));
    let signed_in = codex.as_ref().is_some_and(|value| {
        let mode = text(value, &["auth_mode"]);
        let token = text(&value["tokens"], &["access_token"]);
        mode != "apikey" && !token.is_empty()
    });
    let Some(account) = codex
        .as_ref()
        .and_then(chatgpt_account)
        .filter(|_| signed_in)
    else {
        return out;
    };
    // Codex CLI serves its whole catalogue from this sign-in.
    out.push(EngineAccount {
        engine: "codex".into(),
        agent: "codex".into(),
        provider: String::new(),
        kind: "owner",
    });
    let auth_path = if use_environment {
        crate::provider_usage::opencode_auth_path()
    } else {
        None
    }
    .unwrap_or_else(|| home.join(".local/share/opencode/auth.json"));
    if let Some(auth) = read_json(&auth_path).filter(|value| value.is_object()) {
        for (provider, entry) in auth.as_object().into_iter().flatten() {
            // An API key is a separate credential; only a copied OAuth sign-in
            // can be the same ChatGPT account.
            if entry.get("type").and_then(Value::as_str) != Some("oauth") {
                continue;
            }
            if chatgpt_account(entry).as_deref() != Some(account.as_str()) {
                continue;
            }
            out.push(EngineAccount {
                engine: "codex".into(),
                agent: "opencode".into(),
                provider: provider.clone(),
                kind: "duplicate",
            });
        }
    }
    out
}

/// Attach "this agent already has this account" facts to each source. A native
/// provider counts as the same account only when the endpoint host matches and
/// the credential is provably the same: an identical resolved key, or the same
/// environment variable name on both sides. Matching on a model name alone is
/// deliberately refused — two vendors ship `deepseek-flash`.
fn mark_native_accounts(out: &mut Discovery, home: &Path, use_environment: bool) {
    let native = opencode_native_accounts(home, use_environment);
    if native.is_empty() {
        return;
    }
    for source in &mut out.sources {
        let Some(source_host) = endpoint_host(&source.view.endpoint) else {
            continue;
        };
        for account in &native {
            if account.host != source_host {
                continue;
            }
            let same_key = match (&account.key, source.key.is_empty()) {
                (Some(key), false) => key == &source.key,
                _ => false,
            };
            let same_env = account
                .env_names
                .iter()
                .any(|name| source.env_names.iter().any(|own| own == name));
            if !same_key && !same_env {
                continue;
            }
            let providers = source
                .view
                .native_accounts
                .entry(account.agent.clone())
                .or_default();
            if !providers.iter().any(|p| p.provider == account.provider) {
                providers.push(NativeAccountRef {
                    provider: account.provider.clone(),
                    label: account.label.clone(),
                });
            }
        }
    }
}

fn prefs_path() -> Result<PathBuf, String> {
    Ok(crate::app_paths::global_dir()?.join("model-library.json"))
}

pub fn load_preferences() -> Result<Value, String> {
    let path = prefs_path()?;
    match fs::read_to_string(path) {
        Ok(s) => serde_json::from_str(&s).map_err(|_| {
            crate::ts!(
                "模型显示设置损坏，请恢复默认后保存",
                "Model display preferences are corrupted; restore defaults and save again"
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(e) => Err(crate::ts!(
            "读取模型显示设置失败：{e}",
            "Failed to read model display preferences: {e}"
        )),
    }
}

pub fn save_preferences(preferences: Value) -> Result<(), String> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK.lock().map_err(|_| {
        crate::ts!(
            "模型显示设置正在保存",
            "Model display preferences are being saved"
        )
    })?;
    if !preferences.is_object() {
        return Err(crate::ts!(
            "模型显示设置格式无效",
            "Invalid model display preferences format"
        ));
    }
    let path = prefs_path()?;
    let temp = path.with_extension("json.tmp");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(
        &temp,
        serde_json::to_vec_pretty(&preferences).map_err(|e| e.to_string())?,
    )
    .and_then(|_| fs::rename(temp, path))
    .map_err(|e| {
        crate::ts!(
            "保存模型显示设置失败：{e}",
            "Failed to save model display preferences: {e}"
        )
    })
}

fn clean_jsonc(input: &str) -> String {
    let stripped = crate::context_inventory::strip_jsonc_comments(input);
    let chars: Vec<char> = stripped.chars().collect();
    let mut out = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for (i, ch) in chars.iter().copied().enumerate() {
        if quoted {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
        } else if ch == '"' {
            quoted = true;
        } else if ch == ','
            && matches!(
                chars[i + 1..].iter().find(|c| !c.is_whitespace()),
                Some('}') | Some(']')
            )
        {
            continue;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_grok_models_and_keeps_secrets_out_of_views() {
        let root = std::env::temp_dir().join(format!(
            "marionette-sources-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join(".grok")).unwrap();
        let path = root.join(".grok/config.toml");
        fs::write(
            &path,
            r#"
[models]
default = "ollama-flash"
[model."ollama-flash"]
model = "deepseek-v4.1-flash:1017"
name = "Flash on Ollama"
base_url = "https://ollama.com/v1"
api_key = "sk-fixture-must-stay-private"
[model."marionette/injected"]
model = "other/m"
base_url = "http://127.0.0.1:3425/v1"
api_key = "magpie"
"#,
        )
        .unwrap();
        let discovery = discover_at(&root, false);
        assert_eq!(discovery.sources.len(), 1);
        let source = &discovery.sources[0];
        assert_eq!(source.key, "sk-fixture-must-stay-private");
        assert_eq!(source.view.models[0].id, "deepseek-v4.1-flash:1017");
        assert_eq!(
            source.view.models[0].native_ids["grok-build"],
            vec!["ollama-flash"]
        );
        let payload = serde_json::to_string(&source.view).unwrap();
        assert!(!payload.contains("sk-fixture"));
        assert!(discovery
            .paths
            .iter()
            .any(|p| Path::new(&p.path) == path && p.status == "found"));
        assert!(discovery.paths.iter().any(|p| p.status == "missing"));
        fs::write(&path, "invalid TOML [").unwrap();
        let broken = discover_at(&root, false);
        assert!(broken.sources.is_empty());
        assert!(broken.paths.iter().any(|p| p.status == "error"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shares_one_env_provider_and_preserves_distinct_endpoints() {
        let mut out = Discovery::default();
        for (route, endpoint) in [
            ("first", "https://one.example/v1"),
            ("second", "https://one.example/v1"),
            ("third", "https://two.example/v1"),
        ] {
            out.add(
                Path::new("config.toml"),
                "grok-build",
                route,
                endpoint,
                "chat",
                "${MARIONETTE_TEST_MISSING_SECRET_470182}",
                BTreeMap::new(),
                vec![(route.into(), route.into(), route.into())],
            );
        }
        assert_eq!(out.sources.len(), 2);
        assert_eq!(out.sources[0].view.models.len(), 2);
        assert!(out.sources[0].view.issue.is_some());
        assert!(out.sources[0].key.is_empty());
    }

    #[test]
    fn jsonc_preserves_urls_and_commas_in_strings() {
        let v: Value = serde_json::from_str(&clean_jsonc(
            r#"{ // comment
          "url":"https://example.com/a,}", "models":["中文",], /* comment */
        }"#,
        ))
        .unwrap();
        assert_eq!(v["url"], "https://example.com/a,}");
        assert_eq!(v["models"][0], "中文");
    }

    /// The same account configured in two agents must collapse onto one source.
    /// Identity = same host + provably the same credential (resolved key, or the
    /// same environment variable). A matching model name alone never merges.
    #[test]
    fn native_provider_with_same_host_and_key_joins_the_source() {
        let root = std::env::temp_dir().join(format!(
            "marionette-native-account-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join(".grok")).unwrap();
        fs::create_dir_all(root.join(".cache/opencode")).unwrap();
        fs::create_dir_all(root.join(".local/share/opencode")).unwrap();
        // Grok's own table for the official DeepSeek endpoint.
        fs::write(
            root.join(".grok/config.toml"),
            r#"
[model."deepseek-v4.1-flash"]
model = "deepseek-flash"
name = "DeepSeek V4.1 Flash (官方)"
base_url = "https://api.deepseek.com/v1"
env_key = "MARIONETTE_TEST_DEEPSEEK_KEY"
"#,
        )
        .unwrap();
        // OpenCode's cached provider catalogue + auth.json: same vendor, key in
        // auth.json rather than the environment.
        fs::write(
            root.join(".cache/opencode/models.json"),
            r#"{
  "deepseek": {"name": "DeepSeek", "api": "https://api.deepseek.com", "env": ["DEEPSEEK_API_KEY"]},
  "ollama-cloud": {"name": "Ollama Cloud", "api": "https://ollama.com/v1", "env": ["OLLAMA_API_KEY"]}
}"#,
        )
        .unwrap();
        fs::write(
            root.join(".local/share/opencode/auth.json"),
            r#"{"deepseek": {"type": "api", "key": "sk-shared-account"}}"#,
        )
        .unwrap();
        std::env::set_var("MARIONETTE_TEST_DEEPSEEK_KEY", "sk-shared-account");

        let discovery = discover_at(&root, true);
        let deepseek = &discovery.sources[0];
        assert_eq!(deepseek.view.endpoint, "https://api.deepseek.com/v1");
        let merged = &deepseek.view.native_accounts["opencode"];
        assert_eq!(merged.len(), 1, "only the matching account joins");
        assert_eq!(merged[0].provider, "deepseek");
        assert_eq!(merged[0].label, "DeepSeek");
        // No Ollama source in this fixture → nothing else can claim an account.
        assert!(!deepseek.view.native_accounts.contains_key("grok-build"));

        // Different key on the same host is a different account: no merge.
        std::env::set_var("MARIONETTE_TEST_DEEPSEEK_KEY", "sk-another-account");
        let other = discover_at(&root, true);
        assert!(other.sources[0].view.native_accounts.is_empty());

        std::env::remove_var("MARIONETTE_TEST_DEEPSEEK_KEY");
        fs::remove_dir_all(root).unwrap();
    }

    /// Host comparison ignores the path shape vendors publish (`…/v1` or bare).
    #[test]
    fn endpoint_host_ignores_path_and_scheme() {
        assert_eq!(
            endpoint_host("https://api.deepseek.com/v1").as_deref(),
            Some("api.deepseek.com")
        );
        assert_eq!(
            endpoint_host("https://API.DeepSeek.com").as_deref(),
            Some("api.deepseek.com")
        );
        assert_eq!(
            endpoint_host("http://127.0.0.1:3425/v1").as_deref(),
            Some("127.0.0.1:3425")
        );
        assert_eq!(endpoint_host("not-a-url"), None);
    }

    /// Grok writes `/inference/go/openai/v1` where the preset publishes
    /// `/inference/go/v1`; same vendor account, different path.
    #[test]
    fn same_vendor_host_ignores_path_but_not_vendor() {
        assert!(same_vendor_host(
            "https://opencode.ai/inference/go/openai/v1",
            Some("https://opencode.ai/inference/go/v1"),
            Some("https://opencode.ai/inference/go"),
        ));
        assert!(same_vendor_host(
            "https://api.deepseek.com/v1",
            Some("https://api.deepseek.com/v1"),
            None,
        ));
        assert!(!same_vendor_host(
            "https://api.deepseek.com/v1",
            Some("https://opencode.ai/inference/go/v1"),
            None,
        ));
        assert!(!same_vendor_host(
            "not-a-url",
            Some("https://api.deepseek.com/v1"),
            None
        ));
    }

    /// One ChatGPT sign-in, read from wherever each agent keeps its copy: Codex
    /// CLI in `~/.codex/auth.json`, OpenCode under `metadata.accountId`. The
    /// engine provider that runs the login owns the group; the copy is a
    /// duplicate listing, never a second account.
    #[test]
    fn a_chatgpt_sign_in_is_one_account_across_agents() {
        let root = std::env::temp_dir().join(format!(
            "marionette-signin-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join(".codex")).unwrap();
        fs::create_dir_all(root.join(".local/share/opencode")).unwrap();
        fs::write(
            root.join(".codex/auth.json"),
            r#"{"auth_mode":"chatgpt","tokens":{"access_token":"at","account_id":"acct-1"}}"#,
        )
        .unwrap();
        fs::write(
            root.join(".local/share/opencode/auth.json"),
            r#"{"openai":{"type":"oauth","access":"at","metadata":{"accountId":"acct-1"}},
                "copilot":{"type":"oauth","access":"other","metadata":{"accountId":"acct-2"}},
                "opencode-go":{"type":"api","key":"sk-not-a-login"}}"#,
        )
        .unwrap();

        let discovery = discover_at(&root, false);
        let owner = discovery
            .accounts
            .iter()
            .find(|a| a.kind == "owner")
            .unwrap();
        assert_eq!(
            (
                owner.engine.as_str(),
                owner.agent.as_str(),
                owner.provider.as_str()
            ),
            ("codex", "codex", "")
        );
        let copies: Vec<&str> = discovery
            .accounts
            .iter()
            .filter(|a| a.kind == "duplicate")
            .map(|a| a.provider.as_str())
            .collect();
        assert_eq!(
            copies,
            vec!["openai"],
            "only the same ChatGPT account is a copy"
        );

        // An API key is a separate credential, and a different account id is a
        // different login: neither is folded into the engine group.
        fs::write(
            root.join(".local/share/opencode/auth.json"),
            r#"{"openai":{"type":"api","key":"sk-openai"}}"#,
        )
        .unwrap();
        assert!(discover_at(&root, false)
            .accounts
            .iter()
            .all(|a| a.kind == "owner"));

        // Signed out of Codex: no engine account, so nothing folds.
        fs::write(
            root.join(".codex/auth.json"),
            r#"{"auth_mode":"chatgpt","tokens":{}}"#,
        )
        .unwrap();
        assert!(discover_at(&root, false).accounts.is_empty());

        fs::remove_dir_all(root).unwrap();
    }

    // ── 自定义来源的目录预览（probe_custom）────────────────────────────────

    /// 回环假目录服务：按请求路径回一条 HTTP 应答，把收到的请求头（首行 +
    /// 鉴权相关头）交回调用方判定（靠前的路由先匹配，没命中的路径回 404）。`routes` = (路径, "状态行\x0d\x0a头…\x0d\x0a\x0d\x0a体")。
    fn catalog_server(
        routes: Vec<(&'static str, &'static str)>,
    ) -> (u16, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).unwrap_or(0) != 1 {
                        break;
                    }
                    head.push(byte[0]);
                }
                let text = String::from_utf8_lossy(&head).into_owned();
                let path = text
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("")
                    .to_string();
                let auth: Vec<String> = text
                    .lines()
                    .filter(|line| {
                        let lower = line.to_ascii_lowercase();
                        lower.starts_with("authorization:")
                            || lower.starts_with("x-api-key:")
                            || lower.starts_with("anthropic-version:")
                    })
                    .map(str::to_string)
                    .collect();
                let response = routes
                    .iter()
                    .find(|(prefix, _)| path.starts_with(prefix))
                    .map(|(_, response)| *response)
                    .unwrap_or("HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                let _ = tx.send(format!("{}\n{}", path, auth.join("\n")));
                let _ = stream.write_all(response.as_bytes());
            }
        });
        (port, rx)
    }

    fn probe_text_is(detail: &str, zh: &str, en: &str) -> bool {
        detail.contains(zh) || detail.contains(en)
    }

    /// OpenAI 形状：`data[].id`，Bearer 鉴权；结尾的 `/` 归一掉；重复/空 id 丢掉、保序。
    #[test]
    fn custom_probe_lists_an_openai_shaped_catalog_with_bearer_auth() {
        let (port, seen) = catalog_server(vec![(
            "/v1/models",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"object\":\"list\",\"data\":[{\"id\":\"b\",\"owned_by\":\"x\"},{\"id\":\"a\"},{\"id\":\"a\"},{\"id\":\"  \"},{\"no_id\":1}]}",
        )]);
        let probe = probe_custom_with(
            &format!("http://127.0.0.1:{port}/v1/"),
            "chat",
            "sk-test",
            None,
        );
        assert!(probe.ok, "{}", probe.detail);
        assert_eq!(
            probe
                .models
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["b", "a"]
        );
        assert!(probe.models.iter().all(|m| m.label.is_empty()));
        let request = seen.recv().unwrap();
        assert!(
            request.starts_with("/v1/models\n"),
            "trailing slash normalised: {request}"
        );
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer sk-test"),
            "{request}"
        );
    }

    /// Anthropic 目录：`x-api-key` + 版本头，不用 Bearer；`has_more` + `last_id`
    /// 翻页取全；`display_name` 进 label（与 id 相同则留空）。
    #[test]
    fn custom_probe_follows_anthropic_pagination_with_its_own_headers() {
        let (port, seen) = catalog_server(vec![
            (
                "/v1/models?after_id=claude-b",
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"data\":[{\"id\":\"claude-c\",\"display_name\":\"Claude C\"}],\"has_more\":false,\"last_id\":\"claude-c\"}",
            ),
            (
                "/v1/models",
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"data\":[{\"id\":\"claude-a\",\"display_name\":\"Claude A\"},{\"id\":\"claude-b\",\"display_name\":\"claude-b\"}],\"has_more\":true,\"last_id\":\"claude-b\"}",
            ),
        ]);
        let probe = probe_custom_with(
            &format!("http://127.0.0.1:{port}/v1"),
            "anthropic",
            "sk-ant",
            None,
        );
        assert!(probe.ok, "{}", probe.detail);
        let models: Vec<(&str, &str)> = probe
            .models
            .iter()
            .map(|m| (m.id.as_str(), m.label.as_str()))
            .collect();
        let first = seen.recv().unwrap();
        assert_eq!(
            models,
            vec![
                ("claude-a", "Claude A"),
                ("claude-b", ""),
                ("claude-c", "Claude C")
            ]
        );
        assert!(first.starts_with("/v1/models\n"), "{first}");
        let lower = first.to_ascii_lowercase();
        assert!(
            lower.contains("x-api-key: sk-ant") && lower.contains("anthropic-version: 2023-06-01"),
            "{first}"
        );
        assert!(
            !lower.contains("authorization"),
            "no Bearer on an Anthropic catalogue: {first}"
        );
        assert!(seen
            .recv()
            .unwrap()
            .starts_with("/v1/models?after_id=claude-b\n"));
        // 最后一页 has_more=false：不再往下要。
        assert!(
            seen.recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "no third request"
        );
    }

    /// 服务端分页坏了（一直回同一页 + has_more=true）：翻一页发现游标没动就停，
    /// 不对着坏分页空转到页数上限。
    #[test]
    fn custom_probe_stops_when_the_catalog_cursor_does_not_advance() {
        let (port, seen) = catalog_server(vec![(
            "/v1/models",
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"data\":[{\"id\":\"m\"}],\"has_more\":true,\"last_id\":\"m\"}",
        )]);
        let probe = probe_custom_with(
            &format!("http://127.0.0.1:{port}/v1"),
            "anthropic",
            "k",
            None,
        );
        assert!(probe.ok, "{}", probe.detail);
        assert_eq!(
            probe
                .models
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["m"]
        );
        assert!(seen.recv().unwrap().starts_with("/v1/models\n"));
        assert!(seen.recv().unwrap().starts_with("/v1/models?after_id=m\n"));
        assert!(
            seen.recv_timeout(std::time::Duration::from_millis(300))
                .is_err(),
            "a stuck cursor must not be requested a third time"
        );
    }

    /// 失败要翻成能照着做的结论，而不是裸状态码；`ok` 恒为 false、模型恒为空。
    #[test]
    fn custom_probe_turns_failures_into_actionable_results() {
        let cases: Vec<(&'static str, &'static str, &'static str, &'static str)> = vec![
            ("HTTP 401", "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n", "密钥被拒绝", "rejected"),
            ("HTTP 404", "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n", "手动填写", "by hand"),
            ("HTML page", "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n<html>sign in</html>", "不是模型目录", "did not return a model catalogue"),
            ("empty list", "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{\"data\":[]}", "空的", "empty"),
            ("HTTP 500", "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\n\r\n", "HTTP 500", "HTTP 500"),
        ];
        for (name, response, zh, en) in cases {
            let (port, _seen) = catalog_server(vec![("/v1/models", response)]);
            let probe = probe_custom_with(
                &format!("http://127.0.0.1:{port}/v1"),
                "responses",
                "sk-x",
                None,
            );
            assert!(!probe.ok, "{name}");
            assert!(probe.models.is_empty(), "{name}");
            assert!(
                probe_text_is(&probe.detail, zh, en),
                "{name}: {}",
                probe.detail
            );
        }
    }

    /// 地址校验先于联网，与保存同一套规则；连不上的端口如实报连接失败。
    #[test]
    fn custom_probe_rejects_bad_endpoints_before_dialing() {
        for endpoint in [
            "ftp://example.com/v1",
            "http://user:pw@example.com/v1",
            "example.com/v1",
            "https://example.com/v1?x=1",
            "https://example.com/v1#frag",
            "",
        ] {
            let probe = probe_custom_with(endpoint, "chat", "sk", None);
            assert!(!probe.ok && probe.models.is_empty(), "{endpoint}");
            assert!(
                probe_text_is(
                    &probe.detail,
                    "有效的 http/https 接口地址",
                    "valid http/https endpoint"
                ),
                "{endpoint}: {}",
                probe.detail
            );
        }
        let closed = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let probe = probe_custom_with(&format!("http://127.0.0.1:{closed}/v1"), "chat", "", None);
        assert!(!probe.ok);
        assert!(
            probe_text_is(
                &probe.detail,
                "连接失败或超时",
                "Could not connect or timed out"
            ),
            "{}",
            probe.detail
        );
    }

    #[test]
    fn percent_encode_keeps_url_safe_characters() {
        assert_eq!(percent_encode("claude-a_b.c~d"), "claude-a_b.c~d");
        assert_eq!(percent_encode("a b/c+d"), "a%20b%2Fc%2Bd");
    }

    /// 手动自定义来源按 origin 认条目 id，**不是**按 `custom-` 前缀——发现来源的
    /// `view.id` 是 `source-<摘要>`（2026-11-26：引擎的钉清单判定与凭据页的「编辑」
    /// 都靠它，按前缀写的话永远不命中）。
    #[test]
    fn manual_source_is_recognised_by_origin_not_by_id_prefix() {
        let root = std::env::temp_dir().join(format!(
            "marionette_test_manual_source_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".marionette")).unwrap();
        let entry_id = crate::credentials::add_custom_provider_at(
            &root.join(".marionette").join("credentials.json"),
            "Relay",
            "sk-x",
            crate::credentials::CustomProviderConfig {
                endpoint: "https://relay.example/v1".into(),
                protocol: "chat".into(),
                models: vec!["m1".into()],
            },
        )
        .unwrap();
        assert!(
            entry_id.starts_with("custom-"),
            "credential entry ids do carry the prefix"
        );
        // 同一个 home 里再放一个从 agent 配置文件发现的来源。
        std::fs::create_dir_all(root.join(".grok")).unwrap();
        std::fs::write(
            root.join(".grok/config.toml"),
            "[model.\"deepseek-flash\"]\nmodel = \"deepseek-flash\"\nbase_url = \"https://api.deepseek.com/v1\"\napi_key = \"sk-y\"\n",
        )
        .unwrap();

        let discovery = discover_at(&root, false);
        let manual = discovery
            .sources
            .iter()
            .find(|s| s.manual_entry_id().is_some())
            .expect("the custom source is discovered");
        assert_eq!(manual.manual_entry_id(), Some(entry_id.as_str()));
        assert!(
            !manual.view.id.starts_with("custom-"),
            "a source id is a hash of its route, never the entry id"
        );
        assert_eq!(
            manual
                .view
                .models
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            vec!["m1"]
        );
        let discovered: Vec<&Source> = discovery
            .sources
            .iter()
            .filter(|s| s.manual_entry_id().is_none())
            .collect();
        assert!(
            discovered
                .iter()
                .any(|s| s.view.origins.iter().any(|o| o.starts_with("grok-build"))),
            "the grok entry is the non-manual one"
        );
        assert!(discovered.iter().all(|s| s
            .view
            .origins
            .iter()
            .all(|o| !o.starts_with("Marionette · "))));
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Save only an unresolved credential reference advertised by discovery.
pub fn set_environment(source_id: &str, name: &str, value: &str) -> Result<(), String> {
    if name.is_empty()
        || !name
            .bytes()
            .enumerate()
            .all(|(i, b)| b.is_ascii_alphabetic() || b == b'_' || (i > 0 && b.is_ascii_digit()))
    {
        return Err(crate::ts!(
            "环境变量名称无效",
            "Invalid environment variable name"
        ));
    }
    if value.trim().is_empty() || value.contains('\0') {
        return Err(crate::ts!(
            "请输入有效的环境变量值",
            "Enter a valid environment variable value"
        ));
    }
    let discovery = discover();
    if !discovery.sources.iter().any(|source| {
        source.view.id == source_id && source.view.missing_env.iter().any(|n| n == name)
    }) {
        return Err(crate::ts!(
            "该来源不再缺少此环境变量，请重新扫描",
            "This source no longer misses that environment variable; re-scan"
        ));
    }
    #[cfg(windows)]
    {
        crate::shell_integration::reg_set_string("Environment", Some(name), value).map_err(
            |_| {
                crate::ts!(
                    "无法保存 Windows 用户环境变量",
                    "Could not save the Windows user environment variable"
                )
            },
        )?;
        std::env::set_var(name, value);
        // Notify Explorer so subsequently launched applications inherit it.
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
        };
        let section: Vec<u16> = "Environment\0".encode_utf16().collect();
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                section.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                1000,
                std::ptr::null_mut(),
            );
        }
        Ok(())
    }
    #[cfg(not(windows))]
    Err(crate::ts!(
        "此功能目前仅支持 Windows 用户环境变量",
        "This feature currently supports only Windows user environment variables"
    ))
}

#[cfg(test)]
mod scan_oracle {
    /// 只读诊断：打印模型设置左列的来源侧组成（发现的来源 / 别名 / 引擎目录分组 /
    /// 重复 provider 的别名），用来解释「重叠的来源」。不打印任何密钥。
    /// `cargo test --bin marionette model_library_scan_oracle -- --ignored --nocapture`
    #[test]
    #[ignore = "reads this machine's real agent configs and engine catalog"]
    fn model_library_scan_oracle() {
        let discovery = super::discover();
        println!("sources: {}", discovery.sources.len());
        for source in &discovery.sources {
            println!(
                "  source id={} label={:?} endpoint={:?} models={} origins={:?} nativeAccounts={:?}",
                source.view.id,
                source.view.label,
                source.view.endpoint,
                source.view.models.len(),
                source.view.origins,
                source.view.native_accounts.keys().cloned().collect::<Vec<_>>()
            );
        }
        let aliases = crate::engine::model_source_aliases(&discovery.sources);
        println!("source aliases: {aliases:?}");
        let home = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from);
        if let Some(home) = home {
            if let Ok(bytes) = std::fs::read(home.join(".marionette").join("engine-catalog.json")) {
                if let Ok(file) = serde_json::from_slice::<serde_json::Value>(&bytes) {
                    let mut groups: std::collections::BTreeMap<String, usize> = Default::default();
                    for row in file
                        .get("rows")
                        .and_then(|r| r.as_array())
                        .cloned()
                        .unwrap_or_default()
                    {
                        let id = row.get("id").and_then(|i| i.as_str()).unwrap_or("");
                        let provider = id.split('/').next().unwrap_or("").to_string();
                        *groups.entry(provider).or_default() += 1;
                    }
                    println!("engine catalog cache by provider: {groups:?}");
                }
            }
            if let Ok(bytes) = std::fs::read(
                home.join(".marionette")
                    .join("engine")
                    .join("data")
                    .join("providers.json"),
            ) {
                if let Ok(file) =
                    serde_json::from_slice::<crate::engine_gateway::store::ProvidersFile>(&bytes)
                {
                    let aliases = crate::engine::provider_account_aliases_for(
                        &file.providers,
                        &discovery.sources,
                    );
                    println!("provider account aliases: {aliases:?}");
                }
            }
        }
    }
}
