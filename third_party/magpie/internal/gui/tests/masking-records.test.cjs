// Run with Node's test runner and Playwright on the module path; see README.md.
// Settings › Privacy opens on what masking kept from vendors: the number,
// the days, the kinds, agents to providers and the values masked most, a
// value shown only while pointed at (and said gone when this magpie no
// longer knows it), and the newest request as the vendor got it or, asked
// for, as it is here. Its records open from the card, filtered by what was
// clicked, each one opening on the text the vendor saw. A masked call on
// the Gateway page opens its own record. Nothing in the card or the
// records is left in English, nothing is wider than the window, and a
// click scrolls nothing.
// Every language, Chromium and WebKit, 1100 and 640px, tall enough that
// pointing at a row needs no scroll; the API is faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { watchSettingsReads, settingsDrawn } = require("./reader.cjs");

const assets = path.resolve(__dirname, "../assets");
const KEY = "{{API_KEY_aaaa1111}}", NAME = "{{PERSON_bbbb2222}}", MAIL = "{{EMAIL_cccc3333}}";
const SECRET = "sk-proj-abcdEFGH1234ijkl";

function day(back) {
  const d = new Date();
  d.setDate(d.getDate() - back);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}
const ago = (ms) => new Date(Date.now() - ms).toISOString();

function summary(days) {
  const per = [0, 3, 0, 7, 5, 12, 15];
  return {
    days: Array.from({ length: days }, (_, i) => { const n = per[(i + 7 - days % 7) % 7]; return { day: day(days - 1 - i), requests: n ? 2 : 0, n, distinct: n ? 1 : 0 }; }),
    requests: 9, n: 42, distinct: 11, hours: [],
    kinds: [{ name: "API_KEY", n: 20, distinct: 3 }, { name: "EMAIL", n: 12, distinct: 4 }, { name: "PERSON", n: 10, distinct: 4 }],
    by: { rule: 7, model: 4 },
    agents: [{ name: "claude", n: 30, distinct: 8 }, { name: "codex", n: 12, distinct: 3 }],
    providers: [{ name: "anthropic", n: 30, distinct: 8 }, { name: "openai", n: 12, distinct: 3 }],
    values: [
      { p: KEY, kind: "API_KEY", by: "rule", head: "sk-p", len: 24, n: 14, requests: 5, last: ago(60e3) },
      { p: NAME, kind: "PERSON", by: "model", head: "", len: 3, n: 6, requests: 3, last: ago(3600e3) },
    ],
    since: ago(7 * 86400e3),
  };
}
const records = [
  { key: 1002, t: ago(120e3), route: "messages", what: "chat", agent: "claude", provider: "anthropic", model: "claude-opus-4-1", n: 3,
    values: [{ p: KEY, k: "API_KEY", b: "rule", h: "sk-p", l: 24, n: 2, known: true }, { p: MAIL, k: "EMAIL", b: "rule", h: "a", l: 17, n: 1 }],
    // two values in one line: their text is shown once
    snips: { [KEY]: `export OPENAI_API_KEY=${KEY} # mail ${MAIL}`, [MAIL]: `…_KEY=${KEY} # mail ${MAIL}` } },
  { key: 1001, t: ago(300e3), route: "search", what: "search", agent: "codex", provider: "openai", model: "gpt-5", n: 1,
    values: [{ p: NAME, k: "PERSON", b: "model", h: "", l: 3, n: 1 }] },
];

function serve(lang, sum, asked) {
  const settings = { theme: "light", lang, tray: "panel", currency: "usd", redact: true, redactPersonal: true };
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [{ id: "claude", name: "Claude Code", icon: "claude" }, { id: "codex", name: "Codex", icon: "codex" }], profiles: [], settings });
    if (url.pathname === "/api/settings") return json(settings);
    if (url.pathname === "/api/providers") return json({ providers: [{ id: "anthropic", name: "Anthropic", icon: "anthropic", agents: [], models: [] }, { id: "openai", name: "OpenAI", icon: "openai", agents: [], models: [] }], presets: [], excluded: [],
      gateway: { running: true, window: true, mine: true, url: "http://127.0.0.1:3999", models: 0, groups: [],
        calls: [{ id: 7, time: ago(120e3), agent: "claude", model: "claude-opus-4-1", from: "anthropic", to: "anthropic", status: 200, ms: 900, masked: { n: 3, distinct: 2, kinds: { API_KEY: 2, EMAIL: 1 }, by: { rule: 2 }, key: 1002 } },
          { id: 8, time: ago(60e3), agent: "claude", model: "claude-opus-4-1", from: "anthropic", to: "anthropic", status: 200, ms: 700, masked: { n: 0, distinct: 0, warned: 2, key: 1003 } }] } });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/privacy-model") return json({ supported: true, installed: false, size: 1.6e9 });
    if (url.pathname === "/api/masking/summary") return json(sum(+url.searchParams.get("days")));
    if (url.pathname === "/api/masking/records") {
      asked.push(url.search);
      const q = url.searchParams;
      const rs = records.filter((r) => (!q.get("kind") || r.values.some((v) => v.k === q.get("kind"))) && (!q.get("by") || r.values.some((v) => v.b === q.get("by"))));
      return json({ records: rs, more: false });
    }
    if (url.pathname === "/api/masking/record") {
      const r = records.find((r) => r.key === +url.searchParams.get("key"));
      return r ? json(r) : route.fulfill({ status: 404, body: "404 page not found" });
    }
    if (url.pathname === "/api/masking/value") {
      asked.push("value " + url.searchParams.get("p"));
      return url.searchParams.get("p") === KEY ? json({ value: SECRET }) : route.fulfill({ status: 404, body: "404 page not found" });
    }
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    try { await route.fulfill({ body: await fs.readFile(file), contentType }); } catch { await route.fulfill({ status: 404, body: "" }); }
  };
}

