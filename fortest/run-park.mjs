/**
 * Park-control browser check.
 *
 * Builds fortest/park-harness.tsx with esbuild (IIFE), inlines the real app.css,
 * drives the page in headless Edge over CDP, and asserts the row's ⏸ control.
 *
 * Run: node fortest/run-park.mjs
 */
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const repo = path.resolve(here, "..");
const require = createRequire(path.join(repo, "package.json") + "/");
const esbuild = require("esbuild");

const EDGE = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
const PORT = 9377;
const scratch = fs.mkdtempSync(path.join(os.tmpdir(), "marionette-park-"));
const profile = path.join(scratch, "edgeprofile");

const checks = [];
function check(name, ok, detail = "") {
  checks.push({ name, ok: Boolean(ok), detail });
  console.log(`${ok ? "  ok " : "  FAIL"} ${name}${detail ? ` — ${detail}` : ""}`);
}

const bundle = await esbuild.build({
  entryPoints: [path.join(here, "park-harness.tsx")],
  bundle: true,
  format: "iife",
  platform: "browser",
  jsx: "automatic",
  loader: { ".css": "empty" },
  nodePaths: [path.join(repo, "node_modules")],
  define: { "process.env.NODE_ENV": '"development"' },
  write: false,
});
const js = bundle.outputFiles[0].text;
const css = fs.readFileSync(path.join(repo, "src", "styles", "app.css"), "utf8");

const html = `<!doctype html><html><head><meta charset="utf-8"><style>${css}</style></head>
<body><div id="root"></div><script>${js}</script></body></html>`;
const page = path.join(scratch, "park.html");
fs.writeFileSync(page, html, "utf8");

const edge = spawn(EDGE, [
  "--headless=new",
  "--disable-gpu",
  "--no-first-run",
  `--user-data-dir=${profile}`,
  `--remote-debugging-port=${PORT}`,
  "--window-size=900,700",
  "about:blank",
], { detached: true, stdio: "ignore" });
edge.unref();

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function targets() {
  const res = await fetch(`http://127.0.0.1:${PORT}/json/list`);
  return res.json();
}

let ws;
let id = 0;
const pending = new Map();
function send(method, params = {}, sessionId) {
  id += 1;
  const msg = { id, method, params };
  if (sessionId) msg.sessionId = sessionId;
  ws.send(JSON.stringify(msg));
  return new Promise((resolve, reject) => pending.set(id, { resolve, reject }));
}

