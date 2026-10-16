// Caller keys use the same account list and edit controls as provider keys.
let gatewayKeys = null, gatewayKeyDraft = null, gatewayKeysBusy = false;
// gatewayOwnDraft is the value typed for a new key of the user's own (one
// its clients already send, love1sbug on X); "" lets magpie make one
let gatewayOwnDraft = "";
// gatewayAccountNames maps a key's stored account entry ("<provider>/<id>",
// #905) to how it is shown; what no provider has now is shown as it is kept
let gatewayAccountNames = {};
let gatewayKeysRead = 0;
// gatewayLimit is the key whose limit editor is open, and its draft
// (#585): what is picked in it is sent only by its Save.
let gatewayLimit = null;
let connectURL = "", connectKeyID = "", connectSecret = null;
// connectWant and connectKeyWant are the address and gateway key picked in
// Gateway's Connect, kept across reloads, restarts and updates (coeo91 on
// Discord: the LAN address went back to 127.0.0.1); "" for a key is This
// computer
let connectWant = "", connectKeyWant = "";
try {
  connectWant = localStorage.getItem("magpie.connectAddress") || "";
  connectKeyWant = localStorage.getItem("magpie.connectKey") || "";
} catch {}
function keepPick(name, v) {
  try { localStorage.setItem(name, v); } catch {}
}

function connectPick(id, label, text, options, value, choose) {
  const b = el("button", "val");
  b.id = id;
  b.type = "button";
  b.setAttribute("aria-label", t(label));
  b.append(el("code", "", text), svg(CHEV, 11, 1.6));
  b.onclick = () => {
    if (b.classList.contains("open")) return closeProtoMenu();
    openProtoMenu(b, options, value, choose, label);
  };
  return b;
}

async function selectConnectKey(id) {
  connectKeyID = id;
  connectSecret = null;
  renderConnect();
  if (!id || !providers?.gateway.lan) return;
  const key = gatewayKeys?.find((k) => k.id === id && !k.off);
  if (!key) return;
  try {
    const out = await api("caller-keys/copy-key", { key: id });
    if (connectKeyID !== id || gatewayKeys?.find((k) => k.id === id) !== key) return;
    connectSecret = { id, secret: out.secret };
    if (view === "gateway") renderConnect();
  } catch (e) { status(t(e.message), "err"); }
}
async function loadGatewayKeys() {
  const read = ++gatewayKeysRead;
  try {
    const out = await api("caller-keys");
    if (read !== gatewayKeysRead) return;
    // A fresh list also refreshes a selected credential, which may have rotated.
    connectSecret = null;
    gatewayKeys = out.keys || [];
    renderGatewayKeys();
    if (view === "gateway" && providers) selectConnectKey(connectKeyID);
  } catch (e) {
    if (read !== gatewayKeysRead) return;
    $("#gatewayKeys").replaceChildren(el("div", "none", t(e.message)));
  }
}

async function gatewayKeyAction(action, body) {
  if (gatewayKeysBusy) return null;
  gatewayKeysBusy = true;
  gatewayKeysRead++;
  try {
    const out = await api("caller-keys/" + action, body);
    gatewayKeysRead++;
    connectSecret = null;
    gatewayKeys = out.keys || [];
    gatewayAccountNames = out.accountNames || gatewayAccountNames;
    gatewayKeyDraft = null;
    if (action === "limit-key" || !gatewayKeys.some((k) => k.id === gatewayLimit?.id)) gatewayLimit = null;
    renderGatewayKeys();
    if (view === "gateway" && providers) selectConnectKey(connectKeyID);
    return out;
  } catch (e) {
    status(t(e.message), "err");
    return null;
  } finally {
    gatewayKeysBusy = false;
  }
}

function gatewayRename(k) {
  const b = el("button", "n rename", k.name);
  b.title = t("Rename");
  b.onclick = () => {
    const i = input(k.name, t("Name"));
    i.className = "rename-in";
    i.setAttribute("aria-label", t("Name"));
    let finished = false;
    const done = async (save) => {
      if (finished) return;
      finished = true;
      // Keep the other controls in place when blur precedes their click.
      i.replaceWith(b);
      if (save && i.value.trim() !== k.name) await gatewayKeyAction("rename-key", { key: k.id, name: i.value });
    };
    i.onkeydown = (e) => { e.stopPropagation(); if (e.key === "Enter") done(true); else if (e.key === "Escape") done(false); };
    i.onblur = () => done(true);
    b.replaceWith(i);
    i.focus({ preventScroll: true });
  };
  return b;
}

