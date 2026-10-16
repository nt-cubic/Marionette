// Run with Node's test runner and Playwright on the module path; see README.md.
// Sync can run by itself less often, or only when asked, and the server's
// setup can be restored over this computer's, undoably (Sun1090, #847: free
// 坚果云 accounts are limited in requests, and there was no way back). With
// WebDAV sync on, the Sync by itself row's control (no <select>) shows every
// 3 minutes; picking Off posts minutes -1, the row then says only Sync now
// syncs and the sync row says only when asked. Restore from the server asks
// first in the app's own dialog, naming what is replaced and that it is kept
// to undo; Cancel posts nothing; Restore posts it, and the notice it leaves
// has Undo, which posts undo. No click moves the page. In English and
// Chinese, Chromium and WebKit. The API is faked; nothing is synced.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { openPart } = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");

function serve(lang, posts) {
  const settings = {
    theme: "light", lang, tray: "panel", version: "test", dir: "/tmp/magpie",
    gateway: "http://127.0.0.1:3425", visionModels: [], imageGenModels: [],
    fx: { rate: 7.2, stale: false },
  };
  let sync = { on: true, kind: "webdav", url: "https://dav.jianguoyun.com/dav/", user: "me", passwordSet: true, passphraseSet: true,
    keys: true, agents: true, library: true, auto: 3, last: new Date().toISOString() };
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
    if (pathname.startsWith("/api/davsync/")) {
      const action = pathname.slice("/api/davsync/".length);
      posts.push({ action, body: request.postDataJSON() });
      if (action === "auto") sync = { ...sync, auto: Math.max(0, request.postDataJSON().minutes) };
      if (action === "restore") {
        sync = { ...sync, undo: true, notice: { at: new Date().toISOString(), here: ["providers", "settings"], saved: "/tmp/magpie/sync", restored: true } };
        return json({ ...sync, brought: ["providers", "settings"] });
      }
      if (action === "undo") sync = { ...sync, undo: false, notice: undefined };
      return json(sync);
    }
    if (pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, pathname === "/" ? "index.html" : pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    return route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

const words = {
  en: {
    auto: "Sync by itself", every3: "Every 3 minutes while magpie runs", off: "Off", offSub: "Off: only Sync now syncs",
    asked: " · only when asked", restoreRow: "Restore from the server", restore: "Restore…", ask: "Restore from the server?",
    kept: "What is here now is sealed into the sync folder first, and Undo puts it back.", go: "Restore setup", cancel: "Cancel",
    restored: "Restored from the server: providers, settings", undo: "Undo",
  },
  zh: {
    auto: "自动同步", every3: "magpie 运行时每 3 分钟同步一次", off: "关闭", offSub: "已关闭，仅在点「立即同步」时同步",
    asked: " · 仅手动", restoreRow: "从服务器恢复", restore: "恢复…", ask: "从服务器恢复？",
    kept: "当前配置会先加密备份到 sync 文件夹，可点「撤销」换回。", go: "恢复配置", cancel: "取消",
    restored: "已从服务器恢复：", undo: "撤销",
  },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh"]) {
    const w = words[lang];
    test(`${engine} ${lang}: sync by itself can be turned off, and the server's setup restored and undone`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      const page = await (await browser.newContext({ viewport: { width: 900, height: 700 }, reducedMotion: "reduce" })).newPage();
      const errors = [], posts = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/*", serve(lang, posts));
      t.after(async () => {
        if (process.env.ARTIFACT_DIR) {
          await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
          await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `${engine}-${lang}-sync-auto-restore.png`), fullPage: true });
        }
        await browser.close();
      });
      page.setDefaultTimeout(5000);

      await page.goto("http://magpie.test/");
      await page.locator("#prefs").click();
      await openPart(page, "sync");
      const list = page.locator("#syncList");
      const rowNamed = (name) => list.locator(".row.pref").filter({ has: page.locator(".name", { hasText: name }) });
      const auto = rowNamed(w.auto);
      await auto.waitFor({ state: "visible" });
      const toEnd = async () => {
        await page.mouse.move(450, 300);
        const left = () => page.locator("#view-settings").evaluate((v) => v.scrollHeight - v.clientHeight - v.scrollTop);
        for (let i = 0; i < 120 && (await left()) > 0.5; i++) { await page.mouse.wheel(0, 80); await page.waitForTimeout(15); }
        await page.waitForTimeout(300);
      };
      await toEnd();
      const top = () => page.evaluate(() => document.querySelector("#view-settings").scrollTop);
      assert.equal(await page.locator("#syncList select").count(), 0, "no native select");
      assert.equal(await auto.locator(".sub").textContent(), w.every3);
      assert.equal(await auto.locator(".segs .opt.on").textContent(), lang === "en" ? "3 min" : "3 分钟");

      // off: only when asked
      let at = await top();
      await auto.locator(".segs .opt", { hasText: w.off }).click();
      await page.waitForFunction((s) => [...document.querySelectorAll("#syncList .row.pref .sub")].some((x) => x.textContent === s), w.offSub);
      assert.deepEqual(posts.at(-1), { action: "auto", body: { minutes: -1 } });
      const first = list.locator(".row.pref").first();
      assert.ok((await first.locator(".sub").textContent()).endsWith(w.asked), "the sync row says it syncs only when asked");
      assert.equal(await top(), at, "picking Off didn't move the page");

      // restore asks first; cancel posts nothing
      const restoreRow = rowNamed(w.restoreRow);
      await restoreRow.locator("button", { hasText: w.restore }).click();
      const dialog = page.locator(".restore-ask");
      await dialog.waitFor({ state: "visible" });
      assert.equal(await dialog.locator(".ehead b").textContent(), w.ask);
      assert.ok((await dialog.textContent()).includes(w.kept), "the dialog says what is here is kept to undo");
      const n = posts.length;
      await dialog.locator("button", { hasText: w.cancel }).click();
      await dialog.waitFor({ state: "detached" });
      assert.equal(posts.length, n, "Cancel restored nothing");

      // restore: posted, and the notice it leaves undoes it
      at = await top();
      await restoreRow.locator("button", { hasText: w.restore }).click();
      await page.locator(".restore-ask button", { hasText: w.go }).click();
      const note = list.locator(".sync-note");
      await note.waitFor({ state: "visible" });
      assert.equal(posts.at(-1).action, "restore");
      assert.ok((await note.textContent()).includes(w.restored), "the notice says what was restored");
      await note.locator("button", { hasText: w.undo }).click();
      await note.waitFor({ state: "detached" });
      assert.equal(posts.at(-1).action, "undo");
      assert.equal(await top(), at, "restoring and undoing didn't move the page");
      assert.deepEqual(errors, []);
    });
  }
}
