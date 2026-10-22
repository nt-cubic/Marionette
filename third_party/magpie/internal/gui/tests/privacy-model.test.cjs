// Run with Node's test runner and Playwright on the module path; see README.md.
// Settings → Privacy's local masking model (#1977): a row under the masking
// switches, there while either is on. On downloads the model, its progress
// shown and a Cancel beside it; once it is on, the row says where it runs.
// Off, a downloaded model can be deleted, after asking. Its words are shown
// in full, in every language, in a wide window and a narrow one, and a
// click moves nothing.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { watchSettingsReads, settingsDrawn } = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");
const size = 1637572192;
const words = {
  en: { name: "Find with a local model", on: "On", gpu: "On, on the GPU (MTL0)" },
  zh: { name: "用本地模型查找", on: "开启", gpu: "已开启，运行在 GPU（MTL0）" },
  "zh-TW": { name: "用本機模型查找", on: "開啟", gpu: "已開啟，執行於 GPU（MTL0）" },
  ja: { name: "ローカルモデルで検出", on: "オン", gpu: "オン、GPU で実行中（MTL0）" },
  de: { name: "Mit lokalem Modell finden", on: "An", gpu: "An, auf der GPU (MTL0)" },
};

// model is the page's /api/privacy-model: a POST is recorded and moves it
// along, as magpie would (downloading, then on and running on the GPU)
function serve(lang, masking, model, posts) {
  const settings = () => ({ lang, theme: "light", redact: masking, redactPersonal: false });
  const state = { agents: [], profiles: [], settings: settings() };
  return async (r) => {
    const url = new URL(r.request().url());
    const json = (data) => r.fulfill({ json: data });
    if (url.pathname === "/boot.js") return r.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return r.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json(state);
    if (url.pathname === "/api/settings") return json(settings());
    if (url.pathname === "/api/privacy-model") {
      if (r.request().method() === "POST") {
        const { do: doing } = JSON.parse(r.request().postData());
        posts.push(doing);
        if (doing === "on") Object.assign(model, { downloading: true, done: 0, total: size, ticks: 0 });
        if (doing === "cancel") Object.assign(model, { downloading: false, done: 0 });
        if (doing === "remove") Object.assign(model, { installed: false, on: false });
      } else if (model.downloading && ++model.ticks >= 2) {
        Object.assign(model, { downloading: false, installed: true, on: true, running: true, gpu: true, device: "MTL0" });
      } else if (model.downloading) {
        model.done = Math.round(size * 0.4);
      }
      const { ticks, ...out } = model;
      return json(out);
    }
    if (url.pathname === "/api/groups") return json({ models: [], groups: [], pools: [] });
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true, url: "http://127.0.0.1:3999" } });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await r.fulfill({ body: await fs.readFile(file), contentType });
  };
}

async function open(engine, lang, width, masking, model, posts) {
  const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
  const page = await (await browser.newContext({ viewport: { width, height: 1400 } })).newPage();
  page.setDefaultTimeout(8000);
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.route("**/*", serve(lang, masking, model, posts));
  await watchSettingsReads(page);
  await page.goto("http://magpie.test/?view=settings&tab=privacy");
  await page.locator("#redactList .row").first().waitFor();
  await settingsDrawn(page);
  return { browser, page, errors };
}