function gatewayKeyForm() {
  const box = el("div", "acc adding");
  const name = input(gatewayKeyDraft, t("Key name, e.g. Laptop"));
  name.setAttribute("aria-label", t("Gateway key name"));
  name.oninput = () => { gatewayKeyDraft = name.value; };
  const own = input(gatewayOwnDraft, t("Your own key (optional)"), "password");
  own.setAttribute("aria-label", t("Gateway key value"));
  own.title = t("Leave empty and magpie makes a key. Or enter one your clients already send, from another gateway, so they keep working.");
  own.autocomplete = "off";
  own.oninput = () => { gatewayOwnDraft = own.value; };
  const add = el("button", "text primary", t("Create"));
  const go = async () => {
    add.disabled = true;
    const mine = own.value.trim() !== "";
    const out = await gatewayKeyAction("add-key", mine ? { name: name.value, secret: own.value } : { name: name.value });
    if (!out) { add.disabled = false; return; }
    gatewayOwnDraft = "";
    status(t(mine ? "Gateway key created. Clients that send this key reach magpie now." : "Gateway key created. Use Copy on its row to connect a client."), "ok");
  };
  add.onclick = go;
  const cancelled = () => { gatewayKeyDraft = null; gatewayOwnDraft = ""; renderGatewayKeys(); };
  for (const i of [name, own]) i.onkeydown = (e) => {
    e.stopPropagation();
    if (e.key === "Enter" && !add.disabled) go();
    else if (e.key === "Escape") cancelled();
  };
  const cancel = el("button", "text", t("Cancel"));
  cancel.onclick = cancelled;
  const fields = el("div", "kf");
  fields.append(name, own);
  const bar = el("div", "kb");
  bar.append(el("span", "grow"), cancel, add);
  box.append(fields, bar);
  queueMicrotask(() => name.focus({ preventScroll: true }));
  return box;
}

function copyCallerKeyBtn(k) {
  const cp = copyBtn("", t("Gateway key"));
  cp.setAttribute("aria-label", t("Copy gateway key"));
  cp.onclick = async () => {
    try {
      const out = await api("caller-keys/copy-key", { key: k.id });
      await copy(out.secret, t("Gateway key"), cp);
    } catch (e) { status(t(e.message), "err"); }
  };
  return cp;
}

function renderGatewayKeys() {
  const box = $("#gatewayKeys");
  $("#gatewayKeysBlock").hidden = !providers?.gateway.lan;
  box.replaceChildren();
  $("#addGatewayKey").onclick = () => { gatewayKeyDraft = ""; renderGatewayKeys(); };
  if (gatewayKeys === null) return;
  const list = el("div", "accts");
  for (const k of gatewayKeys) {
    const row = el("div", "acc" + (k.off ? " off" : " in-use"));
    row.dataset.key = k.id;
    const tick = el("button", "dot tick");
    tick.title = t(k.off ? "Enable key" : "Disable key");
    tick.setAttribute("aria-label", tick.title);
    if (!k.off) tick.append(svg(CHECK, 10, 2.2));
    tick.onclick = () => gatewayKeyAction(k.off ? "on-key" : "off-key", { key: k.id });
    const rm = el("button", "text quiet", t("Remove"));
    rm.onclick = () => askGatewayKey(k, false);
    const rotate = el("button", "text quiet", t("Rotate key"));
    rotate.onclick = () => askGatewayKey(k, true);
    row.append(tick, gatewayRename(k), el("span", "plan mono", k.masked), el("span", "grow"), gatewayModelsBadge(k), gatewayQuotasBadge(k), gatewayLimitBadge(k), copyCallerKeyBtn(k), rotate, rm);
    const under = gatewayLimit?.id === k.id ? [gatewayLimitEditor(k)] : keyUsed(k).map((u) => gatewayLimitLine(k, u));
    if (under.length) row.classList.add("with-limit"), row.append(...under);
    list.append(row);
  }
  if (gatewayKeyDraft !== null) list.append(gatewayKeyForm());
  else if (!gatewayKeys.length) list.append(el("div", "none", t("No gateway keys yet")));
  box.append(list);
}

