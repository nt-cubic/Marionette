// Run with Node's test runner and Playwright on the module path; see README.md.
// A Claude subscription's editor says whether its requests run Claude
// Code's interactive mode (TUI) or claude -p, and hovering it shows what
// each is and what each is good for (the owner, 10-31). Here:
// - on, its last request interactive: a green pill naming the interactive
//   mode, and the last request said;
// - on, its last request -p for a JSON schema: why that one was -p;
// - turned off: the pill says claude -p and why, and Change in Settings
//   opens Settings › Models at the row that turns it on;
// - hovering or focusing the pill shows both modes, the one in use marked,
//   each with its advantage, inside the window; moving away hides it;
// - Claude Code in WSL whose home wasn't found says so;
// - Interactive only: a request it can't take fails, and the card says
//   nothing runs as -p; where it can't run, the pill names Interactive
//   only (never claude -p), says Claude requests fail and why, and
//   Change in Settings goes to the row;
// - another subscription (Codex), and Only switch, which serves nothing,
//   have no such row, and /api/claude/mode isn't read for them.
// The mode is read from /api/claude/mode once as the editor opens.
// Chromium and WebKit, 900 and 420 wide, in every language, the API faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const ago = (min) => new Date(Date.now() - min * 60e3).toISOString();
const USER = "me@example.invalid";
const claude = (switchOnly) => ({ id: "claude", name: "Claude Code", icon: "claude", models: [{ id: "claude-opus-5-5", on: true }], agents: [], key: {}, switchOnly,
  account: { agent: "claude", agentName: "Claude Code", agentIcon: "claude", user: USER, logins: [{ user: USER, plan: "max", active: true, on: true }] } });
const codex = { id: "codex", name: "Codex", icon: "codex", models: [{ id: "gpt-6", on: true }], agents: [], key: {},
  account: { agent: "codex", agentName: "Codex", agentIcon: "codex", user: USER, logins: [{ user: USER, plan: "plus", active: true, on: true }] } };

const RUNS = {
  tui: { tui: true, last: { tui: true, at: ago(3) } },
  schema: { tui: true, last: { tui: false, why: "schema", at: ago(3) } },
  off: { tui: false, why: "off", last: { tui: false, why: "off", at: ago(3) } },
  wsl: { tui: false, why: "wsl-home", wsl: true },
  only: { tui: true, only: true },
  onlyBroken: { tui: false, why: "broken", only: true },
};

// each language's words, as the i18n table has them
const W = {
  en: { row: "Claude Code runs as", tui: "Interactive mode (TUI)", inUse: "In use", lastTui: "interactive mode", schema: "the request asked for a JSON schema", off: "the interactive mode is off in Settings", change: "Change in Settings", wsl: "WSL" },
  zh: { row: "Claude Code 运行方式", tui: "交互模式（TUI）", inUse: "使用中", lastTui: "交互模式", schema: "请求要求按 JSON schema 回复", off: "设置里关闭了交互模式", change: "在设置中修改", wsl: "WSL" },
  "zh-TW": { row: "Claude Code 執行方式", tui: "互動模式（TUI）", inUse: "使用中", lastTui: "互動模式", schema: "請求要求依 JSON schema 回覆", off: "設定裡關閉了互動模式", change: "在設定中修改", wsl: "WSL" },
  ja: { row: "Claude Code の実行方式", tui: "対話モード（TUI）", inUse: "使用中", lastTui: "対話モード", schema: "リクエストが JSON schema での応答を求めました", off: "設定で対話モードがオフになっています", change: "設定で変更", wsl: "WSL" },
  de: { row: "Claude Code läuft als", tui: "Interaktiver Modus (TUI)", inUse: "In Verwendung", lastTui: "interaktiver Modus", schema: "die Anfrage verlangte ein JSON-Schema", off: "der interaktive Modus ist in den Einstellungen aus", change: "In den Einstellungen ändern", wsl: "WSL" },
};

