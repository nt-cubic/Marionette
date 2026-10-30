import type { zh } from "./common.zh";

/** English mirror of the shared fragment — must cover every zh key (tsc). */
export const en = {
  "common.languageToggle": "Language: {current} (click to switch to {next})",
  "common.chat": "Chat",
  "common.cancel": "Cancel",
  "common.confirm": "OK",
  "common.close": "Close",
  "common.delete": "Delete",
  "common.remove": "Remove",
  "common.retry": "Retry",
  "common.copy": "Copy",
  "common.save": "Save",
  "common.open": "Open",
  "common.search": "Search",
  "common.today": "Today",
  "common.yesterday": "Yesterday",
  "common.thisWeek": "This week",
  "common.earlier": "Earlier",
  "common.on": "On",
  "common.off": "Off",
  "common.minutesAgo": "{n} min ago",
  "common.hoursAgo": "{n} hr ago",
  "common.daysAgo": "{n} d ago",
  "common.loading": "Loading…",
  "common.listSep": ", ",
} satisfies Record<keyof typeof zh, string>;
