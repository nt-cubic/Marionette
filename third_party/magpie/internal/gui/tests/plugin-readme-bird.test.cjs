// Run with Node's test runner and Playwright on the module path; see README.md.
// A plugin's README loads under magpie's bird: it drops onto the skeleton's
// lines, each one bending under it as it bounces down them. When the README
// comes the bird flies off and the box goes. With reduced motion the lines
// stay straight and the bird sits still on the first.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const listings = [{ package: "opencode-copilot-auth", name: "GitHub Copilot", icon: "githubcopilot", providers: ["github-copilot"],
  summary: { en: "Your GitHub Copilot plan." }, npm: { version: "0.0.9", weekly: 48213, publisher: "thdxr" } }];
const readme = (n) => ["# opencode-copilot-auth", ...Array.from({ length: n }, (_, i) => "\nParagraph " + i + ".")].join("\n");

function server(reply) {
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"en",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang: "en", theme: "light" } });
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true }, plugins: [] });
    if (url.pathname === "/api/groups") return json({ groups: [], pools: [] });
    const state = { bun: true, bunVersion: "1.3.0", plugins: [] };
    if (url.pathname === "/api/plugins/market") return json({ listings, state });
    if (url.pathname === "/api/plugins") return json(state);
    if (url.pathname === "/api/plugins/listings") return json({ listings });
    if (url.pathname === "/api/plugins/search") return json({ hits: [{ package: "opencode-copilot-auth", version: "0.0.9" }] });
    if (url.pathname === "/api/plugins/page") return json(await reply());
    if (url.pathname.startsWith("/api/")) return json({});
    if (!/^\/[\w./-]*$/.test(url.pathname) || url.host !== "magpie.test") return route.fulfill({ status: 404, body: "" });
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

// the loader's state in one frame: each line's path, and the bird's place
const look = (page) => page.evaluate(() => {
  const b = document.querySelector("#modal .pm-bounce");
  if (!b) return null;
  return { paths: [...b.querySelectorAll("path")].map((p) => p.getAttribute("d")), bird: b.querySelector(".pm-bird").style.transform,
    feet: b.querySelector(".pm-bird svg").getBoundingClientRect().height };
});
// how far a line's middle bends below its ends
const bend = (d) => { const n = (d || "").match(/-?[\d.]+/g)?.map(Number) || []; return n.length < 14 ? 0 : n[7] - n[1]; };

async function open(browser, reduced) {
  const page = await (await browser.newContext({ viewport: { width: 980, height: 820 }, reducedMotion: reduced ? "reduce" : "no-preference" })).newPage();
  page.setDefaultTimeout(5000);
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  let release;
  const next = new Promise((r) => { release = r; });
  await page.route("**/*", server(() => next));
  await page.goto("http://magpie.test/?view=plugins");
  await page.locator("#view-plugins .pm-find input").fill("copilot");
  const card = page.locator('.pm-card[data-pkg="opencode-copilot-auth"]');
  await card.locator(".pm-dl", { hasText: "48k/week" }).waitFor();
  await page.waitForLoadState("networkidle");
  await card.locator(".pm-sum").click();
  await page.locator("#modal .pm-bounce").waitFor();
  return { page, errors, release };
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": the README loads under a bird bouncing down its lines, and it flies off when the README comes", async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    const { page, errors, release } = await open(browser, false);
    const first = await look(page);
    assert.equal(first.paths.length, 4, "four lines");
    assert(first.feet > 10, `the bird is drawn: ${JSON.stringify(first)}`);
    // over a round, a line bends under the bird and the bird moves
    const seen = await page.evaluate(() => new Promise((done) => {
      const b = document.querySelector("#modal .pm-bounce"), t0 = performance.now(), out = { bend: 0, places: new Set(), lit: 0 };
      const step = () => {
        for (const p of b.querySelectorAll("path")) {
          const n = (p.getAttribute("d") || "").match(/-?[\d.]+/g)?.map(Number) || [];
          if (n.length >= 14) out.bend = Math.max(out.bend, n[7] - n[1]);
        }
        out.lit = Math.max(out.lit, b.querySelectorAll("path[data-lit]").length);
        out.places.add(b.querySelector(".pm-bird").style.transform);
        if (performance.now() - t0 > 2000) return done({ bend: out.bend, places: out.places.size, lit: out.lit });
        requestAnimationFrame(step);
      };
      step();
    }));
    assert(seen.bend > 4, `a line bent under the bird: ${JSON.stringify(seen)}`);
    assert(seen.places > 20, `the bird moved: ${JSON.stringify(seen)}`);
    assert(seen.lit >= 2, `the lines it landed on lit: ${JSON.stringify(seen)}`);

    release({ readme: readme(12), updated: "2026-09-01T00:00:00Z" });
    await page.locator("#modal .pm-md p", { hasText: "Paragraph 11." }).waitFor();
    // flying off, it sits over the README, not above it, and leaves from
    // where it was
    const md = await page.evaluate(() => {
      const b = document.querySelector("#modal .pm-bounce"), m = document.querySelector("#modal .pm-md");
      return { gone: b ? "gone" in b.dataset : null, inReadme: b ? !!b.closest(".pm-readme") : null, boxTop: b?.getBoundingClientRect().top,
        mdTop: m.getBoundingClientRect().top, readmeTop: m.parentElement.getBoundingClientRect().top };
    });
    assert.notEqual(md.gone, false, `the box leaves when the README comes: ${JSON.stringify(md)}`);
    assert.notEqual(md.inReadme, true, `it leaves from outside the README's flow: ${JSON.stringify(md)}`);
    assert(md.boxTop == null || Math.abs(md.boxTop - md.mdTop) < 4, `it leaves from where it was: ${JSON.stringify(md)}`);
    assert(md.mdTop - md.readmeTop < 20, `the README takes the top, the leaving bird no room: ${JSON.stringify(md)}`);
    await page.waitForFunction(() => !document.querySelector("#modal .pm-bounce"), null, { timeout: 2000 });
    assert.deepEqual(errors, []);
  });

  test(engine + ": with reduced motion the lines stay straight and the bird sits still", async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    const { page, errors, release } = await open(browser, true);
    await page.waitForFunction(() => document.querySelector("#modal .pm-bounce path").getAttribute("d"));
    const a = await look(page);
    await page.waitForTimeout(400);
    const b = await look(page);
    assert.deepEqual(a, b, "nothing moves");
    for (const d of a.paths) assert.equal(bend(d), 0, `straight: ${d}`);
    assert.match(a.bird, /translate\(/);
    release({ readme: readme(3), updated: "2026-09-01T00:00:00Z" });
    await page.locator("#modal .pm-md p", { hasText: "Paragraph 2." }).waitFor();
    assert.equal(await page.locator("#modal .pm-bounce").count(), 0, "the box goes at once");
    assert.deepEqual(errors, []);
  });
}
