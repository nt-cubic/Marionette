// Run with Node's test runner and Playwright on the module path; see README.md.
// A cost note names a model an agent recorded no name for (tony, Discord:
// Usage › Sessions' 「按生效价格」 read "未计入：，没有已知价格"). The
// session stats count tokens under the model "", which has no price; the
// note, the session row's cost and models, and its details now name it "a
// model with no name". English and Chinese, Chromium and WebKit.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const today = new Date().toISOString().slice(0, 10);
const at = (min) => new Date(Date.now() - min * 60e3).toISOString();
const usage = [
  { agent: "codex", cwd: "/work/app", model: "gpt-5", input: 4000, output: 400, cache_read: 0, cache_write: 0, cost: 0.02, priced: true },
  { agent: "codex", cwd: "/work/app", model: "", input: 1000, output: 100, cache_read: 0, cache_write: 0, cost: 0, priced: false },
];
const session = {
  agent: "codex", name: "Codex", icon: "openai", id: "c-1", cwd: "/work/app", title: "port the parser", start: at(30), last: at(5),
  models: [
    { model: "gpt-5", input: 4000, output: 400, cache_read: 0, cache_write: 0, cost: 0.02, priced: true },
    { model: "", input: 1000, output: 100, cache_read: 0, cache_write: 0, cost: 0, priced: false },
  ],
  input: 5000, output: 500, cache_read: 0, cache_write: 0, cost: 0.02, unpriced: 1,
};

function serve(lang) {
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" } });
    if (url.pathname === "/api/sessions/progress") return json({ indexing: false });
    if (url.pathname === "/api/sessions") return json({ sessions: [session], dirs: [] });
    if (url.pathname === "/api/sessions/stats") return json({ from: today, to: today, days: [{ date: today, usage, active: [] }], agents: { codex: "Codex" } });
    if (url.pathname === "/api/groups") return json({ groups: [] });
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    const body = await fs.readFile(file).catch(() => null);
    return body ? route.fulfill({ body, contentType }) : route.fulfill({ status: 404, body: "" });
  };
}

const want = {
  en: { note: "Not counted: a model with no name, with no known price", models: "gpt-5, a model with no name" },
  zh: { note: "未计入：未注明名称的模型，没有已知价格", models: "gpt-5, 未注明名称的模型" },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh"]) {
    test(`${engine} ${lang}: a cost note names a model recorded with no name`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const page = await (await browser.newContext({ viewport: { width: 900, height: 700 }, reducedMotion: "reduce" })).newPage();
      page.setDefaultTimeout(8000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/*", serve(lang));
      await page.addInitScript(() => { localStorage.setItem("magpie.usageTab", "sessions"); localStorage.setItem("magpie.sessRange", "all"); });
      await page.goto("http://magpie.test/?view=usage");
      const row = page.locator("#sessList .row.sess", { hasText: "port the parser" });
      await row.waitFor();
      await page.waitForFunction(() => document.querySelector("#usageCost")?.title);
      const notes = await page.evaluate(() => ({
        header: document.querySelector("#usageCost").title,
        row: document.querySelector("#sessList .row.sess .cost")?.title,
        sub: document.querySelector("#sessList .row.sess .sub")?.textContent,
      }));
      for (const [where, text] of Object.entries(notes)) {
        assert.doesNotMatch(text || "", /[:：]\s*[,，]|,\s*$|,\s*,|^\s*,/, `${where} has a blank where a model's name goes: ${JSON.stringify(text)}`);
      }
      assert.equal(notes.header, want[lang].note, "the cost header's note names the unpriced model");
      assert.equal(notes.row, want[lang].note, "the session row's cost note names it");
      assert(notes.sub.includes(want[lang].models), `the row lists both models: ${JSON.stringify(notes.sub)}`);
      assert.deepEqual(errors, []);
    });
  }
}
