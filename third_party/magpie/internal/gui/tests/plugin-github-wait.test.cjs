// Run with Node's test runner and Playwright on the module path; see README.md.
// GitHub answers after the rest of the plugins page, seconds after on a
// slow line, and the Unofficial section wasn't drawn until it did, so it
// read as there being no unofficial plugins (yetone: 当还没加载完的时候，
// 压根就不显示，会让那些网慢的用户以为没有非官方插件). While
// /api/plugins/github is held here, Discover shows the section in its
// place, before the field, with its head, a word that it is looking and
// cards still to come; let go, its cards come in where those were and the
// word goes. A search while it waits shows the same. When the ask fails,
// the section says so with Try again, which asks again. English, Chinese,
// Japanese and German, wide and 440px, in Chromium and WebKit; nothing
// scrolls on a click. MAGPIE_WAIT_ASSETS names an older assets directory,
// to see this fail on the code before it.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(process.env.MAGPIE_WAIT_ASSETS || path.resolve(__dirname, "../assets"));

const REPOS = [
  { spec: "opencode-claude-auth", repo: "griffinmartin/opencode-claude-auth", url: "https://github.com/griffinmartin/opencode-claude-auth", owner: "griffinmartin", description: "OpenCode plugin that uses your existing Claude Code credentials — no separate login needed. ", stars: 1301, license: "MIT", pushed: "2026-09-22T23:47:15Z", package: "opencode-claude-auth", version: "2.2.1" },
  { spec: "github:keli-wen/agy-staff", repo: "keli-wen/agy-staff", url: "https://github.com/keli-wen/agy-staff", owner: "keli-wen", description: "Hire Google's Antigravity CLI (agy) as a fast Gemini staffer for Claude Code and OpenAI Codex.", stars: 723, license: "MIT", pushed: "2026-10-06T16:29:15Z", package: "agy-staff", version: "0.7.4" },
];
const LISTINGS = [
  { package: "@magpie-community/opencode-zed-auth", name: "Zed", community: true, kind: "pv", npm: { version: "0.1.8", weekly: 90, license: "MIT" } },
];

