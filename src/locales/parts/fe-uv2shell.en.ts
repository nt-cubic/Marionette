import type { zh } from "./fe-uv2shell.zh";

/** English mirror of the fe-uv2shell fragment. */
export const en = {
  "usageV2.title": "Token usage",
  "usageV2.source.measured": "Measured",
  "usageV2.source.estimated": "Estimated",
  "usageV2.source.derived": "Derived",
  "usageV2.source.priced": "List price",
  "usageV2.source.clock": "Local clock",
  "usageV2.range.label": "Range",
  "usageV2.range.1": "Today",
  "usageV2.range.7": "7 days",
  "usageV2.range.30": "30 days",
  "usageV2.range.90": "90 days",
  "usageV2.range.180": "180 days",
  "usageV2.filter.label": "Filter by agent",
  "usageV2.filter.all": "All",
  "usageV2.export": "Export CSV",
  "usageV2.empty": "No usage recorded yet. Send a message and let the turn finish; numbers start here.",
  "usageV2.noDetail": "This range comes from the old ledger and has token figures only; time split, tools and commands are recorded from 0.7.1 on.",
} satisfies Record<keyof typeof zh, string>;
