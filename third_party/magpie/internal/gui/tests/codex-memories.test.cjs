// Run with Node's test runner and Playwright on the module path; see README.md.
// Codex writes its memories with models of its own (gpt-5.6-terra to
// consolidate, gpt-5.6-luna to sum a thread up), whatever its model (Yc on
// Discord). The model it writes them with is a square in Codex's row, the
// [memories] extract_model and consolidation_model: unset it says what
// Codex uses, it opens the app's model picker (no native select) with
// Default first, a model picked is posted and lights the square, and
// Default posts "" to take the keys out. A click scrolls nothing, and
// nothing has a coloured left border. In English and Chinese. No backend:
// the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const models = [{ value: "gpt-5.5", label: "GPT-5.5", ref: "openai/gpt-5.5" }, { value: "gpt-5.4-mini", label: "GPT-5.4 mini", ref: "openai/gpt-5.4-mini" }];
const fresh = () => ({
  agents: [{
    id: "codex", name: "Codex", path: "/test/config.toml", icon: "codex-color", wired: true,
    fields: [
      { key: "model", label: "model", value: "gpt-5.5", options: models },
      { key: "subagent", label: "subagents", value: "", options: models },
      { key: "memories", label: "memories", value: "", options: models },
    ],
  },
  // nothing to pick: no square
  {
    id: "codex2", name: "Codex", path: "/test2/config.toml", icon: "codex-color", wired: false,
    fields: [{ key: "model", label: "model", value: "gpt-5.5", options: [{ value: "gpt-5.5" }] }, { key: "memories", label: "memories", value: "", options: [] }],
  }],
  profiles: [],
});

function server(lang, sets) {
  let cur = fresh();
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return route.fulfill({ json: { ...cur, settings: { lang, theme: "light" } } });
    if (url.pathname === "/api/set") {
      const body = req.postDataJSON();
      sets.push(body);
      cur = JSON.parse(JSON.stringify(cur));
      cur.agents.find((a) => a.id === body.agent).fields.find((f) => f.key === body.field).value = body.value;
      return route.fulfill({ json: { ...cur, settings: { lang, theme: "light" } } });
    }
    if (url.pathname === "/api/usage/quotas") return route.fulfill({ json: [] });
    if (url.pathname === "/api/groups") return route.fulfill({ json: { groups: [] } });
    if (url.pathname === "/api/agents/cli") return route.fulfill({ json: { agents: {}, pending: false } });
    if (url.pathname === "/api/providers") return route.fulfill({ json: { providers: [], gateway: { running: true } } });
    if (url.pathname === "/api/plugins") return route.fulfill({ json: { plugins: [] } });
    if (url.pathname.startsWith("/api/")) return route.fulfill({ json: {} });
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

// the words as i18n.js has them
const W = {
  en: { unset: "Codex memories model: Codex's own: gpt-5.6-terra, threads summed up with gpt-5.6-luna", def: "Default", set: "Codex memories model: GPT-5.4 mini" },
  zh: { unset: "Codex 记忆整理模型：Codex 自带的：整理用 gpt-5.6-terra，总结对话用 gpt-5.6-luna", def: "默认", set: "Codex 记忆整理模型：GPT-5.4 mini" },
};

const codex = '.row.agent[data-id="codex"]';

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh"]) {
    test(`${engine} ${lang}: the model Codex writes its memories with is a square opening the model picker`, async (t) => {
      const w = W[lang];
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      const page = await browser.newPage({ viewport: { width: 1100, height: 700 } });
      page.setDefaultTimeout(5000);
      const errors = [], sets = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/*", server(lang, sets));
      t.after(async () => {
        if (process.env.ARTIFACT_DIR) {
          await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
          await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `${engine}-${lang}-codex-memories.png`) });
        }
        await browser.close();
      });
      await page.goto("http://magpie.test/");
      await page.locator(codex).waitFor();
      await page.locator(`${codex} .ag-link`).click();
      await page.locator(`${codex} .ag-exp`).waitFor();

      assert.equal(await page.locator('.row.agent[data-id="codex2"] .field[data-key="memories"]').count(), 0, "a memories square with nothing to pick");
      const square = page.locator(`${codex} .extras-cell .field.extra[data-key="memories"]`);
      assert.equal(await square.count(), 1);
      assert.equal(await page.locator(`${codex} .field:not(.extra)[data-key="memories"]`).count(), 0, "memories is a picker of its own");
      assert.equal(await square.getAttribute("aria-label"), w.unset);
      assert.equal(await square.evaluate((e) => e.classList.contains("set")), false);

      // the app's model picker, Default first
      const y = await page.evaluate(() => scrollY);
      await square.click();
      await page.locator("#pop:not([hidden]) #list li").first().waitFor();
      assert.equal(await page.evaluate(() => scrollY), y, "the click scrolled the page");
      assert.equal(await page.locator("#pop").evaluate((e) => e.classList.contains("model-picker")), true);
      assert.equal(await page.locator("select").count(), 0, "a native select");
      assert.equal(await page.locator("#pop #list li").first().innerText().then((s) => s.includes(w.def)), true);
      const left = await page.evaluate(() => [...document.querySelectorAll("#pop *, .extras-cell *")].filter((e) => {
        const s = getComputedStyle(e);
        return parseFloat(s.borderLeftWidth) > 1 && s.borderLeftWidth !== s.borderRightWidth;
      }).length);
      assert.equal(left, 0, "a coloured left border");

      await page.locator("#pop #list li", { hasText: "GPT-5.4 mini" }).first().click();
      await page.waitForFunction(() => document.querySelector('.row.agent[data-id="codex"] .field.extra.set[data-key="memories"]'));
      assert.deepEqual(sets, [{ agent: "codex", field: "memories", value: "gpt-5.4-mini" }]);
      assert.equal(await square.getAttribute("aria-label"), w.set);

      // Default takes the keys out
      await square.click();
      await page.locator("#pop:not([hidden]) #list li").first().waitFor();
      await page.locator("#pop #list li").first().click();
      await page.waitForFunction(() => !document.querySelector('.row.agent[data-id="codex"] .field.extra.set[data-key="memories"]'));
      assert.deepEqual(sets.at(-1), { agent: "codex", field: "memories", value: "" });
      assert.equal(await page.evaluate(() => scrollY), y);
      assert.deepEqual(errors, []);
    });
  }
}
