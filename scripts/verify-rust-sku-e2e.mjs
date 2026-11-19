// Rust SKU end-to-end check: drives a REAL running rust-SKU instance over CDP
// (needs the app up with --remote-debugging-port + a fixture vendor on 45777 —
// see docs/plans/plan-engine-rust.md T13 for the launch recipe). Original:
// Tauri commands the UI uses (bridge_add_custom_provider / engine_enable /
// engine_status / engine_quota), then hit the running in-process gateway over
// HTTP for C2 catalog + C5 chat. Run from the repo root; Node 24+ provides the
// browser-compatible WebSocket used below.
if (typeof WebSocket !== "function") throw new Error("Node 24+ is required for WebSocket");

const CDP = (process.env.MARIONETTE_CDP_URL ?? "http://127.0.0.1:9223").replace(/\/$/, "");
const results = [];
const ok = (name, pass, detail = "") => {
  results.push({ name, pass });
  console.log(`${pass ? "PASS" : "FAIL"}  ${name}${detail ? "  — " + detail : ""}`);
};

const targets = await (await fetch(`${CDP}/json/list`)).json();
const page = targets.find((t) => t.type === "page" && /Marionette/i.test(t.title ?? ""))
  ?? targets.find((t) => t.type === "page");
if (!page) throw new Error("no page target: " + JSON.stringify(targets.map((t) => t.title)));
console.log("target:", page.title);

const ws = new WebSocket(page.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  ws.addEventListener("open", resolve, { once: true });
  ws.addEventListener("error", reject, { once: true });
});
let seq = 0;
const pending = new Map();
ws.addEventListener("message", (event) => {
  const msg = JSON.parse(String(event.data));
  if (msg.id && pending.has(msg.id)) {
    pending.get(msg.id)(msg?.result?.result?.value ?? null);
    pending.delete(msg.id);
  }
});
const evaluate = (expr) => new Promise((resolve) => {
  const id = ++seq;
  pending.set(id, resolve);
  ws.send(JSON.stringify({
    id, method: "Runtime.evaluate",
    params: { expression: expr, awaitPromise: true, returnByValue: true },
  }));
});
const invoke = (cmd, args = {}) => evaluate(
  `window.__TAURI_INTERNALS__.invoke('${cmd}', ${JSON.stringify(args)}).then(v => JSON.stringify(v)).catch(e => 'ERR:' + (e && e.toString ? e.toString() : String(e)))`,
);

// 1. initial status (lazy: placed, not started)
const s0 = await invoke("engine_status");
console.log("status(before):", s0);
ok("engine_status reachable", typeof s0 === "string");

// 2. register the fixture source through the real registration chain
const reg = await invoke("bridge_add_custom_provider", {
  label: "t13fixture",
  key: "fixture-key-t13",
  config: { endpoint: "http://127.0.0.1:45777/v1", protocol: "chat", models: [] },
});
console.log("register:", reg);
ok("bridge_add_custom_provider", !String(reg).startsWith("ERR:"), String(reg).slice(0, 120));

// 3. explicit enable — the in-process engine thread should come up
const en = await invoke("engine_enable");
console.log("enable:", en);
ok("engine_enable", !String(en).startsWith("ERR:"));

// 4. status must now be Running with the rust version
let status = null;
for (let i = 0; i < 20; i++) {
  const s = await invoke("engine_status");
  try { status = JSON.parse(s); } catch { status = null; }
  if (status?.state === "running") break;
  await new Promise((r) => setTimeout(r, 500));
}
console.log("status(after):", JSON.stringify(status));
ok("engine Running", status?.state === "running", `state=${status?.state}`);
ok("engine version is rust-*", String(status?.version ?? "").startsWith("rust-"), String(status?.version));

// 4.5 model library scan (the panel's force refresh) — this is the path that
// registers custom sources into the engine (sync_discovered_sources(true)).
const scan = await invoke("model_library_scan");
console.log("scan sources:", String(scan).slice(0, 300));
ok("model_library_scan", !String(scan).startsWith("ERR:"));
await new Promise((r) => setTimeout(r, 4000));

// 5. C2 through the real gateway: /v1/models row fields
if (status?.state === "running") {
  const port = status.port;
  const models = await (await fetch(`http://127.0.0.1:${port}/v1/models`)).json();
  const row = (models.data ?? []).find((r) => String(r.id ?? "").endsWith("/t13-model"));
  const fullId = row?.id;
  ok("C2 catalog row present", !!row, JSON.stringify((models.data ?? []).map((r) => r.id)));
  ok("C2 context_window relayed", row?.context_window === 1234567, String(row?.context_window));
  ok("C2 max_output relayed", row?.max_output_tokens === 65536, String(row?.max_output_tokens));
  const input = row?.modalities?.input ?? [];
  ok("C2 image modality", input.includes("image"), JSON.stringify(input));

  // 6. C5 through the real gateway: non-stream chat
  const chat = await fetch(`http://127.0.0.1:${port}/v1/chat/completions`, {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ model: fullId, stream: false, messages: [{ role: "user", content: "hi" }] }),
  });
  const chatBody = await chat.text();
  ok("C5 chat 200", chat.status === 200, `${chat.status} ${chatBody.slice(0, 120)}`);
  ok("C5 chat content", chatBody.includes("t13-ok"));

  // 7. C5 streaming through the real gateway
  const sse = await fetch(`http://127.0.0.1:${port}/v1/chat/completions`, {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ model: fullId, stream: true, messages: [{ role: "user", content: "hi" }] }),
  });
  const sseBody = await sse.text();
  ok("C5 SSE relay", sse.status === 200 && sseBody.includes("data:") && sseBody.includes("t13-ok"), sseBody.slice(0, 80));

  // 8. C3 shape through the real gateway
  const quotas = await (await fetch(`http://127.0.0.1:${port}/v1/magpie/quotas`)).text();
  let quotasOk = false;
  try { quotasOk = Array.isArray(JSON.parse(quotas).data); } catch {}
  ok("C3 quotas shape", quotasOk, quotas.slice(0, 80));
}

// 9. engine_quota command (frontend panel path)
const q = await invoke("engine_quota", { force: false });
console.log("engine_quota:", String(q).slice(0, 160));
ok("engine_quota command", !String(q).startsWith("ERR:"));

ws.close();
const failed = results.filter((r) => !r.pass);
console.log(`\n=== T13 automated: ${results.length - failed.length}/${results.length} passed ===`);
process.exit(failed.length ? 1 : 0);