function askGatewayKey(k, rotate) {
  const ed = el("div", "editor");
  const head = el("div", "ehead");
  head.append(el("b", "", t(rotate ? "Rotate gateway key?" : "Remove gateway key?")));
  ed.append(head, el("p", "lib-confirm", t(rotate
    ? "Clients using {name} will need the new key. Its name and usage history stay the same."
    : "Clients using {name} will lose access from other computers. Its usage history is kept.", { name: k.name })));
  const bar = el("div", "bar");
  const cancel = el("button", "text", t("Cancel"));
  cancel.onclick = (e) => { e.stopPropagation(); closeConfirmAsk(); };
  const go = el("button", "text primary", t(rotate ? "Rotate key" : "Remove"));
  go.onclick = async (e) => {
    e.stopPropagation();
    if (go.disabled) return;
    go.disabled = true;
    const out = await gatewayKeyAction(rotate ? "rotate-key" : "remove-key", { key: k.id });
    if (!out) { go.disabled = false; return; }
    closeConfirmAsk();
    if (rotate) status(t("Gateway key rotated. Update clients on other computers."), "ok");
  };
  bar.append(el("span", "grow"), cancel, go);
  ed.append(bar);
  confirmAsk = ed;
  openModal(ed);
  $("#modal").classList.add("lib");
  cancel.focus({ preventScroll: true });
}

// ---- the models a gateway key may use (#882), and the accounts (#905) ----

// accountShown is a stored account entry ("<provider>/<id>", #905) as the
// badge says it: by who is signed in, the id itself when no provider has it.
const accountShown = (a) => gatewayAccountNames[a] || a;

// the key whose models menu waits on its list: the badge drawn last opens
// it, so a redraw of the keys while it loads (a rename's reply, the
// providers' poll) doesn't drop the click
let keyModelsLoading = null;

