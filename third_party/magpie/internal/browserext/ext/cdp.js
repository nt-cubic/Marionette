// The DevTools protocol over chrome.debugger, one attachment per tab. A
// tab stays attached while an agent works in it and is let go after a
// quiet spell, so Chrome's "is debugging this browser" bar is up only
// while it is used.

const IDLE_MS = 2 * 60 * 1000;
const LOG_KEPT = 300;

// tabId → what is known of the page while attached
const tabs = new Map();

export function state(tabId) {
  let t = tabs.get(tabId);
  if (!t) {
    t = {
      tabId, attached: false, attaching: null, idle: 0,
      refs: new Map(), keys: new Map(), next: 1, doc: "",
      logs: [], dialog: null, answer: null, chooser: null,
      inflight: new Map(), lastNet: 0, loading: false, reqs: 0,
      stoppedByUser: false,
      // a frame from another site runs in a process of its own, reached
      // through a session of its own: frame id → session, and session →
      // { frame, parent session }
      children: new Map(), frameOf: new Map(),
      worlds: new Map(), // "session|frame" → the isolated world settle reads
      sentInput: false,
    };
    tabs.set(tabId, t);
  }
  return t;
}

export function forget(tabId) {
  const t = tabs.get(tabId);
  if (t) clearTimeout(t.idle);
  tabs.delete(tabId);
}

export async function attach(tabId) {
  const t = state(tabId);
  clearTimeout(t.idle);
  t.idle = setTimeout(() => detach(tabId), IDLE_MS);
  if (t.attached) return t;
  if (!t.attaching) {
    t.attaching = (async () => {
      try {
        await chrome.debugger.attach({ tabId }, "1.3");
      } catch (e) {
        const m = String(e && e.message || e);
        if (!/already attached/i.test(m)) throw fault("PAGE_BLOCKED", why(m));
      }
      t.attached = true;
      t.stoppedByUser = false;
      t.children.clear();
      t.frameOf.clear();
      t.worlds.clear();
      await Promise.all([
        send(tabId, "Page.enable"),
        send(tabId, "Runtime.enable"),
        send(tabId, "Network.enable"),
        send(tabId, "DOM.enable"),
        send(tabId, "Accessibility.enable"),
      ]);
      await send(tabId, "Page.setInterceptFileChooserDialog", { enabled: false }).catch(() => {});
      // frames from other sites: Chrome 125 and on hands an extension
      // their sessions; before it they stay unreadable, as said in the
      // snapshot
      await autoAttach(tabId);
    })().finally(() => { t.attaching = null; });
  }
  await t.attaching;
  return t;
}

function why(m) {
  if (/chrome:\/\/|chrome-extension:\/\/|Cannot access|Cannot attach/i.test(m)) {
    return "Chrome doesn't let an extension control this page (a chrome:// page, the Web Store or another extension's page): open a web page in the tab first";
  }
  if (/Another debugger is already attached/i.test(m)) {
    return "DevTools or another debugger is attached to this tab: close DevTools in it, or use another tab";
  }
  return m;
}

export async function detach(tabId) {
  const t = tabs.get(tabId);
  if (!t || !t.attached) return;
  t.attached = false;
  try { await chrome.debugger.detach({ tabId }); } catch {}
}

// autoAttach asks for the sessions of the frames from other sites that a
// page (or such a frame: session sid) holds, as each one appears
function autoAttach(tabId, sid) {
  return send(tabId, "Target.setAutoAttach", {
    autoAttach: true, waitForDebuggerOnStart: false, flatten: true,
    filter: [{ type: "iframe", exclude: false }, { exclude: true }],
  }, sid).catch(() => {});
}

async function child(t, sid) {
  await Promise.all([
    send(t.tabId, "Runtime.enable", {}, sid),
    send(t.tabId, "Network.enable", {}, sid),
    send(t.tabId, "DOM.enable", {}, sid),
    send(t.tabId, "Accessibility.enable", {}, sid),
  ]).catch(() => {});
  await autoAttach(t.tabId, sid);
}

