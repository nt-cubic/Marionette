// Run with Node's test runner and Playwright on the module path; see README.md.
// Share on local network on a computer with an internet address (Discord,
// 10-28: 203.236.50.70 shown as the LAN address): the private addresses come
// first (gateway.LANURLs), and picking the internet one says anyone online
// can reach magpie there, and how to keep it to this network; with an
// allow list, that only those addresses get in. The Gateway page lists it
// under "On the internet", not "Local network". English and Chinese,
// Chromium and WebKit; no backend, the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { watchSettingsReads, settingsDrawn } = require("./reader.cjs");
const { fixture } = require("./fixtures/caller-keys.cjs");

const assets = path.resolve(__dirname, "../assets");

function settingsPayload(over) {
  return {
    theme: "light", lang: "en", tray: "panel", quotaLeft: false, currency: "usd",
    dock: false, dockWindow: false, proxy: "", redact: false, redactPersonal: false, redactWords: [],
    codexWarmup: "", claudeWarmup: "", codexWarmAt: "", claudeWarmAt: "", workbuddyCheckin: false, noStats: false,
    trayUsage: "", trayUsageEvery: 3, vision: "", imageGen: "",
    version: "0.1.400", dir: "/config/magpie", gateway: "http://0.0.0.0:3425",
    proxyNow: "none", proxySource: "none", login: false,
    visionModels: [], imageGenModels: [], workbuddyCheckins: [],
    lan: true, lanKey: "sk-magpie-0123456789abcdef", lanURLs: ["http://192.168.1.5:3425", "http://203.236.50.70:3425"], lanPublic: ["http://203.236.50.70:3425"],
    fx: { rate: 7.2, at: new Date().toISOString(), stale: false },
    ...over,
  };
}

function server(lang, web, over) {
  const s = settingsPayload({ lang, ...over });
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:${web}};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" }, fx: s.fx });
    if (url.pathname === "/api/settings") return json(s);
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

const words = {
  en: {
    open: "An internet address: anyone online can reach magpie here with a gateway key. To keep it to this network, list its range under Allowed addresses",
    listed: "An internet address: only the addresses under Allowed addresses get in",
    note: "On the internet",
    lan: "Local network", local: "This computer",
  },
  zh: {
    open: "公网地址：互联网上任何人凭网关密钥都能在这里连到 magpie。只想让这个局域网连入，请在「允许的地址」里填上它的网段",
    listed: "公网地址：只有「允许的地址」里的地址能连入",
    note: "在公网上",
    lan: "局域网", local: "本机",
  },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": an internet address under Share on local network says so", async (t) => {
    assert(["chromium", "webkit"].includes(engine), "BROWSER must be chromium or webkit");
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    const open = async (lang, over) => {
      const page = await (await browser.newContext({ viewport: { width: 900, height: 800 }, reducedMotion: "reduce" })).newPage();
      page.setDefaultTimeout(5000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/*", server(lang, false, over));
      await watchSettingsReads(page);
      await page.goto("http://localhost:3430/?view=settings&tab=network");
      await settingsDrawn(page);
      await page.locator("#lanList .lan-interface").waitFor();
      return { page, errors };
    };
    const row = (page) => page.locator("#lanList .lan-address-row");
    const pickFar = async (page) => {
      // in the middle of the view, away from Settings' search head, which
      // stays at the top and would take the click
      await page.locator("#lanList .lan-interface").evaluate((b) => b.scrollIntoView({ block: "center" }));
      await page.locator("#lanList .lan-interface").click();
      const menu = page.locator(".lan-address-menu");
      await menu.waitFor();
      const text = await menu.textContent();
      await menu.getByText("http://203.236.50.70:3425/v1").click();
      return text;
    };
    for (const lang of ["en", "zh"]) {
      const w = words[lang];
      await t.test(lang, async () => {
        {
          const { page, errors } = await open(lang);
          assert.equal((await page.locator("#lanList .lan-url").textContent()).trim(), "http://192.168.1.5:3425/v1");
          assert.equal(await row(page).locator(".sub").count(), 0, "a private address needs no note");
          assert((await pickFar(page)).includes(w.note), "the menu marks the internet address");
          assert.equal((await row(page).locator(".sub").textContent()).trim(), w.open);
          assert.deepEqual(errors, []);
        }
        {
          const { page, errors } = await open(lang, { allowIPs: ["192.168.1.0/24"] });
          await pickFar(page);
          assert.equal((await row(page).locator(".sub").textContent()).trim(), w.listed, "the allow list is named");
          assert.deepEqual(errors, []);
        }
        {
          const page = await (await browser.newContext({ viewport: { width: 900, height: 800 }, reducedMotion: "reduce" })).newPage();
          page.setDefaultTimeout(5000);
          const errors = [];
          page.on("pageerror", (e) => errors.push(e.message));
          await page.route("**/*", fixture(lang, "light", [], { lan: true,
            lanURLs: ["http://192.168.1.10:3999", "http://203.236.50.70:3999"], lanPublic: ["http://203.236.50.70:3999"] }));
          await page.goto("http://magpie.test/?view=gateway");
          const box = page.locator("#gatewayLan");
          await page.waitForFunction(() => document.querySelectorAll("#gatewayLan .gw-url").length === 3);
          assert.deepEqual((await box.locator(".gw-url code").allTextContents()).map((x) => x.trim()),
            ["http://127.0.0.1:3999", "http://192.168.1.10:3999", "http://203.236.50.70:3999"]);
          assert.deepEqual((await box.locator(".gw-url-label").allTextContents()).map((x) => x.trim()), [w.local, w.lan, w.note]);
          assert.deepEqual(errors, []);
        }
      });
    }
  });
}
