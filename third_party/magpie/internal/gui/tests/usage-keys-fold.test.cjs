// Run with Node's test runner and Playwright on the module path; see README.md.
// A provider with many keys, each its balance (KeyBalances), shows the first
// three on the Usage page and the rest behind "Show N more keys" at the
// card's foot, with the keys' sum in the card's head (361 on Discord: an
// OpenRouter card listed every key's balance, a very long card). The button
// opens them in place, the page not moving, and folds them again; it is
// remembered across a reload. Every provider with keys folds the same way
// (DeepSeek too); four keys aren't folded (the button would take the room
// the key does); a subscription's accounts aren't keys and stay as they
// were; a key whose balance couldn't be read leaves the sum out. At 440px
// nothing in the card is cut or spills. English and Chinese; no backend,
// the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const now = new Date().toISOString();
const key = (provider, name, icon, user, balance) => ({ provider, name, icon, user, windows: [], ...(balance ? { balance, readAt: now } : { error: "401 Unauthorized" }) });
const pad = (i) => String(i).padStart(2, "0");
const quotas = () => [
  // 12 keys: $1.25 … $15.00, $97.50 in all
  ...Array.from({ length: 12 }, (_, i) => key("openrouter", "OpenRouter", "openrouter", `key-${pad(i + 1)}`, "$" + (1.25 * (i + 1)).toFixed(2))),
  // 5 keys, one unread: folded, no sum
  ...Array.from({ length: 5 }, (_, i) => key("deepseek", "DeepSeek", "deepseek", `ds-${i + 1}`, i === 4 ? "" : "¥" + (10 * (i + 1)).toFixed(2))),
  // 4 keys: not folded, summed
  ...Array.from({ length: 4 }, (_, i) => key("moonshot", "Moonshot", "moonshot", `ms-${i + 1}`, "¥" + (2 * (i + 1)).toFixed(2))),
  // a subscription's 5 accounts: not keys
  ...Array.from({ length: 5 }, (_, i) => ({ provider: "codex", name: "Codex", kind: "subscription", icon: "openai", plan: "Plus", user: `c${i + 1}@example.com`, windows: [{ name: "5 hours", used: 10 * i }] })),
];

function serve(lang) {
  const settings = { theme: "light", lang, tray: "panel", quotaLeft: false, currency: "usd" };
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings });
    if (url.pathname === "/api/settings") return json(settings);
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true } });
    if (url.pathname === "/api/usage/quotas") return json(quotas());
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

const words = {
  en: { more9: "Show 9 more keys", more2: "Show 2 more keys", fewer: "Show fewer keys", sum: "$97.50 in all", sumMs: "¥20.00 in all", why: "The 12 keys' balances added up" },
  zh: { more9: "展开其余 9 个密钥", more2: "展开其余 2 个密钥", fewer: "收起其余密钥", sum: "合计 $97.50", sumMs: "合计 ¥20.00", why: "12 个密钥的余额之和" },
};

// each card: the keys (accounts) in sight, its sum, its foot button
const cards = (page) => page.evaluate(() => Object.fromEntries([...document.querySelectorAll("#subscriptionUsage > .subscription-card")].map((c) => [c.dataset.key, {
  users: [...c.querySelectorAll(".subscription-account .user")].filter((u) => u.offsetParent).map((u) => u.title),
  balances: [...c.querySelectorAll(".quota-balance b")].filter((b) => b.offsetParent).length,
  sum: c.querySelector(".quota-sum")?.textContent ?? null,
  more: c.querySelector(".quota-keys-more")?.textContent ?? null,
  height: Math.round(c.getBoundingClientRect().height),
}])));

