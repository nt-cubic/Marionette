// Run with Node's test runner and Playwright on the module path; see README.md.
// A provider's rate windows (#2463): the editor's Advanced has the field,
// empty for none; Add a window gives a row of days, a span and a rate;
// Save posts the windows typed, null when none are left, and refuses a
// window with a bad time or rate before anything is posted. A provider
// saved with windows opens on them, its row inside the editor at a
// phone's width too. A row's ×rate is the server's rateNow, never worked
// out on the browser's clock. In every language, wide and narrow.
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
const windows = [{ days: [1, 2, 3, 4, 5], start: "09:00", end: "18:00", rate: 3 }];
const relay = {
  id: "relay", name: "Relay", icon: "generic", chat: "https://relay.example.com/v1", responses: "", anthropic: "", catalog: "",
  models: [{ id: "model-a", name: "Model A", on: true }], agents: [], fallback: [], headers: {},
  key: { set: true, masked: "sk-…one" }, keyList: [], balanceToken: { takes: false, set: false }, proxy: "", maxConcurrency: null,
};
const codex = {
  id: "codex", name: "Codex", icon: "codex-color", chat: "", responses: "", anthropic: "", catalog: "",
  models: [{ id: "gpt-6", name: "GPT-6", on: true }], agents: [], fallback: [], headers: {}, keyList: [],
  account: { agent: "codex", agentName: "Codex", user: "me@example.com", plan: "PLUS", logins: [{ user: "me@example.com", plan: "PLUS", active: true, on: true }] },
  proxy: "", maxConcurrency: null, rateWindows: windows,
};

