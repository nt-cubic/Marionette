// Run with Node's test runner and Playwright on the module path; see README.md.
// The masking demo's letter is made up, and its account (an address) and
// password are the point of it: with Hide accounts on (outloudvi on
// Discord), the window's privacy blur leaves the demo whole, the account
// as the password. So does it any text marked data-no-pii, its tooltips
// too, while an address beside it is still hidden. Chromium and WebKit,
// en and zh, played through and resting on the last step.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const K = "1792987654321000001";
const record = { key: K, t: new Date(Date.now() - 60e3).toISOString(), route: "messages", what: "chat", agent: "claude", provider: "anthropic", model: "claude-opus-4-1", n: 1,
  values: [{ p: "{{PERSON_bbbb2222}}", k: "PERSON", b: "model", h: "", l: 3, n: 1 }], snips: { "{{PERSON_bbbb2222}}": "ask {{PERSON_bbbb2222}} about it" } };

function serve(lang, theme, fetched, more, boot = {}) {
  const settings = { theme, lang, tray: "panel", currency: "usd", redact: true, redactPersonal: true, ...more };
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = ${JSON.stringify({ lang, theme, web: true, ...boot })};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [{ id: "claude", name: "Claude Code", icon: "claudecode-color", fields: [] }], profiles: [], settings });
    if (url.pathname === "/api/settings") return json(settings);
    if (url.pathname === "/api/providers") return json({ providers: [{ id: "anthropic", name: "Anthropic", icon: "anthropic", agents: [], models: [] }], presets: [], excluded: [], gateway: { running: true, window: true, mine: true, url: "http://127.0.0.1:3999", models: 0, groups: [], calls: [] } });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/masking/summary") return json({ days: [], requests: 1, n: 1, distinct: 1, blocked: 0, hours: [], kinds: [{ name: "PERSON", n: 1, distinct: 1 }], by: { model: 1 }, agents: [{ name: "claude", n: 1, distinct: 1 }], providers: [{ name: "anthropic", n: 1, distinct: 1 }], values: [] });
    if (url.pathname === "/api/masking/records") return json({ records: [record], more: false });
    if (url.pathname === "/api/masking/record") return json(record);
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname), ext = path.extname(file);
    if (ext === ".webm" || ext === ".jpg") fetched.push(path.basename(file));
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".jpg": "image/jpeg", ".webm": "video/webm" }[ext];
    let body;
    try { body = await fs.readFile(file); } catch { return route.fulfill({ status: 404, body: "" }); }
    // a video is served in ranges, as the gateway's file server does, or
    // Chromium can't seek in it
    const range = ext === ".webm" && /bytes=(\d+)-(\d*)/.exec(req.headers().range || "");
    if (range) {
      const from = +range[1], to = range[2] ? Math.min(+range[2], body.length - 1) : body.length - 1;
      return route.fulfill({ status: 206, body: body.subarray(from, to + 1), contentType, headers: { "Accept-Ranges": "bytes", "Content-Range": `bytes ${from}-${to}/${body.length}` } });
    }
    await route.fulfill({ body, contentType, headers: ext === ".webm" ? { "Accept-Ranges": "bytes" } : {} });
  };
}

async function open(engine, lang, width, opts = {}) {
  const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
  const page = await (await browser.newContext({ viewport: { width, height: 1000 }, reducedMotion: opts.motion ? "no-preference" : "reduce" })).newPage();
  page.setDefaultTimeout(8000);
  const errors = [], fetched = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.route("**/*", serve(lang, opts.theme || "light", fetched, opts.settings, opts.boot));
  await page.goto("http://magpie.test/?view=masking");
  await page.locator("#mdRow").waitFor();
  return { browser, page, errors, fetched };
}
const state = (page) => page.evaluate(() => document.querySelector(".xqd").__state());
// page, when given, adds the demo's state to the failure
async function until(ok, ms = 8000, page) {
  for (const end = Date.now() + ms; !(await ok());) {
    if (Date.now() > end) throw new Error("never happened: " + ok + (page ? " " + JSON.stringify(await state(page).catch(() => null)) : ""));
    await new Promise((r) => setTimeout(r, 50));
  }
}

const MAIL = "yetone@example.com", PASS = "hunter2-prod";

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh"]) {
    for (const motion of [false, true]) {
      test(`${engine} ${lang}${motion ? " playing" : ""}: Hide accounts leaves the masking demo's letter whole`, async (t) => {
        const { browser, page, errors } = await open(engine, lang, 1100, { motion });
        t.after(() => browser.close());
        // Hide accounts on, as the reporter had it, and an address on the
        // page outside the demo, which it hides
        await page.evaluate(() => hideAccounts.set(true));
        await page.evaluate((mail) => {
          const d = document.createElement("div");
          d.id = "elsewhere";
          d.textContent = "mail " + mail;
          d.title = mail;
          document.querySelector("#maskDemo").before(d);
        }, MAIL);
        await page.waitForFunction(() => document.querySelector("#elsewhere .pii"));
        assert.ok(!(await page.locator("#elsewhere").getAttribute("title")).includes("yetone"), "a tooltip outside is hidden");

        await page.locator("#mdRow").click();
        await page.locator(".xqd .xq-stage").waitFor();
        if (motion) await until(async () => (await state(page)).T > 12, 20000, page);
        else await until(async () => Math.abs((await state(page)).T - 25.6) < .01, 8000, page);
        // the letter's account and password, each as written, nothing blurred
        const slot = (k) => page.locator(`.xq-stage [data-k="${k}"] .xq-real`);
        assert.equal(await slot("s0").textContent(), MAIL);
        assert.equal(await slot("s1").textContent(), PASS);
        assert.equal(await page.locator("#maskDemo .pii").count(), 0, "something in the demo is blurred: " + await page.locator("#maskDemo .pii").evaluateAll((n) => n.map((x) => x.dataset.raw)));
        assert.ok((await page.locator("#maskDemo").textContent()).includes(MAIL));

        // the marker is the rule, not the demo: any illustration so marked,
        // its tooltip too, drawn now or changed later
        await page.evaluate((mail) => {
          const d = document.createElement("div");
          d.dataset.noPii = "";
          d.id = "drawing";
          d.innerHTML = `<span title="${mail}">sign in as ${mail}</span>`;
          document.querySelector("#maskDemo").after(d);
        }, MAIL);
        await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
        await page.evaluate((mail) => { document.querySelector("#drawing span").textContent = "now " + mail; }, MAIL);
        await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
        assert.equal(await page.locator("#drawing .pii").count(), 0);
        assert.equal(await page.locator("#drawing span").textContent(), "now " + MAIL);
        assert.equal(await page.locator("#drawing span").getAttribute("title"), MAIL);
        assert.ok(await page.locator("#elsewhere .pii").count() > 0, "the address outside stays hidden");
        assert.deepEqual(errors, []);
      });
    }
  }
}
