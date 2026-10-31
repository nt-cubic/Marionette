// Run with Node's test runner and Playwright on the module path; see README.md.
// In magpie web at phone width, the Gateway page's Connect menus (the
// address, the gateway key) shut again as soon as they opened, with or
// without MAGPIE_PUBLIC_URL (found during #1551). The phone header takes two
// rows, so Connect is below the fold, and a scroll of the view that wasn't
// the reader's came in the frame after the click: the browser bringing the
// address into sight, code, a scroll whose event lands after the tap. An app
// menu took any scroll outside it as the reader's and closed. Such a scroll
// is put back by the click guard, which holds the anchor where it was, so
// the menu now stays beside it; the reader's own scroll (the wheel, a
// finger, the keys) still closes it, and a scroll of anything that isn't a
// view does as before.
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { fixture } = require("./fixtures/caller-keys.cjs");

const PUBLIC = "https://magpie.example.com";
const LANGS = ["en", "zh", "zh-TW", "ja", "de"];

async function setup(t, engine, lang, width, pub) {
  const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
  t.after(() => browser.close());
  const page = await (await browser.newContext({ viewport: { width, height: 800 }, reducedMotion: "reduce" })).newPage();
  await page.addInitScript(() => { try { localStorage.setItem("magpie.gwConnectFolded", "0"); } catch {} }); // Connect opened, as a user who reads it
  page.setDefaultTimeout(6000);
  const errors = [];
  page.on("pageerror", (e) => errors.push(e.message));
  const options = pub ? { lanURLs: [PUBLIC], publicURL: PUBLIC, web: true } : { lanURLs: ["http://192.168.1.10:3999"], web: true };
  await page.route("**/*", fixture(lang, "light", [], { lan: true, ...options }));
  await page.goto("http://magpie.test/?view=gateway");
  await page.locator("#connectAddress").waitFor();
  // with the public URL Connect starts there, and picks a gateway key for
  // it; its read redraws Connect
  if (pub) await page.waitForFunction(() => (document.querySelector("#connect pre.snip")?.textContent || "").includes("fixture-laptop"));
  await page.locator("#connectKey").waitFor();
  await page.waitForTimeout(300);
  return { page, errors };
}

const view = (page) => page.locator("#view-gateway");
const top = async (page, sel) => (await page.locator(sel).boundingBox()).y;
const menuOpen = (page) => page.locator(".proto-menu").count();

// the reader's own scroll: the wheel, over the view, till sel is in sight
async function readerBrings(page, sel) {
  const vb = await view(page).boundingBox();
  await page.mouse.move(vb.x + vb.width / 2, vb.y + vb.height / 2);
  for (let i = 0; i < 30; i++) {
    const r = await page.locator(sel).boundingBox();
    if (r.y > vb.y + 40 && r.y + r.height < vb.y + vb.height - 200) return;
    await page.mouse.wheel(0, r.y > vb.y + vb.height / 2 ? 120 : -120);
    await page.waitForTimeout(80);
  }
  throw new Error(sel + " never came into sight");
}

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of LANGS) {
    for (const width of [420, 1100]) {
      for (const pub of [true, false]) {
        test(`${engine} ${lang} ${width} ${pub ? "public URL" : "no public URL"}: Connect's menus stay open till the reader scrolls`, async (t) => {
          const { page, errors } = await setup(t, engine, lang, width, pub);
          for (const sel of ["#connectAddress", "#connectKey"]) {
            await readerBrings(page, sel);
            await page.locator(sel).click();
            await page.locator(".proto-menu").waitFor();
            const at = await top(page, sel);
            // a scroll of the view the reader didn't make, in the frame after
            // the click: put back, and the menu stays beside its anchor
            await view(page).evaluate((v) => requestAnimationFrame(() => { v.scrollTop += 40; }));
            await page.waitForTimeout(300);
            assert.equal(await menuOpen(page), 1, `${sel}: the menu stays open after a scroll that wasn't the reader's`);
            assert.ok(Math.abs((await top(page, sel)) - at) < 1, `${sel}: its anchor stays where it was`);
            const [m, a] = [await page.locator(".proto-menu").boundingBox(), await page.locator(sel).boundingBox()];
            assert.ok(Math.abs(m.y - (a.y + a.height + 5)) < 2 || Math.abs(m.y + m.height + 5 - a.y) < 2, `${sel}: the menu is beside its anchor`);
            assert.ok(m.x >= 0 && m.x + m.width <= width, `${sel}: the menu fits the window`);
            // the reader's wheel closes it
            await page.mouse.wheel(0, 60);
            await page.locator(".proto-menu").waitFor({ state: "detached" });
          }
          assert.deepEqual(errors, []);
        });
      }
    }
  }
  // the case as found: the page at its top, the address below the fold at
  // phone width, brought into sight by a scroll that isn't the reader's
  // (a test's click, code) and clicked before the page hears of that
  // scroll, in one task as Playwright's own click can (#1554). The click
  // guard puts the scroll back and brings the focused address into sight
  // itself, before the menu opens; that scroll is the guard's, not the
  // reader's, and leaves the menu open beside the address.
  for (const pub of [true, false]) {
    test(`${engine} 420 ${pub ? "public URL" : "no public URL"}: the address clicked from the page's top opens its menu`, async (t) => {
      const { page, errors } = await setup(t, engine, "en", 420, pub);
      assert.ok((await top(page, "#connectAddress")) > 800, "the address starts below the fold");
      await page.evaluate(() => {
        const a = document.querySelector("#connectAddress");
        a.scrollIntoView({ block: "center" });
        a.focus();
        a.click();
      });
      await page.waitForTimeout(400);
      assert.equal(await menuOpen(page), 1, "the menu stays open");
      const [m, a] = [await page.locator(".proto-menu").boundingBox(), await page.locator("#connectAddress").boundingBox()];
      assert.ok(a.y >= 0 && a.y + a.height <= 800, "the address is in sight");
      assert.ok(Math.abs(m.y - (a.y + a.height + 5)) < 2 || Math.abs(m.y + m.height + 5 - a.y) < 2, "the menu is beside the address");
      await page.locator(".proto-menu .pm-item").first().click();
      await page.locator(".proto-menu").waitFor({ state: "detached" });
      assert.deepEqual(errors, []);
    });
  }
}
