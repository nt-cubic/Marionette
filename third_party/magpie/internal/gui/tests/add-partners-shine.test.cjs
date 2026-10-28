// Run with Node's test runner and Playwright on the module path; see README.md.
// The Partners heading's light (yetone: 每次点击"添加供应商"……要有一个优雅、
// 精致、让人舒服的流光动画; the owner picked the traced outline): once the
// sheet opens and the heading is in view, a hairline runs round it and
// fades. Once an opening: a search drawing the sheet again doesn't play it,
// closing and opening it again does. The outline wraps the heading's words
// and stays in the sheet, and with reduced motion, the system's or
// Settings' (#3813), there is none.
// English, Chinese, Japanese and German, at 900px and 440px; no backend.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const partners = [
  { id: "acme", name: "Acme AI", icon: "generic", kind: "partner", sponsored: true, chat: "https://api.acme.example.com/v1" },
  { id: "beta", name: "Beta Relay", icon: "generic", kind: "partner", sponsored: true, chat: "https://api.beta.example.com/v1" },
];
const presets = [{ id: "openai", name: "OpenAI", icon: "openai", kind: "vendor", chat: "https://api.openai.example.com/v1" }];
// one provider already, so the page is the list with its Add provider
const providers = [{ id: "openai", name: "OpenAI", icon: "openai", preset: "openai", models: [], agents: [], key: { set: true, masked: "sk-…ab12" } }];

function server(lang, prefs = {}) {
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/api/partner") return route.fulfill({ status: 204, body: "" });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = ${JSON.stringify({ lang, theme: "light", web: false, ...prefs })};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light", ...prefs } });
    if (url.pathname === "/api/providers") return json({ providers, presets: [...partners, ...presets], excluded: [], gateway: { running: true, window: true } });
    if (url.pathname === "/api/groups") return json({ groups: [], pools: [] });
    if (url.pathname === "/api/gateway/trace") return json({ routes: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

const L = { en: "Partners", zh: "合作伙伴", ja: "パートナー", de: "Partner" };

// the heading as drawn now: whether it lit, what plays on it, and where its
// outline sits against its words and the sheet
const look = (page) => page.locator("#addSheet .kind").first().evaluate((k) => {
  const r = (e) => e.getBoundingClientRect();
  const ring = k.querySelector("i.ring"), sheet = r(k.closest("#addSheet")), o = ring && r(ring);
  const words = [r(k.querySelector("b")), r(k.querySelector("span"))];
  return {
    shine: k.classList.contains("shine"), lit: k.classList.contains("lit"),
    plays: k.getAnimations({ subtree: true }).map((a) => a.animationName).sort(),
    wraps: !!o && words.every((w) => w.left >= o.left && w.right <= o.right && w.top >= o.top && w.bottom <= o.bottom),
    inSheet: !!o && o.left >= sheet.left && o.right <= sheet.right,
    spans: k.querySelectorAll("span").length,
  };
});

async function open(page) {
  await page.locator("#addProvider").click();
  await page.locator("#addSheet .tile").first().waitFor();
  await page.locator("#addSheet .kind.lit").first().waitFor();
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": the Partners heading lights once an opening", async (t) => {
    assert(["chromium", "webkit"].includes(engine), "BROWSER must be chromium or webkit");
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    for (const lang of Object.keys(L)) for (const width of [900, 440]) {
      await t.test(lang + " " + width, async () => {
        const page = await (await browser.newContext({ viewport: { width, height: 800 } })).newPage();
        const errors = [];
        page.on("pageerror", (e) => errors.push(e.message));
        await page.route("**/*", server(lang));
        await page.goto("http://magpie.test/?view=providers");
        await open(page);

        assert.equal(await page.locator("#addSheet .kind b").first().textContent(), L[lang]);
        let h = await look(page);
        assert(h.shine && h.lit);
        assert.deepEqual(h.plays, ["shine-glow", "shine-trace"]);
        assert(h.wraps, "the outline wraps the heading's words");
        assert(h.inSheet, "the outline stays in the sheet");
        assert.equal(h.spans, 1, "the hint stays the heading's only span");

        // a search draws the sheet again; the light doesn't play again
        const find = page.locator("#addSheet input").first();
        await find.fill("a");
        await find.fill("");
        await page.locator("#addSheet .kind").first().waitFor();
        h = await look(page);
        assert(!h.shine && !h.lit);
        assert.deepEqual(h.plays, []);

        // closed and opened again, it does
        await page.keyboard.press("Escape");
        if (await page.locator("#addSheet").isVisible()) await page.keyboard.press("Escape");
        await page.locator("#addSheet").waitFor({ state: "hidden" });
        await open(page);
        h = await look(page);
        assert(h.lit);
        assert.deepEqual(h.plays, ["shine-glow", "shine-trace"]);
        assert.deepEqual(errors, []);
        await page.context().close();
      });
    }
    for (const [name, ctx, prefs] of [["the system's reduced motion", { reducedMotion: "reduce" }, {}], ["Settings' Reduce motion", {}, { reduceMotion: true }]]) await t.test(name, async () => {
      const page = await (await browser.newContext({ viewport: { width: 900, height: 800 }, ...ctx })).newPage();
      await page.route("**/*", server("en", prefs));
      await page.goto("http://magpie.test/?view=providers");
      await page.locator("#addProvider").click();
      await page.locator("#addSheet .tile").first().waitFor();
      const h = await look(page);
      assert(!h.shine && !h.lit);
      assert.equal(await page.locator("#addSheet .kind i").count(), 0);
      assert.deepEqual(h.plays, []);
      await page.context().close();
    });
  });
}
