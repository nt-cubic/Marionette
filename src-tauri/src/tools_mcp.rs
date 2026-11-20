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
use std::time::{Duration, Instant};

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
        "headers": [{ "name": "Authorization", "value": format!("Bearer {token}") }],
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
            "instructions": "Use browser_open_tab with display=\"workspace\" only when the user explicitly asks to see or work on a rendered web page beside this conversation. Starting a local dev server does not authorize taking the workspace: open it in the background unless the user asked for it to be visible. Keep ordinary research, reference pages, sign-in, API ports, and one-off reads in the background. For a page you will revisit, pass a short stable id such as work-mail; opening an existing id returns that tab without navigating it.",
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({
            "tools": if crate::chief::broker::is_chief_session(session_id) {
                // Chief 会话只看到控制工具（02-architecture.md §5.1：控制
                // 工具只注入这一会话；普通会话的工具列表不出现它们）。
                crate::chief::tools::chief_tool_schemas()
            } else {
                tool_schemas_for_session(session_id)
            }
        })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            let Some(state) = SERVER.get() else {
                return Err(json!({"code": -32000, "message": "server is down"}));
            };
            // Browser/computer tools hop to the main thread and can CDP-wait
            // behind a cold renderer; without this line a hung call left zero
            // trace in dev.log (2026-11-18 freeze was timed by inference only).
            let dispatch_started = Instant::now();
            let outcome = dispatch(&state.app, session_id, name, &arguments);
            let took_ms = dispatch_started.elapsed().as_millis();
            if name.starts_with("browser_") || name.starts_with("computer_") {
                crate::debug_log::append(
                    "mcp",
                    if took_ms > 5_000 { "warn" } else { "info" },
                    session_id,
                    "tool call",
                    Some(&format!(
                        "{name} {}ms {}",
                        took_ms,
                        match &outcome {
                            Ok(value) => {
                                let errored = value.get("error").is_some();
                                if errored { "refused" } else { "ok" }
                            }
                            Err(_) => "host-error",
                        }
                    )),
                );
            }
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
    let generation = || json!({ "type": "string", "description": "The `generation` value from the snapshot these refs came from. One generation covers any number of consecutive actions — reuse it until an action answers stale or the page navigates, then take a new snapshot." });
    let element_ref = || json!({ "type": "string", "description": "Element ref from the snapshot tree (e.g. \"e12\"). Click by ref, not by pixel." });
    json!([
        tool(
            "browser_help",
            "How the browser tools fit together: observation economy (cheap probes before snapshots), reusing one snapshot's generation across many actions, grants, handoff, tab ownership. Optional `topic` deep-dives; default overview.",
            &[],
            json!({
                "topic": {
                    "type": "string",
                    "enum": ["overview", "refs", "snapshot", "actions", "tabs", "handoff"],
                    "description": "Optional deep dive; default overview."
                }
            })
        ),
        tool(
            "browser_list_tabs",
            "List the browser tabs open in Marionette's built-in browser, including stable id, owner session, pinned, busy, status, last use and sharing state (none / read / control).",
            &[],
            json!({})
        ),
        tool(
            "browser_open_tab",
            "Open a new browser tab and navigate to an http(s) URL or explicit local file path/URL. Bare localhost / 127.0.0.1 (with optional port) is accepted and opened over http; file:///… and absolute local paths open as local files. Tabs you open are readable by you immediately (snapshot/console/screenshot); actions still need the user's approval. display=workspace shows the page beside the current Agent conversation; use it only when the user explicitly asks to see or work on a rendered web page. A local dev server you started remains background unless the user asks for it to be visible. Returns the new tab id.",
            &["url"],
            json!({
                "url": { "type": "string", "description": "HTTP(S) URL, explicit file:/// URL, or absolute local file path. localhost:5173 and 127.0.0.1:3000 (no scheme) open as http; a public bare domain gets https://." },
                "display": {
                    "type": "string",
                    "enum": ["background", "workspace"],
                    "default": "background",
                    "description": "workspace only when the user explicitly asks to see or work on the page beside chat; otherwise background."
                },
                "id": {
                    "type": "string",
                    "description": "Optional stable handle (lowercase letters, digits, dashes; max 48). Opening the same id again returns the existing tab without navigating it. Named tabs are never closed by idle cleanup."
                }
            })
        ),
        tool(
            "browser_handoff",
            "Pause and ask the person to complete a step in the shared browser, such as signing in, completing 2FA or a CAPTCHA, confirming a choice, or checking a result that only they can access. This tool blocks until the person clicks Continue, declines, or the timeout expires. Use browser approval/permission flows for your own actions; use handoff only when the human must act.",
            &["tabId", "message"],
            json!({
                "tabId": tab_id(),
                "title": {
                    "type": "string",
                    "maxLength": 120,
                    "description": "Short user-facing title; default 'Complete this step'."
                },
                "message": {
                    "type": "string",
                    "maxLength": 2000,
                    "description": "One concrete instruction telling the person what to do in the page."
                },
                "timeoutSec": {
                    "type": "integer",
                    "minimum": 15,
                    "maximum": 300,
                    "default": 120
                }
            })
        ),
        tool(
            "browser_close_tab",
            "Close a browser tab you opened. User-owned, claimed, or pinned tabs are left open and reported back instead.",
            &["tabId"],
            json!({ "tabId": tab_id() })
        ),
        tool(
            "browser_snapshot",
            "Read a page as an accessibility tree (roles, names, refs). Click/type act on the refs, not pixels. Truncated trees invite a narrower `maxChars` follow-up. Cheap first: browser_page_state's `seq` tells you whether anything moved since your last look — snapshot when you need to find elements, not to detect change.",
            &["tabId"],
            json!({
                "tabId": tab_id(),
                "maxChars": { "type": "integer", "description": "Rendered-tree cap; default 40000." },
            })
        ),
        tool(
            "browser_page_state",
            "Cheap page identity + change counter: url, title, readyState, scroll, viewport, and `seq` (a DOM mutation counter). Compare two `seq` reads to learn whether anything moved — far cheaper than a fresh snapshot.",
            &["tabId"],
            json!({ "tabId": tab_id() })
        ),
        tool(
            "browser_page_ready",
            "Whether the page looks settled: readyState complete, no images still loading, fonts not loading. `settled: false` means wait rather than read a skeleton.",
            &["tabId"],
            json!({ "tabId": tab_id() })
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
            "browser_wait_for",
            "Wait until a snapshot ref reaches a state (attached/visible/hidden/detached) or the timeout expires — never sleep-and-retry with snapshots. Returns the observed state; on timeout names what was actually seen (not_found means the ref no longer matches the page — take a new snapshot).",
            &["tabId", "generation", "ref"],
            json!({
                "tabId": tab_id(),
                "generation": generation(),
                "ref": element_ref(),
                "state": {
                    "type": "string",
                    "enum": ["attached", "visible", "hidden", "detached"],
                    "default": "visible",
                    "description": "hidden = present in the DOM but not visible; detached = removed from it."
                },
                "timeoutMs": { "type": "integer", "description": "Hard deadline; default 3000, max 15000." }
            })
        ),
        tool(
            "browser_wait_for_load_state",
            "Wait for the document to reach domcontentloaded or load. Use after a navigation you caused instead of guessing a fixed sleep.",
            &["tabId"],
            json!({
                "tabId": tab_id(),
                "state": { "type": "string", "enum": ["domcontentloaded", "load"], "default": "load" },
                "timeoutMs": { "type": "integer", "description": "Hard deadline; default 5000, max 30000." }
            })
        ),
        tool(
            "browser_dialog",
            "Answer a page script dialog (alert/confirm/prompt/beforeunload). The default dialog is also visible to the user — they may answer it first, in which case this reports that honestly. While a dialog is open, other page calls fail fast with dialog_blocked instead of hanging. Control level.",
            &["tabId", "accept"],
            json!({
                "tabId": tab_id(),
                "accept": { "type": "boolean", "description": "true accepts (with promptText for a prompt), false dismisses. Nothing is answered automatically." },
                "promptText": { "type": "string", "description": "Text for an accepted prompt()." }
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
            "browser_set_value",
            "Write a control's value by ref without synthesizing keystrokes (Control level): a string for a text field, a boolean for a checkbox/radio, a string or array for a <select>. Fires input+change the way the page's framework expects.",
            &["tabId", "generation", "ref", "value"],
            json!({
                "tabId": tab_id(),
                "generation": generation(),
                "ref": element_ref(),
                "value": { "description": "String for a text field, boolean for a checkbox/radio, string or array of strings for a <select>." },
            })
        ),
        tool(
            "browser_batch",
            "Run a short chain of typed steps against one snapshot's generation in a single call — a form fill, a menu walk, click-then-wait. Closed op set (click/hover/type/setValue/pressKey/selectOption/waitForSelector/waitForLoadState/pageState/pageReady/snapshot); no scripting, no navigation. Each Control step still asks its own permission; a refusal stops the batch. Steps run in order and stop at the first failure unless stopOnError=false. A snapshot step is only allowed as the final step (it replaces the refs).",
            &["tabId", "generation", "steps"],
            json!({
                "tabId": tab_id(),
                "generation": generation(),
                "steps": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 32,
                    "description": "Steps in order, e.g. {\"op\":\"waitForSelector\",\"ref\":\"e12\",\"state\":\"visible\"}, {\"op\":\"click\",\"ref\":\"e12\"}, {\"op\":\"setValue\",\"ref\":\"e18\",\"value\":\"a@b.c\"}, {\"op\":\"pressKey\",\"key\":\"Enter\"}, {\"op\":\"waitForLoadState\",\"state\":\"domcontentloaded\"}.",
                    "items": { "type": "object", "properties": { "op": { "type": "string" } }, "required": ["op"] }
                },
                "stopOnError": { "type": "boolean", "default": true, "description": "false keeps going after a failed step (a permission refusal always stops)." }
            })
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

fn tool_schemas_for_session(session_id: &str) -> Value {
    let schemas = tool_schemas();
    if crate::chief::staff_commands::worker_computer_use_allowed(session_id) {
        return schemas;
    }
    let Some(items) = schemas.as_array() else {
        return schemas;
    };
    Value::Array(
        items
            .iter()
            .filter(|tool| {
                tool.get("name")
                    .and_then(Value::as_str)
                    .map(|name| !name.starts_with("computer_"))
                    .unwrap_or(true)
            })
            .cloned()
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// browser_help — the runtime guide, generated from the schema table so the
// agent reads what this build actually ships, never a second hand-written
// manual that can drift. Topics for unshipped capabilities (waits, batch)
// are deliberately absent; the tests pin coverage both ways.
// ---------------------------------------------------------------------------

#[cfg(test)]
pub const HELP_TOPICS: &[&str] = &[
    "overview", "refs", "snapshot", "actions", "tabs", "handoff", "waits", "batch",
];

const HELP_OVERVIEW: &str = "Marionette's browser tools in one breath. Open tabs with browser_open_tab (background by default; display \"workspace\" only when the user explicitly asks to see or work on a rendered page beside chat). Observe cheaply first: browser_page_state's `seq` counter answers \"did anything change\" for a few hundred bytes, and browser_page_ready answers \"is it settled\"; browser_snapshot only when you need to find elements to act on; browser_screenshot only for visual or layout questions. Reach the goal and stop — do not re-observe a result a tool already reported. When an action fails, read the page state before retrying, and never open a new tab to escape a stuck one. Reads need the tab shared at read level; actions ask the user unless already allowed. browser_handoff pauses for steps only the human can do (sign-in, 2FA, CAPTCHA, payment confirmation).";

const HELP_REFS: &str = "Refs and generations. A snapshot returns a `generation` string and a tree of refs (e12-style). One generation covers any number of consecutive actions — click, type, select, press in any order, reusing the same generation; successful actions echo it with generationStillValid: true. Every action re-validates the page and answers {\"ok\":false,\"error\":\"stale\"} the moment the document, the route or the target element moved; take a new snapshot only then, or after a navigation you caused. A stale answer may carry a `reason`: document_navigation, route_navigation, user_annotation (the user's annotate flow re-snapshotted the tab — expected, just re-snapshot), stale_answer, superseded_by_snapshot. Truncation cuts refs too: a tree trimmed by maxChars does not name elements beyond the cut — re-snapshot with a larger maxChars when the element you need was cut.";

const HELP_SNAPSHOT: &str = "When and how to snapshot. Snapshot to find elements to act on — not to detect change (browser_page_state's `seq` does that for far less) and not to judge appearance (browser_screenshot). Reuse the returned `generation` for every action that follows; re-snapshot only after a stale answer or a navigation. `maxChars` caps the rendered tree (default 40000); truncated: true means the tree was cut and refs beyond the cut are not usable — narrow to the region you need or raise maxChars. The snapshot reads; it never clicks anything.";

const HELP_ACTIONS: &str = "Acting on the page. Actions are Control level: the user approves per tab (a permission card) unless they shared control or put your session in always-approve; a refusal is a value, not an error — say what you wanted and wait. Act by ref from a snapshot, never by pixel. browser_type replaces a field's text (submit: true presses Enter after); browser_set_value writes a control's value directly — a boolean for a checkbox or radio, a string or array for a select — and fires the events the page's framework expects; browser_press_key works on whatever has focus when no ref is given. ok: true is a receipt that the input was delivered, not that the business goal succeeded — confirm outcomes with browser_page_state, the URL, or a fresh look at the page.";

const HELP_TABS: &str = "Tabs and ownership. browser_open_tab returns a tab id; passing a stable `id` (a lowercase slug) makes reopening return the same tab without navigating, and named tabs are never closed by idle cleanup. A local preview you start stays in the background unless the user explicitly asks to see it; then use display=workspace. Tabs you opened can be closed with browser_close_tab; user-owned, claimed or pinned tabs are left open and reported back. Plain agent tabs idle long enough are cleaned up automatically — name or pin a tab that must survive. List everything (owner, busy, status, sharing level) with browser_list_tabs. Don't open a new tab to work around a stuck page — diagnose the tab you have.";

const HELP_HANDOFF: &str = "Handing a step to the human. browser_handoff pauses your turn and shows the user one concrete instruction in the page — signing in, completing 2FA or a CAPTCHA, confirming a payment, or checking something only they can see. It blocks until they click Continue, decline, or the timeout (default 120s). Use it only when the human must act; for your own actions go through the normal approval flow. After a Continue the page may have changed completely — take a fresh snapshot before acting further.";

const HELP_WAITS: &str = "Waiting instead of guessing. browser_wait_for polls a snapshot ref until it is attached, visible, hidden or detached — use it after a click that reveals something (a menu, a spinner, a row), never sleep-and-retry with fresh snapshots. browser_wait_for_load_state waits for domcontentloaded or load after a navigation. Every wait has a hard deadline (defaults 3000/5000ms; raise timeoutMs for genuinely slow pages, cap 15000/30000). A timeout is an observation, not a failure to retry blindly: the answer names what was actually seen — not_found means the ref no longer matches the page (take a new snapshot), a still-hidden state means look at why. When a page opens alert/confirm/prompt, other page calls fail fast with dialog_blocked instead of hanging; answer with browser_dialog (accept/dismiss, promptText for prompts — nothing is ever answered automatically) or let the user click the visible dialog; browser_dialog reports honestly when they already did.";

const HELP_BATCH: &str = "browser_batch chains typed steps against one snapshot's generation in a single call: a form fill is waitForSelector, click, setValue, pressKey, waitForLoadState instead of five round trips. The op set is closed (click, hover, type, setValue, pressKey, selectOption, waitForSelector, waitForLoadState, pageState, pageReady, snapshot) — no scripting, no evaluate, no navigation. Rules that keep it safe: at most 32 steps; steps run strictly in order; every Control step still asks its own permission and a refusal stops the batch; a failed step stops it too unless stopOnError=false; a snapshot step may only be last (it replaces the refs every later ref-based step relies on); if the page navigates mid-batch, later ref steps fail stale and stop. Each step's audit record lands in the result (index, op, ok, error) plus a final page state and the still-valid generation.";

/// Answer `browser_help`. The tool list is generated from `tool_schemas()`
/// (single source of truth); each topic narrows it to the tools its prose is
/// about. Unknown topics are errors — help must not guess.
fn help_payload(topic: &str) -> Result<Value, String> {
    let (text, related): (&str, &[&str]) = match topic {
        "overview" => (HELP_OVERVIEW, &[]), // empty = every browser tool
        "refs" => (
            HELP_REFS,
            &[
                "browser_snapshot",
                "browser_click",
                "browser_hover",
                "browser_type",
                "browser_press_key",
                "browser_select_option",
                "browser_set_value",
            ],
        ),
        "snapshot" => (
            HELP_SNAPSHOT,
            &[
                "browser_snapshot",
                "browser_page_state",
                "browser_page_ready",
                "browser_screenshot",
            ],
        ),
        "actions" => (
            HELP_ACTIONS,
            &[
                "browser_click",
                "browser_hover",
                "browser_type",
                "browser_press_key",
                "browser_select_option",
                "browser_set_value",
                "browser_handoff",
            ],
        ),
        "tabs" => (
            HELP_TABS,
            &["browser_list_tabs", "browser_open_tab", "browser_close_tab"],
        ),
        "handoff" => (HELP_HANDOFF, &["browser_handoff"]),
        "waits" => (
            HELP_WAITS,
            &[
                "browser_wait_for",
                "browser_wait_for_load_state",
                "browser_dialog",
                "browser_page_state",
                "browser_page_ready",
            ],
        ),
        "batch" => (
            HELP_BATCH,
            &[
                "browser_batch",
                "browser_snapshot",
                "browser_click",
                "browser_type",
                "browser_set_value",
                "browser_select_option",
                "browser_press_key",
                "browser_wait_for",
                "browser_wait_for_load_state",
                "browser_page_state",
            ],
        ),
        other => return Err(format!("unknown help topic: {other}")),
    };
    let tools: Vec<Value> = tool_schemas()
        .as_array()
        .unwrap()
        .iter()
        .filter(|tool| {
            tool["name"].as_str().is_some_and(|name| name.starts_with("browser_"))
        })
        .filter(|tool| {
            related.is_empty()
                || related.contains(&tool["name"].as_str().unwrap_or(""))
        })
        .map(|tool| {
            json!({
                "name": tool["name"],
                "description": tool["description"],
                "params": tool["inputSchema"]["properties"],
            })
        })
        .collect();
    Ok(json!({
        "topic": topic,
        "text": text,
        "tools": tools,
    }))
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
use crate::browser::agent::SnapshotRequest;
#[cfg(target_os = "windows")]
use crate::browser::{self, batch};
#[cfg(target_os = "windows")]
use crate::browser::surface;

/// Interval between two polls of a wait condition. Each poll is one short
/// eval through the process-wide CDP lease, so other tabs interleave freely.
#[cfg(target_os = "windows")]
const POLL_EVERY_MS: u64 = 150;

/// Poll one cheap page-side check until `done` says so or the deadline
/// passes. Runs on the MCP worker thread: sleeps happen HERE, never on the
/// main thread (a main-thread sleep is the watchdog's MAIN THREAD STALLED),
/// and never while holding the CDP lease.
#[cfg(target_os = "windows")]
fn poll_until(
    app: &AppHandle,
    tab_id: &str,
    call: String,
    deadline: Instant,
    done: impl Fn(&Value) -> bool,
) -> Result<Value, String> {
    let mut last = json!({});
    loop {
        let value = browser::tool_read(app, tab_id, {
            let call = call.clone();
            move |s| surface::probe(s, &call)
        });
        match value {
            Ok(value) => {
                // A grant refusal cannot be fixed by polling — terminal.
                if value.get("error").and_then(Value::as_str) == Some("browser_grant_required") {
                    return Ok(value);
                }
                last = value;
            }
            // Page-side failures (dialog_blocked, world gone) are terminal too.
            Err(error) => return Err(error),
        }
        if done(&last) || Instant::now() >= deadline {
            return Ok(last);
        }
        std::thread::sleep(Duration::from_millis(POLL_EVERY_MS));
    }
}

/// Classify a finished refState poll. Success keeps the observed state; a
/// timeout names what was actually seen — never a bare "try again".
fn wait_ref_record(target: &str, state: &str, waited_ms: u64, last: &Value) -> Value {
    let found = last.get("found").and_then(Value::as_bool).unwrap_or(false);
    let connected = last.get("connected").and_then(Value::as_bool).unwrap_or(false);
    let visible = last.get("visible").and_then(Value::as_bool).unwrap_or(false);
    let reached = match state {
        "attached" => found && connected,
        "visible" => found && connected && visible,
        "hidden" => found && connected && !visible,
        // Gone is gone: a stale generation also means the element is no
        // longer on this page in any sense that matters.
        "detached" => !connected,
        _ => false,
    };
    let mut record = json!({ "ok": reached, "ref": target, "state": state, "waitedMs": waited_ms });
    if reached {
        record["observed"] = json!({ "found": found, "connected": connected, "visible": visible });
    } else if !found {
        record["error"] = json!("not_found");
        record["detail"] = json!(
            "the ref no longer matches the page — the element moved or the page changed; take a new snapshot"
        );
        record["observed"] = json!({ "found": false });
    } else {
        record["error"] = json!("timeout");
        record["detail"] = json!(match state {
            "visible" => "present in the DOM but not visible",
            "hidden" => "still visible",
            "attached" => "not connected",
            "detached" => "still connected",
            _ => "state not reached",
        });
        record["observed"] = json!({ "found": found, "connected": connected, "visible": visible });
    }
    record
}

fn load_state_reached(state: &str, ready_state: &str) -> bool {
    match state {
        "domcontentloaded" => ready_state == "interactive" || ready_state == "complete",
        "load" => ready_state == "complete",
        _ => false,
    }
}

/// Classify a finished load-state poll the same way.
fn load_state_record(state: &str, waited_ms: u64, last: &Value) -> Value {
    let ready = last.get("readyState").and_then(Value::as_str).unwrap_or("unknown");
    let reached = load_state_reached(state, ready);
    let mut record = json!({ "ok": reached, "state": state, "readyState": ready, "waitedMs": waited_ms });
    if !reached {
        record["error"] = json!("timeout");
        record["detail"] = json!(format!("the document was still in readyState '{ready}' when the deadline passed"));
    }
    record
}

/// Execute one validated batch step. Each hop goes through the normal
/// tool wrappers, so grants, busy status and the ref lease apply per step.
#[cfg(target_os = "windows")]
fn execute_batch_step(
    app: &AppHandle,
    tab_id: &str,
    index: usize,
    step: &batch::BatchStep,
    generation: &str,
    batch_deadline: Instant,
    final_generation: &mut String,
) -> Result<Value, String> {
    match step {
        batch::BatchStep::Click { .. }
        | batch::BatchStep::Hover { .. }
        | batch::BatchStep::Type { .. }
        | batch::BatchStep::PressKey { .. }
        | batch::BatchStep::SelectOption { .. } => {
            let request = batch::action_request(step, generation)
                .ok_or("batch control step lost its action during execution")?;
            browser::tool_call(app, tab_id, move |s| surface::act(s, &request))
                .map(|mut value| {
                    batch::condense(index, step, &mut value);
                    value
                })
        }
        batch::BatchStep::SetValue { target, value } => {
            let generation = generation.to_string();
            let target = target.clone();
            let value = value.clone();
            browser::tool_call(app, tab_id, move |s| {
                surface::set_control_value(s, &generation, &target, &value)
            })
            .map(|mut answer| {
                batch::condense(index, step, &mut answer);
                answer
            })
        }
        batch::BatchStep::WaitForSelector { target, state, timeout_ms } => {
            let step_deadline = Instant::now() + Duration::from_millis(*timeout_ms);
            let deadline = batch_deadline.min(step_deadline);
            let call = browser::probe::wait_ref_state_call(generation, target);
            let started = Instant::now();
            let target = target.clone();
            let state = *state;
            let last = poll_until(app, tab_id, call, deadline, move |value| {
                let connected = value.get("connected").and_then(Value::as_bool);
                let visible = value.get("visible").and_then(Value::as_bool);
                match state {
                    "attached" => connected == Some(true),
                    "visible" => visible == Some(true),
                    "hidden" => connected == Some(true) && visible == Some(false),
                    "detached" => connected == Some(false),
                    _ => false,
                }
            })?;
            let mut record = wait_ref_record(&target, state, started.elapsed().as_millis() as u64, &last);
            record["index"] = json!(index);
            record["op"] = json!("waitForSelector");
            Ok(record)
        }
        batch::BatchStep::WaitForLoadState { state, timeout_ms } => {
            let step_deadline = Instant::now() + Duration::from_millis(*timeout_ms);
            let deadline = batch_deadline.min(step_deadline);
            let call = browser::probe::state_call();
            let started = Instant::now();
            let state = *state;
            let last = poll_until(app, tab_id, call, deadline, move |value| {
                let ready = value.get("readyState").and_then(Value::as_str).unwrap_or("");
                load_state_reached(state, ready)
            })?;
            let mut record = load_state_record(state, started.elapsed().as_millis() as u64, &last);
            record["index"] = json!(index);
            record["op"] = json!("waitForLoadState");
            Ok(record)
        }
        batch::BatchStep::PageState => {
            browser::tool_read(app, tab_id, |s| surface::probe(s, &browser::probe::state_call()))
                .map(|state| json!({ "index": index, "op": "pageState", "ok": true, "state": state }))
        }
        batch::BatchStep::PageReady => {
            browser::tool_read(app, tab_id, |s| surface::probe(s, &browser::probe::ready_call()))
                .map(|ready| json!({ "index": index, "op": "pageReady", "ok": true, "ready": ready }))
        }
        batch::BatchStep::Snapshot { max_chars } => {
            let request = SnapshotRequest { max_chars: *max_chars };
            browser::tool_read(app, tab_id, move |s| surface::snapshot(s, &request))
                .map(|value| {
                    // A successful snapshot step re-seats the batch's
                    // generation: later steps would use the new refs table
                    // (and since snapshot is terminal-only, nothing here
                    // still holds the old one).
                    if let Some(token) = value.get("generation").and_then(Value::as_str) {
                        *final_generation = token.to_string();
                    }
                    json!({
                        "index": index,
                        "op": "snapshot",
                        "ok": true,
                        "generation": value.get("generation"),
                        "title": value.get("title"),
                        "url": value.get("url"),
                        "refsCount": value.get("refsCount"),
                        "truncated": value.get("truncated"),
                        "tree": value.get("tree"),
                    })
                })
        }
    }
}

fn arg_str(args: &Value, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("missing argument: {key}"))
}

/// The one place a Control-level browser action asks permission. A grant
/// already at Control, or a session in always-approve, skips the card; the
/// card's timeout is a refusal like any other. `Ok(Some(value))` is the
/// refusal to hand back verbatim — a value the agent can relay, not an error.
fn control_gate(
    app: &AppHandle,
    session_id: &str,
    tab_id: &str,
    summary: &str,
) -> Result<Option<Value>, String> {
    use crate::browser;
    let level = browser::grant_level(app, tab_id)?;
    let opened_by_agent = browser::tab_opened_by_agent(app, tab_id)?;
    if control_auto_allowed(level, opened_by_agent, browser::session_auto_approves(session_id)) {
        return Ok(None);
    }
    let allowed = browser::request_permission(
        app,
        session_id,
        tab_id,
        &browser::tab_url(app, tab_id).unwrap_or_default(),
        &browser::tab_origin(app, tab_id).unwrap_or_default(),
        summary,
    );
    if allowed {
        return Ok(None);
    }
    refusal(
        tab_id,
        "browser_control_required",
        "The user declined (or did not answer) this action. Say what you wanted to do and why, then wait for them to allow actions on this tab.",
    )
    .map(Some)
}

fn control_auto_allowed(
    level: crate::browser::surface::GrantLevel,
    opened_by_agent: bool,
    session_always_approve: bool,
) -> bool {
    level.allows(crate::browser::surface::GrantLevel::Control)
        || (opened_by_agent && session_always_approve)
}

fn refusal(tab_id: &str, error: &str, note: &str) -> Result<Value, String> {
    Ok(json!({ "tabId": tab_id, "error": error, "note": note }))
}

#[cfg(target_os = "windows")]
fn dispatch(app: &AppHandle, session_id: &str, name: &str, args: &Value) -> Result<Value, String> {
    // Chief 双保险（02-architecture.md §5.1 第 3 条）：schema 已按会话身份
    // 分列，dispatch 入口再按会话身份挡一次——Chief 只走控制工具，普通
    // worker 猜到控制工具名一样被拒。
    if crate::chief::broker::is_chief_session(session_id) {
        return crate::chief::tools::dispatch(app, session_id, name, args);
    }
    if crate::chief::tools::is_chief_tool(name) {
        return Err(format!(
            "tool `{name}` is a Chief control tool and is only available to the Chief session"
        ));
    }
    if name.starts_with("computer_")
        && !crate::chief::staff_commands::worker_computer_use_allowed(session_id)
    {
        return Ok(json!({
            "error": "computer_use_not_allowed",
            "note": "This staff worker's project policy does not allow Computer Use."
        }));
    }
    use crate::browser::agent::{ActionKind, ActionRequest, SnapshotRequest};
    use crate::browser::surface;
    use crate::browser::{self, batch};
    use crate::computer;

    match name {
        "browser_help" => {
            let topic = args
                .get("topic")
                .and_then(Value::as_str)
                .unwrap_or("overview");
            help_payload(topic)
        }
        "browser_list_tabs" => {
            let tabs = browser::list_tabs(app)?;
            Ok(json!({ "tabs": tabs }))
        }
        "browser_open_tab" => {
            let url = arg_str(args, "url")?;
            let display = browser::normalize_open_display(
                args.get("display").and_then(Value::as_str),
            )?;
            let chosen_id = args.get("id").and_then(Value::as_str);
            let (tab_id, existed) = browser::open_tab(
                app,
                &url,
                true,
                display,
                Some(session_id),
                chosen_id,
            )?;
            Ok(json!({
                "tabId": tab_id,
                "id": chosen_id,
                "existed": existed,
                "display": display,
                "note": if existed {
                    "that named tab already existed; it was returned without navigating"
                } else if display == "workspace" {
                    "opened beside the current Agent conversation; reads are allowed, actions ask the user"
                } else {
                    "opened in the background; reads are allowed, actions ask the user"
                }
            }))
        }
        "browser_handoff" => {
            let tab_id = arg_str(args, "tabId")?;
            let message = arg_str(args, "message")?;
            let title = args.get("title").and_then(Value::as_str).unwrap_or("");
            let timeout_secs = args
                .get("timeoutSec")
                .and_then(Value::as_u64)
                .unwrap_or(120)
                .clamp(15, 300);
            browser::request_handoff(
                app,
                session_id,
                &tab_id,
                title,
                &message,
                timeout_secs,
            )
        }
        "browser_close_tab" => {
            let tab_id = arg_str(args, "tabId")?;
            let closed = browser::agent_close_tab(app, &tab_id)?;
            Ok(json!({
                "tabId": tab_id,
                "closed": closed,
                "note": if closed {
                    "tab closed"
                } else {
                    "the tab is user-owned, claimed, or pinned; it was left open for the user"
                }
            }))
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
        "browser_page_state" => {
            let tab_id = arg_str(args, "tabId")?;
            browser::tool_read(app, &tab_id, move |s| surface::probe(s, &browser::probe::state_call()))
                .map(|mut value| {
                    if let Some(object) = value.as_object_mut() {
                        object.insert("tabId".to_string(), json!(tab_id));
                    }
                    value
                })
        }
        "browser_page_ready" => {
            let tab_id = arg_str(args, "tabId")?;
            browser::tool_read(app, &tab_id, move |s| surface::probe(s, &browser::probe::ready_call()))
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
        // ── Waits (read level): poll a cheap page-side check, never sleep
        // on the main thread, every wait has a hard deadline ──────────────
        "browser_wait_for" => {
            let tab_id = arg_str(args, "tabId")?;
            let generation = arg_str(args, "generation")?;
            let target = arg_str(args, "ref")?;
            let state = args.get("state").and_then(Value::as_str).unwrap_or("visible");
            if !batch::WAIT_STATES.contains(&state) {
                return Err(format!("state must be one of {}", batch::WAIT_STATES.join("|")));
            }
            let timeout = args
                .get("timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or(batch::DEFAULT_WAIT_MS)
                .clamp(100, batch::MAX_WAIT_MS);
            let call = browser::probe::wait_ref_state_call(&generation, &target);
            let started = Instant::now();
            let state_owned = state.to_string();
            let last = poll_until(app, &tab_id, call, Instant::now() + Duration::from_millis(timeout), {
                let state_owned = state_owned.clone();
                move |value| {
                    let connected = value.get("connected").and_then(Value::as_bool);
                    let visible = value.get("visible").and_then(Value::as_bool);
                    match state_owned.as_str() {
                        "attached" => connected == Some(true),
                        "visible" => visible == Some(true),
                        "hidden" => connected == Some(true) && visible == Some(false),
                        "detached" => connected == Some(false),
                        _ => false,
                    }
                }
            })?;
            let mut record =
                wait_ref_record(&target, state, started.elapsed().as_millis() as u64, &last);
            if let Some(object) = record.as_object_mut() {
                object.insert("tabId".to_string(), json!(tab_id));
            }
            Ok(record)
        }
        "browser_wait_for_load_state" => {
            let tab_id = arg_str(args, "tabId")?;
            let state = args.get("state").and_then(Value::as_str).unwrap_or("load");
            if !batch::LOAD_STATES.contains(&state) {
                return Err(format!("state must be one of {}", batch::LOAD_STATES.join("|")));
            }
            let timeout = args
                .get("timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or(batch::DEFAULT_LOAD_WAIT_MS)
                .clamp(100, batch::MAX_LOAD_WAIT_MS);
            let call = browser::probe::state_call();
            let started = Instant::now();
            let state_owned = state.to_string();
            let last = poll_until(app, &tab_id, call, Instant::now() + Duration::from_millis(timeout), {
                let state_owned = state_owned.clone();
                move |value| {
                    let ready = value.get("readyState").and_then(Value::as_str).unwrap_or("");
                    load_state_reached(state_owned.as_str(), ready)
                }
            })?;
            let mut record =
                load_state_record(state, started.elapsed().as_millis() as u64, &last);
            if let Some(object) = record.as_object_mut() {
                object.insert("tabId".to_string(), json!(tab_id));
            }
            Ok(record)
        }
        "browser_dialog" => {
            let tab_id = arg_str(args, "tabId")?;
            let accept = args
                .get("accept")
                .and_then(Value::as_bool)
                .ok_or("missing argument: accept (true to accept, false to dismiss)")?;
            let prompt_text = args.get("promptText").and_then(Value::as_str).map(str::to_string);
            let summary = if accept {
                "accept the page dialog"
            } else {
                "dismiss the page dialog"
            };
            if let Some(refusal) = control_gate(app, session_id, &tab_id, summary)? {
                return Ok(refusal);
            }
            browser::tool_call(app, &tab_id, move |s| {
                surface::resolve_dialog(s, accept, prompt_text.as_deref())
            })
            .map(|mut value| {
                if let Some(object) = value.as_object_mut() {
                    object.insert("tabId".to_string(), json!(tab_id));
                }
                value
            })
            .map_err(|e| format!("dialog failed: {e}"))
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
            if let Some(refusal) = control_gate(app, session_id, &tab_id, &summary)? {
                return Ok(refusal);
            }
            let request = ActionRequest { generation, target, action };
            browser::tool_call(app, &tab_id, move |s| surface::act(s, &request))
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
        "browser_set_value" => {
            let tab_id = arg_str(args, "tabId")?;
            let generation = arg_str(args, "generation")?;
            let target = arg_str(args, "ref")?;
            let value = args
                .get("value")
                .cloned()
                .ok_or_else(|| "missing argument: value".to_string())?;
            if let Some(refusal) = control_gate(app, session_id, &tab_id, "set a control's value")? {
                return Ok(refusal);
            }
            browser::tool_call(app, &tab_id, move |s| {
                surface::set_control_value(s, &generation, &target, &value)
            })
            .map(|mut value| {
                if let Some(object) = value.as_object_mut() {
                    object.insert("tabId".to_string(), json!(tab_id));
                }
                value
            })
            .map_err(|e| format!("set value failed: {e}"))
        }
        // ── Typed batch (M2): one call, several steps, same permissions ────
        "browser_batch" => {
            let tab_id = arg_str(args, "tabId")?;
            let generation = arg_str(args, "generation")?;
            let stop_on_error = args.get("stopOnError").and_then(Value::as_bool).unwrap_or(true);
            let steps = batch::validate(args.get("steps").unwrap_or(&Value::Null))?;
            let started = Instant::now();
            let mut records: Vec<Value> = Vec::new();
            let mut deadline = Instant::now() + Duration::from_millis(batch::BATCH_DEADLINE_MS);
            let mut stop: Option<(&'static str, usize)> = None;
            let mut final_generation = generation.clone();
            let mut generation_valid = true;
            for (index, step) in steps.iter().enumerate() {
                if batch::is_control(step) {
                    let summary = batch::step_summary(step);
                    // The permission wait must not burn the batch budget:
                    // freeze what is left and restore it after the card.
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if let Some(refusal) = control_gate(app, session_id, &tab_id, &summary)? {
                        let mut record = refusal;
                        record["index"] = json!(index);
                        record["op"] = json!(step.op());
                        record["ok"] = json!(false);
                        records.push(record);
                        stop = Some(("refused", index));
                        break;
                    }
                    deadline = Instant::now() + remaining;
                }
                if Instant::now() >= deadline {
                    stop = Some(("timeout", index));
                    break;
                }
                let record = execute_batch_step(
                    app,
                    &tab_id,
                    index,
                    step,
                    &generation,
                    deadline,
                    &mut final_generation,
                );
                match record {
                    Ok(record) => {
                        if record.get("error").and_then(Value::as_str) == Some("stale") {
                            generation_valid = false;
                        }
                        let ok = record.get("ok").and_then(Value::as_bool).unwrap_or(false);
                        // Audit trail: one line per executed Control step.
                        if batch::is_control(step) {
                            crate::debug_log::append(
                                "browser-batch",
                                "info",
                                session_id,
                                &format!("step {index} {}", step.op()),
                                Some(if ok { "ok" } else { "failed" }),
                            );
                        }
                        records.push(record);
                        if !ok && stop_on_error {
                            stop = Some(("error", index));
                            break;
                        }
                    }
                    Err(error) => {
                        records.push(json!({
                            "index": index,
                            "op": step.op(),
                            "ok": false,
                            "error": "step_failed",
                            "detail": error,
                        }));
                        stop = Some(("error", index));
                        break;
                    }
                }
            }
            // Final observation, best effort: the state the tab is in after
            // the chain (a refusal stop skips it — nothing was touched).
            let final_state = if stop.map(|(reason, _)| reason) == Some("refused") {
                None
            } else {
                browser::tool_read(app, &tab_id, |s| {
                    surface::probe(s, &browser::probe::state_call())
                })
                .ok()
            };
            Ok(json!({
                "tabId": tab_id,
                "ok": stop.is_none(),
                "stopReason": stop.map(|(reason, _)| reason),
                "stoppedAt": stop.map(|(_, index)| index),
                "steps": records,
                "generation": final_generation,
                "generationStillValid": generation_valid,
                "elapsedMs": started.elapsed().as_millis() as u64,
                "finalState": final_state,
            }))
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
        let instructions = result["instructions"].as_str().unwrap_or("");
        assert!(instructions.contains("Starting a local dev server does not authorize"));
        assert!(instructions.contains("explicitly asks"));
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
    fn always_approve_never_bypasses_user_tab_sharing() {
        use crate::browser::surface::GrantLevel;
        assert!(!control_auto_allowed(GrantLevel::None, false, true));
        assert!(!control_auto_allowed(GrantLevel::Read, false, true));
        assert!(control_auto_allowed(GrantLevel::Read, true, true));
        assert!(control_auto_allowed(GrantLevel::Control, false, false));
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
            "browser_help",
            "browser_list_tabs",
            "browser_open_tab",
            "browser_handoff",
            "browser_close_tab",
            "browser_snapshot",
            "browser_page_state",
            "browser_page_ready",
            "browser_wait_for",
            "browser_wait_for_load_state",
            "browser_dialog",
            "browser_console_messages",
            "browser_screenshot",
            "browser_click",
            "browser_hover",
            "browser_type",
            "browser_press_key",
            "browser_select_option",
            "browser_set_value",
            "browser_batch",
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
        let open = tools
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["name"] == "browser_open_tab")
            .unwrap();
        assert_eq!(
            open["inputSchema"]["properties"]["display"]["enum"],
            json!(["background", "workspace"])
        );
        assert!(open["inputSchema"]["properties"]["id"].is_object());
        assert!(open["inputSchema"]["properties"]["url"]["description"]
            .as_str()
            .unwrap_or_default()
            .contains("file:///"));
    }

    // browser_help is generated from the schema table, so the two can never
    // drift apart: the overview lists every shipped browser tool, and no
    // topic's prose may mention a browser_* tool that does not exist.
    #[test]
    fn help_covers_every_browser_tool_and_names_no_ghosts() {
        let names: Vec<String> = tool_schemas()
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .filter(|n| n.starts_with("browser_"))
            .map(str::to_string)
            .collect();

        let overview = help_payload("overview").unwrap();
        let listed: Vec<&str> = overview["tools"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        for name in &names {
            assert!(
                listed.contains(&name.as_str()),
                "help overview does not list {name}"
            );
        }
        assert_eq!(
            listed.len(),
            names.len(),
            "help lists a tool the schema table does not have"
        );

        for topic in HELP_TOPICS {
            let payload = help_payload(topic)
                .unwrap_or_else(|e| panic!("topic {topic} must exist: {e}"));
            let text = payload["text"].as_str().unwrap_or("");
            for word in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
                if word.starts_with("browser_") {
                    assert!(
                        names.contains(&word.to_string()),
                        "topic {topic} mentions unknown tool {word}"
                    );
                }
            }
            // A narrowed topic must not filter down to nothing — that would
            // mean its related-tool list rotted while the schema table moved.
            assert!(
                !payload["tools"].as_array().unwrap().is_empty(),
                "topic {topic} lists no tools"
            );
        }

        // Shipped capabilities ARE advertised: waits and batch landed with
        // their tools (2026-11-18), so their topics must resolve.
        assert!(help_payload("batch").is_ok());
        assert!(help_payload("waits").is_ok());
    }
}
