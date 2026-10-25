#!/usr/bin/env node
//! Web Chat Bridge — 把 Marionette 会话桥接到 ChatGPT / DeepSeek 网页版（纯聊天）。
//!
//! 用法：node bridge.mjs <chatgpt|deepseek|echo|echo2>
//!       node bridge.mjs <chatgpt|deepseek> --open  只打开标签页，不发送消息
//!   chatgpt / deepseek — 真·网页版；echo / echo2 — 本地自测页面（无外网、无登录，
//!   echo2 专供「双开两座桥验证共享浏览器」用）。
//!
//! 形态：一个 ACP agent（stdin/stdout 上换行分隔 JSON-RPC，与 acp.rs 的
//! 读写口径一致）。Marionette 发 initialize / session/new / session/prompt /
//! session/cancel；桥用 CDP 驱动真 Edge/Chrome——chatgpt / deepseek 两个桥
//! **共用同一个浏览器实例**（一个窗口、每个会话一个标签页）：先启动的负责拉起，
//! 后到的从 DevToolsActivePort 读端口直接 attach；桥退出只断开连接，浏览器由
//! 用户关闭，挂起/重连不会重启浏览器。BRIDGE_CDP_URL 可连接已开放调试的浏览器。
//! 专用窗口首次从同种日常浏览器复制配置，之后固定复用（登录可能需要重新认证）。
//! 把用户的话敲进网页输入框，把流式回复抄回
//! session/update 增量 chunk。
//!
//! 不接模型共享引擎、不充当 provider、不给任何 agent 供能 —— 只当聊天窗。
//!
//! 行为约定（与壳的注入对齐，见 src/lib/sessionHistory.ts / memory.rs）：
//!   - prompt 首块可能是 <marionette-memory>…</marionette-memory> 记忆卡 → 剥掉；
//!   - 挂起重连的首条 prompt 带 [Marionette — prior conversation …] 历史注入
//!     → 剥到 [End of prior conversation…] 标记之后，只发用户新话；
//!   - 图片块与宿主显式选择的文件通过网页附件入口上传，完成后才提交。
//!
//! 环境变量：
//!   BRIDGE_BROWSER       浏览器 exe 绝对路径（默认自动找 Edge → Chrome）
//!   BRIDGE_CDP_URL       连接已有浏览器的本机 CDP HTTP / WebSocket 地址，不另开窗口
//!   BRIDGE_HEADLESS=1    无头跑（看不到窗口；人机验证时没法手动救）
//!   BRIDGE_POLL_MS       轮询间隔（默认 700）
//!   BRIDGE_MAX_WAIT_S    单回合最长等待（默认 300）
//!   BRIDGE_STATE_DIR     状态/用户目录根（默认 %LOCALAPPDATA%\MarionetteWebBridge）
//!   BRIDGE_CLONE_FROM    首装克隆源 User Data 目录（默认自动找日常 Edge → Chrome）
//!   MARIONETTE_SESSION_ID 宿主注入的稳定会话编号，用于重启后恢复对应网页对话

import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import readline from "node:readline";
import { prepareAttachments, uploadAttachments, clearUploadedAttachments } from "./attachments.mjs";
import { createChatgptReplyCapture, chatgptRootsJs } from "./rich-reply.mjs";
import { extractMarkdownSnippet } from "./markdown-reply.mjs";

const SITE_KEY = (process.argv[2] || "").trim().toLowerCase();
const VERSION = "0.4.0";

/**
 * 抄回复用的序列化器：纯 textContent 遍历，不碰 innerText —— detached clone
 * 上 innerText 恒为空（无布局），后台标签页布局被节流时也不可靠。`<pre>`
 * 还原成 ``` 围栏，块级元素补换行，表格单元格补 tab。
 */
function extractSnippet(selector, leafOnly = false) {
  return `(() => {
  const nodes = [...document.querySelectorAll(${JSON.stringify(selector)})]${leafOnly ? `.filter(n => !n.querySelector(${JSON.stringify(selector)}))` : ""};
  const BLOCK = new Set(["P","DIV","LI","UL","OL","TR","TABLE","THEAD","TBODY","TFOOT","H1","H2","H3","H4","H5","H6","BLOCKQUOTE","SECTION","ARTICLE","FIGURE","FIGCAPTION","HR","DL","DT","DD","PRE"]);
  const walk = (el) => {
    let out = "";
    for (const child of el.childNodes) {
      if (child.nodeType === 3) { out += child.textContent; continue; }
      if (child.nodeType !== 1) continue;
      const tag = child.tagName;
      if (tag === "PRE") {
        out += "\\n\`\`\`\\n" + (child.textContent || "") + "\\n\`\`\`\\n";
      } else if (tag === "BR") {
        out += "\\n";
      } else {
        out += walk(child);
        if (tag === "TD" || tag === "TH") out += "\\t";
        if (BLOCK.has(tag)) out += "\\n";
      }
    }
    return out;
  };
  const fmt = (n) => walk(n).replace(/\\n{3,}/g, "\\n\\n").trim()${leafOnly ? ' || (n.querySelector("img") ? "[图片回复]" : "")' : ""};
  return JSON.stringify(nodes.map(fmt));
})()`;
}

// ── 站点适配 ────────────────────────────────────────────────────────────
// 每个站点给一组「页面里执行的 JS 片段」构造器，统一返回 JSON 字符串。
// 选择器尽量用 id / data-testid 这类稳定锚点，构造器内自带回退链。

// echo / echo2 这类本地自测站：页面没有可辨认的 URL/选择器锚点，靠页面里的
// <title> 找回自己的 tab（两座自测桥同时跑在共享浏览器里也不串）。
const echoTitleSel = (title) => `(() => {
  const t = document.title;
  return JSON.stringify({ isMine: t === ${JSON.stringify(title)} });
})()`;

// ChatGPT 回复节点选择器：2026-10 起新 UI 用 data-markdown-text-style，
// 旧 UI 用 data-message-author-role；两条都留着，哪个在线都能接住。
const CHATGPT_ASSISTANT_SEL =
  'div[data-message-author-role="assistant"], div[data-markdown-text-style="assistant-message"]';
// DeepSeek renders reasoning as a separate `div.ds-markdown` inside
// `.ds-think-content`; only the final answer carries this semantic class.
const DEEPSEEK_ASSISTANT_SEL = "div.ds-markdown.ds-assistant-message-main-content";
const DEEPSEEK_THOUGHT_SEL = ".ds-think-content div.ds-markdown";