// nothing in the card cut short or past its edge
const fits = (page, key) => page.evaluate((key) => {
  const c = document.querySelector(`#subscriptionUsage > [data-key=${key}]`);
  const box = c.getBoundingClientRect();
  const bad = [];
  if (document.documentElement.scrollWidth > innerWidth) bad.push("page scrolls sideways");
  if (c.scrollWidth > c.clientWidth) bad.push("card spills");
  for (const e of c.querySelectorAll(".subscription-head > *, .quota-keys-more, .quota-balance b, .subscription-account .user")) {
    if (!e.offsetParent) continue;
    const r = e.getBoundingClientRect();
    if (r.left < box.left - 0.5 || r.right > box.right + 0.5) bad.push(`${e.className || e.tagName} past the card`);
    if ((e.matches(".quota-sum, .quota-keys-more, .quota-balance b")) && e.scrollWidth > e.clientWidth + 0.5) bad.push(`${e.className || e.tagName} cut: ${e.textContent}`);
  }
  return bad;
}, key);

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh"]) {
    const w = words[lang];
    test(`${engine} ${lang}: many keys fold behind a button, open in place, sum in the head`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const context = await browser.newContext({ viewport: { width: 900, height: 500 }, reducedMotion: "reduce" });
      const page = await context.newPage();
      page.setDefaultTimeout(5000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/*", serve(lang));
      await page.addInitScript(() => { try { if (!sessionStorage.getItem("set")) { localStorage.clear(); localStorage.setItem("magpie.quotaRange", "off"); sessionStorage.setItem("set", "1"); } } catch {} });
      await page.goto("http://magpie.test/?view=usage");
      await page.waitForSelector("#subscriptionUsage > [data-key=openrouter] .subscription-account .user", { state: "attached" });

      let got = await cards(page);
      // folded: the first three keys, the rest behind the button
      assert.deepEqual(got.openrouter.users, ["key-01", "key-02", "key-03"]);
      assert.equal(got.openrouter.balances, 3);
      assert.equal(got.openrouter.more, w.more9);
      assert.equal(got.openrouter.sum, w.sum);
      assert.equal(await page.locator("#subscriptionUsage > [data-key=openrouter] .quota-sum").getAttribute("title"), w.why);
      // the class: DeepSeek's keys fold too; an unread key leaves no sum
      assert.deepEqual(got.deepseek.users, ["ds-1", "ds-2", "ds-3"]);
      assert.equal(got.deepseek.more, w.more2);
      assert.equal(got.deepseek.sum, null);
      // four keys: all in sight, no button
      assert.deepEqual(got.moonshot.users, ["ms-1", "ms-2", "ms-3", "ms-4"]);
      assert.equal(got.moonshot.more, null);
      assert.equal(got.moonshot.sum, w.sumMs);
      // a subscription's accounts are not keys
      assert.equal(got.codex.users.length, 5);
      assert.equal(got.codex.more, null);
      assert.equal(got.codex.sum, null);
      const foldedHeight = got.openrouter.height;

      // opening them doesn't move the page
      const view = page.locator("#view-usage");
      await page.mouse.move(450, 300);
      await page.mouse.wheel(0, 80);
      await page.waitForFunction(() => document.querySelector("#view-usage").scrollTop > 0);
      await page.waitForTimeout(300);
      const more = page.locator("#subscriptionUsage > [data-key=openrouter] .quota-keys-more");
      const head = page.locator("#subscriptionUsage > [data-key=openrouter] .subscription-head");
      const before = await view.evaluate((v) => v.scrollTop);
      const headAt = await head.evaluate((e) => e.getBoundingClientRect().top);
      const box = await more.boundingBox();
      assert.ok(box.y > 0 && box.y + box.height < 500, "the button is in sight");
      await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
      await page.waitForTimeout(500);
      assert.equal(await view.evaluate((v) => v.scrollTop), before, "the view didn't scroll");
      assert.equal(await head.evaluate((e) => e.getBoundingClientRect().top), headAt, "the card stays where it was");
      got = await cards(page);
      assert.deepEqual(got.openrouter.users, Array.from({ length: 12 }, (_, i) => `key-${pad(i + 1)}`));
      assert.equal(got.openrouter.balances, 12);
      assert.equal(got.openrouter.more, w.fewer);
      assert.equal(await more.getAttribute("aria-expanded"), "true");
      assert.ok(got.openrouter.height > foldedHeight, "the card grew");
      assert.equal(got.deepseek.more, w.more2, "another provider stays as it was");

      // remembered across a reload
      await page.reload();
      await page.waitForSelector("#subscriptionUsage > [data-key=openrouter] .subscription-account .user", { state: "attached" });
      got = await cards(page);
      assert.equal(got.openrouter.users.length, 12);

      // and folded again from its foot
      const fewer = page.locator("#subscriptionUsage > [data-key=openrouter] .quota-keys-more");
      // wheeled to, as a reader would (the page holds still for a script's scroll)
      await page.mouse.move(450, 300);
      for (let i = 0; i < 20 && (await fewer.boundingBox()).y > 400; i++) {
        await page.mouse.wheel(0, 120);
        await page.waitForTimeout(150);
      }
      await page.waitForTimeout(300);
      const at = await head.evaluate((e) => e.getBoundingClientRect().top);
      const fb = await fewer.boundingBox();
      assert.ok(fb.y > 0 && fb.y + fb.height < 500, "the button is in sight");
      await page.mouse.click(fb.x + fb.width / 2, fb.y + fb.height / 2);
      await page.waitForTimeout(500);
      got = await cards(page);
      assert.deepEqual(got.openrouter.users, ["key-01", "key-02", "key-03"]);
      assert.equal(got.openrouter.more, w.more9);
      assert.equal(await head.evaluate((e) => e.getBoundingClientRect().top), at, "the card's head stays where it was");

      // 440px: nothing cut or spilling, folded and open
      await page.setViewportSize({ width: 440, height: 700 });
      await page.waitForTimeout(300);
      for (const k of ["openrouter", "deepseek", "moonshot"]) assert.deepEqual(await fits(page, k), [], `${k} fits at 440px, folded`);
      // (pressed where it is: the page holds still for Playwright's scroll to it)
      await page.locator("#subscriptionUsage > [data-key=openrouter] .quota-keys-more").evaluate((b) => b.click());
      await page.waitForFunction(() => document.querySelectorAll("#subscriptionUsage > [data-key=openrouter] .subscription-account").length === 12);
      assert.deepEqual(await fits(page, "openrouter"), [], "openrouter fits at 440px, open");
      assert.deepEqual(errors, []);
    });
  }
}
