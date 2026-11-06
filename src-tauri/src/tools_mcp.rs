//! Marionette's own tool channel: a localhost HTTP MCP server inside the
//! main process (tiny_http, grok_dialect.rs shape). Every ACP session gets
//! an entry with its own bearer token — the token names the session, which is
//! how a Control request knows which conversation's permission card (or
//! always-approve mode) to consult. One channel reaches every agent: zcode's
//! session/new only accepts url-type servers, and http entries work for the
//! rest.
//!
//! Tool results follow the codeg posture: a refusal is a value
//! (`{"error": "browser_grant_required", "note": …}`), never a transport
//! error — the agent can relay "ask the user" instead of losing its turn.

use std::collections::HashMap;
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

use serde_json::{json, Value};
use tauri::AppHandle;

const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
/// Default aria-tree cap (codeg `browser_tools.rs`).
const DEFAULT_SNAPSHOT_MAX_CHARS: u64 = 40_000;
const DEFAULT_CONSOLE_LIMIT: usize = 100;

struct ServerState {
    port: u16,
    app: AppHandle,
}

static SERVER: OnceLock<ServerState> = OnceLock::new();
static TOKENS: RwLock<Option<HashMap<String, String>>> = RwLock::new(None);
static TOKEN_SEED: AtomicU64 = AtomicU64::new(0);

fn mint_token(session_id: &str) -> String {
    use sha2::{Digest, Sha256};
    let nonce = TOKEN_SEED.fetch_add(1, Ordering::Relaxed);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut hasher = Sha256::new();
    hasher.update(session_id.as_bytes());
    hasher.update(nonce.to_le_bytes());
    hasher.update(time.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    hex[..48].to_string()
}

/// The synthetic mcpServers entry for one session. `None` when the tool
/// surface is disabled (settings) — the agent then simply never sees it.
pub fn session_entry(app: &AppHandle, session_id: &str) -> Option<Value> {
    if !crate::browser::tools_enabled() {
        return None;
    }
    let port = ensure_started(app)?;
    let token = register_session_token(session_id);
    Some(json!({
        "name": "marionette-tools",
        "url": format!("http://127.0.0.1:{port}/mcp"),
        "headers": { "Authorization": format!("Bearer {token}") },
        "type": "http",
    }))
}

/// Mint a bearer token that names `session_id` on this server. Shared by the
/// built-in tool surface and the universal plugin layer (plugins must keep
/// working when the browser tool surface is toggled off, so they mint their
/// own token instead of piggybacking on `session_entry`).
pub(crate) fn register_session_token(session_id: &str) -> String {
    let token = mint_token(session_id);
    if let Ok(mut guard) = TOKENS.write() {
        guard
            .get_or_insert_with(HashMap::new)
            .insert(token.clone(), session_id.to_string());
    }
    token
}

/// Make sure the localhost listener exists and return its port. Independent of
/// the browser tool toggle — the plugin routes live on the same server.
pub(crate) fn ensure_listening(app: &AppHandle) -> Option<u16> {
    ensure_started(app)
}

fn ensure_started(app: &AppHandle) -> Option<u16> {
    if let Some(state) = SERVER.get() {
        return Some(state.port);
    }
    let server = tiny_http::Server::http("127.0.0.1:0").ok()?;
    let port = server.server_addr().to_ip().map(|addr| addr.port())?;
    let _ = SERVER.set(ServerState { port, app: app.clone() });
    std::thread::Builder::new()
        .name("tools-mcp".to_string())
        .spawn(move || {
            for request in server.incoming_requests() {
                // A permission card can hold a call for two minutes; the
                // server must keep answering everything else meanwhile.
                std::thread::spawn(move || handle_request(request));
            }
        })
        .ok()?;
    crate::debug_log::append(
        "tools-mcp",
        "info",
        "",
        "localhost mcp listening",
        Some(&format!("127.0.0.1:{port}/mcp")),
    );
    Some(port)
}

fn handle_request(mut request: tiny_http::Request) {
    let method = request.method().to_string();
    // The streamable-HTTP client may probe GET for server-push; a stateless
    // server answers "not offered" and that is spec-conformant.
    if method == "GET" {
        let _ = request.respond(tiny_http::Response::empty(405));
        return;
    }
    if method != "POST" {
        let _ = request.respond(tiny_http::Response::empty(405));
        return;
    }
    let session_id = bearer_session(&request);
    let Some(session_id) = session_id else {
        let _ = request.respond(tiny_http::Response::from_string("unauthorized").with_status_code(401));
        return;
    };
    let mut body = String::new();
    let capped = request
        .as_reader()
        .take((MAX_BODY_BYTES + 1) as u64)
        .read_to_string(&mut body);
    if capped.is_err() || body.len() > MAX_BODY_BYTES {
        let _ = request.respond(tiny_http::Response::from_string("body too large").with_status_code(413));
        return;
    }
    let message: Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(_) => {
            let _ = request.respond(
                tiny_http::Response::from_string(
                    json!({"jsonrpc": "2.0", "id": null, "error": {"code": -32700, "message": "parse error"}}).to_string(),
                )
                .with_status_code(400),
            );
            return;
        }
    };
    // Universal plugin routes live under /mcp/p/<id>. They own their whole
    // message lifecycle: notifications relay to the plugin (stdio child or
    // remote) instead of the bare 202 the built-in surface answers, and a
    // plugin's own JSON-RPC errors pass through verbatim.
    let path = request.url().to_string();
    if let Some(plugin_id) = plugin_route(&path) {
        crate::plugin_host::proxy_handle(plugin_id, session_id, message, request);
        return;
    }
    // Notifications (no id) get a bare accepted — there is nothing to answer.
    let id = message.get("id").cloned();
    if id.is_none() {
        let _ = request.respond(tiny_http::Response::empty(202));
        return;
    }
    let id = id.unwrap_or(Value::Null);
    let method = message.get("method").and_then(Value::as_str).unwrap_or("").to_string();
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    let result = route(&method, &params, &session_id);
    let payload = match result {
        Ok(value) => json!({"jsonrpc": "2.0", "id": id, "result": value}),
        Err(error) => json!({"jsonrpc": "2.0", "id": id, "error": error}),
    };
    let response = tiny_http::Response::from_string(payload.to_string())
        .with_header(tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap());
    let _ = request.respond(response);
}

