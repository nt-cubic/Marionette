/**
 * Composer capability-merge smoke test.
 *
 * Run: npx tsx scripts/caps-smoke.mts
 *
 * Guards the two ways this has gone wrong before:
 *  - inventing a control the agent will reject ("Unknown config option: effort")
 *  - hiding a control the agent does support, because it advertises it somewhere
 *    other than `configOptions` (Grok)
 *
 * Also pins the display cleanup: tier labels must not repeat the "Effort" header.
 */
import assert from "node:assert/strict";
import {
  expandAcpConfigAttempts,
  mergeAcpCapabilities,
  modelAssertForEffortRefresh,
} from "../src/lib/acpSupplements.ts";
import {
  groupModelsByProvider,
  modelProviderGroup,
  modelRowLabel,
  prettyEffortLabel,
  prettyModelChip,
} from "../src/lib/modelLabel.ts";
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
  // max sits on top for bridged/config.toml models, which carry no live
  // _meta.reasoningEfforts menu; live menus still win for grok's own models.
  assert.deepEqual(caps.effortOptions.map((o) => o.id), ["max", "high", "medium", "low"]);
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

console.log("\ngrok-build (a custom catalog entry publishes its own tiers)");

// Exactly what session/new returns for a `config.toml` entry that declares all
// seven tiers: the menu is per model, so it must not be flattened to three.
const grokDeepSeekLive = emptyLive({
  models: [{ id: "deepseek-flash", label: "DeepSeek Flash (Official)" }],
  currentModel: "deepseek-flash",
  currentEffortId: "high",
  effortOptions: [
    { id: "none", label: "No Reasoning" },
    { id: "minimal", label: "Minimal Effort" },
    { id: "low", label: "Low Effort" },
    { id: "medium", label: "Medium Effort" },
    { id: "high", label: "High Effort" },
    { id: "xhigh", label: "Extra High Effort" },
    { id: "max", label: "Max Effort" },
  ],
});

check("a live per-model menu wins over the supplement's three levels", () => {
  const caps = mergeAcpCapabilities("grok-build", grokDeepSeekLive)!;
  assert.deepEqual(
    caps.effortOptions.map((o) => o.id),
    ["none", "minimal", "low", "medium", "high", "xhigh", "max"],
  );
  assert.equal(caps.effortOptions.find((o) => o.id === "max")?.label, "Max Effort");
});

check("max is routable as a logical knob, not dropped for being non-canonical", () => {
  const caps = mergeAcpCapabilities("grok-build", grokDeepSeekLive)!;
  assert.deepEqual(expandAcpConfigAttempts("grok-build", { effortId: "max" }, caps), [
    { effortId: "max" },
  ]);
});

check("a model whose menu is smaller does not inherit the fallback's levels", () => {
  const caps = mergeAcpCapabilities(
    "grok-build",
    emptyLive({
      models: [{ id: "grok-4.5", label: "Grok 4.5" }],
      currentModel: "grok-4.5",
      currentEffortId: "high",
      effortOptions: [
        { id: "high", label: "High" },
        { id: "medium", label: "Medium" },
        { id: "low", label: "Low" },
      ],
    }),
  )!;
  assert.deepEqual(caps.effortOptions.map((o) => o.id), ["high", "medium", "low"]);
  assert.equal(caps.currentEffortId, "high");
});

check("the live current level still wins over the supplement default", () => {
  const caps = mergeAcpCapabilities(
    "grok-build",
    emptyLive({ ...grokDeepSeekLive, currentEffortId: "max" }),
  )!;
  assert.equal(caps.currentEffortId, "max");
});

console.log("\nagents with no supplement (opencode, kimi, cursor, …)");

check("their live effort list is passed through untouched", () => {
  for (const agentId of ["opencode", "kimi-code", "cursor", "openclaw", "pi", "cline"]) {
    const caps = mergeAcpCapabilities(agentId, grokDeepSeekLive)!;
    assert.deepEqual(
      caps.effortOptions.map((o) => o.id),
      ["none", "minimal", "low", "medium", "high", "xhigh", "max"],
      agentId,
    );
  }
});

check("and no list is invented when they advertise none", () => {
  const caps = mergeAcpCapabilities("opencode", emptyLive())!;
  assert.deepEqual(caps.effortOptions, []);
  assert.equal(caps.currentEffortId, null);
});

console.log("\ncodex (empty catalog: effort lives in model[effort])");