// gatewayModelsBadge picks the models and the accounts a key may use, in
// the app's menu: "All models" on hover when it may use any, else which
// it may, always shown. A pick is sent when the menu closes.
function gatewayModelsBadge(k) {
  const ms = k.models || [];
  const as = k.accounts || [];
  const b = el("button", "amodels key-models" + (ms.length || as.length ? " set" : ""));
  b.type = "button";
  b.textContent = !ms.length && !as.length ? t("All models")
    : ms.length ? (ms.length === 1 && !as.length ? ms[0] : t("{n} models", { n: ms.length }))
      : (as.length === 1 ? accountShown(as[0]) : t("{n} accounts", { n: as.length }));
  b.title = (ms.length ? t("Only these models: {list}", { list: ms.join(", ") }) : t("Pick the models this key may use"))
    + (as.length ? "\n" + t("Only these accounts: {list}", { list: as.map(accountShown).join(", ") })
      + "\n" + t("A provider with no account picked is used on every account; to keep the key off it, pick only other providers' models") : "");
  b.setAttribute("aria-label", t("Models this key may use"));
  b.setAttribute("aria-haspopup", "menu");
  b.setAttribute("aria-expanded", "false");
  const open = (models, accounts) => {
    if (!b.isConnected) return;
    const opts = [{ v: "", name: "All models", note: "Any model and any account, now and later" }];
    const seen = new Set();
    // the accounts and keys the key may be held to (#905), each right
    // after its provider's models, not one block at the end: an account
    // by who is signed in, a key by its fingerprint
    const accountsSet = new Set();
    const ofProvider = new Map();
    // the list holds the key only to the accounts of the providers it
    // names (#905): a provider none of whose accounts is picked is used on
    // every one, and its unticked accounts say so (#2077: unticking all of
    // Codex's left Codex's models served)
    const named = new Set(as.map((v) => v.split("/")[0].toLowerCase()));
    for (const a of accounts || []) {
      accountsSet.add(a.id);
      const note = as.length && !named.has(String(a.provider).toLowerCase())
        ? t("Used: no {provider} account is picked", { provider: a.providerName })
        : a.plan ? a.providerName + " · " + a.plan : a.providerName;
      const o = { v: a.id, name: a.name, note, literalName: true };
      ofProvider.set(a.provider, [...(ofProvider.get(a.provider) || []), o]);
    }
    let listed = "";  // the provider whose models the list is in
    const endBlock = () => {
      for (const o of ofProvider.get(listed) || []) opts.push(o);
      ofProvider.delete(listed);
      listed = "";
    };
    for (const m of models || []) {
      // a routing group the key names is its with every member in it
      // (Magic_zero on Discord); the groups come first
      if (m.group) {
        endBlock();
        if (!seen.has("group")) {
          seen.add("group");
          opts.push({ v: "group/*", name: "Every routing group", note: "group/*" });
        }
        opts.push({ v: m.id, name: m.name, note: m.id, literalName: true });
        continue;
      }
      if (m.provider !== listed) {
        endBlock();
        listed = m.provider;
      }
      if (!seen.has(m.provider)) {
        seen.add(m.provider);
        opts.push({ v: m.provider + "/*", name: t("Every {provider} model", { provider: m.providerName }), note: m.provider + "/*", literalName: true });
      }
      opts.push({ v: m.id, name: m.name, note: m.id, literalName: true });
    }
    endBlock();
    // a provider with accounts and no model in the list (one kept
    // unlisted, or serving none now) keeps its accounts after the models
    for (const os of ofProvider.values()) opts.push(...os);
    // what the CLI kept that the list hasn't, a pattern, a model gone or
    // an account signed out: behind every live entry, a model gone
    // before an account signed out
    for (const v of ms) if (!opts.some((o) => o.v === v)) opts.push({ v, name: v, note: "Not served now", literalName: true });
    for (const v of as) if (!accountsSet.has(v)) { accountsSet.add(v); opts.push({ v, name: v, note: "Not signed in now", literalName: true }); }
    openProtoMenu(b, opts, [...ms, ...as], (picked) => {
      // one action at a time: gatewayKeyAction is busy while one is sent.
      // The accounts are sent only when they changed: a model picked says
      // nothing of the key's accounts.
      (async () => {
        const pickedModels = picked.filter((v) => !accountsSet.has(v));
        const pickedAccounts = picked.filter((v) => accountsSet.has(v));
        const out = await gatewayKeyAction("models-key", { key: k.id, models: pickedModels });
        if (out) status(t(pickedModels.length ? "{name} may use only the models picked" : "{name} may use every model", { name: k.name }), "ok");
        if (pickedAccounts.length !== as.length || pickedAccounts.some((v) => !as.includes(v))) {
          await gatewayKeyAction("accounts-key", { key: k.id, accounts: pickedAccounts });
        }
      })();
    }, "Models this key may use", "sess-menu", "right");
  };
  b.onclick = async (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (b.classList.contains("open")) return closeProtoMenu();
    const loading = keyModelsLoading = { id: k.id, open };
    let models = [], accounts = [];
    try { ({ models, accounts } = await api("caller-keys/models")); } catch (err) {
      if (keyModelsLoading === loading) keyModelsLoading = null;
      status(t(err.message), "err");
      return;
    }
    if (keyModelsLoading !== loading) return;
    keyModelsLoading = null;
    loading.open(models, accounts);
  };
  if (keyModelsLoading?.id === k.id) keyModelsLoading.open = open;
  return b;
}

// ---- the quotas a gateway key is told (#2216) ----

// gatewayQuotasBadge turns on or off a key's being told the quotas of what
// it may use: a key held to a budget, models or accounts is told none of
// this magpie's quotas (#1981) unless it is on, and then only the cards of
// the providers whose every model it may use, of the accounts it may use.
// "Quotas" on hover when off, "Quotas shown" always when on.
function gatewayQuotasBadge(k) {
  const b = el("button", "amodels key-quotas" + (k.quotas ? " set" : ""));
  b.type = "button";
  b.textContent = t(k.quotas ? "Quotas shown" : "Quotas");
  b.title = t(k.quotas
    ? "This key is told the quotas of the providers whose every model it may use, and of the accounts it may use, account names included. Click to stop."
    : "A key held to a budget, models or accounts is told no quotas. Click to tell it the quotas of the providers whose every model it may use, and of the accounts it may use, account names included.");
  b.setAttribute("aria-label", t("Show this key its quotas"));
  b.setAttribute("aria-pressed", k.quotas ? "true" : "false");
  b.onclick = async (e) => {
    e.preventDefault();
    const out = await gatewayKeyAction("quotas-key", { key: k.id, quotas: !k.quotas });
    if (out) status(t(k.quotas ? "{name} is told no quotas" : "{name} is told the quotas of what it may use", { name: k.name }), "ok");
  };
  return b;
}

