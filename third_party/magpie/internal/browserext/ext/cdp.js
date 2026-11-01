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
      inflight: new Map(), lastNet: 0, loading: false,
      stoppedByUser: false,
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
        if (!/already attached/i.test(m)) throw new Error(why(m));
      }
      t.attached = true;
      t.stoppedByUser = false;
      await Promise.all([
        send(tabId, "Page.enable"),
        send(tabId, "Runtime.enable"),
        send(tabId, "Network.enable"),
        send(tabId, "DOM.enable"),
        send(tabId, "Accessibility.enable"),
      ]);
      await send(tabId, "Page.setInterceptFileChooserDialog", { enabled: false }).catch(() => {});
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

export function send(tabId, method, params = {}) {
  return chrome.debugger.sendCommand({ tabId }, method, params).catch((e) => {
    const m = String(e && e.message || e);
    if (/Debugger is not attached|No tab with given id|detached/i.test(m)) {
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
  switch (method) {
    case "Network.requestWillBeSent":
      if (!STREAMING.has(p.type) && !/^data:|^blob:/.test(p.request.url)) t.inflight.set(p.requestId, Date.now());
      t.lastNet = Date.now();
      break;
    case "Network.loadingFinished":
    case "Network.loadingFailed":
      t.inflight.delete(p.requestId);
      t.lastNet = Date.now();
      if (method === "Network.loadingFailed" && !p.canceled && p.type === "Document") log(t, "network", `failed: ${p.errorText}`);
      break;
    case "Network.responseReceived":
      if (p.response.status >= 400) log(t, "network", `${p.response.status} ${p.type} ${p.response.url.slice(0, 300)}`);
      // a streamed answer (server-sent events, a long poll) isn't waited for
      if (/text\/event-stream/i.test(p.response.mimeType || "")) t.inflight.delete(p.requestId);
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
  // the user pressed Cancel on Chrome's bar: they want it stopped
  if (reason === "canceled_by_user") t.stoppedByUser = true;
});

function log(t, level, text) {
  t.logs.push({ at: Date.now(), level, text: String(text).slice(0, 2000) });
  if (t.logs.length > LOG_KEPT) t.logs.splice(0, t.logs.length - LOG_KEPT);
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export { sleep };

// settle waits for the page to go quiet after an action: no load under
// way and no request open for a short spell, or until the bound.
export async function settle(t, bound = 2500, quiet = 400) {
  const end = Date.now() + bound;
  await sleep(60);
  while (Date.now() < end) {
    const now = Date.now();
    for (const [id, at] of t.inflight) if (now - at > 10000) t.inflight.delete(id); // a long poll
    if (!t.loading && t.inflight.size === 0 && now - t.lastNet >= quiet) return;
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
