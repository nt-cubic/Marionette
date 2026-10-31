// Run with Node's test runner and Playwright on the module path; see README.md.
// Usage's Context tab tells each user, for each agent, the settings that
// would have spent the fewest tokens on their own calls (/api/tune), and
// puts one in on a click: Claude Code's auto-compact window for a model and
// its two cache lifetimes, Codex's compact limit. A setting already the
// leanest says so and has no button; one a variable comes before is told
// of and has none either; one magpie put in has Undo. Drawn before the
// gateway has read anything, in Chromium and WebKit, at a phone's width and
// a desk's, in every language, none of it wider than the page. Each row
// draws what every setting would have spent past the advice. The advice is
// named "Fewest tokens", and a window it would change says it is a token
// estimate that doesn't weigh what a compaction loses of the task (#1854).
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");

const assets = path.resolve(__dirname, "../assets");
const curve = (lo, hi, best) => {
  const pts = [];
  for (let v = lo; v <= hi; v += 10000) pts.push({ value: v, cost: 1e9 + Math.abs(v - best) * 3000, compacts: Math.round(9e6 / v) });
  return pts;
};
const facts = { calls: 4100, streams: 60, compacts: 813, peak: 166000, after: 57000, growth: 1000, rework: 4700, quick: 0.95, pause: 0.045, away: 0.005, hour: 0.9 };
function tune(set) {
  const opus = set["claude/compact/claude-opus-5-5"];
  return {
    days: 7,
    agents: [
      { agent: "claude", calls: 4300, advice: [
        { knob: "compact", model: "claude-opus-5-5", setting: opus || { value: "300000", source: "settings" },
          compact: { model: "claude-opus-5-5", current: opus ? +opus.value : 300000, best: 160000, cost: 1.9e9, low: 1.46e9, curve: curve(110000, 300000, 160000), facts } },
        { knob: "compact", model: "claude-haiku-4-5-20251001-with-a-very-long-name", setting: {},
          compact: { model: "claude-haiku-4-5", current: 200000, best: 200000, cost: 2e8, low: 2e8, curve: curve(100000, 200000, 200000), facts: { ...facts, compacts: 0 } } },
        { knob: "cache_ttl", setting: { value: "1h", source: "env", locked: "CLAUDE_CODE_PROMPT_CACHE_TTL" },
          ttl: { current: "5m", best: "1h", cost: 5.5e8, low: 4.6e8, short: 5.5e8, hour: 4.6e8, facts } },
        { knob: "subagent_cache_ttl", setting: {},
          ttl: { current: "5m", best: "5m", cost: 9e7, low: 9e7, short: 9e7, hour: 1.2e8, facts } },
      ] },
      { agent: "codex", calls: 900, advice: [] },
    ],
  };
}

function serve(lang, posts) {
  const state = { agents: [{ id: "claude", name: "Claude Code", path: "/test/claude", fields: [] }, { id: "codex", name: "Codex", path: "/test/codex", fields: [] }], profiles: [], settings: { lang, theme: "light" } };
  const set = {};
  return async (r) => {
    const url = new URL(r.request().url());
    const json = (data) => r.fulfill({ json: data });
    if (url.pathname === "/boot.js") return r.fulfill({ contentType: "text/javascript", body: `window.bootPrefs = {lang:"${lang}",theme:"light",web:true};` });
    if (url.pathname === "/wails/runtime.js") return r.fulfill({ contentType: "text/javascript", body: "export const Window = {};" });
    if (url.pathname === "/api/state") return json(state);
    if (url.pathname === "/api/tune") return json(tune(set));
    if (url.pathname === "/api/tune/apply" || url.pathname === "/api/tune/undo") {
      const b = JSON.parse(r.request().postData());
      posts.push({ path: url.pathname, ...b });
      const key = [b.agent, b.knob, b.model].join("/");
      if (url.pathname.endsWith("apply")) set[key] = { value: b.value, source: "model", ours: true };
      else delete set[key];
      return json(set[key] || {});
    }
    // the gateway has read nothing yet
    if (url.pathname === "/api/context") return json({ days: 7, agents: [], sessions: [] });
    if (url.pathname === "/api/gateway/trace") {
      if (url.searchParams.get("wait")) return new Promise(() => {});
      return json({ mine: true, now: new Date().toISOString(), seq: 0, totals: { requests: 0, rerouted: 0, errors: 0 }, routes: [] });
    }
    if (url.pathname === "/api/providers") return json({ providers: [], presets: [], excluded: [], gateway: { running: true, window: true, url: "http://127.0.0.1:3999" } });
    if (url.pathname === "/api/usage/quotas") return json([]);
    if (url.pathname === "/api/groups") return json({ groups: [], models: [] });
    if (url.pathname === "/api/plugins") return json({ plugins: [] });
    if (url.pathname.startsWith("/api/")) return json({});
    const file = path.join(assets, url.pathname === "/" ? "index.html" : url.pathname);
    const contentType = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png" }[path.extname(file)];
    await r.fulfill({ body: await fs.readFile(file).catch(() => ""), contentType, status: 200 });
  };
}

