// Drive the shipped callbacks with held API replies. A background read and
// two different card refreshes may finish in any order (#1630).
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const vm = require("node:vm");
const { test } = require("node:test");

const source = fs.readFileSync(process.env.MAGPIE_APP_JS || path.resolve(__dirname, "../assets/app.js"), "utf8").replace(/\r\n/g, "\n");
function section(start, end) {
  const a = source.indexOf(start), b = source.indexOf(end, a);
  assert(a >= 0 && b > a, `missing ${start}`);
  return source.slice(a, b);
}
const plain = (x) => JSON.parse(JSON.stringify(x));
const card = (user, used) => ({ provider: "claude", user, windows: [{ name: "7 days", used }] });

function page() {
  const context = vm.createContext({ Date, performance, setTimeout });
  vm.runInContext(`
    let quotas = [], quotasAt = 0, quotaHist = [], quotasAskedAt = 0, view = "usage";
    const requests = [], errors = [];
    const usageAcctKey = q => q.provider + "\\n" + (q.user || "").toLowerCase();
    const quotaCycleHolds = new Map(); // past cycles' estimates (#2009), cleared by a refresh
    const t = x => x;
    const el = () => ({ classList: { add() {} }, setAttribute() {} });
    function renderQuotas() {}
    function loadUpstream() {}
    function status(x) { errors.push(x); }
    function api(path) {
      if (path.includes("history")) return Promise.resolve([]);
      return new Promise((resolve, reject) => requests.push({ path, resolve, reject }));
    }
    ${section("let quotasLoading =", "// A period of picked days is")}
    ${section("const quotaRefreshing =", "// familyQuota:")}
    globalThis.p = {
      requests, errors,
      seed: x => { quotas = x; },
      cards: () => quotas,
      load: () => loadQuotas(),
      refresh: q => quotaRefresh(q).onclick({ stopPropagation() {} }),
      answer: (i, x) => requests[i].resolve(x),
      fail: i => requests[i].reject(new Error("offline")),
    };
  `, context);
  return context.p;
}
const tick = () => new Promise(setImmediate);

for (const when of ["before", "during"]) {
  test(`a background read started ${when} a card refresh cannot undo its result`, async () => {
    const p = page(), old = card("a@x.com", 0), fresh = card("a@x.com", 32);
    p.seed([old]);
    let loading, refreshing;
    if (when === "before") {
      loading = p.load(); await tick(); refreshing = p.refresh(old);
    } else {
      refreshing = p.refresh(old); loading = p.load(); await tick();
    }
    const post = p.requests.findIndex(r => r.path.includes("/refresh?"));
    const get = p.requests.findIndex(r => r.path === "usage/quotas");
    assert(post >= 0 && get >= 0);
    p.answer(post, [fresh]); await refreshing;
    assert.equal(p.cards()[0].windows[0].used, 32);
    p.answer(get, [old]); await loading;
    assert.equal(p.cards()[0].windows[0].used, 32, "an older whole-page reply overwrote the refreshed card");
    // A later whole-page reading remains authoritative, including removals.
    const later = p.load(); await tick();
    p.answer(p.requests.length - 1, []); await later;
    assert.deepEqual(plain(p.cards()), []);
  });
}

test("refreshing two cards keeps both results when their full-list replies cross", async () => {
  const p = page(), a = card("a@x.com", 0), b = card("b@x.com", 0);
  p.seed([a, b]);
  const first = p.refresh(a), second = p.refresh(b);
  p.answer(1, [a, card("b@x.com", 61)]); await second;
  p.answer(0, [card("a@x.com", 32), b]); await first;
  assert.deepEqual(plain(p.cards()).map(q => q.windows[0].used), [32, 61], "one card's reply undid another card's completed refresh");
});