// ---- a gateway key's own limit (#585) ----

// "days" is every N days from when the limit is set or reset (#1509),
// "hours" every N hours (#2102), a subscription's 5-hour window's pace
const limitPeriods = ["day", "week", "month", "days", "hours"];
const limitPer = { day: "per day", week: "per week", month: "per month", days: "Every N days", hours: "Every N hours" };
const cyclic = (period) => period === "days" || period === "hours";
// perWords is a limit's window as the badge says it: "per week", "every
// 10 days", "every 5 hours"
const perWords = (period, days, hours) => period === "days"
  ? (days === 1 ? t("every 1 day") : t("every {n} days", { n: days }))
  : period === "hours" ? (hours === 1 ? t("every 1 hour") : t("every {n} hours", { n: hours }))
    : t(limitPer[period] || period);
// limitDays reads a cycle's days: a whole number from 1 to 3650, else NaN
function limitDays(v) {
  const s = String(v ?? "").trim();
  return /^\d{1,4}$/.test(s) && +s >= 1 && +s <= 3650 ? +s : NaN;
}
// limitHours reads a cycle's hours: a whole number from 1 to 8760, else NaN
function limitHours(v) {
  const s = String(v ?? "").trim();
  return /^\d{1,4}$/.test(s) && +s >= 1 && +s <= 8760 ? +s : NaN;
}

// limitTokens reads a count as typed: 2000000, 2,000,000, 500k, 2M, 1.5m,
// 100万, 1亿; "" for none, NaN for what isn't one.
function limitTokens(v) {
  const s = String(v ?? "").trim().toLowerCase().replace(/[,\s_]/g, "");
  if (!s) return 0;
  const m = s.match(/^(\d+(?:\.\d+)?)(k|m|b|万|亿)?$/);
  if (!m) return NaN;
  const mult = { k: 1e3, m: 1e6, b: 1e9, "万": 1e4, "亿": 1e8 }[m[2]] || 1;
  return Math.round(parseFloat(m[1]) * mult);
}
function limitCost(v) {
  const s = String(v ?? "").trim().replace(/^[$＄]/, "");
  if (!s) return 0;
  return /^\d+(\.\d+)?$/.test(s) ? parseFloat(s) : NaN;
}
const usd = (n) => "$" + (n >= 100 ? n.toFixed(0) : n.toFixed(2));

// keyLimits are a key's limits, its first and the rest (#2227), and
// keyUsed what it has used of each, in the same order.
const keyLimits = (k) => k.limit ? [k.limit, ...(k.moreLimits || [])] : [];
const keyUsed = (k) => k.usedAll || (k.used ? [k.used] : []);
// limitDraft is the editor's draft of a key's nth limit, or of a new one
// at n past the last.
function limitDraft(k, n) {
  const l = keyLimits(k)[n] || {};
  const taken = keyLimits(k).map((x) => x.period);
  // a new one in a window the key has no limit in yet: a week beside 5 hours
  const period = l.period || (n ? ["week", "hours", "day", "month"].find((p) => !taken.includes(p)) || "days" : "day");
  return { id: k.id, n, period, days: l.days ? String(l.days) : "", hours: l.hours ? String(l.hours) : (n && period === "hours" ? "5" : ""), tokens: l.tokens ? String(l.tokens) : "", cost: l.cost ? String(l.cost) : "", cacheReads: !!l.cacheReads };
}
// bindingUsed is the limit holding a key most: of those spent, the one
// that resets last, as it is refused until then; else the first
function bindingUsed(k) {
  const all = keyUsed(k), spent = all.filter((u) => u.spent);
  return spent.length ? spent.reduce((a, b) => (new Date(b.reset) > new Date(a.reset) ? b : a)) : all[0];
}

