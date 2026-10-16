// Run with Node's test runner and Playwright on the module path; see README.md.
// Discord (lc): "rtk 没有识别 deepseek harness". RTK has no hook for
// DeepSeek Harness (dsh's hooks can't rewrite a command, and rtk init has no
// --agent dsh), and the RTK tab left it out, as if magpie hadn't seen it.
// It is listed now, tagged No RTK hook with why, and its switch can't be
// turned on; in English and Chinese. No backend: the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const HOME = "/Users/aimer";
const agent = (id, name) => ({ id, name, icon: "", skills: `${HOME}/.${id}/skills`, mcp: `${HOME}/.${id}/mcp.json` });
const lib = () => ({
  dir: `${HOME}/.magpie/library`, backups: `${HOME}/.magpie/backups`, home: HOME,
  agents: [agent("pi", "Pi"), agent("dsh", "DeepSeek Harness")],
  instructions: { agents: [], sets: [] }, foundServers: [], projects: [], foundSkills: [], skills: [], servers: [],
});
const rtkView = () => ({
  path: `${HOME}/.local/bin/rtk`, version: "0.51.0", url: "https://www.rtk-ai.app",
  agents: [
    { id: "pi", name: "Pi", icon: "", on: false },
    { id: "dsh", name: "DeepSeek Harness", icon: "", on: false, noHook: true, blocked: "RTK has no hook for DeepSeek Harness yet" },
  ],
});

function server(lang, calls) {
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return route.fulfill({ json: { agents: [], profiles: [], settings: { lang, theme: "light" } } });
    if (url.pathname === "/api/plugins") return route.fulfill({ json: { plugins: [] } });
    if (url.pathname === "/api/library") return route.fulfill({ json: lib() });
    if (url.pathname === "/api/library/rtk") {
      if (req.method() !== "GET") calls.push(req.postData());
      return route.fulfill({ json: rtkView() });
    }
    if (url.pathname === "/api/usage/quotas") return route.fulfill({ json: [] });
    if (url.pathname === "/api/groups") return route.fulfill({ json: { groups: [] } });
    if (url.pathname === "/api/providers") return route.fulfill({ json: { providers: [], gateway: { running: true } } });
    if (url.pathname.startsWith("/api/")) return route.fulfill({ json: {} });
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

const words = {
  en: { tag: "No RTK hook", why: "DeepSeek Harness's hooks can only allow or deny a command, not change it" },
  zh: { tag: "RTK 暂不支持", why: "DeepSeek Harness 的 hook 只能放行或拦截命令，不能改写" },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": an agent RTK has no hook for is listed with why", async (t) => {
    assert(["chromium", "webkit"].includes(engine), "BROWSER must be chromium or webkit");
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    const errors = [];
    t.after(async () => {
      if (errors.length) console.log(errors);
      await browser.close();
    });
    for (const lang of ["en", "zh"]) {
      await t.test(lang, async () => {
        const w = words[lang], calls = [];
        const ctx = await browser.newContext({ viewport: { width: 980, height: 800 } });
        await ctx.addInitScript(() => { try { localStorage.setItem("magpie.libTab", "rtk"); } catch {} });
        const page = await ctx.newPage();
        page.setDefaultTimeout(5000);
        page.on("pageerror", (e) => errors.push(e.message));
        await page.route("**/*", server(lang, calls));
        await page.goto("http://magpie.test/");
        await page.locator('button[data-view="library"]').click();
        const v = page.locator("#view-library");
        const row = (name) => v.locator(".lib-row").filter({ has: page.locator(".name", { hasText: name }) });
        await row("DeepSeek Harness").waitFor();
        const tag = row("DeepSeek Harness").locator(".lib-tag");
        assert.deepEqual(await tag.allTextContents(), [w.tag]);
        assert((await tag.getAttribute("title")).includes(w.why), await tag.getAttribute("title"));
        const sw = row("DeepSeek Harness").locator(".lib-switch");
        assert.equal(await sw.isDisabled(), true);
        // Pi has a hook: nothing to say, and it can be switched on
        assert.equal(await row("Pi").locator(".lib-tag").count(), 0);
        assert.equal(await row("Pi").locator(".lib-switch").isDisabled(), false);
        await sw.click({ force: true });
        assert.deepEqual(calls, []);
        await ctx.close();
      });
    }
    assert.deepEqual(errors, []);
  });
}
