import type { zh } from "./fe-usagedash.zh";

/** English mirror of the fe-usagedash fragment. */
export const en = {
  "usageLedger.title": "Token totals",
  "usageLedger.last7": "Last 7 days",
  "usageLedger.last30": "Last 30 days",
  "usageLedger.turns": "{n} turns",
  "usageLedger.turns_one": "{n} turn",
  "usageLedger.turns_other": "{n} turns",
  "usageLedger.byAgent": "By agent · last 7 days",
  "usageLedger.hint": "↓output · ↑input (incl. cache and reasoning); local stats only, nothing is sent.",
} satisfies Record<keyof typeof zh, string>;
