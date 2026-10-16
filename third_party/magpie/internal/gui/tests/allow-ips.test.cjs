// Run with Node's test runner and Playwright on the module path; see README.md.
// Allowed addresses (lc on Discord: magpie in Docker on a public host):
// Settings › Network, under Share on local network, lists the IPs and CIDR
// ranges other computers may reach magpie from at all. Shown while magpie
// is shared or listens on the network, not otherwise. One is added from a
// field and its Add button (or Enter), saved as the gateway cleaned it,
// and taken away with its Remove; one that isn't an address, and a list
// that would shut out the page saving it, are refused in the page's
// language and nothing is saved. Set by MAGPIE_ALLOW_IPS, the list is
// shown with no field and no Remove. At a 390px panel every row fits, and
// no click moves the page. English and Chinese, Chromium and WebKit, the
// strings in all four languages; no backend, the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { watchSettingsReads, settingsDrawn } = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");

function settingsPayload(over) {
  return {
    theme: "light", lang: "en", tray: "panel", quotaLeft: false, currency: "usd",
    dock: false, dockWindow: false, proxy: "", redact: false, redactPersonal: false, redactWords: [],
    codexWarmup: "", claudeWarmup: "", codexWarmAt: "", claudeWarmAt: "", workbuddyCheckin: false, noStats: false,
    trayUsage: "", trayUsageEvery: 3, vision: "", imageGen: "",
    version: "0.1.400", dir: "/config/magpie", gateway: "http://127.0.0.1:3425",
    proxyNow: "none", proxySource: "none", login: false,
    visionModels: [], imageGenModels: [], workbuddyCheckins: [],
    lan: true, lanURLs: ["http://192.168.1.20:3425"],
    fx: { rate: 7.2, at: new Date().toISOString(), stale: false },
    ...over,
  };
}

// what the gateway does with a list (internal/settings/allowips.go and
// gui/api.go), for the few forms this test writes
function clean(list) {
  const out = [];
  for (const raw of list) {
    let e = raw.trim();
    if (e === "10.1.2.3/8") e = "10.0.0.0/8";
    if (!/^[0-9a-f:.]+(\/\d+)?$/i.test(e)) return { error: `"${raw}" is not an IP address or range: write it as 203.0.113.7, 10.0.0.0/8 or 2001:db8::/32` };
    if (!out.includes(e)) out.push(e);
  }
  if (out.length && !out.includes("203.0.113.50") && out.every((e) => e.startsWith("172.16."))) return { error: "that allow list would shut out 203.0.113.50, the address this page is open from: add it first" };
  return { ips: out };
}

function server(lang, posts, over) {
  const s = settingsPayload({ lang, allowIPs: [], ...over });
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data, status = 200) => route.fulfill({ json: data, status });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light"};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" }, fx: s.fx });
    if (url.pathname === "/api/settings") return json(s);
    if (url.pathname === "/api/settings/allowips") {
      const body = route.request().postDataJSON();
      posts.push(body.ips);
      if (s.allowIPsEnv) return json({ error: "MAGPIE_ALLOW_IPS sets the allow list; change it there" }, 400);
      const c = clean(body.ips);
      if (c.error) return json({ error: c.error }, 400);
      s.allowIPs = c.ips;
      return json(s);
    }
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await route.fulfill({ body: await fs.readFile(file), contentType });
  };
}

const en = {
  name: "Allowed addresses", add: "Add", remove: "Remove", field: "IP address or range",
  sub: "Other computers reach magpie only from these addresses or ranges (CIDR), even with a gateway key; this computer always can. None listed lets every address in",
  envSub: "Set by MAGPIE_ALLOW_IPS. Other computers reach magpie only from these addresses, even with a gateway key; this computer always can",
  notIP: "Write an address as 203.0.113.7, or a range as 10.0.0.0/8 or 2001:db8::/32",
  shutOut: "That list leaves out {addr}, the address magpie sees this page open from: add it first, or this page can’t reach magpie",
  fix: "Add {addr}",
  fixTip: "Behind Docker or a reverse proxy, this is the proxy’s address, not your computer’s: adding it lets in everyone who comes through that proxy",
  envSet: "MAGPIE_ALLOW_IPS sets this list; change it there",
};
const zh = { name: "允许的地址", add: "添加", remove: "移除" };

async function open(browser, lang, posts, over) {
  const page = await (await browser.newContext({ viewport: { width: 390, height: 560 }, reducedMotion: "reduce" })).newPage();
  page.setDefaultTimeout(5000);
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.route("**/*", server(lang, posts, over));
  await watchSettingsReads(page);
  await page.goto("http://127.0.0.1:4999/?view=settings&tab=network");
  await settingsDrawn(page);
  return { page, errors };
}