const SITES = {
  chatgpt: {
    label: "ChatGPT 网页版",
    home: "https://chatgpt.com/",
    // 发送后 URL 变成 /c/<uuid>；把它存进 state，重启后接着上次聊。
    conversationUrlRe: /chatgpt\.com\/c\/[0-9a-f-]{8,}/i,
    loginCheckJs: () => `(() => {
      const url = location.href;
      if (/\\/auth\\//.test(url)) return JSON.stringify({ ok: false });
      if (document.querySelector('input[name="password"]')) return JSON.stringify({ ok: false });
      if (document.querySelector('a[href*="/auth/login"], [data-testid="login-button"]')) {
        return JSON.stringify({ ok: false });
      }
      return JSON.stringify({ ok: true });
    })()`,
    composerOk: `document.querySelector('#prompt-textarea, div.ProseMirror[contenteditable="true"]')`,
    sendJs: (text) => `(() => {
      const TEXT = ${JSON.stringify(text)};
      const el = document.querySelector('#prompt-textarea, div.ProseMirror[contenteditable="true"]');
      if (!el) return JSON.stringify({ ok: false, why: "composer" });
      el.focus();
      const dt = new DataTransfer();
      dt.setData("text/plain", TEXT);
      el.dispatchEvent(new ClipboardEvent("paste", { clipboardData: dt, bubbles: true, cancelable: true }));
      return JSON.stringify({ ok: true });
    })()`,
    submitJs: (preferButton) => `(() => {
      const el = document.querySelector('#prompt-textarea, div.ProseMirror[contenteditable="true"]');
      if (!el) return JSON.stringify({ ok: false });
      if (${preferButton ? "true" : "false"}) {
        const scope = el.closest('form') || el.closest('[data-composer-surface-variant], [data-composer-root]') || document;
        const btn = scope.querySelector('button[data-testid="send-button"], button[aria-label="发送"], button[aria-label="发送消息"], button[aria-label="发送提示"], button[aria-label="Send"], button[aria-label="Send prompt"], button[aria-label="Send message"], button[type="submit"]');
        if (btn && !btn.disabled && btn.getAttribute("aria-disabled") !== "true") { btn.click(); return JSON.stringify({ ok: true, method: "button" }); }
      }
      const ev = new KeyboardEvent("keydown", { key: "Enter", code: "Enter", bubbles: true, cancelable: true });
      el.dispatchEvent(ev);
      return JSON.stringify({ ok: true, method: "enter" });
    })()`,
    // 「这条消息真的发出去了吗」：composer 空了 = 发出去了。
    composerLenJs: `(() => {
      const el = document.querySelector('#prompt-textarea, div.ProseMirror[contenteditable="true"]');
      return JSON.stringify({ len: el ? (el.innerText || "").trim().length : -1 });
    })()`,
    preCountJs: () => `(() => {
      const nodes = ${chatgptRootsJs};
      return JSON.stringify({ lens: nodes.map(n => (n.innerText || "").length), url: location.href });
    })()`,
    pollJs: `(() => {
      const nodes = ${chatgptRootsJs};
      const streaming = !!document.querySelector(
        'button[data-testid="stop-button"], button[aria-label*="Stop" i], button[aria-label*="停止"]');
      return JSON.stringify({
        lens: nodes.map(n => (n.innerText || "").length),
        streaming,
        url: location.href
      });
    })()`,
    extractJs: extractSnippet(CHATGPT_ASSISTANT_SEL, true),
    stopJs: `(() => {
      const btn = document.querySelector('button[data-testid="stop-button"]');
      if (btn) { btn.click(); return JSON.stringify({ ok: true }); }
      document.body.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
      return JSON.stringify({ ok: true, method: "escape" });
    })()`,
  },

  deepseek: {
    label: "DeepSeek 网页版",
    home: "https://chat.deepseek.com/",
    newChatUrl: "https://chat.deepseek.com/a/chat",
    conversationUrlRe: /chat\.deepseek\.com\/a\/chat\/s\//i,
    loginCheckJs: () => `(() => {
      if (location.pathname.startsWith("/login") || /sign_in|login/.test(location.href)) {
        return JSON.stringify({ ok: false });
      }
      if (document.querySelector('input[type="password"]')) return JSON.stringify({ ok: false });
      return JSON.stringify({ ok: true });
    })()`,
    composerOk: `document.querySelector('#chat-input') || [...document.querySelectorAll('textarea')].find(t => t.offsetParent)`,
    sendJs: (text) => `(() => {
      const TEXT = ${JSON.stringify(text)};
      const ta = document.querySelector('#chat-input')
        || [...document.querySelectorAll('textarea')].find(t => t.offsetParent);
      if (!ta) return JSON.stringify({ ok: false, why: "composer" });
      const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set;
      setter.call(ta, TEXT);
      ta.dispatchEvent(new Event("input", { bubbles: true }));
      ta.focus();
      return JSON.stringify({ ok: true });
    })()`,
    submitJs: (preferButton) => `(() => {
      const ta = document.querySelector('#chat-input')
        || [...document.querySelectorAll('textarea')].find(t => t.offsetParent);
      if (!ta) return JSON.stringify({ ok: false });
      if (${preferButton ? "true" : "false"}) {
        let scope = ta.parentElement;
        while (scope && scope !== document.body && !scope.querySelector('input[type="file"]')) scope = scope.parentElement;
        const btn = scope?.querySelector('[role="button"].ds-button--primary.ds-button--circle, button[type="submit"]');
        if (btn && !btn.disabled && btn.getAttribute("aria-disabled") !== "true" &&
            !btn.classList.contains("ds-button--disabled")) {
          btn.click();
          return JSON.stringify({ ok: true, method: "button" });
        }
      }
      const opts = { key: "Enter", code: "Enter", bubbles: true, cancelable: true };
      const down = new KeyboardEvent("keydown", opts);
      const up = new KeyboardEvent("keyup", opts);
      for (const ev of [down, up]) {
        Object.defineProperty(ev, "keyCode", { get: () => 13 });
        Object.defineProperty(ev, "which", { get: () => 13 });
      }
      ta.dispatchEvent(down);
      ta.dispatchEvent(up);
      return JSON.stringify({ ok: true, method: "enter" });
    })()`,
    composerLenJs: `(() => {
      const ta = document.querySelector('#chat-input')
        || [...document.querySelectorAll('textarea')].find(t => t.offsetParent);
      return JSON.stringify({ len: ta ? ta.value.length : -1 });
    })()`,
    preCountJs: () => `(() => {
      const nodes = [...document.querySelectorAll(${JSON.stringify(DEEPSEEK_ASSISTANT_SEL)})];
      return JSON.stringify({ lens: nodes.map(n => (n.innerText || "").length), url: location.href });
    })()`,
    pollJs: `(() => {
      const nodes = [...document.querySelectorAll(${JSON.stringify(DEEPSEEK_ASSISTANT_SEL)})];
      return JSON.stringify({
        lens: nodes.map(n => (n.innerText || "").length),
        streaming: false,
        url: location.href
      });
    })()`,
    extractJs: extractMarkdownSnippet(DEEPSEEK_ASSISTANT_SEL),
    textSnapshot: true,
    thoughtExtractJs: extractSnippet(DEEPSEEK_THOUGHT_SEL),
    stopJs: `(() => {
      const ta = document.querySelector('#chat-input');
      const target = ta || document.body;
      const ev = new KeyboardEvent("keydown", { key: "Escape", code: "Escape", bubbles: true, cancelable: true });
      Object.defineProperty(ev, "keyCode", { get: () => 27 });
      target.dispatchEvent(ev);
      return JSON.stringify({ ok: true, method: "escape" });
    })()`,
  },

  // 本地自测站点：about:blank 上现场搭一个假聊天页（含流式 + <pre>）。
  // prepareJs / isMyPageJs 在本表后面统一装配（标题参数化，echo / echo2 复用）。
  echo: {
    label: "Echo（自测）",
    home: "about:blank",
    conversationUrlRe: /(?!.*)/, // 永不命中
    loginCheckJs: () => `(() => JSON.stringify({ ok: true }))()`,
    composerOk: `document.querySelector('#chat-input')`,
    sendJs: (text) => `(() => {
      const TEXT = ${JSON.stringify(text)};
      const ta = document.querySelector('#chat-input');
      if (!ta) return JSON.stringify({ ok: false, why: "composer" });
      const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set;
      setter.call(ta, TEXT);
      ta.dispatchEvent(new Event("input", { bubbles: true }));
      return JSON.stringify({ ok: true });
    })()`,
    submitJs: () => `(() => {
      document.querySelector('#send').click();
      return JSON.stringify({ ok: true, method: "button" });
    })()`,
    composerLenJs: `(() => {
      const ta = document.querySelector('#chat-input');
      return JSON.stringify({ len: ta ? ta.value.length : -1 });
    })()`,
    preCountJs: () => `(() => {
      const nodes = [...document.querySelectorAll("div.ds-markdown")];
      return JSON.stringify({ lens: nodes.map(n => (n.innerText || "").length), url: location.href });
    })()`,
    pollJs: `(() => {
      const nodes = [...document.querySelectorAll("div.ds-markdown")];
      return JSON.stringify({
        lens: nodes.map(n => (n.innerText || "").length),
        streaming: !!window.__busy,
        url: location.href
      });
    })()`,
    extractJs: extractSnippet("div.ds-markdown"),
    stopJs: `(() => { window.__busy = false; return JSON.stringify({ ok: true }); })()`,
  },
};

