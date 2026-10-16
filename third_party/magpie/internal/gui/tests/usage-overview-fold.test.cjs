// Run with Node's test runner and Playwright on the module path; see README.md.
// #2378 (Usage › Overview): the Agents, Models, Accounts and Gateway keys
// lists each fold away under their heading (日常看账号维度较多，都要拉到最底下),
// by a chevron on the heading, and stay folded across a reload; a click on
// a heading leaves the heading where it is. The tokens tile's label is a
// pop-up (the app's own menu) of how the totals are counted: without the
// cache, as before, or with the cache read and written counted too, in the
// tile, the chart and every row; kept across a reload. And on the Requests
// tab the Tokens card's line (in · out · cache) wraps between its parts
// rather than cutting the cache off in a wide window's five cards (缓存
// 879…). Every language, 1100 and 420 wide (the Requests card at 1500 too);
// no backend, the API is faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const reader = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");

// a row: tokens in and out, and the cache read beside them
const G = (id, name, input, output, cache_read, extra) => ({ id, name, icon: "generic", calls: 10, input, output, cache_read, cache_write: 0, cost: 0, ...extra });
const AGENTS = [G("codex", "Codex", 5856000, 2217000, 560000000), G("pi", "Pi", 38000, 361, 9216), G("curl", "curl", 17000, 218, 0)];
const MODELS = [G("gpt-6", "gpt-6", 5856000, 2217000, 560000000, { model: "gpt-6" }), G("glm-5", "glm-5", 55000, 579, 9216, { model: "glm-5" })];
const ACCOUNTS = [G("acct-a", "a@example.com", 4000000, 1500000, 400000000), G("acct-b", "b@example.com", 1911000, 717579, 160009216)];
const KEYS = [G("key-1", "laptop", 5911000, 2217579, 560009216)];
const sum = (k) => AGENTS.reduce((n, g) => n + g[k], 0);
const USAGE = {
  calls: 30, errors: 0, input: sum("input"), output: sum("output"), reasoning: 0, cache_read: sum("cache_read"), cache_write: 0, unpriced: 0, cost: 0, bucket: "day",
  series: [{ label: "Oct 15", time: new Date().toISOString(), calls: 30, input: sum("input"), output: sum("output"), cache_read: sum("cache_read"), cache_write: 0, cost: 0 }],
  path: "~/.config/magpie/usage.jsonl",
  agents: AGENTS, models: MODELS, accounts: ACCOUNTS, callerKeys: KEYS,
};
// the Requests tab's totals, the reporter's: 183.2万 in, 71.1万 out, 8793.3万 cache
const LEDGER = {
  period: "today", rows: [], offset: 0, total: 938, calls: 938, errors: 4, retried: 7,
  input: 1832000, output: 711000, cache_read: 87933000, cache_write: 0, cost: 12.3, unpriced: 0,
  by: {}, bucket: "hour", series: [], agents: [],
};

function server(lang) {
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    // zh and zh-TW in 万 and 亿, as the reporter reads them (183.2万 in · 8793.3万 cache)
    const settings = { lang, theme: "light", chineseUnits: lang.startsWith("zh") };
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings, fx: { rate: 7.2, at: new Date().toISOString() } });
    if (url.pathname === "/api/settings") return json(settings);
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/usage/requests") return json(LEDGER);
    if (url.pathname === "/api/usage") return json(USAGE);
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], models: [], gateway: { running: false } });
    if (url.pathname === "/api/sessions") return json({ sessions: [], dirs: [] });
    if (url.pathname === "/api/sessions/stats") return json({ from: "", to: "", days: [], agents: {} });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

// the words the reader sees: the headings' titles, the tile's label, the menu
const L = {
  en: { foldAcc: "Fold the accounts away", showAcc: "Show the accounts", tokens: "tokens", withCache: "tokens with cache", without: "Without cache", with: "With cache", head: "Token totals" },
  zh: { foldAcc: "收起账号", showAcc: "展开账号", tokens: "token", withCache: "token（含缓存）", without: "不含缓存", with: "含缓存", head: "token 汇总" },
  "zh-TW": { foldAcc: "收起帳號", showAcc: "展開帳號", tokens: "token", withCache: "token（含快取）", without: "不含快取", with: "含快取", head: "token 總計" },
  ja: { foldAcc: "アカウントを折りたたむ", showAcc: "アカウントを表示", tokens: "トークン", withCache: "トークン（キャッシュ込み）", without: "キャッシュなし", with: "キャッシュ込み", head: "トークンの合計" },
  de: { foldAcc: "Konten einklappen", showAcc: "Konten anzeigen", tokens: "Token", withCache: "Token mit Cache", without: "Ohne Cache", with: "Mit Cache", head: "Token-Summen" },
};

