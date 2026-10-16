// Run with Node's test runner and Playwright on the module path; see README.md.
// Plugins that give tools on the Plugins page: the row lists each tool by
// the name magpie's plugin tools MCP server gives it, its description on
// hover, and a button that copies the server's address. A plugin of tools
// alone wears no Provider chip and isn't said to sign in to nothing; one
// that signs in too keeps its line; one switched off lists none. Every
// language, both engines, at a narrow width too; the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");

const WEATHER = "/Users/x/pl/weather-tools", BOTH = "/Users/x/pl/both", OFF = "/Users/x/pl/off";
const state = { bun: true, bunVersion: "1.3.0", named: true, plugins: [
  { spec: WEATHER, providers: [], moved: [], named: true, tools: [
    { name: "now", description: "The weather now in a city.", mcp: "weather-tools_now" },
    { name: "week", description: "Seven days ahead.", mcp: "weather-tools_week" },
  ] },
  { spec: BOTH, providers: ["Acme"], moved: [], named: true, tools: [{ name: "lookup", mcp: "both_lookup" }] },
  { spec: OFF, off: true, providers: [], moved: [], tools: [{ name: "x", mcp: "off_x" }] },
] };

function server(lang) {
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light", gateway: "http://127.0.0.1:3425" } });
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true }, plugins: [] });
    if (url.pathname === "/api/groups") return json({ groups: [], pools: [] });
    if (url.pathname === "/api/gateway/trace") return json({ routes: [] });
    if (url.pathname === "/api/plugins") return json(state);
    if (url.pathname === "/api/plugins/listings") return json({ listings: [] });
    if (url.pathname === "/api/plugins/market") return json({ listings: [], state });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

const L = {
  en: { installed: "Installed", tools: "Tools for every agent: weather-tools_now, weather-tools_week", nothing: "Signs in to nothing", provider: "Provider" },
  zh: { installed: "已安装", tools: "给每个智能体的工具：weather-tools_now、weather-tools_week", nothing: "不能登录", provider: "供应商" },
  "zh-TW": { installed: "已安裝", tools: /^給每個代理的工具：weather-tools_now/ },
  ja: { installed: "インストール済み", tools: /^すべてのエージェントのツール：weather-tools_now/ },
  de: { installed: "Installiert", tools: /^Werkzeuge für jeden Agenten: weather-tools_now/ },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": plugin tools in Installed", async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    for (const lang of Object.keys(L)) for (const width of [980, 420]) {
      await t.test(lang + " " + width, async () => {
        const w = L[lang];
        const ctx = await browser.newContext({ viewport: { width, height: 820 } });
        const page = await ctx.newPage();
        page.setDefaultTimeout(5000);
        const errors = [];
        page.on("pageerror", (e) => errors.push(e.message));
        await page.route("**/*", server(lang));
        // what Copy put on the clipboard, without the system's
        await page.addInitScript(() => {
          window.__copied = [];
          Object.defineProperty(navigator, "clipboard", { value: { writeText: async (s) => { window.__copied.push(s); } }, configurable: true });
        });
        await page.goto("http://magpie.test/?view=plugins");
        const view = page.locator("#view-plugins");
        await view.page().locator(":is(.lib-tabs .opt, .nav-s):visible", { hasText: w.installed }).click();
        const row = (name) => view.locator(".pm-row").filter({ has: page.locator(".name", { hasText: name }) });
        const weather = row("weather-tools");
        await weather.waitFor();
        const line = weather.locator(".pm-tools");
        const said = (await line.locator("span").first().innerText()).trim();
        if (typeof w.tools === "string") assert.equal(said, w.tools);
        else assert.match(said, w.tools);
        assert.match(await line.getAttribute("title"), /weather-tools_now — The weather now in a city\.\nweather-tools_week — Seven days ahead\./);
        if (w.nothing) assert.ok(!(await weather.innerText()).includes(w.nothing), "a plugin of tools isn't said to sign in to nothing");
        if (w.provider) assert.ok(!(await weather.locator(".pm-chip.kind").allInnerTexts()).map((x) => x.trim()).includes(w.provider), "no Provider chip on a plugin of tools alone");
        await line.locator(".copy").click();
        await page.waitForFunction(() => window.__copied.length > 0 || document.querySelector(".toast, #status"));
        const copied = await page.evaluate(() => window.__copied);
        if (copied.length) assert.equal(copied[0], "http://127.0.0.1:3425/mcp/magpie/tools");

        const both = row("both");
        assert.ok((await both.innerText()).includes("Acme"));
        assert.ok((await both.locator(".pm-tools").innerText()).includes("both_lookup"));
        assert.equal(await row("off").locator(".pm-tools").count(), 0, "a plugin off lists no tools");

        const over = await page.evaluate(() => document.scrollingElement.scrollWidth - document.scrollingElement.clientWidth);
        assert.ok(over <= 0, "page scrolls sideways by " + over);
        const [lb, rb] = await Promise.all([line.boundingBox(), weather.boundingBox()]);
        assert.ok(lb.x + lb.width <= rb.x + rb.width + 1, "the tools line overflows its row");
        if (process.env.ARTIFACT_DIR) await view.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `plugin-tools-${engine}-${lang}-${width}.png`) });
        assert.deepEqual(errors, []);
        await ctx.close();
      });
    }
  });
}