// ── echo / echo2 自测站装配：同一段假页面代码，标题不同 ─────────────────
// 共享浏览器里两座自测桥靠 <title> 分清各自的 tab。echo2 的存在是为了双开
// 两座桥、验证「共用一个浏览器实例」本身。
function echoPrepareJs(title) {
  return `(() => {
  if (window.__echoReady) return JSON.stringify({ ok: true });
  document.title = ${JSON.stringify(title)};
  document.body.innerHTML = [
    '<textarea id="chat-input"></textarea>',
    '<button id="send"></button>',
    '<div id="out"></div>'
  ].join("");
  const out = document.getElementById("out");
  document.getElementById("send").addEventListener("click", () => {
    const text = document.getElementById("chat-input").value;
    document.getElementById("chat-input").value = "";
    const div = document.createElement("div");
    div.className = "ds-markdown";
    out.appendChild(div);
    // 假流式：每 80ms 吐两个字，末尾带一段 <pre> 测围栏还原。
    const full = "PONG:" + text.split("").reverse().join("") + "\\n\\nFENCE:\\n";
    const pre = document.createElement("pre");
    pre.textContent = "print(\\"hello\\")";
    let i = 0;
    window.__busy = true;
    const tick = () => {
      if (!window.__busy) return;
      i = Math.min(full.length, i + 2);
      div.textContent = full.slice(0, i);
      if (i >= full.length) { div.appendChild(pre); window.__busy = false; return; }
      setTimeout(tick, 80);
    };
    setTimeout(tick, 80);
  });
  window.__echoReady = true;
  return JSON.stringify({ ok: true });
})()`;
}
SITES.echo.prepareJs = echoPrepareJs("Marionette Echo 1");
SITES.echo.isMyPageJs = echoTitleSel("Marionette Echo 1");
SITES.echo2 = {
  ...SITES.echo,
  label: "Echo 2（自测）",
  prepareJs: echoPrepareJs("Marionette Echo 2"),
  isMyPageJs: echoTitleSel("Marionette Echo 2"),
};

const site = SITES[SITE_KEY];
if (!site) {
  console.error(`usage: node bridge.mjs <chatgpt|deepseek|echo|echo2> (got "${SITE_KEY}")`);
  process.exit(2);
}

// ── 日志（落文件，不进 stderr —— stderr 会被壳当 agent 输出事件收走）──────
const STATE_ROOT = process.env.BRIDGE_STATE_DIR
  || path.join(process.env.LOCALAPPDATA || path.join(process.env.USERPROFILE || ".", "AppData", "Local"),
    "MarionetteWebBridge");
fs.mkdirSync(STATE_ROOT, { recursive: true });
const captureChatgptReply = createChatgptReplyCapture(STATE_ROOT);
const LOG_FILE = path.join(STATE_ROOT, `bridge-${SITE_KEY}.log`);
function log(level, msg) {
  const line = `${new Date().toISOString()} [${level}] ${msg}`;
  try { fs.appendFileSync(LOG_FILE, line + "\n"); } catch { /* 尽力而为 */ }
  if (level === "error") console.error(`[web-bridge ${SITE_KEY}] ${msg}`);
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const POLL_MS = Math.max(200, Number(process.env.BRIDGE_POLL_MS) || 700);
const MAX_WAIT_MS = Math.max(30, Number(process.env.BRIDGE_MAX_WAIT_S) || 300) * 1000;
const STABLE_NEEDED = 3; // 连续 3 拍无增长 ≈ 回复收尾（思考型模型的段落间停顿也扛得住）
// 页面上一个回复节点都没有时不能判稳：模型可能还在思考，首个节点要几秒才出现
// （ChatGPT Medium 档实测）。宽限期内继续等，超了才报「没等到回复」。
const NO_REPLY_GRACE_MS = 25_000;

// 每个 Marionette 会话独立绑定网页。旧的每站 state 文件只供 --open 使用，
// 不能迁给新会话，否则新建空白会话又会接上旧网页。无宿主编号的 ACP 客户端
// 每次 session/new 都从空白开始，不猜测它属于哪条历史对话。
function conversationContext(key, preview = false) {
  const suffix = preview ? "" : `-${createHash("sha256").update(key).digest("hex")}`;
  const file = path.join(STATE_ROOT, `state-${SITE_KEY}${suffix}.json`);
  let state = {};
  try {
    const saved = JSON.parse(fs.readFileSync(file, "utf8"));
    if (saved && typeof saved === "object" && !Array.isArray(saved)) state = saved;
  } catch { /* 新会话 / 不可读状态：从空白开始 */ }
  return { file, state, page: null, preview };
}
function saveState(context) {
  const temp = `${context.file}.${process.pid}.tmp`;
  fs.writeFileSync(temp, JSON.stringify(context.state, null, 2));
  fs.renameSync(temp, context.file);
}
function rememberConversation(context, url) {
  if (url && site.conversationUrlRe.test(url) && url !== context.state.conversationUrl) {
    context.state.conversationUrl = url;
    saveState(context);
  }
}

// ── CDP（DevTools 协议）最小客户端：Node 22+ 内置 WebSocket，零依赖 ─────
class Cdp {
  constructor(wsUrl) {
    this.ws = new WebSocket(wsUrl);
    this.nextId = 1;
    this.pending = new Map();
    this.openPromise = new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        reject(new Error("CDP connection timed out"));
        this.abandon();
      }, 5000);
      this.ws.addEventListener("open", () => { clearTimeout(timer); resolve(); });
      this.ws.addEventListener("error", () => { clearTimeout(timer); reject(new Error("CDP websocket error")); });
      this.ws.addEventListener("close", () => { clearTimeout(timer); reject(new Error("CDP websocket closed")); });
    });
    this.ws.addEventListener("message", (ev) => {
      let msg;
      try { msg = JSON.parse(typeof ev.data === "string" ? ev.data : String(ev.data)); }
      catch { return; }
      if (msg.id != null && this.pending.has(msg.id)) {
        const { resolve, reject, timer } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        clearTimeout(timer);
        if (msg.error) reject(new Error(`CDP ${msg.error.message || "error"}`));
        else resolve(msg.result ?? {});
      }
    });
    this.ws.addEventListener("close", () => {
      for (const { reject, timer } of this.pending.values()) {
        clearTimeout(timer);
        reject(new Error("CDP websocket closed"));
      }
      this.pending.clear();
    });
  }
  get alive() { return this.ws.readyState === 1; }
  async send(method, params = {}, sessionId = undefined) {
    await this.openPromise;
    if (!this.alive) throw new Error("CDP websocket closed");
    const id = this.nextId++;
    const payload = { id, method, params };
    if (sessionId) payload.sessionId = sessionId;
    const done = new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        if (this.pending.delete(id)) reject(new Error(`CDP ${method} timed out`));
      }, 30_000);
      this.pending.set(id, { resolve, reject, timer });
    });
    this.ws.send(JSON.stringify(payload));
    return done;
  }
  /** attach 失败时丢弃这条连接：不让后续 close 事件干扰流程。 */
  abandon() {
    this.openPromise.catch(() => {});
    try { this.ws.close(); } catch { /* 忽略 */ }
  }
}