// what the row shows: its words, whether they are cut, and that it is
// inside the list
const look = (page) => page.evaluate(() => {
  const r = document.querySelector("#privacyModelRow"), list = r.closest(".list").getBoundingClientRect();
  const sub = r.querySelector(".sub"), b = r.getBoundingClientRect(), v = r.querySelector(".val").getBoundingClientRect();
  return {
    name: r.querySelector(".name").textContent, sub: sub.textContent,
    on: r.querySelector(".segs .opt.on")?.textContent, buttons: [...r.querySelectorAll("button.text")].map((x) => x.textContent),
    cut: sub.scrollWidth > sub.clientWidth + 1 || sub.scrollHeight > sub.clientHeight + 1,
    inside: b.left >= list.left - 1 && b.right <= list.right + 1 && v.right <= b.right + 1,
    wide: document.documentElement.scrollWidth > document.documentElement.clientWidth,
  };
});

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of Object.keys(words)) {
    for (const width of [1100, 640]) {
      const w = words[lang];
      test(`${engine} ${lang} ${width}px: turned on, the model downloads and says where it runs`, async (t) => {
        const posts = [], model = { supported: true, installed: false, size };
        const { browser, page, errors } = await open(engine, lang, width, true, model, posts);
        t.after(() => browser.close());
        const row = page.locator("#privacyModelRow");
        await row.locator(".segs").waitFor();
        let m = await look(page);
        assert.equal(m.name, w.name);
        assert(m.sub.includes("1.6 GB"), `says what it downloads: ${m.sub}`);
        if (lang !== "en") assert(!/no rule can find|Turning it on/.test(m.sub), `translated: ${m.sub}`);
        assert(!m.cut && m.inside && !m.wide, `shown in full and inside: ${JSON.stringify(m)}`);

        await page.mouse.move(width / 2, 300);
        for (let i = 0; i < 20 && (await row.boundingBox()).y > 900; i++) {
          await page.mouse.wheel(0, 200);
          await page.waitForTimeout(80);
        }
        await page.waitForTimeout(300);
        const before = await page.evaluate(() => [scrollX, scrollY, document.querySelector("#view-settings").scrollTop]);
        const b = await row.locator(".segs .opt", { hasText: w.on }).boundingBox();
        await page.mouse.click(b.x + b.width / 2, b.y + b.height / 2);
        // downloading: its progress, and a Cancel
        await page.waitForFunction(() => /655 MB/.test(document.querySelector("#privacyModelRow .sub").textContent));
        m = await look(page);
        assert.equal(m.buttons.length, 1, "a Cancel while it downloads");
        assert.equal(m.on, w.on);
        assert(!m.cut && m.inside && !m.wide, `shown in full and inside: ${JSON.stringify(m)}`);
        // then on, on the GPU
        await page.waitForFunction((want) => document.querySelector("#privacyModelRow .sub").textContent === want, w.gpu);
        m = await look(page);
        assert.deepEqual(m.buttons, [], "nothing to cancel or delete while it is on");
        assert(!m.cut && m.inside && !m.wide, `shown in full and inside: ${JSON.stringify(m)}`);
        assert.deepEqual(posts, ["on"]);
        assert.deepEqual(await page.evaluate(() => [scrollX, scrollY, document.querySelector("#view-settings").scrollTop]), before, "the click moved nothing");
        assert.deepEqual(errors, []);
      });
    }
  }

  test(`${engine}: a download is cancelled, and a downloaded model deleted after asking`, async (t) => {
    const posts = [], model = { supported: true, installed: false, size };
    const { browser, page, errors } = await open(engine, "en", 1100, true, model, posts);
    t.after(() => browser.close());
    const row = page.locator("#privacyModelRow");
    await row.locator(".segs .opt", { hasText: "On" }).click();
    await row.locator("button.text", { hasText: "Cancel" }).click();
    await page.waitForFunction(() => document.querySelector("#privacyModelRow .segs .opt.on")?.textContent === "Off");
    assert.deepEqual(posts, ["on", "cancel"]);

    Object.assign(model, { installed: true, on: false });
    await page.reload();
    await row.locator("button.text", { hasText: "Delete model" }).click();
    await page.locator("dialog.action-confirm button", { hasText: /^Delete$/ }).click();
    await page.waitForFunction(() => !document.querySelector("#privacyModelRow button.text"));
    assert.deepEqual(posts, ["on", "cancel", "remove"]);
    assert.match((await look(page)).sub, /downloads 1\.6 GB once/);
    assert.deepEqual(errors, []);
  });

  test(`${engine}: while nothing is masked the model row waits under Rules, and none to turn on where it can't run`, async (t) => {
    const { browser, page, errors } = await open(engine, "en", 1100, false, { supported: true, size }, []);
    t.after(() => browser.close());
    await page.locator("#redactList .row", { hasText: "Mask personal data" }).waitFor();
    // How it's found: the rules first, then the model, both waiting on
    // something to mask, with nothing to press
    const found = await page.$$eval("#findList > .row", (rs) => rs.map((r) => [r.id, r.querySelector(".name").textContent, r.querySelector(".sub").textContent, r.querySelectorAll("button").length]));
    assert.deepEqual(found, [
      ["maskRulesRow", "Rules", "Turn on Mask secrets or Mask personal data first", 0],
      ["privacyModelRow", "Find with a local model", "Turn on Mask secrets or Mask personal data first", 0],
    ]);
    assert.equal(await page.locator("#privacyModelRow.pm-wait").count(), 1);
    await browser.close();

    const two = await open(engine, "en", 1100, true, { supported: false, size }, []);
    t.after(() => two.browser.close());
    await two.page.waitForFunction(() => document.querySelector("#privacyModelRow .sub")?.textContent === "Not available on this system yet");
    assert.equal(await two.page.locator("#privacyModelRow .segs").count(), 0);
    assert.deepEqual([...errors, ...two.errors], []);
  });
}