const goUsage = async (p) => {
  const nav = p.locator('[data-view="usage"]:visible').first();
  if (await nav.count()) await nav.click();
  else await p.evaluate(() => show("usage"));
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": the Overview's lists fold, the totals count the cache on request, the Requests card's cache is whole", async (t) => {
    assert(["chromium", "webkit"].includes(engine), "BROWSER must be chromium or webkit");
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    for (const lang of Object.keys(L)) {
      const w = L[lang];
      for (const width of [1100, 420]) {
        await t.test(`${lang} ${width}: folds and the cache`, async () => {
          const errors = [];
          const ctx = await browser.newContext({ viewport: { width, height: 700 }, reducedMotion: "reduce" });
          t.after(() => ctx.close());
          const open = async () => {
            const p = await ctx.newPage();
            p.setDefaultTimeout(5000);
            p.on("pageerror", (e) => errors.push(e.message));
            await p.route("**/*", server(lang));
            await p.goto("http://magpie.test/");
            await goUsage(p);
            // drawn, folded or not: a reload may find Accounts folded away
            await p.locator("#usageAccounts .row.stat").first().waitFor({ state: "attached" });
            return p;
          };
          let p = await open();
          const fold = (id) => p.locator(`#usagePane .row-head .fold[data-fold="${id}"]`);
          const shown = (sel) => p.locator(sel).evaluate((e) => !!e.offsetParent);
          const top = (loc) => loc.evaluate((e) => e.getBoundingClientRect().top);

          // every list's heading is a fold with its chevron, open at first
          for (const id of ["agents", "models", "accounts", "keys"]) {
            assert.equal(await fold(id).getAttribute("aria-expanded"), "true", id + " starts open");
            assert.equal(await fold(id).locator("svg").count(), 1, id + " has its chevron");
          }
          assert.equal(await fold("accounts").getAttribute("title"), w.foldAcc);
          // no coloured stripe on a heading
          assert.equal(await fold("accounts").evaluate((e) => getComputedStyle(e).borderLeftWidth), "0px");

          // Accounts folds where the reader has it, and the heading stays put
          await reader.inView(p, fold("accounts"));
          const was = await top(fold("accounts"));
          await fold("accounts").click();
          await p.waitForTimeout(150);
          assert.equal(await fold("accounts").getAttribute("aria-expanded"), "false");
          assert.equal(await fold("accounts").getAttribute("title"), w.showAcc);
          assert.equal(await shown("#usageAccounts"), false, "the accounts are folded away");
          assert(Math.abs((await top(fold("accounts"))) - was) <= 1, "folding moved the heading");
          assert.equal(await shown("#usageKeys"), true, "the keys below are still open");

          // Agents and Models fold too: Accounts' heading comes up the page
          await reader.inView(p, fold("agents"));
          const at = await top(fold("agents"));
          await fold("agents").click();
          await p.waitForTimeout(150);
          assert(Math.abs((await top(fold("agents"))) - at) <= 1, "folding Agents moved its heading");
          assert.equal(await shown("#usageAgents"), false);
          await reader.inView(p, fold("models"));
          await fold("models").click();
          await p.waitForTimeout(150);
          assert.equal(await shown("#usageModels"), false);

          // a reload keeps what was folded
          await p.close();
          p = await open();
          assert.equal(await fold("agents").getAttribute("aria-expanded"), "false", "Agents stays folded");
          assert.equal(await fold("models").getAttribute("aria-expanded"), "false", "Models stays folded");
          assert.equal(await fold("accounts").getAttribute("aria-expanded"), "false", "Accounts stays folded");
          assert.equal(await shown("#usageAgents"), false);
          assert.equal(await shown("#usageAccounts"), false);
          assert.equal(await shown("#usageKeys"), true);
          // Accounts opened again, the others still folded: it is near the top
          await reader.inView(p, fold("accounts"));
          await fold("accounts").click();
          await p.waitForTimeout(150);
          assert.equal(await shown("#usageAccounts"), true);
          assert.equal(await p.locator("#usageAccounts .row.stat").count(), 2);
          assert.equal(await shown("#usageAgents"), false);

          // the tokens tile: in and out, as before
          const tile = p.locator("#stats .kpi").first();
          const pick = tile.locator(".kpi-pick");
          const fmt = (n) => p.evaluate((n) => fmtN(n), n);
          const io = USAGE.input + USAGE.output, all = io + USAGE.cache_read;
          assert.equal((await tile.locator("b").textContent()).trim(), await fmt(io));
          assert.equal((await pick.textContent()).trim(), w.tokens);
          assert.equal(await p.locator("#usagePane select").count(), 0, "no native select");
          // the pop-up takes the label's room and the line under it reads as
          // the next tile's does: not the label's small capitals
          const looks = await p.locator("#stats .kpi").evaluateAll((ks) => ks.slice(0, 2).map((k) => {
            const s = k.querySelector("small"), part = s.querySelector("span") || s, cs = getComputedStyle(part);
            // the label's words: in the pop-up, its span
            const label = (k.querySelector(".kpi-pick > span") || k.querySelector(":scope > span")).getBoundingClientRect();
            return { at: Math.round(label.top - k.getBoundingClientRect().top), gap: Math.round(s.getBoundingClientRect().top - label.bottom), case: cs.textTransform, weight: cs.fontWeight, size: cs.fontSize };
          }));
          assert.deepEqual(looks[0], looks[1], "the tokens tile's label and line sit and read as the next tile's");
          // the pop-up is the app's own menu
          await reader.inView(p, pick);
          const vt = await p.locator("#view-usage").evaluate((v) => v.scrollTop);
          await pick.click();
          const menu = p.locator(".proto-menu:visible");
          await menu.waitFor();
          assert.equal((await menu.locator(".pm-head").textContent()).trim(), w.head);
          assert.deepEqual(await menu.locator(".pm-name").allTextContents(), [w.without, w.with]);
          await menu.locator(".pm-item").nth(1).click();
          await p.waitForTimeout(150);
          assert.equal(await p.locator("#view-usage").evaluate((v) => v.scrollTop), vt, "the pick moved the page");
          // with the cache: the tile, its label and line, the rows
          assert.equal((await tile.locator("b").textContent()).trim(), await fmt(all));
          assert.equal((await tile.locator(".kpi-pick").textContent()).trim(), w.withCache);
          assert.equal(await tile.locator("small.parts > span").count(), 3, "in, out and the cache");
          const acct = p.locator('#usageAccounts .row.stat[data-id="acct-a"] .num b');
          assert.equal((await acct.textContent()).trim(), await fmt(4000000 + 1500000 + 400000000));
          // kept across a reload
          await p.close();
          p = await open();
          assert.equal((await p.locator("#stats .kpi b").first().textContent()).trim(), await fmt(all), "the choice is kept");
          // back to without
          const pick2 = p.locator("#stats .kpi .kpi-pick");
          await reader.inView(p, pick2);
          await pick2.click();
          await p.locator(".proto-menu:visible .pm-item").first().click();
          await p.waitForTimeout(150);
          assert.equal((await p.locator("#stats .kpi b").first().textContent()).trim(), await fmt(io));
          assert.equal((await p.locator('#usageAccounts .row.stat[data-id="acct-a"] .num b').textContent()).trim(), await fmt(5500000));
          // nothing in a tile cut or spilling out of the window
          const spill = await p.locator("#stats .kpi").evaluateAll((ks) => ks.filter((k) => k.scrollWidth > k.clientWidth + 1).map((k) => k.innerText.replace(/\s+/g, " ") + " " + k.scrollWidth + "/" + k.clientWidth));
          assert.deepEqual(spill, [], "a tile spills");
          assert.deepEqual(errors, []);
          await p.close();
        });
      }
      for (const width of [1500, 1100, 420]) {
        await t.test(`${lang} ${width}: the Requests card's cache in full`, async () => {
          const errors = [];
          const ctx = await browser.newContext({ viewport: { width, height: 760 }, reducedMotion: "reduce" });
          t.after(() => ctx.close());
          const p = await ctx.newPage();
          p.setDefaultTimeout(5000);
          p.on("pageerror", (e) => errors.push(e.message));
          await p.route("**/*", server(lang));
          await p.goto("http://magpie.test/");
          await goUsage(p);
          await p.locator(":is(#usageTab .opt, .nav-s):visible").nth(1).click();
          await p.locator("#ledKpi .blk").first().waitFor();
          await p.waitForTimeout(200);
          const fit = await p.locator("#ledKpi .blk").first().evaluate((b) => {
            const sub = b.querySelector(".sub");
            const box = b.getBoundingClientRect(), pad = parseFloat(getComputedStyle(b).paddingRight);
            const parts = [...sub.children].map((s) => ({ text: s.textContent, right: s.getBoundingClientRect().right, w: s.scrollWidth, cw: s.getBoundingClientRect().width }));
            return { text: sub.textContent, clipped: sub.scrollWidth > sub.clientWidth + 1, parts: parts.length, out: parts.filter((x) => x.right > box.right - pad + 1).map((x) => x.text) };
          });
          assert.equal(fit.parts, 3, "in, out and the cache: " + fit.text);
          assert.equal(fit.clipped, false, "the line is cut: " + fit.text);
          assert.deepEqual(fit.out, [], "a part runs past the card");
          assert.match(fit.text, new RegExp((await p.evaluate(() => fmtN(87933000))).replace(/[.]/g, "\\.")), "the cache's whole count is there");
          assert.deepEqual(errors, []);
          await p.close();
        });
      }
    }
  });
}