// gh: how /api/plugins/github answers each time it is asked, in turn: a
// promise held until let go, or "fail"
function server(lang, gh) {
  let asked = 0;
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" } });
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true }, plugins: [] });
    if (url.pathname === "/api/groups") return json({ groups: [], pools: [] });
    if (url.pathname === "/api/plugins") return json({ bun: true, bunVersion: "1.4.2", plugins: [], picker: false, movable: [] });
    if (url.pathname === "/api/plugins/listings") return json({ listings: LISTINGS });
    if (url.pathname === "/api/plugins/github") {
      const next = gh[asked++] ?? gh.at(-1);
      if (next === "fail") return route.fulfill({ status: 502, contentType: "text/plain", body: "GitHub didn't answer" });
      await next;
      return json({ repos: REPOS, topic: "magpie-plugin" }).catch(() => {});
    }
    if (url.pathname === "/api/plugins/search") return json({ hits: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    if (!/^\/[\w./-]*$/.test(url.pathname) || url.host !== "magpie.test") return route.fulfill({ status: 404, body: "" });
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

const L = {
  en: { section: "Unofficial, on GitHub", look: "Looking on GitHub", failed: /Couldn't ask GitHub/, again: "Try again" },
  zh: { section: "非官方插件 · GitHub", look: "正在 GitHub 上查找", failed: /无法连接 GitHub/, again: "重试" },
  ja: { section: "非公式 · GitHub", look: "GitHub を検索中", failed: /GitHub に接続できませんでした/, again: "再試行" },
  de: { section: "Inoffiziell, auf GitHub", look: "Suche auf GitHub", failed: /GitHub nicht erreichbar/, again: "Erneut versuchen" },
};

const shot = async (loc, name) => {
  if (!process.env.ARTIFACT_DIR) return;
  await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
  await loc.screenshot({ path: path.join(process.env.ARTIFACT_DIR, name + ".png") });
};
const held = () => { let go; const p = new Promise((r) => { go = r; }); p.go = go; return p; };

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": the GitHub section while GitHub hasn't answered", async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || "chromium" }));
    t.after(() => browser.close());
    for (const lang of Object.keys(L)) {
      for (const width of [980, 440]) {
        await t.test(lang + " " + width, async () => {
          const w = L[lang];
          const open = async (gh) => {
            const ctx = await browser.newContext({ viewport: { width, height: 860 } });
            const page = await ctx.newPage();
            page.setDefaultTimeout(5000);
            const errors = [];
            page.on("pageerror", (e) => errors.push(e.message));
            await page.route("**/*", server(lang, gh));
            await page.addInitScript(() => { try { localStorage.setItem("magpie.pluginTab", "discover"); } catch {} });
            await page.goto("http://magpie.test/?view=plugins");
            const view = page.locator("#view-plugins");
            await view.waitFor({ state: "visible" });
            await view.locator(".pm-card", { hasText: "Zed" }).waitFor();
            const sec = view.locator(".pm-sec", { has: page.locator("h3", { hasText: w.section }) });
            return { ctx, page, view, sec, errors };
          };

          // held: the section is there, before the field, looking
          const wait = held();
          const { ctx, page, view, sec, errors } = await open([wait]);
          t.after(() => ctx.close());
          await sec.waitFor();
          const order = await view.locator(".pm-body > *").evaluateAll((es) => es.map((e) => e.className));
          assert.ok(order.findIndex((c) => c.includes("pm-ghsec")) < order.findIndex((c) => c.includes("pm-manual")), "before the field: " + order);
          assert.equal(await sec.getAttribute("aria-busy"), "true");
          assert.match(await sec.locator(".pm-sechead > span").first().innerText(), /magpie-plugin/);
          const look = sec.locator(".pm-ghlook");
          assert.equal((await look.innerText()).trim(), w.look);
          assert.equal(await look.getAttribute("role"), "status");
          assert.equal(await sec.locator(".pm-card.ghost").count(), 3);
          // the light moves: a ghost's bar is animated, and each card a beat after the last
          const anim = await sec.locator(".pm-card.ghost .sk").evaluateAll((es) => es.map((e) => [getComputedStyle(e).animationName, getComputedStyle(e).animationDelay]));
          assert.ok(anim.every(([n]) => n === "pm-sheen"), "sheen: " + JSON.stringify(anim));
          assert.notEqual(anim[0][1], anim.at(-1)[1], "a beat apart");
          // a ghost is laid out as a card: nothing spills at this width
          for (const g of await sec.locator(".pm-card.ghost").all()) {
            assert.ok(await g.evaluate((e) => e.scrollWidth <= e.clientWidth + 1 && e.getBoundingClientRect().height > 70), "ghost fits");
          }
          await shot(sec, `wait-${engine}-${lang}-${width}`);

          // a search while it waits: the same section, waiting
          const q = view.locator(".pm-head .pm-find input");
          await q.fill("claude");
          await sec.locator(".pm-ghlook").waitFor();
          assert.equal(await sec.locator(".pm-card.ghost").count(), 3);
          await q.fill("");
          await sec.locator(".pm-ghlook").waitFor();

          // let go: its cards in the ghosts' place, the word gone
          wait.go();
          await sec.locator(".pm-card:not(.ghost)").first().waitFor();
          assert.equal(await sec.locator(".pm-card.ghost").count(), 0);
          assert.equal(await sec.locator(".pm-ghlook").count(), 0);
          assert.equal(await sec.getAttribute("aria-busy"), null);
          assert.deepEqual(await sec.locator(".pm-card .pm-name b").allInnerTexts(), ["opencode-claude-auth", "agy-staff"]);
          await page.waitForFunction(() => !document.querySelector(".pm-arrive"));
          await shot(sec, `came-${engine}-${lang}-${width}`);
          assert.deepEqual(errors, []);

          // failed: said so, and Try again asks again
          const again = held();
          const f = await open(["fail", again]);
          t.after(() => f.ctx.close());
          await f.sec.locator(".pm-ghnone").waitFor();
          assert.match(await f.sec.locator(".pm-ghnone").innerText(), w.failed);
          assert.equal(await f.sec.locator(".pm-card").count(), 0);
          await shot(f.sec, `failed-${engine}-${lang}-${width}`);
          const before = await f.page.evaluate(() => [window.scrollY, ...[...document.querySelectorAll("*")].filter((e) => e.scrollTop > 0).map((e) => (e.id || e.className) + ":" + e.scrollTop)].join());
          await f.sec.getByRole("button", { name: w.again }).click();
          await f.sec.locator(".pm-ghlook").waitFor();
          assert.equal(await f.page.evaluate(() => [window.scrollY, ...[...document.querySelectorAll("*")].filter((e) => e.scrollTop > 0).map((e) => (e.id || e.className) + ":" + e.scrollTop)].join()), before, "nothing scrolls on Try again");
          again.go();
          await f.sec.locator(".pm-card:not(.ghost)").first().waitFor();
          assert.equal(await f.sec.locator(".pm-ghnone").count(), 0);
          assert.deepEqual(f.errors, []);
        });
      }
    }
  });
}
