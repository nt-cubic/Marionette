import { spawn } from "child_process";
import readline from "node:readline";

const grok = "C:\\Users\\NT's Station\\.grok\\bin\\grok.exe";
const child = spawn(grok, ["--trust", "agent", "stdio"], {
  cwd: "D:\\Projects\\Marionette",
  stdio: ["pipe", "pipe", "pipe"],
});

let seq = 0;
const pending = new Map();
const out = readline.createInterface({ input: child.stdout });
const err = readline.createInterface({ input: child.stderr });

err.on("line", (l) => console.log("[stderr]", l));

function rpc(method, params) {
  const id = ++seq;
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
  });
}

out.on("line", (line) => {
  let msg;
  try { msg = JSON.parse(line); } catch { console.log("[raw]", line); return; }
  if (msg.id != null) {
    const p = pending.get(msg.id);
    if (p) { pending.delete(msg.id); p.resolve(msg); }
  } else {
    console.log("[event]", msg.method, JSON.stringify(msg.params).slice(0, 400));
  }
});

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

try {
  const init = await rpc("initialize", {
    protocolVersion: 1,
    clientCapabilities: { fs: { readTextFile: true }, glob: true, testing: false },
    clientID: "marionette-test",
  });
  console.log("[initialize] ok, hasModels:", !!init.result?._meta?.modelState?.availableModels);

  const ns = await rpc("session/new", {
    cwd: "D:\\Projects\\Marionette",
    mcpServers: [],
    additionalDirectories: [],
  });
  console.log("[session/new] sessionId:", ns.result?.sessionId);
  const models = ns.result?.models?.availableModels;
  console.log("[session/new] models:", models?.map((m) => m.modelId).join(", "));
  console.log("[session/new] currentModelId:", ns.result?.models?.currentModelId);

  // Pick a local model explicitly
  await rpc("session/set_model", {
    sessionId: ns.result?.sessionId,
    modelId: "qwen3.8-27b",
    _meta: {},
  });
  console.log("[set_model qwen3.8-27b] sent");

  // Run a trivial prompt
  const start = Date.now();
  const promptP = rpc("session/prompt", {
    sessionId: ns.result?.sessionId,
    prompt: "Reply with exactly: PONG",
    files: [],
  });
  // session/prompt resolves on completion of the turn; give it a timeout
  const timeout = new Promise((_, rej) => setTimeout(() => rej(new Error("prompt timeout")), 60000));
  const res = await Promise.race([promptP, timeout]);
  console.log("[session/prompt] elapsed(ms):", Date.now() - start);
  const data = res.result ?? res.error;
  console.log("[session/prompt] result keys:", Object.keys(data || {}).join(","));
  console.log("[session/prompt] error:", res.error ? JSON.stringify(res.error) : "none");
  console.log("[session/prompt] content:", JSON.stringify(data).slice(0, 800));
} catch (e) {
  console.log("FAILED:", e.message);
} finally {
  child.kill();
  process.exit(0);
}
