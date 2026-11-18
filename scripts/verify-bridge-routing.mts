/**
 * Cross-use id/stale-state regression.
 * Run: npx tsx scripts/verify-bridge-routing.mts
 */
import assert from "node:assert/strict";
import { expandAcpConfigAttempts } from "../src/lib/acpSupplements.ts";
import {
  bridgeBindingIsStale,
  bridgeSubmissionModelId,
  nativeBridgeModelId,
  reportsNativeBridgeModel,
} from "../src/lib/bridgeRouting.ts";

const engineId = "source-73800e7663517058/deepseek-v4.1-flash:1017";
const wireId = `marionette/${engineId}`;

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`ok ${passed} - ${name}`);
}

check("bridge wire ids use each agent's native provider separator", () => {
  assert.equal(nativeBridgeModelId("opencode", engineId), `marionette/${engineId}`);
  assert.equal(nativeBridgeModelId("deepseek", engineId), `marionette::${engineId}`);
  assert.equal(nativeBridgeModelId("zcode", engineId), `marionette\\${engineId}`);
  assert.equal(bridgeSubmissionModelId("opencode", engineId), `marionette/${engineId}`);
  assert.equal(bridgeSubmissionModelId("claude-code", engineId), engineId);
});

check("OpenCode config attempts carry the prefixed wire id", () => {
  assert.deepEqual(
    expandAcpConfigAttempts(
      "opencode",
      { model: bridgeSubmissionModelId("opencode", engineId) },
      null,
    ),
    [{ configId: "model", value: wireId }],
  );
});

check("only catalog-native agents are strict about the live bridge id", () => {
  assert.equal(reportsNativeBridgeModel("opencode"), true);
  assert.equal(reportsNativeBridgeModel("grok-build"), true);
  assert.equal(reportsNativeBridgeModel("deepseek"), true);
  assert.equal(reportsNativeBridgeModel("pi"), true);
  assert.equal(reportsNativeBridgeModel("zcode"), true);
  assert.equal(reportsNativeBridgeModel("claude-code"), false);
  assert.equal(reportsNativeBridgeModel("codex"), false);
});

check("OpenCode fallback to another provider marks the binding stale", () => {
  assert.equal(
    bridgeBindingIsStale({
      agentId: "opencode",
      binding: { modelId: engineId },
      capsLive: true,
      currentModel: "openrouter/openai/gpt-6.2-sol",
      models: [
        { id: `marionette/${engineId}` },
        { id: "openrouter/openai/gpt-6.2-sol" },
      ],
    }),
    true,
  );
});

check("missing injected bridge provider marks the binding stale", () => {
  assert.equal(
    bridgeBindingIsStale({
      agentId: "opencode",
      binding: { modelId: engineId },
      capsLive: true,
      currentModel: null,
      models: [{ id: "openrouter/openai/gpt-6.2-sol" }],
    }),
    true,
  );
});

check("matching catalog-native live model is not stale", () => {
  assert.equal(
    bridgeBindingIsStale({
      agentId: "opencode",
      binding: { modelId: engineId },
      capsLive: true,
      currentModel: `marionette/${engineId}`,
      models: [{ id: `marionette/${engineId}` }],
    }),
    false,
  );
});

check("a different bridge-native model is stale", () => {
  assert.equal(
    bridgeBindingIsStale({
      agentId: "opencode",
      binding: { modelId: engineId },
      capsLive: true,
      currentModel: "marionette/source-other/deepseek-v4-pro",
      models: [{ id: "marionette/source-other/deepseek-v4-pro" }],
    }),
    true,
  );
});

check("cache-only capabilities do not raise a false stale warning", () => {
  assert.equal(
    bridgeBindingIsStale({
      agentId: "opencode",
      binding: { modelId: engineId },
      capsLive: false,
      currentModel: "openrouter/openai/gpt-6.2-sol",
      models: [{ id: "openrouter/openai/gpt-6.2-sol" }],
    }),
    false,
  );
});

check("indirect Claude bridge keeps its native live id", () => {
  assert.equal(
    bridgeBindingIsStale({
      agentId: "claude-code",
      binding: { modelId: engineId },
      capsLive: true,
      currentModel: "claude-sonnet-5",
      models: [{ id: "claude-sonnet-5" }],
    }),
    false,
  );
});

console.log(`\n${passed} checks passed.`);
