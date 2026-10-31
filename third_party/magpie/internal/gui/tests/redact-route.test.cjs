// Run with Node's test runner and Playwright on the module path; see README.md.
// Settings › Privacy's "Instead of masking, send to" (jessyoungyu on X:
// 「{{百度}}这家公司怎么样」 can't be answered from a placeholder): a row
// while anything is masked, Off unless the user set it, so every request
// is masked as before. Its button opens the app menu (never a native
// select) with Off and only the models listed as able to take requests
// unmasked; a pick posts settings/redact-route and shows the model, and
// Off clears it. With none listed it says how to get one. In every
// language, Chromium and WebKit, in a wide window and a narrow one.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { reach, watchSettingsReads, settingsDrawn } = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");
const words = {
  en: { name: "Instead of masking, send to", off: "Off" },
  zh: { name: "命中脱敏时改用", off: "关闭" },
  "zh-TW": { name: "命中脫敏時改用", off: "關閉" },
  ja: { name: "マスクせずに送る先", off: "オフ" },
  de: { name: "Statt zu maskieren senden an", off: "Aus" },
};
const near = { id: "near/qwen", name: "qwen3", provider: "near", providerName: "Ollama" };

function serve(lang, posted, { redact = true, models = [near] } = {}) {
  let settings = { lang, theme: "light", redact, redactPersonal: false, redactWords: redact ? ["百度"] : [],
    titleModels: [{ id: "cloud/big", name: "big", provider: "cloud", providerName: "Cloud" }, ...models], maskRouteModels: models };
  const state = { agents: [], profiles: [], settings };
  return async (r) => {
    const url = new URL(r.request().url());
    const json = (data) => r.fulfill({ json: data });
    if (url.pathname === "/boot.js") return r.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return r.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json(state);
    if (url.pathname === "/api/settings/redact-route") {
      const { model } = JSON.parse(r.request().postData());
      posted.push(model);
      settings = { ...settings, redactRouteTo: model || undefined };
      return json(settings);
    }
    if (url.pathname === "/api/settings") {
      if (r.request().method() === "POST") settings = { ...settings, ...JSON.parse(r.request().postData()) };
      return json(settings);
    }
    if (url.pathname === "/api/groups") return json({ models: [], groups: [], pools: [] });
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true, url: "http://127.0.0.1:3999" } });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await r.fulfill({ body: await fs.readFile(file), contentType });
  };
}

async function open(engine, lang, width, posted, opts) {
  const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
  const page = await (await browser.newContext({ viewport: { width, height: 1400 } })).newPage();
  page.setDefaultTimeout(5000);
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.route("**/*", serve(lang, posted, opts));
  await watchSettingsReads(page);
  await page.goto("http://magpie.test/?view=settings&tab=privacy");
  await page.locator("#redactList .row").first().waitFor();
  await settingsDrawn(page);
  return { browser, page, errors };
}

const pop = "#pop:not([hidden]) #list li";

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const [lang, w] of Object.entries(words)) {
    for (const width of [1100, 640]) {
      test(`${engine} ${lang} ${width}px: a request masking would change goes to the model picked`, async (t) => {
        const posted = [];
        const { browser, page, errors } = await open(engine, lang, width, posted);
        t.after(() => browser.close());
        const row = page.locator("#redactRouteRow");
        await reach(page, row);
        const m = await row.evaluate((r) => {
          const sub = r.querySelector(".sub"), list = document.querySelector("#redactList").getBoundingClientRect(), b = r.getBoundingClientRect();
          return {
            name: r.querySelector(".name").textContent, sub: sub.textContent,
            button: r.querySelector("button.rt-cond")?.textContent.trim(),
            select: r.querySelectorAll("select").length,
            inside: b.left >= list.left - 1 && b.right <= list.right + 1,
            wide: document.documentElement.scrollWidth > document.documentElement.clientWidth,
          };
        });
        assert.equal(m.name, w.name);
        assert.equal(m.button, w.off, "off unless the user set it");
        assert.equal(m.select, 0, "no native select");
        assert(m.inside && !m.wide, "inside the page");
        if (lang !== "en") assert(!m.sub.startsWith("A request masking"), `its words translated: ${m.sub}`);

        await row.locator("button.rt-cond").click();
        await page.locator(pop).first().waitFor();
        const items = await page.locator(pop).allTextContents();
        assert(items[0].startsWith(w.off), `Off first: ${items}`);
        assert.equal(items.filter((x) => x.includes("qwen3")).length, 1, `the one model: ${items}`);
        assert(!items.some((x) => x.includes("big") || x.includes("Cloud")), `only a model that takes requests unmasked: ${items}`);
        await page.locator(pop).filter({ hasText: "qwen3" }).first().click();
        await page.waitForFunction(() => document.querySelector("#redactRouteRow button.rt-cond")?.textContent.includes("qwen3"));
        assert.deepEqual(posted, ["near/qwen"]);
        assert.match(await row.locator("button.rt-cond").textContent(), /qwen3 · Ollama/);

        await page.locator("#redactRouteRow button.rt-cond").click();
        await page.locator(pop).first().waitFor();
        await page.locator(pop).filter({ hasText: w.off }).first().click();
        await page.waitForFunction((off) => document.querySelector("#redactRouteRow button.rt-cond")?.textContent.trim() === off, w.off);
        assert.deepEqual(posted, ["near/qwen", ""]);
        assert.deepEqual(errors, []);
      });
    }
  }
  test(`${engine}: with no model to take it, the row says how to get one`, async (t) => {
    const { browser, page, errors } = await open(engine, "en", 1100, [], { models: [] });
    t.after(() => browser.close());
    const row = page.locator("#redactRouteRow");
    await reach(page, row);
    assert(await row.locator("button.rt-cond").isDisabled());
    assert.match(await row.locator(".sub").textContent(), /Send requests unmasked/);
    assert.deepEqual(errors, []);
  });
  test(`${engine}: no row while nothing is masked`, async (t) => {
    const { browser, page, errors } = await open(engine, "en", 1100, [], { redact: false });
    t.after(() => browser.close());
    assert.equal(await page.locator("#redactRouteRow").count(), 0);
    assert.deepEqual(errors, []);
  });
}
