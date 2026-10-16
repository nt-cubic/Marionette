// A gateway key can be held to some accounts (#905): the key's models
// menu lists the accounts and keys right after their own provider's
// models, a plan in their note, and what is picked is sent as
// "accounts-key" — "All models" takes them all off, a model picked alone
// leaves the accounts as they were.
const assert = require("node:assert/strict");
const { test } = require("node:test");
const { chromium, webkit } = require("playwright");
const { fixture } = require("./fixtures/caller-keys.cjs");

for (const engine of (process.env.BROWSER ? [process.env.BROWSER] : ["chromium", "webkit"])) {
  for (const lang of ["en", "zh"]) {
    test(`${engine} ${lang}: a gateway key is held to the accounts picked for it`, async (t) => {
      const browser = await (engine === "webkit" ? webkit.launch() : chromium.launch({ channel: "chromium" }));
      t.after(() => browser.close());
      const page = await (await browser.newContext({ viewport: { width: 1000, height: 600 }, reducedMotion: "reduce" })).newPage();
      page.setDefaultTimeout(6000);
      const events = [], errors = [];
      page.on("pageerror", (e) => errors.push(e.message));
      await page.route("**/*", fixture(lang, "light", events, { lan: true, keyHolds: { server: { accounts: ["relay/me@example.com"] }, laptop: { models: ["relay/gone"], accounts: ["relay/nobody@example.com"] } } }));
      const zh = lang === "zh";
      const w = zh
        ? { label: "此密钥可用的模型", all: "全部模型", gone: "当前未提供", off: "当前未登录", used: "会被使用：OpenAI 没有勾选任何账号", hint: "没勾选任何账号的供应商" }
        : { label: "Models this key may use", all: "All models", gone: "Not served now", off: "Not signed in now", used: "Used: no OpenAI account is picked", hint: "A provider with no account picked is used on every account" };
      await page.goto("http://magpie.test/?view=gateway");
      await page.locator("#gatewayKeys .acc[data-key]").last().waitFor();
      const row = (id) => page.locator(`#gatewayKeys .acc[data-key="${id}"]`);
      // what the menu sends on close is sent asynchronously: wait for it
      const until = async (fn) => {
        for (let i = 0; i < 150 && !fn(); i++) await new Promise((r) => setTimeout(r, 20));
      };
      const badge = (id) => row(id).getByRole("button", { name: w.label, exact: true });
      // a key held to one account says which
      assert.equal((await badge("server").textContent()).trim(), "relay/me@example.com");
      assert.match(await badge("server").getAttribute("title"), /relay\/me@example\.com/);
      const view = page.locator("#view-gateway");
      const scrolled = async () => [await view.evaluate((v) => v.scrollTop), await page.evaluate(() => window.scrollY)];
      await row("work").hover();
      await badge("work").click();
      const menu = page.locator(".proto-menu");
      await menu.waitFor();
      assert.equal(await page.locator("select").count(), 0, "no native select");
      // the accounts come right after their own provider's models, not
      // one block at the end: relay's two behind its models and ahead of
      // OpenAI's, the key behind GPT-5 — an account by who is signed in
      // with their plan in the note, a key by its fingerprint
      const names = (await menu.locator(".pm-item").allTextContents()).map((s) => s.trim());
      assert(!(await badge("work").getAttribute("title")).includes(w.hint), "a key held to no account names the rule");
      assert.equal((await menu.locator(".pm-item", { hasText: "k-1a2b3c" }).locator(".pm-note").textContent()).trim(), "OpenAI");
      const at = (s) => names.findIndex((n) => n.includes(s));
      assert(at("me@example.com") > at("Model mini"), names.join(" | "));
      assert(at("me@example.com") < at("GPT-5"), names.join(" | "));
      assert.match(names[at("me@example.com")], /Pro/);
      assert(at("spare@example.com") > at("me@example.com"), names.join(" | "));
      assert(at("spare@example.com") < at("GPT-5"), names.join(" | "));
      assert(at("k-1a2b3c") === at("GPT-5") + 1, names.join(" | "));
      assert(at("k-1a2b3c") > at("GPT-5"), names.join(" | "));
      await menu.locator(".pm-item", { hasText: "spare@example.com" }).click();
      await page.keyboard.press("Escape");
      await menu.waitFor({ state: "detached" });
      await until(() => events.some((e) => e.action === "accounts-key"));
      assert.deepEqual(events.filter((e) => e.action === "accounts-key").map((e) => e.body), [{ key: "work", accounts: ["relay/spare@example.com"] }]);
      // a model picked alone leaves the key's accounts as they were
      events.length = 0;
      await row("work").hover();
      await badge("work").click();
      await menu.waitFor();
      await menu.locator(".pm-item", { hasText: "GPT-5" }).click();
      await page.keyboard.press("Escape");
      await menu.waitFor({ state: "detached" });
      await until(() => events.some((e) => e.action === "models-key"));
      assert.deepEqual(events.filter((e) => e.action === "models-key").map((e) => e.body), [{ key: "work", models: ["openai/gpt-5"] }]);
      assert.equal(events.filter((e) => e.action === "accounts-key").length, 0, "a model pick sent the accounts");
      // a key held to an account has it ticked; All models takes them
      // all off, models and accounts
      await badge("server").click();
      await menu.waitFor();
      assert.deepEqual((await menu.locator(".pm-item.on").allTextContents()).map((s) => s.trim()), ["me@example.comRelay · Pro"]);
      // #2077: the list holds the key to the accounts of the providers it
      // names only, so OpenAI's key, none of OpenAI's picked, is used, and
      // says so; Relay's unpicked account is not, and says its provider
      const noteOf = async (s) => (await menu.locator(".pm-item", { hasText: s }).locator(".pm-note").textContent()).trim();
      assert.equal(await noteOf("k-1a2b3c"), w.used);
      assert.equal(await noteOf("spare@example.com"), "Relay");
      assert(String(await badge("server").getAttribute("title")).includes(w.hint), await badge("server").getAttribute("title"));
      await menu.locator(".pm-item", { hasText: "me@example.com" }).click();
      await menu.locator(".pm-item", { hasText: "k-1a2b3c" }).click();
      await page.keyboard.press("Escape");
      await menu.waitFor({ state: "detached" });
      await until(() => events.filter((e) => e.action === "accounts-key").length > 0 && events.filter((e) => e.action === "accounts-key").at(-1).body.key === "server");
      assert.deepEqual(events.filter((e) => e.action === "accounts-key").at(-1).body, { key: "server", accounts: ["openai/k-1a2b3c"] });
      await badge("server").click();
      await menu.waitFor();
      await menu.locator(".pm-item", { hasText: w.all }).first().click();
      await menu.waitFor({ state: "detached" });
      await until(() => events.some((e) => e.action === "accounts-key" && e.body.key === "server" && !e.body.accounts.length));
      assert.deepEqual(events.filter((e) => e.action === "models-key").at(-1).body, { key: "server", models: [] });
      assert.deepEqual(events.filter((e) => e.action === "accounts-key").at(-1).body, { key: "server", accounts: [] });
      // the answers come back before the row is drawn with them: wait for it
      await page.waitForFunction((want) => document.querySelector('#gatewayKeys .acc[data-key="server"] .key-models')?.textContent.trim() === want, w.all);
      assert.equal((await badge("server").textContent()).trim(), w.all);
      assert.deepEqual(await scrolled(), [await view.evaluate((v) => v.scrollTop), await page.evaluate(() => window.scrollY)], "a click moved the page");
      // what a key kept that no provider has now is listed behind every
      // live entry, a model gone before an account signed out, both ticked
      await row("laptop").hover();
      await badge("laptop").click();
      await menu.waitFor();
      const dead = (await menu.locator(".pm-item").allTextContents()).map((s) => s.trim());
      const atDead = (s) => dead.findIndex((n) => n.includes(s));
      assert(dead[atDead("relay/gone")] === "relay/gone" + w.gone, dead.join(" | "));
      assert(dead[atDead("relay/nobody@example.com")] === "relay/nobody@example.com" + w.off, dead.join(" | "));
      assert(atDead("relay/gone") > atDead("k-1a2b3c"), dead.join(" | "));
      assert(atDead("relay/nobody@example.com") > atDead("relay/gone"), dead.join(" | "));
      assert.deepEqual((await menu.locator(".pm-item.on").allTextContents()).map((s) => s.trim()), ["relay/gone" + w.gone, "relay/nobody@example.com" + w.off]);
      await page.keyboard.press("Escape");
      await menu.waitFor({ state: "detached" });
      const lefts = await page.locator("#gatewayKeys .key-models, .proto-menu .pm-item").evaluateAll((els) => els.map((e) => getComputedStyle(e).borderLeftStyle));
      assert(lefts.every((s) => s === "none"), "a coloured left border");
      assert.deepEqual(errors, []);
    });
  }
}
