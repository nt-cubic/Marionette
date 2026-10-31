// Run with Node's test runner and Playwright on the module path; see README.md.
// The masking alert (Must2333 on X: 我还希望能即时的知道发生了替换，但不是
// 通过随时监控网关的方式): Settings › Usage has "Masking alert" after the
// task done alert, Off to begin with; On saves maskAlert, another setting
// saved later keeps it, Off saves false, and no click moves the page. With
// masking switched off in Plugins › Built-in the row goes with it. Every
// language, wide and narrow, both engines; the API is faked here.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { watchSettingsReads, settingsDrawn, reach } = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");

function server(lang, posts, featuresOff) {
  const fixed = {
    theme: "light", lang, tray: "panel", quotaLeft: false, currency: "usd", dock: false, dockWindow: false, proxy: "",
    redact: true, redactPersonal: false, redactWords: [], codexWarmup: "", claudeWarmup: "", codexWarmAt: "", claudeWarmAt: "",
    trayUsage: "", trayUsageEvery: 3, vision: "", imageGen: "", version: "0.1.900", dir: "~/.config/magpie",
    gateway: "http://127.0.0.1:3425", proxyNow: "none", proxySource: "none", visionModels: [], imageGenModels: [], lanURLs: [],
    fx: { rate: 7.2, at: new Date().toISOString(), stale: false }, turnDoneAlert: 0, featuresOff,
  };
  let cur = fixed;
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false,featuresOff:${JSON.stringify(featuresOff)}};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light" }, fx: fixed.fx });
    if (url.pathname === "/api/settings") {
      if (req.method() === "POST") {
        const body = req.postDataJSON();
        posts.push(body);
        cur = { ...fixed, ...body };
      }
      return json(cur);
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

const words = {
  en: { name: "Masking alert", off: "Off", on: "On",
    sub: "A notification when masking swaps a value for the first time: its kind and how many, never the value itself" },
  zh: { name: "脱敏提醒", off: "关闭", on: "开启", sub: "首次脱敏某个值时通知：只说类型和个数，从不显示值本身" },
  "zh-TW": { name: "脫敏提醒", off: "關閉", on: "開啟", sub: "首次脫敏某個值時通知：只說類型和個數，從不顯示值本身" },
  ja: { name: "マスク通知", off: "オフ", on: "オン", sub: "値を初めてマスクしたときに通知：種類と件数だけで、値そのものは表示しません" },
  de: { name: "Benachrichtigung beim Maskieren", off: "Aus", on: "An",
    sub: "Eine Benachrichtigung, wenn ein Wert zum ersten Mal maskiert wird: Art und Anzahl, nie der Wert selbst" },
};

async function open(engine, width, lang, posts, featuresOff, t, name) {
  const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
  const page = await (await browser.newContext({ viewport: { width, height: 700 }, reducedMotion: "reduce" })).newPage();
  t.after(async () => {
    if (process.env.ARTIFACT_DIR) {
      await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
      await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `${engine}-${lang}-${width}-${name}.png`) });
    }
    await browser.close();
  });
  page.setDefaultTimeout(5000);
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.route("**/*", server(lang, posts, featuresOff));
  await watchSettingsReads(page);
  await page.goto("http://magpie.test/?view=settings&tab=usage");
  await settingsDrawn(page); // rows found are the ones that stay (#1659)
  return { page, errors };
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of Object.keys(words)) {
    for (const width of [900, 380]) {
      test(`${engine} ${lang} ${width}px: the masking alert is turned on and off on the Settings page`, async (t) => {
        const posts = [];
        const { page, errors } = await open(engine, width, lang, posts, [], t, "mask-alert");
        const posted = async (n) => { for (let i = 0; i < 100 && posts.length < n; i++) await page.waitForTimeout(20); assert.equal(posts.length, n, "posts"); return posts[n - 1]; };
        const w = words[lang];
        const opts = page.locator("#maskAlertSegs .opt");
        await opts.first().waitFor();

        assert.equal((await page.locator("#maskAlertRow .name").textContent()).trim(), w.name);
        assert.equal((await page.locator("#maskAlertRow .sub").textContent()).trim(), w.sub);
        assert.deepEqual(await opts.allTextContents(), [w.off, w.on]);
        assert.equal(await page.locator("#maskAlertSegs .opt.on").textContent(), w.off, "off until turned on");
        // it is the row after the task done alert, in the same group
        assert.equal(await page.locator("#turnDoneRow + #maskAlertRow").count(), 1);
        const over = await page.evaluate(() => {
          const row = document.querySelector("#maskAlertRow").getBoundingClientRect();
          return [...document.querySelectorAll("#maskAlertRow *")].filter((e) => e.getBoundingClientRect().right > Math.max(row.right, innerWidth) + 1).map((e) => e.className || e.id);
        });
        assert.deepEqual(over, [], "nothing past the edge");

        const scrolled = () => page.locator("#view-settings").evaluate((v) => v.scrollTop);
        await reach(page, opts.nth(1));
        let top = await scrolled();
        await opts.nth(1).click();
        assert.equal((await posted(1)).maskAlert, true);
        await page.waitForTimeout(300);
        assert.equal(await page.locator("#maskAlertSegs .opt.on").textContent(), w.on);
        assert.equal(await scrolled(), top, "the click moved the page");

        // another setting saved later keeps it
        await reach(page, page.locator("#currencySegs .opt").nth(1));
        top = await scrolled();
        await page.locator("#currencySegs .opt").nth(1).click();
        assert.equal((await posted(2)).maskAlert, true, "kept by another save");
        await page.waitForTimeout(300);
        assert.equal(await scrolled(), top, "no click moved the page");

        await reach(page, opts.nth(0));
        await opts.nth(0).click();
        assert.equal((await posted(3)).maskAlert, false, "Off saves false");
        assert.equal(await page.locator("#maskAlertRow").evaluate((e) => getComputedStyle(e).borderLeftWidth), "0px");
        assert.deepEqual(errors, []);
      });
    }
  }
  test(`${engine}: with masking switched off, its alert goes too`, async (t) => {
    const { page, errors } = await open(engine, 900, "en", [], ["redact"], t, "mask-alert-off");
    await page.locator("#turnDoneSegs .opt").first().waitFor();
    assert.equal(await page.locator("#maskAlertRow").isVisible(), false);
    assert.equal(await page.locator("#turnDoneRow").isVisible(), true);
    assert.deepEqual(errors, []);
  });
}