const fewest = { en: "Fewest tokens", zh: "最省 Token", "zh-TW": "最省 Token", ja: "トークン最少", de: "Wenigste Tokens" };
const caveat = {
  en: "A token estimate only: it doesn't measure how well the agent keeps the task after a compaction. The lower the window, the more often it compacts.",
  zh: "只是 Token 估算：没有衡量压缩后 Agent 还能多好地接着做任务。阈值越低，压缩越频繁。",
  "zh-TW": "只是 Token 估算：沒有衡量壓縮後 Agent 還能多好地接著做任務。閾值越低，壓縮越頻繁。",
  ja: "トークンの見積もりだけです。コンパクトのあとエージェントが作業をどれだけ保てるかは測っていません。しきい値が低いほどコンパクトの回数は増えます。",
  de: "Nur eine Token-Schätzung: Wie gut der Agent die Aufgabe nach einer Kompaktierung behält, misst sie nicht. Je niedriger die Schwelle, desto öfter wird kompaktiert.",
};
const heading = { en: "Tuned for you", zh: "为你调优", "zh-TW": "為你調校", ja: "あなた向けの調整", de: "Für dich abgestimmt" };

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(`${engine}: the Context tab advises each agent's settings and puts one in`, async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    const errors = [];
    t.after(async () => {
      if (errors.length) console.log(errors);
      await browser.close();
    });
    for (const lang of Object.keys(heading)) for (const [width, height] of [[360, 760], [1100, 760]]) {
      await t.test(`${lang}, ${width}x${height}`, async () => {
        const ctx = await browser.newContext({ viewport: { width, height }, reducedMotion: "reduce" });
        await ctx.addInitScript(() => { try { localStorage.setItem("magpie.usageTab", "context"); } catch {} });
        const page = await ctx.newPage();
        page.setDefaultTimeout(5000);
        page.on("pageerror", (e) => errors.push(e.message));
        const posts = [];
        await page.route("**/*", serve(lang, posts));
        await page.goto("http://magpie.test/?view=usage");
        const rows = page.locator(".ctx-tune-row");
        await rows.first().waitFor();
        assert.equal(await page.getByText(heading[lang], { exact: true }).count(), 1, "the heading in " + lang);
        // Codex had too few calls: no card for it
        assert.equal(await page.locator(".ctx-tune").count(), 1);
        assert.equal(await rows.count(), 4);
        // only the window a change saves on has a Use button; the locked one
        // says why; each window has a picker of its own (#1860), the cache
        // lifetimes none
        const buttons = page.locator(".ctx-tune-row button");
        assert.equal(await page.locator(".ctx-tune-row button.primary").count(), 1);
        assert.equal(await rows.nth(0).locator(".ctx-tune-pick").count(), 1);
        assert.equal(await rows.nth(1).locator(".ctx-tune-pick").count(), 1, "a window already the leanest can still be changed");
        assert.equal(await page.locator(".ctx-tune-pick").count(), 2);
        assert.match(await rows.nth(0).locator(".ctx-tune-val.best b").innerText(), /160K/);
        // the advice is named for tokens and says what it doesn't weigh
        // (#1854): on a window it would change, not on one already the
        // leanest, nor on a cache lifetime
        assert.equal(await rows.nth(0).locator(".ctx-tune-val.best .ctx-k").textContent(), fewest[lang]);
        assert.equal(await rows.nth(0).locator(".ctx-tune-caveat").innerText(), caveat[lang]);
        assert.ok(await rows.nth(0).locator(".ctx-tune-caveat").isVisible());
        for (const i of [1, 2, 3]) assert.equal(await rows.nth(i).locator(".ctx-tune-caveat").count(), 0, `row ${i} has no caveat`);
        assert.match(await rows.nth(2).locator(".ctx-tune-src").innerText(), /CLAUDE_CODE_PROMPT_CACHE_TTL/);
        assert.equal(await rows.nth(1).locator(".ctx-tune-val.best").count(), 0, "a window already the leanest shows no other");
        // the curve is what each setting spends past the advice: the window
        // set now says how much more, the advice sits on the level
        assert.equal(await rows.nth(0).locator(".ctx-tc-dot.now .ctx-tc-tag").innerText(), "+42%");
        assert.equal(await rows.nth(0).locator(".ctx-tc-dot.best").evaluate((e) => e.style.top), "84%");
        assert.equal(await rows.nth(2).locator(".ctx-tc-x span").count(), 2, "a cache lifetime's two choices");
        assert.ok(await rows.nth(0).locator(".ctx-tune-chart svg").isVisible());

        // nothing wider than the page, nor than its card
        const over = await page.evaluate(() => {
          const out = [];
          const view = document.querySelector("#view-usage");
          if (view.scrollWidth > view.clientWidth + 1) out.push("page " + view.scrollWidth + ">" + view.clientWidth);
          for (const c of document.querySelectorAll(".ctx-tune")) {
            const b = c.getBoundingClientRect();
            for (const e of c.querySelectorAll("*")) {
              const r = e.getBoundingClientRect();
              if (r.width && (r.right > b.right + 1 || r.left < b.left - 1)) out.push((e.className.baseVal ?? e.className) + " " + Math.round(r.left) + "–" + Math.round(r.right) + " of " + Math.round(b.left) + "–" + Math.round(b.right));
            }
          }
          return out;
        });
        assert.deepEqual(over, []);

        await buttons.first().evaluate((b) => b.click());
        await page.locator(".ctx-tune-row button", { hasText: { en: "Undo", zh: "撤销", "zh-TW": "復原", ja: "取り消す", de: "Rückgängig" }[lang] }).waitFor();
        assert.deepEqual(posts, [{ path: "/api/tune/apply", agent: "claude", knob: "compact", model: "claude-opus-5-5", value: "160000" }]);
        assert.match(await rows.nth(0).getAttribute("class"), /done/);
        await page.locator(".ctx-tune-row button").first().evaluate((b) => b.click());
        await page.locator(".ctx-tune-row button.primary").waitFor();
        assert.equal(posts[1].path, "/api/tune/undo");
        await ctx.close();
      });
    }
    assert.deepEqual(errors, []);
  });
}