// ── 浏览器进程管理：找 exe → 拉起 → 等 DevToolsActivePort → 连 WS ───────
function findBrowserExe() {
  if (process.env.BRIDGE_BROWSER) return process.env.BRIDGE_BROWSER;
  const pf = process.env["ProgramFiles"] || "C:\\Program Files";
  const pf86 = process.env["ProgramFiles(x86)"] || "C:\\Program Files (x86)";
  const lf = process.env.LOCALAPPDATA || "";
  return [
    path.join(pf86, "Microsoft", "Edge", "Application", "msedge.exe"),
    path.join(pf, "Microsoft", "Edge", "Application", "msedge.exe"),
    path.join(pf, "Google", "Chrome", "Application", "chrome.exe"),
    path.join(pf86, "Google", "Chrome", "Application", "chrome.exe"),
    path.join(lf, "Google", "Chrome", "Application", "chrome.exe"),
  ].find((p) => p && fs.existsSync(p));
}

// 浏览器持续保留；桥只持有自己的连接，不拥有浏览器生命周期。
let browser = null;
let browserIsExternal = false;
const SHARED_PROFILE = path.join(STATE_ROOT, "bridge-profile");
const SITE_GROUP = ["chatgpt", "deepseek"].includes(SITE_KEY) ? "shared" : "echo";
const PROFILE_DIR = SITE_GROUP === "shared" ? SHARED_PROFILE : path.join(STATE_ROOT, "profile-echo");
const LAUNCH_LOCK = path.join(STATE_ROOT, `browser-launch-${SITE_GROUP}.json`);

function pidAlive(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return false;
  try { process.kill(pid, 0); return true; } catch (err) { return err?.code === "EPERM"; }
}

// 只在冷启动期间互斥：锁内再检查一次端口，防止同时复制/启动。
async function withLaunchLock(work) {
  const deadline = Date.now() + 120_000;
  while (true) {
    let fd;
    try {
      fd = fs.openSync(LAUNCH_LOCK, "wx");
    } catch (err) {
      if (err.code !== "EEXIST") throw err;
      try {
        const stat = fs.statSync(LAUNCH_LOCK);
        let owner;
        try { owner = JSON.parse(fs.readFileSync(LAUNCH_LOCK, "utf8")); } catch { /* 尚未写完 */ }
        if ((owner && !pidAlive(owner.pid)) || (!owner && Date.now() - stat.mtimeMs > 5000)) {
          fs.rmSync(LAUNCH_LOCK, { force: true });
          continue;
        }
      } catch (lockErr) { if (lockErr.code === "ENOENT") continue; throw lockErr; }
      if (Date.now() >= deadline) throw new Error("另一座桥正在启动浏览器，请稍后重试。");
      await sleep(200);
      continue;
    }
    try {
      fs.writeFileSync(fd, JSON.stringify({ pid: process.pid }));
      return await work();
    } finally {
      fs.closeSync(fd);
      try {
        if (JSON.parse(fs.readFileSync(LAUNCH_LOCK, "utf8")).pid === process.pid) fs.rmSync(LAUNCH_LOCK);
      } catch { /* 退出或外部清理 */ }
    }
  }
}

function localCdpUrl(value) {
  const url = new URL(value);
  if (!["http:", "ws:"].includes(url.protocol) ||
      !["localhost", "127.0.0.1", "[::1]"].includes(url.hostname) || url.username || url.password) {
    throw new Error("BRIDGE_CDP_URL 必须是本机 http:// 或 ws:// 调试地址。");
  }
  return url;
}

async function attachToBrowser(endpoint, expectedPath) {
  const url = localCdpUrl(endpoint);
  let wsUrl = url.href;
  if (url.protocol === "http:") {
    const res = await fetch(new URL("/json/version", url), { signal: AbortSignal.timeout(1500) });
    if (!res.ok) throw new Error(`CDP HTTP ${res.status}`);
    wsUrl = localCdpUrl((await res.json()).webSocketDebuggerUrl).href;
  }
  // 残留端口可能已被其他程序占用，必须匹配浏览器 id。
  if (expectedPath && new URL(wsUrl).pathname !== expectedPath) throw new Error("stale DevToolsActivePort");
  const cdp = new Cdp(wsUrl);
  try { await cdp.openPromise; return cdp; }
  catch (err) { cdp.abandon(); throw err; }
}

async function attachToProfile(profileDir) {
  try {
    const [port, wsPath] = fs.readFileSync(path.join(profileDir, "DevToolsActivePort"), "utf8").trim().split(/\r?\n/);
    if (!/^\d+$/.test(port) || !wsPath?.startsWith("/devtools/browser/")) return null;
    return await attachToBrowser(`http://127.0.0.1:${port}`, wsPath);
  } catch { return null; }
}

// 首次复制同种浏览器的配置，之后固定复用；凭据可能需要重新认证。
const SEEDED_FILE = path.join(SHARED_PROFILE, ".seeded-from");
const CLONE_SKIP_DIRS = new Set([
  "Cache", "Cache_Data", "Code Cache", "GPUCache", "GrShaderCache", "ShaderCache",
  "Crashpad", "BrowserMetrics", "Snapshots", "component_crx_cache",
  "extensions_crx_cache", "WidevineCdm", "OptimizationGuidePredictionModels",
  "Subresource Filter", "Speech Recognition", "hyphen-data", "ZxcvbnData",
  "Typosquatting", "PKIMetadata", "CertificateRevocation", "MEIPreload",
  "PlatformNotifications", "DirectoryHashTable", "EdgeUpdate",
]);
const CLONE_SKIP_FILES = new Set(["lockfile", "DevToolsActivePort", "Single", "browser-error-collection"]);

function cloneSourceUserDirs(exe) {
  const fromEnv = process.env.BRIDGE_CLONE_FROM;
  if (fromEnv) return [fromEnv];
  const lf = process.env.LOCALAPPDATA || "";
  if (/msedge\.exe$/i.test(exe)) return [path.join(lf, "Microsoft", "Edge", "User Data")];
  if (/chrome\.exe$/i.test(exe)) return [path.join(lf, "Google", "Chrome", "User Data")];
  return [];
}

/** 找一个有 Default 子目录的日常浏览器 User Data 当克隆源。源正在运行也照克隆
 * （robocopy 对源只读，不会弄坏它；锁住的个别文件跳过即可，见 seedSharedProfile）。 */
function pickCloneSource(exe) {
  for (const dir of cloneSourceUserDirs(exe)) {
    try {
      if (!fs.existsSync(path.join(dir, "Default"))) continue;
      if (fs.existsSync(path.join(dir, "lockfile"))) {
        log("info", `clone source is running — cloning read-only from: ${dir}`);
      }
      return dir;
    } catch { /* 试下一个 */ }
  }
  return null;
}

