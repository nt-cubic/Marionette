// Settings › Menu bar windows. app.js imports this only when the menu bar
// shows a card (renderTrayUsage), so it weighs nothing on the core.

// which of a subscription's windows its menu bar card shows, by kind
// (settings.TrayWindows, StrikingRaw on X): a pill each for those ticked
// with two or more, every window unless some are picked; not on Windows,
// whose tooltip says them all
window.trayWindowsRow = function trayWindowsRow(s, keep, ids, mac) {
  const pick = s.trayWindows || {}, box = $("#trayWindowsPick"), subs = [];
  for (const pid of new Set(ids.filter((id) => !id.startsWith("usage:")).map((id) => id.split("|")[0]))) {
    const ws = [], q = (quotas || []).find((q) => q.provider === pid);
    for (const w of q?.windows || []) if (w.trayKind && !ws.some((x) => x.v === w.trayKind)) ws.push({ v: w.trayKind, name: w.name, note: "" });
    if (ws.length > 1) subs.push([pid, q.name, ws]);
  }
  $("#trayWindowsRow").hidden = !subs.length || !mac && document.documentElement.classList.contains("win");
  if (!quotas && ids.some((id) => !id.startsWith("usage:"))) loadQuotas().then(() => box.isConnected && trayWindowsRow(s, keep, ids, mac));
  box.replaceChildren(...subs.map(([pid, name, ws]) => {
    const on = (pick[pid] || []).filter((k) => ws.some((w) => w.v === k)), b = el("button", "proto pick" + (on.length ? " set" : ""));
    b.type = "button";
    b.append(el("span", "", name + " · " + (on.length ? on.map((k) => t(ws.find((w) => w.v === k).name)).join(" · ") : t("All windows"))), svg(CHEV, 11, 1.6));
    b.onclick = (e) => {
      e.stopPropagation();
      if (b.classList.contains("open")) return closeProtoMenu();
      openProtoMenu(b, [{ v: "", name: "All windows", note: "" }, ...ws], on, (v) => {
        const next = { ...pick };
        if (v.length && v.length < ws.length) next[pid] = v; else delete next[pid];
        savePrefs({ ...keep, trayWindows: next });
      }, "Shown in the menu bar");
    };
    return b;
  }));
};
