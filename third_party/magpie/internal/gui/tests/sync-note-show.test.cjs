// Run with Node's test runner and Playwright on the module path; see README.md.
// A sync's notice (what it replaced here or on the server) has Show, which
// opens the sync folder in the file manager. When that fails, Show says why
// on the status line, in red, as any failed call does, and nothing throws.
// renderSync named its row's text status, a local that hid status(), so
// Show's catch called that string, threw, and the status line stayed empty
// (#4066). In English and Chinese, Chromium and WebKit. The API is faked;
// nothing is synced or opened.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { openPart } = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");
// Wails' words on a Linux with no xdg-open
const FAILED = 'failed to open the file explorer: exec: "xdg-open": executable file not found in $PATH';

function serve(lang, posts) {
  const settings = {
    theme: "light", lang, tray: "panel", version: "test", dir: "/tmp/magpie",
    gateway: "http://127.0.0.1:3425", visionModels: [], imageGenModels: [],
    fx: { rate: 7.2, stale: false },
  };
  const sync = { on: true, kind: "webdav", url: "https://dav.jianguoyun.com/dav/", user: "me", passwordSet: true, passphraseSet: true,
    keys: true, agents: true, library: true, auto: 3, last: new Date().toISOString(),
    notice: { at: new Date().toISOString(), here: ["providers"], saved: "/tmp/magpie/sync" } };
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
    if (pathname === "/api/davsync/reveal") {
      posts.push("reveal");
      return route.fulfill({ status: 400, json: { error: FAILED } });
    }
    if (pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, pathname === "/" ? "index.html" : pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    return route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

const show = { en: "Show", zh: "显示" };

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh"]) {
    test(`${engine} ${lang}: a sync notice's Show says why the folder didn't open`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      const page = await (await browser.newContext({ viewport: { width: 900, height: 700 }, reducedMotion: "reduce" })).newPage();
      const errors = [], posts = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/*", serve(lang, posts));
      t.after(async () => {
        if (process.env.ARTIFACT_DIR) {
          await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
          await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `${engine}-${lang}-sync-note-show.png`), fullPage: true });
        }
        await browser.close();
      });
      page.setDefaultTimeout(5000);

      await page.goto("http://magpie.test/");
      await page.locator("#prefs").click();
      await openPart(page, "sync");
      const note = page.locator("#syncList .sync-note");
      await note.waitFor({ state: "visible" });
      await note.getByRole("button", { name: show[lang], exact: true }).click();
      await page.waitForFunction(() => document.querySelector("#status").textContent !== "", null, { timeout: 3000 }).catch(() => {});
      assert.deepEqual(posts, ["reveal"], "Show asked for the sync folder");
      const status = page.locator("#status");
      assert.equal(await status.textContent(), FAILED, "the status line says why the folder didn't open");
      assert.ok(await status.evaluate((s) => s.classList.contains("err")), "in red");
      assert.equal(await status.getAttribute("role"), "alert");
      assert.deepEqual(errors, [], "nothing threw");
    });
  }
}