/**
 * 桥 profile 为空且能找到克隆源时，把日常浏览器的 User Data 克隆进来。
 * 只在首装做一次（.seeded-from 记账）；克隆失败不拦启动——空 profile 照样能聊，
 * 只是回到「窗口里登录一次」的老路。
 */
async function seedSharedProfile(exe) {
  if (fs.existsSync(SEEDED_FILE)) return;
  const seededMarker = path.join(SHARED_PROFILE, "Default", "Preferences");
  if (fs.existsSync(seededMarker)) {
    try { fs.writeFileSync(SEEDED_FILE, `preexisting ${new Date().toISOString()}`); } catch { /* 尽力而为 */ }
    return;
  }
  const src = pickCloneSource(exe);
  if (!src) {
    log("info", "no clonable browser profile found — starting with empty bridge profile");
    try { fs.writeFileSync(SEEDED_FILE, `none ${new Date().toISOString()}`); } catch { /* 尽力而为 */ }
    return;
  }
  log("info", `seeding bridge profile by cloning ${src} -> ${SHARED_PROFILE}`);
  const code = await new Promise((resolve) => {
    // /XD 传裸目录名：任意层级同名目录都排除（Default\Cache 这类深层缓存也在内）。
    // 源在运行时个别被锁的文件拷不动（/R:0 /W:0 直接跳过）——成败不看
    // robocopy 退出码，看下面的关键文件是否落盘。
    const rc = spawn("robocopy.exe", [
      src, SHARED_PROFILE, "/E", "/XJ", "/NFL", "/NDL", "/NJH", "/R:0", "/W:0",
      "/XD", ...CLONE_SKIP_DIRS,
      "/XF", ...CLONE_SKIP_FILES,
    ], { stdio: "ignore", windowsHide: true });
    rc.on("error", () => resolve(-1));
    rc.on("exit", (c) => resolve(c));
  });
  // 成功判据：Local State 与 Default/Preferences 都在（浏览器 profile 的最小骨架）。
  const seededOk = fs.existsSync(path.join(SHARED_PROFILE, "Local State")) &&
    fs.existsSync(path.join(SHARED_PROFILE, "Default", "Preferences"));
  if (!seededOk) {
    log("warn", `profile clone incomplete (robocopy code ${code}) — bridge profile starts empty`);
    return; // 不记账：下次冷启动再试一次
  }
  if (code >= 8 || code < 0) log("warn", `profile clone skipped locked/unreadable files (robocopy ${code}); login may be needed`);
  // WAL/journal 可能含尚未落主库的数据，保留，不做手工删除。
  // 摘掉日常会话的现场：会话恢复文件 + 「继续上次的页面」设置。不清的话，桥窗口
  // 每次启动都会把日常浏览器上次的标签页原样开一遍。
  try { fs.rmSync(path.join(SHARED_PROFILE, "Default", "Sessions"), { recursive: true, force: true }); } catch { /* 忽略 */ }
  for (const junk of ["Current Session", "Current Tabs", "Last Session", "Last Tabs"]) {
    try { fs.rmSync(path.join(SHARED_PROFILE, "Default", junk), { force: true }); } catch { /* 忽略 */ }
  }
  try {
    const prefsPath = path.join(SHARED_PROFILE, "Default", "Preferences");
    const prefs = JSON.parse(fs.readFileSync(prefsPath, "utf8"));
    prefs.session = prefs.session || {};
    prefs.session.restore_on_startup = 5; // Chromium/Edge: 5 = 开新标签页
    fs.writeFileSync(prefsPath, JSON.stringify(prefs));
  } catch { /* Preferences 读不动就不管：无伤大雅 */ }
  try { fs.writeFileSync(SEEDED_FILE, `${src} ${new Date().toISOString()}`); } catch { /* 尽力而为 */ }
  log("info", `bridge profile seeded from ${src}`);
}


async function launchBrowser(exe, profileDir) {
  fs.mkdirSync(profileDir, { recursive: true });
  if (profileDir === SHARED_PROFILE) await seedSharedProfile(exe);
  const args = [
    `--remote-debugging-port=0`,
    `--user-data-dir=${profileDir}`,
    `--no-first-run`,
    `--no-default-browser-check`,
    `--disable-session-crashed-bubble`,
    `--no-startup-window`,
  ];
  if (process.env.BRIDGE_HEADLESS === "1") args.push("--headless=new");
  log("info", `launch ${exe} profile=${profileDir}`);
  // 关键：经 cmd start 中转拉起（分离）。Edge/Chrome 在 Windows 上会监视父进程，
  // 父进程（桥）被壳硬杀时浏览器会跟着自尽 —— 实测 2026-10-24，有头/无头都中。
  // cmd /c start 立即退出，浏览器父指针落在已死的 cmd 上，桥被杀不再连累它，
  // 「共享浏览器跨桥存活」才成立。代价：拿不到 ChildProcess 句柄，退出码看不见。
  // verbatim 拼接下 start 的第一个带引号 token 会被当窗口标题，所以先放 "" 占位。
  const cmdLine = ["start", '""', `"${exe}"`, ...args.map((a) => `"${a}"`)].join(" ");
  const cmd = spawn("cmd.exe", ["/d", "/s", "/c", cmdLine], {
    stdio: "ignore", detached: true, windowsHide: true, windowsVerbatimArguments: true,
  });
  cmd.unref();
  cmd.on("error", (err) => log("warn", `cmd start failed: ${err?.message || err}`));

  const deadline = Date.now() + 45_000;
  while (Date.now() < deadline) {
    const cdp = await attachToProfile(profileDir);
    if (cdp) return cdp;
    await sleep(150);
  }
  throw new Error(
    "浏览器没有开放调试端口。可能是企业策略限制，或专用配置已被不带调试参数的浏览器占用；" +
    "关闭专用浏览器后重试。日常浏览器无需关闭。",
  );
}

async function ensureBrowser() {
  if (browser?.alive) return browser;
  // 显式配置时只连接指定实例，失败不悄悄开另一窗口。自测组忽略此设置。
  if (SITE_GROUP === "shared" && process.env.BRIDGE_CDP_URL) {
    browser = await attachToBrowser(process.env.BRIDGE_CDP_URL);
    browserIsExternal = true;
    log("info", "CDP attached to configured browser");
    return browser;
  }
  browserIsExternal = false;
  browser = await attachToProfile(PROFILE_DIR);
  if (browser) return browser;
  browser = await withLaunchLock(async () => {
    const existing = await attachToProfile(PROFILE_DIR);
    if (existing) return existing;
    const exe = findBrowserExe();
    if (!exe) throw new Error("没有找到 Edge 或 Chrome，设 BRIDGE_BROWSER 指定浏览器路径。");
    // 日常浏览器已开放调试时直接复用；不扫描端口、不接管其他程序的实例。
    if (SITE_GROUP === "shared") {
      for (const dailyProfile of cloneSourceUserDirs(exe)) {
        const daily = await attachToProfile(dailyProfile);
        if (daily) {
          browserIsExternal = true;
          log("info", "CDP attached to daily browser");
          return daily;
        }
      }
    }
    return launchBrowser(exe, PROFILE_DIR);
  });
  return browser;
}

async function evaluate(conn, sessionId, expression) {
  const res = await conn.send(
    "Runtime.evaluate",
    { expression, awaitPromise: true, returnByValue: true, userGesture: true },
    sessionId,
  );
  if (res.exceptionDetails) {
    const detail = res.exceptionDetails.exception?.description || res.exceptionDetails.text;
    throw new Error(`page eval failed: ${detail}`);
  }
  return res.result?.value;
}