// each curve draws itself once: the routing history answering after the
// advice, and a setting put in, redraw the pane, and what was on it already
// holds still
for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(`${engine}: the advice's curves play their entrance once`, async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    const ctx = await browser.newContext({ viewport: { width: 1100, height: 900 } });
    await ctx.addInitScript(() => {
      try { localStorage.setItem("magpie.usageTab", "context"); } catch {}
      window.drawn = 0;
      document.addEventListener("animationstart", (e) => { if (e.animationName === "ctx-draw") window.drawn++; }, true);
    });
    const page = await ctx.newPage();
    page.setDefaultTimeout(5000);
    const errors = [];
    page.on("pageerror", (e) => errors.push(e.message));
    const posts = [];
    const base = serve("en", posts);
    let tuned;
    const answered = new Promise((res) => { tuned = res; });
    await page.route("**/*", async (r) => {
      const p = new URL(r.request().url()).pathname;
      // the history comes after the advice is drawn
      if (p === "/api/context") await answered.then(() => new Promise((res) => setTimeout(res, 300)));
      await base(r);
      if (p === "/api/tune") tuned();
    });
    await page.goto("http://magpie.test/?view=usage");
    await page.locator(".ctx-tune-chart svg").first().waitFor();
    const charts = await page.locator(".ctx-tune-chart svg").count();
    assert.ok(charts > 0);
    await page.locator(".ctx-none").waitFor(); // the history has answered and the pane is redrawn
    await page.waitForTimeout(300);
    assert.equal(await page.evaluate(() => window.drawn), charts, "after the history answered");
    await page.locator(".ctx-tune-row button.primary").first().evaluate((b) => b.click());
    await page.locator(".ctx-tune-row.done").first().waitFor();
    await page.waitForTimeout(300);
    assert.equal(await page.evaluate(() => window.drawn), charts, "after a setting was put in");
    assert.deepEqual(errors, []);
  });
}

