/**
 * Layout check: long "tokens" usage rows (Last turn / Session total) must
 * render their value as a small line UNDER the label, not inline beside it.
 *
 * Run: npx tsx scripts/verify-usage-row-layout.mts
 */
import assert from "node:assert/strict";
import * as React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { ContextPanel } from "../src/components/ContextPanel";
import { buildUsageSnapshot, emptySessionUsage } from "../src/lib/usage";

// Mirrors the reported panel: a Grok turn with ~139K in / 2.2K out + speeds.
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

/** Isolate one usage-row chunk: from its opening tag to the next row/section. */
function row(label: string): string {
  const start = html.indexOf(`<span>${label}</span>`);
  assert.ok(start > 0, `row for "${label}" not found`);
  const rowStart = html.lastIndexOf("<div class=\"usage-row", start);
  const next = html.indexOf("<div class=\"usage-row", start + 1);
  assert.ok(rowStart > 0, `row div for "${label}" not found`);
  return html.slice(rowStart, next > start ? next : html.indexOf("</section>", start));
}

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

check("Last turn value moves to a small detail line under the label", () => {
  const r = row("Last turn");
  assert.match(r, /<span class="usage-row__detail">/);
  assert.match(r, /139K in · 2\.2K out · 139K cached/);
  assert.match(r, /13\.8 tok\/s out/);
});

check("Last turn no longer carries an inline <strong> value", () => {
  assert.doesNotMatch(row("Last turn"), /<strong/);
});

check("Session total gets the same stacked layout", () => {
  const r = row("Session total");
  assert.match(r, /<span class="usage-row__detail">/);
  assert.match(r, /25K in · 58 out · 5\.6K cached · 2 turns/);
  assert.doesNotMatch(r, /<strong/);
});

check("context row keeps its inline % + meter", () => {
  const r = row("Context window");
  assert.match(r, /<strong>26\.5%<\/strong>/);
  assert.match(r, /usage-meter__track/);
});

check("cost row keeps its inline value", () => {
  const r = row("Session cost");
  assert.match(r, /<strong/);
  assert.doesNotMatch(r, /usage-row__detail/);
});

console.log(`\n${passed} checks passed.`);
