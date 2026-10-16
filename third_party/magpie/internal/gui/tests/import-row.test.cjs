// An app magpie is added to by a link of its own (Cindy, Lody) is a row like the
// others on the Agents page (the owner: its wide button, with no line under
// the name, sat apart from every other): a line under its name says whether
// magpie is added, and its button stands where the others' model picker
// does, as wide and lined up with it.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");

// connected when Cindy has magpie, so neither is folded under Show more
const crush = (wired) => ({
  id: "crush", name: "Crush", path: "/test/crush.json", icon: "crush", wired,
  fields: [{ key: "model", label: "model", value: wired ? "magpie/deepseek-flash" : "", options: [{ value: "magpie/deepseek-flash", label: "DeepSeek Flash", ref: "deepseek/deepseek-flash" }] }],
});
const cindy = (added) => ({ id: "cindy", name: "Cindy", path: "", icon: "cindy", import: "cindy://import?x=1", added, fields: [] });

async function serve(route, lang, added) {
  const url = new URL(route.request().url());
  const json = (data) => route.fulfill({ json: data });
  if (url.pathname === "/boot.js") {
    return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false,textSize:100};` });
  }
  if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
  if (url.pathname === "/api/state") return json({ agents: [crush(added), cindy(added), { id: "lody", name: "Lody", path: "", icon: "lody", import: "lody://provider/import?v=1&data=fixture", fields: [] }], profiles: [], settings: { lang, theme: "light", textSize: 100 } });
  if (url.pathname === "/api/providers") return json({ providers: [], gateway: { running: true, window: true } });
  if (url.pathname === "/api/plugins") return json({ plugins: [] });
  if (url.pathname.startsWith("/api/")) return json({});
  const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
  const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
  await route.fulfill({ body: await fs.readFile(file), contentType });
}

const SAID = {
  en: [/^Not added · Cindy asks to add magpie/, /^Added · magpie is a provider in Cindy/],
  zh: [/^未添加 · /, /^已添加 · /],
  "zh-TW": [/^未新增 · /, /^已新增 · /],
  ja: [/^未追加 · /, /^追加済み · /],
  de: [/^Nicht hinzugefügt · /, /^Hinzugefügt · /],
};
// Lody has no status magpie can read: its line asks to confirm in Lody, in
// the reader's language, and never says added or not
const UNKNOWN = {
  en: "Opens Lody to add magpie as a provider — confirm it there",
  zh: "打开 Lody 添加 magpie 供应商，在其中确认",
  "zh-TW": "開啟 Lody 新增 magpie 供應商，在其中確認",
  ja: "Lody を開いて magpie をプロバイダとして追加します。そちらで確認してください",
  de: "Öffnet Lody, um magpie als Anbieter hinzuzufügen — dort bestätigen",
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of Object.keys(SAID)) {
    for (const added of [false, true]) {
      test(`${engine} ${lang} ${added ? "added" : "not added"}: import rows show known or unknown status and open their app`, async (t) => {
        const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
        const page = await browser.newPage({ viewport: { width: 960, height: 600 } });
        t.after(async () => {
          if (process.env.ARTIFACT_DIR) {
            await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
            await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `${engine}-${lang}-${added}-import-row.png`) });
          }
          await browser.close();
        });
        page.setDefaultTimeout(5000);
        await page.route("**/*", (route) => serve(route, lang, added));
        await page.goto("http://magpie.test/");
        await page.waitForLoadState("networkidle");

        const more = page.locator('.agent-more[aria-expanded="false"]');
        if (await more.count()) await more.click();
        const lody = page.locator('.row.agent[data-id="lody"]');
        await lody.scrollIntoViewIfNeeded();
        await lody.locator(".ag-st-t").waitFor();
        const hint = await lody.locator(".ag-st-t").innerText();
        assert.equal(hint, UNKNOWN[lang]);
        for (const said of Object.values(SAID).flat()) assert.doesNotMatch(hint, said);
        assert.equal(await lody.locator(".ag-st").evaluate((e) => e.classList.contains("on")), false);
        const lodyOpened = page.waitForRequest((r) => r.url().includes("/api/open"));
        await lody.locator(".ag-start").click();
        assert.equal(JSON.parse((await lodyOpened).postData()).url, "lody://provider/import?v=1&data=fixture");

        const row = page.locator('.row.agent[data-id="cindy"]');
        await row.locator(".ag-st").waitFor();
        assert.match(await row.locator(".ag-st-t").innerText(), SAID[lang][added ? 1 : 0]);
        assert.equal(await row.locator(".ag-st").evaluate((e) => e.classList.contains("on")), added);
        for (const width of [960, 700]) {
          await page.setViewportSize({ width, height: 600 });
          const box = (sel) => page.locator(sel).evaluate((e) => { const r = e.getBoundingClientRect(); return { x: r.x, right: r.right, width: r.width, y: r.y, bottom: r.bottom }; });
          const pick = await box('.row.agent[data-id="crush"] > .ag-start');
          const link = await box('.row.agent[data-id="cindy"] > .ag-start');
          assert(Math.abs(pick.right - link.right) < 1 && Math.abs(pick.width - link.width) < 1, `Cindy's button lines up with the model picker at ${width}px: ${JSON.stringify({ pick, link })}`);
          const who = await box('.row.agent[data-id="cindy"] > .who');
          assert(link.x >= who.right, `the button stays clear of the name at ${width}px`);
        }
        // narrow: no page scroll sideways, and both import rows keep their
        // line and button inside the window
        await page.setViewportSize({ width: 420, height: 700 });
        // the header's segments settle a moment after a resize, so wait for
        // the state rather than reading it at once
        await page.waitForFunction(() => document.documentElement.scrollWidth <= window.innerWidth, null, { timeout: 3000 }).catch(async () => {
          assert.fail(`sideways scroll at 420px: ${await page.evaluate(() => document.documentElement.scrollWidth)}`);
        });
        for (const id of ["lody", "cindy"]) {
          const r = page.locator(`.row.agent[data-id="${id}"]`);
          for (const sel of [".ag-start", ".ag-st-t"]) {
            const b = await r.locator(sel).evaluate((e) => { const x = e.getBoundingClientRect(); return { left: x.left, right: x.right, width: x.width }; });
            assert(b.width > 0 && b.left >= 0 && b.right <= 420 + 0.5, `${id} ${sel} inside the window at 420px: ${JSON.stringify(b)}`);
          }
        }
        await page.setViewportSize({ width: 960, height: 600 });
        // a click opens the app's link, the row's one control
        const opened = page.waitForRequest((r) => r.url().includes("/api/open"));
        await row.locator(".ag-start").click();
        const req = await opened;
        assert.equal(JSON.parse(req.postData() || "{}").url, "cindy://import?x=1");
      });
    }
  }
}
