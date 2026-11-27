/**
 * usageOverrides - user-owned adjustments to the Token usage page.
 *
 * Three things live here, all persisted in localStorage so they survive a
 * reload, and all applied by `usageInsights.aggregateInsights`:
 *
 *   1. custom price per million tokens, per model (four buckets). A custom
 *      price is an **estimate**: it replaces the API list price only for the
 *      part of the usage the agent did not itself report. Agent-reported cost
 *      (`repUsd`) stays authoritative and is never overwritten.
 *   2. model aliases: map several raw ids (a gateway prefix, a dated snapshot)
 *      onto one canonical id so they group and price together.
 *   3. a model include/exclude filter for the model breakdown. This is a
 *      display filter only - period totals still describe every model.
 *
 * Nothing here writes to the ledger. Removing an override immediately returns
 * the page to catalogue pricing.
 */

import { rateForModel, type RateIndex, type TokenRate } from "./tokenPrices";

export type CustomPrice = {
  /** USD per 1M tokens; null = this bucket is not priced by the override. */
  input: number | null;
  output: number | null;
  cacheRead: number | null;
  cacheWrite: number | null;
};

export type CustomPriceMap = Record<string, CustomPrice>;
export type ModelAliasMap = Record<string, string>;
export type ModelFilter = { include: string[]; exclude: string[] };

export type UsageOverrides = {
  prices: CustomPriceMap;
  aliases: ModelAliasMap;
  filter: ModelFilter;
};

export const USAGE_CUSTOM_PRICES_KEY = "marionette-usage-custom-prices";
export const USAGE_MODEL_ALIASES_KEY = "marionette-usage-model-aliases";
export const USAGE_MODEL_FILTER_KEY = "marionette-usage-model-filter";

const MAX_MODELS = 300;

export function emptyUsageOverrides(): UsageOverrides {
  return { prices: {}, aliases: {}, filter: { include: [], exclude: [] } };
}

function storage(): Storage | null {
  if (typeof window === "undefined") return null;
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

function readJson(key: string): unknown {
  try {
    const raw = storage()?.getItem(key);
    return raw ? JSON.parse(raw) : null;
  } catch {
    return null;
  }
}

function writeJson(key: string, value: unknown): void {
  try {
    storage()?.setItem(key, JSON.stringify(value));
  } catch {
    /* private mode / quota: the page still works for this mount */
  }
}

function finiteOrNull(value: unknown): number | null {
  return typeof value === "number" && Number.isFinite(value) && value >= 0 ? value : null;
}

function cleanPrice(value: unknown): CustomPrice | null {
  if (!value || typeof value !== "object") return null;
  const row = value as Record<string, unknown>;
  const price: CustomPrice = {
    input: finiteOrNull(row.input),
    output: finiteOrNull(row.output),
    cacheRead: finiteOrNull(row.cacheRead),
    cacheWrite: finiteOrNull(row.cacheWrite),
  };
  return price.input == null && price.output == null && price.cacheRead == null && price.cacheWrite == null
    ? null
    : price;
}

function cleanPrices(value: unknown): CustomPriceMap {
  const out: CustomPriceMap = {};
  if (!value || typeof value !== "object" || Array.isArray(value)) return out;
  let n = 0;
  for (const [modelId, raw] of Object.entries(value as Record<string, unknown>)) {
    const id = modelId.trim();
    if (!id) continue;
    const price = cleanPrice(raw);
    if (!price) continue;
    out[id] = price;
    if (++n >= MAX_MODELS) break;
  }
  return out;
}

function cleanAliases(value: unknown): ModelAliasMap {
  const out: ModelAliasMap = {};
  if (!value || typeof value !== "object" || Array.isArray(value)) return out;
  let n = 0;
  for (const [rawFrom, rawTo] of Object.entries(value as Record<string, unknown>)) {
    const from = rawFrom.trim();
    const to = typeof rawTo === "string" ? rawTo.trim() : "";
    if (!from || !to || from === to) continue;
    out[from] = to;
    if (++n >= MAX_MODELS) break;
  }
  return out;
}

function cleanList(value: unknown): string[] {
  if (!Array.isArray(value)) return [];
  const seen = new Set<string>();
  const out: string[] = [];
  for (const raw of value) {
    if (typeof raw !== "string") continue;
    const id = raw.trim();
    if (!id) continue;
    const key = id.toLowerCase();
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(id);
    if (out.length >= MAX_MODELS) break;
  }
  return out;
}

function cleanFilter(value: unknown): ModelFilter {
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    return { include: [], exclude: [] };
  }
  const row = value as Record<string, unknown>;
  return { include: cleanList(row.include), exclude: cleanList(row.exclude) };
}