// gatewayLimitBadge opens a key's limit editor: "Limit" on hover when it
// has none, else what it has used, "1.2M / 2M · day", always shown; with
// several limits, the one holding it most and how many more there are.
function gatewayLimitBadge(k) {
  const u = bindingUsed(k), more = keyUsed(k).length - 1;
  const b = el("button", "amodels limit" + (u ? " set" : "") + (u?.spent ? " spent" : "") + (gatewayLimit?.id === k.id ? " open" : ""));
  b.type = "button";
  if (!u) b.textContent = t("Limit");
  else {
    const parts = [];
    if (u.tokenLimit) parts.push(fmtN(u.tokens) + " / " + fmtN(u.tokenLimit));
    if (u.costLimit) parts.push(usd(u.cost) + " / " + usd(u.costLimit));
    b.textContent = (u.spent ? t("Spent") + " · " : "") + parts.join(" · ") + " · " + perWords(u.period, u.days, u.hours)
      + (more > 0 ? " · " + t(more === 1 ? "+1 limit" : "+{n} limits", { n: more }) : "");
  }
  b.title = t(u ? "Change this key's limit" : "Set a limit for this key");
  b.setAttribute("aria-label", b.title);
  b.setAttribute("aria-expanded", gatewayLimit?.id === k.id ? "true" : "false");
  b.onclick = (e) => {
    e.preventDefault();
    if (gatewayLimit?.id === k.id) gatewayLimit = null;
    else gatewayLimit = limitDraft(k, Math.max(0, keyUsed(k).indexOf(u)));
    renderGatewayKeys();
  };
  return b;
}

// gatewayLimitLine says what a limited key has used of a limit u, has
// left and when it resets, under its row; with several, each says its
// window first.
function gatewayLimitLine(k, u) {
  const line = el("div", "key-limit" + (u.spent ? " spent" : ""));
  const parts = [];
  if (keyUsed(k).length > 1) parts.push(perWords(u.period, u.days, u.hours));
  if (u.tokenLimit) parts.push(t("{used} of {limit} tokens · {left} left", { used: fmtN(u.tokens), limit: fmtN(u.tokenLimit), left: fmtN(u.tokensLeft) }));
  if (u.costLimit) parts.push(t("{used} of {limit} estimated · {left} left", { used: usd(u.cost), limit: usd(u.costLimit), left: usd(u.costLeft) }));
  const at = new Date(u.reset);
  parts.push(t(u.spent ? "refused until {when}" : "resets {when}", { when: resetClock(at) }));
  if (u.inFlight) parts.push(t(u.inFlight === 1 ? "1 in flight" : "{n} in flight", { n: u.inFlight }));
  line.textContent = parts.join(" · ");
  line.title = t("Counted from {start}; resets {when}", { start: new Date(u.start).toLocaleString(intlLang()), when: at.toLocaleString(intlLang()) })
    + (u.unpriced ? "\n" + t("{n} calls without a known price aren't in the cost", { n: u.unpriced }) : "");
  return line;
}