check("effort chip is reconstructed when live advertised none", () => {
  const caps = mergeAcpCapabilities(
    "codex",
    emptyLive({
      models: [{ id: "gpt-5.6-luna", label: "gpt-5.6-luna" }],
      currentModel: "gpt-5.6-luna",
      currentEffortId: "xhigh",
    }),
  )!;
  assert.deepEqual(
    caps.effortOptions.map((o) => o.id),
    ["xhigh", "high", "medium", "low", "minimal", "none"],
  );
  assert.equal(caps.currentEffortId, "xhigh");
  assert.equal(caps.effortConfigId, null);
  assert.equal(caps.thinkingEffort, null);
});

check("effort is sent as a logical knob, not a fabricated config id", () => {
  const caps = mergeAcpCapabilities(
    "codex",
    emptyLive({
      currentModel: "gpt-5.6-luna",
      currentEffortId: "xhigh",
    }),
  )!;
  assert.deepEqual(expandAcpConfigAttempts("codex", { effortId: "high" }, caps), [
    { effortId: "high" },
  ]);
  assert.ok(!caps.effortConfigId);
});

check("a live reasoning_effort option still uses the advertised id", () => {
  const caps = mergeAcpCapabilities(
    "codex",
    emptyLive({
      effortConfigId: "reasoning_effort",
      effortOptions: [
        { id: "low", label: "Low" },
        { id: "high", label: "High" },
      ],
      currentEffortId: "low",
      currentModel: "gpt-5.6-terra",
    }),
  )!;
  assert.equal(caps.effortConfigId, "reasoning_effort");
  assert.deepEqual(
    caps.effortOptions.map((o) => o.id),
    ["low", "high"],
  );
  assert.deepEqual(expandAcpConfigAttempts("codex", { effortId: "high" }, caps), [
    { configId: "reasoning_effort", value: "high" },
  ]);
});

console.log("\neffort tier labels (the composer prints the axis name itself)");

check("a trailing 'Effort' is dropped so the header is not repeated", () => {
  // Exactly the labels Grok forwards from a config.toml reasoning_efforts entry.
  const live = [
    { id: "none", label: "No Reasoning" },
    { id: "minimal", label: "Minimal Effort" },
    { id: "low", label: "Low Effort" },
    { id: "medium", label: "Medium Effort" },
    { id: "high", label: "High Effort" },
    { id: "xhigh", label: "Extra High Effort" },
    { id: "max", label: "Max Effort" },
  ];
  assert.deepEqual(
    live.map((o) => prettyEffortLabel(o.label, o.id)),
    ["No Reasoning", "Minimal", "Low", "Medium", "High", "Extra High", "Max"],
  );
});

check("labels that never carried the axis name survive verbatim", () => {
  assert.equal(prettyEffortLabel("High", "high"), "High");
  assert.equal(prettyEffortLabel("Max", "max"), "Max");
  assert.equal(prettyEffortLabel("推理档位", "high"), "推理档位");
  // Rust substitutes the wire id when a tier arrives with no label at all.
  assert.equal(prettyEffortLabel("", "xhigh"), "xhigh");
  assert.equal(prettyEffortLabel(null, null), "");
  // A label that *is* the axis name must not be reduced to nothing.
  assert.equal(prettyEffortLabel("Effort", "effort"), "Effort");
});

console.log("\nmodel menu groups (Grok publishes a flat catalog: modelId + name)");

/** The catalog this user's Grok advertises, name tags included. */
const GROK_CATALOG = [
  { id: "grok-4.7", label: "Grok 4.7" },
  { id: "grok-4.7-build-fast", label: "Grok 4.7 Fast" },
  { id: "grok-4.6", label: "Grok 4.6" },
  { id: "qwen3.8-27b-efficientthink", label: "Qwen 3.8 27B EfficientThink (Local)" },
  { id: "qwen3.8-27b-dflash", label: "Qwen 3.8 27B EfficientThink (Local, DFlash2)" },
  { id: "deepseek-v4-dspark", label: "DeepSeek V4 Flash 0731 DSpark (Local, 1M)" },
  { id: "deepseek-v4-vision", label: "DeepSeek V4 Flash Vision (Local, 256K)" },
  { id: "teamo-claude-opus-5", label: "claude-opus-5 (Teamo)" },
  { id: "teamo-deepseek-v4-pro", label: "deepseek-v4-pro (Teamo)" },
  { id: "teamo-gpt-image-2", label: "gpt-image-2 (Teamo)" },
  { id: "deepseek-flash", label: "DeepSeek Flash (Official)" },
  { id: "deepseek-v4-pro", label: "DeepSeek V4 Pro (Official)" },
];