test("a failed card refresh leaves background readings usable", async () => {
  const p = page(), old = card("a@x.com", 0);
  p.seed([old]);
  const refresh = p.refresh(old), loading = p.load(); await tick();
  p.fail(0); await refresh;
  p.answer(1, [card("a@x.com", 32)]); await loading;
  assert.equal(p.cards()[0].windows[0].used, 32);
  assert.deepEqual(plain(p.errors), ["offline"]);
});

test("a card that comes back under another key still shows the reply", async () => {
  // A card read before its account was known has no user; its refresh
  // answers under the account's name.
  const p = page(), unnamed = card("", 0);
  p.seed([unnamed]);
  const refresh = p.refresh(unnamed);
  p.answer(0, [card("a@x.com", 32)]); await refresh;
  assert.deepEqual(plain(p.cards()).map(q => [q.user, q.windows[0].used]), [["a@x.com", 32]], "the refreshed card's reply was dropped");
});

test("an account first named by refresh survives an older whole-page reply", async () => {
  const p = page(), unnamed = card("", 0);
  p.seed([unnamed]);
  const loading = p.load(); await tick();
  const refresh = p.refresh(unnamed);
  p.answer(1, [card("a@x.com", 32)]); await refresh;
  p.answer(0, [unnamed]); await loading;
  assert.deepEqual(plain(p.cards()).map(q => [q.user, q.windows[0].used]), [["a@x.com", 32]], "an old reply restored the unnamed card over its refresh");
});

test("an account first named by refresh keeps another card's completed refresh", async () => {
  const p = page(), unnamed = card("", 0), b = card("b@x.com", 0);
  p.seed([unnamed,b]);
  const first = p.refresh(unnamed), second = p.refresh(b);
  p.answer(1,[unnamed,card("b@x.com",61)]); await second;
  p.answer(0,[card("a@x.com",32),b]); await first;
  assert.deepEqual(plain(p.cards()).map(q => [q.user,q.windows[0].used]), [["a@x.com",32],["b@x.com",61]], "the renamed card's reply undid another card's completed refresh");
});

test("a renamed card cannot retire another provider's protection against a pending GET", async () => {
  const p = page(), unnamed = card("", 0), b = { ...card("b@x.com", 0), provider: "codex" };
  const freshB = { ...card("b@x.com", 61), provider: "codex" };
  p.seed([unnamed, b]);
  const loading = p.load(); await tick();
  const second = p.refresh(b);
  p.answer(1, [unnamed, freshB]); await second;
  const first = p.refresh(unnamed);
  p.answer(2, [card("a@x.com", 32), b]); await first;
  const readings = () => Object.fromEntries(plain(p.cards()).map(q => [q.user, q.windows[0].used]));
  assert.deepEqual(readings(), { "a@x.com": 32, "b@x.com": 61 }, "the provider refresh replaced another provider's newer card");
  p.answer(0, [unnamed, b]); await loading;
  assert.deepEqual(readings(), { "a@x.com": 32, "b@x.com": 61 }, "the card reply retired protection needed by the older GET");
});

test("a remote summary becomes its child cards without undoing a local refresh", async () => {
  const p = page(), summary = { ...card("", 0), provider: "home", from: "Home" };
  const child = { ...card("a@x.com", 32), provider: "home/claude", from: "Home" };
  const local = { ...card("b@x.com", 0), provider: "codex" };
  const freshLocal = { ...local, windows: [{ name: "7 days", used: 61 }] };
  p.seed([summary, local]);
  const loading = p.load(); await tick();
  const remote = p.refresh(summary), own = p.refresh(local);
  p.answer(2, [summary, freshLocal]); await own;
  p.answer(1, [child, local]); await remote;
  const readings = () => Object.fromEntries(plain(p.cards()).map(q => [q.provider, q.windows[0].used]));
  assert.deepEqual(readings(), { "home/claude": 32, codex: 61 });
  p.answer(0, [summary, local]); await loading;
  assert.deepEqual(readings(), { "home/claude": 32, codex: 61 }, "the late GET restored the remote summary or an old local reading");
});
