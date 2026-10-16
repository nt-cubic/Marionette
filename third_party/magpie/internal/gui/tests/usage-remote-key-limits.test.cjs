// Run with Node's test runner and Playwright on the module path; see README.md.
// A remote magpie's gateway keys (#1547, PennyVibe): a computer whose
// provider is another's magpie, through gateway keys that magpie gave it,
// shows each key's own limit on one card named for the keys ("Gateway
// keys · Office"), a section per key: its tokens and spend against its
// caps, named for the period, when they reset, a cycle of days, its calls
// and that it is used up; a key with no limit says "Unlimited", and a
// magpie too old to tell says so, in the reader's language. English,
// Chinese (both), Japanese and German, wide and at a phone's width; no
// backend, the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const H = 3600e3, M = 60e3;
const iso = (ms) => new Date(ms).toISOString();

function fixtures(now) {
  const card = { provider: "office/~keys", name: "Office", icon: "magpie", from: "Office", kind: "plan" };
  return [
    { ...card, user: "laptop", readAt: iso(now - 2 * M), lastServedAt: iso(now - M),
      gatewayKey: { limited: true, period: "day", start: iso(now - 10 * H), reset: iso(now + 5 * H), calls: 3, tokensLeft: 8950, costLeft: 1.54 },
      windows: [
        { name: "Tokens today", used: 10.5, amount: 1050, limit: 10000, unit: "tokens", resetsAt: iso(now + 5 * H) },
        { name: "Spend today", used: 23, amount: 0.46, limit: 2, unit: "USD", resetsAt: iso(now + 5 * H) },
      ] },
    { ...card, user: "desk", readAt: iso(now - 2 * M), gatewayKey: { limited: false }, windows: [{ name: "Limit", used: 0, unlimited: true }] },
    { ...card, user: "build-box-with-a-rather-long-key-name", readAt: iso(now - 2 * M),
      gatewayKey: { limited: true, period: "days", days: 7, reset: iso(now + 50 * H), calls: 2, tokensLeft: 0, spent: true },
      windows: [{ name: "Tokens this cycle", used: 100, amount: 5000, limit: 5000, unit: "tokens", resetsAt: iso(now + 50 * H) }] },
    { ...card, user: "spare", windows: [], error: "remote magpie is too old to tell this key's limit; update magpie on that computer" },
  ];
}

function serve(lang, state) {
  const settings = { theme: "light", lang, tray: "panel", quotaLeft: false, currency: "usd" };
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (body) => route.fulfill({ json: body });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings });
    if (url.pathname === "/api/settings") return json(settings);
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true } });
    if (url.pathname === "/api/usage/quotas") return json(state.quotas);
    if (url.pathname === "/api/usage/quotas/refresh") {
      state.refreshes.push(url.search);
      return json(state.quotas);
    }
    if (url.pathname === "/api/usage/quotas/history") return json([]);
    if (url.pathname === "/api/usage") return json({ calls: 0, cost: 0, days: [], agents: [], models: [], keys: [] });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