async function evalJson(pageRef, expression) {
  const raw = await evaluate(pageRef.cdp, pageRef.sessionId, expression);
  if (typeof raw !== "string") throw new Error("page snippet returned non-JSON");
  return JSON.parse(raw);
}

function cdpErrorLooksDead(err) {
  return /target closed|session.*not found|No session with|websocket closed|No target with|Cannot find context/i.test(String(err?.message || err));
}

// 只认当前会话保存的 tab 或精确对话 URL。不能按站点域名随手认领：
// 同站点的另一个 tab 可能属于另一条 Marionette 会话。
async function pageLooksMine(conn, targetId) {
  if (site.isMyPageJs) {
    try {
      const { sessionId } = await conn.send("Target.attachToTarget", { targetId, flatten: true });
      const raw = await evaluate(conn, sessionId, site.isMyPageJs);
      return typeof raw === "string" && JSON.parse(raw).isMine === true;
    } catch { return false; }
  }
  return true; // 真站点：由调用方先用 URL 筛过
}

async function ensurePage(context) {
  const { state } = context;
  if (context.page) {
    try {
      const current = await evalJson(context.page, `(() => JSON.stringify({url: location.href}))()`);
      // 用户在这个 tab 中手动切过对话时，也恢复本会话绑定的目标。
      if (state.conversationUrl && current.url !== state.conversationUrl) {
        await context.page.cdp.send("Page.navigate", { url: state.conversationUrl }, context.page.sessionId);
      }
      return context.page;
    } catch (err) {
      if (!cdpErrorLooksDead(err)) throw err;
      log("warn", `page dead (${String(err?.message || err).slice(0, 120)}) — recreating`);
      context.page = null;
    }
  }
  const conn = await ensureBrowser();
  // 复用还开着的站点 tab：桥重启 / 会话挂起唤醒都会重新走这里，
  // 每次都 createTarget 会把旧 tab 越堆越多。
  const { targetInfos } = await conn.send("Target.getTargets");
  const pages = targetInfos.filter((t) => t.type === "page");
  const wantUrl = state.conversationUrl || site.newChatUrl || site.home;
  let reuse = null;
  if (state.pageTargetId) {
    const saved = pages.find((t) => t.targetId === state.pageTargetId);
    // 记录还在且确实是我的页面（title/URL 锚点对得上）才认；对不上就当丢了。
    if (saved && (site.isMyPageJs ? await pageLooksMine(conn, saved.targetId) : saved.url.startsWith(site.home))) reuse = saved;
  }
  if (!reuse) {
    reuse = pages.find((t) => state.conversationUrl && t.url === state.conversationUrl);
    if (reuse && site.isMyPageJs && !(await pageLooksMine(conn, reuse.targetId))) reuse = null;
  }
  if (!reuse && context.preview && !browserIsExternal) {
    reuse = pages.find((t) => t.url.startsWith(site.home));
    if (reuse && site.isMyPageJs && !(await pageLooksMine(conn, reuse.targetId))) reuse = null;
  }
  if (!reuse && context.preview && site.isMyPageJs) {
    // 自测站最后兜底：任何 title 匹配本站的 tab（URL 可能还不是 home）。
    for (const t of pages) {
      if (await pageLooksMine(conn, t.targetId)) { reuse = t; break; }
    }
  }
  let targetId;
  let sessionId;
  if (reuse) {
    targetId = reuse.targetId;
    ({ sessionId } = await conn.send("Target.attachToTarget", { targetId, flatten: true }));
    await conn.send("Page.enable", {}, sessionId);
    if (reuse.url !== wantUrl) {
      await conn.send("Page.navigate", { url: wantUrl }, sessionId);
    }
  } else {
    const created = await conn.send("Target.createTarget", { url: "about:blank" });
    targetId = created.targetId;
    ({ sessionId } = await conn.send("Target.attachToTarget", { targetId, flatten: true }));
    await conn.send("Page.enable", {}, sessionId);
    await conn.send("Page.navigate", { url: wantUrl }, sessionId);
  }
  context.page = { cdp: conn, sessionId, targetId };
  if (state.pageTargetId !== targetId) {
    state.pageTargetId = targetId;
    saveState(context);
  }
  return context.page;
}

// 等 composer 出现（首页加载 / 登录跳转都需要时间）。
async function waitComposer(pageRef, timeoutMs = 45_000) {
  const t0 = Date.now();
  while (Date.now() - t0 < timeoutMs) {
    const ok = await evalJson(pageRef, `(() => JSON.stringify({ ok: !!(${site.composerOk}) }))()`);
    if (ok.ok) return true;
    await sleep(500);
  }
  return false;
}

// ── 一回合的驱动：登录检查 → 发送 → 确认 → 流式抄回 ─────────────────────
let activeTurn = null; // { agentSessionId, cancelled }

