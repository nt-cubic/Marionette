import type { zh } from "./fe-uv2c.zh";

/** English mirror of the fe-uv2c fragment. */
export const en = {
  "usageV2.agents.title": "By agent · model",
  "usageV2.agents.meta": "Click an agent to expand the models it used",
  "usageV2.agents.empty": "No agent usage in this period",
  "usageV2.agents.col.agent": "agent · model",
  "usageV2.agents.col.tokens": "tokens",
  "usageV2.agents.col.trend": "Trend",
  "usageV2.agents.col.hit": "Cache hit",
  "usageV2.agents.col.cost": "List-price cost",
  "usageV2.agents.col.turns": "Turns",
  "usageV2.agents.col.ttft": "First token p50",
  "usageV2.agents.col.tps": "Output tok/s",
  "usageV2.agents.noRate": "No list price for this model in the catalog",
  "usageV2.agents.trend": "Daily token trend for {agent}",
} satisfies Record<keyof typeof zh, string>;