export function loadUsageOverrides(): UsageOverrides {
  return {
    prices: cleanPrices(readJson(USAGE_CUSTOM_PRICES_KEY)),
    aliases: cleanAliases(readJson(USAGE_MODEL_ALIASES_KEY)),
    filter: cleanFilter(readJson(USAGE_MODEL_FILTER_KEY)),
  };
}

export function saveUsageOverrides(overrides: UsageOverrides): void {
  writeJson(USAGE_CUSTOM_PRICES_KEY, overrides.prices);
  writeJson(USAGE_MODEL_ALIASES_KEY, overrides.aliases);
  writeJson(USAGE_MODEL_FILTER_KEY, overrides.filter);
}

export function hasAnyOverride(overrides: UsageOverrides): boolean {
  return (
    Object.keys(overrides.prices).length > 0 ||
    Object.keys(overrides.aliases).length > 0 ||
    overrides.filter.include.length > 0 ||
    overrides.filter.exclude.length > 0
  );
}

export function hasCustomPrices(overrides: UsageOverrides): boolean {
  return Object.keys(overrides.prices).length > 0;
}

function modelKeyMatch(a: string, b: string): boolean {
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}

/** Follow the alias chain (one hop is normal, the loop guard is for bad data). */
export function canonicalModelId(modelId: string, aliases: ModelAliasMap): string {
  let current = modelId.trim();
  const seen = new Set<string>([current.toLowerCase()]);
  for (let i = 0; i < 8; i++) {
    const entry = Object.entries(aliases).find(([from]) => modelKeyMatch(from, current));
    const next = entry?.[1]?.trim();
    if (!next || modelKeyMatch(next, current)) break;
    const key = next.toLowerCase();
    if (seen.has(key)) break;
    seen.add(key);
    current = next;
  }
  return current;
}

function lookupPrice(prices: CustomPriceMap, modelId: string): CustomPrice | undefined {
  const id = modelId.trim();
  if (!id) return undefined;
  const direct = prices[id];
  if (direct) return direct;
  for (const [key, value] of Object.entries(prices)) {
    if (modelKeyMatch(key, id)) return value;
  }
  return undefined;
}

/**
 * The custom price for a model id, or null. Raw id wins over the alias target
 * so a user can price one raw snapshot differently from its canonical group.
 */
export function customPriceForModel(modelId: string, overrides: UsageOverrides): TokenRate | null {
  const canonical = canonicalModelId(modelId, overrides.aliases);
  const price =
    lookupPrice(overrides.prices, modelId) ??
    (canonical !== modelId.trim() ? lookupPrice(overrides.prices, canonical) : undefined);
  if (!price) return null;
  if (price.input == null && price.output == null && price.cacheRead == null && price.cacheWrite == null) {
    return null;
  }
  return {
    providerId: "custom",
    id: canonical,
    name: canonical,
    input: price.input,
    output: price.output,
    cacheRead: price.cacheRead,
    cacheWrite: price.cacheWrite,
  };
}

/** Custom price first; otherwise the catalogue, trying the alias target too. */
export function effectiveRateForModel(
  modelId: string,
  prices: RateIndex | null,
  overrides: UsageOverrides,
): { rate: TokenRate | null; custom: boolean } {
  const custom = customPriceForModel(modelId, overrides);
  if (custom) return { rate: custom, custom: true };
  if (!prices) return { rate: null, custom: false };
  const direct = rateForModel(modelId, prices);
  if (direct) return { rate: direct, custom: false };
  const canonical = canonicalModelId(modelId, overrides.aliases);
  if (canonical !== modelId.trim()) {
    return { rate: rateForModel(canonical, prices), custom: false };
  }
  return { rate: null, custom: false };
}

/** Exclude wins; a non-empty include list is a whitelist. Case-insensitive. */
export function modelAllowed(modelId: string, overrides: UsageOverrides): boolean {
  const canonical = canonicalModelId(modelId, overrides.aliases);
  const candidates = [modelId.trim(), canonical].filter(Boolean);
  const matches = (id: string, list: readonly string[]) =>
    list.some((entry) => modelKeyMatch(entry, id));
  if (candidates.some((id) => matches(id, overrides.filter.exclude))) return false;
  if (overrides.filter.include.length === 0) return true;
  return candidates.some((id) => matches(id, overrides.filter.include));
}
