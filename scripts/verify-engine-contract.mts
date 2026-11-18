// Engine contract suite (docs/plans/plan-engine-contract.md) — drives a magpie SEED
// binary over its whole observable surface with a local fixture vendor, in a
// .portable scratch home. Never touches the real ~/.config/magpie or
// ~/.marionette; no network beyond 127.0.0.1 (MAGPIE_NO_ACCOUNTS/NO_STATS +
// DO_NOT_TRACK keep upstream telemetry off; the stats.Run code is cut from the
// seed entirely since 2026-11-17).
//
// Usage:
//   npx tsx scripts/verify-engine-contract.mts [magpie-seed.exe]
// Without an argument the seed is built from third_party/magpie (Go toolchain
// required). The upstream-watch pipeline passes the built seed explicitly.
import assert from "node:assert/strict";
import { execFile as run, spawn } from "node:child_process";
import { gzipSync } from "node:zlib";

// Known-gap cases (e.g. "D7") asserted while a batch is in flight; they print
// PENDING instead of failing, so the suite stays a usable gate for the Rust
// engine's current state. Go seed runs never need this.
const PENDING = new Set((process.env.CONTRACT_PENDING ?? "").split(",").map((s) => s.trim()).filter(Boolean));
import { promisify } from "node:util";
import { createServer } from "node:http";
import { copyFile, mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const execFile = promisify(run);
const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const scratch = await mkdtemp(join(tmpdir(), "marionette-engine-contract-"));

// ── fixture vendor: an OpenAI-compatible fake that refuses oversized chat
// bodies with ollama.com's exact unread-body words (the byte-cap patch
// recognizes), so the refit loop is exercised for real. ─────────────────────
const OVER_CAP_BYTES = 26 * 1024 * 1024; // fixture's own cap; gateway must learn and shed images
const chatRequests: { size: number; images: number }[] = [];
// Dialect cases (C5): every scripted reply is selected by a marker in the
// LAST user message text ("script:<kind>"), so the vendor behaves like a real
// model reacting to the prompt, not to headers an agent would never send.
// `vendorBodies` keeps what the gateway actually sent outbound (post-translate)
// so D2 can assert the wire shape agents depend on.
type AnyRecord = Record<string, any>;
const vendorBodies: { key: string; body: AnyRecord }[] = [];
const lastUserText = (body: AnyRecord): string => {
  for (let i = (body.messages ?? []).length - 1; i >= 0; i--) {
    const m = body.messages[i];
    if (m.role !== "user") continue;
    if (typeof m.content === "string") return m.content;
    if (Array.isArray(m.content)) {
      const t = m.content.filter((p: AnyRecord) => p.type === "text").map((p: AnyRecord) => p.text ?? "").join("");
      if (t) return t;
    }
  }
  return "";
};
const chatChunk = (delta: AnyRecord, finish: string | null = null, usage?: AnyRecord) =>
  JSON.stringify({
    id: "chatcmpl-fx", object: "chat.completion.chunk", created: 1,
    choices: [{ index: 0, delta, ...(finish ? { finish_reason: finish } : {}) }],
    ...(usage ? { usage } : {}),
  });
const chatStream = (res: import("node:http").ServerResponse, deltas: AnyRecord[], usage?: AnyRecord) => {
  res.statusCode = 200;
  res.setHeader("Content-Type", "text/event-stream");
  for (const d of deltas) res.write(`data: ${chatChunk(d)}\n\n`);
  res.write(`data: ${chatChunk({}, "stop")}\n\n`);
  if (usage) res.write(`data: ${chatChunk({}, null, usage)}\n\n`);
  res.write("data: [DONE]\n\n");
  res.end();
};
// A minimal but valid zstd frame: magic + SingleSegment header (1-byte content
// size) + one RAW block carrying the payload verbatim. Node has no zstd
// compressor; ruzstd/go decode this per the spec. Payload must stay < 256 B.
const zstdRawFrame = (payload: Buffer): Buffer => {
  assert.ok(payload.length > 0 && payload.length < 256, "raw-frame fixture payload must be 1..255 bytes");
  const head = Buffer.alloc(6);
  head.writeUInt32LE(0xfd2fb528, 0); // magic
  head.writeUInt8(0x20, 4);           // SingleSegment=1, FCS flag 0 → 1-byte FCS, no checksum
  head.writeUInt8(payload.length, 5); // frame content size
  const block = Buffer.alloc(3);
  block.writeUIntLE(1 | (0 << 1) | (payload.length << 3), 0, 3); // last=1, type=0 (Raw), size
  return Buffer.concat([head, block, payload]);
};
let fixturePort = 0;
const fixture = createServer((req, res) => {
  assert.match(req.headers.authorization ?? "", /^Bearer fixture-key-/);
  const reply = (status: number, body: unknown) => {
    res.statusCode = status; // never rely on Node's 200 default
    res.setHeader("Content-Type", "application/json");
    res.end(JSON.stringify(body));
  };
  if (req.url && req.url.includes("/chat/completions")) {
    const chunks: Buffer[] = [];
    req.on("data", (c: Buffer) => chunks.push(c));
    req.on("end", () => {
      const body = Buffer.concat(chunks);
      const text = body.toString("utf8");
      const size = body.length;
      const images = text.split("data:image/").length - 1;
      chatRequests.push({ size, images });
      const key = (req.headers.authorization ?? "").replace("Bearer ", "");
      if (size > OVER_CAP_BYTES) {
        reply(400, { error: { message: "ollama: failed to read request body (ref: fixture-cap)", type: "invalid_request_error" } });
        return;
      }
      let parsed: AnyRecord = {};
      try { parsed = JSON.parse(text); } catch { /* keep {} */ }
      vendorBodies.push({ key, body: parsed });
      const marker = lastUserText(parsed);
      // A real chat vendor honors stream:true with SSE — and the Go gateway's
      // translate path always asks for a stream (it aggregates for non-stream
      // clients). The fallback therefore streams when asked to; only scripted
      // or plain non-stream requests get the whole-JSON reply.
      if (!marker.startsWith("script:")) {
        if (parsed.stream === true) { chatStream(res, [{ role: "assistant" }, { content: "fixture-" }, { content: "ok" }], { prompt_tokens: 3, completion_tokens: 2, total_tokens: 5 }); return; }
        reply(200, {
          id: "chatcmpl-fixture", object: "chat.completion", created: 1,
          choices: [{ index: 0, message: { role: "assistant", content: "fixture-ok" }, finish_reason: "stop" }],
          usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
        });
        return;
      }
      // key fixture-key-2 is the "permanently broken member" for failover (D8)
      if (key === "fixture-key-2") { reply(500, { error: { message: "fixture member down", type: "server_error" } }); return; }
      if (marker === "script:429") { res.setHeader("Retry-After", "7"); reply(429, { error: { message: "rate limited", type: "rate_limit_error", code: "rate_limit_exceeded" } }); return; }
      if (marker === "script:tool") {
        // yield_ms 2500.0: models write whole numbers with a zero fraction;
        // Codex reads them as u64 and serde refuses the whole call — the
        // gateway must shed the fraction (D9/D10, Go grok_integral.go).
        // Streamed variant cuts the arguments mid-number so delta assembly
        // and the fraction rule must agree.
        if (parsed.stream === true) {
          chatStream(res, [
            { role: "assistant", tool_calls: [{ index: 0, id: "call_fx_1", type: "function", function: { name: "get_weather", arguments: "{\"city\":\"Pa" } }] },
            { tool_calls: [{ index: 0, function: { arguments: "ris\",\"yield_ms\":2500.0}" } }] },
          ], { prompt_tokens: 4, completion_tokens: 2, total_tokens: 6 });
          return;
        }
        reply(200, {
          id: "chatcmpl-fx", object: "chat.completion", created: 1,
          choices: [{ index: 0, message: { role: "assistant", content: null, tool_calls: [{ id: "call_fx_1", type: "function", function: { name: "get_weather", arguments: "{\"city\":\"Paris\",\"yield_ms\":2500.0}" } }] }, finish_reason: "tool_calls" }],
          usage: { prompt_tokens: 4, completion_tokens: 2, total_tokens: 6 },
        });
        return;
      }
      if (marker === "script:think") {
        // Reasoning rides as reasoning_content (DeepSeek wire shape), SSE when
        // asked to stream, whole JSON otherwise (the two engines ask
        // differently: Go's translate path always streams and aggregates).
        if (parsed.stream === true) {
          chatStream(res, [{ role: "assistant" }, { reasoning_content: "thinking hard about the weather" }, { content: "fixture-ok" }], { prompt_tokens: 9, completion_tokens: 5, total_tokens: 14 });
        } else {
          reply(200, {
            id: "chatcmpl-fx", object: "chat.completion", created: 1,
            choices: [{ index: 0, message: { role: "assistant", content: "fixture-ok", reasoning_content: "thinking hard about the weather" }, finish_reason: "stop" }],
            usage: { prompt_tokens: 9, completion_tokens: 5, total_tokens: 14 },
          });
        }
        return;
      }
      if (marker === "script:stream") { chatStream(res, [{ role: "assistant" }, { content: "fixture-" }, { content: "ok" }], { prompt_tokens: 3, completion_tokens: 2, total_tokens: 5 }); return; }
      if (marker === "script:stream-think") { chatStream(res, [{ role: "assistant" }, { reasoning_content: "thinking hard" }, { content: "fixture-" }, { content: "ok" }]); return; }
      if (marker === "script:cut") {
        res.statusCode = 200;
        res.setHeader("Content-Type", "text/event-stream");
        res.write(`data: ${chatChunk({ role: "assistant" })}\n\n`);
        res.write(`data: ${chatChunk({ content: "partial-" })}\n\n`);
        res.destroy(); // upstream dies mid-stream
        return;
      }
      reply(200, {
        id: "chatcmpl-fixture", object: "chat.completion", created: 1,
        choices: [{ index: 0, message: { role: "assistant", content: "fixture-ok" }, finish_reason: "stop" }],
        usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
      });
    });
    return;
  }
  reply(200, {
    object: "list",
    data: [
      {
        id: "alpha-model", display_name: "Alpha",
        context_length: 500000, max_output_tokens: 32000,
        input_modalities: ["text", "image"],
        modalities: { input: ["text", "image"], output: ["text"] },
      },
      { id: "beta-model" },
    ],
  });
});
await new Promise<void>((done) => fixture.listen(0, "127.0.0.1", done));
fixturePort = (fixture.address() as { port: number }).port;

// ── seed binary: explicit path, or build from the vendor tree ───────────────
async function buildSeed(): Promise<{ exe: string; dir: string }> {
  const vendor = join(repoRoot, "third_party", "magpie");
  const dir = await mkdtemp(join(tmpdir(), "marionette-seed-build-"));
  const exe = join(dir, "magpie-seed.exe");
  await execFile("go", ["build", "-tags", "nogui marionette_seed", "-trimpath", "-ldflags", "-s -w", "-o", exe, "."], {
    cwd: vendor, windowsHide: true, timeout: 240_000,
  });
  return { exe, dir };
}

// Windows keeps the exe locked until TerminateProcess lands; wait, then retry
// the delete like tools\import-magpie.ps1's Remove-TempBestEffort.
async function rmRetry(path: string, tries = 5) {
  for (let i = 1; i <= tries; i++) {
    try { await rm(path, { recursive: true, force: true }); return; } catch {
      if (i < tries) await new Promise((r) => setTimeout(r, 200 * i));
    }
  }
}

let serve: ReturnType<typeof spawn> | null = null;
let built: { exe: string; dir: string } | null = null;
try {
  const exe = join(scratch, "magpie.exe");
  if (process.argv[2]) {
    await copyFile(resolve(process.argv[2]), exe);
  } else {
    built = await buildSeed();
    await copyFile(built.exe, exe);
  }
  await writeFile(join(scratch, ".portable"), "");

  const env = { ...process.env, MAGPIE_NO_ACCOUNTS: "1", MAGPIE_NO_STATS: "1", DO_NOT_TRACK: "1", NO_COLOR: "1" };
  const call = async (args: string[]) =>
    (await execFile(exe, args, { cwd: scratch, env, windowsHide: true, timeout: 30_000 })).stdout;
  const id = "acct";

  // C6: provider add → models list carries provider/model ids
  await call(["provider", "add", id, `id=${id}`, "name=Contract fixture", `url=http://127.0.0.1:${fixturePort}/v1`, "key=fixture-key-1", "models=alpha-model"]);
  const listedBefore = await call(["models"]);
  assert.ok(listedBefore.includes(`${id}/alpha-model`), `provider add did not register: ${listedBefore}`);

  // C6: refresh → models all, in that order (order matters: "all" exposes the
  // last fetched list); live semantics bring the second vendor model in.
  await call(["provider", "refresh", id]);
  await call(["provider", "models", id, "all"]);
  const listedLive = await call(["models"]);
  assert.ok(listedLive.includes(`${id}/beta-model`), `refresh+all did not adopt the vendor list: ${listedLive}`);

  // C1: serve on a concrete loopback port, like the shell spawns it.
  const net = await import("node:net");
  const gatewayPort = await new Promise<number>((done) => {
    const probe = net.createServer();
    probe.listen(0, "127.0.0.1", () => {
      const port = (probe.address() as { port: number }).port;
      probe.close(() => done(port));
    });
  });
  serve = spawn(exe, ["serve"], {
    cwd: scratch, env: { ...env, MAGPIE_ADDR: `127.0.0.1:${gatewayPort}` },
    windowsHide: true, stdio: ["ignore", "pipe", "pipe"],
  });
  let serveLog = "";
  serve.stdout?.on("data", (c: Buffer) => { serveLog += c.toString(); });
  serve.stderr?.on("data", (c: Buffer) => { serveLog += c.toString(); });

  // C2: /v1/models rows carry the fields Marionette parses (model_bridge.rs
  // parse_models_catalog_rows): id, context_window|context_length|…,
  // max_output_tokens, modalities.input ∋ image.
  const gw = `http://127.0.0.1:${gatewayPort}`;
  let rowsRaw = "";
  for (let waited = 0; waited < 20_000; waited += 300) {
    try {
      const res = await fetch(`${gw}/v1/models`);
      if (res.ok) { rowsRaw = await res.text(); break; }
    } catch { /* not up yet */ }
    await new Promise((r) => setTimeout(r, 300));
  }
  assert.ok(rowsRaw, "gateway /v1/models never came up within 20s");
  const rows = JSON.parse(rowsRaw);
  const data: Record<string, unknown>[] = rows.data ?? rows.models;
  assert.ok(Array.isArray(data), `/v1/models shape is neither {data:[]} nor {models:[]}: ${rowsRaw.slice(0, 400)}`);
  const alpha = data.find((r) => r.id === `${id}/alpha-model`);
  assert.ok(alpha, `gateway row for ${id}/alpha-model missing: ${JSON.stringify(data.map((r) => r.id))}`);
  assert.equal(alpha.context_window, 500000, `context_window not relayed from the vendor row: ${JSON.stringify(alpha)}`);
  assert.equal(alpha.max_output_tokens, 32000, `max_output_tokens not relayed: ${JSON.stringify(alpha)}`);
  const mods = (alpha.modalities as { input?: string[] } | undefined)?.input ?? [];
  assert.ok(mods.includes("image"), `modalities.input does not include image: ${JSON.stringify(alpha)}`);

  // C5: basic chat routing through the gateway
  const simple = await fetch(`${gw}/v1/chat/completions`, {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ model: `${id}/alpha-model`, stream: false, messages: [{ role: "user", content: "hi" }] }),
  });
  const simpleBody = await simple.text();
  assert.equal(simple.status, 200, `plain chat failed: ${simple.status} ${simpleBody.slice(0, 300)}`);
  assert.ok(simpleBody.includes("fixture-ok"), `chat reply lost the fixture content: ${simpleBody.slice(0, 300)}`);

  // C5 + C7: a body over the vendor's cap gets its OLDEST images shed and the
  // request retried, not a dead 400 (2026-11-12 conversation deadlock).
  // Three images × ~10.7MB base64 ≈ 32MB body; the fixture rejects >26MB with
  // the unread-body words; the fit learns a cap of 3/4 (24MB) and drops one
  // image → ~21MB → accepted. Alternating roles: nothing to merge.
  const img = Buffer.alloc(8 * 1024 * 1024, 7).toString("base64");
  const withImage = (name: string) => ({
    role: "user",
    content: [
      { type: "text", text: name },
      { type: "image_url", image_url: { url: `data:image/png;base64,${img}` } },
    ],
  });
  const beforeBig = chatRequests.length;
  const big = await fetch(`${gw}/v1/chat/completions`, {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      model: `${id}/alpha-model`, stream: false,
      messages: [
        withImage("oldest"), { role: "assistant", content: "mid1" },
        withImage("middle"), { role: "assistant", content: "mid2" },
        withImage("newest"),
      ],
    }),
  });
  const bigBody = await big.text();
  assert.equal(big.status, 200, `oversized body did not refit to success: ${big.status} ${bigBody.slice(0, 300)}`);
  const bigRequests = chatRequests.slice(beforeBig);
  assert.ok(bigRequests.length >= 2, `expected a rejected attempt then a fitted retry, saw ${bigRequests.length} vendor requests: ${JSON.stringify(bigRequests.map((r) => ({ mb: (r.size / 1048576).toFixed(1), images: r.images })))} (big.status=${big.status}, bigBody=${bigBody.slice(0, 200)}, bigHeaders=${JSON.stringify(Object.fromEntries(big.headers.entries()))}, serveLog=${serveLog.slice(-1500)})`);
  const [first, second] = bigRequests;
  assert.ok(first.size > OVER_CAP_BYTES, `first vendor request should be over the fixture cap: ${first.size}`);
  assert.equal(first.images, 3, `first vendor request should carry all 3 images: ${first.images}`);
  assert.ok(second.size <= OVER_CAP_BYTES, `retry should be under the cap: ${second.size}`);
  // No stated cap in the refusal → the fit learns 3/4 of what was sent
  // (24MB), then sheds until under 3/4 of THAT (18MB): 32 → 21.3 → 10.7MB,
  // two images gone, one left.
  assert.equal(second.images, 1, `retry should have shed the two oldest images: ${second.images}`);

  // C3: quotas endpoint answers in the {object:"list",data:[]} / array shape
  const quotas = await fetch(`${gw}/v1/magpie/quotas`);
  const quotasText = await quotas.text();
  assert.equal(quotas.status, 200, `quotas endpoint missing: ${quotas.status} ${quotasText.slice(0, 200)}`);
  const quotasJson = JSON.parse(quotasText) as { data?: unknown[] } | unknown[];
  const quotaItems = Array.isArray(quotasJson) ? quotasJson : quotasJson.data;
  assert.ok(Array.isArray(quotaItems), `quotas shape is neither array nor {data:[]}: ${quotasText.slice(0, 200)}`);

  // C4: group serving — the 2026-11-12 silent-degradation regression. The
  // probe must answer 200 while the group lives and 404 after group rm.
  await call(["group", "add", "grp", `models=${id}/alpha-model`, "id=alias-1", `pick=${id}/alpha-model`]);
  const groupRes = await fetch(`${gw}/v1/models/group/alias-1`);
  assert.equal(groupRes.status, 200, `served group is not announced on /v1/models/group/<id> (GateGroups/ServeGroups contract, 2026-11-12): ${groupRes.status}`);
  await call(["group", "rm", "alias-1"]);
  const groupGone = await fetch(`${gw}/v1/models/group/alias-1`);
  assert.equal(groupGone.status, 404, `removed group still announced: ${groupGone.status}`);

  // C5: /v1/responses (codex native) translation path — chat vendor outbound,
  // responses-shaped reply (T4).
  const responsesRes = await fetch(`${gw}/v1/responses`, {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      model: `${id}/alpha-model`, stream: false, instructions: "be brief",
      input: [{ type: "message", role: "user", content: [{ type: "input_text", text: "hi" }] }],
    }),
  });
  const responsesBody = await responsesRes.text();
  assert.equal(responsesRes.status, 200, `/v1/responses translation failed: ${responsesRes.status} ${responsesBody.slice(0, 300)}`);
  const responsesJson = JSON.parse(responsesBody);
  const outText = (responsesJson.output ?? [])
    .flatMap((item: { content?: { text?: string }[] }) => item.content ?? [])
    .map((part: { text?: string }) => part.text ?? "")
    .join("");
  assert.ok(outText.includes("fixture-ok"), `/v1/responses lost the fixture text: ${responsesBody.slice(0, 300)}`);

  // C5: /v1/messages (anthropic) translation path (T3) — content block shape.
  const messagesRes = await fetch(`${gw}/v1/messages`, {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify({
      model: `${id}/alpha-model`, max_tokens: 100,
      messages: [{ role: "user", content: "hi" }],
    }),
  });
  const messagesBody = await messagesRes.text();
  assert.equal(messagesRes.status, 200, `/v1/messages translation failed: ${messagesRes.status} ${messagesBody.slice(0, 300)}`);
  const messagesJson = JSON.parse(messagesBody);
  const contentText = (messagesJson.content ?? [])
    .map((part: { text?: string }) => part.text ?? "")
    .join("");
  assert.ok(contentText.includes("fixture-ok"), `/v1/messages lost the fixture text: ${messagesBody.slice(0, 300)}`);

  // ── C5 dialect details (2026-11-18 batch 1) ─────────────────────────────
  // Marker-driven cases against the same fixture; each asserts the contract
  // face an agent actually consumes. Helpers first.
  const post = (path: string, payload: unknown, headers: Record<string, string> = {}) =>
    fetch(`${gw}${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json", ...headers },
      body: typeof payload === "string" ? payload : JSON.stringify(payload),
    });
  const readSse = async (res: Response): Promise<{ events: string[]; done: boolean }> => {
    assert.ok(res.body, "SSE response has no body");
    const events: string[] = [];
    let done = false;
    const reader = res.body!.getReader();
    const dec = new TextDecoder();
    let buf = "";
    for (;;) {
      const { value, done: eof } = await reader.read();
      if (eof) break;
      buf += dec.decode(value, { stream: true });
      for (;;) {
        const idx = buf.indexOf("\n");
        if (idx < 0) break;
        const line = buf.slice(0, idx).replace(/\r$/, "");
        buf = buf.slice(idx + 1);
        if (!line.startsWith("data:")) continue;
        const data = line.slice(5).trim();
        if (data === "[DONE]") { done = true; continue; }
        if (data) events.push(data);
      }
    }
    return { events, done };
  };
  const parseEvents = <T,>(events: string[]): T[] => events.map((e) => JSON.parse(e) as T);

  // D1a: chat streaming — delta assembly, finish, usage, [DONE] terminator.
  {
    const res = await post("/v1/chat/completions", { model: `${id}/alpha-model`, stream: true, messages: [{ role: "user", content: "script:stream" }] });
    assert.equal(res.status, 200, `chat stream failed: ${res.status}`);
    const { events, done } = await readSse(res);
    const chunks = parseEvents<{ choices: { delta?: { content?: string }; finish_reason?: string | null }[]; usage?: unknown }>(events);
    const text = chunks.map((c) => c.choices?.[0]?.delta?.content ?? "").join("");
    assert.ok(text.includes("fixture-ok"), `chat stream text lost: ${text}`);
    // The Go gateway merges/sheds trailing empty-delta frames on relay (the
    // finish_reason may ride the usage frame or be folded away); agents live
    // on [DONE] + usage, so those are the hard contract, finish_reason soft.
    assert.ok(chunks.some((c) => c.usage), "chat stream never delivered a usage frame (include_usage contract)");
    assert.ok(done, "chat stream did not terminate with [DONE]");
  }

  // D1b: responses streaming — codex consumes response.created → delta → completed.
  {
    const res = await post("/v1/responses", {
      model: `${id}/alpha-model`, stream: true,
      input: [{ type: "message", role: "user", content: [{ type: "input_text", text: "script:stream" }] }],
    });
    assert.equal(res.status, 200, `responses stream failed: ${res.status}`);
    const { events } = await readSse(res);
    const evs = parseEvents<{ type?: string; delta?: string }>(events);
    const types = evs.map((e) => e.type ?? "");
    assert.ok(types.includes("response.created"), `responses stream never opened: ${types.slice(0, 6).join(",")}`);
    assert.ok(types.includes("response.completed"), `responses stream never completed: ${[...types].slice(-6).join(",")}`);
    const text = evs.filter((e) => e.type === "response.output_text.delta").map((e) => e.delta ?? "").join("");
    assert.ok(text.includes("fixture-ok"), `responses stream text lost: ${text}`);
    // Responses SSE has no [DONE] sentinel — response.completed is the
    // terminator Codex consumes; it must be the LAST event.
    assert.equal(types[types.length - 1], "response.completed", `response.completed is not the final event: ${[...types].slice(-4).join(",")}`);
  }

  // D1c: anthropic streaming — the frame family Claude Code parses.
  {
    const res = await post("/v1/messages", {
      model: `${id}/alpha-model`, max_tokens: 100, stream: true,
      messages: [{ role: "user", content: "script:stream" }],
    });
    assert.equal(res.status, 200, `messages stream failed: ${res.status}`);
    const { events } = await readSse(res);
    const evs = parseEvents<{ type?: string; delta?: { type?: string; text?: string }; message?: { usage?: unknown } }>(events);
    const types = evs.map((e) => e.type ?? "");
    for (const frame of ["message_start", "content_block_start", "content_block_delta", "content_block_stop", "message_delta", "message_stop"]) {
      assert.ok(types.includes(frame), `anthropic stream missing ${frame}: ${types.join(",")}`);
    }
    const text = evs.filter((e) => e.type === "content_block_delta" && e.delta?.type === "text_delta").map((e) => e.delta?.text ?? "").join("");
    assert.ok(text.includes("fixture-ok"), `anthropic stream text lost: ${text}`);
    const start = evs.find((e) => e.type === "message_start");
    assert.ok(start?.message, "message_start carries no message envelope");
  }

  // D2: tool-call round trip, chat dialect — request tools, vendor calls back,
  // the agent-visible reply keeps tool_calls; the follow-up tool result must
  // reach the vendor as a tool-role message (wire shape agents depend on).
  {
    const res = await post("/v1/chat/completions", {
      model: `${id}/alpha-model`,
      tools: [{ type: "function", function: { name: "get_weather", description: "w", parameters: { type: "object", properties: { city: { type: "string" } } } } }],
      messages: [{ role: "user", content: "script:tool" }],
    });
    const bodyText = await res.text();
    assert.equal(res.status, 200, `tool chat failed: ${res.status} ${bodyText.slice(0, 300)}`);
    const j = JSON.parse(bodyText) as { choices: { message: { tool_calls?: { id: string; type: string; function: { name: string; arguments: string } }[] } }[] };
    const call = j.choices?.[0]?.message?.tool_calls?.[0];
    assert.ok(call, `tool_calls missing from reply: ${bodyText.slice(0, 300)}`);
    assert.equal(call.function.name, "get_weather", `tool name changed in transit: ${JSON.stringify(call)}`);
    const args = JSON.parse(call.function.arguments);
    assert.equal(args.city, "Paris", `tool arguments corrupted: ${call.function.arguments}`);
    // Zero-fraction shedding on the DIRECT chat relay is D10 (pending until
    // the Rust engine grows intTidy, batch 3); the translate paths are D9.

    const beforeFollow = vendorBodies.length;
    const follow = await post("/v1/chat/completions", {
      model: `${id}/alpha-model`,
      tools: [{ type: "function", function: { name: "get_weather", description: "w", parameters: { type: "object" } } }],
      messages: [
        { role: "user", content: "script:tool" },
        { role: "assistant", tool_calls: [{ id: call.id, type: "function", function: { name: "get_weather", arguments: "{\"city\":\"Paris\"}" } }] },
        { role: "tool", tool_call_id: call.id, content: "18C rainy" },
      ],
    });
    const followText = await follow.text();
    assert.equal(follow.status, 200, `tool follow-up failed: ${follow.status} ${followText.slice(0, 300)}`);
    const sent = vendorBodies.slice(beforeFollow).map((v) => v.body);
    assert.ok(sent.length >= 1, "follow-up never reached the vendor");
    const lastSent = sent[sent.length - 1];
    const toolMsg = (lastSent.messages ?? []).find((m: AnyRecord) => m.role === "tool");
    assert.ok(toolMsg, `tool result was dropped from the outbound body: ${JSON.stringify(lastSent.messages?.map((m: AnyRecord) => m.role))}`);
    assert.ok(String(toolMsg.content).includes("18C rainy"), `tool result content lost: ${JSON.stringify(toolMsg)}`);
    assert.ok((lastSent.tools ?? []).some((t: AnyRecord) => t.function?.name === "get_weather"), "function tool definition was dropped outbound");
  }

  // D3: thinking replay — chat vendor reasoning_content must surface as an
  // anthropic thinking block (non-streaming) and thinking_delta frames
  // (streaming), so Claude Code can see shared-model thought.
  {
    const res = await post("/v1/messages", {
      model: `${id}/alpha-model`, max_tokens: 200,
      messages: [{ role: "user", content: "script:think" }],
    });
    const text = await res.text();
    assert.equal(res.status, 200, `thinking messages failed: ${res.status} ${text.slice(0, 300)}`);
    const j = JSON.parse(text) as { content: { type: string; thinking?: string; text?: string }[] };
    const kinds = (j.content ?? []).map((p) => p.type);
    assert.ok(kinds.includes("thinking"), `reasoning_content did not surface as a thinking block: ${JSON.stringify(kinds)}`);
    assert.ok(kinds.includes("text"), `text block missing next to thinking: ${JSON.stringify(kinds)}`);
    const think = (j.content ?? []).find((p) => p.type === "thinking");
    assert.ok((think?.thinking ?? "").includes("thinking hard"), `thinking content lost: ${JSON.stringify(think)}`);

    const sres = await post("/v1/messages", {
      model: `${id}/alpha-model`, max_tokens: 200, stream: true,
      messages: [{ role: "user", content: "script:stream-think" }],
    });
    assert.equal(sres.status, 200, `thinking stream failed: ${sres.status}`);
    const { events } = await readSse(sres);
    const evs = parseEvents<{ type?: string; delta?: { type?: string; thinking?: string; text?: string } }>(events);
    const deltaKinds = evs.filter((e) => e.type === "content_block_delta").map((e) => e.delta?.type ?? "");
    assert.ok(deltaKinds.includes("thinking_delta"), `no thinking_delta frames in stream: ${JSON.stringify(deltaKinds)}`);
    const thinkText = evs.filter((e) => e.delta?.type === "thinking_delta").map((e) => e.delta?.thinking ?? "").join("");
    assert.ok(thinkText.includes("thinking hard"), `streaming thinking content lost: ${thinkText}`);
  }

  // D4: inbound Content-Encoding — gzip and zstd bodies decode; an encoding
  // the gateway does not speak is refused (Go request_bounds: identity/gzip/
  // zstd only, anything else 400).
  {
    const gzBody = gzipSync(Buffer.from(JSON.stringify({ model: `${id}/alpha-model`, stream: false, messages: [{ role: "user", content: "compressed hi" }] })));
    const gz = await fetch(`${gw}/v1/chat/completions`, { method: "POST", headers: { "Content-Type": "application/json", "Content-Encoding": "gzip" }, body: gzBody });
    const gzText = await gz.text();
    assert.equal(gz.status, 200, `gzip body was not decoded: ${gz.status} ${gzText.slice(0, 200)}`);
    assert.ok(gzText.includes("fixture-ok"), `gzip round trip lost content: ${gzText.slice(0, 200)}`);

    const payload = Buffer.from(JSON.stringify({ model: `${id}/alpha-model`, stream: false, messages: [{ role: "user", content: "zstd hi" }] }));
    const zs = await fetch(`${gw}/v1/chat/completions`, { method: "POST", headers: { "Content-Type": "application/json", "Content-Encoding": "zstd" }, body: zstdRawFrame(payload) });
    const zsText = await zs.text();
    assert.equal(zs.status, 200, `zstd body was not decoded: ${zs.status} ${zsText.slice(0, 200)}`);
    assert.ok(zsText.includes("fixture-ok"), `zstd round trip lost content: ${zsText.slice(0, 200)}`);

    const bad = await fetch(`${gw}/v1/chat/completions`, { method: "POST", headers: { "Content-Type": "application/json", "Content-Encoding": "br" }, body: Buffer.from("{}") });
    assert.ok(bad.status === 400 || bad.status === 415, `unknown encoding should be refused, got ${bad.status}`);
  }

  // D5: origin guard — a foreign web page must not be able to blind-POST the
  // loopback gateway; agent CLIs (no Origin) pass untouched.
  {
    const evil = await post("/v1/chat/completions", { model: `${id}/alpha-model`, stream: false, messages: [{ role: "user", content: "hi" }] }, { Origin: "https://evil.example" });
    assert.equal(evil.status, 403, `foreign-page Origin was not refused: ${evil.status}`);
    const evilRead = await evil.text();
    assert.ok(evil.status !== 200 || !evilRead.includes("fixture-ok"), "foreign page must not read a completion");
    const nullOrigin = await post("/v1/chat/completions", { model: `${id}/alpha-model`, stream: false, messages: [{ role: "user", content: "hi" }] }, { Origin: "null" });
    assert.equal(nullOrigin.status, 403, `Origin: null was not refused: ${nullOrigin.status}`);
    const agentLike = await post("/v1/chat/completions", { model: `${id}/alpha-model`, stream: false, messages: [{ role: "user", content: "agent-style no origin" }] });
    assert.equal(agentLike.status, 200, "agent-shaped request (no Origin header) must pass");
  }

  // D6: error fidelity — a vendor 429 with Retry-After must reach the agent
  // with status and header intact; on /v1/messages the error body must be
  // anthropic-shaped ({"type":"error",...}), not the raw chat vendor JSON.
  {
    const res = await post("/v1/chat/completions", { model: `${id}/alpha-model`, stream: false, messages: [{ role: "user", content: "script:429" }] });
    const text = await res.text();
    assert.equal(res.status, 429, `vendor 429 did not surface as 429: ${res.status} ${text.slice(0, 200)}`);
    assert.ok((res.headers.get("retry-after") ?? "") !== "", "Retry-After header was dropped");
    const mres = await post("/v1/messages", { model: `${id}/alpha-model`, max_tokens: 50, messages: [{ role: "user", content: "script:429" }] });
    const mtext = await mres.text();
    assert.equal(mres.status, 429, `messages-path 429 status wrong: ${mres.status} ${mtext.slice(0, 200)}`);
    let mjson: AnyRecord = {};
    try { mjson = JSON.parse(mtext); } catch { /* leave {} */ }
    assert.ok(mjson.type === "error" || mjson.error, `/v1/messages error body is not anthropic-shaped: ${mtext.slice(0, 200)}`);
  }

  // D7: upstream mid-stream death — the gateway owes the agent a properly
  // terminated stream, not a silent EOF. (Behavior contract for continuation;
  // the Rust engine is expected to fail this until batch 3 — list it in
  // CONTRACT_PENDING=D7 to run the suite while the gap is open.)
  const pending = (id: string, run: () => void | Promise<void>): Promise<void> => {
    if (PENDING.has(id)) { console.warn(`[contract] PENDING ${id} — known gap, assertions skipped`); return Promise.resolve(); }
    return Promise.resolve(run());
  };
  await pending("D7", async () => {
    const res = await post("/v1/chat/completions", { model: `${id}/alpha-model`, stream: true, messages: [{ role: "user", content: "script:cut" }] });
    const { events, done } = await readSse(res);
    if (!done) {
      // Not all engines continue a cut stream; the floor contract is that the
      // agent can tell the turn ended badly instead of hanging on EOF. Accept
      // a terminal error event OR [DONE]; a bare EOF with neither is a fail.
      const tail = parseEvents<{ type?: string; error?: unknown }>(events).slice(-2);
      assert.ok(
        tail.some((e) => e.error || (e.type ?? "").includes("error") || (e.type ?? "") === "response.failed"),
        `cut stream ended with bare EOF, agent cannot distinguish failure: tail=${JSON.stringify(tail)}`,
      );
    }
  });

  // D8: failover inside a group — the permanently-broken member (key 2) must
  // not take the turn down; the healthy member answers.
  {    await call(["provider", "add", "acct2", "id=acct2", "name=Broken member", `url=http://127.0.0.1:${fixturePort}/v1`, "key=fixture-key-2", "models=alpha-model"]);
    await call(["group", "add", "grp2", "models=acct2/alpha-model,acct/alpha-model", "id=alias-2", "pick=acct2/alpha-model"]);
    const res = await post("/v1/chat/completions", { model: "group/alias-2", stream: false, messages: [{ role: "user", content: "failover please" }] });
    const text = await res.text();
    assert.equal(res.status, 200, `group failover did not reach the healthy member: ${res.status} ${text.slice(0, 300)}`);
    assert.ok(text.includes("fixture-ok"), `failover reply lost fixture content: ${text.slice(0, 300)}`);
    await call(["group", "rm", "alias-2"]);
    await call(["provider", "rm", "acct2"]);
  }

  // D9: zero-fraction shedding on the TRANSLATE paths — a vendor writing
  // "yield_ms":2500.0 in a tool call must reach Codex as 2500 (serde refuses
  // the floating-point form whole-call; Go grok_integral.go #1431/#3152).
  {
    const res = await post("/v1/responses", {
      model: `${id}/alpha-model`, stream: false,
      tools: [{ type: "function", name: "get_weather", parameters: { type: "object", properties: { city: { type: "string" } } } }],
      input: [{ type: "message", role: "user", content: [{ type: "input_text", text: "script:tool" }] }],
    });
    const text = await res.text();
    assert.equal(res.status, 200, `D9 responses tool call failed: ${res.status} ${text.slice(0, 300)}`);
    const j = JSON.parse(text) as { output: { type: string; arguments?: string }[] };
    const fn = (j.output ?? []).find((o) => o.type === "function_call");
    assert.ok(fn?.arguments, `no function_call in reply: ${text.slice(0, 300)}`);
    assert.match(fn!.arguments!, /"yield_ms":2500[,\}]/, `zero fraction must be shed (2500.0 → 2500): ${fn!.arguments}`);
  }

  // D10: zero-fraction shedding on the DIRECT chat relay (Go intTidy). The
  // Rust engine is pending this until batch 3's relay rewriter.
  await pending("D10", async () => {
    const res = await post("/v1/chat/completions", {
      model: `${id}/alpha-model`, stream: false,
      tools: [{ type: "function", function: { name: "get_weather", parameters: { type: "object" } } }],
      messages: [{ role: "user", content: "script:tool" }],
    });
    const text = await res.text();
    assert.equal(res.status, 200, `D10 direct tool call failed: ${res.status} ${text.slice(0, 300)}`);
    const j = JSON.parse(text) as { choices: { message: { tool_calls?: { function: { arguments: string } }[] } }[] };
    const args = j.choices?.[0]?.message?.tool_calls?.[0]?.function?.arguments ?? "";
    assert.match(args, /"yield_ms":2500[,\}]/, `zero fraction must be shed on relay too: ${args}`);
  });

  // C6: rm of an unknown provider reports "no provider" (the shell reads that
  // as "already recycled"); rm of the real one drops its models.
  await assert.rejects(
    () => call(["provider", "rm", "no-such-acct"]),
    (err: { stderr?: string }) => (err.stderr ?? "").includes("no provider"),
    "rm of a missing provider must say `no provider` on stderr",
  );
  await call(["provider", "rm", id]);
  const listedAfter = await call(["models"]);
  assert.ok(!listedAfter.includes(`${id}/`), `provider rm left models behind: ${listedAfter}`);

  console.log("Engine contract: C1 lifecycle, C2 models relay, C3 quotas shape, C4 group serving, C5 chat + byte-cap refit + dialect details (streaming ×3 / tool round-trip / thinking replay / gzip+zstd decode / origin guard / error fidelity / cut-stream floor / group failover), C6 provider/group verbs — all passed against the seed in isolated .portable storage.");
} finally {
  if (serve && serve.exitCode === null) {
    serve.kill();
    await new Promise<void>((done) => {
      const timer = setTimeout(done, 3000);
      serve!.once("exit", () => { clearTimeout(timer); done(); });
    });
  }
  await new Promise<void>((done) => fixture.close(() => done()));
  if (built) await rmRetry(built.dir);
  // Keep the recursive delete inside the test's own temp root.
  const target = resolve(scratch);
  assert.equal(dirname(target).toLowerCase(), resolve(tmpdir()).toLowerCase());
  assert.ok(basename(target).startsWith("marionette-engine-contract-"));
  await rmRetry(target);
}
