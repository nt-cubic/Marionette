// Run with Node's test runner and Playwright on the module path; see README.md.
// cybercaius on Discord: with WSL not started, the Library's MCP page
// didn't list the agents installed in it. A stopped distro's agents
// (asleep) are listed by what magpie last saw there: a chip is lit as the
// library has it, quietly greyer, and says the distro isn't running; a
// click is sent as any is, and magpie writes it there when the distro
// next runs. One whose places magpie never saw (unseen) is only on the
// rows that name it, greyed out, kept by every click on the row, and a
// click on it says why. One line under the list names the distro. Nothing
// here starts WSL (#3896): the page only reads /api/library. Chromium and
// WebKit, 1100px and 390px, in en, zh, zh-TW, ja and de. No backend: the
// API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { strings } = require("./strings.cjs");

const assets = path.resolve(__dirname, "../assets");
const HOME = "/Users/me", D = "Ubuntu-24.04";
const CX = "codex@wsl:" + D, PI = "pi@wsl:" + D;
const srv = (name, agents, more = {}) => ({ name, transport: "stdio", command: "uvx", args: [name], agents, ...more });
const lib = (servers) => ({
  dir: `${HOME}/.magpie/library`, backups: `${HOME}/.magpie/backups`, home: HOME,
  agents: [
    { id: "claude", name: "Claude Code", icon: "", mcp: `${HOME}/.claude.json`, skills: `${HOME}/.claude/skills` },
    { id: CX, name: "Codex · WSL " + D, icon: "", mcp: "\\\\wsl.localhost\\" + D + "\\home\\me\\.codex\\config.toml", skills: "\\\\wsl.localhost\\" + D + "\\home\\me\\.codex\\skills", noSSE: true, mustCopy: true, how: "copy", asleep: D },
    { id: PI, name: "Pi · WSL " + D, icon: "", asleep: D, unseen: true },
  ],
  instructions: { agents: [] }, servers, skills: [], foundServers: [], projects: [], foundSkills: [], problems: [],
});

let I18N;
function tr(lang, key, vars = {}) {
  let s = key;
  if (lang !== "en") {
    assert.equal(typeof I18N[lang][key], "string", `${lang} has no ${JSON.stringify(key)}`);
    s = I18N[lang][key];
  }
  return s.replace(/\{(\w+)\}/g, (m, k) => (k in vars ? String(vars[k]) : m));
}

