/**
 * Patch `zcode-acp-server` so personal providers that carry no
 * `modelConfigRules.providerModelRules` still advertise their models — and so
 * those models get the right context ceiling.
 *
 * Why: the ACP model dropdown is assembled by `dist/config/options.js`
 * `loadAllModels()` from
 *   1. `~/.zcode/v2/config.json` providers (account layer), and
 *   2. `provider_config.json` → `modelConfigRules.providerModelRules`.
 * A provider that exists ONLY in `provider_config.json` (3.12+ desktop stores
 * every user-added key there) contributes models solely through (2). Providers
 * created from a builtin template carry no such rules — the desktop renders
 * their models straight from `zcode-builtin.json`'s `builtinModelIds` — so
 * they contribute zero models and never appear in `session/new`'s
 * configOptions. Observed 2026-10-23: a BigModel Coding Plan key (`bigmodel-api`,
 * `personalModelIds: []`, no providerModelRules) was selectable in the desktop
 * and completely absent from the ACP dropdown.
 *
 * `modelContextWindow()` has the same gap (it reads config.json, then
 * providerModelRules), so a freshly listed model reports size 0 and the client's
 * context meter reads "used / 0". Both fall back to the builtin table's
 * `builtinModelIds` / `modelRules`.
 *
 * `npm i -g zcode-acp-server` (Marionette's agent upgrade button) replaces
 * `dist/`, so re-run this script after every adapter upgrade. Piecewise and
 * idempotent: each edit is applied only when missing, so an older patch is
 * upgraded instead of skipped.
 *
 * Usage:
 *   node scripts/patch-zcode-acp-personal-models.mjs [--dry-run]
 */
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";

const dryRun = process.argv.includes("--dry-run");