const words = {
  en: { head: "Gateway keys · Office", tokens: "Tokens today", spend: "Spend today", count: "1,050 / 10,000 tokens", cycle: "Tokens this cycle", unlimited: "Unlimited",
    line: "7-day cycle · 2 calls · Used up: that computer refuses this key until it resets", calls: "3 calls", old: "That computer's magpie is too old to tell this key's limit — update magpie there" },
  zh: { head: "网关密钥 · Office", tokens: "今日 Token", spend: "今日花费", count: "1,050 / 10,000 token", cycle: "本周期 Token", unlimited: "无限",
    line: "7 天周期 · 2 次调用 · 已用完：那台电脑在重置前会拒绝这个密钥", calls: "3 次调用", old: "那台电脑的 magpie 版本太旧，无法告知这个密钥的限额 — 请在那边更新 magpie" },
  "zh-TW": { head: "閘道金鑰 · Office", tokens: "今日 Token", spend: "今日花費", count: "1,050 / 10,000 token", cycle: "本週期 Token", unlimited: "無限",
    line: "7 天週期 · 2 次呼叫 · 已用完：那臺電腦在重設前會拒絕這個金鑰", calls: "3 次呼叫", old: "那臺電腦的 magpie 版本太舊，無法告知這個金鑰的限額 — 請在那邊更新 magpie" },
  ja: { head: "ゲートウェイキー · Office", tokens: "今日のトークン", spend: "今日の費用", count: "1,050 / 10,000 トークン", cycle: "この周期のトークン", unlimited: "無制限",
    line: "7 日の周期 · 2 回の呼び出し · 使い切りました：リセットまでそのコンピューターはこのキーを拒否します", calls: "3 回の呼び出し", old: "そのコンピューターの magpie は古く、このキーの上限を伝えられません — そちらで magpie を更新してください" },
  de: { head: "Zugangsschlüssel · Office", tokens: "Token heute", spend: "Ausgaben heute", count: "1,050 / 10,000 Token", cycle: "Token in diesem Zyklus", unlimited: "Unbegrenzt",
    line: "7-Tage-Zyklus · 2 Aufrufe · Aufgebraucht: Dieser Computer lehnt den Schlüssel bis zum Zurücksetzen ab", calls: "3 Aufrufe", old: "Das magpie auf diesem Computer ist zu alt, um das Limit dieses Schlüssels zu nennen — aktualisiere magpie dort" },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
    for (const width of [900, 400]) {
      const w = words[lang];
      test(`${engine} ${lang} ${width}px: a remote magpie's gateway keys`, async (t) => {
        const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
        t.after(() => browser.close());
        const errors = [];
        const page = await (await browser.newContext({ viewport: { width, height: 800 }, reducedMotion: "reduce" })).newPage();
        page.setDefaultTimeout(5000);
        page.on("pageerror", (e) => errors.push(e.message));
        const state = { quotas: fixtures(Date.now()), refreshes: [] };
        await page.route("**/*", serve(lang, state));
        await page.goto("http://magpie.test/?view=usage");

        const card = page.locator(".subscription-card", { hasText: w.head });
        await card.waitFor();
        assert.equal((await card.locator(".subscription-head b").textContent()).trim(), w.head);
        // every key in full
        const more = card.locator(".quota-more");
        if (await more.getAttribute("aria-expanded") === "false") await more.click();
        await page.waitForFunction(() => !document.querySelector('.subscription-card[data-provider="office/~keys"] .quota-windows.brief'));

        // the key used last: its caps, named for the day, with when they reset
        const labels = card.locator(".quota-windows").first().locator(".quota-labels");
        assert.equal((await labels.nth(0).locator("span").first().textContent()).trim(), w.tokens);
        const n0 = await labels.nth(0).locator(".quota-n").textContent();
        assert.ok(n0.startsWith(w.count + " · ") && n0.includes("10.5%"), n0);
        assert.equal((await labels.nth(1).locator("span").first().textContent()).trim(), w.spend);
        assert.match(await labels.nth(1).locator(".quota-n").textContent(), /0\.46 \/ 2 USD/);
        assert.equal(await card.locator(".quota-windows").first().locator(".quota-reset").count(), 2);
        const lines = card.locator(".quota-keyline");
        assert.equal(await lines.count(), 2);
        assert.equal((await lines.nth(0).textContent()).trim(), w.calls);
        // a key with no limit says so, and has no bar
        const desk = card.locator(".quota-windows").nth(1);
        assert.equal((await desk.locator(".quota-n").textContent()).trim(), w.unlimited);
        assert.equal(await desk.locator(".quota-track").count(), 0);
        // a cycle of days, used up
        assert.equal((await card.locator(".quota-windows").nth(2).locator(".quota-labels span").first().textContent()).trim(), w.cycle);
        assert.equal((await lines.nth(1).textContent()).trim(), w.line);
        assert.match(await lines.nth(1).getAttribute("class"), /spent/);
        // a magpie too old to tell
        assert.equal((await card.locator(".subscription-error").textContent()).trim(), w.old);

        // Used / Left turns the key's count to what is left
        await page.locator("#quotaMode .opt").nth(1).click();
        assert.match(await labels.nth(0).locator(".quota-n").textContent(), /8,950 \/ 10,000/);

        // a key's refresh asks for the keys' card by that key's name
        // scrolled to by the reader (a click holds the page where it was)
        await page.mouse.move(width / 2, 400);
        await page.mouse.wheel(0, 3000);
        await page.waitForTimeout(300);
        await card.locator(".subscription-error").hover();
        await card.locator(".subscription-error button.quota-refresh").click();
        await page.waitForFunction(() => !document.querySelector(".quota-refresh.busy"));
        assert.deepEqual(state.refreshes, ["?provider=office%2F~keys&user=spare"]);
        // nothing wider than the page, at a phone's width too
        const over = await page.evaluate(() => document.scrollingElement.scrollWidth - innerWidth);
        assert.ok(over <= 0, `page scrolls sideways by ${over}px`);
        const box = await card.boundingBox();
        assert.ok(box.x >= 0 && box.x + box.width <= width + 0.5, `card at ${box.x}+${box.width} in ${width}`);
        assert.deepEqual(errors, []);
      });
    }
  }
}
