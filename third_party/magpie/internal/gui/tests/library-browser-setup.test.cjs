// Run with Node's test runner and Playwright on the module path; see README.md.
// Magpie Browser's sheet in the Library's Discover: the agents it gives the
// browser act in the user's own tabs, so the sheet says so, and sets up the
// extension it needs in the browser the user is signed in to. Not set up,
// it offers Set up the extension…, which writes the folder and shows it,
// and the steps to load it there (chrome://extensions → Load unpacked);
// once the browser connects, the sheet says it is connected without being
// opened again. Every language, Chromium and WebKit, at 1100 and 420 wide,
// nothing running off to the side; no backend, the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { strings } = require("./strings.cjs");

const assets = path.resolve(__dirname, "../assets");
const { I18N } = strings();
const LANGS = ["en", "zh", "zh-TW", "ja", "de"];
const tr = (lang, s, vars = {}) => Object.entries(vars).reduce((o, [k, v]) => o.replace(`{${k}}`, v), lang === "en" ? s : I18N[lang]?.[s] ?? s);
const HOME = "/Users/tester";
const DIR = `${HOME}/.config/magpie/browser-extension`;
const ABOUT = "Use your own browser: your tabs, signed in to the sites you use. Open pages, click, type, upload, read and take screenshots, through the Magpie Browser extension.";

function fixture(lang) {
  const st = { written: false, connected: false, setups: 0 };
  const browser = () => ({ dir: DIR, written: st.written, version: "0.1.0", connected: st.connected, ...(st.connected ? { extension: "0.1.0", browser: "Chrome/141" } : {}) });
  const route = async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:${JSON.stringify(lang)},theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" } });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/library") {
      return json({
        dir: `${HOME}/.magpie/library`, backups: `${HOME}/.magpie/backups`, home: HOME,
        agents: [{ id: "claude", name: "Claude Code", icon: "claudecode-color", mcp: `${HOME}/.claude.json`, skills: `${HOME}/.claude/skills` }],
        instructions: { agents: [], sets: [] }, servers: [], foundServers: [], projects: [], foundSkills: [], skills: [], problems: [],
      });
    }
    if (url.pathname === "/api/library/market/servers") {
      return json({ items: [
        { id: "magpie-browser", name: "magpie-browser", title: "Magpie Browser", publisher: "Magpie", description: ABOUT, transport: "stdio", runs: "magpie", featured: true, optIn: true, have: "" },
        { id: "fetch", name: "fetch", title: "Fetch", description: "Read the web.", transport: "stdio", runs: "uvx", featured: true, have: "" },
      ] });
    }
    if (url.pathname === "/api/library/market/skills") return json({ items: [] });
    if (url.pathname === "/api/library/browser") return json(browser());
    if (url.pathname === "/api/library/browser/setup" && req.method() === "POST") {
      st.setups++;
      st.written = true;
      return json(browser());
    }
    if (url.pathname === "/api/groups") return json({ groups: [] });
    if (url.pathname === "/api/providers") return json({ providers: [], gateway: { running: true } });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    const body = await fs.readFile(file).catch(() => null);
    await (body ? route.fulfill({ body, contentType }) : route.fulfill({ status: 404, body: "" }));
  };
  return { route, st };
}

const wide = (page) => page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": Magpie Browser's sheet sets up the extension in the user's browser", async (t) => {
    assert(["chromium", "webkit"].includes(engine), "BROWSER must be chromium or webkit");
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    const shots = [];
    t.after(async () => {
      if (process.env.ARTIFACT_DIR) {
        await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
        for (const [name, buf] of shots) await fs.writeFile(path.join(process.env.ARTIFACT_DIR, `${engine}-library-browser-setup-${name}.png`), buf);
      }
      await browser.close();
    });
    for (const lang of LANGS) {
      const w = (s, vars) => tr(lang, s, vars);
      for (const width of [1100, 420]) {
        await t.test(`${lang} at ${width}px: set up, then connected`, async () => {
          const f = fixture(lang);
          const context = await browser.newContext({ viewport: { width, height: 800 }, reducedMotion: "reduce" });
          const page = await context.newPage();
          page.setDefaultTimeout(5000);
          page.setDefaultNavigationTimeout(60000);
          const errors = [];
          page.on("pageerror", (e) => errors.push(e.message));
          await page.route("**/*", f.route);
          await page.addInitScript(() => { localStorage.setItem("magpie.libTab", "mcp"); });
          await page.goto("http://magpie.test/?view=library");
          await page.locator('#view-library .mk[data-market="mcp"] .mk-card[data-id="magpie-browser"]').click();
          const sheet = page.locator("#modal .mk-sheet");
          await sheet.waitFor();
          assert.equal((await sheet.locator(".mk-name.big").textContent()).trim(), "Magpie Browser");
          assert.equal(await sheet.locator(".mk-about").textContent(), w(ABOUT));
          const hints = await sheet.locator(".mk-hint").allTextContents();
          assert(hints.includes(w("Pick the agents that may use your browser: they act in your tabs, signed in as you are.")), hints.join(" | "));
          // not the image server's word about what each image costs
          assert(!hints.some((h) => h.includes(w("Pick the agents that may generate images: each image costs what the model set in Settings → Images charges."))));

          const box = sheet.locator(".mk-browser");
          const state = box.locator(".mk-browser-state");
          const go = box.locator("button");
          await page.waitForFunction((s) => document.querySelector("#modal .mk-browser-state")?.textContent === s,
            w("Not set up: set up the extension in Chrome, Edge, Arc, Brave or another Chromium browser"));
          assert.equal((await go.textContent()).trim(), w("Set up the extension…"));
          assert.equal(await box.locator(".mk-browser-steps").isVisible(), false);
          assert.equal(await state.locator(".mk-browser-dot.on").count(), 0);
          assert.equal(await wide(page), 0);
          shots.push([`${lang}-${width}-new`, await sheet.screenshot()]);

          await go.click();
          await page.waitForFunction((s) => document.querySelector("#modal .mk-browser-state")?.textContent === s,
            w("Not connected: load the folder below in your browser, and keep that browser open"));
          assert.equal(f.st.setups, 1);
          assert.equal((await go.textContent()).trim(), w("Show the extension folder"));
          assert.deepEqual(await box.locator(".mk-browser-steps li").allTextContents(), [
            w("Open chrome://extensions (edge://extensions in Edge) in the browser you use every day."),
            w("Turn on Developer mode."),
            w("Click Load unpacked and choose {dir}.", { dir: "~/.config/magpie/browser-extension" }),
          ]);
          assert.equal(await box.locator(".mk-browser-steps").isVisible(), true);
          // nothing in the sheet runs off its side, the folder's path included
          assert.equal(await wide(page), 0);
          assert.equal(await sheet.evaluate((e) => e.scrollWidth - e.clientWidth), 0);
          shots.push([`${lang}-${width}-written`, await sheet.screenshot()]);

          // the extension loaded: the sheet says so on its next look
          f.st.connected = true;
          await page.waitForFunction((s) => document.querySelector("#modal .mk-browser-state")?.textContent === s,
            w("Connected to your browser") + " · 0.1.0", { timeout: 10000 });
          assert.equal(await state.locator(".mk-browser-dot.on").count(), 1);
          assert.equal(await box.locator(".mk-browser-steps").isVisible(), false);
          assert.equal(await wide(page), 0);
          shots.push([`${lang}-${width}-connected`, await sheet.screenshot()]);
          assert.deepEqual(errors, []);
          await context.close();
        });
      }
    }
  });
}
