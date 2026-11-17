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
      if (size > OVER_CAP_BYTES) {
        reply(400, { error: { message: "ollama: failed to read request body (ref: fixture-cap)", type: "invalid_request_error" } });
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

  console.log("Engine contract: C1 lifecycle, C2 models relay (window/output/modalities), C3 quotas shape, C4 group serving, C5 chat + byte-cap refit, C6 provider/group verbs — all passed against the seed in isolated .portable storage.");
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
