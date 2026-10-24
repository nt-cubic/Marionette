import { useEffect, useState } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import { isTauriRuntime, type BridgeModel, type BridgeModelsResult } from "./api";
import { bridgePresetLabel, engineProviderLabel, invalidateBridgeModels } from "./bridge";
import { modelProviderGroup } from "./modelLabel";
import type { ModelDef } from "./types";

export type SourceModel = { id: string; label: string; nativeIds: Record<string, string[]> };
/** A provider the agent already lists itself that is the same account as a source. */
export type NativeAccountRef = { provider: string; label: string };
export type ModelSource = { id: string; label: string; endpoint: string; protocol: string; origins: string[]; models: SourceModel[]; issue: string | null; missingEnv?: string[]; nativeAccounts?: Record<string, NativeAccountRef[]> };
/**
 * One subscription sign-in. `owner` names the agent whose catalogue serves the
 * account and the native provider it lists the account under (`""` when that
 * agent's whole catalogue is the login). `duplicate` marks a second agent
 * holding a copy of the same sign-in, whose listing must not appear twice.
 */
export type EngineAccount = { engine: string; agent: string; provider: string; kind: "owner" | "duplicate" };
export type SourceScan = { sources: ModelSource[]; warnings: string[]; paths: { path: string; status: string; detail: string }[]; aliases: Record<string, string>; accounts?: EngineAccount[] };
export type ModelLibraryPreferences = {
  hiddenProviders: string[];
  hiddenModels: string[];
  providerOrder: string[];
  modelOrder: Record<string, string[]>;
};
export type LibraryRow = {
  key: string;
  label: string;
  modelId: string;
  native?: ModelDef;
  bridge?: BridgeModel;
  origin?: string;
  /**
   * Former identities of this row. A native model folded into a discovered
   * source keeps the key it had before the merge, so hide settings saved
   * earlier keep applying to the same model.
   */
  legacyKeys?: string[];
  /**
   * Provider groups this row was listed under before the merge. Hiding that
   * provider earlier must keep the row hidden instead of resurfacing it.
   */
  legacyProviders?: string[];
};
export type LibraryGroup = { key: string; label: string; rows: LibraryRow[] };
export const DEFAULT_MODEL_PREFERENCES: ModelLibraryPreferences = { hiddenProviders: [], hiddenModels: [], providerOrder: [], modelOrder: {} };
const EVENT = "model-library-changed";
const SOURCES_EVENT = "model-library-sources-changed";
let preferences: ModelLibraryPreferences = DEFAULT_MODEL_PREFERENCES;
let scan: SourceScan = { sources: [], warnings: [], paths: [], aliases: {} };
let scanPromise: Promise<SourceScan> | null = null;
let scannedAt = 0;
const subscribers = new Set<() => void>();
let loaded: Promise<void> | null = null;

function normalize(value: Partial<ModelLibraryPreferences>): ModelLibraryPreferences {
  const strings = (v: unknown): string[] => Array.isArray(v) ? v.filter((x): x is string => typeof x === "string") : [];
  return {
    hiddenProviders: strings(value.hiddenProviders), hiddenModels: strings(value.hiddenModels), providerOrder: strings(value.providerOrder),
    modelOrder: Object.fromEntries(Object.entries(value.modelOrder ?? {}).map(([key, list]) => [key, strings(list)])),
  };
}
const notify = () => subscribers.forEach(fn => fn());

export async function refreshModelSources(): Promise<SourceScan> {
  if (!isTauriRuntime()) return scan;
  scanPromise = invoke<SourceScan>("model_library_scan");
  try {
    scan = await scanPromise; scannedAt = Date.now(); invalidateBridgeModels(); notify();
    await emit(SOURCES_EVENT, scan);
    return scan;
  }
  catch (error) { scanPromise = null; throw error; }
}

export async function ensureModelSources(): Promise<SourceScan> {
  if (scanPromise && Date.now() - scannedAt < 15_000) return scanPromise;
  return refreshModelSources();
}

