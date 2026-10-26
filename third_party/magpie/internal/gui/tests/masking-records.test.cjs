// Run with Node's test runner and Playwright on the module path; see README.md.
// Insights › Masking: a short card of what masking kept from vendors (the
// number, the days, agents to providers, each with its logo, through
// magpie's bird in the middle of its ring) over the requests it changed,
// by day, narrowed by facets, agents and providers, or the values masked
// most. A request opens on its steps, its text as
// written here, as the vendor got it or side by side (only the parts
// masking changed, the rest folded) and how each value was masked; picking
// a value lights its places. A request held back says why and offers to
// mask instead; one whose text is gone (a restart, or older than the
// newest kept) shows the lines that are left. Masking off and nothing
// masked, the card offers to turn it on. A masked call on the Gateway page
// and a key= in the address open its request. Nothing is left in English,
// nothing is wider than the window, and a click scrolls nothing.
// Every language, Chromium and WebKit, 1100 and 640px; the API is faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const KEY = "{{API_KEY_aaaa1111}}", NAME = "{{PERSON_bbbb2222}}", MAIL = "{{EMAIL_cccc3333}}", PK = "{{PRIVATE_KEY_dddd4444}}";
const SECRET = "sk-proj-abcdEFGH1234ijkl", ADDR = "ann@example.com";

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
    requests: 9, n: 42, distinct: 11, blocked: 1, hours: [],
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
const filler = "x".repeat(900);
// the body 1002 went out with: a long system prompt nothing was masked in,
// and a user message with two values
const SENT = JSON.stringify({ model: "claude-opus-4-1", system: "You are Claude Code. " + filler, messages: [
  { role: "user", content: [{ type: "text", text: "earlier, unchanged" }] },
  { role: "user", content: [{ type: "text", text: `export OPENAI_API_KEY=${KEY} # mail ${MAIL}, again ${KEY}` }] },
] });
const records = [
  { key: 1003, t: ago(60e3), route: "messages", what: "chat", agent: "claude", provider: "anthropic", model: "claude-opus-4-1", n: 1, blocked: true,
    values: [{ p: PK, k: "PRIVATE_KEY", b: "rule", h: "b3Bl", l: 400, n: 1, a: "block" }],
    snips: { [PK]: `-----BEGIN OPENSSH PRIVATE KEY----- ${PK}` } },
  { key: 1002, t: ago(120e3), route: "messages", what: "chat", agent: "claude", provider: "anthropic", model: "claude-opus-4-1", n: 3, took: 1800, back: { [KEY]: 1 },
    values: [{ p: KEY, k: "API_KEY", b: "rule", h: "sk-p", l: 24, n: 2, known: true }, { p: MAIL, k: "EMAIL", b: "rule", h: "a", l: 15, n: 1, known: true }],
    snips: { [KEY]: `export OPENAI_API_KEY=${KEY} # mail ${MAIL}`, [MAIL]: `…_KEY=${KEY} # mail ${MAIL}` } },
  // from before a restart: no body, no values in memory
  { key: 1001, t: ago(300e3), route: "search", what: "search", agent: "codex", provider: "openai", model: "gpt-5", n: 1,
    values: [{ p: NAME, k: "PERSON", b: "model", h: "", l: 3, n: 1 }], snips: { [NAME]: `ask ${NAME} about it` } },
];

