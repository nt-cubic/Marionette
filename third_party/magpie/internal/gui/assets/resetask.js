// A Codex reset's confirmation (askReset in app.js): spending a reset
// can't be taken back, so it asks first, names the account and which of
// its resets is used, and lets the user pick another (01huadalang on
// Discord). app.js imports this when a reset's Use is pressed, so it
// weighs nothing on the core.
window.askResetSheet = function (q) {
  const ed = el("div", "editor reset-ask");
  const head = el("div", "ehead");
  head.append(icon(q.icon || "codex"), el("b", "", t("Use a Codex reset?")));
  ed.append(head);
  const who = q.alias || q.user || q.name;
  ed.append(el("p", "lib-confirm", t(q.resets.count === 1
    ? "{who} has 1 reset. Using it starts its windows again at once, as if none of them had been used. It can't be undone."
    : "{who} has {n} resets. Using one starts its windows again at once, as if none of them had been used. It can't be undone.", { who, n: q.resets.count })));
  // which of them is used: the one that runs out first unless the user
  // picks another of those listed one by one (01huadalang on Discord),
  // and the dialog names the one it spends
  const cards = q.resets.each?.length > 1 && q.resets.each.every((c) => c.id) ? q.resets.each : null;
  let chosen = 0;
  const which = el("p", "lib-confirm reset-which");
  const sooner = el("p", "lib-confirm reset-sooner");
  const cardName = (i, at = resetClock) => {
    const c = cards[i];
    return c.until ? t("Reset {n} runs out {when}", { n: i + 1, when: at(new Date(c.until)) }) : t("Reset {n} never runs out", { n: i + 1 });
  };
  const sayWhich = () => {
    const full = (d) => d.toLocaleString(intlLang());
    if (!chosen) {
      which.textContent = q.resets.until ? t("The one used is the one that runs out first, {when}.", { when: full(new Date(q.resets.until)) }) : "";
      which.hidden = !q.resets.until;
      sooner.hidden = true;
      return;
    }
    const c = cards[chosen];
    which.textContent = c.until ? t("The one used is {who}'s reset that runs out {when}.", { who, when: full(new Date(c.until)) })
      : t("The one used is {who}'s reset that never runs out.", { who });
    which.hidden = false;
    // one that runs out sooner is left, and lost if it isn't used in time
    sooner.textContent = cards[0].until ? t("The one that runs out first, {when}, is kept, and is lost if it isn't used by then.", { when: full(new Date(cards[0].until)) }) : "";
    sooner.hidden = !cards[0].until || cards[0].until === c.until;
  };
  if (cards) {
    const row = el("div", "reset-pick");
    const pick = el("button", "sess-pick reset-card");
    pick.type = "button";
    pick.setAttribute("aria-haspopup", "menu");
    pick.setAttribute("aria-expanded", "false");
    pick.append(el("span", "", cardName(0)), svg(CHEV, 11, 1.6));
    pick.title = cardName(0, (d) => d.toLocaleString(intlLang()));
    pick.onclick = (e) => {
      e.stopPropagation();
      if (pick.classList.contains("open")) return closeProtoMenu();
      openProtoMenu(pick, cards.map((c, i) => ({ v: String(i), name: cardName(i), note: i ? "" : t("Used unless you pick another"), literalName: true })), String(chosen), (v) => {
        chosen = Number(v);
        pick.firstChild.textContent = cardName(chosen);
        pick.title = cardName(chosen, (d) => d.toLocaleString(intlLang()));
        pick.classList.toggle("set", chosen > 0);
        sayWhich();
      }, "Reset to use", "sess-menu");
    };
    row.append(el("span", "reset-pick-label", t("Reset to use")), pick);
    ed.append(row);
  }
  sayWhich();
  ed.append(which, sooner);
  // nothing used yet: a reset would start nothing again
  if (!q.windows?.some((w) => w.used > 0)) ed.append(el("p", "lib-confirm reset-idle", t("None of its windows has been used yet, so there is nothing to start again.")));
  const bar = el("div", "bar");
  const go = el("button", "text primary", t("Use a reset"));
  go.onclick = async (e) => {
    e.stopPropagation();
    go.disabled = true;
    go.classList.add("busy");
    try {
      const out = await api("usage/codex-reset", chosen ? { user: q.user || "", credit: cards[chosen].id } : { user: q.user || "" });
      closeConfirmAsk();
      status(who + ": " + resetOutcome(out), out.code === "reset" ? "ok" : "err");
      // the providers' editor reads the account's windows and resets
      // on its own, kept a minute: read again, as Usage is (#1491)
      delete loginUsage.codex;
      if (editing && view === "providers") renderProviders();
      loadQuotas();
    } catch (err) {
      go.disabled = false;
      go.classList.remove("busy");
      statusErr(err);
    }
  };
  const cancel = el("button", "text", t("Cancel"));
  cancel.onclick = cancelAsk;
  bar.append(el("span", "grow"), cancel, go);
  ed.append(bar);
  confirmAsk = ed;
  openModal(ed);
  $("#modal").classList.add("lib");
  go.focus();
};