// A window of the reader's own (#1860, MrMengJ: 200K put in, and 250K or
// 300K wanted for fewer compactions): each window's picker lists every
// value the replay tried, with what it spends past the leanest and about
// how many compactions it would have made, in the app's own menu (no native
// select), inside a phone's width; a pick puts it in and Undo stays. The
// row already at the advice keeps its picker. No click scrolls the page.
const pickHead = { en: "Compact at · estimated from your calls of the range", zh: "压缩阈值 · 按这段时间你的调用估算", "zh-TW": "壓縮閾值 · 依這段時間你的呼叫估算", ja: "コンパクトする値 · この期間の呼び出しからの見積もり", de: "Kompaktieren bei · geschätzt aus deinen Aufrufen im Zeitraum" };
const note250 = { en: "+270M tokens (+27%) · about 36 compactions", zh: "多花 270M Token（+27%） · 约压缩 36 次", "zh-TW": "多花 270M Token（+27%） · 約壓縮 36 次", ja: "270M トークン多い（+27%） · コンパクト約 36 回", de: "+270M Tokens (+27%) · etwa 36 Kompaktierungen" };
const note160 = { en: "Fewest tokens · about 56 compactions", zh: "最省 Token · 约压缩 56 次", "zh-TW": "最省 Token · 約壓縮 56 次", ja: "トークン最少 · コンパクト約 56 回", de: "Wenigste Tokens · etwa 56 Kompaktierungen" };
for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(`${engine}: a window of the reader's own, with what it costs`, async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    const errors = [];
    t.after(() => browser.close());
    for (const lang of Object.keys(heading)) for (const [width, height] of [[360, 760], [1100, 760]]) {
      await t.test(`${lang}, ${width}x${height}`, async () => {
        const ctx = await browser.newContext({ viewport: { width, height }, reducedMotion: "reduce" });
        await ctx.addInitScript(() => { try { localStorage.setItem("magpie.usageTab", "context"); } catch {} });
        const page = await ctx.newPage();
        page.setDefaultTimeout(5000);
        page.on("pageerror", (e) => errors.push(e.message));
        const posts = [];
        await page.route("**/*", serve(lang, posts));
        await page.goto("http://magpie.test/?view=usage");
        const row = page.locator(".ctx-tune-row").nth(0);
        await row.locator(".ctx-tune-pick").waitFor();
        await page.locator(".ctx-none").waitFor(); // the history has answered: the pane is its full length
        // the reader scrolls to the picker, as a wheel does
        await page.mouse.move(width / 2, 300);
        // the row can re-render under the wheel, and a picker being replaced has no box
        // for a moment: wait for one instead of reading null (#4221)
        const pickY = async () => {
          for (let i = 0; i < 50; i++) {
            const b = await row.locator(".ctx-tune-pick").boundingBox();
            if (b) return b.y;
            await page.waitForTimeout(20);
          }
          throw new Error("the picker never had a box");
        };
        for (let i = 0; i < 20 && (await pickY()) > height * 0.6; i++) {
          await page.mouse.wheel(0, 120);
          await page.waitForTimeout(60);
        }
        await page.waitForTimeout(200);
        assert.equal(await page.locator("select").count(), 0, "no native select");
        const y = await page.evaluate(() => document.querySelector("#view-usage").scrollTop + scrollY);
        await row.locator(".ctx-tune-pick").click();
        const menu = page.locator(".proto-menu.ctx-tune-menu");
        await menu.waitFor();
        assert.equal(await menu.locator(".pm-head").innerText(), pickHead[lang]);
        // every value the replay tried, 110K to 300K; the one set now ticked
        assert.equal(await menu.locator(".pm-item").count(), 20);
        const item = (v) => menu.locator(".pm-item", { has: page.locator(".pm-name", { hasText: new RegExp("^" + v + "$") }) });
        assert.match(await item("300K").getAttribute("class"), /\bon\b/);
        assert.equal(await item("250K").locator(".pm-note").innerText(), note250[lang]);
        assert.equal(await item("160K").locator(".pm-note").innerText(), note160[lang]);
        const box = await menu.boundingBox();
        assert.ok(box.x >= 0 && box.x + box.width <= width + 1, `the menu is inside the window: ${JSON.stringify(box)}`);
        await item("250K").click();
        await page.locator(".ctx-tune-row").nth(0).locator("button", { hasText: { en: "Undo", zh: "撤销", "zh-TW": "復原", ja: "取り消す", de: "Rückgängig" }[lang] }).waitFor();
        assert.deepEqual(posts, [{ path: "/api/tune/apply", agent: "claude", knob: "compact", model: "claude-opus-5-5", value: "250000" }]);
        assert.equal(await page.evaluate(() => document.querySelector("#view-usage").scrollTop + scrollY), y, "no click scrolled the page");
        // now 250K: the advice and its Use button stand beside it, the
        // picker ticks 250K
        assert.match(await page.locator(".ctx-tune-row").nth(0).locator(".ctx-tune-val.now b").innerText(), /250K/);
        assert.equal(await page.locator(".ctx-tune-row").nth(0).locator("button.primary").count(), 1);
        await page.locator(".ctx-tune-row").nth(0).locator(".ctx-tune-pick").click();
        await menu.waitFor();
        assert.match(await item("250K").getAttribute("class"), /\bon\b/);
        await page.keyboard.press("Escape");
        assert.equal(posts.length, 1, "closing the menu puts nothing in");
        await ctx.close();
      });
    }
    assert.deepEqual(errors, []);
  });
}