async function open(engine, lang, width, sum, where) {
  const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
  const page = await (await browser.newContext({ viewport: { width, height: 1800 }, reducedMotion: "reduce" })).newPage();
  page.setDefaultTimeout(8000);
  const errors = [], asked = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.route("**/*", serve(lang, sum, asked));
  await watchSettingsReads(page);
  await page.goto("http://magpie.test/" + (where || "?view=settings&tab=privacy"));
  if (!where) await settingsDrawn(page);
  return { browser, page, errors, asked };
}

// English that a translated page must not show
const english = ["Kept from vendors", "What the vendor saw", "As written here", "Latest request", "Different values", "values masked", "What was kept", "Masked most", "View records", "What was masked", "Every kind", "Found by", "Local model", "Not in memory", "This request"];
const wide = (page) => page.evaluate(() => {
  const over = [...document.querySelectorAll("#maskObs *, #maskRecs *")].filter((n) => n.getClientRects().length && n.getBoundingClientRect().right > innerWidth + 1).map((n) => n.className?.baseVal ?? n.className);
  return { page: document.documentElement.scrollWidth > document.documentElement.clientWidth, over };
});
const scrolled = (page) => page.evaluate(() => [scrollX, scrollY, document.querySelector("#view-settings").scrollTop]);

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
    for (const width of [1100, 640]) {
      test(`${engine} ${lang} ${width}px: the card says what was kept, and its records open on what the vendor saw`, async (t) => {
        const { browser, page, errors, asked } = await open(engine, lang, width, summary);
        t.after(() => browser.close());
        const card = page.locator("#maskObs");
        await card.locator(".mo-num").waitFor();
        assert.equal(await card.locator(".mo-num").textContent(), "42");
        assert.equal(await card.locator(".mo-days .mo-day").count(), 7, "a bar a day");
        assert.equal(await card.locator(".mo-days .mo-day.zero").count(), 2, "a day with nothing, a dot");
        assert.equal(await card.locator(".mo-ledg.kind .mo-row:enabled").count(), 3);
        assert.equal(await card.locator(".mo-ledg.by .mo-row:enabled").count(), 2, "rules, then the model");
        assert.equal(await card.locator(".mo-row.model").count(), 1, "what only the model found");
        // the newest lines, as the vendor got them: one line shown once
        await card.locator(".mo-saw .mo-snip").first().waitFor();
        assert.equal(await card.locator(".mo-saw .mo-snip").count(), 1);
        assert.deepEqual(await card.locator(".mo-saw mark.mo-ph").allTextContents(), [KEY, MAIL]);
        assert.equal(await page.evaluate(() => [document.querySelector("#maskObs"), ...document.querySelectorAll("#maskObs *")].some((n) => /gradient/.test(getComputedStyle(n).backgroundImage + getComputedStyle(n, "::before").backgroundImage))), false, "flat: no gradient");
        assert.deepEqual(await card.locator(".mo-col.agent .nm").allTextContents(), ["Claude Code", "Codex"]);
        assert.deepEqual(await card.locator(".mo-col.provider .nm").allTextContents(), ["Anthropic", "OpenAI"]);
        // hairlines into this computer and out of it, drawn once it is laid out
        await card.locator(".mo-flow path.mo-th:not(.out)").first().waitFor({ state: "attached" });
        await card.locator(".mo-flow path.mo-th.out").first().waitFor({ state: "attached" });
        assert.equal(await page.evaluate(() => getComputedStyle(document.querySelector("#maskObs")).boxShadow), "none", "no shadow");
        assert.deepEqual(await card.locator(".mo-val .sh").allTextContents(), ["sk-p••••••••", "•••"]);
        assert.equal((await card.textContent()).includes(SECRET), false, "no value until pointed at");
        assert.equal(asked.filter((a) => a.startsWith("value")).length, 0, "nothing asked until pointed at");
        if (lang !== "en") {
          const said = await page.locator("#maskObs").innerText();
          for (const w of english) assert(!said.includes(w), `${w} left in English: ${said}`);
        }
        assert.deepEqual(await wide(page), { page: false, over: [] });

        // pointed at, a value says what it was; one this magpie no longer
        // knows says so
        const before = await scrolled(page);
        await card.locator(".mo-val").first().hover();
        await page.waitForFunction((s) => document.querySelector(".mask-tip .v")?.textContent === s, SECRET);
        await card.locator(".mo-val").nth(1).hover();
        await page.waitForFunction(() => document.querySelector(".mask-tip .v.gone"));
        await page.mouse.move(1, 1);
        await page.locator(".mask-tip").waitFor({ state: "detached" });

        // a line's placeholder says what it stood for
        await card.locator(".mo-saw mark.mo-ph").first().hover();
        await page.waitForFunction((s) => document.querySelector(".mask-tip .v")?.textContent === s, SECRET);
        await page.mouse.move(1, 1);
        await page.locator(".mask-tip").waitFor({ state: "detached" });

        // asked for, the newest request as it is here: each value read then,
        // one this magpie no longer knows said so; and back
        await card.locator(".mo-saw .segs .opt").nth(1).click();
        await page.waitForFunction((s) => document.querySelector("#maskObs .mo-orig:not(.gone)")?.textContent === s, SECRET);
        await card.locator(".mo-saw .mo-orig.gone").waitFor();
        assert.equal(await card.locator(".mo-saw mark.mo-ph").count(), 0);
        await card.locator(".mo-saw .segs .opt").first().click();
        assert.deepEqual(await card.locator(".mo-saw mark.mo-ph").allTextContents(), [KEY, MAIL]);

        // a kind's row opens its records, filtered by it
        await card.locator(".mo-ledg.kind .mo-row").nth(1).click();
        const recs = page.locator("#maskRecs");
        await recs.locator(".mr-item").first().waitFor();
        assert.match(asked.filter((a) => a.startsWith("?")).at(-1), /kind=EMAIL/);
        assert.equal(await page.locator("#redactList").isVisible(), false, "the records stand in for the page");
        assert.equal(await recs.locator(".mr-item").count(), 1);
        if (lang !== "en") {
          const said = await recs.innerText();
          for (const w of english) assert(!said.includes(w), `${w} left in English: ${said}`);
        }
        // opened: each value, and the text the vendor saw with its
        // placeholders marked
        await recs.locator(".mr-row").first().click();
        await recs.locator(".mr-body .mr-snip").first().waitFor();
        assert.equal(await recs.locator(".mr-val").count(), 2);
        assert.equal(await recs.locator(".mr-body .mr-snip").count(), 1, "a shared line once");
        assert.deepEqual(await recs.locator(".mr-snip mark.mr-ph").allTextContents(), [KEY, MAIL]);
        assert.equal(await recs.locator(".mr-shape.known").count(), 1, "only a value it still knows offers to show");
        await recs.locator(".mr-snip mark.mr-ph").first().hover();
        await page.waitForFunction((s) => document.querySelector(".mask-tip .v")?.textContent === s, SECRET);
        assert.deepEqual(await wide(page), { page: false, over: [] });
        assert.deepEqual(await scrolled(page), before, "no click moved the page");

        // back to Privacy, as it was
        await recs.locator(".mr-back").click();
        await page.locator("#redactList").waitFor();
        assert.equal(await recs.isHidden(), true);
        assert.deepEqual(errors, []);
      });
    }
  }

  test(`${engine}: a summary of nothing, an empty one and records filtered to nothing each say so`, async (t) => {
    let shape = {};
    const { browser, page, errors } = await open(engine, "en", 1100, () => shape);
    t.after(() => browser.close());
    // an old magpie's {}: the card waits on requests, without failing
    await page.locator("#maskObs .mo-empty").waitFor();
    assert.match(await page.locator("#maskObs").innerText(), /A request that carries a key/);
    await page.locator("#maskObs .mo-open").count().then((n) => assert.equal(n, 0));
    shape = { ...summary(7), requests: 0, n: 0, distinct: 0, kinds: [], agents: [], providers: [], values: [], by: {} };
    // the records with a filter matching nothing
    await page.evaluate(() => maskOpenRecords({ by: "yours" }));
    await page.locator("#maskRecs .mr-none", { hasText: "Nothing masked matches these filters" }).waitFor();
    assert.deepEqual(errors, []);
  });

  test(`${engine}: a masked call on the Gateway page opens its record`, async (t) => {
    const { browser, page, errors } = await open(engine, "en", 1100, summary, "?view=gateway");
    t.after(() => browser.close());
    const badge = page.locator("#activity .call .mask-badge:not(.warn)");
    await badge.waitFor();
    // a call whose values only went as written, as the user set (#1977)
    const warned = page.locator("#activity .call .mask-badge.warn");
    assert.equal(await warned.textContent(), "2");
    assert.equal(await warned.getAttribute("title"), "2 values went to the vendor as written, as you set");
    assert.equal(await badge.textContent(), "3");
    assert.match(await badge.getAttribute("title"), /^3 values masked before it went to the vendor: API keys 2, Emails 1$/);
    await badge.click();
    const recs = page.locator("#maskRecs");
    await recs.locator(".row-head .label", { hasText: "This request" }).waitFor();
    // first, and open on what the vendor saw
    await recs.locator(".mr-item.open .mr-snip").first().waitFor();
    assert.equal(await page.locator("#activity .call-item.open").count(), 0, "the click wasn't the row's");
    assert.deepEqual(errors, []);
  });
}