function serve(lang, sum, asked, opts = {}) {
  const settings = { theme: "light", lang, tray: "panel", currency: "usd", redact: !opts.off, redactPersonal: !opts.off, redactActions: { PRIVATE_KEY: "block" }, ...opts.settings };
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [{ id: "claude", name: "Claude Code", icon: "claudecode-color", fields: [] }, { id: "codex", name: "Codex", icon: "codex-color", fields: [] }], profiles: [], settings });
    if (url.pathname === "/api/settings") {
      if (req.method() === "POST") { asked.push("settings " + req.postData()); Object.assign(settings, JSON.parse(req.postData())); }
      return json(settings);
    }
    if (url.pathname === "/api/settings/redact-actions") {
      asked.push("actions " + req.postData());
      settings.redactActions = JSON.parse(req.postData()).actions;
      return json(settings);
    }
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
      const q = url.searchParams, kinds = q.get("kind")?.split(",");
      const rs = (opts.none ? [] : opts.records || records).filter((r) => (!kinds || r.values.some((v) => kinds.includes(v.k))) && (!q.get("by") || r.values.some((v) => v.b === q.get("by")))
        && (!q.get("agent") || r.agent === q.get("agent")) && (!q.get("provider") || r.provider === q.get("provider")) && (!q.get("blocked") || r.blocked));
      return json({ records: rs, more: false });
    }
    if (url.pathname === "/api/masking/record") {
      const r = (opts.records || records).find((r) => r.key === +url.searchParams.get("key"));
      return r ? json(r) : route.fulfill({ status: 404, body: "404 page not found" });
    }
    if (url.pathname === "/api/masking/sent") {
      asked.push("sent " + url.searchParams.get("key"));
      const body = opts.sent ? opts.sent[url.searchParams.get("key")] : url.searchParams.get("key") === "1002" && SENT;
      return body ? json({ body }) : route.fulfill({ status: 404, body: "404 page not found" });
    }
    if (url.pathname === "/api/masking/values") {
      const known = { [KEY]: SECRET, [MAIL]: ADDR, ...opts.known };
      const ps = JSON.parse(req.postData()).ps;
      asked.push("values " + ps.join(","));
      return json({ values: Object.fromEntries(ps.filter((p) => known[p]).map((p) => [p, known[p]])) });
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

async function open(engine, lang, width, sum, where, opts) {
  const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
  const page = await (await browser.newContext({ viewport: { width, height: 1000 }, reducedMotion: opts?.motion ? "no-preference" : "reduce" })).newPage();
  page.setDefaultTimeout(8000);
  const errors = [], asked = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.route("**/*", serve(lang, sum, asked, opts));
  await page.goto("http://magpie.test/" + (where || "?view=masking"));
  return { browser, page, errors, asked };
}
// what the fake API was asked is on this side, not the page's
async function until(ok, ms = 8000) {
  for (const end = Date.now() + ms; !ok();) {
    if (Date.now() > end) throw new Error("never happened: " + ok);
    await new Promise((r) => setTimeout(r, 25));
  }
}
const shot = (page, name) => process.env.MASK_SHOTS && page.screenshot({ path: path.join(process.env.MASK_SHOTS, name + ".png"), fullPage: true });

// English that a translated page must not show
const english = ["Kept from vendors", "By request", "By value", "masked these on this computer", "Different values", "values masked", "Held back",
  "Request content", "How it was masked", "What happened", "Side by side", "Only the parts masking changed", "parts unchanged", "Found by", "In the answer",
  "This request was not sent", "Mask it instead", "See the rules", "Nothing to put back", "no longer in memory", "Masked most", "Local model"];
const untranslated = async (page, lang) => {
  if (lang === "en") return;
  const said = await page.locator("#view-masking").innerText();
  for (const w of english) assert(!said.includes(w), `${w} left in English: ${said}`);
};
const wide = (page) => page.evaluate(() => {
  const over = [...document.querySelectorAll("#view-masking *")].filter((n) => n.getClientRects().length && n.getBoundingClientRect().right > innerWidth + 1 && !n.closest(".msk-text, .msk-same")).map((n) => n.className?.baseVal ?? n.className);
  return { page: document.documentElement.scrollWidth > document.documentElement.clientWidth, over };
});
const scrolled = (page) => page.evaluate(() => [scrollX, scrollY, document.querySelector("#view-masking").scrollTop, document.querySelector("main, .main, #main")?.scrollTop ?? 0]);
// a click that moves nothing: Playwright brings the target into view
// first, so the page is read after that and again after the click
async function stillClick(page, loc) {
  await loc.scrollIntoViewIfNeeded();
  const before = await scrolled(page);
  await loc.click();
  await page.waitForTimeout(100);
  assert.deepEqual(await scrolled(page), before, "no click moved the page");
}

// The page comes to rest while it sits open (#3185): the flow's dots, its
// scan mark and the Live dot each play a few times when drawn. Endless,
// the dots (SMIL) laid the page out ~700 times a second in Chromium, 16%
// of a core and ~0.5W; in WebKit 180 wakeups a second. SMIL's clock is
// moved on and each CSS animation's end is read, so nothing waits.
for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const width of [1100, 640]) {
    test(`${engine} ${width}px: the masking page's motion ends`, async (t) => {
      const { browser, page, errors } = await open(engine, "en", width, summary, "", { motion: true });
      t.after(() => browser.close());
      const card = page.locator("#maskCard");
      await card.locator(".mo-flow .mo-dot").first().waitFor({ state: "attached" });
      await page.locator("#maskList .msk-live i").waitFor({ state: "attached" });
      const rest = await page.evaluate(() => {
        const smil = [...document.querySelectorAll("#view-masking animate, #view-masking animateMotion")];
        for (const svg of new Set(smil.map((a) => a.ownerSVGElement))) svg.setCurrentTime(3600);
        // WebKit keeps an ended one's start time, so only Chromium is asked
        // the clock; both are asked each one's own timing
        const on = smil.filter((a) => { try { a.getStartTime(); return true; } catch { return false; } }).length;
        const forever = smil.filter((a) => ["repeatCount", "repeatDur", "dur", "end"].some((k) => a.getAttribute(k) === "indefinite") || !(+a.getAttribute("repeatCount") > 0)).length;
        const endless = document.getAnimations().filter((a) => a.effect?.getComputedTiming().endTime === Infinity).map((a) => a.animationName || a.id || "?");
        return { smil: smil.length, on, forever, endless };
      });
      assert(rest.smil > 0, "the flow moves when drawn");
      assert.equal(rest.forever, 0, "a dot set to go round for good");
      if (engine === "chromium") assert.equal(rest.on, 0, "an hour on, a dot still goes round");
      assert.deepEqual(rest.endless, [], "an animation that never ends");
      assert.deepEqual(errors, []);
    });
  }
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
    for (const width of [1100, 640]) {
      test(`${engine} ${lang} ${width}px: the card says what was kept, and each request opens on what changed`, async (t) => {
        const { browser, page, errors, asked } = await open(engine, lang, width, summary);
        t.after(() => browser.close());
        const card = page.locator("#maskCard"), list = page.locator("#maskList");
        await card.locator(".msk-num").waitFor();
        assert.equal(await card.locator(".msk-num").textContent(), "42");
        assert.equal(await card.locator(".msk-days .msk-day").count(), 7, "a bar a day");
        assert.equal(await card.locator(".msk-days .msk-day.zero").count(), 2, "a day with nothing, a dot");
        assert.deepEqual(await card.locator(".mo-col.agent .nm").allTextContents(), ["Claude Code", "Codex"]);
        assert.deepEqual(await card.locator(".mo-col.provider .nm").allTextContents(), ["Anthropic", "OpenAI"]);
        await card.locator(".mo-flow path.mo-th.out").first().waitFor({ state: "attached" });
        // magpie's bird on the line through this computer, and the words
        // that say magpie masked them, for a screenshot shared elsewhere
        assert.equal(await card.locator(".msk-hub use").getAttribute("href"), "#bird");
        const hub = await card.locator(".msk-hub").boundingBox(), mid = await card.locator(".mo-mid").boundingBox();
        assert(Math.abs(hub.x + hub.width / 2 - (mid.x + mid.width / 2)) < 1.5, "the bird sits on the centre line");
        // and in the middle of its ring (the lines' svg rule once pinned it
        // to the ring's top left)
        const bird = await card.locator(".msk-hub svg").boundingBox();
        assert(Math.abs(bird.x + bird.width / 2 - (hub.x + hub.width / 2)) < 1 && Math.abs(bird.y + bird.height / 2 - (hub.y + hub.height / 2)) < 1, "the bird in the middle of its ring");
        // each agent and provider with its logo
        assert.deepEqual(await card.locator(".mo-node .ic").evaluateAll((ns) => ns.map((n) => n.dataset.icon)), ["claudecode-color", "codex-color", "anthropic", "openai"]);
        // on both sides the logo first, then the name; a side's logos in one
        // column and its names on one edge; every logo on a disc of one size
        // (bare, a logo that fills its box and one that doesn't read as two
        // sizes)
        const nodes = await card.locator(".mo-node:not(:disabled)").evaluateAll((ns) => ns.map((b) => {
          const ic = b.querySelector(".ic").getBoundingClientRect(), nm = b.querySelector(".nm").getBoundingClientRect();
          return { side: b.parentElement.classList.contains("agent") ? "agent" : "provider", ic: { x: ic.x, w: ic.width, h: ic.height, r: getComputedStyle(b.querySelector(".ic")).borderRadius }, nm: nm.x };
        }));
        for (const n of nodes) {
          assert(n.ic.x + n.ic.w <= n.nm, `the logo before the name: ${JSON.stringify(n)}`);
          assert.deepEqual([n.ic.w, n.ic.h, n.ic.r], [24, 24, "50%"], `one disc for every logo: ${JSON.stringify(n)}`);
        }
        for (const side of ["agent", "provider"]) {
          const of = nodes.filter((n) => n.side === side);
          assert(of.every((n) => Math.abs(n.ic.x - of[0].ic.x) < 0.5 && Math.abs(n.nm - of[0].nm) < 0.5), `${side}s' logos in one column, names on one edge: ${JSON.stringify(of)}`);
        }
        assert.match(await card.locator(".mo-cap").textContent(), /magpie/);
        assert.equal(await page.evaluate(() => [...document.querySelectorAll("#maskCard, #maskCard *")].some((n) => /gradient/.test(getComputedStyle(n).backgroundImage))), false, "flat: no gradient");
        // short: the list starts in the first screen
        assert((await list.locator(".msk-row").first().boundingBox()).y < 760, "the requests show without a scroll");

        // the requests, by day, the held one red
        await list.locator(".msk-row").first().waitFor();
        assert.equal(await list.locator(".msk-row").count(), 3);
        assert.equal(await list.locator(".msk-row.held .msk-held").count(), 1);
        assert.equal(await list.locator(".msk-gday").count(), 1);
        // facets with their counts, a zero left out
        assert.deepEqual(await list.locator(".msk-facet b").allTextContents(), ["42", "20", "7", "4", "1"]);
        await untranslated(page, lang);
        assert.deepEqual(await wide(page), { page: false, over: [] });
        await shot(page, `${engine}-${lang}-${width}-main`);
        const before = await scrolled(page);

        // a facet narrows the list, and All brings it back
        await list.locator(".msk-facet.model").click();
        await page.waitForFunction(() => document.querySelectorAll("#maskList .msk-row").length === 1);
        assert.match(asked.filter((a) => a.startsWith("?")).at(-1), /by=model/);
        await list.locator(".msk-facet").first().click();
        await page.waitForFunction(() => document.querySelectorAll("#maskList .msk-row").length === 3);
        // an agent in the flow narrows it to that agent
        await card.locator(".mo-col.agent .mo-node", { hasText: "Codex" }).click();
        await page.waitForFunction(() => document.querySelectorAll("#maskList .msk-row").length === 1);
        assert.match(asked.filter((a) => a.startsWith("?")).at(-1), /agent=codex/);
        await card.locator(".mo-col.agent .mo-node.on").click();
        await page.waitForFunction(() => document.querySelectorAll("#maskList .msk-row").length === 3);

        // by value: the values masked most, each said only while pointed at
        await list.locator(".msk-bar .segs .opt").nth(1).click();
        await list.locator(".msk-vrow").first().waitFor();
        assert.deepEqual(await list.locator(".msk-vrow .sh").allTextContents(), ["sk-p••••••••", "•••"]);
        assert.equal((await list.textContent()).includes(SECRET), false, "no value until pointed at");
        await list.locator(".msk-vrow").first().hover();
        await page.waitForFunction((s) => document.querySelector(".mask-tip .v")?.textContent === s, SECRET);
        await page.mouse.move(1, 1);
        await untranslated(page, lang);
        await list.locator(".msk-bar .segs .opt").first().click();

        // a request: its steps, only the text masking changed, side by side
        await list.locator(".msk-row:not(.held)").first().click();
        const one = page.locator("#maskOne");
        await one.locator(".msk-msg").first().waitFor();
        assert.equal(await card.isHidden(), true);
        assert.equal(await one.locator(".msk-steps li").count(), 5);
        assert.equal(await one.locator(".msk-msg").count(), 1, "one piece of text had values");
        assert.equal(await one.locator(".msk-fold").count(), 1, "the system prompt and the earlier message, folded as one");
        assert.deepEqual(await one.locator(".msk-msg .msk-text").last().locator("mark").allTextContents(), [KEY, MAIL, KEY], "as the vendor got it");
        // as written here; an address stays hidden while Hide accounts is on
        // (it is, while Privacy masks personal data), for a screenshot
        const here = one.locator(".msk-msg .msk-text").first().locator("mark");
        assert.deepEqual(await here.evaluateAll((ms) => ms.map((m) => m.querySelector(".pii")?.dataset.raw ?? m.textContent)), [SECRET, ADDR, SECRET], "as written here");
        assert.equal(await here.nth(1).locator(".pii").count(), 1, "the address hidden");
        assert.equal(await one.locator(".msk-val").count(), 2);
        assert.match(await one.locator(".msk-steps").innerText(), /1[.,]8 ms/);
        await untranslated(page, lang);
        assert.deepEqual(await wide(page), { page: false, over: [] });
        await shot(page, `${engine}-${lang}-${width}-request`);
        // picked, a value lights its places and says what it was
        await stillClick(page, one.locator(".msk-val").first());
        assert.equal(await one.locator("mark.lit").count(), 4);
        assert.equal(await one.locator(".msk-val[aria-pressed=true] .msk-vshown").textContent(), SECRET);
        // a fold opens on what it holds
        await stillClick(page, one.locator(".msk-foldb").first());
        await one.locator(".msk-sameline").first().waitFor();

        // the older one is from before a restart: the lines that are left,
        // as the vendor got them
        await one.locator(".msk-step").nth(1).click();
        await one.locator(".msk-gone").waitFor();
        assert.equal(await one.locator(".msk-chead .segs .opt:disabled").count(), 2, "only what the vendor saw");
        assert.deepEqual(await one.locator(".msk-msg mark").allTextContents(), [NAME]);
        await untranslated(page, lang);
        // and the newest was held back: why, and how to send it masked
        await one.locator(".msk-step").first().click();
        await one.locator(".msk-step").first().click();
        await one.locator(".msk-blocked").waitFor();
        await untranslated(page, lang);
        assert.deepEqual(await wide(page), { page: false, over: [] });
        await shot(page, `${engine}-${lang}-${width}-blocked`);
        await one.locator(".msk-btools button").first().click();
        await until(() => asked.some((x) => x.startsWith("actions")));
        assert.deepEqual(JSON.parse(asked.find((x) => x.startsWith("actions")).slice(8)), { actions: { PRIVATE_KEY: "mask" } });

        // back to the list, as it was
        await one.locator(".msk-back").click();
        await list.locator(".msk-row").first().waitFor();
        assert.equal(await one.isHidden(), true);
        assert.deepEqual(await scrolled(page), before, "no click moved the page");
        assert.deepEqual(errors, []);
      });
    }
    test(`${engine} ${lang}: masking off and nothing masked, the card offers to turn it on`, async (t) => {
      const empty = (d) => ({ ...summary(d), requests: 0, n: 0, distinct: 0, blocked: 0, kinds: [], agents: [], providers: [], values: [], by: {} });
      const { browser, page, errors, asked } = await open(engine, lang, 640, empty, null, { off: true, none: true });
      t.after(() => browser.close());
      await page.locator("#maskCard.off").waitFor();
      await page.locator("#maskList .msk-none").waitFor();
      if (lang !== "en") {
        const said = await page.locator("#view-masking").innerText();
        for (const w of ["Requests go to vendors", "Turn on masking", "You can also", "Nothing masked yet"]) assert(!said.includes(w), `${w} left in English: ${said}`);
      }
      assert.deepEqual(await wide(page), { page: false, over: [] });
      await shot(page, `${engine}-${lang}-off`);
      await page.locator("#maskCard .primary").click();
      await until(() => asked.some((x) => x.startsWith("settings")));
      assert.equal(JSON.parse(asked.find((x) => x.startsWith("settings")).slice(9)).redact, true);
      await page.locator("#maskCard:not(.off) .msk-head").waitFor();
      assert.deepEqual(errors, []);
    });
    // what the off card promises is what its button turns on: secrets,
    // not emails or computer names. Then, with secrets alone on and
    // nothing masked, the card says so and where personal data is turned
    // on, not a bare 0 (Lesnolie on X: "一直是 0"); your own words alone
    // mask, and the card says masking is on
    test(`${engine} ${lang}: secrets alone and nothing masked, the card says what is masked and where personal data is`, async (t) => {
      const empty = (d) => ({ ...summary(d), requests: 0, n: 0, distinct: 0, blocked: 0, kinds: [], agents: [], providers: [], values: [], by: {} });
      const { browser, page, errors } = await open(engine, lang, 640, empty, null, { off: true, none: true });
      t.after(() => browser.close());
      await page.locator("#maskCard.off").waitFor();
      if (lang === "en") {
        const off = await page.locator("#maskCard").innerText();
        assert(!/computer names|emails become/.test(off), `the off card promises what Turn on masking doesn't turn on: ${off}`);
      }
      await page.locator("#maskCard .primary").click();
      const none = page.locator("#maskCard .msk-flow-none");
      await none.locator("button").waitFor();
      if (lang === "en") assert.match(await none.innerText(), /^Only keys, passwords and tokens are masked now.*Mask personal data is on, in Settings › Privacy$/s);
      else for (const w of ["Only keys", "Emails and phone", "A request that carries"]) assert(!(await none.innerText()).includes(w), `${w} left in English`);
      assert.deepEqual(await wide(page), { page: false, over: [] });
      await shot(page, `${engine}-${lang}-secrets-only`);
      await none.locator("button").click();
      await page.locator("#view-settings").waitFor({ state: "visible" });
      assert.deepEqual(errors, []);
    });
    test(`${engine} ${lang}: your own words alone are masking on`, async (t) => {
      const empty = (d) => ({ ...summary(d), requests: 0, n: 0, distinct: 0, blocked: 0, kinds: [], agents: [], providers: [], values: [], by: {} });
      const { browser, page, errors } = await open(engine, lang, 640, empty, null, { off: true, none: true, settings: { redactWords: ["Nightjar"] } });
      t.after(() => browser.close());
      await page.locator("#maskCard .msk-head").waitFor();
      assert.equal(await page.locator("#maskCard.off").count(), 0, "words alone mask, the card says off");
      assert.equal(await page.locator("#maskCard .msk-head button").count(), 0, "no Turn on masking while words mask");
      assert(!(await page.locator("#maskCard .msk-flow-none").innerText()).includes("Only keys"), "words alone: not the secrets-only line");
      assert.deepEqual(errors, []);
    });
  }

  test(`${engine}: a masked call on the Gateway page, and a key in the address, open its request`, async (t) => {
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
    await page.locator("#view-masking #maskOne .msk-msg").first().waitFor();
    assert.equal(await page.locator("#activity .call-item.open").count(), 0, "the click wasn't the row's");
    assert.match(await page.locator("#maskOne .msk-when").textContent(), /\d/);
    // the tray panel's badge opens the window on key=
    await page.goto("http://magpie.test/?view=masking&key=1001");
    await page.locator("#maskOne .msk-gone").waitFor();
    assert.equal(new URL(page.url()).searchParams.get("key"), null, "read once");
    // and Settings › Privacy links to the page
    await page.goto("http://magpie.test/?view=settings&tab=privacy");
    await page.locator("#redactList .row", { hasText: "What was masked" }).locator("button").click();
    await page.locator("#maskCard .msk-num").waitFor();
    assert.deepEqual(errors, []);
  });
}