// a value the advice or the setting now sits on, close to an end of the
// range, keeps its label and the end's is left off, so no two labels are
// drawn over each other (#1549, Ti800: the leanest 170K one step from the
// axis's 160K read as "1170K"), at a phone's width and a desk's
for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(`${engine}: the curve's axis labels and tags never overlap`, async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    const close = {
      days: 7,
      agents: [{ agent: "claude", calls: 4300, advice: [
        { knob: "compact", model: "claude-opus-5-5", setting: { value: "400000", source: "env", locked: "CLAUDE_CODE_AUTO_COMPACT_WINDOW" },
          compact: { model: "claude-opus-5-5", current: 400000, best: 170000, cost: 1.9e9, low: 1.46e9, curve: curve(160000, 1000000, 170000), facts } },
        { knob: "compact", model: "claude-sonnet-5", setting: {},
          compact: { model: "claude-sonnet-5", current: 990000, best: 500000, cost: 1.9e9, low: 1.46e9, curve: curve(160000, 1000000, 500000), facts } },
        // the setting now one step from the advice: its "+3%" tag would sit
        // on "Leanest"
        { knob: "compact", model: "claude-haiku-4-5", setting: {},
          compact: { model: "claude-haiku-4-5", current: 180000, best: 170000, cost: 1.9e9, low: 1.46e9, curve: curve(160000, 1000000, 170000), facts } },
      ] }],
    };
    // German's "Sparsamste" is the widest "Leanest"
    for (const lang of ["en", "de"]) for (const [width, height] of [[360, 760], [1100, 760]]) {
      const ctx = await browser.newContext({ viewport: { width, height }, reducedMotion: "reduce" });
      await ctx.addInitScript(() => { try { localStorage.setItem("magpie.usageTab", "context"); } catch {} });
      const page = await ctx.newPage();
      page.setDefaultTimeout(5000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      const base = serve(lang, []);
      await page.route("**/*", (r) => new URL(r.request().url()).pathname === "/api/tune" ? r.fulfill({ json: close }) : base(r));
      await page.goto("http://magpie.test/?view=usage");
      await page.locator(".ctx-tune-chart").nth(2).waitFor();
      const axes = await page.locator(".ctx-tc-x").evaluateAll((all) => all.map((a) => [...a.children].map((s) => {
        const r = s.getBoundingClientRect();
        return { text: s.textContent, cls: s.className, left: r.left, right: r.right };
      })));
      for (const ticks of axes) {
        for (let i = 0; i < ticks.length; i++) for (let j = i + 1; j < ticks.length; j++) {
          const [a, b] = [ticks[i], ticks[j]];
          assert.ok(a.right <= b.left || b.right <= a.left, `${width}px: "${a.text}" (${Math.round(a.left)}–${Math.round(a.right)}) overlaps "${b.text}" (${Math.round(b.left)}–${Math.round(b.right)})`);
        }
      }
      // nor the tags over the dots, and "Leanest" always keeps its own
      const tags = await page.locator(".ctx-tune-chart").evaluateAll((all) => all.map((c) => [...c.querySelectorAll(".ctx-tc-tag")].map((s) => {
        const r = s.getBoundingClientRect();
        return { text: s.textContent, left: r.left, right: r.right, top: r.top, bottom: r.bottom };
      })));
      for (const list of tags) {
        assert.ok(list.some((s) => s.text === { en: "Leanest", de: "Sparsamste" }[lang]), `${lang} ${width}px: no "Leanest" tag`);
        for (let i = 0; i < list.length; i++) for (let j = i + 1; j < list.length; j++) {
          const [a, b] = [list[i], list[j]];
          assert.ok(a.right <= b.left || b.right <= a.left || a.bottom <= b.top || b.bottom <= a.top, `${lang} ${width}px: tag "${a.text}" overlaps "${b.text}"`);
        }
      }
      // the advice's value and the one set now keep their labels
      assert.deepEqual(axes[0].map((s) => s.text), ["170K", "400K", "1M"], lang + " " + width + "px");
      assert.deepEqual(axes[1].map((s) => s.text), ["160K", "500K", "990K"], lang + " " + width + "px");
      assert.deepEqual(errors, []);
      await ctx.close();
    }
  });
}