fn bearer_session(request: &tiny_http::Request) -> Option<String> {
    let header = request.headers().iter().find(|h| h.field.equiv("Authorization"))?;
    let value = header.value.as_str();
    let token = value.strip_prefix("Bearer ").map(str::trim_start)?;
    let guard = TOKENS.read().ok()?;
    guard.as_ref()?.get(token).cloned()
}

/// `/mcp/p/<plugin-id>` → plugin id. Anything else (including the bare `/mcp`
/// built-in route) is `None`. One path segment only; query strings ignored.
fn plugin_route(url: &str) -> Option<&str> {
    let path = url.split('?').next().unwrap_or(url);
    let rest = path.strip_prefix("/mcp/p/")?;
    let id = rest.trim_end_matches('/');
    if id.is_empty() || id.contains('/') {
        return None;
    }
    Some(id)
}

fn route(method: &str, params: &Value, session_id: &str) -> Result<Value, Value> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or("2024-11-05"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "marionette-tools", "version": "1.0.0" },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_schemas() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            let Some(state) = SERVER.get() else {
                return Err(json!({"code": -32000, "message": "server is down"}));
            };
            let outcome = dispatch(&state.app, session_id, name, &arguments);
            match outcome {
                Ok(value) => Ok(json!({
                    "content": [{ "type": "text", "text": value.to_string() }],
                    "isError": false,
                })),
                // Unexpected host failures only; every refusal the agent can
                // act on already came back as a successful tool result.
                Err(message) => Ok(json!({
                    "content": [{ "type": "text", "text": message }],
                    "isError": true,
                })),
            }
        }
        _ => Err(json!({"code": -32601, "message": format!("unknown method: {method}")})),
    }
}

// ---------------------------------------------------------------------------
// Tool schemas
// ---------------------------------------------------------------------------

