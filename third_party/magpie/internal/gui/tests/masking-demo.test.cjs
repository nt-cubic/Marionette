// Run with Node's test runner and Playwright on the module path; see README.md.
// Insights › Masking's demo (plugins/redact/demo.js): a folded row between
// the card and the requests. Folded, it fetches no video. Opened, it fetches
// the take for the theme in use (and the other one when the theme changes),
// plays while open and on screen, rests at the end, and stops when folded
// or when a request is opened over it. With reduced motion it starts on the
// last step and doesn't play. Her video is drawn on the card's own colour.
// Nothing is left in English, nothing is wider than the window.
// Every language, Chromium and WebKit, 1100 and 640px; the API is faked.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const K = "1792987654321000001";
const record = { key: K, t: new Date(Date.now() - 60e3).toISOString(), route: "messages", what: "chat", agent: "claude", provider: "anthropic", model: "claude-opus-4-1", n: 1,
  values: [{ p: "{{PERSON_bbbb2222}}", k: "PERSON", b: "model", h: "", l: 3, n: 1 }], snips: { "{{PERSON_bbbb2222}}": "ask {{PERSON_bbbb2222}} about it" } };

function serve(lang, theme, fetched, more, boot = {}) {
  const settings = { theme, lang, tray: "panel", currency: "usd", redact: true, redactPersonal: true, ...more };
  return async (route) => {
    const req = route.request(), url = new URL(req.url());
    const json = (data) => route.fulfill({ json: data });
    if (url.pathname === "/boot.js") return route.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = ${JSON.stringify({ lang, theme, web: true, ...boot })};` });
    if (url.pathname === "/wails/runtime.js") return route.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json({ agents: [{ id: "claude", name: "Claude Code", icon: "claudecode-color", fields: [] }], profiles: [], settings });
    if (url.pathname === "/api/settings") return json(settings);
    if (url.pathname === "/api/providers") return json({ providers: [{ id: "anthropic", name: "Anthropic", icon: "anthropic", agents: [], models: [] }], presets: [], excluded: [], gateway: { running: true, window: true, mine: true, url: "http://127.0.0.1:3999", models: 0, groups: [], calls: [] } });
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname === "/api/masking/summary") return json({ days: [], requests: 1, n: 1, distinct: 1, blocked: 0, hours: [], kinds: [{ name: "PERSON", n: 1, distinct: 1 }], by: { model: 1 }, agents: [{ name: "claude", n: 1, distinct: 1 }], providers: [{ name: "anthropic", n: 1, distinct: 1 }], values: [] });
    if (url.pathname === "/api/masking/records") return json({ records: [record], more: false });
    if (url.pathname === "/api/masking/record") return json(record);
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname), ext = path.extname(file);
    if (ext === ".webm" || ext === ".jpg") fetched.push(path.basename(file));
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".jpg": "image/jpeg", ".webm": "video/webm" }[ext];
    let body;
    try { body = await fs.readFile(file); } catch { return route.fulfill({ status: 404, body: "" }); }
    // a video is served in ranges, as the gateway's file server does, or
    // Chromium can't seek in it
    const range = ext === ".webm" && /bytes=(\d+)-(\d*)/.exec(req.headers().range || "");
    if (range) {
      const from = +range[1], to = range[2] ? Math.min(+range[2], body.length - 1) : body.length - 1;
      return route.fulfill({ status: 206, body: body.subarray(from, to + 1), contentType, headers: { "Accept-Ranges": "bytes", "Content-Range": `bytes ${from}-${to}/${body.length}` } });
    }
    await route.fulfill({ body, contentType, headers: ext === ".webm" ? { "Accept-Ranges": "bytes" } : {} });
  };
}

async function open(engine, lang, width, opts = {}) {
  const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
  const page = await (await browser.newContext({ viewport: { width, height: 1000 }, reducedMotion: opts.motion ? "no-preference" : "reduce" })).newPage();
  page.setDefaultTimeout(8000);
  const errors = [], fetched = [];
  page.on("pageerror", (e) => errors.push(e.message));
  await page.route("**/*", serve(lang, opts.theme || "light", fetched, opts.settings, opts.boot));
  await page.goto("http://magpie.test/?view=masking");
  await page.locator("#mdRow").waitFor();
  return { browser, page, errors, fetched };
}
const state = (page) => page.evaluate(() => document.querySelector(".xqd").__state());
// page, when given, adds the demo's state to the failure
async function until(ok, ms = 8000, page) {
  for (const end = Date.now() + ms; !(await ok());) {
    if (Date.now() > end) throw new Error("never happened: " + ok + (page ? " " + JSON.stringify(await state(page).catch(() => null)) : ""));
    await new Promise((r) => setTimeout(r, 50));
  }
}

const english = ["How masking works", "Little Magpie", "Over the network", "notebook", "Model service", "Letter to the provider", "Your computer",
  "Sign in to github.com", "Signed in to github.com", "Nothing in it yet", "Signing in on your computer", "on your computer", "placeholders"];
const wide = (page) => page.evaluate(() => {
  const over = [...document.querySelectorAll("#view-masking *")].filter((n) => n.getClientRects().length && n.getBoundingClientRect().right > innerWidth + 1).map((n) => n.className?.baseVal ?? n.className);
  return { page: document.documentElement.scrollWidth > document.documentElement.clientWidth, over };
});

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
    for (const width of [1100, 640]) {
      test(`${engine} ${lang} ${width}px: the demo is folded, and opens on its last step`, async (t) => {
        const { browser, page, errors, fetched } = await open(engine, lang, width);
        t.after(() => browser.close());
        const row = page.locator("#mdRow");
        assert.equal(await row.getAttribute("aria-expanded"), "false");
        assert(await page.locator("#mdBody").isHidden(), "folded at first");
        // it sits between the card and the requests
        assert.deepEqual(await page.evaluate(() => [document.querySelector("#maskDemo").previousElementSibling?.id, document.querySelector("#maskDemo").nextElementSibling?.id]), ["maskCard", "maskList"]);
        // with the same room above it as the list has below it
        assert.deepEqual(await page.evaluate(() => {
          const r = (s) => document.querySelector(s).getBoundingClientRect();
          return [Math.round(r("#maskDemo").top - r("#maskCard").bottom), Math.round(r("#maskList").top - r("#maskDemo").bottom)];
        }), [14, 14]);
        await page.waitForTimeout(300);
        assert(!fetched.some((f) => f.endsWith(".webm")), "a video fetched while folded: " + fetched);

        await row.click();
        assert.equal(await row.getAttribute("aria-expanded"), "true");
        await page.locator(".xqd .xq-stage").waitFor();
        await until(() => fetched.includes("xq_light.webm"));
        assert(!fetched.includes("xq_dark.webm"), "only the theme's take");
        // reduced motion: on the last step, not playing
        const s = await state(page);
        assert.equal(s.playing, false);
        assert.equal(s.raf, false);
        assert(Math.abs(s.T - 25.6) < .01, "T " + s.T);
        assert.match(await page.locator(".xq-cap-t").innerText(), /^8\/8/);
        await page.waitForTimeout(300);
        assert(Math.abs((await state(page)).T - 25.6) < .01, "it moved under reduced motion");
        // the stage fits the card
        const fit = await page.evaluate(() => { const vp = document.querySelector(".xq-vp").getBoundingClientRect(), st = document.querySelector(".xq-stage").getBoundingClientRect(); return [vp.width, st.width, vp.height, st.height]; });
        assert(Math.abs(fit[0] - fit[1]) < 1 && Math.abs(fit[2] - fit[3]) < 1, "stage " + fit);

        if (lang !== "en") {
          const said = await page.locator("#maskDemo").innerText();
          for (const w of english) assert(!said.includes(w), `${w} left in English: ${said}`);
          assert(!(await page.locator(".xq-play").getAttribute("title")).includes("demo"), "the play button's name");
        }
        assert.deepEqual(await wide(page), { page: false, over: [] });
        // every box's text ends inside it; the two whose text flows keep room for a font a
        // little taller than this one (WebKitGTK's Noto ran the provider's note 16px past its box)
        assert.deepEqual(await page.evaluate(() => {
          const k = document.querySelector(".xq-stage").getBoundingClientRect().width / 1000;
          return [...document.querySelectorAll(".xq-stage .xq-box")].flatMap((b) => {
            const end = b.getBoundingClientRect().bottom - k;
            const low = Math.max(...[...b.querySelectorAll("*")].filter((n) => n.getClientRects().length).map((n) => n.getBoundingClientRect().bottom));
            return (end - low) / k < ({ req: 4, prov: 4 }[b.dataset.k] ?? 0) - .5 ? [`${b.dataset.k} ${Math.round((end - low) / k)}px to spare`] : [];
          });
        }), []);
        // each box's icon is a 22px square (the app's own .acc once made the notebook's
        // 38px tall), and the notebook's entries sit under its header, one after the other
        assert.deepEqual(await page.evaluate(() => {
          const k = document.querySelector(".xq-stage").getBoundingClientRect().width / 1000, r = (n) => n.getBoundingClientRect();
          const bad = [...document.querySelectorAll(".xq-stage .xq-ic")].map((n) => [Math.round(r(n).width / k), Math.round(r(n).height / k)]).filter(([w, h]) => w !== 22 || h !== 22);
          const book = document.querySelector('.xq-stage [data-k="ledger"]'), head = r(book.querySelector(".xq-bh")).bottom;
          const rows = [...book.querySelectorAll(".xq-lrow")].map(r);
          return { bad, gaps: [rows[0].top - head, rows[1].top - rows[0].bottom].map((g) => g / k >= 4) };
        }), { bad: [], gaps: [true, true] });
        if (process.env.MASK_SHOTS) await page.screenshot({ path: path.join(process.env.MASK_SHOTS, `demo_${engine}_${lang}_${width}.png`), fullPage: true });

        await row.click();
        assert(await page.locator("#mdBody").isHidden(), "folded again");
        assert.deepEqual(errors, []);
      });
    }
  }

  test(`${engine}: the demo plays while open, stops when folded or covered, and rests at the end`, async (t) => {
    const { browser, page, errors } = await open(engine, "en", 1100, { motion: true });
    t.after(() => browser.close());
    await page.locator("#mdRow").click();
    await page.locator(".xqd .xq-stage").waitFor();
    await until(async () => (await state(page)).T > .8);
    const playing = await state(page);
    assert.equal(playing.playing, true);
    if (engine === "chromium") {
      // Chromium plays VP9: the video is the clock
      await until(async () => { const s = await state(page); return s.rs >= 2 && !s.paused; });
      await until(async () => { const s = await state(page); return Math.abs(s.ct - s.T) < .3; });
    }

    // folded: no frames, the video paused, T held
    await page.locator("#mdRow").click();
    await until(async () => !(await state(page)).raf);
    const held = await state(page);
    assert.equal(held.paused, true);
    await page.waitForTimeout(400);
    assert.equal((await state(page)).T, held.T, "T moved while folded");

    // opened again it goes on from there
    await page.locator("#mdRow").click();
    await until(async () => (await state(page)).T > held.T + .3);

    // a request opened over it: hidden and stopped; back, it plays again
    await page.evaluate((k) => maskOneOpen(k), K);
    await until(async () => page.locator("#maskDemo").isHidden());
    await until(async () => !(await state(page)).raf);
    assert.equal((await state(page)).paused, true);
    await page.evaluate(() => maskOneClose());
    await until(async () => page.locator("#maskDemo").isVisible());
    await until(async () => (await state(page)).raf);

    // the last step plays to the end and stops there: the page comes to rest
    await page.locator(".xq-steps button").last().click();
    await until(async () => { const s = await state(page); return !s.playing && !s.raf; }, 12000, page);
    const end = await state(page);
    assert(end.T >= 27.4, "rests at the end, T " + end.T);
    const endless = await page.evaluate(() => document.getAnimations().filter((a) => a.effect?.getComputedTiming().endTime === Infinity).length);
    assert.equal(endless, 0, "an animation that never ends");
    // play from the end starts it over
    await page.locator(".xq-play").click();
    await until(async () => { const s = await state(page); return s.playing && s.T < 5; });
    assert.deepEqual(errors, []);
  });

  test(`${engine}: the dark theme gets the dark take, on the card's colour`, async (t) => {
    const { browser, page, errors, fetched } = await open(engine, "en", 1100, { theme: "dark" });
    t.after(() => browser.close());
    await page.locator("#mdRow").click();
    await until(() => fetched.includes("xq_dark.webm"));
    assert(!fetched.includes("xq_light.webm"), "only the theme's take");
    if (engine === "chromium") {
      await until(async () => (await state(page)).rs >= 2);
      await onCard(page);
    }
    // the theme changed: the other take, on the light card
    await page.evaluate(() => { document.documentElement.dataset.theme = "light"; });
    await until(async () => (await state(page)).src === "xq_light.webm");
    if (engine === "chromium") {
      await until(async () => (await state(page)).rs >= 2);
      await onCard(page);
    }
    assert.deepEqual(errors, []);
  });
  test(`${engine}: a theme changed while it plays goes on playing, in the new take`, async (t) => {
    const { browser, page, errors } = await open(engine, "en", 1100, { motion: true });
    t.after(() => browser.close());
    await page.locator("#mdRow").click();
    await until(async () => { const s = await state(page); return !s.paused && s.ct > 1; }, 8000, page);
    await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
    await until(async () => (await state(page)).src === "xq_dark.webm", 8000, page);
    const a = await state(page);
    // the new take plays on from where the old one was, in step with the sheets
    await until(async () => { const s = await state(page); return !s.paused && s.ct > a.T + .5 && Math.abs(s.ct - s.T) < .3; }, 8000, page);
    assert.deepEqual(errors, []);
  });
  // on Linux without gst-plugins-good WebKitGTK's web process aborts on any
  // video (stock Omarchy), so there she is her poster and the sheets still
  // move. The app says so in boot.js, as the server does; /api/state's
  // settings never carry it (avk458 on X: the demo crashed omarchy's app)
  test(`${engine}: where the app can't play video she is her poster, and the demo still plays`, async (t) => {
    const { browser, page, errors, fetched } = await open(engine, "en", 1100, { motion: true, boot: { noVideo: true } });
    t.after(() => browser.close());
    await page.locator("#mdRow").click();
    await until(async () => (await state(page)).T > 1, 8000, page);
    await page.evaluate(() => { document.documentElement.dataset.theme = "dark"; });
    await until(() => fetched.includes("xq_dark.jpg"));
    assert.deepEqual(await page.evaluate(() => { const v = document.querySelector(".xqd video"); return [v.getAttribute("src"), v.poster.split("/").pop()]; }), [null, "xq_dark.jpg"]);
    assert.deepEqual(fetched.filter((f) => f.endsWith(".webm")), []);
    const a = await state(page);
    await until(async () => (await state(page)).T > a.T + .5, 8000, page);
    assert.deepEqual(errors, []);
  });
  // folded, or with the page left, the video lets go of its source, so it
  // holds no decoder or buffered video; opened again it goes on where it was
  test(`${engine}: folded or left, the demo holds no video; opened again it goes on`, async (t) => {
    const { browser, page, errors } = await open(engine, "en", 1100, { motion: true });
    t.after(() => browser.close());
    const src = () => page.evaluate(() => document.querySelector(".xqd video").getAttribute("src"));
    await page.locator("#mdRow").click();
    await until(async () => { const s = await state(page); return !s.paused && s.ct > 1; }, 8000, page);
    await page.locator("#mdRow").click();
    await until(async () => (await src()) === null, 8000, page);
    const folded = await state(page);
    assert.equal(folded.rs, 0, "no data held while folded");
    await page.locator("#mdRow").click();
    await until(async () => { const s = await state(page); return s.src === "xq_light.webm" && !s.paused && s.ct > folded.T; }, 8000, page);
    // another page: the masking view hidden
    await page.evaluate(() => { document.querySelector("#view-masking").hidden = true; });
    await until(async () => (await src()) === null, 8000, page);
    await page.evaluate(() => { document.querySelector("#view-masking").hidden = false; });
    await until(async () => (await src()) !== null, 8000, page);
    assert.deepEqual(errors, []);
  });
}

// her background is the card's: the video's own corners, against the card
async function onCard(page) {
  const [card, px] = await page.evaluate(() => {
    const v = document.querySelector(".xqd video"), c = document.createElement("canvas");
    c.width = v.videoWidth; c.height = v.videoHeight;
    const g = c.getContext("2d");
    g.drawImage(v, 0, 0);
    const at = [[4, 4], [c.width - 5, 4], [4, c.height / 2]].map(([x, y]) => [...g.getImageData(x, y, 1, 1).data.slice(0, 3)]);
    const bg = getComputedStyle(document.querySelector("#maskDemo")).backgroundColor.match(/\d+/g).slice(0, 3).map(Number);
    return [bg, at];
  });
  for (const p of px) assert(p.every((v, i) => Math.abs(v - card[i]) <= 6), `video ${p} on card ${card}`);
}
