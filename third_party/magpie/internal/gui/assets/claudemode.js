// The Claude Code provider editor's "Claude Code runs as" row: whether a
// Claude subscription's runs are Claude Code's interactive mode (TUI) or
// claude -p, why -p, how the last request went, and on hover what each is
// and what it is good for (the owner, 10-31). app.js imports this with the
// row's first read of /api/claude/mode (claudeModeRow), so it weighs
// nothing on the core.

// why a run is -p, by the gateway's code (claudeMachineWhy, claudePrint)
const CC_WHY = {
  off: "the interactive mode is off in Settings",
  broken: "the interactive mode failed to start lately, and is tried again later",
  os: "this system can't run the interactive mode",
  "wsl-home": "magpie couldn't find the home folder of Claude Code in WSL",
  missing: "Claude Code isn't installed on this computer",
  start: "the interactive mode didn't start",
  safeguards: "the request carried safeguards",
  cache: "the client marked its own cache points",
  schema: "the request asked for a JSON schema",
  image: "the request had an image by its URL",
  big: "its first message was over 400KB",
  branch: "it branched off another request's conversation, which a sub-agent's first request does",
  wslread: "Windows can't read the transcripts of Claude Code in WSL",
};
const ccWhy = (code) => t(CC_WHY[code] || CC_WHY.os);

// ccTip: the two modes side by side, the one in use marked
let ccTipEl = null;
function ccTip(anchor, tui, only) {
  ccTipOff();
  const tip = ccTipEl = el("div", "cc-tip");
  tip.setAttribute("role", "tooltip");
  const mode = (on, name, what, good, bad) => {
    const m = el("div", "cc-m" + (on ? " on" : ""));
    const h = el("div", "h");
    h.append(el("b", "", name));
    if (on) h.append(el("span", "now", t("In use")));
    m.append(h, el("p", "", what), el("p", "good", good));
    if (bad) m.append(el("p", "bad", bad));
    return m;
  };
  tip.append(
    mode(tui, t("Interactive mode (TUI)"),
      t("magpie opens Claude Code as a person does, in a terminal of its own that you don't see, types each request in and reads the reply from its transcript."),
      t("Your plan counts these requests as Claude Code's own use, at about half of the 5-hour allowance that -p uses for the same work.")),
    mode(!tui, "claude -p",
      t("Claude Code's print mode, the one scripts and the Agent SDK use: a request in, its reply out."),
      t("It starts at once, and takes what the interactive mode can't: a JSON schema, an image by its URL, safeguards, a client's own cache points, a first message over 400KB. Such a request runs as -p even with the interactive mode on."),
      t("The plan counts a -p request at about twice the interactive mode's.")),
    el("div", "foot", t(only
      ? "Interactive only is set in Settings › Models › Replies: nothing runs as -p, and a request the interactive mode can't take, or one it fails to start for, fails saying why."
      : "Turned on or off in Settings › Models › Replies. When the interactive mode fails to start, requests run as -p for a while and it is tried again later.")));
  document.body.append(tip);
  const a = anchor.getBoundingClientRect(), b = tip.getBoundingClientRect();
  tip.style.left = Math.max(8, Math.min(a.left, innerWidth - b.width - 8)) + "px";
  tip.style.top = (a.bottom + 8 + b.height < innerHeight - 8 ? a.bottom + 8 : Math.max(8, a.top - b.height - 8)) + "px";
}
function ccTipOff() {
  ccTipEl?.remove();
  ccTipEl = null;
}

// claudeModeDraw fills box, the row app.js put in the editor, from m, a
// read of /api/claude/mode
window.claudeModeDraw = function (box, m) {
  if (!m) return;
  const v = el("div", "cc-run");
  const pill = el("span", "cc-pill" + (m.tui ? " tui" : ""));
  pill.tabIndex = 0;
  // Interactive only where it can't run: nothing runs, as -p or not
  pill.append(el("i"), el("span", "", m.tui ? t("Interactive mode (TUI)") : m.only ? t("Interactive only") : "claude -p"));
  pill.onmouseenter = pill.onfocus = () => ccTip(pill, m.tui, m.only);
  pill.onmouseleave = pill.onblur = ccTipOff;
  v.append(pill);
  const last = m.last;
  let said = "";
  if (!m.tui && m.only) said = t("Claude requests fail: {why}", { why: ccWhy(m.why) });
  else if (!m.tui) said = ccWhy(m.why);
  else if (last && last.tui) said = t("Last request {ago}: interactive mode", { ago: ago(last.at) });
  else if (last) said = t("Last request {ago} ran as -p: {why}", { ago: ago(last.at), why: ccWhy(last.why) });
  else if (m.only) said = t("Interactive only: a request it can't take fails instead of running as -p");
  if (said) v.append(el("span", "cc-said", said));
  if (m.why === "off" || (!m.tui && m.only)) {
    const b = el("button", "text action", t("Change in Settings"));
    b.type = "button";
    b.hidden = gatewayMode; // Settings has no such row then
    b.onclick = (e) => { ccTipOff(); openSettingsAt(e, "models", "#claudeTuiRow"); };
    v.append(b);
  }
  box.replaceChildren(...field(t("Claude Code runs as"), v, ""));
};
// the editor going away takes its tip with it
addEventListener("pointerdown", ccTipOff, true);