// gatewayLimitEditor stages a key's limit: nothing is sent until Save.
function gatewayLimitEditor(k) {
  const d = gatewayLimit;
  const box = el("div", "acct-models key-limit-ed");
  box.onclick = (e) => e.stopPropagation();
  const limits = keyLimits(k), n = d.n || 0, used = keyUsed(k)[n];
  const kept = limits[n] || {};
  const changed = () => d.period !== (kept.period || "day") || limitTokens(d.tokens) !== (kept.tokens || 0)
    || limitCost(d.cost) !== (kept.cost || 0) || d.cacheReads !== !!kept.cacheReads
    || (d.period === "days" && limitDays(d.days) !== kept.days)
    || (d.period === "hours" && limitHours(d.hours) !== kept.hours);
  const fields = el("div", "klf");
  // the app's own menu, not a native select
  const period = el("button", "sess-pick klf-period");
  period.type = "button";
  period.setAttribute("aria-label", t("Limit window"));
  period.setAttribute("aria-haspopup", "menu");
  period.setAttribute("aria-expanded", "false");
  const paintPeriod = () => {
    period.dataset.value = d.period;
    period.replaceChildren(el("span", "", t(limitPer[d.period])), svg(CHEV, 11, 1.6));
    fields.classList.toggle("with-days", cyclic(d.period));
    dl.hidden = d.period !== "days";
    hl.hidden = d.period !== "hours";
  };
  // the days of a cycle, shown for "every N days"
  const days = input(d.days, "10");
  days.setAttribute("aria-label", t("Days in a cycle"));
  days.inputMode = "numeric";
  const dl = el("label", "klf-field klf-days");
  dl.append(el("span", "klf-name", t("Days")), days);
  // the hours of a cycle, shown for "every N hours"
  const hours = input(d.hours, "5");
  hours.setAttribute("aria-label", t("Hours in a cycle"));
  hours.inputMode = "numeric";
  const hl = el("label", "klf-field klf-days klf-hours");
  hl.append(el("span", "klf-name", t("Hours")), hours);
  paintPeriod();
  const tokens = input(d.tokens, t("No token cap"));
  tokens.setAttribute("aria-label", t("Token limit"));
  tokens.inputMode = "decimal";
  const cost = input(d.cost, t("No cost cap"));
  cost.setAttribute("aria-label", t("Estimated cost limit, US$"));
  cost.inputMode = "decimal";
  const tl = el("label", "klf-field");
  tl.append(el("span", "klf-name", t("Tokens")), tokens);
  const cl = el("label", "klf-field");
  cl.append(el("span", "klf-name", t("Estimated cost, US$")), cost);
  const pl = el("label", "klf-field");
  pl.append(el("span", "klf-name", t("Window")), period);
  fields.append(pl, dl, hl, tl, cl);
  // a key's limits (#2227), each a tab, when it has more than one or one
  // is being added: the editor is of one at a time
  let which = null;
  if (limits.length > 1 || n >= limits.length && limits.length) {
    which = el("div", "klf-which");
    which.setAttribute("role", "tablist");
    which.setAttribute("aria-label", t("This key's limits"));
    const tabs = limits.map((l, i) => ({ i, name: perWords(l.period, l.days, l.hours) }));
    if (n >= limits.length) tabs.push({ i: n, name: t("New limit") });
    for (const x of tabs) {
      const tab = el("button", "klf-tab" + (x.i === n ? " on" : ""), x.name);
      tab.type = "button";
      tab.setAttribute("role", "tab");
      tab.setAttribute("aria-selected", String(x.i === n));
      tab.onclick = (e) => {
        e.stopPropagation();
        if (x.i === n) return;
        gatewayLimit = limitDraft(k, x.i);
        renderGatewayKeys();
      };
      which.append(tab);
    }
  }
  const cache = el("label", "klf-check");
  const cb = el("input");
  cb.type = "checkbox";
  cb.checked = d.cacheReads;
  cache.append(cb, el("span", "", t("Count cache reads too")));
  const hint = el("div", "hint", t("Tokens are each call's uncached input, output and cache writes. The cost is an estimate at the Usage page's prices, not a bill. Windows are calendar ones in this computer's time (a week from Monday), or every N days or N hours from when the limit is set or reset. Requests from this computer without a key aren't limited."));
  const err = el("div", "hint klf-err");
  const unsaved = el("span", "hint munsaved", t("unsaved"));
  unsaved.title = t("Made when you Save; Cancel drops it");
  const save = el("button", "text primary", t("Save"));
  const update = () => {
    const bad = d.period === "days" && Number.isNaN(limitDays(d.days)) ? t("Days is a whole number from 1 to 3650")
      : d.period === "hours" && Number.isNaN(limitHours(d.hours)) ? t("Hours is a whole number from 1 to 8760")
      : Number.isNaN(limitTokens(d.tokens)) ? t("Tokens is a count, like 2000000 or 2M")
        : Number.isNaN(limitCost(d.cost)) ? t("The cost is US dollars, like 5 or 2.50") : "";
    err.textContent = bad;
    err.hidden = !bad;
    unsaved.hidden = !changed();
    // a limit being added has a cap to save
    save.disabled = !!bad || !changed() || (n >= limits.length && !limitTokens(d.tokens) && !limitCost(d.cost));
  };
  period.onclick = (e) => {
    e.preventDefault();
    e.stopPropagation();
    if (period.classList.contains("open")) return closeProtoMenu();
    openProtoMenu(period, limitPeriods.map((p) => ({ v: p, name: t(limitPer[p]), note: "", literalName: true })), d.period, (v) => {
      d.period = v;
      paintPeriod();
      update();
      if (v === "days") days.focus({ preventScroll: true });
      if (v === "hours") hours.focus({ preventScroll: true });
    }, "Limit window", "sess-menu");
  };
  days.oninput = () => { d.days = days.value; update(); };
  hours.oninput = () => { d.hours = hours.value; update(); };
  tokens.oninput = () => { d.tokens = tokens.value; update(); };
  cost.oninput = () => { d.cost = cost.value; update(); };
  cb.onchange = () => { d.cacheReads = cb.checked; update(); };
  const close = () => { gatewayLimit = null; renderGatewayKeys(); };
  for (const i of [days, hours, tokens, cost]) i.onkeydown = (e) => {
    e.stopPropagation();
    if (e.key === "Escape") close();
    else if (e.key === "Enter" && !save.disabled) save.click();
  };
  const cancel = el("button", "text", t("Cancel"));
  cancel.onclick = close;
  save.onclick = async () => {
    if (save.disabled) return;
    save.disabled = true;
    const tk = limitTokens(d.tokens), c = limitCost(d.cost);
    const limit = tk || c ? { period: d.period, tokens: tk, cost: c, cacheReads: d.cacheReads } : null;
    if (limit && d.period === "days") limit.days = limitDays(d.days);
    if (limit && d.period === "hours") limit.hours = limitHours(d.hours);
    // which one only when there are several: one is the key's only limit
    const out = await gatewayKeyAction("limit-key", which ? { key: k.id, limit, index: n } : { key: k.id, limit });
    if (!out) { update(); return; }
    status(t(limit ? "Limit saved for {name}" : "Limit removed for {name}", { name: k.name }), "ok");
  };
  const foot = el("div", "acm-foot");
  foot.append(cache, el("span", "grow"), unsaved);
  if (limits.length && n < limits.length && limits.length < 5) {
    // one more limit in a window of its own (#2227): 5 hours and a week,
    // as a subscription's; a request passes only while all have some left
    const more = el("button", "text", t("Add a limit"));
    more.title = t("Another limit in a window of its own; a request passes only while every limit has something left");
    more.onclick = (e) => { e.stopPropagation(); gatewayLimit = limitDraft(k, limits.length); renderGatewayKeys(); };
    foot.append(more);
  }
  if (n < limits.length) {
    const off = el("button", "text", t(limits.length > 1 ? "Remove this limit" : "No limit"));
    off.title = t("Clears the caps; Save makes it");
    off.onclick = () => { d.tokens = ""; d.cost = ""; tokens.value = ""; cost.value = ""; update(); };
    foot.append(off);
  }
  foot.append(cancel, save);
  if (which) box.append(which);
  box.append(fields, hint, err);
  if (used) {
    // count from 0 again, tokens and cost together (#1509)
    const zero = el("button", "text klf-reset", t("Reset usage"));
    zero.title = t("Count this key's use from 0 again, tokens and cost together");
    zero.onclick = (e) => { e.stopPropagation(); askResetLimit(k, n); };
    const line = el("div", "klf-used");
    line.append(gatewayLimitLine(k, used), zero);
    box.append(line);
  }
  box.append(foot);
  update();
  return box;
}