// An agent whose files time its compactions (HirotaNagai on X, a Codex
// user: a window that compacts more often keeps you waiting more): the
// advice is named for tokens and the wait, the why says how long a
// compaction took, the wait at the advice against the window now, and the
// cheaper window it passed over for waiting longer; the caveat no longer
// says tokens only, and each window in the picker says its wait beside its
// tokens, "Fewest tokens" on the cheapest one, not the advice. A row whose
// compactions aren't timed reads as before. In both engines, every
// language, a phone's width and a desk's.
const timedCurve = () => {
  const pts = [];
  for (let v = 100000; v <= 400000; v += 10000) {
    const compacts = Math.round(9e6 / v);
    // 100K the cheapest, but it compacts more often than the window now
    pts.push({ value: v, cost: v === 100000 ? 0.95e9 : 1e9 + Math.abs(v - 200000) * 500, compacts, wait: compacts * 76 });
  }
  return pts;
};
const timedTune = {
  days: 7,
  agents: [{ agent: "codex", calls: 5200, advice: [
    { knob: "compact", setting: { value: "120000", source: "config" },
      compact: { model: "gpt-6-astra", current: 120000, window: 272000, best: 240000, cost: 1.3e9, low: 1.02e9, curve: timedCurve(), wait: 5700, wait_best: 2888, cheapest: 100000, wait_cheapest: 6840, facts: { ...facts, took: 76, timed: 14 } } },
  ] }],
};
const waitBest = { en: "Tokens + wait", zh: "Token + 等待", "zh-TW": "Token + 等待", ja: "トークン + 待ち時間", de: "Tokens + Wartezeit" };
const waitWhy = {
  en: "Each compaction kept you waiting about 76s (14 timed). Over the range that's about 48m at 240K, against 1h 35m at 120K. 100K would spend 7% fewer tokens, but wait about 1h 54m.",
  zh: "每次压缩让你等了约 76 秒（计时 14 次）。这段时间里，240K 下约等 48 分钟，120K 下约等 1 小时 35 分。 100K 能少花 7% Token，但要等约 1 小时 54 分。",
  "zh-TW": "每次壓縮讓你等了約 76 秒（計時 14 次）。這段時間裡，240K 下約等 48 分鐘，120K 下約等 1 小時 35 分。 100K 能少花 7% Token，但要等約 1 小時 54 分。",
  ja: "コンパクト 1 回の待ち時間は約 76 秒（14 回を計測）。この期間では 240K で約 48 分、120K では約 1 時間 35 分待ちます。 100K ならトークンは 7% 減りますが、約 1 時間 54 分待ちます。",
  de: "Jede Kompaktierung ließ dich etwa 76 s warten (14 gemessen). Im Zeitraum sind das bei 240K etwa 48 Min., gegenüber 1 Std. 35 Min. bei 120K. 100K würde 7% weniger Tokens verbrauchen, aber etwa 1 Std. 54 Min. warten lassen.",
};
const waitTip = {
  en: "240K: the fewest tokens without waiting longer on compactions",
  zh: "240K：不多等压缩的前提下，Token 最省",
  "zh-TW": "240K：不多等壓縮的前提下，Token 最省",
  ja: "240K: コンパクトの待ち時間を増やさずに、トークンが最も少ない",
  de: "240K: die wenigsten Tokens, ohne länger auf Kompaktierungen zu warten",
};
const waitCaveat = {
  en: "Tokens and waiting only: it doesn't measure how well the agent keeps the task after a compaction. The lower the window, the more often it compacts.",
  zh: "只算了 Token 和等待：没有衡量压缩后 Agent 还能多好地接着做任务。阈值越低，压缩越频繁。",
  "zh-TW": "只算了 Token 和等待：沒有衡量壓縮後 Agent 還能多好地接著做任務。閾值越低，壓縮越頻繁。",
  ja: "トークンと待ち時間だけの見積もりです。コンパクトのあとエージェントが作業をどれだけ保てるかは測っていません。しきい値が低いほどコンパクトの回数は増えます。",
  de: "Nur Tokens und Wartezeit: Wie gut der Agent die Aufgabe nach einer Kompaktierung behält, misst das nicht. Je niedriger die Schwelle, desto öfter wird kompaktiert.",
};
const waitNote100 = { en: "Fewest tokens · about 90 compactions · about 1h 54m waiting", zh: "最省 Token · 约压缩 90 次 · 约等 1 小时 54 分", "zh-TW": "最省 Token · 約壓縮 90 次 · 約等 1 小時 54 分", ja: "トークン最少 · コンパクト約 90 回 · 待ち約 1 時間 54 分", de: "Wenigste Tokens · etwa 90 Kompaktierungen · etwa 1 Std. 54 Min. Wartezeit" };
const waitNote240 = { en: "+70M tokens (+7%) · about 38 compactions · about 48m waiting", zh: "多花 70M Token（+7%） · 约压缩 38 次 · 约等 48 分钟", "zh-TW": "多花 70M Token（+7%） · 約壓縮 38 次 · 約等 48 分鐘", ja: "70M トークン多い（+7%） · コンパクト約 38 回 · 待ち約 48 分", de: "+70M Tokens (+7%) · etwa 38 Kompaktierungen · etwa 48 Min. Wartezeit" };
for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(`${engine}: timed compactions put their wait beside the tokens`, async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    const errors = [];
    t.after(() => browser.close());
    for (const lang of Object.keys(heading)) for (const [width, height] of [[360, 760], [1100, 760]]) {
      await t.test(`${lang}, ${width}x${height}`, async () => {
        const ctx = await browser.newContext({ viewport: { width, height }, reducedMotion: "reduce" });
        await ctx.addInitScript(() => { try { localStorage.setItem("magpie.usageTab", "context"); } catch {} });
        const page = await ctx.newPage();
        page.setDefaultTimeout(5000);
        page.on("pageerror", (e) => errors.push(e.message));
        const base = serve(lang, []);
        await page.route("**/*", (r) => new URL(r.request().url()).pathname === "/api/tune" ? r.fulfill({ json: timedTune }) : base(r));
        await page.goto("http://magpie.test/?view=usage");
        const row = page.locator(".ctx-tune-row").nth(0);
        await row.locator(".ctx-tune-pick").waitFor();
        await page.locator(".ctx-none").waitFor();
        assert.equal(await row.locator(".ctx-tune-val.best .ctx-k").textContent(), waitBest[lang]);
        assert.equal(await row.locator(".ctx-tc-dot.best .ctx-tc-tag").innerText(), waitBest[lang]);
        assert.equal(await row.locator(".ctx-tc-dot.best").getAttribute("data-tt"), waitTip[lang]);
        const why = await row.locator(".ctx-tune-why").innerText();
        assert.ok(why.endsWith(" " + waitWhy[lang]), `${lang}: ${why}`);
        assert.equal(await row.locator(".ctx-tune-caveat").innerText(), waitCaveat[lang]);
        const over = await page.evaluate(() => {
          const out = [];
          const view = document.querySelector("#view-usage");
          if (view.scrollWidth > view.clientWidth + 1) out.push("page " + view.scrollWidth + ">" + view.clientWidth);
          for (const c of document.querySelectorAll(".ctx-tune")) {
            const b = c.getBoundingClientRect();
            for (const e of c.querySelectorAll("*")) {
              const r = e.getBoundingClientRect();
              if (r.width && (r.right > b.right + 1 || r.left < b.left - 1)) out.push((e.className.baseVal ?? e.className) + " " + Math.round(r.left) + "–" + Math.round(r.right));
            }
          }
          return out;
        });
        assert.deepEqual(over, []);
        await row.locator(".ctx-tune-pick").evaluate((b) => b.click());
        const menu = page.locator(".proto-menu.ctx-tune-menu");
        await menu.waitFor();
        const item = (v) => menu.locator(".pm-item", { has: page.locator(".pm-name", { hasText: new RegExp("^" + v + "$") }) });
        assert.equal(await item("100K").locator(".pm-note").innerText(), waitNote100[lang]);
        assert.equal(await item("240K").locator(".pm-note").innerText(), waitNote240[lang]);
        const box = await menu.boundingBox();
        assert.ok(box.x >= 0 && box.x + box.width <= width + 1, `the menu is inside the window: ${JSON.stringify(box)}`);
        await page.keyboard.press("Escape");
        await ctx.close();
      });
    }
    assert.deepEqual(errors, []);
  });
}

