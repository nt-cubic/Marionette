// Run with Node's test runner and Playwright on the module path; see README.md.
// "Count its models as free" (jiabubu on Discord: ModelScope's free daily
// quota) is the user's word that a provider costs them nothing, so Free
// first spends it first. The editor's Free for me row, under Advanced
// beside Price rate, opens as it was saved (and opens Advanced when on),
// and Save posts it: for a provider of keys and for a signed-in account.
// In every language, in both engines, wide and narrow.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const base = {
  icon: "generic", chat: "", responses: "", anthropic: "", catalog: "",
  models: [{ id: "qwen3", name: "Qwen3", on: true }], agents: [], fallback: [], headers: {},
  key: { set: true, masked: "ms-…1234" }, keyList: [], balanceToken: { takes: false, set: false }, proxy: "", searches: false, unredacted: false,
};
const scope = { ...base, id: "modelscope", name: "ModelScope", preset: "modelscope", chat: "https://api-inference.modelscope.cn/v1" };
const counted = { ...base, id: "free-relay", name: "Free Relay", chat: "https://relay.example.com/v1", countFree: true };
const codex = {
  id: "codex", name: "Codex", icon: "codex-color", chat: "", responses: "", anthropic: "", catalog: "",
  models: [{ id: "gpt-6", name: "GPT-6", on: true }], agents: [], fallback: [], headers: {}, keyList: [],
  account: { agent: "codex", agentName: "Codex", user: "me@example.com", plan: "PLUS", logins: [{ user: "me@example.com", plan: "PLUS", active: true, on: true }] },
  proxy: "",
};

function serve(lang, posts) {
  const providers = { providers: [scope, counted, codex], presets: [], excluded: [], gateway: { running: true, window: true } };
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

const TICK = "Count its models as free";
const STRINGS = [
  "Free for me", TICK,
  "For a free quota, like a daily allowance: Free first uses its models before paid ones, and Cheapest first counts them as costing nothing. Prices and the costs Usage counts stay as listed.",
];

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
    for (const width of [900, 440]) {
      const open = async (t, name) => {
        const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
        let page;
        t.after(async () => {
          if (process.env.ARTIFACT_DIR && page) {
            await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
            await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `${engine}-${lang}-${width}-${name.replace(/\W+/g, "-")}-count-free.png`), fullPage: true });
          }
          await browser.close();
        });
        page = await (await browser.newContext({ viewport: { width, height: 760 }, reducedMotion: "reduce" })).newPage();
        page.setDefaultTimeout(5000);
        page.setDefaultNavigationTimeout(60000);
        const errors = [];
        page.on("pageerror", (e) => errors.push(e.message));
        const posts = [];
        await page.route("**/*", serve(lang, posts));
        await page.goto("http://magpie.test/?view=providers");
        await page.locator(".row.provider", { hasText: name }).click();
        await page.locator(".editor .bar").waitFor();
        const tick = await page.evaluate((k) => t(k), TICK);
        return { page, errors, posts, tick };
      };
      const adv = (page) => page.locator(".editor details.advanced");
      const unfold = async (page) => {
        await adv(page).waitFor({ state: "attached" });
        if (!(await adv(page).evaluate((d) => d.open))) await adv(page).locator(":scope > summary").click();
      };
      const box = (page, tick) => page.locator(".editor label.tick.count-free", { hasText: tick }).locator("input");
      const save = async (page, posts) => {
        const n = posts.length;
        const label = await page.evaluate(() => t("Save"));
        await page.locator(".editor .bar").getByRole("button", { name: label, exact: true }).click();
        for (let i = 0; i < 50 && posts.length === n; i++) await page.waitForTimeout(50);
        return posts.at(-1);
      };
      const fits = async (page, tick) => {
        const r = await page.locator(".editor label.tick.count-free").boundingBox();
        assert.ok(r && r.width > 60 && r.x >= 0 && r.x + r.width <= width, `the row fits: ${JSON.stringify(r)}`);
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, "no sideways scroll");
        assert.equal(await page.locator(".editor label.tick.count-free").textContent(), tick);
      };

      test(`${engine} ${lang} ${width}: ModelScope is counted free and saved so`, async (t) => {
        const { page, errors, posts, tick } = await open(t, "ModelScope");
        assert.equal(await adv(page).evaluate((d) => d.open), false, "nothing set opens Advanced");
        await unfold(page);
        assert.equal(await box(page, tick).isChecked(), false, "opens off");
        await box(page, tick).scrollIntoViewIfNeeded();
        await fits(page, tick);
        await box(page, tick).check();
        const saved = await save(page, posts);
        assert.equal(saved.action, "save");
        assert.equal(saved.body.id, "modelscope");
        assert.equal(saved.body.countFree, true);
        if (lang !== "en") {
          const missing = await page.evaluate(([l, ks]) => ks.filter((k) => !I18N[l][k]), [lang, STRINGS]);
          assert.deepEqual(missing, [], `every string has its ${lang}`);
        }
        assert.deepEqual(errors, []);
      });

      test(`${engine} ${lang} ${width}: one saved counted free opens Advanced ticked, and is saved off`, async (t) => {
        const { page, errors, posts, tick } = await open(t, "Free Relay");
        assert.equal(await adv(page).evaluate((d) => d.open), true, "set, it opens Advanced");
        assert.equal(await box(page, tick).isChecked(), true);
        await box(page, tick).scrollIntoViewIfNeeded();
        await box(page, tick).uncheck();
        const saved = await save(page, posts);
        assert.equal(saved.body.id, "free-relay");
        assert.equal(saved.body.countFree, false);
        assert.deepEqual(errors, []);
      });

      test(`${engine} ${lang} ${width}: a signed-in account is counted free too`, async (t) => {
        const { page, errors, posts, tick } = await open(t, "Codex");
        await unfold(page);
        assert.equal(await box(page, tick).isChecked(), false);
        await box(page, tick).scrollIntoViewIfNeeded();
        await fits(page, tick);
        await box(page, tick).check();
        const saved = await save(page, posts);
        assert.equal(saved.action, "save");
        assert.equal(saved.body.id, "codex");
        assert.equal(saved.body.countFree, true);
        assert.deepEqual(errors, []);
      });
    }
  }
}