async function load(): Promise<void> {
  if (!isTauriRuntime()) return;
  preferences = normalize(await invoke<Partial<ModelLibraryPreferences>>("model_library_preferences"));
  notify();
}

export async function saveModelPreferences(next: ModelLibraryPreferences): Promise<void> {
  if (isTauriRuntime()) await invoke("model_library_save_preferences", { preferences: next });
  preferences = normalize(next); notify();
  if (isTauriRuntime()) await emit(EVENT);
}

export function useModelLibrary(scanOnMount = true) {
  const [, update] = useState(0);
  const [error, setError] = useState("");
  useEffect(() => {
    let active = true;
    const changed = () => { if (active) update(n => n + 1); };
    subscribers.add(changed);
    loaded ??= load().catch(e => { loaded = null; throw e; });
    void loaded.catch(e => { if (active) setError(String(e)); });
    if (scanOnMount && !scanPromise) void refreshModelSources().catch(e => { if (active) setError(String(e)); });
    const disposers: (() => void)[] = [];
    if (isTauriRuntime()) {
      void listen(EVENT, () => { void load().catch(e => { if (active) setError(String(e)); }); }).then(fn => { if (active) disposers.push(fn); else fn(); });
      void listen<SourceScan>(SOURCES_EVENT, event => {
        // Own-window emits already share this object. Other windows receive
        // fresh, secret-free source metadata and drop stale menu catalogs.
        if (JSON.stringify(scan) === JSON.stringify(event.payload)) return;
        scan = event.payload; scannedAt = Date.now(); invalidateBridgeModels(); notify();
      }).then(fn => { if (active) disposers.push(fn); else fn(); });
    }
    return () => { active = false; subscribers.delete(changed); disposers.forEach(fn => fn()); };
  }, [scanOnMount]);
  return { preferences, sources: scan.sources, warnings: scan.warnings, paths: scan.paths ?? [], aliases: scan.aliases ?? {}, accounts: scan.accounts ?? [], error };
}

export function ordered<T>(items: T[], order: string[], key: (item: T) => string, label: (item: T) => string): T[] {
  const rank = new Map(order.map((id, i) => [id, i]));
  return items.slice().sort((a,b) => (rank.get(key(a)) ?? Infinity) - (rank.get(key(b)) ?? Infinity) || label(a).localeCompare(label(b)));
}

/**
 * One identity per source/model. Never deduplicate by model name alone.
 *
 * A discovered source can carry `nativeAccounts` (backend proof: same host,
 * same credential) naming providers the agent already has in its own
 * catalogue. Those rows join the source group so one account is listed once,
 * with both discovery locations on the row.
 */
