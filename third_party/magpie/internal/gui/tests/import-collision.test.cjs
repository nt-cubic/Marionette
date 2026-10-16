// A provider another app has, named as one magpie has (#1487, Dudung1018):
// one named as a subscription (a relay called "claude", a proxy called
// "workbuddy") is picked like a new one and is never offered to replace the
// subscription; a name that collides can be changed right in the row, and
// a renamed one is added under that name, never in another's place. All
// APIs and keys are fixtures.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const assets = process.env.ASSET_DIR || path.resolve(__dirname, "../assets");
const engines = process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"];
const langs = ["en", "zh", "zh-TW", "ja", "de"];

const sources = () => [{
  id: "cc-switch", name: "CC Switch", path: "/fixture/.cc-switch/cc-switch.db", found: true,
  items: [
    { ref: "claude/cl2", from: "Claude Code", fingerprint: "cl2", status: "taken", reserved: true, existing: "Claude",
      provider: { id: "claude", name: "claude", anthropic: "https://relay-1487.example.com", key: "sk-…beef", models: ["claude-sonnet-5"] } },
    { ref: "pi/wb", from: "Pi", fingerprint: "wb", status: "taken", reserved: true, existing: "workbuddy",
      provider: { id: "workbuddy", name: "workbuddy", chat: "http://127.0.0.1:7863/v1", key: "sk-5…3432", models: ["glm-5"] } },
    { ref: "opencode/mine", from: "OpenCode", fingerprint: "mine", status: "taken", existing: "My Relay",
      provider: { id: "my-relay", name: "My Relay", chat: "https://other.example.com/v1", key: "sk-…1234", models: [] } },
  ],
}];

async function fixture(t, engine, lang, width) {
  const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
  const page = await browser.newPage({ viewport: { width, height: 900 }, reducedMotion: "reduce" });
  page.setDefaultTimeout(7000);
  const errors = [], posts = [];
  page.on("pageerror", (e) => errors.push(e.stack || e.message));
  await page.route("**/*", async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs={lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" } });
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true } });
    if (url.pathname === "/api/groups") return json({ groups: [], pools: [] });
    if (url.pathname === "/api/importapps/discovery") return json([]);
    if (url.pathname === "/api/importapps") {
      if (req.method() === "GET") return json(sources());
      posts.push(req.postDataJSON());
      return json({ added: ["x"], state: { providers: [], presets: [], excluded: [], gateway: { running: true, window: true } } });
    }
    if (url.pathname === "/api/gateway/trace") return new Promise(() => {});
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  });
  t.after(async () => {
    if (process.env.ARTIFACT_DIR) {
      await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
      await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `${engine}-${lang}-${width}-import-collision.png`) });
    }
    await browser.close();
    assert.deepEqual(errors, []);
  });
  await page.goto("http://magpie.test/?view=providers");
  await page.waitForFunction(() => typeof openImportApps === "function");
  await page.evaluate(() => openImportApps());
  const picker = page.locator("#modal .importapps");
  await picker.locator(".approw").first().waitFor();
  return { page, picker, posts };
}

const opts = (row) => row.locator(".segs .opt").allTextContents();

for (const engine of engines) {
  for (const lang of langs) {
    for (const width of [900, 390]) {
      test(`${engine} ${lang} ${width}px: a name that collides can be changed in the row, and a subscription is never replaced`, async (t) => {
        const { page, picker, posts } = await fixture(t, engine, lang, width);
        const T = (s, v) => page.evaluate(([s, v]) => t(s, v), [s, v]);
        const claude = picker.locator(".approw", { hasText: "Claude Code" });
        const wb = picker.locator(".approw", { hasText: "127.0.0.1:7863" });
        const mine = picker.locator(".approw", { hasText: "OpenCode" });

        // named as a subscription: picked, said so, added beside it only
        for (const row of [claude, wb]) {
          assert.equal(await row.locator('input[type="checkbox"]').isChecked(), true);
          assert.deepEqual(await opts(row), [], "no choice: it can only come in beside the subscription");
          assert.equal(await row.locator("input.appname").isVisible(), true);
        }
        assert.equal(await claude.locator(".sub", { hasText: await T("{name} is a subscription in magpie; this one comes in beside it as a provider of its own", { name: "Claude" }) }).count(), 1);
        // a name of the user's own provider: off until chosen, Replace offered
        assert.equal(await mine.locator('input[type="checkbox"]').isChecked(), false);
        assert.deepEqual(await opts(mine), [await T("Keep both"), await T("Replace it")]);

        // renamed right there: picked, and Replace is gone
        await mine.locator(".segs .opt", { hasText: await T("Replace it") }).click();
        await mine.locator("input.appname").fill("Other Relay");
        assert.equal(await mine.locator('input[type="checkbox"]').isChecked(), true);
        assert.deepEqual(await opts(mine), [], "renamed, it can only be added");
        // typing in the field never ticks or unticks the row
        await claude.locator("input.appname").click();
        assert.equal(await claude.locator('input[type="checkbox"]').isChecked(), true);
        await claude.locator("input.appname").fill("Findcg Claude");

        // the rows fit the window
        const fits = await picker.evaluate((ed) => [...ed.querySelectorAll(".approw")].every((r) => r.scrollWidth <= r.clientWidth + 1) && ed.getBoundingClientRect().right <= innerWidth);
        assert(fits, "rows fit the window");

        await picker.locator(".bar button.primary").click();
        await page.waitForFunction(() => !importingApps);
        assert.deepEqual(posts.length, 1);
        const picks = Object.fromEntries(posts[0].picks.map((p) => [p.ref, p]));
        assert.deepEqual(picks["claude/cl2"], { source: "cc-switch", ref: "claude/cl2", mode: "add", name: "Findcg Claude" });
        assert.deepEqual(picks["pi/wb"], { source: "cc-switch", ref: "pi/wb", mode: "add" });
        assert.deepEqual(picks["opencode/mine"], { source: "cc-switch", ref: "opencode/mine", mode: "add", name: "Other Relay" });
      });
    }
  }
}
