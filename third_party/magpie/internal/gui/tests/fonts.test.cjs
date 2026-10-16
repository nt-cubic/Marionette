// Installed desktop fonts: real assets with an isolated API, in both engines.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { watchSettingsReads, settingsDrawn } = require("./reader.cjs");
const assets = process.env.MAGPIE_FONT_ASSETS || path.resolve(__dirname, "../assets");
const face = (family, name = "Regular", weight = 400, style = "normal", stretch = 100) => ({ family, name, weight, style, stretch });
const harmony = [face("HarmonyOS Sans SC"), face("HarmonyOS Sans SC", "Medium", 500), face("HarmonyOS Sans SC", "Bold", 700)];
const mono = [face("Consolas"), face("Consolas", "Italic", 400, "italic")];
const odd = face('Reader\'s "中文" \\ family');
const catalogue = [
  { name: harmony[0].family, styles: harmony }, { name: mono[0].family, styles: mono }, { name: odd.family, styles: [odd] },
  ...Array.from({ length: 200 }, (_, i) => ({ name: "Test Family " + i, styles: [face("Test Family " + i)] })),
];

async function open(browser, lang = "en", options = {}) {
  const context = await browser.newContext({ locale: lang === "zh" ? "zh-CN" : "en-US", viewport: options.viewport || { width: 1000, height: 820 } });
  const api = { posts: [], reads: 0, fonts: structuredClone(catalogue), fontError: false, saveError: false, delay: 0,
    cur: { lang, theme: "light", textSize: 100, currency: "usd", tray: "panel", version: "0.1.0", visionModels: [], imageGenModels: [], ...options.settings } };
  const page = await context.newPage();
  page.setDefaultTimeout(5000);
  const errors = [];
  page.on("pageerror", e => { errors.push(e.message); console.error(e.message); });
  await page.route("**/*", async route => {
    const req = route.request(), url = new URL(req.url());
    const json = data => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs=${JSON.stringify({ ...api.cur, web: !!options.web, ...(options.omarchy ? { omarchy: { mode: "dark", stamp: "one", name: "Test theme", vars: { "--font": '"Theme Font", sans-serif', "--code": '"Theme Mono", monospace' } } } : {}) })};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/fonts") {
      api.reads++;
      return api.fontError ? route.fulfill({ status: 503, body: "could not read installed fonts" }) : json(api.fonts);
    }
    if (url.pathname === "/api/settings") {
      if (req.method() === "POST") {
        const body = req.postDataJSON();
        api.posts.push(body);
        if (api.delay) await new Promise(r => setTimeout(r, api.delay));
        if (api.saveError) return route.fulfill({ status: 400, body: "test save failed" });
        api.cur = { ...api.cur, ...body };
      }
      return json(api.cur);
    }
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: api.cur });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/window/fit") return route.fulfill({ status: 204 });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { return await route.fulfill({ body: await fs.readFile(file), contentType }); }
    catch { return route.fulfill({ status: 404, body: "not found" }); }
  });
  await watchSettingsReads(page);
  await page.goto("http://magpie.test/?view=settings&tab=general");
  await settingsDrawn(page); // rows found are the ones that stay (#1659)
  await page.locator("#themeSegs .opt").first().waitFor({ state: "attached" });
  // A real assertion, also usable against the original assets, rather than
  // a timeout or a compile error as the negative-control result.
  assert.equal(await page.locator("#uiFontRow").count(), 1, "Settings must offer an interface font picker");
  if (!options.web) await page.waitForFunction(() => fontList !== null || fontListError);
  await page.evaluate(async () => {
    await document.fonts.ready;
    await Promise.all(document.getAnimations().map(a => a.finished.catch(() => {})));
  });
  return { page, api, errors, close: () => context.close() };
}

async function family(page, row, name) {
  await page.locator(`#${row} .font-family`).click();
  await page.locator(".font-menu .sc-q").fill(name);
  await page.locator(".font-menu .pm-item").filter({ has: page.locator(".pm-name", { hasText: name }) }).first().click();
}
async function style(page, row, name) {
  await page.locator(`#${row} .font-style`).click();
  await page.locator(".font-menu .pm-item").filter({ has: page.locator(".pm-name", { hasText: name }) }).click();
}
async function reset(page, row) {
  await page.locator(`#${row} .font-family`).click();
  await page.locator(".font-menu .pm-item").first().click();
}
const settled = page => page.waitForFunction(() => prefsBusy === 0 && Object.keys(fontDrafts).length === 0);
const css = (page, selector) => page.locator(selector).evaluate(e => {
  const s = getComputedStyle(e);
  return { family: s.fontFamily, weight: s.fontWeight, style: s.fontStyle, stretch: s.fontStretch };
});
const engines = [["chromium", chromium], ["webkit", webkit]].filter(([name]) => !process.env.BROWSER || process.env.BROWSER === name);