export function libraryGroups(agentId: string, native: ModelDef[], catalog: BridgeModelsResult | null, sources: ModelSource[], aliases: Record<string, string> = {}, accounts: EngineAccount[] = []): LibraryGroup[] {
  const groups = new Map<string, LibraryGroup>();
  // One sign-in is one group. `owner` says this agent's own catalogue *is* an
  // engine provider's account (Codex CLI and the engine's `codex`); `duplicate`
  // says another agent copied that sign-in into its own auth store (OpenCode's
  // OAuth `openai`), whose listing would repeat the same models.
  const owner = accounts.find(a => a.kind === "owner" && a.agent === agentId && !a.provider);
  // A merged group is named the way an agent names the vendor; a bare host
  // ("deepseek.com") only stands in when no agent lists that account. The name
  // comes from any agent, so every agent's menu shows the same header.
  const vendorLabels = new Map<string, string>();
  for (const source of sources) {
    const vendor = Object.values(source.nativeAccounts ?? {}).flat()[0]?.label?.trim();
    if (vendor) vendorLabels.set(source.id, vendor);
  }
  const sourceLabel = (source: ModelSource | undefined, fallback: string) =>
    (source ? vendorLabels.get(source.id) ?? source.label : fallback);
  for (const source of catalog?.sources ?? []) {
    if (source.entryId !== "engine") { const key=aliases[`entry:${source.entryId}`] ?? `entry:${source.entryId}`; groups.set(key, { key, label: source.label, rows: [] }); }
  }
  const put = (key: string, label: string, row: LibraryRow) => {
    const group = groups.get(key) ?? { key, label, rows: [] };
    const existing = group.rows.findIndex(r => r.key === row.key);
    if (existing < 0) group.rows.push(row);
    else if (row.native) group.rows[existing] = row;
    groups.set(key, group);
  };
  for (const m of catalog?.models ?? []) {
    const provider = m.entryId === "engine" ? m.modelId.split("/")[0]! : m.entryId;
    const rawKey = m.entryId === "engine" ? `engine:${provider}` : `entry:${provider}`;
    const key = aliases[rawKey] ?? rawKey;
    const source = sources.find(s => s.id === provider);
    const modelId = m.entryId === "engine" ? m.modelId.slice(provider.length + 1) : m.modelId;
    const configured = source?.models.find(row => row.id === modelId);
    const sourceLabelFromCatalog = catalog?.sources?.find(s => s.entryId === m.entryId)?.label;
    put(key, sourceLabel(source, m.entryId === "engine" ? engineProviderLabel(provider) : sourceLabelFromCatalog ?? bridgePresetLabel(m.preset)), {
      key: `${key}/${modelId}`, label: configured?.label || modelId, modelId, bridge: m, origin: source?.origins.join("\n"),
    });
  }
  // Include discovered definitions even while their gateway is stopped or the
  // key is unavailable. The configuration page can still manage their rows.
  for (const source of sources) {
    const key = aliases[`engine:${source.id}`] ?? `engine:${source.id}`;
    for (const m of source.models) {
      const nativeId = m.nativeIds[agentId]?.find(id => native.some(n => n.id === id));
      const existing = groups.get(key)?.rows.some(row => row.key === `${key}/${m.id}`);
      if (!existing && !nativeId) continue;
      if (nativeId) {
        const model = native.find(n => n.id === nativeId)!;
        put(key, sourceLabel(source, source.label), { key: `${key}/${m.id}`, label: m.label || model.label, modelId: m.id, native: model, origin: source.origins.join("\n") });
      }
    }
  }
  // A copied sign-in is left out only while the account is already on screen
  // through the engine group that runs it. With the engine down that group
  // holds no rows, and the copy is the only route left to the account.
  const duplicates = new Set(accounts
    .filter(a => a.kind === "duplicate" && a.agent === agentId)
    .filter(a => (groups.get(aliases[`engine:${a.engine}`] ?? `engine:${a.engine}`)?.rows.length ?? 0) > 0)
    .map(a => accountName(a.provider)));
  for (const m of native) {
    if (m.id.startsWith("marionette/") || m.id.startsWith("marionette::")) continue;
    if ([...groups.values()].some(g => g.rows.some(r => r.native?.id === m.id))) continue;
    if (sources.some(s => s.models.some(row => row.nativeIds[agentId]?.includes(m.id)))) continue;
    const label = modelProviderGroup(m);
    // A copied sign-in would list the same models a second time.
    if (duplicates.has(accountName(nativeProviderPrefix(m.id))) || duplicates.has(accountName(label))) continue;
    // This agent's catalogue runs on an engine provider's sign-in: its rows
    // belong to that provider's group, so one login keeps one header. The
    // native id and header join the row's old identities, so a model hidden
    // under `native:codex:OpenAI` stays hidden after the merge.
    if (owner) {
      const key = aliases[`engine:${owner.engine}`] ?? `engine:${owner.engine}`;
      const group = groups.get(key);
      if (group) {
        const local = nativeProviderPrefix(m.id) ? m.id.slice(m.id.indexOf("/") + 1) : m.id;
        const legacyKeys = [`native:${agentId}:${label}/${m.id}`];
        const legacyProviders = [`native:${agentId}:${label}`];
        const twin = group.rows.find(row => row.modelId === local);
        if (twin) {
          twin.native ??= m;
          twin.label = twin.label || m.label || m.id;
          twin.legacyKeys = [...(twin.legacyKeys ?? []), ...legacyKeys];
          twin.legacyProviders = [...(twin.legacyProviders ?? []), ...legacyProviders];
        } else {
          put(key, group.label, { key: `${key}/${local}`, label: m.label || m.id, modelId: local, native: m, legacyKeys, legacyProviders });
        }
        continue;
      }
    }
    // Same account already discovered elsewhere (backend-proved by host + key):
    // its models belong to that source group, not a second header of their own.
    // Match by the id's own provider prefix — group headers merge vendors
    // (`opencode-go/…` reads as "OpenCode"), so the header is only a fallback.
    const account = sourceForNativeAccount(sources, agentId, nativeProviderPrefix(m.id), label);
    if (account) {
      const key = aliases[`engine:${account.id}`] ?? `engine:${account.id}`;
      const group = groups.get(key);
      const local = localModelId(m.id, account, agentId);
      // The engine row and the agent's own row can be the same model: fold the
      // native identity into it so one model keeps one row.
      const twin = group?.rows.find(row => row.bridge != null && row.modelId === local);
      if (twin) {
        twin.native = m;
        twin.label = m.label || twin.label;
        twin.legacyKeys = [...(twin.legacyKeys ?? []), `native:${agentId}:${label}/${m.id}`];
        twin.legacyProviders = [...(twin.legacyProviders ?? []), `native:${agentId}:${label}`];
        continue;
      }
      put(key, sourceLabel(account, account.label), { key: `${key}/${local}`, label: m.label || m.id, modelId: local, native: m, origin: account.origins.join("\n"),
        legacyKeys: [`native:${agentId}:${label}/${m.id}`], legacyProviders: [`native:${agentId}:${label}`] });
      continue;
    }
    const key = `native:${agentId}:${label}`;
    put(key, label, { key: `${key}/${m.id}`, label: m.label || m.id, modelId: m.id, native: m });
  }
  return [...groups.values()];
}

