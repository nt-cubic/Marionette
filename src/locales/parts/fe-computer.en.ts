/** Computer Use share panel + driver installer (fe-computer batch). */
export const en = {
  "computer.title": "Computer Use · Share windows",
  "computer.hint":
    "Once shared, the agent can see (read) or operate (click/type) the window. Agent-initiated requests surface as permission cards in the conversation; this panel is your manual entry.",
  "computer.installIntro": "First use downloads the action engine (~35MB, from the open-source trycua/cua project; SHA-256 verified, installed under ~/.marionette/cua/). Never downloaded unless you use this feature.",
  "computer.installButton": "Download and install engine",
  "computer.progressMb": "Downloading {done} / {total} MB",
  "computer.progressIndeterminate": "Downloading…",
  "computer.installFailed": "Install failed — check the network and retry (details in dev.log under [computer])",
  "computer.noWindows": "No shareable windows (Marionette itself, password managers and friends are never shareable)",
  "computer.untitledWindow": "(untitled window)",
  "computer.minimized": "minimized",
  "computer.shared.read": "shared: read",
  "computer.shared.control": "shared: control",
  "computer.shareRead": "Read",
  "computer.shareControl": "Act",
  "computer.shareReadHint": "agent can screenshot / read the UI tree, not click or type",
  "computer.shareControlHint": "agent can click / type (key whitelist applies; paste stays restricted)",
  "computer.refresh": "Refresh",
  "computer.stopAll": "Stop all",
  "computer.stopShortcutHint": "Global stop Ctrl+Alt+Shift+S revokes every share at any time",
  "palette.computerShare": "Share a window with agents (Computer Use)",
};
