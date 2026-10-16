// Run with Node's test runner and Playwright on the module path; see README.md.
// An answer that is a proxy's own web page, not magpie's: magpie web behind
// Cloudflare, an install that outlasted its 100 seconds, and the toast
// showed "<!DOCTYPE html> <!--[if lt IE 7]> <html class="no-js ie…"
// (precisionz installing Zed, #2032). The page says a proxy answered, by
// its title, and for a timeout that magpie may still be finishing; a
// plain-text error is still said as it came. English, Chinese, Japanese
// and German; the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");

// Cloudflare's 5xx page: its head as the reporter's toast shows it, the
// rest as Cloudflare's template has it
const CF524 = `<!DOCTYPE html>
<!--[if lt IE 7]> <html class="no-js ie6 oldie" lang="en-US"> <![endif]-->
<!--[if IE 7]>    <html class="no-js ie7 oldie" lang="en-US"> <![endif]-->
<!--[if IE 8]>    <html class="no-js ie8 oldie" lang="en-US"> <![endif]-->
<!--[if gt IE 8]><!--> <html class="no-js" lang="en-US"> <!--<![endif]-->
<head>
<title>magpie.example.com | 524: A timeout occurred</title>
<meta charset="UTF-8" />
<meta http-equiv="Content-Type" content="text/html; charset=UTF-8" />
<meta http-equiv="X-UA-Compatible" content="IE=Edge" />
<meta name="robots" content="noindex, nofollow" />
<meta name="viewport" content="width=device-width,initial-scale=1" />
<link rel="stylesheet" id="cf_styles-css" href="/cdn-cgi/styles/main.css" />
</head>
<body>
<div id="cf-wrapper">
    <div id="cf-error-details" class="p-0">
        <header class="mx-auto pt-10 lg:pt-6 lg:px-8 w-240 lg:w-full mb-8">
            <h1 class="inline-block sm:block sm:mb-2 font-light text-60 lg:text-4xl text-black-dark leading-tight mr-2">
              <span class="inline-block">A timeout occurred</span>
              <span class="code-label">Error code 524</span>
            </h1>
        </header>
    </div>
</div>
</body>
</html>
`;
const CF502 = CF524.replace("524: A timeout occurred", "502: Bad gateway");

const PKG = "@magpie-community/opencode-zed-auth";

function server(lang, answer) {
  const market = { listings: [], state: { bun: true, bunVersion: "1.3.0", plugins: [], picker: false } };
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" } });
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true }, plugins: [] });
    if (url.pathname === "/api/groups") return json({ groups: [], pools: [] });
    if (url.pathname === "/api/gateway/trace") return json({ routes: [] });
    if (url.pathname === "/api/plugins") return json(market.state);
    if (url.pathname === "/api/plugins/listings") return json({ listings: market.listings });
    if (url.pathname === "/api/plugins/market") return json(market);
    if (url.pathname === "/api/plugins/search") return json({ hits: [] });
    if (url.pathname === "/api/plugins/add") return route.fulfill(answer);
    if (url.pathname.startsWith("/api/")) return json({});
    if (!/^\/[\w./-]*$/.test(url.pathname) || url.host !== "magpie.test") return route.fulfill({ status: 404, body: "" });
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

const what = "magpie.example.com | 524: A timeout occurred";
const bad = "magpie.example.com | 502: Bad gateway";
const L = {
  en: {
    install: "Install",
    timeout: `A proxy in front of magpie stopped waiting; magpie may still be finishing, check again in a minute (${what})`,
    other: `A proxy in front of magpie answered, not magpie (${bad})`,
  },
  zh: {
    install: "安装",
    timeout: `magpie 前面的代理等不及先返回了；magpie 可能仍在处理，请过一分钟再看看（${what}）`,
    other: `这是 magpie 前面的代理返回的，不是 magpie 的回应（${bad}）`,
  },
  ja: {
    install: "インストール",
    timeout: `magpie の前のプロキシが待ちきれずに応答しました。magpie はまだ処理中かもしれません。1 分ほどして確認してください（${what}）`,
    other: `magpie ではなく、その前のプロキシからの応答です（${bad}）`,
  },
  de: {
    install: "Installieren",
    timeout: `Ein Proxy vor magpie hat nicht länger gewartet; magpie arbeitet möglicherweise noch, prüfen Sie es in einer Minute erneut (${what})`,
    other: `Ein Proxy vor magpie hat geantwortet, nicht magpie (${bad})`,
  },
};

async function installOnce(browser, lang, answer) {
  const page = await (await browser.newContext({ viewport: { width: 420, height: 820 } })).newPage();
  page.setDefaultTimeout(5000);
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.route("**/*", server(lang, answer));
  await page.goto("http://magpie.test/?view=plugins");
  const manual = page.locator("#view-plugins .pm-manual");
  await manual.waitFor();
  await manual.locator(".pm-minput").fill(PKG);
  await manual.locator("button", { hasText: new RegExp("^" + L[lang].install + "$") }).click();
  const s = page.locator("#status.err");
  await s.waitFor();
  const said = (await s.textContent()).trim();
  if (process.env.ARTIFACT_DIR) {
    await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
    await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `plugin-proxy-page-${lang}-${answer.status}.png`) });
  }
  await page.context().close();
  return { said, errors };
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(engine + ": a proxy's own page is said by its title", async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: process.env.PLAYWRIGHT_CHANNEL || "chromium" }));
    t.after(() => browser.close());
    for (const lang of Object.keys(L)) {
      await t.test(lang, async () => {
        let r = await installOnce(browser, lang, { status: 524, contentType: "text/html; charset=UTF-8", body: CF524 });
        assert.equal(r.said, L[lang].timeout);
        assert.doesNotMatch(r.said, /<!DOCTYPE|<html/i, "no markup in the toast");
        assert.deepEqual(r.errors, []);
        // a proxy that says no type still sent a page
        r = await installOnce(browser, lang, { status: 502, contentType: "", body: CF502 });
        assert.equal(r.said, L[lang].other);
        assert.deepEqual(r.errors, []);
      });
    }
    // a plain-text error (http.Error) is said as it came
    await t.test("plain text", async () => {
      const r = await installOnce(browser, "en", { status: 500, contentType: "text/plain; charset=utf-8", body: "bun add " + PKG + ": exit status 1\n" });
      assert.equal(r.said, "bun add " + PKG + ": exit status 1");
    });
  });
}
