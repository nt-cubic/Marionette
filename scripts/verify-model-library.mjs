import assert from "node:assert/strict";
import { build } from "esbuild";
const bundle = await build({ stdin: { contents: 'export * from "./src/lib/modelLibrary.ts"; export * from "./src/lib/credentialScan.ts";', resolveDir: process.cwd() }, bundle: true, platform: "node", format: "esm", write: false });
const { libraryGroups, visibleLibraryGroups, DEFAULT_MODEL_PREFERENCES, normalizeCredentialScan, rowHidden, toggleRow, expandHiddenProviders } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].contents).toString("base64")}`);
const source = { id: "source-ollama", label: "Ollama", endpoint: "https://ollama.com/v1", protocol: "chat", origins: ["Grok config.toml"], issue: null,
  models: [{ id: "deepseek-v4.1-flash:1017", label: "Flash", nativeIds: { "grok-build": ["ollama-flash", "ollama-flash-alias"] } }] };
const model = { entryId: "engine", preset: "engine", modelId: "source-ollama/deepseek-v4.1-flash:1017", label: "wire id" };
const catalog = { models: [model, { ...model, modelId: "another-account/deepseek-v4.1-flash:1017" }], note: null };
const native = [{ id: "ollama-flash", label: "Flash" }, { id: "ollama-flash-alias", label: "Flash alias" }, { id: "marionette/source-ollama/deepseek-v4.1-flash:1017", label: "Injected duplicate" }];
const groups = libraryGroups("grok-build", native, catalog, [source]);
assert.equal(groups.length, 2, "same model on different providers must survive");
assert.equal(groups.flatMap(g => g.rows).length, 2, "native aliases and injected bridge rows collapse into their exact source/model");
const ollama = groups.find(g => g.label === "Ollama");
assert.equal(ollama.rows[0].native.id, "ollama-flash", "the owning agent uses its existing native model");
const codex = libraryGroups("codex", [], catalog, [source]);
assert.equal(codex.find(g => g.label === "Ollama").rows[0].bridge.modelId, model.modelId, "another agent uses the shared model without a second key");
assert.equal(visibleLibraryGroups(groups, { ...DEFAULT_MODEL_PREFERENCES, hiddenProviders: [ollama.key] }).length, 1);
assert.equal(visibleLibraryGroups(groups, { ...DEFAULT_MODEL_PREFERENCES, hiddenModels: [ollama.rows[0].key] }).length, 1);
const custom = [{ key: "a", label: "A", rows: [{ key: "a/1", label: "1", modelId: "1" }, { key: "a/2", label: "2", modelId: "2" }] }, { key: "b", label: "B", rows: [{ key: "b/1", label: "1", modelId: "1" }] }];
const sorted = visibleLibraryGroups(custom, { ...DEFAULT_MODEL_PREFERENCES, providerOrder: ["b", "a"], modelOrder: { a: ["a/2", "a/1"] } });
assert.deepEqual(sorted.map(g => g.key), ["b", "a"]);
assert.deepEqual(sorted[1].rows.map(r => r.key), ["a/2", "a/1"]);
assert.equal(visibleLibraryGroups(groups, DEFAULT_MODEL_PREFERENCES, "ollama").length, 1);
const sameAccount = libraryGroups("grok-build", [], { models: [
  { entryId: "deepseek-key", modelId: "flash", label: "flash", preset: "deepseek" },
  { entryId: "engine", modelId: "deepseek/flash", label: "flash", preset: "engine" },
  { entryId: "another-key", modelId: "flash", label: "flash", preset: "deepseek" },
], note: null }, [], { "entry:deepseek-key": "engine:deepseek" });
assert.equal(sameAccount.length, 2, "only aliases verified as the same account merge");
assert.equal(sameAccount.find(g => g.key === "engine:deepseek").rows.length, 1);
assert.equal(visibleLibraryGroups(sameAccount, { ...DEFAULT_MODEL_PREFERENCES, hiddenProviders: ["engine:deepseek"] }).length, 1, "hiding a source hides every verified route to it");
const report = normalizeCredentialScan({
  grok: { agent: "grok", entries: [{ id: "grok/channel/ollama", source: "scan:grok", label: "Ollama", kind: "reference" }], note: null, error: null },
  codex: { agent: "codex", entries: [], note: "not installed", error: null },
  claude: { agent: "claude", entries: [], error: "cannot read credentials" },
});
assert.equal(report.sources.find(s => s.agent === "grok").status, "found");
assert.equal(report.entries.length, 1, "scanned references are displayed rather than discarded");
assert.equal(report.sources.find(s => s.agent === "claude").detail, "cannot read credentials");
assert.throws(() => normalizeCredentialScan({}), /未知格式/);

// ── One account, two discovery paths ─────────────────────────────────────────
// OpenCode's native `deepseek` provider and Grok's own `deepseek-v4.1-flash`
// table are the same account (same host, same DEEPSEEK_API_KEY). The backend
// proves it and reports `nativeAccounts`; the picker must list it once.
const duplicateSource = { id: "source-deepseek", label: "deepseek.com", endpoint: "https://api.deepseek.com/v1", protocol: "chat",
  origins: ["grok-build · ~/.grok/config.toml#deepseek-v4.1-flash"], issue: null,
  nativeAccounts: { opencode: [{ provider: "deepseek", label: "DeepSeek" }] },
  models: [{ id: "deepseek-flash", label: "DeepSeek V4.1 Flash 1017", nativeIds: {} }] };
const opencodeNative = [
  { id: "deepseek/deepseek-v4-pro", label: "DeepSeek/DeepSeek V4 Pro" },
  { id: "deepseek/deepseek-flash", label: "DeepSeek/DeepSeek V4.1 Flash 1017" },
  { id: "opencode/beam-free", label: "OpenCode/Beam Free" },
];
const engineCatalog = { models: [
  { entryId: "engine", modelId: "source-deepseek/deepseek-flash", label: "wire", preset: "engine" },
], note: null };
const merged = libraryGroups("opencode", opencodeNative, engineCatalog, [duplicateSource]);
const deepseekGroup = merged.find(g => g.rows.some(r => r.native?.id === "deepseek/deepseek-flash"));
assert.ok(deepseekGroup, "the DeepSeek account keeps a group");
assert.equal(deepseekGroup.key, "engine:source-deepseek", "native rows join the discovered source");
assert.equal(deepseekGroup.label, "DeepSeek", "the group is named the way the agent names the vendor");
assert.equal(deepseekGroup.rows.filter(r => r.modelId === "deepseek-flash").length, 1, "one model keeps one row across both paths");
assert.equal(deepseekGroup.rows.find(r => r.modelId === "deepseek-flash").native.id, "deepseek/deepseek-flash", "the row carries the agent's own id");
assert.equal(merged.filter(g => /deepseek\.com|DeepSeek/.test(g.label)).length, 1, "no second DeepSeek header");
assert.ok(merged.some(g => g.label === "DeepSeek" && g.rows.some(r => r.modelId === "deepseek-v4-pro")), "other models of the same account follow it");
assert.ok(merged.some(g => g.label === "OpenCode"), "unrelated native families stay separate");

// A retired provider key (the pre-merge `native:opencode:DeepSeek` header) must
// keep its models hidden, and showing a row clears its stale key.
const preMerge = { ...DEFAULT_MODEL_PREFERENCES, hiddenProviders: ["native:opencode:DeepSeek"] };
const migrated = expandHiddenProviders(preMerge, merged);
assert.deepEqual(migrated.hiddenProviders, [], "the retired provider header is dropped");
assert.ok(migrated.hiddenModels.length >= 1, "its models become hidden rows");
assert.equal(visibleLibraryGroups(merged, migrated).some(g => g.label === "DeepSeek"), false, "a hidden account stays hidden after the merge");
const row = merged.find(g => g.key === "engine:source-deepseek").rows[0];
assert.equal(rowHidden(migrated, row), true);
assert.equal(rowHidden(toggleRow(migrated, row), row), false, "showing a merged row clears every identity it had");
assert.throws(() => normalizeCredentialScan({ a: 1 }), /未知格式/);

// ── One subscription sign-in, one header ─────────────────────────────────────
// Codex CLI's own catalogue and the engine's `codex` provider run on the same
// ~/.codex/auth.json login. OpenCode keeps a copy of that OAuth token under its
// own `openai` provider; listing it again would repeat the account's models.
const chatgptAccounts = [
  { engine: "codex", agent: "codex", provider: "", kind: "owner" },
  { engine: "codex", agent: "opencode", provider: "openai", kind: "duplicate" },
];
const codexNative = [
  { id: "gpt-6.1-sol", label: "GPT-6.1-Sol" },
  { id: "gpt-5.6-luna", label: "GPT-5.6-Luna" },
  { id: "codex-auto-review", label: "Codex Auto Review" },
];
const openaiSubset = { models: [
  { entryId: "engine", modelId: "codex/gpt-6.1-sol", label: "codex/gpt-6.1-sol", preset: "engine" },
  { entryId: "engine", modelId: "codex/gpt-5.6-luna", label: "codex/gpt-5.6-luna", preset: "engine" },
  { entryId: "engine", modelId: "codex/gpt-5.6-sol", label: "codex/gpt-5.6-sol", preset: "engine" },
], note: null };

const codexMenu = libraryGroups("codex", codexNative, openaiSubset, [], {}, chatgptAccounts);
assert.equal(codexMenu.filter(g => /chatgpt|openai/i.test(g.label)).length, 1, "Codex's own catalogue joins the engine group");
const chatgpt = codexMenu.find(g => g.key === "engine:codex");
assert.ok(chatgpt, "the merged group keeps the engine key, so saved order and visibility still apply");
assert.deepEqual(chatgpt.rows.map(r => r.modelId), ["gpt-6.1-sol", "gpt-5.6-luna", "gpt-5.6-sol", "codex-auto-review"], "one model keeps one row");
assert.equal(chatgpt.rows.find(r => r.modelId === "gpt-6.1-sol").native.id, "gpt-6.1-sol", "the row still switches through Codex's own id");
assert.equal(codexMenu.some(g => g.label === "Models"), false, "a leftover header of the same login folds in too");

// A model hidden before the merge keeps its hide through the old native keys.
const hiddenBefore = { ...DEFAULT_MODEL_PREFERENCES, hiddenModels: ["native:codex:OpenAI/gpt-5.6-luna"] };
const afterHide = expandHiddenProviders(hiddenBefore, codexMenu);
assert.equal(rowHidden(afterHide, chatgpt.rows.find(r => r.modelId === "gpt-5.6-luna")), true, "the pre-merge hide follows the row");
assert.equal(visibleLibraryGroups(codexMenu, afterHide).find(g => g.key === "engine:codex").rows.length, 3);

// OpenCode's copied sign-in is not a second OpenAI group.
const copilotNative = [
  { id: "openai/gpt-6.1-sol", label: "OpenAI/GPT-6.1 Sol" },
  { id: "openai/gpt-5.4", label: "OpenAI/GPT-5.4" },
  { id: "opencode/big-pickle", label: "OpenCode/Big Pickle" },
];
const opencodeMenu = libraryGroups("opencode", copilotNative, openaiSubset, [], {}, chatgptAccounts);
assert.equal(opencodeMenu.filter(g => /openai/i.test(g.label)).length, 0, "the copied OAuth listing is left out while the account is listed");
assert.ok(opencodeMenu.some(g => g.label === "OpenCode"), "unrelated providers of the same agent stay");

// Engine down: the owner group has no rows, and the copy is the only route left.
const engineDown = libraryGroups("opencode", copilotNative, { models: [], note: null }, [], {}, chatgptAccounts);
assert.ok(engineDown.some(g => g.rows.some(r => r.native?.id === "openai/gpt-6.1-sol")), "without the engine the copied sign-in still reaches the account");
console.log("Model library: source identity, native/shared selection, visibility, ordering, search and credential scan contract passed.");