const BUILTIN_MODELS_MODULE = `/**
 * Model metadata from the bundled provider table (\`zcode-builtin.json\`).
 *
 * Two gaps this fills, both only for providers that live exclusively in
 * \`provider_config.json\` (3.12+ desktop stores every user-added key there):
 *
 *  - **Which models exist.** \`loadAllModels()\` reads
 *    \`modelConfigRules.providerModelRules\` for such providers; providers
 *    created from a builtin template have none, because the desktop renders
 *    them from the template's \`builtinModelIds\`. Without a fallback they
 *    contribute zero models to \`session/new\` configOptions (2026-10-23:
 *    a keyed BigModel Coding Plan provider was selectable in the desktop and
 *    absent from the ACP dropdown).
 *  - **Context ceiling.** \`modelContextWindow()\` has the same two sources, so
 *    a template-listed model reports 0 and the client meter reads
 *    "used / 0". \`modelRules\` carry the per-model windows.
 *
 * Table lookup order mirrors account-provider.ts's \`builtinTablePath()\` — the
 * derived injection first (that is the file the spawned backend boots from),
 * then the ambient env, then the desktop bundle and the CLI's runtime copy.
 */
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { BUILTIN_PROVIDER_ENV, builtinProviderEnv } from "../backend/resolve.js";
import { zcodeHomeDir } from "../utils.js";

const PROVIDER_CONFIG_NAME = "zcode-builtin.json";
/** Parsed tables by absolute path; the file changes only on app/CLI update. */
const cache = new Map();

/** Newest \`~/.zcode/v2/runtime/provider/<plat>/<ver>/<endpoint>/\` copy. */
function newestRuntimeTable() {
    const root = path.join(zcodeHomeDir(), "v2", "runtime", "provider");
    const hits = [];
    let platforms;
    try {
        platforms = readdirSync(root);
    }
    catch {
        return null;
    }
    for (const plat of platforms) {
        const platDir = path.join(root, plat);
        let versions;
        try {
            versions = readdirSync(platDir);
        }
        catch {
            continue;
        }
        for (const version of versions) {
            const versionDir = path.join(platDir, version);
            let endpoints;
            try {
                endpoints = readdirSync(versionDir);
            }
            catch {
                continue;
            }
            for (const endpoint of endpoints) {
                const file = path.join(versionDir, endpoint, PROVIDER_CONFIG_NAME);
                try {
                    if (existsSync(file))
                        hits.push({ file, mtime: statSync(file).mtimeMs });
                }
                catch {
                    /* vanished mid-scan */
                }
            }
        }
    }
    hits.sort((a, b) => b.mtime - a.mtime);
    return hits[0]?.file ?? null;
}

/** Desktop installs that keep the table under \`resources/config/provider\`. */
function desktopTable() {
    const candidates = [];
    const local = process.env.LOCALAPPDATA;
    if (local)
        candidates.push(path.join(local, "Programs", "ZCode", "resources", "config", "provider", PROVIDER_CONFIG_NAME));
    const pf = process.env.ProgramFiles;
    if (pf)
        candidates.push(path.join(pf, "ZCode", "resources", "config", "provider", PROVIDER_CONFIG_NAME));
    const pf86 = process.env["ProgramFiles(x86)"];
    if (pf86)
        candidates.push(path.join(pf86, "ZCode", "resources", "config", "provider", PROVIDER_CONFIG_NAME));
    if (process.platform === "darwin")
        candidates.push("/Applications/ZCode.app/Contents/Resources/config/provider/" + PROVIDER_CONFIG_NAME);
    return candidates.find((file) => existsSync(file)) ?? null;
}

function tablePath() {
    const injected = builtinProviderEnv()[BUILTIN_PROVIDER_ENV];
    if (injected)
        return injected;
    const fromEnv = process.env[BUILTIN_PROVIDER_ENV]?.trim();
    if (fromEnv)
        return fromEnv;
    return desktopTable() ?? newestRuntimeTable();
}

function loadTable() {
    const file = tablePath();
    if (!file)
        return null;
    if (cache.has(file))
        return cache.get(file);
    let table = null;
    try {
        table = JSON.parse(readFileSync(file, "utf8"));
    }
    catch {
        table = null;
    }
    cache.set(file, table);
    return table;
}

/** Model ids the builtin template of \`templateId\` declares. */
export function builtinModelIdsFor(templateId) {
    if (!templateId)
        return [];
    const table = loadTable();
    const templates = table?.config?.providerConfigRules?.templateRules;
    if (!Array.isArray(templates))
        return [];
    const hit = templates.find((t) => t?.templateId === templateId);
    const ids = hit?.config?.builtinModelIds;
    return Array.isArray(ids) ? ids.filter((id) => typeof id === "string" && id) : [];
}

/**
 * Context window the builtin \`modelRules\` assign to \`modelId\`.
 *
 * The table is ordered general → specific and later rules refine earlier ones
 * (rule properties are merged, not replaced: \`.*glm-5.3-flash\` carries no
 * window and must not clear the 1M set by \`.*glm-5.3\`), so walk every rule and
 * keep the last one that declares a positive window. Matching is anchored and
 * case-insensitive — the table spells rules in lowercase while its own model
 * ids are mixed case ("GLM-5.3"), so a case-sensitive read would silently fall
 * back to the 200K \`.*\` rule.
 */
export function builtinContextWindowFor(modelId) {
    const table = loadTable();
    const rules = table?.config?.modelConfigRules?.modelRules;
    if (!Array.isArray(rules) || !modelId)
        return 0;
    let window = 0;
    for (const rule of rules) {
        const match = rule?.modelMatch;
        if (typeof match !== "string" || !match)
            continue;
        let hit = false;
        try {
            hit = new RegExp(\`^(?:\${match})$\`, "i").test(modelId);
        }
        catch {
            hit = false;
        }
        if (!hit)
            continue;
        const value = rule?.config?.properties?.contextWindow;
        if (typeof value === "number" && value > 0)
            window = value;
    }
    return window;
}
`;

const LOOP_BEFORE = `        const providerName = rule.providerName ?? pid;
        for (const modelId of personal.modelsByProvider.get(pid) ?? []) {
            out.push({ providerId: pid, providerName, modelId });
        }`;

const LOOP_AFTER = `        const providerName = rule.providerName ?? pid;
        // providerModelRules are authoritative when present (the desktop writes
        // one per model the user configured). Providers created from a builtin
        // template carry none — the desktop renders them from the template — so
        // without the fallback they contribute zero models and vanish from the
        // dropdown (2026-10-23: a keyed BigModel Coding Plan provider was
        // selectable in the desktop, absent here).
        let ids = personal.modelsByProvider.get(pid) ?? [];
        if (ids.length === 0) {
            const declared = Array.isArray(rule.config?.personalModelIds)
                ? rule.config.personalModelIds.filter((id) => typeof id === "string" && id)
                : [];
            ids = declared.length > 0
                ? declared
                : builtinModelIdsFor(rule.templateId ?? pid);
        }
        for (const modelId of ids) {
            out.push({ providerId: pid, providerName, modelId });
        }`;

const CONTEXT_BEFORE = `    return personalModelSpec(pid, modelId)?.contextWindow ?? 0;`;

const CONTEXT_AFTER = `    return personalModelSpec(pid, modelId)?.contextWindow ?? builtinContextWindowFor(modelId);`;