function server(lang, state, posts) {
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data, status = 200) => route.fulfill({ status, json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" } });
    if (url.pathname === "/api/library") return json(lib(state.servers));
    if (url.pathname.startsWith("/api/library/") && req.method() === "POST" && !url.pathname.endsWith("/reveal") && !url.pathname.includes("/check")) {
      const body = req.postDataJSON(), what = url.pathname.replace("/api/library/", "");
      posts.push({ what, body });
      if (what === "servers/agents") {
        state.servers = state.servers.map((s) => (s.name === body.name ? { ...s, agents: [...body.agents].sort() } : s));
        return json({ ...lib(state.servers), result: { changed: [] } });
      }
      return json(lib(state.servers));
    }
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/groups") return json({ groups: [] });
    if (url.pathname === "/api/providers") return json({ providers: [], gateway: { running: true } });
    if (url.pathname.startsWith("/api/")) return json({});
    if (url.host !== "magpie.test") return route.fulfill({ status: 404, body: "" });
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

const sideways = (page) => page.evaluate(() => {
  const over = [];
  const w = document.documentElement.clientWidth;
  if (document.documentElement.scrollWidth > w) over.push("page " + document.documentElement.scrollWidth);
  for (const e of document.querySelectorAll("#view-library .lib-server, #view-library .lib-agents, #view-library .lib-asleep")) {
    const r = e.getBoundingClientRect();
    if (r.width && (r.right > w + 0.5 || r.left < -0.5)) over.push((e.className || e.tagName) + " " + Math.round(r.left) + "–" + Math.round(r.right));
  }
  return over;
});

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": a stopped WSL distro's agents are on the MCP page as last seen", async (t) => {
    assert(["chromium", "webkit"].includes(engine), "BROWSER must be chromium or webkit");
    I18N = strings().I18N;
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || "chromium" }));
    t.after(() => browser.close());
    for (const width of [1100, 390]) for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
      await t.test(`${lang} at ${width}px`, async () => {
        const w = (key, vars) => tr(lang, key, vars);
        const state = { servers: [srv("fetch", [CX, PI]), srv("git", ["claude"]), srv("events", [], { transport: "sse", command: "", url: "https://example.invalid/sse" })] };
        const posts = [], errors = [];
        const ctx = await browser.newContext({ viewport: { width, height: 900 }, reducedMotion: "reduce" });
        await ctx.addInitScript(() => { try { localStorage.setItem("magpie.libTab", "mcp"); } catch {} });
        const page = await ctx.newPage();
        page.setDefaultTimeout(5000);
        page.on("pageerror", (e) => errors.push(e.message));
        page.on("dialog", (d) => { errors.push("a browser dialog: " + d.message()); d.dismiss(); });
        await page.route("**/*", server(lang, state, posts));
        await page.goto("http://magpie.test/");
        await page.locator('button[data-view="library"]').click();
        const v = page.locator("#view-library");
        const row = (name) => v.locator(".lib-server").filter({ has: page.locator(".name", { hasText: new RegExp("^" + name) }) });
        const chips = (name) => row(name).locator(":scope > .lib-agents > .lib-ag[data-agent]").evaluateAll((cs) => cs.map((c) => c.dataset.agent));
        const chip = (name, id) => row(name).locator(`.lib-ag[data-agent="${id}"]`);
        await row("git").waitFor();

        // Codex in the stopped distro is on every row, Pi only on fetch's
        assert.deepEqual(await chips("fetch"), ["claude", CX, PI]);
        assert.deepEqual(await chips("git"), ["claude", CX]);
        assert.deepEqual(await chips("events"), ["claude", CX]);
        const cx = chip("fetch", CX);
        assert.match(await cx.getAttribute("class"), /\basleep\b/);
        assert.equal(await cx.getAttribute("aria-pressed"), "true");
        assert.equal(await cx.getAttribute("aria-disabled"), null);
        assert.equal(await cx.getAttribute("title"), w("WSL {distro} isn't running: {agent} has it — click to take it away, and magpie writes the change there the next time it runs", { distro: D, agent: "Codex · WSL " + D }));
        assert.equal(await chip("git", CX).getAttribute("title"), w("WSL {distro} isn't running: a click gives it to {agent} the next time the distro runs", { distro: D, agent: "Codex · WSL " + D }));
        // what it can't take says so, as on any chip: no SSE for Codex
        assert.match(await chip("events", CX).getAttribute("class"), /\bblocked\b/);
        assert.notEqual(await chip("events", CX).getAttribute("title"), await chip("git", CX).getAttribute("title"));
        const pi = chip("fetch", PI);
        assert.match(await pi.getAttribute("class"), /\bunseen\b/);
        assert.equal(await pi.getAttribute("aria-disabled"), "true");
        assert.equal(await pi.getAttribute("aria-pressed"), "true");
        const unseen = w("WSL {distro} isn't running, and magpie hasn't seen {agent}'s files there yet: start the distro to change this", { distro: D, agent: "Pi · WSL " + D });
        assert.equal(await pi.getAttribute("title"), unseen);
        // one line for the distro, naming Pi; the line of agents with no
        // MCP servers doesn't
        const aside = v.locator(".lib-asleep");
        assert.equal(await aside.count(), 1);
        assert.equal(await aside.textContent(), w("WSL {distro} isn't running: its agents show what magpie last saw there, and what you change is written there the next time it runs.", { distro: D }) + " " + w("{agents} can be changed once it runs.", { agents: "Pi · WSL " + D }));
        assert.equal(await v.locator(".lib-aside").filter({ hasText: w("{agents} has no MCP servers magpie can write.", { agents: "Pi · WSL " + D }) }).count(), 0);
        assert.deepEqual(await sideways(page), [], "sideways with the rows");

        // Pi's chip clicked: nothing sent, the reason shown
        await row("fetch").locator(":scope > .lib-agents").hover();
        await pi.click({ force: true });
        await page.waitForTimeout(200);
        assert.deepEqual(posts, []);
        // Claude Code given fetch: Pi keeps it, as Codex does
        await chip("fetch", "claude").click();
        await page.waitForTimeout(300);
        assert.deepEqual(posts.map((p) => p.what), ["servers/agents"]);
        assert.deepEqual([...posts[0].body.agents].sort(), ["claude", CX, PI]);
        // Codex given git while its distro is stopped: sent, kept
        await row("git").locator(":scope > .lib-agents").hover();
        await chip("git", CX).click();
        await page.waitForTimeout(300);
        assert.deepEqual(posts[1], { what: "servers/agents", body: { name: "git", agents: ["claude", CX] } });
        assert.deepEqual(errors, []);
        await ctx.close();
      });
    }
  });
}
