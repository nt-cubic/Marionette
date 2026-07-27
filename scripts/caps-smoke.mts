/**
 * Composer capability-merge smoke test.
 *
 * Run: npx tsx scripts/caps-smoke.mts
 *
 * Guards the two ways this has gone wrong before:
 *  - inventing a control the agent will reject ("Unknown config option: effort")
 *  - hiding a control the agent does support, because it advertises it somewhere
 *    other than `configOptions` (Grok)
 */
import assert from "node:assert/strict";
import {
  expandAcpConfigAttempts,
  mergeAcpCapabilities,
} from "../src/lib/acpSupplements.ts";
import type { CapabilitySnapshot } from "../src/lib/types.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

const emptyLive = (over: Partial<CapabilitySnapshot> = {}): CapabilitySnapshot => ({
  modes: [],
  models: [],
  thinkingEffort: null,
  effortOptions: [],
  supportsCancel: true,
  currentMode: null,
  currentModel: null,
  currentEffort: null,
  currentEffortId: null,
  modelConfigId: null,
  modeConfigId: null,
  effortConfigId: null,
  ...over,
});

console.log("grok-build (session/new advertises models only)");

// Exactly what `grok agent stdio` 0.2.104 returns: models, no modes, no configOptions.
const grokLive = emptyLive({
  models: [{ id: "grok-4.5", label: "Grok 4.5" }],
  currentModel: "grok-4.5",
});

check("real plan/build modes are exposed", () => {
  const caps = mergeAcpCapabilities("grok-build", grokLive)!;
  assert.deepEqual(caps.modes.map((m) => m.id), ["plan", "build"]);
  // Grok silently no-ops these, so they must not be offered as modes.
  assert.ok(!caps.modes.some((m) => m.id === "ask" || m.id === "auto-approve"));
});

check("mode chip defaults to build, which is where Grok actually starts", () => {
  const caps = mergeAcpCapabilities("grok-build", grokLive)!;
  assert.equal(caps.currentMode, "build");
});

check("effort survives having no config id, and claims no config id", () => {
  const caps = mergeAcpCapabilities("grok-build", grokLive)!;
  assert.deepEqual(caps.effortOptions.map((o) => o.id), ["high", "medium", "low"]);
  assert.equal(caps.currentEffortId, "high");
  // Both must stay null: there is no config option, and no 0-1 slider.
  assert.equal(caps.effortConfigId, null);
  assert.equal(caps.thinkingEffort, null);
});

check("effort is sent as a logical knob for Rust to route", () => {
  const caps = mergeAcpCapabilities("grok-build", grokLive)!;
  const attempts = expandAcpConfigAttempts("grok-build", { effortId: "low" }, caps);
  assert.deepEqual(attempts, [{ effortId: "low" }]);
  // A fabricated `effort` config option is exactly the bug to avoid.
  assert.ok(!attempts.some((a) => a.configId === "effort"));
});

check("mode is sent as a config option, so Rust can retry on -32601", () => {
  const caps = mergeAcpCapabilities("grok-build", grokLive)!;
  assert.deepEqual(expandAcpConfigAttempts("grok-build", { mode: "plan" }, caps), [
    { configId: "mode", value: "plan" },
  ]);
});

console.log("\nclaude-code (effort is model-dependent)");

check("no effort control is invented when Claude advertises none", () => {
  const caps = mergeAcpCapabilities(
    "claude-code",
    emptyLive({ models: [{ id: "opus", label: "Opus" }], modeConfigId: "mode" }),
  )!;
  assert.deepEqual(caps.effortOptions, []);
  assert.equal(caps.effortConfigId, null);
  assert.equal(caps.currentEffortId, null);
  assert.deepEqual(expandAcpConfigAttempts("claude-code", { effortId: "high" }, caps), []);
});

check("a live effort config is used verbatim", () => {
  const caps = mergeAcpCapabilities(
    "claude-code",
    emptyLive({
      effortConfigId: "effort",
      effortOptions: [{ id: "low", label: "Low" }, { id: "high", label: "High" }],
      currentEffortId: "low",
    }),
  )!;
  assert.equal(caps.effortConfigId, "effort");
  assert.deepEqual(expandAcpConfigAttempts("claude-code", { effortId: "high" }, caps), [
    { configId: "effort", value: "high" },
  ]);
});

check("live modes win over the supplement's wire-id list", () => {
  const caps = mergeAcpCapabilities(
    "claude-code",
    emptyLive({ modes: [{ id: "plan", label: "Plan Mode" }], currentMode: "plan" }),
  )!;
  assert.deepEqual(caps.modes.map((m) => m.id), ["plan"]);
});

console.log(`\n${passed} checks passed.`);