function serve(lang, run, switchOnly, reads) {
  return async (route) => {
    const url = new URL(route.request().url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:false};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [], profiles: [], settings: { lang, theme: "light", claudePrintMode: run.why === "off" } });
    if (url.pathname === "/api/providers") return json({ providers: [claude(switchOnly), codex], presets: [], excluded: [], gateway: { running: true, window: true }, plugins: [] });
    if (url.pathname === "/api/claude/mode") { reads.n++; return json(run); }
    if (url.pathname === "/api/groups") return json({ groups: [], pools: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    if (!/^\/[\w./-]*$/.test(url.pathname) || url.host !== "magpie.test") return route.fulfill({ status: 404, body: "" });
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

async function openEditor(browser, lang, run, { id = "claude", width = 900, switchOnly = false } = {}) {
  const page = await (await browser.newContext({ viewport: { width, height: 760 } })).newPage();
  page.setDefaultTimeout(5000);
  page.setDefaultNavigationTimeout(60000);
  const errors = [], reads = { n: 0 };
  page.on("pageerror", (e) => errors.push(e.message));
  await page.route("**/*", serve(lang, run, switchOnly, reads));
  await page.goto("http://magpie.test/?view=providers");
  // the first draw waits on the page's first reads: a load's room, not a click's (#4470)
  const row = page.locator(`.row.provider[data-id="${id}"]`);
  await row.waitFor({ timeout: 60000 });
  await row.click();
  await page.locator("#modal .ebody").waitFor();
  return { page, errors, reads };
}

// the row's label and value, in a slot that lays out as the editor's rows
const runRow = (page) => page.locator("#modal .claude-mode", { has: page.locator(".cc-run") });

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const width of [900, 420]) for (const lang of Object.keys(W)) {
    const w = W[lang];
    test(`${engine} ${width} ${lang}: the interactive mode in use, its last request said, both modes on hover`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const { page, errors, reads } = await openEditor(browser, lang, RUNS.tui, { width });
      const row = runRow(page);
      await row.waitFor();
      assert.equal((await row.locator(":scope > label").textContent()).trim(), w.row);
      assert(await row.locator(":scope > label").evaluate((l) => l.compareDocumentPosition(document.querySelector("#modal .accts")) & Node.DOCUMENT_POSITION_FOLLOWING), "the row is above the accounts");
      const pill = row.locator(".cc-pill");
      assert.equal(await pill.evaluate((e) => e.classList.contains("tui")), true, "the pill is the interactive mode's");
      assert.equal((await pill.textContent()).trim(), w.tui);
      assert.match(await row.locator(".cc-said").textContent(), new RegExp(w.lastTui.replace(/[()]/g, "\\$&") + "$"));
      assert.equal(await row.locator("button", { hasText: w.change }).count(), 0, "nothing to change while it's on");
      assert.equal(await page.locator(".cc-tip").count(), 0);
      await pill.hover();
      const tip = page.locator(".cc-tip");
      await tip.waitFor();
      const modes = await tip.locator(".cc-m").evaluateAll((ms) => ms.map((m) => ({ on: m.classList.contains("on"), name: m.querySelector(".h b").textContent, now: m.querySelector(".now")?.textContent || "", good: m.querySelectorAll("p.good").length, bad: m.querySelectorAll("p.bad").length })));
      assert.deepEqual(modes, [
        { on: true, name: w.tui, now: w.inUse, good: 1, bad: 0 },
        { on: false, name: "claude -p", now: "", good: 1, bad: 1 },
      ]);
      const box = await tip.boundingBox();
      assert(box.x >= 0 && box.y >= 0 && box.x + box.width <= width && box.y + box.height <= 760, `the card is inside the window: ${JSON.stringify(box)}`);
      if (lang !== "en") {
        // every line of the card is in the reader's language
        const text = await tip.textContent();
        assert(!/magpie opens Claude Code as a person does|Your plan counts|It starts at once|Turned on or off in Settings/.test(text), `English left in the card: ${text}`);
      }
      await page.mouse.move(5, 5);
      await tip.waitFor({ state: "detached" });
      // the keyboard reaches it too
      await pill.focus();
      await tip.waitFor();
      assert.equal(reads.n, 1, "the mode is read once, not on each draw");
      assert.deepEqual(errors, []);
    });

    if (width !== 900) continue;
    test(`${engine} ${lang}: a request that was -p for a JSON schema says so`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const { page, errors } = await openEditor(browser, lang, RUNS.schema);
      const row = runRow(page);
      await row.waitFor();
      assert.equal(await row.locator(".cc-pill.tui").count(), 1, "still the interactive mode");
      assert((await row.locator(".cc-said").textContent()).endsWith(w.schema), "why the last one was -p");
      assert.deepEqual(errors, []);
    });

    test(`${engine} ${lang}: turned off, it says claude -p and why, and goes to the setting`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const { page, errors } = await openEditor(browser, lang, RUNS.off);
      const row = runRow(page);
      await row.waitFor();
      const pill = row.locator(".cc-pill");
      assert.equal(await pill.evaluate((e) => e.classList.contains("tui")), false);
      assert.equal((await pill.textContent()).trim(), "claude -p");
      assert.equal((await row.locator(".cc-said").textContent()).trim(), w.off);
      await pill.hover();
      assert.deepEqual(await page.locator(".cc-tip .cc-m").evaluateAll((ms) => ms.map((m) => m.classList.contains("on"))), [false, true], "-p is the one in use");
      await row.locator("button", { hasText: w.change }).click();
      await page.locator("#view-settings").waitFor();
      // the row that turns it on, lit, and still the page's after it lit
      await page.locator("#view-settings #claudeTuiRow.flash").waitFor();
      await page.waitForTimeout(300);
      assert(await page.locator("#claudeTuiRow.flash").isVisible(), "the interactive mode's row is shown, lit");
      assert.equal(await page.locator(".cc-tip").count(), 0, "the card went with the editor");
      assert.deepEqual(errors, []);
    });

    test(`${engine} ${lang}: Interactive only says a request fails rather than running as -p`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const { page, errors } = await openEditor(browser, lang, RUNS.only);
      const row = runRow(page);
      await row.waitFor();
      const tr = (k, v) => page.evaluate(([k, v]) => t(k, v), [k, v]);
      assert.equal(await row.locator(".cc-pill.tui").count(), 1);
      assert.equal((await row.locator(".cc-said").textContent()).trim(), await tr("Interactive only: a request it can't take fails instead of running as -p"));
      assert.equal(await row.locator("button", { hasText: w.change }).count(), 0);
      await row.locator(".cc-pill").hover();
      const foot = await tr("Interactive only is set in Settings › Models › Replies: nothing runs as -p, and a request the interactive mode can't take, or one it fails to start for, fails saying why.");
      assert.equal((await page.locator(".cc-tip .foot").textContent()).trim(), foot);
      if (lang !== "en") assert(!/^Interactive only/.test(foot), `not translated: ${foot}`);
      assert.deepEqual(errors, []);
    });

    test(`${engine} ${lang}: Interactive only where it can't run says Claude requests fail, and why`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const { page, errors } = await openEditor(browser, lang, RUNS.onlyBroken);
      const row = runRow(page);
      await row.waitFor();
      const tr = (k, v) => page.evaluate(([k, v]) => t(k, v), [k, v]);
      const pill = row.locator(".cc-pill");
      assert.equal(await pill.evaluate((e) => e.classList.contains("tui")), false);
      assert.equal((await pill.textContent()).trim(), await tr("Interactive only"), "never claude -p: nothing runs as it");
      const why = await tr("the interactive mode failed to start lately, and is tried again later");
      assert.equal((await row.locator(".cc-said").textContent()).trim(), await tr("Claude requests fail: {why}", { why }));
      if (lang !== "en") assert(!/Claude requests fail/.test(await row.locator(".cc-said").textContent()));
      // the way out is the setting
      await row.locator("button", { hasText: w.change }).click();
      await page.locator("#view-settings #claudeTuiRow.flash").waitFor();
      assert.deepEqual(errors, []);
    });

    test(`${engine} ${lang}: Claude Code in WSL whose home wasn't found says so`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const { page, errors } = await openEditor(browser, lang, RUNS.wsl);
      const row = runRow(page);
      await row.waitFor();
      assert.match(await row.locator(".cc-said").textContent(), new RegExp(w.wsl));
      assert.equal(await row.locator("button", { hasText: w.change }).count(), 0);
      assert.deepEqual(errors, []);
    });
  }

  test(`${engine}: another subscription has no such row`, async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    const { page, errors, reads } = await openEditor(browser, "en", RUNS.tui, { id: "codex" });
    await page.locator("#modal .acc").first().waitFor();
    assert.equal(await page.locator("#modal .cc-run, #modal .claude-mode").count(), 0);
    assert.equal(reads.n, 0, "nothing is read for a row not shown");
    assert.deepEqual(errors, []);
  });

  test(`${engine}: Only switch, which serves nothing, has no such row`, async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    const { page, errors, reads } = await openEditor(browser, "en", RUNS.tui, { switchOnly: true });
    await page.locator("#modal .acc").first().waitFor();
    assert.equal(await page.locator("#modal .cc-run, #modal .claude-mode").count(), 0);
    assert.equal(reads.n, 0, "nothing is read for a row not shown");
    assert.deepEqual(errors, []);
  });
}
