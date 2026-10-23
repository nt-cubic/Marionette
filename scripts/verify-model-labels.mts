/**
 * Model label / menu grouping for ACP option names.
 *
 * Locks in the zcode shapes: `provider\model` ids and the adapter's
 * `Provider › Model` display names (2026-10-23, docs/pitfalls-zcode-acp.md
 * 雷 12). A provider that arrives as a bare `bigmodel-api` group header, or a
 * row that repeats the provider the header already prints, is the regression
 * this guards against.
 *
 * Run: npx tsx scripts/verify-model-labels.mts
 */
import assert from "node:assert/strict";
import {
  modelProviderGroup,
  modelRowLabel,
  prettyModelChip,
} from "../src/lib/modelLabel.ts";

let passed = 0;
function check(name: string, fn: () => void) {
  fn();
  passed += 1;
  console.log(`  ok  ${name}`);
}

const BS = "\\";

console.log("modelProviderGroup / modelRowLabel across agent id shapes");

check("zcode personal provider groups by its display name", () => {
  const model = { id: `bigmodel-api${BS}GLM-5.3`, label: "BigModel Coding Plan › GLM-5.3" };
  assert.equal(modelProviderGroup(model), "BigModel");
  assert.equal(modelRowLabel(model), "GLM-5.3");
  assert.equal(prettyModelChip(model), "BigModel · GLM-5.3");
});

check("zcode uuid provider (account layer) groups by its display name", () => {
  const model = {
    id: `58062756-63ee-4a86-94e1-3e56c44b4a0a${BS}deepseek-v4-pro`,
    label: "Deepseek › deepseek-v4-pro",
  };
  assert.equal(modelProviderGroup(model), "DeepSeek");
  assert.equal(modelRowLabel(model), "deepseek-v4-pro");
});

check("zcode bridge id keeps provider\\model; the slash inside the model stays", () => {
  const model = { id: `marionette${BS}zhipu/glm-5.3`, label: "Marionette Engine › zhipu/glm-5.3" };
  assert.equal(modelProviderGroup(model), "Marionette");
  assert.equal(modelRowLabel(model), "glm-5.3");
});

check("engine `provider/model` and DSH `provider::model` are unchanged", () => {
  assert.equal(modelProviderGroup({ id: "zhipu/glm-5.3", label: "zhipu/glm-5.3" }), "Z.AI");
  assert.equal(
    modelProviderGroup({ id: "marionette::opencode-go/glm-5.3", label: "marionette::opencode-go/glm-5.3" }),
    "Marionette",
  );
});

check("plain catalog names still fall back to the family table", () => {
  assert.equal(modelProviderGroup({ id: "grok-4.7", label: "Grok 4.7" }), "xAI");
  assert.equal(modelRowLabel({ id: "grok-4.7", label: "Grok 4.7" }), "Grok 4.7");
});

console.log(`\n${passed} checks passed`);