// "Keep a key's form" sends a made-up key of the real one's form in place of
// a {{…}} placeholder (redact's shape.go): the page marks it, and puts the
// real key against it, as it does a placeholder (#3599)
const MADE = "sk_hYQWRBFmdHNIo5XVT2VnIIGh4mmMiJNm", REAL = "sk_w3axCByTQgK1PxqPhfKTrJOLBqz6sbZv";
const made = {
  records: [{ key: 1005, t: ago(60e3), route: "chat", what: "chat", agent: "codex", provider: "openai", model: "gpt-5", n: 2, back: { [MADE]: 1 },
    values: [{ p: MADE, k: "API_KEY", b: "rule", h: "sk_w", l: 35, n: 2, known: true }],
    snips: { [MADE]: `OPENAI_API_KEY=${MADE}` } }],
  sent: { 1005: JSON.stringify({ model: "gpt-5", messages: [{ role: "user", content: `OPENAI_API_KEY=${MADE} sk_live_other\nonce more: ${MADE}` }] }) },
  known: { [MADE]: REAL },
};
for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh"]) {
    test(`${engine} ${lang}: a made-up key is marked as a placeholder is`, async (t) => {
      const { browser, page, errors } = await open(engine, lang, 1100, summary, "", made);
      t.after(() => browser.close());
      await page.locator("#maskList .msk-row").first().click();
      const one = page.locator("#maskOne");
      await one.locator(".msk-msg").first().waitFor();
      assert.deepEqual(await one.locator(".msk-msg .msk-text").last().locator("mark").allTextContents(), [MADE, MADE], "as the vendor got it");
      assert.deepEqual(await one.locator(".msk-msg .msk-text").first().locator("mark").allTextContents(), [REAL, REAL], "as written here");
      await stillClick(page, one.locator(".msk-val").first());
      assert.equal(await one.locator("mark.lit").count(), 4);
      await untranslated(page, lang);
      assert.deepEqual(errors, []);
    });
  }
}