async function runTurn(agentSessionId, text, blocks, filePaths, emitChunk) {
  const context = sessions.get(agentSessionId);
  const cancelledRef = { value: false };
  activeTurn = { agentSessionId, cancelledRef };

  const note = async (msg) => emitChunk(`ℹ️ ${msg}`);
  let attachments;
  try {
    const pageRef = await ensurePage(context);

    // 自测站点先搭页面。
    if (site.prepareJs) await evalJson(pageRef, site.prepareJs);

    // 等待目标对话加载，不能在输入框尚未挂载时跳回首页丢掉续聊目标。
    if (!(await waitComposer(pageRef))) {
      await note("网页没有出现输入框（可能在加载或弹了人机验证）——到打开的浏览器窗口里看一眼，处理后重发这条消息。");
      return "end_turn";
    }

    // 登录墙：把窗口交给用户，重发即可。
    const login = await evalJson(pageRef, site.loginCheckJs());
    if (!login.ok) {
      await note(
        `这个站点还没有登录。已经打开浏览器窗口（独立用户目录，与日常浏览器互不影响）——` +
        `请在窗口里登录 ${site.label}，登录成功后把这条消息重新发一遍。只需登录一次。`,
      );
      return "end_turn";
    }

    const pending = await evalJson(pageRef, `JSON.stringify(window.__marionettePendingUpload || [])`);
    if (pending.length) {
      throw new Error("上次的附件尚未确认发送。请在浏览器移除输入框中的附件并刷新该标签页，再重发。");
    }
    attachments = prepareAttachments(blocks, filePaths);
    // File paths in the Composer become readable filenames on the website.
    for (const file of filePaths) text = text.split(file).join(path.basename(file));
    const message = text.trim() || "请查看附件。";
    if (cancelledRef.value) return "cancelled";
    // 填入文本（ paste 事件 / native setter，都能带换行不误触发送）。
    const put = await evalJson(pageRef, site.sendJs(message));
    if (!put.ok) {
      await note("输入框注入失败（页面结构可能变了）——详见桥日志 bridge-" + SITE_KEY + ".log。");
      return "end_turn";
    }

    if (attachments.files.length) {
      if (!(await uploadAttachments(pageRef, SITE_KEY, attachments.files, evalJson, () => cancelledRef.value))) {
        await note("已取消附件发送；附件可能还留在网页输入框中，请移除并刷新该标签页后继续。");
        return "cancelled";
      }
    }
    if (cancelledRef.value) return "cancelled";

    // 提交 + 确认真发出去了：composer 清空 / 出现流式 / 节点增长 / URL 变化。
    const pre = await evalJson(pageRef, site.preCountJs());
    // 基线：提交前页面上已有的节点全文记作「已发送」——网页会话流里是全部
    // 历史回复，不设基线的话每轮都会把旧回复整段重抄一遍。
    const baseline = await evalJson(pageRef, site.extractJs);
    const thoughtBaseline = site.thoughtExtractJs
      ? await evalJson(pageRef, site.thoughtExtractJs) : [];
    const preUrl = pre.url || "";
    let submitted = await evalJson(pageRef, site.submitJs(true));
    const confirmStart = Date.now();
    const confirmed = async () => {
      const comp = await evalJson(pageRef, site.composerLenJs);
      if (comp.len === 0) return true;
      const now = await evalJson(pageRef, site.pollJs);
      rememberConversation(context, now.url);
      const grew = now.lens.some((n, i) => n > (pre.lens[i] ?? 0)) || now.lens.length > pre.lens.length;
      if (grew || now.streaming || (now.url && now.url !== preUrl)) return true;
      return false;
    };
    let ok = false;
    while (Date.now() - confirmStart < 8000) {
      if (cancelledRef.value) break;
      if (await confirmed()) { ok = true; break; }
      await sleep(400);
    }
    if (!ok && !cancelledRef.value) {
      // 重试一次：换提交方式（按钮 → 回车）。文本还在 composer 里，重填无害。
      submitted = await evalJson(pageRef, site.submitJs(!submitted || submitted.method !== "button"));
      const retryStart = Date.now();
      while (Date.now() - retryStart < 8000) {
        if (await confirmed()) { ok = true; break; }
        await sleep(400);
      }
    }
    if (!ok) {
      await note("网页没有接受这条消息（多次重试无效）。可能弹了人机验证或限流——到浏览器窗口里看一眼，处理后重发。");
      return "end_turn";
    }
    if (attachments.files.length) {
      try { await clearUploadedAttachments(pageRef, evalJson); }
      catch (err) { log("warn", `clear upload marker failed: ${err?.message || err}`); }
    }

    // 流式抄回：按 assistant 节点增量发 chunk（messageId 按节点序号，壳会合并）。
    // 增量只从「提交前基线」之上算：历史节点全文在基线里，一个字都不会重发。
    const sent = baseline.map((t) => (t || "").length); // nodeIndex → 已发出的长度（初值 = 基线全文长）
    let lastMarkdown = "";
    const thoughtSent = thoughtBaseline.map((t) => (t || "").length);
    let lastLens = [...baseline, ...thoughtBaseline].map((t) => (t || "").length);
    let stable = 0;
    const t0 = Date.now();
    let richEnabled = SITE_KEY === "chatgpt";
    let richSent = false;
    let lastRichSignature = "";
    let richWarningLogged = false;
    while (true) {
      if (cancelledRef.value) break;
      await sleep(POLL_MS);
      let st;
      try {
        st = await evalJson(pageRef, site.pollJs);
      } catch (err) {
        if (cdpErrorLooksDead(err)) throw err;
        await note("读取回复时页面出错：" + String(err?.message || err).slice(0, 160));
        break;
      }
      // 先读思考再读正文；使用 textContent 序列化，网页折叠思考区也能读取。
      const thoughts = site.thoughtExtractJs
        ? await evalJson(pageRef, site.thoughtExtractJs) : [];
      const texts = await evalJson(pageRef, site.extractJs);
      rememberConversation(context, st.url);
      for (let i = 0; i < thoughts.length; i++) {
        const have = thoughtSent[i] ?? 0;
        const full = thoughts[i] || "";
        if (full.length > have) {
          emitChunk(full.slice(have), `t${i}`, "agent_thought_chunk");
          thoughtSent[i] = full.length;
        }
      }
      let richChanged = false;
      if (richEnabled) {
        const indexes = texts.flatMap((text, i) => text?.trim() &&
          (i >= baseline.length || !baseline[i]?.trim()) ? [i] : []);
        if (indexes.length) {
          try {
            const webReply = await captureChatgptReply(pageRef, indexes, evalJson);
            if (webReply && !cancelledRef.value) {
              const richText = indexes.map(i => texts[i]).join("\n\n");
              webReply.textLength = richText.length;
              const signature = createHash("sha256").update(webReply.html).update(webReply.cssPath)
                .update(indexes.map(i => texts[i]).join("\n\n")).digest("hex");
              richChanged = signature !== lastRichSignature;
              if (richChanged) {
                send({ jsonrpc: "2.0", method: "session/update", params: {
                  sessionId: agentSessionId,
                  update: {
                    sessionUpdate: "agent_message_chunk",
                    messageId: `${agentSessionId}-rich-${baseline.length}`,
                    content: { type: "text", text: richText },
                    _meta: { "marionette.webReply": webReply },
                  },
                } });
                lastRichSignature = signature;
                richSent = true;
              }
            }
          } catch (err) {
            if (cdpErrorLooksDead(err)) throw err;
            if (!richWarningLogged) log("warn", `HTML capture unavailable: ${err?.message || err}`);
            richWarningLogged = true;
            // Once HTML has arrived, keep its card and retry next tick. Before
            // that, falling back to the existing text stream remains usable.
            if (!richSent) richEnabled = false;
          }
        }
      }
      let markdownChanged = false;
      if (site.textSnapshot) {
        // Formatting can rewrite an earlier prefix (bold, lists, tables, code).
        // Send this turn as a snapshot; slicing by the previous length corrupts it.
        const markdown = texts.filter((_, i) => i >= baseline.length || !baseline[i]?.trim()).join("\n\n");
        markdownChanged = markdown !== lastMarkdown;
        if (markdownChanged && !cancelledRef.value) {
          send({ jsonrpc: "2.0", method: "session/update", params: {
            sessionId: agentSessionId,
            update: {
              sessionUpdate: "agent_message_chunk",
              messageId: `${agentSessionId}-markdown-${baseline.length}`,
              content: { type: "text", text: markdown },
              _meta: { "marionette.textSnapshot": true },
            },
          } });
          lastMarkdown = markdown;
        }
      } else if (!richEnabled) {
        for (let i = 0; i < texts.length; i++) {
          const have = sent[i] ?? 0;
          const full = texts[i] || "";
          if (full.length > have) {
            emitChunk(full.slice(have), `n${i}`);
            sent[i] = full.length;
          }
        }
      }
      const lensNow = [...texts, ...thoughts].map((t) => (t || "").length);
      // 只看本轮新增内容，历史回答不能让正在思考的新回合提前结束。
      const sawAnswer = texts.some((t, i) => (t || "").length > (baseline[i] || "").length);
      const sawThought = thoughts.some((t, i) => (t || "").length > (thoughtBaseline[i] || "").length);
      const sawAnyNode = sawAnswer || sawThought;
      const lensSame = lensNow.length === lastLens.length &&
        lensNow.every((n, i) => n === lastLens[i]);
      // 思考中的停顿不算完成；须等本轮正式回答出现再判断收尾。
      if (lensSame && !richChanged && !markdownChanged && !st.streaming && sawAnswer) stable++; else stable = 0;
      if (!sawAnyNode && !st.streaming && Date.now() - t0 > NO_REPLY_GRACE_MS) {
        await note(
          "等了半分钟页面上没有出现任何回复——如果浏览器窗口里其实已经有回复，" +
          "说明桥的回复选择器过期了（页面改版），需要更新 bridge.mjs 里该站点的选择器。",
        );
        break;
      }
      lastLens = lensNow;
      if (stable >= STABLE_NEEDED) break;
      if (Date.now() - t0 > MAX_WAIT_MS) {
        await note("等回复超时了——把已收到的部分先给你；网页若还在生成，之后的内容收不到。");
        break;
      }
    }
    if (cancelledRef.value) {
      try { await evalJson(pageRef, site.stopJs); } catch { /* 尽力而为 */ }
      return "cancelled";
    }

    // 收尾：URL 若进入具体对话，记下来（重启后接着聊）。
    try {
      const now = await evalJson(pageRef, site.pollJs);
      rememberConversation(context, now.url);
    } catch { /* 尽力而为 */ }
    return "end_turn";
  } catch (err) {
    log("error", `turn failed: ${err?.stack || err}`);
    if (cdpErrorLooksDead(err)) {
      context.page = null; // 下回合重建本会话的 tab
      await note("浏览器页面失联了（可能被手动关掉）。重发这条消息，我会重新打开并接着上次的对话聊。");
      return "end_turn";
    }
    await note("出错：" + String(err?.message || err).slice(0, 200));
    return "end_turn";
  } finally {
    attachments?.cleanup();
    if (activeTurn && activeTurn.agentSessionId === agentSessionId) activeTurn = null;
  }
}

