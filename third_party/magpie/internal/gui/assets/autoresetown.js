// Settings › Usage's Codex auto-use rows: each ChatGPT account's switches
// and, under them, how early a reset about to expire is used. app.js
// imports this when Settings draws them (renderAutoResetOwn), so it weighs
// nothing on the core.

// renderAutoResetOwn lists each ChatGPT account under Settings › Usage's
// "Codex: auto-use resets", with its switch, the same say as on the
// account's card (settings' codexAutoReset, the CLI's quota auto-reset).
// Avalon on Discord, after foobra12 asked where it was: it was only on the
// accounts' cards, and they found it from the menu bar's panel alone,
// nothing for it in the window's Settings. The row shows while a Codex
// account is signed in, or one is still on.
window.autoResetOwnPaint = function (s) {
  const head = $("#codexAutoResetRow");
  const list = head.parentElement;
  list.querySelectorAll(".ar-own").forEach((r) => r.remove());
  const on = s.codexAutoReset || [];
  const users = [...(s.codexUsers || [])];
  for (const u of on) if (!users.some((x) => x.toLowerCase() === u)) users.push(u);
  head.hidden = !users.length;
  let after = head;
  for (const user of users) {
    const r = el("div", "row pref ar-own");
    r.dataset.user = user;
    const who = el("div", "who");
    const name = el("div", "name", user);
    name.title = user;
    who.append(name);
    // each of the two times its own Off/On (#2078)
    const was = on.includes(user.toLowerCase()) ? autoResetWhenOf(s.codexAutoResetOnly?.[user.toLowerCase()]) : { week: false, expiring: false };
    const cases = el("div", "ar-own-cases");
    for (const c of AUTO_RESET_CASES) {
      const line = el("div", "ar-own-case");
      line.dataset.case = c.id;
      const say = el("span", "ar-own-say", t(c.say));
      say.title = t(was[c.id] ? c.on : c.off, resetLeadVars());
      const pick = segs([["off", t("Off")], ["on", t("On")]], was[c.id] ? "on" : "off", async (v) => {
        try {
          prefs = await writingPrefs(api("settings/codex-auto-reset", { user, on: v === "on", when: c.id }));
          if (state) state.settings = prefs;
          const now = (prefs.codexAutoReset || []).includes(user.toLowerCase()) ? autoResetWhenOf(prefs.codexAutoResetOnly?.[user.toLowerCase()]) : { week: false, expiring: false };
          status(autoResetStatus(user, now), "ok");
          renderSettings();
          if (quotas) renderQuotas(); // the account's card says the same
        } catch (e) {
          statusTErr(e);
          renderSettings();
        }
      });
      pick.setAttribute("aria-label", t("Auto-use resets") + " · " + t(c.say) + " · " + user);
      line.append(say, pick);
      cases.append(line);
    }
    r.append(who, cases);
    after.after(r);
    after = r;
  }
  renderResetLead(s, head.hidden);
};

// renderResetLead: how long before a reset about to run out unused it is
// used, under the accounts' switches, one for them all; a pick from the
// app's menu (settings/codex-reset-lead). A value set outside the choices
// (the CLI takes any minutes in range) is shown and kept.
function renderResetLead(s, hidden) {
  const row = $("#codexResetLeadRow");
  if (!row) return;
  row.hidden = hidden;
  const cur = resetLeadMinutes(s);
  const leads = RESET_LEADS.includes(cur) ? RESET_LEADS : [...RESET_LEADS, cur].sort((a, b) => a - b);
  const name = (m) => t("{lead} before", { lead: resetLeadWords(m) });
  const b = el("button", "sess-pick reset-lead");
  b.type = "button";
  b.setAttribute("aria-haspopup", "menu");
  b.setAttribute("aria-label", t("Codex: how early before one expires") + ": " + name(cur));
  // every choice in one cell, the current one seen: the button is as wide
  // as the widest, so a pick doesn't rewrap the row and move the page
  const say = el("span", "rl-say");
  for (const m of leads) {
    const w = el("span", m === cur ? "on" : "", name(m));
    if (m !== cur) w.setAttribute("aria-hidden", "true");
    say.append(w);
  }
  b.append(say, svg(CHEV, 11, 1.6));
  b.onclick = (e) => {
    e.stopPropagation();
    if (b.classList.contains("open")) return closeProtoMenu();
    openProtoMenu(b, leads.map((m) => ({ v: String(m), name: name(m), note: m === 30 ? "Default" : "", literalName: true })), String(cur), async (v) => {
      if (Number(v) === cur) return;
      try {
        prefs = await writingPrefs(api("settings/codex-reset-lead", { minutes: Number(v) }));
        if (state) state.settings = prefs;
        renderSettings();
        if (quotas) renderQuotas(); // the cards' titles say it too
      } catch (err) {
        statusTErr(err);
        renderSettings();
      }
    }, "Used before it expires", "sess-menu", "right");
  };
  $("#codexResetLeadPick").replaceChildren(b);
}