/** Vendor names compare across spellings: `ollama-cloud` ≡ `Ollama Cloud`. */
function accountName(value: string): string {
  return value.toLowerCase().replace(/[^a-z0-9]/g, "");
}

/** Provider segment of a native model id (`opencode-go/deepseek-v4.1-flash`). */
function nativeProviderPrefix(modelId: string): string {
  const id = modelId.trim();
  if (id.includes("::")) return id.split("::")[0]!;
  return id.includes("/") ? id.split("/")[0]! : "";
}

/**
 * Model id inside its source group. Native ids carry the agent's provider
 * prefix (`deepseek/deepseek-flash`); discovered source rows do not.
 */
function localModelId(nativeId: string, account: ModelSource, agentId: string): string {
  for (const ref of account.nativeAccounts?.[agentId] ?? []) {
    const prefix = `${ref.provider}/`;
    if (nativeId.startsWith(prefix)) return nativeId.slice(prefix.length);
  }
  return nativeId.includes("/") ? nativeId.slice(nativeId.lastIndexOf("/") + 1) : nativeId;
}

/**
 * Source that already holds this native provider's account. The backend matches
 * models.dev vendor names against these candidates: the id's provider segment
 * (`deepseek`), its display group header (`DeepSeek`), and vice versa.
 */
function sourceForNativeAccount(sources: ModelSource[], agentId: string, ...candidates: string[]): ModelSource | null {
  const wanted = candidates.map(accountName).filter(Boolean);
  if (wanted.length === 0) return null;
  return sources.find(source => (source.nativeAccounts?.[agentId] ?? []).some(ref => {
    const names = [accountName(ref.label), accountName(ref.provider)].filter(Boolean);
    return names.some(name => wanted.includes(name));
  })) ?? null;
}