check("a backend tag in the name beats the model family", () => {
  const byId = (id: string) => GROK_CATALOG.find((m) => m.id === id)!;
  // These two carry a DeepSeek family name but run on a Team Router / local box.
  assert.equal(modelProviderGroup(byId("teamo-deepseek-v4-pro")), "Team Router");
  assert.equal(modelProviderGroup(byId("deepseek-v4-dspark")), "Local");
  assert.equal(modelProviderGroup(byId("qwen3.8-27b-efficientthink")), "Local");
  // Untagged official entries fall back to the family prefix.
  assert.equal(modelProviderGroup(byId("deepseek-flash")), "DeepSeek");
  assert.equal(modelProviderGroup(byId("grok-4.6")), "xAI");
});

check("rows drop the tag the group header already prints", () => {
  const byId = (id: string) => GROK_CATALOG.find((m) => m.id === id)!;
  assert.equal(modelRowLabel(byId("teamo-claude-opus-5")), "Claude Opus 5");
  // "(Local, 1M)" keeps the size and loses the redundant word.
  assert.equal(
    modelRowLabel(byId("deepseek-v4-dspark")),
    "DeepSeek V4 Flash 0731 DSpark (1M)",
  );
  assert.equal(modelRowLabel(byId("deepseek-flash")), "DeepSeek Flash");
  // A tag that separates two same-family entries survives.
  assert.equal(
    modelRowLabel(byId("qwen3.8-27b-dflash")),
    "Qwen 3.8 27B EfficientThink (DFlash2)",
  );
});

check("the catalog lands in one group per backend, not one pile", () => {
  const groups = groupModelsByProvider(GROK_CATALOG);
  assert.deepEqual(
    groups.map((g) => `${g.provider}:${g.models.length}`),
    ["DeepSeek:2", "Local:4", "Team Router:3", "xAI:3"],
  );
});

check("provider/model ids and Claude aliases keep their old groups", () => {
  assert.equal(modelProviderGroup({ id: "opencode/zen", label: "opencode/zen" }), "OpenCode");
  assert.equal(
    modelProviderGroup({ id: "OpenCode Zen/foo", label: "OpenCode Zen/foo" }),
    "OpenCode",
  );
  assert.equal(modelProviderGroup({ id: "opus", label: "Opus" }), "Claude");
  assert.equal(modelProviderGroup({ id: "claude-sonnet-4-6", label: "Sonnet 4.6" }), "Claude");
  // Nothing recognisable still has to land somewhere the user can click.
  assert.equal(modelProviderGroup({ id: "mystery-1", label: "Mystery One" }), "Models");
});

check("closed chip prefixes the source when the pretty name hides it", () => {
  assert.equal(
    prettyModelChip({ id: "logfare/deepseek-v4-pro", label: "DeepSeek V4 Pro" }),
    "Logfare · DeepSeek V4 Pro",
  );
  assert.equal(
    prettyModelChip({ id: "home-qwen/qwen3.8-27b-dflash", label: "Qwen3.8-27B DFlash" }),
    "home-qwen · Qwen3.8-27B DFlash",
  );
  // Official DeepSeek already says DeepSeek, so the chip stays short.
  assert.equal(
    prettyModelChip({ id: "deepseek/deepseek-v4.1-flash", label: "DeepSeek V4.1 Flash" }),
    "DeepSeek V4.1 Flash",
  );
  assert.equal(
    prettyModelChip({ id: "opencode/cat-jam-free", label: "Cat Jam Free" }),
    "OpenCode · Cat Jam Free",
  );
});

check("Grok catalog keys group by billed host; [Provider] names are display-only", () => {
  assert.equal(
    modelProviderGroup({ id: "zhipu-glm-4.5", label: "[Zhipu] GLM-4.5" }),
    "Z.AI",
  );
  assert.equal(
    modelProviderGroup({ id: "zhipu-glm-4.5", label: "GLM-4.5" }),
    "Z.AI",
  );
  assert.equal(
    modelProviderGroup({ id: "opencode-go-cat-jam-free", label: "[OpenCode] cat-jam-free" }),
    "OpenCode",
  );
  assert.equal(
    modelProviderGroup({ id: "opencode-go-glm-5.3", label: "[OpenCode] GLM-5.3" }),
    "OpenCode",
  );
  assert.equal(
    modelProviderGroup({ id: "deepseek-flash", label: "[DeepSeek] Flash (V4.1)" }),
    "DeepSeek",
  );
  assert.equal(
    modelProviderGroup({
      id: "home-qwen-dflash",
      label: "[SelfHosted] Qwen3.8-27B DFlash (Tailscale)",
    }),
    "SelfHosted",
  );
  assert.equal(modelProviderGroup({ id: "grok-4.6", label: "Grok 4.6" }), "xAI");
});

