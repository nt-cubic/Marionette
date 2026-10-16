// Run with Node's test runner and Playwright on the module path; see README.md.
// Settings' Web search section, a search API with several keys
// (SlouchyQuill on Discord, after dsh-tavily-pool): under it a row of its
// keys, In turn (the default) or In order, sent as pick "turns" or "order";
// for Tavily, what each key has used of its limit, read once as the page
// is drawn and again, fresh, on Check again. Each key is shown masked; a
// read that failed says Unknown and why, never 0; a key used up says so,
// and one asked last says until when and why. An API with one key that
// magpie can't read gets no row. In five languages, Chromium and WebKit,
// 1100px and 420px wide, with the API faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { watchSettingsReads, settingsDrawn, reach } = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");
const LANGS = ["en", "zh", "zh-TW", "ja", "de"];
// the words as i18n.js has them
const I18N = (() => {
  const src = require("node:fs").readFileSync(path.join(assets, "i18n.js"), "utf8");
  return new Function(src.slice(0, src.indexOf("\n};") + 3) + "; return I18N;")();
})();
const tr = (lang, s, vars = {}) => {
  const got = lang === "en" ? s : I18N[lang][s];
  assert(got, `"${s}" has no ${lang} translation`);
  return got.replace(/\{(\w+)\}/g, (_, k) => String(vars[k]));
};

const vendors = [
  { id: "tavily", name: "Tavily", keysURL: "https://app.tavily.com/home", base: "https://api.tavily.com" },
  { id: "brave", name: "Brave Search", keysURL: "https://api-dashboard.search.brave.com/app/keys", base: "https://api.search.brave.com" },
];
const restUntil = new Date(Date.now() + 50 * 60 * 1000).toISOString();
const quotas = (used) => [
  { key: "tvly-…aaaa", known: true, used, limit: 1000, of: "key" },
  { key: "tvly-…bbbb", known: true, used: 1000, limit: 1000, of: "plan", plan: "Researcher", usedUp: true, resting: "used up", restUntil },
  { key: "tvly-…cccc", known: false, used: 0, limit: 0, err: "401 Unauthorized" },
];

