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

/// Lightweight authenticated catalogue request; never sends a completion.
pub fn check_source(id: &str) -> Value {
    let result = |state: &str, detail: &str| serde_json::json!({ "state": state, "detail": detail });
    let Some(source) = discover().sources.into_iter().find(|source| source.view.id == id) else {
        return result(&crate::ts!("配置已变化", "Configuration changed"), &crate::ts!("请重新扫描", "Re-scan required"));
    };
    if let Some(issue) = &source.view.issue {
        return result(&crate::ts!("配置待处理", "Configuration needs attention"), issue);
    }
    let Ok(connector) = ureq::native_tls::TlsConnector::new() else {
        return result(&crate::ts!("检测失败", "Check failed"), &crate::ts!("无法初始化 TLS", "Could not initialize TLS"));
    };
    let mut builder = ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(connector))
        .timeout(std::time::Duration::from_secs(8))
        .redirects(0);
    let config = crate::proxy::load_proxy_config();
    if config.enabled && !config.url.trim().is_empty() {
        let Ok(proxy) = ureq::Proxy::new(config.url.trim()) else {
            return result(&crate::ts!("配置待处理", "Configuration needs attention"), &crate::ts!("代理地址无效", "Invalid proxy URL"));
        };
        builder = builder.proxy(proxy);
    }
    let agent = builder.build();
    let url = format!("{}/models", source.view.endpoint.trim_end_matches('/'));
    let mut request = agent.get(&url);
    if !source.key.is_empty() {
        request = if source.view.protocol == "anthropic" {
            request.set("x-api-key", &source.key).set("anthropic-version", "2023-06-01")
        } else {
            request.set("Authorization", &format!("Bearer {}", source.key))
        };
    }
    for (name, value) in &source.headers {
        request = request.set(name, value);
    }
    let started = std::time::Instant::now();
    match request.call() {
        Ok(response) => {
            // Bound the body and require a catalogue, since a login HTML page can also be HTTP 200.
            let body: Result<Value, _> = serde_json::from_reader(std::io::Read::take(response.into_reader(), 1024 * 1024));
            if body.as_ref().ok().is_some_and(|body| body.get("data").is_some_and(Value::is_array) || body.get("models").is_some_and(Value::is_array)) {
                result(&crate::ts!("已连接 · {ms} ms", "Connected · {ms} ms", ms = started.elapsed().as_millis()), &crate::ts!("模型目录可访问；实际调用还取决于模型权限和余额", "Model catalogue reachable; actual calls still depend on model permissions and balance"))
            } else {
                result(&crate::ts!("待确认", "Needs confirmation"), &crate::ts!("接口有响应，但未返回模型目录", "Endpoint responded but returned no model catalogue"))
            }
        }
        Err(ureq::Error::Status(401 | 403, _)) => result(&crate::ts!("认证受限", "Auth limited"), &crate::ts!("请检查密钥和访问权限", "Check the API key and access permissions")),
        Err(ureq::Error::Status(404 | 405, _)) => result(&crate::ts!("待确认", "Needs confirmation"), &crate::ts!("服务不支持模型目录检测，可尝试实际调用", "Service does not support catalogue probing; try an actual call")),
        Err(ureq::Error::Status(status, _)) => result(&crate::ts!("检测失败", "Check failed"), &format!("HTTP {status}")),
        Err(_) => result(&crate::ts!("连接失败", "Connection failed"), &crate::ts!("连接超时或网络、TLS 异常", "Timed out or a network/TLS error")),
    }
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
    raw.strip_prefix("{env:").and_then(|s| s.strip_suffix('}'))
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
            .ok_or_else(|| crate::ts!("环境变量 {name} 不可用，请在下方填写并保存", "Environment variable {name} is unavailable; fill it in below and save"));
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
                    detail: crate::ts!("文件存在，但读取或解析失败", "File exists but could not be read or parsed"),
                });
                self.warnings
                    .push(crate::ts!("无法读取配置：{path}", "Could not read config: {path}", path = path.display()));
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
                .map_err(|_| crate::ts!("无法读取原配置引用的密钥文件", "Could not read the key file referenced by the original config"))
        } else {
            resolve(auth)
        };
        let mut missing_env: Vec<String> = std::iter::once(auth)
            .chain(headers.values().map(String::as_str))
            .filter_map(env_reference)
            .filter(|name| std::env::var(name).map(|v| v.is_empty()).unwrap_or(true))
            .map(str::to_string).collect();
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
            issue = Some(crate::ts!("未找到可共享的 API key 或环境变量引用", "No shareable API key or environment variable reference found"));
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
            if !source.view.missing_env.contains(&name) { source.view.missing_env.push(name); }
        }
        for name in &env_names {
            if !source.env_names.contains(name) { source.env_names.push(name.clone()); }
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
pub fn same_vendor_host(endpoint: &str, chat_base: Option<&str>, anthropic_base: Option<&str>) -> bool {
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
                        Some(crate::ts!("无法解密此来源的 API key", "Could not decrypt this source's API key"))
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
        .or_else(|| value.get("metadata").and_then(|meta| meta.get("account_id")))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(str::to_string)
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
    let Some(account) = codex.as_ref().and_then(chatgpt_account).filter(|_| signed_in) else {
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
        Ok(s) => serde_json::from_str(&s).map_err(|_| crate::ts!("模型显示设置损坏，请恢复默认后保存", "Model display preferences are corrupted; restore defaults and save again")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::json!({})),
        Err(e) => Err(crate::ts!("读取模型显示设置失败：{e}", "Failed to read model display preferences: {e}")),
    }
}

pub fn save_preferences(preferences: Value) -> Result<(), String> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = LOCK
        .lock()
        .map_err(|_| crate::ts!("模型显示设置正在保存", "Model display preferences are being saved"))?;
    if !preferences.is_object() {
        return Err(crate::ts!("模型显示设置格式无效", "Invalid model display preferences format"));
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
    .map_err(|e| crate::ts!("保存模型显示设置失败：{e}", "Failed to save model display preferences: {e}"))
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
        assert_eq!(endpoint_host("https://api.deepseek.com/v1").as_deref(), Some("api.deepseek.com"));
        assert_eq!(endpoint_host("https://API.DeepSeek.com").as_deref(), Some("api.deepseek.com"));
        assert_eq!(endpoint_host("http://127.0.0.1:3425/v1").as_deref(), Some("127.0.0.1:3425"));
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
        assert!(!same_vendor_host("not-a-url", Some("https://api.deepseek.com/v1"), None));
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
        let owner = discovery.accounts.iter().find(|a| a.kind == "owner").unwrap();
        assert_eq!((owner.engine.as_str(), owner.agent.as_str(), owner.provider.as_str()), ("codex", "codex", ""));
        let copies: Vec<&str> = discovery.accounts.iter().filter(|a| a.kind == "duplicate").map(|a| a.provider.as_str()).collect();
        assert_eq!(copies, vec!["openai"], "only the same ChatGPT account is a copy");

        // An API key is a separate credential, and a different account id is a
        // different login: neither is folded into the engine group.
        fs::write(
            root.join(".local/share/opencode/auth.json"),
            r#"{"openai":{"type":"api","key":"sk-openai"}}"#,
        )
        .unwrap();
        assert!(discover_at(&root, false).accounts.iter().all(|a| a.kind == "owner"));

        // Signed out of Codex: no engine account, so nothing folds.
        fs::write(root.join(".codex/auth.json"), r#"{"auth_mode":"chatgpt","tokens":{}}"#).unwrap();
        assert!(discover_at(&root, false).accounts.is_empty());

        fs::remove_dir_all(root).unwrap();
    }
}

/// Save only an unresolved credential reference advertised by discovery.
pub fn set_environment(source_id: &str, name: &str, value: &str) -> Result<(), String> {
    if name.is_empty() || !name.bytes().enumerate().all(|(i, b)| b.is_ascii_alphabetic() || b == b'_' || (i > 0 && b.is_ascii_digit())) {
        return Err(crate::ts!("环境变量名称无效", "Invalid environment variable name"));
    }
    if value.trim().is_empty() || value.contains('\0') {
        return Err(crate::ts!("请输入有效的环境变量值", "Enter a valid environment variable value"));
    }
    let discovery = discover();
    if !discovery.sources.iter().any(|source| source.view.id == source_id && source.view.missing_env.iter().any(|n| n == name)) {
        return Err(crate::ts!("该来源不再缺少此环境变量，请重新扫描", "This source no longer misses that environment variable; re-scan"));
    }
    #[cfg(windows)]
    {
        crate::shell_integration::reg_set_string("Environment", Some(name), value)
            .map_err(|_| crate::ts!("无法保存 Windows 用户环境变量", "Could not save the Windows user environment variable"))?;
        std::env::set_var(name, value);
        // Notify Explorer so subsequently launched applications inherit it.
        use windows_sys::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE};
        let section: Vec<u16> = "Environment\0".encode_utf16().collect();
        unsafe { SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, 0, section.as_ptr() as isize, SMTO_ABORTIFHUNG, 1000, std::ptr::null_mut()); }
        Ok(())
    }
    #[cfg(not(windows))]
    Err(crate::ts!("此功能目前仅支持 Windows 用户环境变量", "This feature currently supports only Windows user environment variables"))
}
