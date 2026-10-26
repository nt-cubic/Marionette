// Settings › Observability's Langfuse setup: the user picks Langfuse and
// gives its host and the project's two keys, and magpie makes the endpoint,
// the Basic sign-in and the ingestion header from them. A Langfuse set up by
// hand keeps every value the user set; another collector's headers never go
// to Langfuse. Chromium and WebKit, every language, down to 360px.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { watchSettingsReads, settingsDrawn } = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");
const basic = (pk, sk) => "Basic " + Buffer.from(pk + ":" + sk).toString("base64");

function server(lang, otel, posts) {
  const fixed = {
    theme: "light", lang, tray: "panel", currency: "usd", redactWords: [], version: "0.1.400", dir: "~/.config/magpie",
    gateway: "http://127.0.0.1:3425", proxyNow: "none", proxySource: "none", visionModels: [], imageGenModels: [],
    workbuddyCheckins: [], lanURLs: [], fx: { rate: 7.2, at: new Date().toISOString(), stale: false }, otel,
  };
  let cur = fixed;
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" }, fx: fixed.fx });
    if (url.pathname === "/api/settings") {
      if (req.method() === "POST") {
        const body = req.postDataJSON();
        posts.push(body);
        cur = { ...fixed, ...body };
      }
      return json(cur);
    }
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

async function open(browser, lang, otel, width = 1100) {
  const posts = [], errors = [];
  const context = await browser.newContext({ viewport: { width, height: 1000 }, reducedMotion: "reduce" });
  const page = await context.newPage();
  page.on("pageerror", (e) => errors.push(e.message));
  await page.route("**/*", server(lang, otel, posts));
  await watchSettingsReads(page);
  await page.goto("http://magpie.test/?view=settings&tab=otel");
  await settingsDrawn(page);
  await page.locator("#otelKindRow").waitFor();
  // a save, waited for by its answer
  const saved = async (action) => {
    const n = posts.length + 1;
    const response = page.waitForResponse((r) => r.url().endsWith("/api/settings") && r.request().method() === "POST");
    await action(); await response;
    await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
    assert.equal(posts.length, n);
    return posts[n - 1].otel;
  };
  return { context, page, posts, errors, saved };
}

const keys = (page) => page.locator("#otelLangfuseKeysRow input");

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": Langfuse setup", async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());

    await t.test("a first setup needs only the two keys", async () => {
      const { context, page, posts, errors, saved } = await open(browser, "en", {});
      assert.equal(await page.locator("#otelKindRow .opt.on").textContent(), "OTLP");
      await page.locator("#otelKindRow .opt", { hasText: "Langfuse" }).click();
      await page.locator("#otelLangfuseHostRow").waitFor();
      assert.equal(await page.locator("#otelEndpointRow").count(), 0);
      assert.equal(await page.locator("#otelLangfuseHostRow .opt.on").textContent(), "Cloud EU");
      assert.equal(await keys(page).nth(1).getAttribute("type"), "password");
      // one key alone saves nothing
      await keys(page).first().fill("pk-lf-1");
      await keys(page).first().press("Tab");
      await page.waitForTimeout(150);
      assert.equal(posts.length, 0, "nothing saved before both keys are there");
      let o = await saved(async () => { await keys(page).nth(1).fill("sk-lf-2"); await keys(page).nth(1).press("Enter"); });
      assert.equal(o.endpoint, "https://cloud.langfuse.com/api/public/otel");
      assert.deepEqual(o.headers, { Authorization: basic("pk-lf-1", "sk-lf-2"), "x-langfuse-ingestion-version": "4" });
      assert.equal(o.metrics, false);
      assert.equal(o.enabled, true);
      assert.equal(await page.locator("#otelExportRow .opt.on").textContent(), "On");
      assert((await page.locator("#otelLangfuseKeysRow .sub").textContent()).includes("https://cloud.langfuse.com/api/public/otel"));
      // the US region, then a host of the user's own
      o = await saved(() => page.locator("#otelLangfuseHostRow .opt", { hasText: "Cloud US" }).click());
      assert.equal(o.endpoint, "https://us.cloud.langfuse.com/api/public/otel");
      assert.equal(o.headers.Authorization, basic("pk-lf-1", "sk-lf-2"));
      await page.locator("#otelLangfuseHostRow .opt", { hasText: "Self-hosted" }).click();
      const host = page.locator("#otelLangfuseHostRow input");
      await host.waitFor();
      o = await saved(async () => { await host.fill("langfuse.example.com/api/public/otel/"); await host.press("Enter"); });
      assert.equal(o.endpoint, "https://langfuse.example.com/api/public/otel");
      assert.equal(await page.locator("#otelLangfuseHostRow .opt.on").textContent(), "Self-hosted");
      assert.equal(await host.inputValue(), "https://langfuse.example.com");
      // a reload finds it all as Langfuse again
      await page.reload();
      await page.locator("#otelLangfuseKeysRow").waitFor();
      assert.equal(await keys(page).first().inputValue(), "pk-lf-1");
      assert.equal(await keys(page).nth(1).inputValue(), "sk-lf-2");
      assert.equal(await page.locator("#otelKindRow .opt.on").textContent(), "Langfuse");
      assert.deepEqual(errors, []);
      await context.close();
    });

    await t.test("a Langfuse set up by hand keeps its values", async () => {
      const hand = {
        enabled: false, metrics: true, endpoint: "https://cloud.langfuse.com/api/public/otel",
        headers: { Authorization: basic("pk-lf-a", "sk-lf-a"), "X-Team": "a,b" },
      };
      const { context, page, posts, errors, saved } = await open(browser, "en", hand);
      await page.locator("#otelLangfuseKeysRow").waitFor();
      assert.equal(await keys(page).first().inputValue(), "pk-lf-a");
      await page.waitForTimeout(150);
      assert.equal(posts.length, 0, "opening the page saves nothing");
      const o = await saved(async () => { await keys(page).nth(1).fill("sk-lf-b"); await keys(page).nth(1).press("Enter"); });
      assert.deepEqual(o.headers, { Authorization: basic("pk-lf-a", "sk-lf-b"), "X-Team": "a,b" }, "no header added or dropped");
      assert.equal(o.metrics, true, "metrics as the user set them");
      assert.equal(o.enabled, false, "export as the user set it");
      assert.deepEqual(errors, []);
      await context.close();
    });

    await t.test("another collector's headers stay with it", async () => {
      const { context, page, errors, saved } = await open(browser, "en", { enabled: true, endpoint: "http://localhost:4318", headers: { Authorization: "Bearer other", "X-Other": "1" } });
      await page.locator("#otelKindRow .opt", { hasText: "Langfuse" }).click();
      await keys(page).first().fill("pk-lf-1");
      const o = await saved(async () => { await keys(page).nth(1).fill("sk-lf-2"); await keys(page).nth(1).press("Enter"); });
      assert.deepEqual(o.headers, { Authorization: basic("pk-lf-1", "sk-lf-2"), "x-langfuse-ingestion-version": "4" });
      assert.equal(o.endpoint, "https://cloud.langfuse.com/api/public/otel");
      assert.deepEqual(errors, []);
      await context.close();
    });

    const names = {
      zh: ["采集服务", "Langfuse 地址", "API 密钥", "Cloud 欧洲"],
      "zh-TW": ["採集服務", "Langfuse 位址", "API 金鑰", "Cloud 歐洲"],
      ja: ["コレクター", "Langfuse のホスト", "API キー", "Cloud EU"],
      de: ["Collector", "Langfuse-Host", "API-Schlüssel", "Cloud EU"],
      en: ["Collector", "Langfuse host", "API keys", "Cloud EU"],
    };
    for (const [lang, want] of Object.entries(names)) {
      await t.test(lang + " at narrow widths", async () => {
        const { context, page, errors } = await open(browser, lang, { endpoint: "https://cloud.langfuse.com/api/public/otel", headers: { Authorization: basic("pk-lf-1", "sk-lf-2") } });
        await page.locator("#otelLangfuseKeysRow").waitFor();
        assert.deepEqual([
          await page.locator("#otelKindRow .name").textContent(),
          await page.locator("#otelLangfuseHostRow .name").textContent(),
          await page.locator("#otelLangfuseKeysRow .name").textContent(),
          await page.locator("#otelLangfuseHostRow .opt.on").textContent(),
        ], want);
        for (const width of [1100, 520, 360]) {
          await page.setViewportSize({ width, height: 1000 });
          await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
          const layout = await page.locator("#otelList").evaluate((list) => {
            const row = list.querySelector("#otelLangfuseKeysRow");
            return {
              fields: [...row.querySelectorAll("input")].map((f) => f.getBoundingClientRect().width),
              rowWidth: row.getBoundingClientRect().width,
              overflow: [...list.querySelectorAll(".row, .val, .segs")].filter((e) => e.scrollWidth > e.clientWidth + 1).map((e) => e.id || e.className),
              page: document.documentElement.scrollWidth > innerWidth + 1,
            };
          });
          for (const w of layout.fields) assert(w > layout.rowWidth - 40, `key fields span the row at ${width}px`);
          assert.deepEqual(layout.overflow, [], `nothing overflows at ${width}px`);
          assert.equal(layout.page, false, `no sideways scroll at ${width}px`);
        }
        if (process.env.ARTIFACT_DIR) await page.locator("#otelList").screenshot({ path: path.join(process.env.ARTIFACT_DIR, `langfuse-${engine}-${lang}.png`) });
        assert.deepEqual(errors, []);
        await context.close();
      });
    }
  });
}