function serve(lang, posted) {
  let pick = "";
  let reads = 0;
  const settings = () => ({
    lang, theme: "light", searchVendors: vendors,
    searchAPIs: [
      { vendor: "tavily", name: "Tavily", key: "tvly-…aaaa, tvly-…bbbb, tvly-…cccc", keys: 3, quota: true, pick, ready: true },
      { vendor: "brave", name: "Brave Search", key: "BSA…wxyz", keys: 1, ready: true },
    ],
  });
  return async (r) => {
    const url = new URL(r.request().url());
    const json = (data, status = 200) => r.fulfill({ status, json: data });
    if (url.pathname === "/boot.js") return r.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return r.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: settings() });
    if (url.pathname === "/api/settings") return json(settings());
    if (url.pathname === "/api/settings/search-api") {
      const b = JSON.parse(r.request().postData());
      posted.push(b);
      if (b.pick) pick = b.pick === "turns" ? "" : b.pick;
      return json(settings());
    }
    if (url.pathname === "/api/settings/search-quota") {
      const b = JSON.parse(r.request().postData());
      posted.push({ quota: b });
      reads++;
      return json({ keys: quotas(reads > 1 ? 813 : 812) });
    }
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/groups") return json({ models: [], groups: [], pools: [] });
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true, url: "http://127.0.0.1:3999" } });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await r.fulfill({ body: await fs.readFile(file), contentType });
  };
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of LANGS) for (const width of [1100, 420]) {
    test(`${engine} ${lang} ${width}px: a search API's keys take turns and say what each has used`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const page = await (await browser.newContext({ viewport: { width, height: 1400 }, reducedMotion: "reduce" })).newPage();
      page.setDefaultTimeout(5000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      const posted = [];
      await page.route("**/*", serve(lang, posted));
      await watchSettingsReads(page);
      await page.goto("http://magpie.test/?view=settings&tab=models");
      await settingsDrawn(page);
      const keys = page.locator("#searchList .row.search-keys");
      await keys.first().waitFor();
      // Tavily's only: Brave has one key and no quota magpie can read
      assert.equal(await keys.count(), 1);
      assert.equal(await keys.getAttribute("data-vendor"), "tavily");
      assert.equal(await keys.locator(".name").innerText(), tr(lang, "Keys of {name}", { name: "Tavily" }));

      const lines = keys.locator(".search-quota .quota-key");
      await page.waitForFunction(() => document.querySelectorAll("#searchList .search-keys .quota-key").length === 3);
      assert.deepEqual(await lines.locator("code").allInnerTexts(), ["tvly-…aaaa", "tvly-…bbbb", "tvly-…cccc"]);
      assert.equal(await lines.nth(0).locator(".used").innerText(), tr(lang, "{used} of {limit} used", { used: "812", limit: await page.evaluate(() => (1000).toLocaleString(intlLang())) }));
      // used up: said, with the plan it is of and why it is asked last
      assert.equal(await lines.nth(1).locator(".warn").last().innerText(), tr(lang, "used up"));
      const note = await lines.nth(1).locator(".faint").innerText();
      assert(note.includes(tr(lang, "{plan} plan, shared by its account's keys", { plan: "Researcher" })), note);
      const until = await page.evaluate((u) => new Date(u).toLocaleTimeString(intlLang(), { hour: "2-digit", minute: "2-digit" }), restUntil);
      assert(note.includes(tr(lang, "asked last until {time}: {why}", { time: until, why: tr(lang, "used up") })), note);
      // a failed read: unknown and why, never 0
      const unknown = await lines.nth(2).locator(".used").innerText();
      assert.equal(unknown, tr(lang, "Unknown: {why}", { why: "401 Unauthorized" }));
      assert(!/(^|\D)0(\D|$)/.test(unknown.replace("401", "")), unknown);
      assert.equal(await lines.nth(2).locator(".used.err").count(), 1);
      assert.deepEqual(posted, [{ quota: { vendor: "tavily", fresh: false } }]);
      assert(await page.evaluate(() => document.scrollingElement.scrollWidth <= innerWidth), "nothing wider than the window");
      // the page clips what is past its edge, so each control is measured
      const past = await keys.evaluate((r) => [...r.querySelectorAll("button, code, .used")]
        .filter((e) => e.getBoundingClientRect().right > innerWidth + 0.5).map((e) => e.textContent));
      assert.deepEqual(past, [], "nothing past the window's edge");

      const where = () => page.evaluate(() => [scrollX, scrollY, document.scrollingElement.scrollTop, document.querySelector("#view-settings")?.scrollTop]);
      // Settings is drawn again after each change: a box is taken from the
      // row that stays
      const click = async (loc) => {
        let b = null;
        for (let i = 0; i < 20 && !b; i++) b = await loc.boundingBox().catch(() => null) || (await page.waitForTimeout(50), null);
        assert(b, "the button is in the page");
        await page.mouse.click(b.x + b.width / 2, b.y + b.height / 2);
      };
      // the row's foot is past the window's at 420px: the reader wheels down
      // to it before the clicks, which then must not move the page
      await reach(page, keys.locator("button.search-quota-again"));
      const before = await where();
      // Check again: read fresh
      await click(keys.locator("button.search-quota-again", { hasText: tr(lang, "Check again") }));
      await page.waitForFunction(() => /813/.test(document.querySelector("#searchList .search-keys .quota-key .used")?.innerText || ""));
      assert.deepEqual(posted.at(-1), { quota: { vendor: "tavily", fresh: true } });
      // In order, then In turn again
      const segs = keys.locator(".segs .opt");
      assert.deepEqual(await segs.allInnerTexts(), [tr(lang, "In turn"), tr(lang, "In order")]);
      assert.equal(await segs.nth(0).getAttribute("aria-pressed"), "true");
      await click(segs.nth(1));
      await page.waitForFunction(() => document.querySelectorAll("#searchList .search-keys .segs .opt")[1]?.getAttribute("aria-pressed") === "true");
      assert.deepEqual(posted.filter((p) => p.pick), [{ vendor: "tavily", pick: "order" }]);
      await click(keys.locator(".segs .opt").nth(0));
      await page.waitForFunction(() => document.querySelectorAll("#searchList .search-keys .segs .opt")[0]?.getAttribute("aria-pressed") === "true");
      assert.deepEqual(posted.filter((p) => p.pick), [{ vendor: "tavily", pick: "order" }, { vendor: "tavily", pick: "turns" }]);
      // redrawn with the same keys, it isn't read again
      assert.equal(posted.filter((p) => p.quota).length, 2);
      assert.equal(await keys.locator(".quota-key").count(), 3);
      assert.deepEqual(await where(), before, "a click doesn't scroll");
      assert(await page.evaluate(() => document.scrollingElement.scrollWidth <= innerWidth), "nothing wider than the window");
      assert.deepEqual(errors, []);
    });
  }
}