// askResetLimit confirms a key's Reset (#1509): what it has used goes back
// to 0, tokens and cost together; an N-day cycle starts again from now, a
// calendar window keeps its end.
function askResetLimit(k, n) {
  const u = keyUsed(k)[n] || {};
  const ed = el("div", "editor");
  const head = el("div", "ehead");
  head.append(el("b", "", t("Reset {name}'s usage?", { name: k.name })));
  ed.append(head, el("p", "lib-confirm", u.period === "days"
    ? t("The tokens and cost {name} has used go back to 0 together, and a new {n}-day cycle starts now.", { name: k.name, n: u.days })
    : u.period === "hours" ? t("The tokens and cost {name} has used go back to 0 together, and a new {n}-hour cycle starts now.", { name: k.name, n: u.hours })
    : t("The tokens and cost {name} has used go back to 0 together. Calls so far in this window stop counting; it still resets {when}.", { name: k.name, when: resetClock(new Date(u.reset)) })));
  const bar = el("div", "bar");
  const cancel = el("button", "text", t("Cancel"));
  cancel.onclick = (e) => { e.stopPropagation(); closeConfirmAsk(); };
  const go = el("button", "text primary", t("Reset usage"));
  go.onclick = async (e) => {
    e.stopPropagation();
    if (go.disabled) return;
    go.disabled = true;
    const out = await gatewayKeyAction("reset-limit-key", keyUsed(k).length > 1 ? { key: k.id, index: n } : { key: k.id });
    if (!out) { go.disabled = false; return; }
    closeConfirmAsk();
    status(t("Usage reset for {name}", { name: k.name }), "ok");
  };
  bar.append(el("span", "grow"), cancel, go);
  ed.append(bar);
  confirmAsk = ed;
  openModal(ed);
  $("#modal").classList.add("lib");
  cancel.focus({ preventScroll: true });
}