// every row fits the panel: nothing cut, nothing past the edge
async function fits(box) {
  const bad = await box.evaluate((b) => {
    const out = [];
    const right = b.getBoundingClientRect().right + 0.5;
    for (const e of b.querySelectorAll(".row, input, button, .name, .sub")) {
      const r = e.getBoundingClientRect();
      if (r.right > right || r.width === 0) out.push(e.className + " " + r.right + ">" + right);
      if (e.matches("button, .name") && e.scrollWidth > e.clientWidth + 1) out.push("cut: " + e.textContent);
    }
    if (document.documentElement.scrollWidth > innerWidth) out.push("page scrolls sideways");
    return out;
  });
  assert.deepEqual(bad, []);
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": allowed addresses are listed under Share on local network (lc)", async (t) => {
    assert(["chromium", "webkit"].includes(engine), "BROWSER must be chromium or webkit");
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());

    await t.test("every language has the strings", async () => {
      const { page } = await open(browser, "en", []);
      for (const lang of ["zh", "zh-TW", "ja", "de"]) {
        const missing = await page.evaluate(([lang, keys]) => keys.filter((k) => {
          const d = I18N[lang];
          return !d || !d[k] || d[k] === k;
        }), [lang, Object.values(en).filter((s) => !["Add", "Remove"].includes(s))]);
        assert.deepEqual(missing, [], lang);
      }
      await page.context().close();
    });

    await t.test("hidden while magpie answers only this computer", async () => {
      const { page, errors } = await open(browser, "en", [], { lan: false, lanURLs: [] });
      await page.locator("#lanList .row").first().waitFor();
      assert.equal(await page.locator("#allowList .row").count(), 0);
      assert.deepEqual(errors, []);
      await page.context().close();
    });

    for (const lang of ["en", "zh"]) {
      await t.test(lang, async () => {
        const posts = [];
        const { page, errors } = await open(browser, lang, posts);
        const box = page.locator("#allowList");
        await box.locator(".rule-row").waitFor();
        const tr = (s) => page.evaluate((s) => t(s), s);
        const w = {};
        for (const [k, s] of Object.entries(en)) {
          w[k] = await tr(s);
          if (lang !== "en") assert.notEqual(w[k], s, `"${s}" has no ${lang} translation`);
          if (lang === "zh" && zh[k]) assert.equal(w[k], zh[k]);
        }
        const row = box.locator(".rule-row");
        assert.equal((await row.locator(".name").textContent()).trim(), w.name);
        assert.equal((await row.locator(".sub").textContent()).trim(), w.sub);
        const field = row.locator("input");
        assert.equal(await field.getAttribute("aria-label"), w.field);
        assert.equal(await page.locator("#allowList select").count(), 0);
        await fits(box);

        const view = page.locator("#view-settings");
        const scroll = () => view.evaluate((v) => new Promise((ok) => requestAnimationFrame(() => requestAnimationFrame(() => ok(v.scrollTop)))));
        await view.hover();
        for (let i = 0; i < 40; i++) {
          const r = await field.boundingBox();
          if (r && r.y > 60 && r.y + r.height < 500 && (await scroll()) > 0) break;
          await page.mouse.wheel(0, 120);
          await page.waitForTimeout(30);
        }

        // added from the field, two at once: saved as the gateway cleaned them
        await field.fill("203.0.113.50, 10.1.2.3/8");
        let before = await scroll();
        await row.locator("button", { hasText: w.add }).click();
        await box.locator(".allow-ip-row .name", { hasText: "10.0.0.0/8" }).waitFor();
        assert.deepEqual(posts.at(-1), ["203.0.113.50", "10.1.2.3/8"]);
        assert.equal(await row.locator("input").inputValue(), "", "the field is left empty for the next one");
        assert.equal(await scroll(), before, "Add moved the page");
        // and with Enter
        await row.locator("input").fill("2001:db8::/32");
        await row.locator("input").press("Enter");
        await box.locator(".allow-ip-row .name", { hasText: "2001:db8::/32" }).waitFor();
        assert.deepEqual(posts.at(-1), ["203.0.113.50", "10.0.0.0/8", "2001:db8::/32"]);
        await fits(box);

        // refused, in the page's language, and nothing saved
        await row.locator("input").fill("my-laptop");
        before = await scroll();
        await row.locator("button", { hasText: w.add }).click();
        await page.waitForFunction((s) => document.querySelector("#allowList .rule-row .sub.err")?.textContent.trim() === s, w.notIP);
        assert.equal(await row.locator("input").inputValue(), "my-laptop", "what the user wrote stays, to be corrected");
        assert.equal(await box.locator(".allow-ip-row").count(), 3);
        assert.equal(await scroll(), before, "a refusal moved the page");
        await fits(box);

        // taken away with its Remove, wheeled into view first as a reader
        // would: the list grew under the field, and in Chinese the button
        // sat under the window's foot, where a click lands on the foot (a
        // scroll that isn't the reader's is put back, #1554)
        const remove = box.locator(".allow-ip-row").filter({ hasText: "10.0.0.0/8" }).locator("button", { hasText: w.remove });
        const viewBox = await view.boundingBox();
        for (let i = 0; i < 20; i++) {
          const r = await remove.boundingBox();
          if (r && r.y > viewBox.y && r.y + r.height < viewBox.y + viewBox.height) break;
          await page.mouse.wheel(0, 40);
          await scroll();
        }
        before = await scroll();
        await remove.click();
        await page.waitForFunction(() => document.querySelectorAll("#allowList .allow-ip-row").length === 2);
        assert.deepEqual(posts.at(-1), ["203.0.113.50", "2001:db8::/32"]);
        assert.equal(await scroll(), before, "Remove moved the page");
        assert.equal((await row.locator(".sub").textContent()).trim(), w.sub, "a save clears the refusal");

        const border = await box.locator(".row").first().evaluate((r) => getComputedStyle(r).borderLeftWidth);
        assert(border === "0px" || border === "", "no left border accent: " + border);
        assert.deepEqual(errors, []);
        await page.context().close();
      });
    }

    await t.test("a list that shuts out this page is refused", async () => {
      const posts = [];
      const { page, errors } = await open(browser, "zh", posts, { allowIPs: ["203.0.113.50"] });
      const box = page.locator("#allowList");
      const row = box.locator(".rule-row");
      await row.waitFor();
      await row.locator("input").fill("172.16.0.0/12");
      await row.locator("button", { hasText: zh.add }).click();
      // only this one: the old address taken away leaves the page out
      await box.locator(".allow-ip-row", { hasText: "172.16.0.0/12" }).waitFor();
      // scrolled with the wheel, as a reader would, until both rows show
      await page.locator("#view-settings").hover();
      for (let i = 0; i < 40; i++) {
        const r = await box.locator(".allow-ip-row").last().boundingBox();
        if (r && r.y > 60 && r.y + r.height < 460) break;
        await page.mouse.wheel(0, 120);
        await page.waitForTimeout(30);
      }
      await box.locator(".allow-ip-row").filter({ hasText: "203.0.113.50" }).locator("button").click();
      // the refusal names the address magpie sees the page from
      // (Mathresearcher on Discord: which one?), in the page's language
      const says = await page.evaluate((s) => t(s, { addr: "203.0.113.50" }), en.shutOut);
      assert.match(says, /203\.0\.113\.50/);
      await page.waitForFunction((s) => document.querySelector("#allowList .rule-row .sub.err")?.textContent.trim() === s, says);
      assert.equal(await box.locator(".allow-ip-row").count(), 2, "nothing saved");
      // and adds it to the list that was refused in one click, which then saves
      const fix = row.locator("button.allow-fix");
      assert.equal((await fix.textContent()).trim(), await page.evaluate((s) => t(s, { addr: "203.0.113.50" }), en.fix));
      assert.equal(await fix.getAttribute("title"), await page.evaluate((s) => t(s), en.fixTip));
      await fits(box);
      const view = page.locator("#view-settings");
      const scroll = () => view.evaluate((v) => new Promise((ok) => requestAnimationFrame(() => requestAnimationFrame(() => ok(v.scrollTop)))));
      const before = await scroll();
      await fix.click();
      await page.waitForFunction(() => !document.querySelector("#allowList .rule-row .sub.err"));
      assert.deepEqual(posts.at(-1), ["172.16.0.0/12", "203.0.113.50"]);
      assert.deepEqual(await box.locator(".allow-ip-row .name").allTextContents(), ["172.16.0.0/12", "203.0.113.50"]);
      assert.equal(await row.locator("button.allow-fix").count(), 0, "the fix goes with the refusal");
      assert.equal(await scroll(), before, "the fix moved the page");
      assert.deepEqual(errors, []);
      await page.context().close();
    });

    await t.test("set by MAGPIE_ALLOW_IPS: shown, not edited", async () => {
      const { page, errors } = await open(browser, "en", [], {
        lan: false, lanURLs: [], onNetwork: true,
        allowIPsEnv: "1.2.3.4,10.0.0.0/8", allowIPsFromEnv: ["1.2.3.4", "10.0.0.0/8"], allowIPs: ["9.9.9.9"],
      });
      const box = page.locator("#allowList");
      await box.locator(".rule-row").waitFor();
      assert.equal((await box.locator(".rule-row .sub").textContent()).trim(), en.envSub);
      assert.deepEqual(await box.locator(".allow-ip-row .name").allTextContents(), ["1.2.3.4", "10.0.0.0/8"]);
      assert.equal(await box.locator("input").count(), 0);
      assert.equal(await box.locator("button").count(), 0);
      await fits(box);
      assert.deepEqual(errors, []);
      await page.context().close();
    });
  });
}
