// Run with Node's test runner and Playwright on the module path; see README.md.
// An agent's field set back on a computer after a sync changed it stays that
// computer's (hayden4608 on Discord: two computers on one GitHub sync, each
// wanting its own Codex model). Settings → Sync names the fields kept here,
// in the page's language, and Follow the others gives them back to sync:
// it asks the server once, and the row goes with the fields. A sync with no
// such fields, or with agents' models off, shows no row. In every language,
// Chromium and WebKit, at a narrow width with nothing wider than the page.
// The API is faked; nothing is synced.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { openPart } = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");

function serve(lang, sync, posts) {
  const settings = {
    theme: "light", lang, tray: "panel", version: "test", dir: "/tmp/magpie",
    gateway: "http://127.0.0.1:3425", visionModels: [], imageGenModels: [],
    fx: { rate: 7.2, stale: false },
  };
  return async (route) => {
    const request = route.request();
    const pathname = new URL(request.url()).pathname;
    const json = (data) => route.fulfill({ json: data });
    if (pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (pathname === "/api/state") return json({ agents: [], profiles: [], settings });
    if (pathname === "/api/settings") return json(settings);
    if (pathname === "/api/usage/quotas") return json([]);
    if (pathname === "/api/groups") return json({ groups: [], models: [] });
    if (pathname === "/api/plugins") return json({ plugins: [] });
    if (pathname === "/api/davsync") return json(sync);
    if (pathname === "/api/davsync/follow") {
      posts.push("follow");
      const { own, ...rest } = sync;
      return json(rest);
    }
    if (pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, pathname === "/" ? "index.html" : pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    return route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

const words = {
  en: { title: "Kept on this computer", follow: "Follow the others", field: "Codex model" },
  zh: { title: "仅本机保留", follow: "跟随其他电脑", field: "Codex 模型" },
  "zh-TW": { title: "僅本機保留", follow: "跟隨其他電腦", field: "Codex 模型" },
  ja: { title: "このコンピュータだけで保持", follow: "ほかのコンピュータに合わせる", field: "Codex のモデル" },
  de: { title: "Nur auf diesem Computer", follow: "Den anderen folgen", field: "Codex: Modell" },
};

const base = () => ({ on: true, kind: "github", url: "github://alice/private/team", passwordSet: true, passphraseSet: true,
  keys: true, agents: true, library: true, auto: 3, last: new Date().toISOString() });

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  const open = async (t, lang, sync, posts, width = 900) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    const page = await (await browser.newContext({ viewport: { width, height: 700 }, reducedMotion: "reduce" })).newPage();
    const errors = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.route("**/*", serve(lang, sync, posts));
    t.after(async () => {
      if (process.env.ARTIFACT_DIR) {
        await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
        await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `${engine}-${lang}-${width}-sync-own-fields.png`), fullPage: true });
      }
      await browser.close();
    });
    page.setDefaultTimeout(5000);
    await page.goto("http://magpie.test/");
    await page.locator("#prefs").click();
    await openPart(page, "sync");
    await page.locator("#syncList .row").first().waitFor({ state: "visible" });
    return { page, errors };
  };

  for (const lang of Object.keys(words)) {
    const w = words[lang];
    test(`${engine} ${lang}: the fields kept on this computer are named, and follow the others on a click`, async (t) => {
      const posts = [];
      const sync = { ...base(), own: [{ agent: "Codex", field: "model" }] };
      const { page, errors } = await open(t, lang, sync, posts, 420);
      const row = page.locator("#syncList .row", { has: page.locator(".sync-own") });
      await row.waitFor({ state: "visible" });
      assert.equal((await row.locator(".name").textContent()).trim(), w.title);
      assert.ok((await row.locator(".sync-own").textContent()).includes(w.field), `names ${w.field}`);
      const over = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
      assert.ok(over <= 0, `nothing wider than the page (${over}px over)`);
      const y = await page.evaluate(() => document.scrollingElement.scrollTop);
      await row.getByRole("button", { name: w.follow, exact: true }).click();
      await row.waitFor({ state: "detached" });
      assert.deepEqual(posts, ["follow"], "asked the server once");
      assert.equal(await page.evaluate(() => document.scrollingElement.scrollTop), y, "the click moved nothing");
      assert.deepEqual(errors, [], "nothing threw");
    });
  }

  for (const [name, sync] of [["no fields kept", base()], ["agents' models off", { ...base(), agents: false, own: [{ agent: "Codex", field: "model" }] }]]) {
    test(`${engine}: ${name} shows no row for them`, async (t) => {
      const { page, errors } = await open(t, "en", sync, []);
      assert.equal(await page.locator("#syncList .sync-own").count(), 0);
      assert.equal(await page.getByText("Kept on this computer").count(), 0);
      assert.deepEqual(errors, [], "nothing threw");
    });
  }
}