async function evaluate(expression) {
  const { result, exceptionDetails } = await send("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (exceptionDetails) throw new Error(exceptionDetails.text + " " + JSON.stringify(exceptionDetails.exception ?? {}));
  return result.value;
}

try {
  let list = [];
  for (let i = 0; i < 60; i += 1) {
    try {
      list = await targets();
      if (list.some((t) => t.type === "page")) break;
    } catch {
      // not up yet
    }
    await sleep(250);
  }
  const pageTarget = list.find((t) => t.type === "page");
  if (!pageTarget) throw new Error("headless Edge never exposed a page target");

  ws = new WebSocket(pageTarget.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    ws.addEventListener("open", resolve, { once: true });
    ws.addEventListener("error", reject, { once: true });
  });
  ws.addEventListener("message", (event) => {
    const msg = JSON.parse(event.data);
    if (msg.id && pending.has(msg.id)) {
      const { resolve, reject } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) reject(new Error(msg.error.message));
      else resolve(msg.result);
    }
  });

  await send("Page.enable");
  await send("Runtime.enable");
  await send("Page.navigate", { url: `file:///${page.replace(/\\/g, "/")}` });
  for (let i = 0; i < 40; i += 1) {
    if (await evaluate("window.__ready === true")) break;
    await sleep(150);
  }
  check("harness mounted", await evaluate("window.__ready === true"));

  const snapshot = await evaluate(`
    (() => {
      const rows = [...document.querySelectorAll(".chat-row")];
      return rows.map((row) => {
        const label = row.querySelector("strong")?.textContent ?? "";
        const actions = [...row.querySelectorAll(".chat-row__actions button")];
        const park = actions[actions.length - 2];
        const rect = park.getBoundingClientRect();
        const rowRect = row.getBoundingClientRect();
        return {
          label,
          buttons: actions.length,
          paused: park.getAttribute("aria-label"),
          title: park.getAttribute("title"),
          disabled: park.disabled,
          svgPath: park.querySelector("svg")?.innerHTML.includes("line") ?? false,
          x: rect.left + rect.width / 2,
          y: rect.top + rect.height / 2,
          rowX: rowRect.left + rowRect.width / 2,
          rowY: rowRect.top + rowRect.height / 2,
        };
      });
    })()
  `);

  const byLabel = (needle) => snapshot.find((row) => row.label === needle);
  check("all four chat rows are drawn", snapshot.length === 4, snapshot.map((r) => r.label).join(" | "));
  check(
    "every row draws the park control in front of delete",
    snapshot.every((row) => row.buttons === 4 && row.title),
    snapshot.map((row) => `${row.label}:${row.buttons}`).join(" "),
  );

  const warm = byLabel("warm and idle");
  const working = byLabel("working right now");
  const failed = byLabel("last turn failed");
  const parked = byLabel("already parked");
  check("a warm idle dialog offers to park", warm && !warm.disabled && /^挂起 /.test(warm.title), warm?.title);
  check("a working dialog refuses and explains why", working && working.disabled && /正在跑/.test(working.title), working?.title);
  check("a failed turn can still be parked", failed && !failed.disabled, failed?.title);
  check("an already parked dialog refuses without pretending", parked && parked.disabled && /已挂起/.test(parked.title), parked?.title);

  // Hover the row, then press the control with real mouse input.
  async function press(row) {
    await send("Input.dispatchMouseEvent", { type: "mouseMoved", x: row.rowX, y: row.rowY });
    await sleep(120);
    const opacity = await evaluate(`
      (() => {
        const rows = [...document.querySelectorAll(".chat-row")];
        const row = rows.find((r) => (r.querySelector("strong")?.textContent ?? "") === ${JSON.stringify(row.label)});
        return getComputedStyle(row.querySelector(".chat-row__actions")).opacity;
      })()
    `);
    for (const type of ["mousePressed", "mouseReleased"]) {
      await send("Input.dispatchMouseEvent", { type, x: row.x, y: row.y, button: "left", clickCount: 1 });
    }
    await sleep(120);
    return opacity;
  }

  const warmOpacity = await press(warm);
  const warmLog = await evaluate("window.__log");
  check("hovering the row reveals the action cluster", warmOpacity === "1", `opacity=${warmOpacity}`);
  check(
    "pressing the park control parks that dialog and does not select the row",
    warmLog.length === 1 && warmLog[0].kind === "suspend" && warmLog[0].id === "s-waiting",
    JSON.stringify(warmLog),
  );

  await evaluate("window.__log = []");
  await press(parked);
  const parkedLog = await evaluate("window.__log");
  check(
    "a disabled park control never parks anything",
    !parkedLog.some((entry) => entry.kind === "suspend"),
    JSON.stringify(parkedLog),
  );

  await evaluate("window.__log = []");
  await press(working);
  const workingLog = await evaluate("window.__log");
  check(
    "a working dialog cannot be parked mid-turn",
    !workingLog.some((entry) => entry.kind === "suspend"),
    JSON.stringify(workingLog),
  );

  const errors = await evaluate("window.__errors ?? []");
  check("page reported no errors", errors.length === 0, JSON.stringify(errors));

  const failedChecks = checks.filter((c) => !c.ok);
  console.log(`\n${checks.length - failedChecks.length}/${checks.length} checks passed.`);
  fs.writeFileSync(path.join(scratch, "result.json"), JSON.stringify({ checks, snapshot, warmLog, parkedLog, workingLog }, null, 2));
  process.exitCode = failedChecks.length === 0 ? 0 : 1;
} catch (error) {
  console.error("harness error:", error);
  process.exitCode = 1;
} finally {
  try {
    const res = await fetch(`http://127.0.0.1:${PORT}/json/version`);
    const { webSocketDebuggerUrl } = await res.json();
    const browserWs = new WebSocket(webSocketDebuggerUrl);
    await new Promise((resolve) => browserWs.addEventListener("open", resolve, { once: true }));
    browserWs.send(JSON.stringify({ id: 1, method: "Browser.close" }));
    await sleep(500);
    browserWs.close();
  } catch {
    // Edge already gone
  }
  try {
    ws?.close();
  } catch {
    // ignore
  }
  console.log(`scratch: ${scratch}`);
}
