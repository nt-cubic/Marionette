// Run with Node's test runner and Playwright on the module path; see README.md.
// The Providers page has a Hide accounts switch of its own, and each page
// follows its own (a Discord user in #feedback: Routing's switch hid the
// Providers page too, which had no switch to turn it back). One who had
// Hide accounts on before keeps Providers hidden: its switch starts from
// the choice made then. Every language, wide and narrow; no backend.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const MAIL = "dev.one@example.com";

function serve(lang) {
  const base = { icon: "generic", chat: "", responses: "", anthropic: "", catalog: "", agents: [], fallback: [], headers: {}, keyList: [], proxy: "" };
  const providers = { providers: [
    { ...base, id: "codex", name: "Codex", icon: "codex-color", models: [{ id: "gpt-6", name: "GPT-6", on: true }], account: { agent: "codex", agentName: "Codex", user: MAIL, plan: "PLUS", logins: [{ agent: "codex", user: MAIL, plan: "PLUS", active: true, on: true }] } },
  ], presets: [], excluded: [], gateway: { running: true, window: true } };
  const settings = { lang, theme: "light" };
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings });
    if (url.pathname === "/api/settings") return json(settings);
    if (url.pathname === "/api/providers") return json(providers);
    if (url.pathname === "/api/groups") return json({ groups: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

const KEYS = ["Hide accounts", "Accounts hidden", "Mask the accounts on this page — email addresses and account names — for a screenshot to share", "Accounts are masked for a screenshot — click to show them"];

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
    for (const width of [1000, 420]) {
      // stored: what an earlier version left in magpie.maskEmails
      const open = async (t, stored) => {
        const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
        t.after(() => browser.close());
        const context = await browser.newContext({ viewport: { width, height: 800 }, reducedMotion: "reduce" });
        await context.addInitScript((v) => { try { if (!sessionStorage.getItem("seeded")) { if (v !== null) localStorage.setItem("magpie.maskEmails", v); sessionStorage.setItem("seeded", "1"); } } catch {} }, stored);
        const page = await context.newPage();
        page.setDefaultTimeout(5000);
        page.setDefaultNavigationTimeout(60000);
        page.errors = [];
        page.on("pageerror", (e) => page.errors.push(e.message));
        await page.route("**/*", serve(lang));
        return page;
      };
      const provText = (page) => page.evaluate(() => document.querySelector("#providers").innerText);
      const listed = (page) => page.waitForFunction(() => document.querySelector("#providers")?.innerText.includes("Codex"));

      test(`${engine} ${lang} ${width}: Providers' Hide accounts is its own`, async (t) => {
        const page = await open(t, null);
        await page.goto("http://magpie.test/?view=routing");
        await page.locator("#rtMask").waitFor();
        await page.evaluate((m) => { const d = document.createElement("div"); d.id = "sample"; d.textContent = "Codex as " + m; document.querySelector("#view-routing").append(d); }, MAIL);

        // Routing's switch hides Routing's accounts
        await page.locator("#rtMask").click();
        await page.waitForFunction(() => document.querySelector("#sample .pii"));

        // and leaves the Providers page alone, whose own switch is off
        await page.evaluate(() => show("providers"));
        await listed(page);
        const pv = page.locator("#pvMask");
        await pv.waitFor();
        assert.ok((await provText(page)).includes(MAIL), "Providers shows the account while its own switch is off");
        assert.equal(await page.locator("#view-providers .pii").count(), 0);
        assert.equal(await pv.getAttribute("aria-pressed"), "false");
        assert.equal(await pv.locator("[data-t]").textContent(), await page.evaluate((k) => t(k), KEYS[0]));
        assert.equal(await pv.getAttribute("title"), await page.evaluate((k) => t(k), KEYS[2]));

        // placed as Routing's: in the page's head row, at its right end
        const box = await pv.boundingBox(), q = await page.locator("#pvQ").boundingBox();
        assert.ok(Math.abs((box.y + box.height / 2) - (q.y + q.height / 2)) < 4, `beside the search box: ${JSON.stringify([box, q])}`);
        assert.ok(box.x >= q.x + q.width - 1, `at the right end: ${JSON.stringify([box, q])}`);
        assert.ok(box.x + box.width <= width, "inside the window");

        // turned on here: Providers hidden, the choice kept
        await pv.click();
        await page.waitForFunction(() => document.querySelector("#view-providers .pii"));
        assert.ok(!(await provText(page)).includes(MAIL));
        assert.equal(await pv.getAttribute("aria-pressed"), "true");
        assert.equal(await pv.locator("[data-t]").textContent(), await page.evaluate((k) => t(k), KEYS[1]));
        assert.equal(await page.evaluate(() => localStorage.getItem("magpie.maskProviders")), "1");

        // Routing's turned off: Routing back, Providers still hidden
        await page.evaluate(() => show("routing"));
        await page.locator("#rtMask").click();
        await page.waitForFunction(() => !document.querySelector("#sample .pii"));
        assert.equal(await page.locator("#sample").textContent(), "Codex as " + MAIL);
        await page.evaluate(() => show("providers"));
        assert.ok(await page.locator("#view-providers .pii").count() > 0, "Routing's switch turned Providers' off");
        assert.ok(!(await provText(page)).includes(MAIL));

        // and after a reload
        await page.reload();
        await listed(page);
        await page.waitForFunction(() => document.querySelector("#view-providers .pii"));
        assert.equal(await page.locator("#pvMask").getAttribute("aria-pressed"), "true");
        assert.equal(await page.evaluate(() => localStorage.getItem("magpie.maskEmails")), "0");

        assert.equal(await page.locator("select").count(), 0, "no native select");
        assert.equal(await pv.evaluate((b) => getComputedStyle(b).borderLeftWidth), "0px");
        assert.ok(!/gradient/.test(await pv.evaluate((b) => getComputedStyle(b).backgroundImage)), "no gradient");
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth), 0, `no sideways scroll at ${width}px`);
        const missing = await page.evaluate(([keys, lang]) => lang === "en" ? [] : keys.filter((k) => !I18N[lang]?.[k]), [KEYS, lang]);
        assert.deepEqual(missing, [], `every string has its ${lang}`);
        assert.deepEqual(page.errors, []);
      });

      // one who had Hide accounts on keeps Providers hidden; one who had it
      // off keeps it off, even with Mask personal data on later
      for (const stored of ["1", "0"]) {
        test(`${engine} ${lang} ${width}: Hide accounts ${stored === "1" ? "on" : "off"} before the update stays so on Providers`, async (t) => {
          const page = await open(t, stored);
          await page.goto("http://magpie.test/?view=providers");
          await listed(page);
          await page.locator("#pvMask").waitFor();
          if (stored === "1") await page.waitForFunction(() => document.querySelector("#view-providers .pii"));
          assert.equal(await page.locator("#view-providers .pii").count() > 0, stored === "1");
          assert.equal((await provText(page)).includes(MAIL), stored === "0");
          assert.equal(await page.locator("#pvMask").getAttribute("aria-pressed"), String(stored === "1"));
          assert.equal(await page.evaluate(() => localStorage.getItem("magpie.maskProviders")), stored);
          await page.evaluate(() => hideAccounts.follow(true));
          assert.equal(await page.locator("#pvMask").getAttribute("aria-pressed"), String(stored === "1"), "a default never turns the user's choice");
          assert.deepEqual(page.errors, []);
        });
      }
    }
  }
}