fn tool_schemas() -> Value {
    fn tool(
        name: &str,
        description: &str,
        required: &[&str],
        properties: Value,
    ) -> Value {
        json!({
            "name": name,
            "description": description,
            "inputSchema": { "type": "object", "required": required, "properties": properties },
        })
    }
    let tab_id = || json!({ "type": "string", "description": "Browser tab id from browser_list_tabs / browser_open_tab." });
    let generation = || json!({ "type": "string", "description": "The `generation` value from the snapshot this ref came from; a stale one is refused." });
    let element_ref = || json!({ "type": "string", "description": "Element ref from the snapshot tree (e.g. \"e12\"). Click by ref, not by pixel." });
    json!([
        tool(
            "browser_list_tabs",
            "List the browser tabs open in Marionette's built-in browser, with each tab's sharing state (none / read / control).",
            &[],
            json!({})
        ),
        tool(
            "browser_open_tab",
            "Open a new browser tab and navigate to an http(s) URL. Tabs you open are readable by you immediately (snapshot/console/screenshot); actions still need the user's approval. Returns the new tab id.",
            &["url"],
            json!({ "url": { "type": "string", "description": "Full http(s) URL (a bare domain gets https:// prepended)." } })
        ),
        tool(
            "browser_close_tab",
            "Close a browser tab.",
            &["tabId"],
            json!({ "tabId": tab_id() })
        ),
        tool(
            "browser_snapshot",
            "Read a page as an accessibility tree (roles, names, refs). Click/type act on the refs, not pixels. Truncated trees invite a narrower `maxChars` follow-up.",
            &["tabId"],
            json!({
                "tabId": tab_id(),
                "maxChars": { "type": "integer", "description": "Rendered-tree cap; default 40000." },
            })
        ),
        tool(
            "browser_console_messages",
            "Read the page's console tail (newest last). `missing` counts lines the ring dropped — it is a fact, not silence.",
            &["tabId"],
            json!({
                "tabId": tab_id(),
                "limit": { "type": "integer", "description": "Newest N lines; default 100." },
            })
        ),
        tool(
            "browser_screenshot",
            "Capture the tab as a JPEG data URL (narrowed to fit ~3.5MB).",
            &["tabId"],
            json!({
                "tabId": tab_id(),
                "maxWidth": { "type": "integer", "description": "Long-edge cap in px; default 1568." },
            })
        ),
        tool(
            "browser_click",
            "Click an element (Control level: asks the user unless already allowed).",
            &["tabId", "generation", "ref"],
            json!({ "tabId": tab_id(), "generation": generation(), "ref": element_ref(), "count": { "type": "integer", "description": "1 = click (default), 2 = double click." } })
        ),
        tool(
            "browser_hover",
            "Hover an element (Control level).",
            &["tabId", "generation", "ref"],
            json!({ "tabId": tab_id(), "generation": generation(), "ref": element_ref() })
        ),
        tool(
            "browser_type",
            "Replace a field's value with text (Control level). `submit` presses Enter after.",
            &["tabId", "generation", "ref", "text"],
            json!({ "tabId": tab_id(), "generation": generation(), "ref": element_ref(), "text": { "type": "string" }, "submit": { "type": "boolean" } })
        ),
        tool(
            "browser_press_key",
            "Press a key (\"Enter\", \"a\", \"Control+Shift+k\") on an element, or on whatever has focus when `ref` is omitted (Control level).",
            &["tabId", "generation", "key"],
            json!({ "tabId": tab_id(), "generation": generation(), "ref": element_ref(), "key": { "type": "string" } })
        ),
        tool(
            "browser_select_option",
            "Choose options of a <select> by value or label (Control level).",
            &["tabId", "generation", "ref", "values"],
            json!({ "tabId": tab_id(), "generation": generation(), "ref": element_ref(), "values": { "type": "array", "items": { "type": "string" } } })
        ),
        tool(
            "computer_list_windows",
            "List windows the Computer Use driver can address (Windows). Each row: pid/windowId/title/bounds + sharing state. Unshareable windows (Marionette itself, password managers) are marked.",
            &[],
            json!({})
        ),
        tool(
            "computer_screenshot",
            "Capture a shared window as a JPEG data URL (long edge <=1568). Requires the window to be shared (read level).",
            &["pid", "windowId"],
            json!({ "pid": { "type": "integer" }, "windowId": { "type": "integer" }, "maxWidth": { "type": "integer" } })
        ),
        tool(
            "computer_snapshot",
            "Read a shared window's accessibility tree (roles, names, element tokens). Act on the `element_token` values, or on x/y points in the screenshot's pixel space.",
            &["pid", "windowId"],
            json!({ "pid": { "type": "integer" }, "windowId": { "type": "integer" } })
        ),
        tool(
            "computer_click",
            "Click in a shared window (control level: the user approves unless already allowed). Give an element_token from a snapshot, or x/y in the screenshot's pixel space.",
            &["pid", "windowId"],
            json!({
                "pid": { "type": "integer" }, "windowId": { "type": "integer" },
                "token": { "type": "string" }, "x": { "type": "number" }, "y": { "type": "number" },
                "button": { "type": "string", "enum": ["left", "right", "middle"] },
                "count": { "type": "integer", "description": "1 = click (default), 2 = double click." }
            })
        ),
        tool(
            "computer_drag",
            "Drag from one point to another in a shared window (control level).",
            &["pid", "windowId", "fromX", "fromY", "toX", "toY"],
            json!({ "pid": { "type": "integer" }, "windowId": { "type": "integer" },
                    "fromX": { "type": "number" }, "fromY": { "type": "number" },
                    "toX": { "type": "number" }, "toY": { "type": "number" },
                    "durationMs": { "type": "integer" } })
        ),
        tool(
            "computer_scroll",
            "Scroll a shared window (control level).",
            &["pid", "windowId", "direction", "amount"],
            json!({ "pid": { "type": "integer" }, "windowId": { "type": "integer" },
                    "direction": { "type": "string", "enum": ["up", "down", "left", "right"] },
                    "amount": { "type": "integer" }, "unit": { "type": "string", "enum": ["line", "page"] },
                    "token": { "type": "string" }, "x": { "type": "number" }, "y": { "type": "number" } })
        ),
        tool(
            "computer_type",
            "Replace a field's value with text in a shared window (control level). `submit` presses Enter after.",
            &["pid", "windowId", "text"],
            json!({ "pid": { "type": "integer" }, "windowId": { "type": "integer" },
                    "token": { "type": "string" }, "text": { "type": "string" }, "submit": { "type": "boolean" } })
        ),
        tool(
            "computer_press_key",
            "Press a key or chord (Enter, a, Control+Shift+k) in a shared window (control level). Key whitelist applies: paste is refused unless the clipboard still holds what you wrote; desktop chords (Win/Alt+Tab/Ctrl+Alt+Del) never go.",
            &["pid", "windowId", "key"],
            json!({ "pid": { "type": "integer" }, "windowId": { "type": "integer" },
                    "key": { "type": "string" }, "token": { "type": "string" } })
        ),
        tool(
            "computer_set_value",
            "Set a field's value directly (control level).",
            &["pid", "windowId", "token", "value"],
            json!({ "pid": { "type": "integer" }, "windowId": { "type": "integer" },
                    "token": { "type": "string" }, "value": { "type": "string" } })
        ),
        tool(
            "computer_restore",
            "Restore a minimized/hidden shared window without stealing the foreground (control level).",
            &["pid", "windowId"],
            json!({ "pid": { "type": "integer" }, "windowId": { "type": "integer" } })
        ),
        tool(
            "computer_clipboard_read",
            "Read the clipboard text. Only offered while a window is actively shared.",
            &[],
            json!({})
        ),
        tool(
            "computer_clipboard_write",
            "Write text to the clipboard (so you can paste it yourself later; pasting it via computer_press_key is allowed only while the clipboard still holds exactly this).",
            &["text"],
            json!({ "text": { "type": "string" } })
        ),
    ])
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

fn arg_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing argument: {key}"))
}

