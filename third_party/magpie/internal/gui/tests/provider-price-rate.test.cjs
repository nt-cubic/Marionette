// Run with Node's test runner and Playwright on the module path; see README.md.
// A provider's price rate (ITea312, #819): its editor has the field, empty
// for the official price; Save posts the number typed, null when it is
// empty, and refuses one out of range or with more than three decimals
// before anything is posted. A signed-in account opens on what it has. In
// English and Chinese.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
// the editor folds what few need under Advanced, this field among them:
// opened as a reader would, unless something set in it opened it already
const unfold = async (page) => {
  const adv = page.locator(".editor details.advanced");
  await adv.waitFor({ state: "attached" });
  if (!(await adv.evaluate((d) => d.open))) await adv.locator(":scope > summary").click();
};
const relay = {
  id: "relay", name: "Relay", icon: "generic", chat: "https://relay.example.com/v1", responses: "", anthropic: "", catalog: "",
  models: [{ id: "model-a", name: "Model A", on: true }], agents: [], fallback: [], headers: {},
  key: { set: true, masked: "sk-…one" }, keyList: [], balanceToken: { takes: false, set: false }, proxy: "", maxConcurrency: null,
};
const codex = {
  id: "codex", name: "Codex", icon: "codex-color", chat: "", responses: "", anthropic: "", catalog: "",
  models: [{ id: "gpt-6", name: "GPT-6", on: true }], agents: [], fallback: [], headers: {}, keyList: [],
  account: { agent: "codex", agentName: "Codex", user: "me@example.com", plan: "PLUS", logins: [{ user: "me@example.com", plan: "PLUS", active: true, on: true }] },
  proxy: "", maxConcurrency: null, priceRate: 0.8,
};

function serve(lang, posts) {
  const providers = { providers: [relay, codex], presets: [], excluded: [], gateway: { running: true, window: true } };
  const state = { agents: [], profiles: [], settings: { lang, theme: "light" } };
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json(state);
    if (url.pathname === "/api/providers") return json(providers);
    if (url.pathname === "/api/groups") return json({ groups: [] });
    if (url.pathname.startsWith("/api/provider/") && route.request().method() === "POST") {
      posts.push({ action: url.pathname.slice("/api/provider/".length), body: route.request().postDataJSON() });
      return json(providers);
    }
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

const words = {
  en: { label: "Price rate", save: "Save", none: "1, the official price", bad: "Price rate: a number from 0 to 1000, at most three decimals" },
  zh: { label: "价格倍率", save: "保存" },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh"]) {
    const w = words[lang];
    const open = async (t, name) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const page = await (await browser.newContext({ viewport: { width: 900, height: 760 }, reducedMotion: "reduce" })).newPage();
      page.setDefaultTimeout(5000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      const posts = [];
      await page.route("**/*", serve(lang, posts));
      await page.goto("http://magpie.test/?view=providers");
      await page.locator(".row.provider", { hasText: name }).click();
      await unfold(page);
      await page.locator(".editor input.price-rate").waitFor();
      await page.waitForTimeout(300);
      return { page, errors, posts };
    };
    const box = (page) => page.locator(".editor input.price-rate");
    const save = async (page, posts) => {
      const n = posts.length;
      await page.locator(".editor .bar").getByRole("button", { name: w.save, exact: true }).click();
      for (let i = 0; i < 50 && posts.length === n; i++) await page.waitForTimeout(50);
      return posts.at(-1);
    };

    test(`${engine} ${lang}: a provider's price rate is typed and saved`, async (t) => {
      const { page, errors, posts } = await open(t, "Relay");
      const zh = await page.evaluate(() => ({
        none: I18N.zh["1, the official price"], bad: I18N.zh["Price rate: a number from 0 to 1000, at most three decimals"],
        hint: I18N.zh["Costs are counted at the official price times this, as a relay bills, like 0.8 or 1.5; a price you set for a model stays as set"],
        label: I18N.zh["Price rate"],
      }));
      assert(zh.none && zh.bad && zh.hint && zh.label, "every string has its Chinese");
      if (lang === "zh") Object.assign(w, { none: zh.none, bad: zh.bad });
      assert.equal(await page.locator(".editor label", { hasText: w.label }).count(), 1);
      assert.equal(await box(page).inputValue(), "", "none set");
      assert.equal(await box(page).getAttribute("placeholder"), w.none);

      for (const bad of ["0.1234", "1001", "-1"]) {
        await box(page).fill(bad);
        await page.locator(".editor .bar").getByRole("button", { name: w.save, exact: true }).click();
        await page.locator(".editor .editor-error").waitFor();
        assert.equal(await page.locator(".editor .editor-error").textContent(), w.bad, bad);
        assert.equal(posts.length, 0, `${bad} is not posted`);
      }

      await box(page).fill("0.85");
      let saved = await save(page, posts);
      assert.equal(saved.action, "save");
      assert.equal(saved.body.id, "relay");
      assert.equal(saved.body.priceRate, 0.85);

      await page.locator(".row.provider", { hasText: "Relay" }).click();
      await unfold(page);
      await box(page).fill("");
      saved = await save(page, posts);
      assert(Object.hasOwn(saved.body, "priceRate"), "an empty field is saved as none, not left out");
      assert.equal(saved.body.priceRate, null);
      assert.deepEqual(errors, []);
    });

    test(`${engine} ${lang}: a signed-in account's rate opens as it is and is saved`, async (t) => {
      const { page, errors, posts } = await open(t, "Codex");
      assert.equal(await box(page).inputValue(), "0.8");
      // a comma for the decimal point, as some keyboards type it
      await box(page).fill("1,5");
      const saved = await save(page, posts);
      assert.equal(saved.body.id, "codex");
      assert.equal(saved.body.priceRate, 1.5);
      assert.deepEqual(saved.body.models, ["gpt-6"], "its picks go with it");
      assert.deepEqual(errors, []);
    });
  }
}