// fault is an error the model is told the kind of, so it knows whether
// trying again can help: STALE_REF, AMBIGUOUS, UNACTIONABLE, BAD_ARGS,
// WAIT_TIMEOUT, SITE_REFUSED, LOAD_FAILED, PAGE_BLOCKED, USER_STOPPED,
// DIALOG_OPEN, OUTCOME_UNKNOWN, SCRIPT_ERROR
export function fault(code, message) {
  const e = new Error(message);
  e.code = code;
  return e;
}

// send sends a command to the tab, or to the session of a frame from
// another site in it (sid)
export function send(tabId, method, params = {}, sid) {
  const to = sid ? { tabId, sessionId: sid } : { tabId };
  if (method.startsWith("Input.")) {
    const t = tabs.get(tabId);
    if (t) t.sentInput = true;
  }
  return chrome.debugger.sendCommand(to, method, params).catch((e) => {
    const m = String(e && e.message || e);
    if (!sid && /Debugger is not attached|No tab with given id|detached/i.test(m)) {
      const t = tabs.get(tabId);
      if (t) t.attached = false;
    }
    throw new Error(m.replace(/^\{"code":-?\d+,"message":"(.*)"\}$/, "$1"));
  });
}

// requests that never end on their own: a page with one open is still
// quiet
const STREAMING = new Set(["EventSource", "WebSocket", "Ping", "CSPViolationReport", "Preflight"]);

chrome.debugger.onEvent.addListener((src, method, p) => {
  const t = tabs.get(src.tabId);
  if (!t) return;
  const sid = src.sessionId;
  // a frame's own session: its requests, logs and frames count; its
  // page events aren't the tab's
  if (sid && method.startsWith("Page.")) return;
  const req = (id) => (sid ? sid + ":" + id : id);
  switch (method) {
    case "Target.attachedToTarget": {
      const info = p.targetInfo || {};
      if (info.type !== "iframe") break;
      t.children.set(info.targetId, p.sessionId);
      t.frameOf.set(p.sessionId, { frame: info.targetId, parent: sid });
      child(t, p.sessionId);
      break;
    }
    case "Target.detachedFromTarget": {
      const f = t.frameOf.get(p.sessionId);
      if (f) {
        if (t.children.get(f.frame) === p.sessionId) t.children.delete(f.frame);
        t.frameOf.delete(p.sessionId);
      }
      for (const k of t.worlds.keys()) if (k.startsWith(p.sessionId + "|")) t.worlds.delete(k);
      break;
    }
    case "Network.requestWillBeSent":
      if (!STREAMING.has(p.type) && !/^data:|^blob:/.test(p.request.url)) t.inflight.set(req(p.requestId), Date.now());
      t.lastNet = Date.now();
      t.reqs++;
      break;
    case "Network.loadingFinished":
    case "Network.loadingFailed":
      t.inflight.delete(req(p.requestId));
      t.lastNet = Date.now();
      if (method === "Network.loadingFailed" && !p.canceled && p.type === "Document") log(t, "network", `failed: ${p.errorText}`);
      break;
    case "Network.responseReceived":
      if (p.response.status >= 400) log(t, "network", `${p.response.status} ${p.type} ${p.response.url.slice(0, 300)}`);
      // what the site answered the page itself with
      if (!sid && p.type === "Document" && (!t.mainFrame || p.frameId === t.mainFrame)) t.docStatus = p.response.status;
      // a streamed answer (server-sent events, a long poll) isn't waited for
      if (/text\/event-stream/i.test(p.response.mimeType || "")) t.inflight.delete(req(p.requestId));
      break;
    case "Page.frameStartedLoading":
      if (!p.frameId || p.frameId === t.mainFrame) t.loading = true;
      break;
    case "Page.loadEventFired":
      t.loading = false;
      break;
    case "Page.frameNavigated":
      if (!p.frame.parentId) {
        t.mainFrame = p.frame.id;
        t.inflight.clear();
      }
      break;
    case "Runtime.consoleAPICalled": {
      const text = (p.args || []).map((a) => a.value !== undefined ? String(a.value) : (a.description || a.type)).join(" ");
      log(t, p.type === "warning" ? "warn" : p.type, text);
      break;
    }
    case "Runtime.exceptionThrown": {
      const d = p.exceptionDetails || {};
      log(t, "error", (d.exception && d.exception.description) || d.text || "exception");
      break;
    }
    case "Page.javascriptDialogOpening":
      if (t.answer) {
        const a = t.answer;
        t.answer = null;
        send(src.tabId, "Page.handleJavaScriptDialog", { accept: a.accept, promptText: a.promptText }).catch(() => {});
        t.answered = { type: p.type, message: p.message, accept: a.accept };
      } else {
        t.dialog = { type: p.type, message: p.message, defaultPrompt: p.defaultPrompt };
      }
      break;
    case "Page.javascriptDialogClosed":
      t.dialog = null;
      break;
    case "Page.fileChooserOpened":
      if (t.chooser) t.chooser(p);
      break;
  }
});

