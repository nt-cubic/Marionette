// Run with Node's test runner and Playwright on the module path; see README.md.
// #1821: a Volcengine Ark plan, or Tencent's Token Plan, whose provider has
// no access key to read its usage with was no card at all, so a key gone
// from providers.json looked like a card gone for no reason. The card now
// says which access key to give, in the user's language, the reason in its
// tooltip, with a button that opens the provider's settings. No backend,
// the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
// accessKeyCard's words (internal/provider/access_key_card.go)
const VOLC = "this plan's usage is read with the account's access key: give its AccessKey ID and Secret in Volcengine Ark's settings";
const TC = "this plan's usage is read with the account's access key: give its SecretId and SecretKey in Tencent Cloud's settings";

const quotas = [
  { provider: "volcengine", name: "Volcengine Ark", icon: "volcengine", plan: "Coding Plan", error: VOLC, needsAccessKey: true, windows: [] },
  { provider: "tencent-cloud", name: "Tencent Cloud", icon: "tencent", plan: "Token Plan Enterprise", error: TC, needsAccessKey: true, windows: [] },
  { provider: "kimi", name: "Kimi", icon: "kimi-color", error: "HTTP 503", windows: [] },
];
const volc = {
  id: "volcengine", name: "Volcengine Ark", icon: "volcengine", preset: "volcengine", chat: "https://ark.cn-beijing.volces.com/api/coding/v3", responses: "", anthropic: "", catalog: "",
  models: [], agents: [], fallback: [], headers: {}, key: { set: true, masked: "ark-…key" }, keyList: [],
  accessKey: { vendor: "volcengine", id: "", secretSet: false },
};

function serve(lang) {
  const settings = { theme: "light", lang, tray: "panel", quotaLeft: false, currency: "usd" };
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (body) => route.fulfill({ json: body });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings });
    if (url.pathname === "/api/settings") return json(settings);
    if (url.pathname === "/api/providers") return json({ providers: [volc], presets: [], excluded: [], gateway: { running: true, window: true } });
    if (url.pathname === "/api/usage/quotas") return json(quotas);
    if (url.pathname === "/api/usage/quotas/history") return json([]);
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

const words = {
  en: ["This plan's usage needs the account's AccessKey ID and Secret — give them in the provider's settings", "This plan's usage needs the account's SecretId and SecretKey — give them in the provider's settings", "Open settings"],
  zh: ["此套餐的用量需要账号的 AccessKey ID 和 Secret 才能读取，请在服务商设置里填写", "此套餐的用量需要账号的 SecretId 和 SecretKey 才能读取，请在服务商设置里填写", "打开设置"],
  "zh-TW": ["此方案的用量需要帳號的 AccessKey ID 和 Secret 才能讀取，請在服務商設定中填寫", "此方案的用量需要帳號的 SecretId 和 SecretKey 才能讀取，請在服務商設定中填寫", "開啟設定"],
  ja: ["このプランの使用量を読むにはアカウントの AccessKey ID と Secret が必要です。プロバイダの設定で入力してください", "このプランの使用量を読むにはアカウントの SecretId と SecretKey が必要です。プロバイダの設定で入力してください", "設定を開く"],
  de: ["Die Nutzung dieses Tarifs wird mit der AccessKey ID und dem Secret des Kontos gelesen – geben Sie sie in den Einstellungen des Anbieters an", "Die Nutzung dieses Tarifs wird mit der SecretId und dem SecretKey des Kontos gelesen – geben Sie sie in den Einstellungen des Anbieters an", "Einstellungen öffnen"],
};

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const [lang, [volcSaid, tcSaid, open]] of Object.entries(words)) {
    for (const width of [900, 420]) {
      test(`${engine} ${lang} ${width}px: a plan card without its access key says so and opens the settings`, async (t) => {
        const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
        t.after(() => browser.close());
        const errors = [];
        const page = await (await browser.newContext({ viewport: { width, height: 800 }, reducedMotion: "reduce" })).newPage();
        page.setDefaultTimeout(5000);
        page.on("pageerror", (e) => errors.push(e.message));
        await page.route("**/*", serve(lang));
        await page.goto("http://magpie.test/?view=usage");

        const v = page.locator(".subscription-card", { hasText: "Volcengine Ark" });
        const err = v.locator(".subscription-error");
        await err.waitFor();
        assert.equal(await err.getAttribute("title"), VOLC, "the reason in the tooltip");
        assert.ok((await err.textContent()).includes(volcSaid), await err.textContent());
        const go = err.locator(".quota-fix");
        assert.equal((await go.textContent()).trim(), open);
        const tc = page.locator(".subscription-card", { hasText: "Tencent Cloud" }).locator(".subscription-error");
        assert.ok((await tc.textContent()).includes(tcSaid), await tc.textContent());
        // a card that failed for another reason has no such button
        assert.equal(await page.locator(".subscription-card", { hasText: "Kimi" }).locator(".quota-fix").count(), 0);
        const [e, b] = [await err.boundingBox(), await go.boundingBox()];
        assert.ok(e.x + e.width <= width && b.x + b.width <= width, "the error and its button fit the page");

        await go.click();
        await page.waitForURL(/view=providers/);
        assert.equal(new URL(page.url()).searchParams.get("edit"), "volcengine", "the provider's own settings");
        await page.locator(".editor .volc-ak").waitFor();
        assert.deepEqual(errors, []);
      });
    }
  }
}