function serve(lang, posts, list = [relay, codex]) {
  const providers = { providers: list, presets: [], excluded: [], gateway: { running: true, window: true } };
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
  en: { label: "Rate windows", save: "Save", add: "Add a window" },
  zh: { label: "时段倍率", save: "保存", add: "添加时段" },
  "zh-TW": { label: "時段倍率", save: "儲存", add: "新增時段" },
  ja: { label: "時間帯倍率", save: "保存", add: "時間帯を追加" },
  de: { label: "Zeitfenster", save: "Speichern", add: "Zeitfenster hinzufügen" },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of Object.keys(words)) for (const width of [900, 420]) {
    const w = words[lang];
    const launch = async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      return (await browser.newContext({ viewport: { width, height: 900 }, reducedMotion: "reduce" })).newPage();
    };
    const open = async (t, name) => {
      const page = await launch(t);
      page.setDefaultTimeout(5000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      const posts = [];
      await page.route("**/*", serve(lang, posts));
      await page.goto("http://magpie.test/?view=providers");
      await page.locator(".row.provider", { hasText: name }).click();
      await unfold(page);
      // ratewindows.js is imported as the field is drawn: its hint is there
      // once the rows can be
      await page.locator(".editor .rate-windows + .hint").waitFor();
      await page.waitForTimeout(300);
      return { page, errors, posts };
    };
    const save = async (page, posts) => {
      const n = posts.length;
      await page.locator(".editor .bar").getByRole("button", { name: w.save, exact: true }).click();
      for (let i = 0; i < 50 && posts.length === n; i++) await page.waitForTimeout(50);
      return posts.at(-1);
    };

    test(`${engine} ${lang} ${width}: rate windows are added, saved and refused when wrong`, async (t) => {
      const { page, errors, posts } = await open(t, "Relay");
      const zh = await page.evaluate(() => ({
        label: I18N.zh["Rate windows"], add: I18N.zh["Add a window"],
        hint: I18N.zh["Inside a window a call costs the official price times its rate, on top of the price rate; outside every window the price rate alone counts. The first window that holds a call wins. Times are on the clock of the computer magpie runs on, and 22:00–6:00 crosses midnight. No day ticked is every day. A price you set for a model is scaled by the window too. Changing the windows prices the calls already counted again, as a changed price rate does."],
        bad: I18N.zh["Rate windows: every window is days, a time like 09:00, another time for it to end, and a rate from 0 to 1000 with at most three decimals"],
      }));
      assert(zh.label && zh.add && zh.hint && zh.bad, "every string has its Chinese");
      // the vendors whose list price has its peak already are named under
      // the windows, in the page's own language
      const peak = "DeepSeek's own API, Command Code and OpenCode Go already count their DeepSeek models' peak hours at double: don't add those hours as a window, or they count twice.";
      const peakText = await page.evaluate(([lang, k]) => lang === "en" ? k : I18N[lang]?.[k], [lang, peak]);
      assert(peakText, "the peak note has its " + lang);
      assert((await page.locator(".editor .rate-windows + .hint").innerText()).includes(peakText.trim()), "the peak note is shown");
      assert.equal(await page.locator(".editor .rate-win").count(), 0, "none set");

      // a window is added, its days ticked, its span and rate typed
      await page.locator(".editor .rate-windows").getByRole("button", { name: w.add }).click();
      const row = page.locator(".editor .rate-win");
      assert.equal(await row.count(), 1);
      await row.locator(".day-chip").nth(1).click();
      await row.locator(".day-chip").nth(3).click();
      await row.locator("input.rate-rate").fill("3");

      // a window with no times picked is refused before anything is posted
      await row.locator("input.rate-rate").fill("3");
      await page.locator(".editor .bar").getByRole("button", { name: w.save, exact: true }).click();
      await page.locator(".editor .editor-error").waitFor();
      assert.equal(posts.length, 0, "no time picked is not posted");
      await row.locator("button.rate-time").nth(0).click();
      await page.getByRole("menuitemradio", { name: "09:00" }).click();
      await row.locator("button.rate-time").nth(1).click();
      await page.getByRole("menuitemradio", { name: "18:00" }).click();
      await row.locator("input.rate-rate").fill("3000");
      await page.locator(".editor .bar").getByRole("button", { name: w.save, exact: true }).click();
      await page.locator(".editor .editor-error").waitFor();
      assert.equal(posts.length, 0, "a rate over 1000 is not posted");
      await row.locator("input.rate-rate").fill("3");

      const saved = await save(page, posts);
      assert.equal(saved.action, "save");
      assert.equal(saved.body.id, "relay");
      assert.deepEqual(saved.body.rateWindows, [{ days: [1, 3], start: "09:00", end: "18:00", rate: 3 }]);
      assert.deepEqual(errors, []);
    });

    test(`${engine} ${lang} ${width}: a provider saved with windows opens on them`, async (t) => {
      const { page, errors, posts } = await open(t, "Codex");
      const row = page.locator(".editor .rate-win");
      assert.equal(await row.count(), 1);
      // the row wraps inside the editor rather than running past it
      const over = await row.evaluate((r) => {
        const box = r.closest(".editor").getBoundingClientRect();
        return [...r.querySelectorAll("*")].filter((c) => c.getBoundingClientRect().right > box.right + 1).map((c) => c.className);
      });
      assert.deepEqual(over, [], "nothing of the row past the editor's edge");
      assert.equal(await row.locator(".day-chip.on").count(), 5, "the five days ticked");
      assert.equal(await row.locator("button.rate-time").nth(0).textContent(), "09:00");
      assert.equal(await row.locator("button.rate-time").nth(1).textContent(), "18:00");
      assert.equal(await row.locator("input.rate-rate").inputValue(), "3");
      const saved = await save(page, posts);
      assert.deepEqual(saved.body.rateWindows, windows, "what it opens with is what it saves");

      // none left is saved as none, not left out: the saved editor closed,
      // the row opens again on what it has
      await page.locator(".row.provider", { hasText: "Codex" }).click();
      await unfold(page);
      await page.locator(".editor .rate-win").waitFor();
      await page.locator(".editor .rate-win .rate-win-del").click();
      const cleared = await save(page, posts);
      assert(Object.hasOwn(cleared.body, "rateWindows"), "no window is saved as none, not left out");
      assert.equal(cleared.body.rateWindows, null);
      assert.deepEqual(errors, []);
    });

    test(`${engine} ${lang} ${width}: a row's rate is the server's, not the browser's clock`, async (t) => {
      // the server says Relay bills at 6× now; Codex has a window over
      // every hour of the browser's day but the server says 1× (its own
      // clock is elsewhere), so it shows none
      const page = await launch(t);
      page.setDefaultTimeout(5000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      const allDay = [{ days: [], start: "00:00", end: "24:00", rate: 3 }];
      await page.route("**/*", serve(lang, [], [{ ...relay, rateNow: 6 }, { ...codex, rateWindows: allDay }]));
      await page.goto("http://magpie.test/?view=providers");
      await page.locator(".row.provider", { hasText: "Relay" }).waitFor();
      assert.equal(await page.locator(".row.provider", { hasText: "Relay" }).locator(".badge.rate-now").textContent(), "×6");
      assert.equal(await page.locator(".row.provider", { hasText: "Codex" }).locator(".badge.rate-now").count(), 0);
      assert.deepEqual(errors, []);
    });
  }
}