fn refusal(tab_id: &str, error: &str, note: &str) -> Result<Value, String> {
    Ok(json!({ "tabId": tab_id, "error": error, "note": note }))
}

#[cfg(target_os = "windows")]
fn dispatch(app: &AppHandle, session_id: &str, name: &str, args: &Value) -> Result<Value, String> {
    use crate::browser::agent::{ActionKind, ActionRequest, SnapshotRequest};
    use crate::browser::surface;
    use crate::browser::{self};
    use crate::computer;

    match name {
        "browser_list_tabs" => {
            let tabs = browser::list_tabs(app)?;
            Ok(json!({ "tabs": tabs }))
        }
        "browser_open_tab" => {
            let url = arg_str(args, "url")?;
            let tab_id = browser::open_tab(app, &url, true)?;
            Ok(json!({ "tabId": tab_id, "note": "opened; reads are allowed, actions ask the user" }))
        }
        "browser_close_tab" => {
            let tab_id = arg_str(args, "tabId")?;
            browser::close_tab(app, &tab_id)?;
            Ok(json!({ "tabId": tab_id, "closed": true }))
        }
        "browser_snapshot" => {
            let tab_id = arg_str(args, "tabId")?;
            let max_chars = args
                .get("maxChars")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_SNAPSHOT_MAX_CHARS)
                .clamp(1000, 2_000_000);
            browser::tool_read(app, &tab_id, move |s| {
                let request = SnapshotRequest { max_chars };
                surface::snapshot(s, &request)
            })
            .map(|mut value| {
                if let Some(object) = value.as_object_mut() {
                    object.insert("tabId".to_string(), json!(tab_id));
                }
                value
            })
        }
        "browser_console_messages" => {
            let tab_id = arg_str(args, "tabId")?;
            let limit = args
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_CONSOLE_LIMIT as u64)
                .clamp(1, 1000) as usize;
            browser::tool_read(app, &tab_id, move |s| Ok(surface::console_lines(s, limit)))
                .map(|mut value| {
                    if let Some(object) = value.as_object_mut() {
                        object.insert("tabId".to_string(), json!(tab_id));
                    }
                    value
                })
        }
        "browser_screenshot" => {
            let tab_id = arg_str(args, "tabId")?;
            let max_width = args.get("maxWidth").and_then(Value::as_u64).map(|w| w as u32);
            browser::tool_read(app, &tab_id, move |s| surface::screenshot(s, max_width))
                .map(|mut value| {
                    if let Some(object) = value.as_object_mut() {
                        object.insert("tabId".to_string(), json!(tab_id));
                    }
                    value
                })
        }
        // ── Control level ──────────────────────────────────────────────
        "browser_click" | "browser_hover" | "browser_type" | "browser_press_key" | "browser_select_option" => {
            let tab_id = arg_str(args, "tabId")?;
            let generation = arg_str(args, "generation")?;
            let target = args.get("ref").and_then(Value::as_str).map(str::to_string);
            let action = match name {
                "browser_click" => ActionKind::Click {
                    count: args.get("count").and_then(Value::as_u64).map(|c| c as u8),
                },
                "browser_hover" => ActionKind::Hover,
                "browser_type" => ActionKind::Type {
                    text: arg_str(args, "text")?,
                    submit: args.get("submit").and_then(Value::as_bool).unwrap_or(false),
                },
                "browser_press_key" => {
                    let key = arg_str(args, "key")?;
                    if target.is_none() && key.is_empty() {
                        return Err("press needs a key".to_string());
                    }
                    ActionKind::Press { key }
                }
                _ => ActionKind::Select {
                    values: args
                        .get("values")
                        .and_then(Value::as_array)
                        .map(|list| {
                            list.iter()
                                .filter_map(Value::as_str)
                                .map(str::to_string)
                                .collect::<Vec<String>>()
                        })
                        .unwrap_or_default(),
                },
            };
            let summary = match &action {
                ActionKind::Click { .. } => "click".to_string(),
                ActionKind::Hover => "hover".to_string(),
                ActionKind::Type { text, .. } => format!("type \"{text}\""),
                ActionKind::Press { key } => format!("press {key}"),
                ActionKind::Select { values } => format!("select {}", values.join(", ")),
            };
            if !browser::grant_level(app, &tab_id)?.allows(surface::GrantLevel::Control)
                && !browser::session_auto_approves(session_id)
                && !browser::request_permission(
                    app,
                    session_id,
                    &tab_id,
                    &browser::tab_url(app, &tab_id).unwrap_or_default(),
                    &browser::tab_origin(app, &tab_id).unwrap_or_default(),
                    &summary,
                )
            {
                return refusal(
                    &tab_id,
                    "browser_control_required",
                    "The user declined (or did not answer) this action. Say what you wanted to do and why, then wait for them to allow actions on this tab.",
                );
            }
            let request = ActionRequest { generation, target, action };
            surface::with_surface(app, &tab_id, move |s| surface::act(s, &request))
                .and_then(|inner| inner)
                .map(|mut value| {
                    if let Some(object) = value.as_object_mut() {
                        object.insert("tabId".to_string(), json!(tab_id));
                    } else {
                        // The absent sentinel came back as a bare string.
                        return json!({ "tabId": tab_id, "error": "browser_read_failed", "note": "the page has no snapshot yet; take one and retry" });
                    }
                    value
                })
                .map_err(|e| format!("action failed: {e}"))
        }
        // -- Computer Use ---------------------------------------------------
        "computer_list_windows" => computer::tool_list_windows(app),
        "computer_screenshot" => computer::tool_capture(app, session_id, args),
        "computer_snapshot" => computer::tool_snapshot(app, session_id, args),
        "computer_clipboard_read" => computer::tool_clipboard(app, session_id, false, args),
        "computer_clipboard_write" => computer::tool_clipboard(app, session_id, true, args),
        "computer_click" | "computer_drag" | "computer_scroll" | "computer_type"
        | "computer_press_key" | "computer_set_value" | "computer_restore" => {
            use crate::computer::protocol::WindowAction;
            let action = match name {
                "computer_click" => WindowAction::Click {
                    x: args.get("x").and_then(Value::as_f64),
                    y: args.get("y").and_then(Value::as_f64),
                    token: args.get("token").and_then(Value::as_str).map(str::to_string),
                    button: args.get("button").and_then(Value::as_str).map(str::to_string),
                    count: args.get("count").and_then(Value::as_u64).map(|c| c as u8),
                    modifiers: Vec::new(),
                },
                "computer_drag" => WindowAction::Drag {
                    from_x: args.get("fromX").and_then(Value::as_f64).unwrap_or(0.0),
                    from_y: args.get("fromY").and_then(Value::as_f64).unwrap_or(0.0),
                    to_x: args.get("toX").and_then(Value::as_f64).unwrap_or(0.0),
                    to_y: args.get("toY").and_then(Value::as_f64).unwrap_or(0.0),
                    duration_ms: args.get("durationMs").and_then(Value::as_u64),
                },
                "computer_scroll" => WindowAction::Scroll {
                    direction: arg_str(args, "direction")?,
                    amount: args.get("amount").and_then(Value::as_u64).unwrap_or(3) as u32,
                    unit: args.get("unit").and_then(Value::as_str).unwrap_or("line").to_string(),
                    x: args.get("x").and_then(Value::as_f64),
                    y: args.get("y").and_then(Value::as_f64),
                    token: args.get("token").and_then(Value::as_str).map(str::to_string),
                },
                "computer_type" => WindowAction::Type {
                    token: args.get("token").and_then(Value::as_str).map(str::to_string),
                    text: arg_str(args, "text")?,
                    submit: args.get("submit").and_then(Value::as_bool).unwrap_or(false),
                },
                "computer_press_key" => WindowAction::Press {
                    key: arg_str(args, "key")?,
                    token: args.get("token").and_then(Value::as_str).map(str::to_string),
                },
                "computer_set_value" => WindowAction::SetValue {
                    token: arg_str(args, "token")?,
                    value: arg_str(args, "value")?,
                },
                _ => WindowAction::Restore,
            };
            computer::tool_act(app, session_id, &json!({
                "pid": args.get("pid"),
                "windowId": args.get("windowId"),
                "action": action,
            }))
        }
        _ => Err(format!("unknown tool: {name}")),
    }
}