// A window the reader picked past the leanest band (yapechan on Discord:
// "Other window…" to a larger one). With compactions timed, tune's advice
// never waits longer than the window now, so the picked window comes back
// as its own advice; the row said "Already the leanest" at +52% tokens.
// It now says it isn't, names the band (the windows within 3% of the
// cheapest), and offers the band's largest window, which compacts least of
// them; a pick inside the band still says it is the leanest and offers
// nothing. In both engines, every language, a phone's width and a desk's.
const bandCurve = () => {
  const pts = [];
  for (let v = 100000; v <= 500000; v += 10000) {
    const compacts = Math.round(9e6 / v);
    pts.push({ value: v, cost: 1e9 + Math.max(0, Math.abs(v - 250000) - 20000) * 4000, compacts, wait: compacts * 61 });
  }
  return pts;
};
const bandTune = (value) => {
  const pts = bandCurve(), at = pts.find((p) => p.value === value);
  return {
    days: 7,
    agents: [{ agent: "claude", calls: 4300, advice: [
      { knob: "compact", model: "claude-opus-5-5", setting: { value: String(value), source: "model", ours: true },
        compact: { model: "claude-opus-5-5", current: value, window: 1000000, best: value, cost: at.cost, low: at.cost, curve: pts, wait: at.wait, wait_best: at.wait, cheapest: 250000, wait_cheapest: 2196, facts: { ...facts, took: 61, timed: 9 } } },
    ] }],
  };
};
const notLeanest = {
  en: "Not the leanest: 230K–270K spends the fewest tokens",
  zh: "当前不是最省：230K–270K 最省 Token",
  "zh-TW": "目前不是最省：230K–270K 最省 Token",
  ja: "最少ではありません：230K–270K でトークンが最も少なくなります",
  de: "Nicht am sparsamsten: 230K–270K verbraucht die wenigsten Tokens",
};
const leanest = { en: "Already the leanest for how you work", zh: "已是最适合你用法的最省配置", "zh-TW": "已是最適合你用法的最省配置", ja: "すでにあなたの使い方で最も省トークン", de: "Schon am sparsamsten für deine Arbeitsweise" };
const use270 = { en: "Use 270K", zh: "使用 270K", "zh-TW": "使用 270K", ja: "270K にする", de: "270K verwenden" };
for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(`${engine}: a window picked past the leanest band isn't called the leanest`, async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    const errors = [];
    t.after(() => browser.close());
    for (const lang of Object.keys(heading)) for (const [width, height] of [[360, 760], [1100, 760]]) {
      await t.test(`${lang}, ${width}x${height}`, async () => {
        const ctx = await browser.newContext({ viewport: { width, height }, reducedMotion: "reduce" });
        await ctx.addInitScript(() => { try { localStorage.setItem("magpie.usageTab", "context"); } catch {} });
        const page = await ctx.newPage();
        page.setDefaultTimeout(5000);
        page.on("pageerror", (e) => errors.push(e.message));
        const posts = [];
        let value = 400000;
        const base = serve(lang, posts);
        await page.route("**/*", (r) => {
          const p = new URL(r.request().url()).pathname;
          if (p === "/api/tune") return r.fulfill({ json: bandTune(value) });
          if (p === "/api/tune/apply") {
            const b = JSON.parse(r.request().postData());
            posts.push(b);
            value = +b.value;
            return r.fulfill({ json: { value: b.value, source: "model", ours: true } });
          }
          return base(r);
        });
        await page.goto("http://magpie.test/?view=usage");
        const row = page.locator(".ctx-tune-row").nth(0);
        await row.locator(".ctx-tune-pick").waitFor();
        const chip = row.locator(".ctx-tune-foot .ctx-chip");
        assert.equal(await chip.innerText(), notLeanest[lang]);
        assert.equal(await chip.getAttribute("class"), "ctx-chip info");
        const use = row.locator(".ctx-tune-foot button.primary");
        assert.equal(await use.count(), 1);
        assert.equal(await use.innerText(), use270[lang]);
        const over = await page.evaluate(() => {
          const view = document.querySelector("#view-usage");
          const out = view.scrollWidth > view.clientWidth + 1 ? ["page " + view.scrollWidth + ">" + view.clientWidth] : [];
          const b = document.querySelector(".ctx-tune").getBoundingClientRect();
          for (const e of document.querySelectorAll(".ctx-tune-foot *")) {
            const r = e.getBoundingClientRect();
            if (r.width && (r.right > b.right + 1 || r.left < b.left - 1)) out.push(e.className + " " + Math.round(r.left) + "–" + Math.round(r.right));
          }
          return out;
        });
        assert.deepEqual(over, []);
        await use.evaluate((b) => b.click());
        await row.locator(".ctx-tune-foot .ctx-chip.good").waitFor();
        assert.deepEqual(posts.map((p) => [p.agent, p.knob, p.model, p.value]), [["claude", "compact", "claude-opus-5-5", "270000"]]);
        // inside the band: the leanest, and no button to change it
        const row2 = page.locator(".ctx-tune-row").nth(0);
        assert.equal(await row2.locator(".ctx-tune-foot .ctx-chip").innerText(), leanest[lang]);
        assert.equal(await row2.locator(".ctx-tune-foot button.primary").count(), 0);
        await ctx.close();
      });
    }
    assert.deepEqual(errors, []);
  });
}

