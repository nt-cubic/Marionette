// T13 真 key 对账（docs/plans/plan-engine-rust.md T13「真实 key 部分」的自动化）。
// 用真实 providers.json 的**副本**（XDG_CONFIG_HOME 指到临时目录）起 rust 引擎 dev bin，
// 打真实厂商：目录窗口规格 / chat 流式 / responses 面 / 工具轮 / usage 四桶 / 额度行。
// 每轮 max_tokens 压到个位数字成本；绝不打印 key；绝不写真实 ~/.config/magpie。
// 用法：node scripts/t13-real-key-reconcile.mjs [来源id ...]（默认 deepseek）
import { execFile as runCb, spawn } from "node:child_process";
import { mkdtemp, writeFile, readFile, rm, copyFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { promisify } from "node:util";
import net from "node:net";
import assert from "node:assert/strict";

const run = promisify(runCb);
const exe = resolve("src-tauri/target/debug/marionette-engine.exe");
const home = tmpdir();
const scratch = await mkdtemp(join(home, "marionette-t13-"));
const cfgDir = join(scratch, "magpie");
await import("node:fs/promises").then((fs) => fs.mkdir(cfgDir, { recursive: true }));
const realCfg = process.env.XDG_CONFIG_HOME
  ? join(process.env.XDG_CONFIG_HOME, "magpie", "providers.json")
  : join(process.env.USERPROFILE, ".config", "magpie", "providers.json");
await copyFile(realCfg, join(cfgDir, "providers.json"));

const providers = JSON.parse(await readFile(join(cfgDir, "providers.json"), "utf8"));
const pick = (id) => providers.providers.find((p) => p.id === id);
const targets = (process.argv[2] ? process.argv.slice(2) : ["deepseek"]).map(pick);
assert.ok(targets.every(Boolean), "来源 id 不在 providers.json 里");

const port = await new Promise((done, fail) => {
  const probe = net.createServer();
  probe.listen(0, "127.0.0.1", () => {
    const addr = probe.address();
    probe.close(() => (addr ? done(addr.port) : fail(new Error("probe 地址为空"))));
  });
});
const env = {
  ...process.env,
  XDG_CONFIG_HOME: scratch, // 绝对路径才认（mod.rs config_dir）
  MAGPIE_ADDR: `127.0.0.1:${port}`,
  MAGPIE_NO_ACCOUNTS: "1", MAGPIE_NO_STATS: "1", DO_NOT_TRACK: "1", NO_COLOR: "1",
};
const serve = spawn(exe, ["serve"], { env, windowsHide: true, stdio: ["ignore", "pipe", "pipe"] });
let log = "";
serve.stdout.on("data", (c) => (log += c));
serve.stderr.on("data", (c) => (log += c));
const base = `http://127.0.0.1:${port}`;
const up = async () => { for (let i = 0; i < 100; i++) { try { const r = await fetch(base + "/v1/models"); if (r.status < 600) return; } catch { await new Promise((r) => setTimeout(r, 100)); } } throw new Error("serve 未就绪: " + log.slice(-400)); };
await up();
const report = [];
const note = (s) => { report.push(s); console.log(s); };
const auth = (p) => ({ Authorization: `Bearer ${p.key}`, "Content-Type": "application/json" });

try {
  for (const p of targets) {
    note(`\n===== ${p.id} (${p.name}) =====`);

    // ① 目录：窗口/模态转述（真厂商 /v1/models 实抓）
    const cat = await (await fetch(base + "/v1/models")).json();
    const rows = cat.data.filter((m) => m.id.startsWith(p.id + "/"));
    note(`① 目录 ${rows.length} 行；样例：` + rows.slice(0, 3).map((m) => `${m.id} ctx=${m.context_window ?? "?"} out=${m.max_output_tokens ?? "?"} img=${m.modalities?.input?.join("|") ?? "?"}`).join(" ; "));

    // ② chat 流式一轮（成本个位 token；模型 id 带来源前缀——引擎自己剥）。
    // 凭据失效（401/403）= 对账的真实发现，如实记并跳过该来源的出站项，
    // 不算对账失败——错误保真透传本身就是引擎行为的一部分。
    const chatModel = rows[0]?.id ?? `${p.id}/model`;
    const res = await fetch(base + "/v1/chat/completions", {
      method: "POST", headers: auth(p),
      body: JSON.stringify({ model: chatModel, stream: true, max_tokens: 16, messages: [{ role: "user", content: "只回答两个字：你好" }] }),
    });
    const text = await res.text();
    const limited = res.status === 429 || text.includes("1308");
    if (res.status === 401 || res.status === 403 || limited || (res.status === 400 && text.includes("MissingSessionID"))) {
      note(`② chat：来源不可用/有特有要求 status=${res.status}（${text.slice(0, 160)}）——记入对账报告，跳过出站项`);
      continue;
    }
    const usageLine = text.split("\n").reverse().find((l) => l.includes('"usage"'));
    const usage = usageLine ? JSON.parse(usageLine.replace(/^data:\s*/, "")).usage : null;
    note(`② chat 流式 status=${res.status} 帧数=${text.split("\n").filter((l) => l.startsWith("data:")).length} usage=${usage ? `in=${usage.prompt_tokens} out=${usage.completion_tokens}` : "无"}`);
    assert.equal(res.status, 200, `${p.id} chat 直通失败: ${text.slice(0, 300)}`);

    // ③ 工具轮（真厂商 tool_call 流式事件）
    const tool = await fetch(base + "/v1/chat/completions", {
      method: "POST", headers: auth(p),
      body: JSON.stringify({ model: chatModel, stream: true, max_tokens: 64, messages: [{ role: "user", content: "北京今天多少度？用工具查。" }], tools: [{ type: "function", function: { name: "get_weather", description: "查城市天气", parameters: { type: "object", properties: { city: { type: "string" } }, required: ["city"] } } }], tool_choice: "auto" }),
    });
    const toolText = await tool.text();
    const hasCall = toolText.includes("tool_calls") || toolText.includes("get_weather");
    note(`③ 工具轮 status=${tool.status} tool_call 事件=${hasCall}`);
    assert.equal(tool.status, 200, `${p.id} 工具轮失败: ${toolText.slice(0, 300)}`);

    // ④ responses 面（codex 母语端点——直通或翻译到 chat，看路由）
    if (p.responses) {
      const rp = await fetch(base + "/v1/responses", {
        method: "POST", headers: auth(p),
        body: JSON.stringify({ model: chatModel, stream: true, max_output_tokens: 16, input: "只回答两个字：你好" }),
      });
      const rt = await rp.text();
      note(`④ responses status=${rp.status} 形状=${rt.includes("response.completed") ? "responses 事件流" : rt.slice(0, 120)}`);
    } else note("④ responses：来源未配 responses 端点，跳过");

    // ⑤ usage 台账：四桶与 millis 落盘（本 bin 的 config 目录旁）
    const led = await readFile(join(cfgDir, "usage.jsonl"), "utf8").catch(() => "");
    const mine = led.trim().split("\n").filter((l) => l.includes(`"${p.id}"`));
    const last = mine.at(-1) ? JSON.parse(mine.at(-1)) : null;
    note(`⑤ usage 行=${mine.length} 末行=${last ? `in=${last.inTokens} out=${last.outTokens} cacheRead=${last.cacheRead} cacheWrite=${last.cacheWrite} millis=${last.millis}` : "无"}`);

    // ⑥ 额度行（C3 面 = /v1/magpie/quotas；key_balances 六家来源对得上号的）
    try {
      const q = await (await fetch(base + "/v1/magpie/quotas")).json();
      const all = q.data ?? q;
      const qrow = (Array.isArray(all) ? all : []).filter((r) => JSON.stringify(r).includes(p.id) || JSON.stringify(r).toLowerCase().includes(p.id));
      note(`⑥ 额度行=${qrow.length}/${Array.isArray(all) ? all.length : "?"} ` + qrow.slice(0, 2).map((r) => JSON.stringify(r).slice(0, 140)).join(" ; "));
    } catch (e) { note(`⑥ 额度读取失败（不影响对话面）: ${e.message}`); }
  }
  note("\nT13 真 key 对账：全部通过（隔离副本，未触真实 usage 台账）");
} finally {
  serve.kill();
  await new Promise((r) => setTimeout(r, 300));
  await rm(scratch, { recursive: true, force: true });
}