const IMPORT_ANCHOR = 'import { configProviderIdFor } from "./account-provider.js";';
const IMPORT_LINE = 'import { builtinContextWindowFor, builtinModelIdsFor } from "./builtin-models.js";';
/** Any earlier spelling of the helper import (kept for self-healing upgrades). */
const IMPORT_PATTERN = /^import \{[^}]*\} from "\.\/builtin-models\.js";[ \t]*\r?\n/gm;

/** Every installed copy of the adapter we can see. */
function adapterRoots() {
    const roots = new Set();
    const appData = process.env.APPDATA;
    if (appData)
        roots.add(path.join(appData, "npm", "node_modules", "zcode-acp-server"));
    const local = process.env.LOCALAPPDATA;
    if (local) {
        const npxRoot = path.join(local, "npm-cache", "_npx");
        let entries = [];
        try {
            entries = readdirSync(npxRoot);
        }
        catch {
            entries = [];
        }
        for (const entry of entries) {
            roots.add(path.join(npxRoot, entry, "node_modules", "zcode-acp-server"));
        }
    }
    // Local project installs (dev checkouts running `npm i zcode-acp-server`).
    roots.add(path.join(process.cwd(), "node_modules", "zcode-acp-server"));
    roots.add(path.join(os.homedir(), ".npm-global", "lib", "node_modules", "zcode-acp-server"));
    return [...roots].filter((root) => existsSync(path.join(root, "dist", "config", "options.js")));
}

function patchOne(root) {
    const optionsPath = path.join(root, "dist", "config", "options.js");
    const modulePath = path.join(root, "dist", "config", "builtin-models.js");
    const source = readFileSync(optionsPath, "utf8");
    const steps = [];

    let patched = source;
    // Exactly one helper import, spelled canonically. An earlier run of this
    // script may have inserted a narrower import; two imports of the same
    // binding is a module-level SyntaxError (ESM redeclaration) that kills the
    // adapter, so drop every variant before re-inserting.
    const importLines = patched.match(/^import \{[^}]*\} from "\.\/builtin-models\.js";/gm) ?? [];
    const importCanonical = importLines.length === 1 && importLines[0] === IMPORT_LINE;
    if (!importCanonical) {
        patched = patched.replace(IMPORT_PATTERN, "");
        if (!patched.includes(IMPORT_ANCHOR)) {
            return "SKIP: import anchor not found (adapter changed?)";
        }
        patched = patched.replace(IMPORT_ANCHOR, `${IMPORT_ANCHOR}\n${IMPORT_LINE}`);
        steps.push("import");
    }
    if (!patched.includes("builtinModelIdsFor(rule.templateId ?? pid)")) {
        if (!patched.includes(LOOP_BEFORE)) {
            return "SKIP: appendPersonalOnlyProviders shape not recognised (adapter changed?)";
        }
        patched = patched.replace(LOOP_BEFORE, LOOP_AFTER);
        steps.push("model-list");
    }
    if (!patched.includes("builtinContextWindowFor(modelId)")) {
        if (!patched.includes(CONTEXT_BEFORE)) {
            return "SKIP: modelContextWindow shape not recognised (adapter changed?)";
        }
        patched = patched.replace(CONTEXT_BEFORE, CONTEXT_AFTER);
        steps.push("context-window");
    }

    const moduleExists = existsSync(modulePath);
    const moduleCurrent = moduleExists && readFileSync(modulePath, "utf8") === BUILTIN_MODELS_MODULE;
    if (!moduleCurrent) {
        steps.push(moduleExists ? "module-refresh" : "module");
    }
    if (steps.length === 0) {
        return "already patched";
    }
    if (dryRun) {
        return `would patch: ${steps.join(", ")}`;
    }
    if (!moduleCurrent) {
        writeFileSync(modulePath, BUILTIN_MODELS_MODULE, "utf8");
    }
    if (patched !== source) {
        writeFileSync(`${optionsPath}.marionette.bak`, source, "utf8");
        writeFileSync(optionsPath, patched, "utf8");
    }
    return `patched: ${steps.join(", ")}`;
}

const roots = adapterRoots();
if (roots.length === 0) {
    console.error("zcode-acp-server not found (global npm, npx cache, or ./node_modules)");
    process.exit(1);
}
for (const root of roots) {
    let result;
    try {
        result = patchOne(root);
    }
    catch (error) {
        result = `ERROR: ${error instanceof Error ? error.message : String(error)}`;
    }
    console.log(`${result.padEnd(46)} ${root}`);
}