for (const [engine, runtime] of engines) {
  for (const lang of ["en", "zh"]) test(`${engine} ${lang}: independent installed fonts, real styles, persistence and reset`, async () => {
    const browser = await runtime.launch();
    try {
      const { page, api, errors } = await open(browser, lang);
      const before = await css(page, "#codeFontRow .font-preview");
      assert.equal(await page.locator("select").count(), 0);
      await family(page, "uiFontRow", "HarmonyOS Sans SC");
      await style(page, "uiFontRow", "Medium");
      await settled(page);
      assert.equal(api.cur.uiFont.weight, 500);
      assert.match((await css(page, "body")).family, /^"?HarmonyOS Sans SC/);
      assert.equal((await css(page, "body")).weight, "500");
      assert.deepEqual(await css(page, "#codeFontRow .font-preview"), before);
      await family(page, "codeFontRow", "Consolas");
      await style(page, "codeFontRow", "Italic");
      await settled(page);
      assert.equal((await css(page, "#codeFontRow .font-preview")).style, "italic");
      assert.equal((await css(page, "body")).style, "normal");
      // Other saved preferences must carry both choices through the queue.
      await page.locator("#themeSegs .opt").nth(2).click();
      await settled(page);
      assert.equal(api.cur.uiFont.weight, 500);
      assert.equal(api.cur.codeFont.style, "italic");
      await page.reload();
      await page.locator("#uiFontRow .font-family").waitFor();
      await page.waitForFunction(() => fontList !== null && !fontFlight);
      await page.evaluate(async () => { await document.fonts.ready; await Promise.all(document.getAnimations().map(a => a.finished.catch(() => {}))); });
      assert.equal((await css(page, "body")).weight, "500");
      assert.equal((await css(page, "#codeFontRow .font-preview")).style, "italic");
      await family(page, "uiFontRow", "Consolas");
      await settled(page);
      assert.equal(api.cur.uiFont.name, "Regular", "a family without Medium chooses a regular face");
      await reset(page, "uiFontRow");
      await settled(page);
      assert.equal(api.cur.uiFont, null);
      assert.equal(api.cur.codeFont.style, "italic");
      await reset(page, "codeFontRow");
      await settled(page);
      assert.deepEqual(await css(page, "#codeFontRow .font-preview"), before);
      assert.deepEqual(errors, []);
    } finally { await browser.close(); }
  });

  test(`${engine}: queued choices and failed writes restore the saved face`, async () => {
    const browser = await runtime.launch();
    try {
      const { page, api, errors } = await open(browser);
      api.delay = 250;
      await family(page, "uiFontRow", "HarmonyOS Sans SC");
      await style(page, "uiFontRow", "Medium");
      await family(page, "codeFontRow", "Consolas");
      await settled(page);
      assert.equal(api.cur.uiFont.name, "Medium");
      assert.equal(api.cur.codeFont.family, "Consolas");
      api.saveError = true;
      await style(page, "uiFontRow", "Bold");
      await settled(page);
      assert.equal((await css(page, "body")).weight, "500");
      assert.equal(await page.locator("#uiFontRow .font-style span").textContent(), "Medium");
      assert.deepEqual(errors, []);
    } finally { await browser.close(); }
  });

  test(`${engine}: refresh errors, missing faces, empty lists and literal names`, async () => {
    const browser = await runtime.launch();
    try {
      const { page, api, errors } = await open(browser, "en", { settings: { uiFont: harmony[1] } });
      api.fontError = true;
      await page.locator("#uiFontRow .font-family").click();
      await page.getByRole("menuitemradio", { name: "Refresh fonts" }).click();
      await page.waitForFunction(() => fontListError);
      assert.equal((await css(page, "body")).weight, "500", "failed discovery must not erase the last collection");
      api.fontError = false;
      api.fonts = [{ name: "Consolas", styles: mono }, { name: odd.family, styles: [odd] }];
      await page.locator("#uiFontRow .font-family").click();
      await page.getByRole("menuitemradio", { name: /Refresh fonts/ }).click();
      await page.waitForFunction(() => !fontFlight && !fontListError);
      assert.match(await page.locator("#uiFontRow .font-note").textContent(), /unavailable/);
      assert.doesNotMatch((await css(page, "body")).family, /HarmonyOS/);
      assert.equal(api.cur.uiFont.family, harmony[0].family, "missing font choice stays saved");
      await family(page, "uiFontRow", odd.family);
      await settled(page);
      assert.equal(await page.locator("#uiFontRow .font-family span").textContent(), odd.family);
      assert.match((await css(page, "body")).family, /Reader/);
      api.fonts = [];
      await page.locator("#uiFontRow .font-family").click();
      await page.getByRole("menuitemradio", { name: "Refresh fonts" }).click();
      await page.waitForFunction(() => !fontFlight && fontList.length === 0);
      await reset(page, "uiFontRow");
      await settled(page);
      assert.equal(api.cur.uiFont, null);
      assert.match(await page.locator("#uiFontRow .font-note").textContent(), /No installed fonts/);
      assert.deepEqual(errors, []);
    } finally { await browser.close(); }
  });

  // #2439: the pointer resting on the menu while a search narrows it got a
  // mouseenter from the redraw, which took the focus out of the field, so
  // Enter chose "Refresh fonts" (the item now under the pointer).
  test(`${engine}: a search narrowing the menu under a still pointer keeps the typing in its field`, async () => {
    const browser = await runtime.launch();
    try {
      const { page, api, errors } = await open(browser, "zh", { viewport: { width: 560, height: 620 } });
      await page.locator("#uiFontRow .font-family").click();
      const second = await page.locator(".font-menu .pm-item").nth(1).boundingBox();
      await page.mouse.move(second.x + second.width / 2, second.y + second.height / 2);
      await page.locator(".font-menu .sc-q").fill("HarmonyOS");
      // the item now under the pointer is Refresh fonts. The redraw's
      // mouseenter comes on the engine's own hover timer (WebKit's is
      // later than a frame), and what is checked is that nothing came, so
      // there is no state to wait for: 300ms is past both engines' timers.
      await page.waitForTimeout(300);
      assert.equal(await page.evaluate(() => document.activeElement.className), "sc-q", "the search field keeps the focus");
      const reads = api.reads;
      await page.keyboard.press("Enter");
      await settled(page);
      assert.equal(await page.locator("#uiFontRow .font-family span").textContent(), "HarmonyOS Sans SC");
      assert.equal(api.reads, reads, "Enter didn't refresh the font list");
      // a pointer that moves still takes an item's focus
      await page.locator("#uiFontRow .font-family").click();
      const item = page.locator(".font-menu .pm-item").nth(2);
      const box = await item.boundingBox();
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2 + 1);
      assert.equal(await item.evaluate(e => e === document.activeElement), true, "a moved pointer focuses the item it's on");
      assert.deepEqual(errors, []);
    } finally { await browser.close(); }
  });

  test(`${engine}: font menu search and keys fit narrow windows without scrolling the page`, async () => {
    const browser = await runtime.launch();
    try {
      const { page, errors } = await open(browser, "zh", { viewport: { width: 560, height: 620 } });
      const top = await page.locator("#view-settings").evaluate(e => e.scrollTop);
      await page.locator("#uiFontRow .font-family").click();
      const menu = page.locator(".font-menu");
      const box = await menu.boundingBox();
      assert(box.x >= 0 && box.x + box.width <= 560 && box.y >= 0 && box.y + box.height <= 620);
      await menu.hover();
      await page.mouse.wheel(0, 240);
      await page.waitForFunction(() => document.querySelector(".font-menu").scrollTop > 0);
      assert.equal(await page.locator("#view-settings").evaluate(e => e.scrollTop), top);
      await page.locator(".font-menu .sc-q").fill("HarmonyOS");
      await page.keyboard.press("Enter");
      await settled(page);
      assert.equal(await page.locator("#uiFontRow .font-family span").textContent(), "HarmonyOS Sans SC");
      assert.equal(await page.locator("#view-settings").evaluate(e => e.scrollTop), top);
      await page.locator("#uiFontRow .font-style").click();
      await page.keyboard.press("ArrowDown");
      await page.keyboard.press("Enter");
      await settled(page);
      assert.equal(await page.locator("#uiFontRow .font-style span").textContent(), "Medium");
      await page.locator("#uiFontRow .font-style").click();
      await page.keyboard.press("Escape");
      assert.equal(await page.locator(".font-menu").count(), 0);
      for (const width of [440, 560, 1000]) {
        await page.setViewportSize({ width, height: 820 });
        assert(await page.locator("#view-settings").evaluate(e => e.scrollWidth <= e.clientWidth));
        for (const row of ["uiFontRow", "codeFontRow"]) {
          const bounds = await page.locator(`#${row} .font-controls`).boundingBox();
          assert(bounds.x >= 0 && bounds.x + bounds.width <= width, `${row} at ${width}`);
        }
      }
      if (process.env.ARTIFACT_DIR) {
        await fs.mkdir(process.env.ARTIFACT_DIR, { recursive: true });
        await style(page, "uiFontRow", "Medium");
        await family(page, "codeFontRow", "Consolas");
        await settled(page);
        await page.screenshot({ path: path.join(process.env.ARTIFACT_DIR, `fonts-${engine}-zh.png`) });
      }
      assert.deepEqual(errors, []);
    } finally { await browser.close(); }
  });

  test(`${engine}: explicit fonts override Omarchy and browser mode keeps its defaults`, async () => {
    const browser = await runtime.launch();
    try {
      const { page, errors } = await open(browser, "en", { omarchy: true, settings: { uiFont: harmony[1], codeFont: mono[0] } });
      assert.match((await css(page, "body")).family, /HarmonyOS/);
      await reset(page, "uiFontRow");
      await settled(page);
      assert.match((await css(page, "body")).family, /Theme Font/);
      assert.match((await css(page, "#codeFontRow .font-preview")).family, /Consolas/);
      assert.deepEqual(errors, []);
      const web = await open(browser, "en", { web: true, settings: { uiFont: harmony[1] } });
      assert.equal(await web.page.locator("#uiFontRow").isVisible(), false);
      assert.equal(web.api.reads, 0);
      assert.doesNotMatch((await css(web.page, "body")).family, /HarmonyOS/);
      assert.deepEqual(web.errors, []);
    } finally { await browser.close(); }
  });

  test(`${engine}: native notifications update the other webview without another save`, async () => {
    const browser = await runtime.launch();
    try {
      const { page, api, errors } = await open(browser);
      api.cur.uiFont = harmony[1];
      api.cur.codeFont = mono[1];
      await page.evaluate(s => window.receiveFonts(s), api.cur);
      assert.equal((await css(page, "body")).weight, "500");
      assert.equal((await css(page, "#codeFontRow .font-preview")).style, "italic");
      assert.equal(await page.locator("#uiFontRow .font-style span").textContent(), "Medium");
      assert.equal(api.posts.length, 0, "a notification must not echo another save");
      api.cur.uiFont = null;
      await page.evaluate(s => window.receiveFonts(s), api.cur);
      assert.doesNotMatch((await css(page, "body")).family, /HarmonyOS/);
      assert.equal((await css(page, "#codeFontRow .font-preview")).style, "italic");
      // A local queued choice wins over an older notice sent before its reply.
      api.delay = 150;
      await family(page, "uiFontRow", "HarmonyOS Sans SC");
      await page.evaluate(s => window.receiveFonts(s), { uiFont: mono[0], codeFont: null });
      await settled(page);
      assert.match((await css(page, "body")).family, /HarmonyOS/);
      assert.equal((await css(page, "#codeFontRow .font-preview")).style, "italic");
      assert.deepEqual(errors, []);
    } finally { await browser.close(); }
  });

  test(`${engine}: named variable traits retain fractional weight and custom width`, async () => {
    const browser = await runtime.launch();
    try {
      const { page, api, errors } = await open(browser);
      const book = face("Variable Sans", "Book", 425.5, "normal", 25);
      api.fonts.push({ name: book.family, styles: [book] });
      await page.locator("#uiFontRow .font-family").click();
      await page.getByRole("menuitemradio", { name: "Refresh fonts" }).click();
      await page.waitForFunction(() => !fontFlight && fontList.some(f => f.name === "Variable Sans"));
      await family(page, "uiFontRow", book.family);
      await settled(page);
      assert.equal(api.cur.uiFont.weight, 425.5);
      assert.equal(api.cur.uiFont.stretch, 25);
      assert.equal((await css(page, "body")).weight, "425.5");
      assert.equal((await css(page, "body")).stretch, "25%");
      assert.deepEqual(errors, []);
    } finally { await browser.close(); }
  });
}