chrome.debugger.onDetach.addListener((src, reason) => {
  const t = tabs.get(src.tabId);
  if (!t) return;
  t.attached = false;
  t.children.clear();
  t.frameOf.clear();
  t.worlds.clear();
  // the user pressed Cancel on Chrome's bar: they want it stopped
  if (reason === "canceled_by_user") t.stoppedByUser = true;
});

function log(t, level, text) {
  t.logs.push({ at: Date.now(), level, text: String(text).slice(0, 2000) });
  if (t.logs.length > LOG_KEPT) t.logs.splice(0, t.logs.length - LOG_KEPT);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export { sleep };

// A MutationObserver in an isolated world of a frame: the page can't see
// it, and it counts what the page changes. Each read answers [how many
// changes so far, ms since the last one].
const WATCH = `(() => {
  if (!window.__magpieWatch) {
    const w = window.__magpieWatch = { n: 0, at: 0 };
    new MutationObserver((r) => { w.n += r.length; w.at = Date.now(); })
      .observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
  }
  const w = window.__magpieWatch;
  return [w.n, w.at ? Date.now() - w.at : 1e9];
})()`;

async function mainFrame(t) {
  if (t.mainFrame) return t.mainFrame;
  try { t.mainFrame = (await send(t.tabId, "Page.getFrameTree")).frameTree.frame.id; } catch {}
  return t.mainFrame;
}

// watch reads the changes in a frame (the tab's main frame without one),
// or null when it can't be read
export async function watch(t, frame, sid) {
  if (!frame) frame = sid ? t.frameOf.get(sid)?.frame : await mainFrame(t);
  if (!frame) return null;
  const k = (sid || "") + "|" + frame;
  for (let i = 0; i < 2; i++) {
    let ctx = t.worlds.get(k);
    if (!ctx) {
      try {
        ctx = (await send(t.tabId, "Page.createIsolatedWorld", { frameId: frame, worldName: "magpie-watch" }, sid)).executionContextId;
        t.worlds.set(k, ctx);
      } catch { return null; }
    }
    try {
      const r = await send(t.tabId, "Runtime.evaluate", { expression: WATCH, contextId: ctx, returnByValue: true }, sid);
      if (r.result && Array.isArray(r.result.value)) return r.result.value;
    } catch {}
    t.worlds.delete(k); // a new document: a new world
  }
  return null;
}

// settle waits for the page to go quiet after an action: no load under
// way, no request open and nothing on the page changed for a short spell
// (in the frames given, the main one besides), or until the bound. A
// frame that can't be watched is waited for by its requests alone.
export async function settle(t, bound = 2500, quiet = 400, frames = []) {
  const end = Date.now() + bound;
  await sleep(60);
  const watched = [{}, ...frames.filter((f) => f && (f.frame || f.sid))];
  while (Date.now() < end) {
    const now = Date.now();
    for (const [id, at] of t.inflight) if (now - at > 10000) t.inflight.delete(id); // a long poll
    if (!t.loading && t.inflight.size === 0 && now - t.lastNet >= quiet) {
      const reads = await Promise.all(watched.map((f) => watch(t, f.frame, f.sid)));
      if (reads.every((r) => !r || r[1] >= quiet)) return;
    }
    await sleep(80);
  }
}

// waitLoad waits for the main frame to finish loading after a navigation.
export async function waitLoad(t, bound = 30000) {
  const end = Date.now() + bound;
  await sleep(100);
  while (Date.now() < end) {
    if (!t.loading) {
      try {
        const r = await send(t.tabId, "Runtime.evaluate", { expression: "document.readyState", returnByValue: true });
        if (r.result.value === "complete" || r.result.value === "interactive") return true;
      } catch {}
    }
    await sleep(150);
  }
  return false;
}
