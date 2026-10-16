const assert = require("node:assert/strict");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { fixture } = require("./fixtures/caller-keys.cjs");

// Gateway keys are left out of Gateway while magpie isn't shared, and a
// tunnel to loopback (Cloudflare Tunnel) is refused until it is. DD on
// Discord looked for the keys to put a tunnel behind one and found none:
// the page says where they come from, and its button opens Settings ›
// Network and sharing. Shared, the line gives way to the keys.
for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const locale of ["en", "zh", "zh-TW", "ja", "de"]) {
    for (const width of [1100, 420]) {
      test(`${engine} ${locale} ${width}: Gateway says where gateway keys come from while not shared`, async (t) => {
        const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
        t.after(() => browser.close());
        const page = await (await browser.newContext({ viewport: { width, height: 760 }, reducedMotion: "reduce" })).newPage();
        page.setDefaultTimeout(6000);
        const errors = [];
        page.on("pageerror", (e) => errors.push(e.message));
        await page.route("**/*", fixture(locale, "dark", []));
        await page.goto("http://magpie.test/?view=gateway");
        await page.locator("#connect .snip").waitFor();

        const hint = page.locator("#gatewayKeysOff");
        await hint.waitFor({ state: "visible" });
        assert.equal(await page.locator("#gatewayKeysBlock").isVisible(), false);
        const text = await hint.textContent();
        assert(text.includes("Cloudflare Tunnel"), text);
        if (locale !== "en") assert(!text.includes("other computers"), "translated: " + text);
        // the line fits the page, the button inside it
        const fits = await hint.evaluate((n) => {
          const b = n.querySelector("button").getBoundingClientRect(), p = n.getBoundingClientRect();
          return b.right <= p.right + 1 && p.right <= document.documentElement.clientWidth + 1;
        });
        assert(fits, "the hint overflows at " + width);

        const y = await page.evaluate(() => scrollY);
        await page.locator("#openLanSettings").click();
        await page.locator("#setPage-network").waitFor({ state: "visible" });
        assert.equal(await page.locator("#setTab-network").getAttribute("aria-selected"), "true");
        const share = await page.locator("#lanList .row").first().textContent();
        assert(share.includes("Cloudflare Tunnel"), share);

        // shared: the keys show and the line is gone
        const on = page.locator("#lanList .segs button").last();
        const bounds = await on.boundingBox();
        await page.mouse.move(width / 2, 400);
        await page.mouse.wheel(0, bounds.y - 250);
        await on.click();
        await page.locator("#lanList .lan-address-row").waitFor();
        await page.goto("http://magpie.test/?view=gateway");
        await page.locator("#gatewayKeysBlock").waitFor({ state: "visible" });
        assert.equal(await hint.isVisible(), false);
        assert.equal(y, 0);
        assert.deepEqual(errors, []);
      });
    }
  }
}