check("Grok row labels drop the [Provider] prefix the group header already prints", () => {
  assert.equal(modelRowLabel({ id: "zhipu-glm-4.5", label: "[Zhipu] GLM-4.5" }), "GLM-4.5");
  assert.equal(
    modelRowLabel({ id: "opencode-go-cat-jam-free", label: "[OpenCode] cat-jam-free" }),
    "cat-jam-free",
  );
  assert.equal(
    modelRowLabel({ id: "deepseek-flash", label: "[DeepSeek] Flash (V4.1)" }),
    "Flash (V4.1)",
  );
});

check("Grok closed chip prefixes every billed host, not only xAI", () => {
  assert.equal(
    prettyModelChip({ id: "opencode-go-cat-jam-free", label: "[OpenCode] cat-jam-free" }),
    "OpenCode · cat-jam-free",
  );
  assert.equal(
    prettyModelChip({ id: "opencode-go-glm-5.3", label: "[OpenCode] GLM-5.3" }),
    "OpenCode · GLM-5.3",
  );
  assert.equal(
    prettyModelChip({ id: "zhipu-glm-4.5", label: "[Zhipu] GLM-4.5" }),
    "Z.AI · GLM-4.5",
  );
  assert.equal(
    prettyModelChip({ id: "deepseek-flash", label: "[DeepSeek] Flash (V4.1)" }),
    "DeepSeek · Flash (V4.1)",
  );
  // After dropping the [DeepSeek] tag, official names already say DeepSeek.
  assert.equal(
    prettyModelChip({ id: "deepseek-flash", label: "DeepSeek Flash (V4.1)" }),
    "DeepSeek Flash (V4.1)",
  );
  assert.equal(prettyModelChip({ id: "grok-4.6", label: "Grok 4.6" }), "xAI · Grok 4.6");
  assert.equal(
    prettyModelChip(
      {
        id: "home-qwen-dflash",
        label: "[SelfHosted] Qwen3.8-27B DFlash (Tailscale)",
      },
      { maxLen: 40 },
    ),
    "SelfHosted · Qwen3.8-27B DFlash (Tailscale)",
  );
});

console.log("\nopencode effort republish (session/new may omit effort when already on model)");

check("re-asserts preferred model when it already matches but effort is missing", () => {
  const caps = emptyLive({
    models: [{ id: "opencode-go/deepseek-v4.1-flash", label: "DeepSeek V4.1 Flash" }],
    currentModel: "opencode-go/deepseek-v4.1-flash",
    modelConfigId: "model",
  });
  assert.deepEqual(
    modelAssertForEffortRefresh(caps, "opencode-go/deepseek-v4.1-flash"),
    { model: "opencode-go/deepseek-v4.1-flash", reason: "republish" },
  );
});

check("switches when preferred model differs (switch itself republishes)", () => {
  const caps = emptyLive({
    models: [
      { id: "opencode/big-pickle", label: "Big Pickle" },
      { id: "opencode-go/deepseek-v4.1-flash", label: "DeepSeek V4.1 Flash" },
    ],
    currentModel: "opencode/big-pickle",
    modelConfigId: "model",
  });
  assert.deepEqual(
    modelAssertForEffortRefresh(caps, "opencode-go/deepseek-v4.1-flash"),
    { model: "opencode-go/deepseek-v4.1-flash", reason: "switch" },
  );
});

check("does nothing when effort is already advertised", () => {
  const caps = emptyLive({
    currentModel: "opencode-go/deepseek-v4.1-flash",
    effortConfigId: "effort",
    effortOptions: [
      { id: "low", label: "Low" },
      { id: "high", label: "High" },
    ],
  });
  assert.equal(modelAssertForEffortRefresh(caps, "opencode-go/deepseek-v4.1-flash"), null);
});

check("republishes current model when prefs are empty but effort is missing", () => {
  const caps = emptyLive({
    currentModel: "opencode-go/deepseek-v4.1-flash",
    modelConfigId: "model",
  });
  assert.deepEqual(modelAssertForEffortRefresh(caps, null), {
    model: "opencode-go/deepseek-v4.1-flash",
    reason: "republish",
  });
});

console.log(`\n${passed} checks passed.`);
