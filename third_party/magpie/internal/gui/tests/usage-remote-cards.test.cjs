// Run with Node's test runner and Playwright on the module path; see README.md.
// Another computer's quotas (莫 on Discord): a computer whose provider is
// another's magpie (remote-magpie) shows that magpie's cards, each named
// with the computer ("Codex · Office"), without the buttons that would act
// on this computer's accounts. The remote's own card says why it has none
// (nothing read there yet, or a magpie too old to share) in the reader's
// language, and its refresh asks for it by its id (?provider=office), the
// remote's card by its own (?provider=office/codex). English, Chinese,
// Japanese and German; no backend, the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const H = 3600e3, M = 60e3;
const iso = (ms) => new Date(ms).toISOString();

function fixtures(now) {
  return [
    { provider: "codex", name: "Codex", icon: "openai", plan: "Plus", user: "b@x.com", readAt: iso(now - 40 * M),
      windows: [{ name: "5 hours", used: 50, resetsAt: iso(now + 2 * H) }], resets: { count: 1 } },
    { provider: "office/codex", name: "Codex · Office", icon: "openai", plan: "Pro", user: "a@x.com", from: "Office", kind: "subscription", readAt: iso(now - 40 * M),
      windows: [{ name: "5 hours", used: 30, resetsAt: iso(now + 2 * H) }] },
    { provider: "home", name: "Home", icon: "magpie", from: "Home", kind: "subscription", windows: [],
      error: "nothing read on that magpie yet; refresh to have it read" },
    { provider: "attic", name: "Attic", icon: "magpie", from: "Attic", kind: "subscription", windows: [],
      error: "remote magpie doesn't share its quotas; update magpie on that computer" },
    // Fim980 on X: these said only "Allowance unavailable"
    { provider: "kestrel", name: "Kestrel", icon: "magpie", from: "Kestrel", kind: "subscription", windows: [],
      error: "remote magpie isn't shared; turn on Share on local network on that computer" },
    { provider: "osprey", name: "Osprey", icon: "magpie", from: "Osprey", kind: "subscription", windows: [],
      error: "remote magpie didn't take this key; use one of its enabled gateway keys" },
    { provider: "heron", name: "Heron", icon: "magpie", from: "Heron", kind: "subscription", windows: [],
      error: "couldn't reach the remote magpie Heron: dial tcp 10.0.0.9:3425: connect: connection refused" },
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
    if (url.pathname === "/api/usage/quotas") { state.gets++; return json(state.quotas); }
    if (url.pathname === "/api/usage/quotas/refresh") {
      assert.equal(route.request().method(), "POST");
      state.refreshes.push(url.search);
      await state.gate;
      const q = state.quotas.find((x) => x.provider === url.searchParams.get("provider") && (x.user || "") === (url.searchParams.get("user") || ""));
      if (q?.windows?.length) { q.windows[0].used = 61; q.readAt = iso(Date.now()); }
      if (q?.error) { state.quotas.push({ provider: q.provider + "/kimi", name: "Kimi · " + q.name, icon: "kimi-color", from: q.name, readAt: iso(Date.now()), windows: [{ name: "Week", used: 10 }] }); state.quotas.splice(state.quotas.indexOf(q), 1); }
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
  en: { none: "Nothing read on that computer yet — refresh this card to have it read", old: "That computer's magpie doesn't share its quotas yet — update magpie there", shed: "That computer's magpie isn't shared — turn on Settings → Share on local network there", den: "That computer's magpie didn't take this key — use one of its gateway keys in this provider", barn: "Couldn't reach that computer's magpie — check that it is running and its address" },
  zh: { none: "那台电脑还没有读过额度 — 刷新这张卡片让它读取", old: "那台电脑的 magpie 还不能共享额度 — 请在那台电脑上更新 magpie", shed: "那台电脑的 magpie 没有共享 — 请在那台电脑上打开 设置 → 局域网共享", den: "那台电脑的 magpie 不认这个密钥 — 请在这个服务商里填它的一个网关密钥", barn: "连不上那台电脑的 magpie — 请确认它在运行、地址正确" },
  ja: { none: "そのコンピューターではまだ読み取っていません — このカードを更新すると読み取ります", old: "そのコンピューターの magpie はまだ利用枠を共有できません — そちらの magpie を更新してください", shed: "そのコンピューターの magpie は共有されていません — そちらで 設定 → ローカルネットワークで共有 をオンにしてください", den: "そのコンピューターの magpie はこのキーを受け付けませんでした — このプロバイダーにそちらのゲートウェイキーを入れてください", barn: "そのコンピューターの magpie に接続できません — 起動しているか、アドレスが正しいか確認してください" },
  de: { none: "Auf diesem Computer wurde noch nichts gelesen — diese Karte aktualisieren, um es lesen zu lassen", old: "Das magpie dieses Computers teilt seine Kontingente noch nicht — aktualisiere magpie dort", shed: "Das magpie dieses Computers ist nicht freigegeben — schalte dort Einstellungen → Im lokalen Netzwerk freigeben ein", den: "Das magpie dieses Computers hat diesen Schlüssel nicht angenommen — trage bei diesem Anbieter einen seiner Zugangsschlüssel ein", barn: "Das magpie dieses Computers ist nicht erreichbar — prüfe, ob es läuft und die Adresse stimmt" },
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh", "ja", "de"]) {
    const w = words[lang];
    test(`${engine} ${lang}: another computer's quota cards`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const errors = [];
      const page = await (await browser.newContext({ viewport: { width: 900, height: 700 }, reducedMotion: "reduce" })).newPage();
      page.setDefaultTimeout(5000);
      page.on("pageerror", (e) => errors.push(e.message));
      const state = { quotas: fixtures(Date.now()), gets: 0, refreshes: [], gate: Promise.resolve() };
      await page.route("**/*", serve(lang, state));
      await page.goto("http://magpie.test/?view=usage");

      const office = page.locator(".subscription-card", { hasText: "Codex · Office" });
      await office.waitFor();
      assert.match(await office.locator(".quota-n").first().textContent(), /30%/);
      const home = page.locator(".subscription-card", { hasText: "Home" });
      assert.equal((await home.locator(".subscription-error").textContent()).trim(), w.none);
      const attic = page.locator(".subscription-card", { hasText: "Attic" });
      assert.equal((await attic.locator(".subscription-error").textContent()).trim(), w.old);
      for (const [k, name] of [["shed", "Kestrel"], ["den", "Osprey"], ["barn", "Heron"]]) {
        const card = page.locator(".subscription-card", { hasText: name });
        assert.equal((await card.locator(".subscription-error").textContent()).trim(), w[k], k);
      }

      // the remote's card reads again over there, by its own id
      await office.hover();
      await office.locator("button.quota-refresh").click();
      await page.waitForFunction(() => !document.querySelector(".quota-refresh.busy"));
      // nothing read there yet: its refresh has the remote read every card
      await home.hover();
      await home.locator("button.quota-refresh").click();
      await page.locator(".subscription-card", { hasText: "Kimi · Home" }).waitFor();
      assert.deepEqual(state.refreshes, ["?provider=office%2Fcodex&user=a%40x.com", "?provider=home"]);
      assert.equal(await page.locator(".subscription-card", { hasText: "Home" }).locator(".subscription-error").count(), 0);
      assert.deepEqual(errors, []);
    });
  }
}
