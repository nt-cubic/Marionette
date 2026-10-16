const assert = require("node:assert/strict");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { fixture } = require("./fixtures/caller-keys.cjs");
const { openPart } = require("./reader.cjs");

// coeo91 on Discord: the LAN address picked as Gateway's Base URL went back
// to 127.0.0.1 after a restart or an update. The address and the gateway key
// picked in Connect, and the address picked under Share on local network,
// are kept across reloads; an address the machine doesn't have right now
// (the network not up yet after a start) shows this computer's, and the
// pick comes back with it.
for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh", "zh-TW", "ja", "de"]) {
    for (const width of [1100, 420]) {
      test(`${engine} ${lang} ${width}: the address and key picked in Connect are kept`, async (t) => {
        const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
        t.after(() => browser.close());
        const page = await (await browser.newContext({ viewport: { width, height: 800 }, reducedMotion: "reduce" })).newPage();
        page.setDefaultTimeout(6000);
        const errors = [];
        page.on("pageerror", (e) => errors.push(e.message));
        const lanURLs = ["http://192.168.1.10:3999", "http://10.0.0.10:3999"];
        await page.route("**/*", fixture(lang, "light", [], { lan: true, lanURLs }));
        const pick = async (button, text) => {
          await page.locator(button).click();
          await page.locator(".proto-menu .pm-item", { hasText: text }).click();
          await page.locator(".proto-menu").waitFor({ state: "detached" });
        };
        const shown = (sel) => page.locator(sel).textContent();
        const open = async () => {
          await page.goto("http://magpie.test/?view=gateway");
          await page.locator("#connect .val code").first().waitFor();
        };

        await open();
        await pick("#connectAddress", "10.0.0.10");
        await page.waitForFunction(() => document.querySelector("#connectAddress")?.textContent.includes("10.0.0.10"));
        await page.locator("#connectKey").waitFor();
        await pick("#connectKey", "Server");
        await page.waitForFunction(() => document.querySelector("#connectKey")?.textContent.includes("Server"));

        // a restart or an update opens the page again
        await open();
        await page.waitForFunction(() => document.querySelector("#connectKey")?.textContent.includes("Server"));
        assert.match(await shown("#connectAddress"), /10\.0\.0\.10:3999\/v1/);
        assert.match(await shown("#connectKey"), /Server/);
        await page.waitForFunction(() => document.querySelector("#connect pre.snip")?.textContent.includes("fixture-server"));

        // the network isn't up yet: this computer's address is shown, and
        // nothing of the pick is lost
        lanURLs.length = 0;
        await open();
        assert.match(await page.locator("#connect").textContent(), /127\.0\.0\.1:3999\/v1/);
        assert.equal(await page.locator("#connectAddress").count(), 0);
        lanURLs.push("http://192.168.1.10:3999", "http://10.0.0.10:3999");
        await open();
        assert.match(await shown("#connectAddress"), /10\.0\.0\.10:3999\/v1/);
        await page.waitForFunction(() => document.querySelector("#connectKey")?.textContent.includes("Server"));

        // Share on local network's address is kept the same way
        await page.locator("#prefs").click();
        await openPart(page, "network");
        await page.locator("#lanList .lan-address-row").waitFor();
        await pick("#lanList .lan-interface", "10.0.0.10");
        await page.reload();
        await page.locator("#prefs").click();
        await openPart(page, "network");
        await page.locator("#lanList .lan-address-row").waitFor();
        assert.match(await shown("#lanList .lan-interface"), /10\.0\.0\.10:3999/);

        // nothing on the page is wider than the window
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true);
        assert.deepEqual(errors, []);
      });
    }
  }
}
