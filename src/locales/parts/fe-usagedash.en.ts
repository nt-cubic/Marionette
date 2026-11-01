import type { zh } from "./fe-usagedash.zh";

/** English mirror of the fe-usagedash fragment. */
export const en = {
  "usageLedger.title": "Token totals",
  "usageLedger.last7": "Last 7 days",
  "usageLedger.last30": "Last 30 days",
  "usageLedger.turns": "{count} turns",
  "usageLedger.turns_one": "{count} turn",
  "usageLedger.turns_other": "{count} turns",
  "usageLedger.byAgent": "By agent · last 7 days",
  "usageLedger.hint": "↓output · ↑input (incl. cache and reasoning); local stats only, nothing is sent.",
  "usageLedger.viewAnalysis": "Analyze",
  "usageLedger.sparkTotal": "7d {tokens}",
  "usageLedger.analysisTitle": "Token usage analysis",
  "usageLedger.bucketInput": "Input",
  "usageLedger.bucketOutput": "Output",
  "usageLedger.bucketCacheRead": "Cache read",
  "usageLedger.bucketCacheWrite": "Cache write",
  "usageLedger.cacheHit": "Cache hit {pct}",
  "usageLedger.dailyTitle": "Daily usage",
  "usageLedger.modelsTitle": "By model",
  "usageLedger.other": "Other",
  "usageLedger.coverage": "{pct} priced",
  "usageLedger.priceUpdated": "Rates updated {when}",
  "usageLedger.costNote":
    "Estimated at public API list prices — not what your plan paid; tokens without a listed rate are excluded.",
  "usageLedger.emptyTitle": "No usage recorded yet",
  "usageLedger.emptyNote":
    "Finish your first turn and today / last 7 / last 30 totals will appear here.",
  "usageLedger.paletteOpen": "Token usage analysis",
} satisfies Record<keyof typeof zh, string>;
