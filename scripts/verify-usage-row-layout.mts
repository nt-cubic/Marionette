/**
 * Layout check: the quota rail no longer renders Last turn / Session total, and
 * session context/cost remain facts on the snapshot instead of quota rows.
 *
 * Run: npx tsx scripts/verify-usage-row-layout.mts
 */
import assert from "node:assert/strict";
import * as React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { ContextPanel } from "../src/components/ContextPanel";
import { applyLocale } from "../src/lib/i18n";
import { buildUsageSnapshot, emptySessionUsage } from "../src/lib/usage";

// This layout fixture uses the English labels it asserts below; verify-all pins
// the default verification locale to zh-CN for the other scripts.
await applyLocale("en");

const state: any = {
  ...emptySessionUsage(),
  contextUsed: 53000,
  contextSize: 200000,
  costAmount: 0.045,
  costCurrency: "USD",
  lastTurnStats: {
    input: 139000,
    output: 2200,
    cached: 139000,
    reasoning: null,
    total: 141200,
    ttftMs: 1200,
    durationMs: 8000,
    outputTps: 13.8,
    ppTps: 11583.3,
  },
};
const snap = buildUsageSnapshot({
  agentId: "grok-build",
  agentLabel: "Grok Build",
  state,
  connected: true,
  cumulative: { input: 25458, output: 58, cached: 5632, reasoning: 0, turns: 2 },
});

const html = renderToStaticMarkup(
  React.createElement(ContextPanel, {
    collapsed: false,
    onCollapse: () => {},
    onExpand: () => {},
    usage: snap,
    onUsageRefresh: () => {},
  })
);

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

check("Last turn is not rendered", () => {
  assert.equal(html.includes("Last turn"), false);
  assert.equal(snap.windows.find((w) => w.id === "last-turn"), undefined);
});

check("Session total is not rendered", () => {
  assert.equal(html.includes("Session total"), false);
  assert.equal(snap.windows.find((w) => w.id === "session-total"), undefined);
});

check("context and cost stay session facts, not quota rows", () => {
  assert.equal(html.includes("Context window"), false);
  assert.equal(html.includes("Session cost"), false);
  assert.equal(snap.windows.find((w) => w.id === "context")?.percentage, 26.5);
  assert.equal(snap.windows.find((w) => w.id === "cost")?.detail, "$0.045");
});

console.log(`\n${passed} checks passed.`);