#[cfg(not(target_os = "windows"))]
fn dispatch(_app: &AppHandle, _session_id: &str, name: &str, _args: &Value) -> Result<Value, String> {
    Err(format!("tool unavailable on this platform: {name}"))
}

/// Keep the type checker honest about unused imports on non-Windows builds.
#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
fn _touch() {
    let _ = Duration::from_secs(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_echoes_the_client_protocol_version() {
        let params = json!({"protocolVersion": "2025-03-26"});
        let result = route("initialize", &params, "s1").unwrap();
        assert_eq!(result["protocolVersion"], "2025-03-26");
        let result = route("initialize", &json!({}), "s1").unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
    }

    #[test]
    fn unknown_methods_are_protocol_errors() {
        assert!(route("resources/list", &json!({}), "s1").is_err());
    }

    #[test]
    fn tokens_are_long_and_unique() {
        assert_ne!(mint_token("a"), mint_token("a"));
        assert_eq!(mint_token("a").len(), 48);
    }

    #[test]
    fn tool_schemas_cover_the_documented_set() {
        let tools = tool_schemas();
        let names: Vec<&str> = tools
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        for expected in [
            "browser_list_tabs",
            "browser_open_tab",
            "browser_close_tab",
            "browser_snapshot",
            "browser_console_messages",
            "browser_screenshot",
            "browser_click",
            "browser_hover",
            "browser_type",
            "browser_press_key",
            "browser_select_option",
            "computer_list_windows",
            "computer_screenshot",
            "computer_snapshot",
            "computer_click",
            "computer_drag",
            "computer_scroll",
            "computer_type",
            "computer_press_key",
            "computer_set_value",
            "computer_restore",
            "computer_clipboard_read",
            "computer_clipboard_write",
        ] {
            assert!(names.contains(&expected), "missing tool {expected}");
        }
    }
}