// A model's own Compact at, over Long conversations (hoobnn on Discord):
// Long conversations held the 170K magpie put in, the advice was 170K, and
// the row said "In use" beside "Now 430K", where the provider's own
// threshold has Codex compact. It now names that threshold, as it does a
// variable that comes first, and offers no window Long conversations
// can't make so; Undo still puts Long conversations back. Without one,
// Codex runs at the advice, which says so. In both engines, every language, a phone's
// width and a desk's.
const ownTune = (own) => ({
  days: 1,
  agents: [{ agent: "codex", calls: 2808, advice: [
    { knob: "compact", setting: { value: "170000", source: "long", ours: true }, ...(own ? { own: { model: "cpa/gpt-6-astra", value: 430000 } } : {}),
      compact: { model: "cpa/gpt-6-astra", current: own ? 430000 : 170000, window: 440000, best: 170000, cost: own ? 1.4e9 : 1.02e9, low: 1.02e9, curve: curve(90000, 440000, 170000), facts } },
  ] }],
});
const ownNote = {
  en: "cpa/gpt-6-astra compacts at its own 430K, which comes first",
  zh: "cpa/gpt-6-astra 有自己的压缩阈值 430K，它优先",
  "zh-TW": "cpa/gpt-6-astra 有自己的壓縮閾值 430K，它優先",
  ja: "cpa/gpt-6-astra には独自の圧縮しきい値 430K があり、そちらが優先されます",
  de: "cpa/gpt-6-astra hat ein eigenes „Komprimieren ab“ (430K), das Vorrang hat",
};
const inUse = { en: "In use", zh: "使用中", "zh-TW": "使用中", ja: "使用中", de: "In Verwendung" };
for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  test(`${engine}: a model's own Compact at isn't called In use`, async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    const errors = [];
    t.after(() => browser.close());
    for (const lang of Object.keys(heading)) for (const [width, height] of [[360, 760], [1100, 760]]) {
      await t.test(`${lang}, ${width}x${height}`, async () => {
        const ctx = await browser.newContext({ viewport: { width, height }, reducedMotion: "reduce" });
        await ctx.addInitScript(() => { try { localStorage.setItem("magpie.usageTab", "context"); } catch {} });
        const page = await ctx.newPage();
        page.setDefaultTimeout(5000);
        page.on("pageerror", (e) => errors.push(e.message));
        let own = true;
        const base = serve(lang, []);
        await page.route("**/*", (r) => new URL(r.request().url()).pathname === "/api/tune" ? r.fulfill({ json: ownTune(own) }) : base(r));
        await page.goto("http://magpie.test/?view=usage");
        const row = page.locator(".ctx-tune-row").nth(0);
        await row.locator(".ctx-tune-foot .ctx-chip").waitFor();
        assert.equal(await row.locator(".ctx-tune-src").innerText(), ownNote[lang]);
        assert.notEqual(await row.locator(".ctx-tune-foot .ctx-chip").innerText(), inUse[lang]);
        assert.equal(await row.locator(".ctx-tune-foot .ctx-chip").getAttribute("class"), "ctx-chip info");
        assert.equal(await row.getAttribute("class"), "ctx-tune-row", "Now is struck through: it isn't the advice");
        assert.equal(await row.locator(".ctx-tune-foot button.primary").count(), 0);
        assert.equal(await row.locator(".ctx-tune-pick").count(), 0);
        assert.equal(await row.locator(".ctx-tune-foot button.text:not(.primary)").count(), 1, "Undo stays");
        const over = await page.evaluate(() => {
          const view = document.querySelector("#view-usage");
          const out = view.scrollWidth > view.clientWidth + 1 ? ["page " + view.scrollWidth + ">" + view.clientWidth] : [];
          const b = document.querySelector(".ctx-tune").getBoundingClientRect();
          for (const e of document.querySelectorAll(".ctx-tune *")) {
            const r = e.getBoundingClientRect();
            if (r.width && (r.right > b.right + 1 || r.left < b.left - 1)) out.push((e.className.baseVal ?? e.className) + " " + Math.round(r.left) + "–" + Math.round(r.right));
          }
          return out;
        });
        assert.deepEqual(over, []);
        // Long conversations is where Codex runs: it runs at the advice
        own = false;
        await page.evaluate(() => window.loadContext());
        await page.locator(".ctx-tune-row.done .ctx-chip.good").waitFor();
        assert.equal(await page.locator(".ctx-tune-row .ctx-tune-foot .ctx-chip").innerText(), leanest[lang]);
        await ctx.close();
      });
    }
    assert.deepEqual(errors, []);
  });
}