/** True when the row is hidden, including keys saved before a merge. */
export function rowHidden(prefs: ModelLibraryPreferences, row: LibraryRow): boolean {
  return prefs.hiddenModels.includes(row.key) || (row.legacyKeys ?? []).some(key => prefs.hiddenModels.includes(key));
}

/** Every identity of a row — the current key plus any it had before a merge. */
export function rowKeys(row: LibraryRow): string[] {
  return [row.key, ...(row.legacyKeys ?? [])];
}

/** Flip one row's visibility. Showing it clears stale pre-merge keys too. */
export function toggleRow(prefs: ModelLibraryPreferences, row: LibraryRow): ModelLibraryPreferences {
  if (rowHidden(prefs, row)) {
    const keys = new Set(rowKeys(row));
    return { ...prefs, hiddenModels: prefs.hiddenModels.filter(key => !keys.has(key)) };
  }
  return { ...prefs, hiddenModels: [...new Set([...prefs.hiddenModels, row.key])] };
}

export function visibleLibraryGroups(groups: LibraryGroup[], prefs: ModelLibraryPreferences, query = ""): LibraryGroup[] {
  const q = query.trim().toLowerCase();
  return ordered(groups.filter(g => !prefs.hiddenProviders.includes(g.key)), prefs.providerOrder, g => g.key, g => g.label)
    .map(g => ({ ...g, rows: ordered(g.rows.filter(r => !rowHidden(prefs, r) && (!q || `${g.label} ${r.label} ${r.modelId} ${r.native?.description ?? ""}`.toLowerCase().includes(q))), prefs.modelOrder[g.key] ?? [], r => r.key, r => r.label) }))
    .filter(g => g.rows.length > 0);
}

/**
 * Convert the former provider hide switch into model selections, and carry
 * settings saved before a merge across: a provider key or row key that no
 * longer exists because its models joined a shared source now points at the
 * merged row. Visibility and order both follow.
 */
export function expandHiddenProviders(prefs: ModelLibraryPreferences, groups: LibraryGroup[]): ModelLibraryPreferences {
  const groupKeys = new Set(groups.map(g => g.key));
  const retired = new Set(prefs.hiddenProviders.filter(key => !groupKeys.has(key)));
  const retiredRows = groups.flatMap(g => g.rows.filter(r => (r.legacyProviders ?? []).some(key => retired.has(key))).map(r => r.key));
  const rowKeyMap = new Map<string, string>();
  const providerKeyMap = new Map<string, string>();
  for (const group of groups) {
    for (const row of group.rows) {
      for (const key of row.legacyKeys ?? []) rowKeyMap.set(key, row.key);
      for (const key of row.legacyProviders ?? []) providerKeyMap.set(key, group.key);
    }
  }
  const rename = (keys: string[]) => [...new Set(keys.map(key => rowKeyMap.get(key) ?? key))];
  const modelOrder: Record<string, string[]> = {};
  for (const [key, list] of Object.entries(prefs.modelOrder)) {
    modelOrder[providerKeyMap.get(key) ?? key] = rename(list);
  }
  return {
    hiddenProviders: prefs.hiddenProviders.filter(key => groupKeys.has(key)),
    hiddenModels: [
      ...new Set([
        ...rename(prefs.hiddenModels),
        ...retiredRows,
        ...groups.filter(g => prefs.hiddenProviders.includes(g.key)).flatMap(g => g.rows.map(r => r.key)),
      ]),
    ],
    providerOrder: [...new Set(prefs.providerOrder.map(key => providerKeyMap.get(key) ?? key))],
    modelOrder,
  };
}

export async function configureSourceEnvironment(sourceId: string, name: string, value: string): Promise<void> {
  await invoke("model_library_set_environment", { sourceId, name, value });
}