// ── ACP server ─────────────────────────────────────────────────────────
const sessions = new Map();
let turnChain = Promise.resolve(); // 每站串行：同一网页对话不能并发两回合

function send(obj) {
  process.stdout.write(JSON.stringify(obj) + "\n");
}
function emitChunk(agentSessionId, text, nodeTag = "main", updateKind = "agent_message_chunk") {
  send({
    jsonrpc: "2.0",
    method: "session/update",
    params: {
      sessionId: agentSessionId,
      update: {
        sessionUpdate: updateKind,
        content: { type: "text", text },
        messageId: `${agentSessionId}-${nodeTag}`,
      },
    },
  });
}

/** 记忆卡 / 历史注入剥除（口径见文件头注释）。 */
export function extractUserText(blocks) {
  let text = "";
  for (const b of blocks || []) {
    if (b && b.type === "text" && typeof b.text === "string") {
      text += (text ? "\n" : "") + b.text;
    }
  }
  const t = text.trimStart();
  if (t.startsWith("<marionette-memory>")) {
    const end = t.indexOf("</marionette-memory>");
    if (end >= 0) text = t.slice(end + "</marionette-memory>".length);
  }
  const hStart = text.indexOf("[Marionette — prior conversation in this dialog]");
  if (hStart >= 0) {
    const marker = "[End of prior conversation. The user's new message follows.]";
    const hEnd = text.indexOf(marker, hStart);
    if (hEnd >= 0) text = text.slice(hEnd + marker.length);
  }
  return text.trim();
}

function errResponse(id, code, message) {
  send({ jsonrpc: "2.0", id, error: { code, message } });
}

async function handleRequest(id, method, params) {
  switch (method) {
    case "initialize":
      send({
        jsonrpc: "2.0", id, result: {
          protocolVersion: 1,
          agentCapabilities: {
            promptCapabilities: { image: true },
            _meta: { "marionette.localFileUpload": SITE_KEY === "chatgpt" || SITE_KEY === "deepseek" },
          },
          authMethods: [],
          agentInfo: { name: `web-${SITE_KEY}`, title: site.label, version: VERSION },
        },
      });
      return;
    case "session/new": {
      const sid = `web-${SITE_KEY}-${randomUUID()}`;
      // Marionette 每进程一条 ACP 会话；额外 session/new 仍须彼此隔离。
      const key = sessions.size === 0 && process.env.MARIONETTE_SESSION_ID
        ? process.env.MARIONETTE_SESSION_ID : sid;
      sessions.set(sid, conversationContext(key));
      send({ jsonrpc: "2.0", id, result: { sessionId: sid } });
      log("info", `session/new -> ${sid}`);
      return;
    }
    case "session/prompt": {
      const sid = params?.sessionId;
      if (!sid || !sessions.has(sid)) return errResponse(id, -32000, "unknown sessionId");
      const text = extractUserText(params?.prompt);
      const filePaths = params?._meta?.["marionette.attachments"] ?? [];
      if (!Array.isArray(filePaths) || !filePaths.every(p => typeof p === "string")) {
        return errResponse(id, -32602, "invalid attachment paths");
      }
      const hasImage = params?.prompt?.some(block => block?.type === "image");
      if (!text && !hasImage && !filePaths.length) {
        emitChunk(sid, "（这条消息没有可发送的文字或附件。）");
        send({ jsonrpc: "2.0", id, result: { stopReason: "end_turn" } });
        return;
      }
      turnChain = turnChain
        .then(() => runTurn(sid, text, params?.prompt, filePaths, (chunk, tag, kind) => emitChunk(sid, chunk, tag, kind)))
        .then((stopReason) => send({ jsonrpc: "2.0", id, result: { stopReason } }))
        .catch((err) => {
          log("error", `turn chain error: ${err?.stack || err}`);
          errResponse(id, -32000, String(err?.message || err).slice(0, 300));
        });
      return;
    }
    case "session/cancel": {
      const sid = params?.sessionId;
      if (activeTurn && activeTurn.agentSessionId === sid) activeTurn.cancelledRef.value = true;
      send({ jsonrpc: "2.0", id, result: {} });
      return;
    }
    case "authenticate":
      send({ jsonrpc: "2.0", id, result: {} });
      return;
    case "session/set_mode":
    case "session/set_model":
    case "session/set_config_option":
      send({ jsonrpc: "2.0", id, result: {} });
      return;
    default:
      errResponse(id, -32601, `method not supported by web bridge: ${method}`);
  }
}

// 手动初始化/恢复窗口，不经过 prompt，也不会向站点发送测试消息。
if (process.argv.includes("--open")) {
  try {
    await ensurePage(conversationContext("preview", true));
    console.log(`已打开 ${site.label} 标签页；浏览器会保留，供聊天桥复用。`);
    shutdown("open-only");
  } catch (err) {
    log("error", `open failed: ${err?.stack || err}`);
    browser?.abandon();
    process.exit(1);
  }
}

const rl = readline.createInterface({ input: process.stdin, terminal: false });
rl.on("line", (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { return; }
  if (msg.id != null && msg.method) {
    handleRequest(msg.id, msg.method, msg.params).catch((err) => {
      log("error", `handle ${msg.method} failed: ${err?.stack || err}`);
      errResponse(msg.id, -32603, String(err?.message || err).slice(0, 300));
    });
    return;
  }
  // 通知（无 id）：壳的 session/cancel 走 notify，不带 jsonrpc id。
  if (msg.method === "session/cancel") {
    const sid = msg.params?.sessionId;
    if (activeTurn && activeTurn.agentSessionId === sid) activeTurn.cancelledRef.value = true;
  }
  // 其余通知与我们不发起 client 请求——忽略。
});

// 桥退出只断开自己的连接。浏览器和标签页由用户保留/关闭。
function shutdown(reason) {
  log("info", `shutdown (${reason}); browser left running`);
  browser?.abandon();
  process.exit(0);
}
process.stdin.on("end", () => shutdown("stdin-eof"));
process.on("SIGINT", () => shutdown("sigint"));
process.on("SIGTERM", () => shutdown("sigterm"));
process.on("uncaughtException", (err) => log("error", `uncaught: ${err?.stack || err}`));
process.on("unhandledRejection", (err) => log("error", `unhandled: ${err?.stack || err}`));

log("info", `bridge started site=${SITE_KEY} pid=${process.pid} state=${STATE_ROOT}`);
