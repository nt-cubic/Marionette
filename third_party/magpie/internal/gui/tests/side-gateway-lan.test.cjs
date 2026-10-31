const assert = require("node:assert/strict");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { fixture } = require("./fixtures/caller-keys.cjs");
const { openPart, reach } = require("./reader.cjs");

// coeo91 on Discord: with Share on local network on, the sidebar's gateway
// line at its foot still said "Gateway 127.0.0.1:3425", which other
// computers can't reach. Shared, it names the LAN address, the one picked
// under Share on local network, from the first paint; its tooltip names
// both that and this computer's. Sharing turned off or on in Settings, or
// another address picked there, redraws it at once. Not shared, it is
// this computer's, as before.
const LOCAL = "http://127.0.0.1:3999";
const lanURLs = ["http://192.168.1.10:3999", "http://10.0.0.10:3999"];

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
    test(`${engine} ${lang}: the sidebar's gateway names the address shared on the local network`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const page = await (await browser.newContext({ viewport: { width: 1100, height: 800 }, reducedMotion: "reduce" })).newPage();
      page.setDefaultTimeout(6000);
      const errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      // boot.js says the gateway (bootGateway); the providers' read comes
      // 1.5s after, so what is read at once is boot.js's line
      await page.route("**/*", fixture(lang, "light", [], { lan: true, lanURLs, bootGateway: true }));
      const line = () => page.locator("#sideGw span").textContent();
      const tip = () => page.locator("#sideGw").getAttribute("title");
      const says = (host) => page.waitForFunction((h) => document.querySelector("#sideGw span")?.textContent.includes(h), host);

      await page.goto("http://magpie.test/?view=settings");
      await page.locator("#sideGw").waitFor();
      assert.match(await line(), /192\.168\.1\.10:3999/);
      assert.doesNotMatch(await line(), /127\.0\.0\.1/);
      assert.match(await tip(), /192\.168\.1\.10:3999/);
      assert.match(await tip(), /127\.0\.0\.1:3999/);

      // another address picked under Share on local network: the line
      // follows at once, and after a reload
      await openPart(page, "network");
      await page.locator("#lanList .lan-address-row").waitFor();
      await reach(page, page.locator("#lanList .lan-interface"));
      await page.locator("#lanList .lan-interface").click();
      await page.locator(".proto-menu .pm-item", { hasText: "10.0.0.10" }).click();
      await says("10.0.0.10:3999");
      await page.reload();
      await page.locator("#sideGw").waitFor();
      assert.match(await line(), /10\.0\.0\.10:3999/);

      // sharing off: this computer's address, and nothing else in the tooltip
      await openPart(page, "network");
      await page.locator("#lanList .lan-address-row").waitFor();
      await page.locator("#lanList .segs").first().locator(".opt").nth(0).click();
      await page.locator("#lanList .lan-address-row").waitFor({ state: "detached" });
      await says("127.0.0.1:3999");
      assert.doesNotMatch(await tip(), /192\.168|10\.0\.0/);
      assert.match(await tip(), new RegExp(LOCAL.replace(/\./g, "\\.") + "$"));
      // and on again: the address picked comes back
      await page.locator("#lanList .segs").first().locator(".opt").nth(1).click();
      await page.locator("#lanList .lan-address-row").waitFor();
      await says("10.0.0.10:3999");
      assert.deepEqual(errors, []);
    });
  }
  test(`${engine}: not shared, the sidebar's gateway is this computer's`, async (t) => {
    const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
    t.after(() => browser.close());
    const page = await (await browser.newContext({ viewport: { width: 1100, height: 800 }, reducedMotion: "reduce" })).newPage();
    page.setDefaultTimeout(6000);
    const errors = [];
    page.on("pageerror", (e) => errors.push(e.message));
    await page.route("**/*", fixture("en", "light", [], { lanURLs, bootGateway: true }));
    // past the providers' read, which says the same
    const read = page.waitForResponse((r) => r.url().endsWith("/api/providers"));
    await page.goto("http://magpie.test/?view=settings");
    await page.locator("#sideGw").waitFor();
    await read;
    await page.evaluate(() => new Promise(requestAnimationFrame));
    assert.equal(await page.locator("#sideGw span").textContent(), "Gateway 127.0.0.1:3999");
    assert.equal(await page.locator("#sideGw").getAttribute("title"), "Gateway " + LOCAL);
    assert.deepEqual(errors, []);
  });
}
