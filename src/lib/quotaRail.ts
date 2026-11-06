import { t } from "./i18n";
import { isBridgeNativeModelId } from "./errors";
import type { UsageWindow } from "./types";

/**
 * Right-rail quota model (docs/ui-language.md v2「右栏 · 额度」).
 *
 * The highlighted block is "who is paying for THIS turn": the agent's own
 * subscription for its own models, the matching source for a model that comes
 * through the model-sharing bridge, and the agent's subscription again (marked
 * "source unknown") when a bridged id cannot be matched. Pure functions only —
 * the component just renders what `buildQuotaView` returns.
 */

export type UsageTone = "ok" | "warn" | "hot" | "none";

export type UsageGroup = {
  key: string;
  name: string;
  windows: UsageWindow[];
  /** Highest used % across the source's windows. */
  peak: number | null;
  tone: UsageTone;
};

export function meterTone(window: UsageWindow): UsageTone {
  if (window.percentage == null) return "none";
  if (window.kind === "cost") return "none";
  if (window.percentage >= 90) return "hot";
  if (window.percentage >= 75) return "warn";
  return "ok";
}

/**
 * Which source a window belongs to. Engine rows carry it in the id
 * (`engine:<provider>:<window>`); the dialog's own agent reports its limits as
 * bare `5-hour limit` / `codex Weekly limit`, so those group under the agent.
 */
export function usageSource(window: UsageWindow, agentLabel: string): { key: string; name: string } {
  if (window.id.startsWith("engine:")) {
    const name = window.label.split(" · ")[0] || window.label;
    return { key: window.id.split(":")[1] ?? name, name };
  }
  if (window.kind === "rate_limit") return { key: "agent", name: agentLabel || t("context.currentAgent") };
  const name = window.label.split(" · ")[0] || window.label;
  return { key: `label:${name}`, name };
}

/**
 * The window's own part of the label: `7 days`, `MCP · Month`; an agent's
 * `codex 5-hour limit` reads `5 hours`, `Weekly (Opus)` reads `Weekly · Opus`.
 */
export function usagePeriod(window: UsageWindow, source: string): string {
  const label = window.label;
  if (label.startsWith(`${source} · `)) return label.slice(source.length + 3);
  if (window.kind !== "rate_limit" || window.id.startsWith("engine:")) return label;
  return (
    label
      .replace(/^(?:codex|chatgpt|claude|grok)\s+/i, "")
      .replace(/\s*limit$/i, "")
      .replace(/^(\d+)[\s-]*h(?:ours?)?$/i, (_, n: string) => `${n} hours`)
      .replace(/\s*\(([^()]+)\)\s*$/, " · $1")
      .trim() || label
  );
}

/** Localised short name for the common window lengths; anything else stays as reported. */
export function periodLabel(period: string): string {
  const p = period.trim();
  let m = /^(\d+(?:\.\d+)?)\s*[- ]?h(?:ours?)?$/i.exec(p);
  if (m) return t("rail.periodHours", { n: m[1]! });
  m = /^(\d+(?:\.\d+)?)\s*[- ]?d(?:ays?)?$/i.exec(p);
  if (m) return t("rail.periodDays", { n: m[1]! });
  if (/^(?:weekly|week|7 days?)$/i.test(p)) return t("rail.periodWeek");
  if (/^(?:monthly|month|30 days?)$/i.test(p)) return t("rail.periodMonth");
  return p;
}

/** Rough window length in hours for ordering; unknown keeps its place. */
export function periodHours(period: string): number | null {
  const p = period.toLowerCase();
  const n = Number(/(\d+(?:\.\d+)?)/.exec(p)?.[1] ?? 1);
  if (/hour|小时/.test(p)) return n;
  if (/day|天/.test(p)) return n * 24;
  if (/week|周/.test(p)) return n * 24 * 7;
  if (/month|月/.test(p)) return 24 * 31;
  return null;
}

const MONEY_RE = /^(?:[¥$€£]\s*[\d,]+(?:\.\d+)?|[\d,]+(?:\.\d+)?\s*(?:CNY|USD|RMB|\$))/i;

/** `¥71.85 total · topped-up …` → `¥71.85`; null when the row carries no money. */
export function windowBalance(window: UsageWindow): string | null {
  if (window.percentage != null) return null;
  const text = window.detail ?? "";
  // `余额 ¥28.30` / `Balance ¥28.30` (usage.balance) → strip the label first.
  const stripped = text.replace(/^(?:余额|Balance)\s*/i, "");
  const money = MONEY_RE.exec(stripped);
  return money ? money[0].replace(/\s+/g, " ").trim() : null;
}

/** A window earns a row only when it has a percentage or a balance — no N/A lines. */
export function isMeaningfulWindow(window: UsageWindow): boolean {
  if (window.kind === "context" || window.kind === "cost" || window.kind === "tokens") return false;
  return window.percentage != null || windowBalance(window) != null;
}

export function groupUsageBySource(windows: UsageWindow[], agentLabel: string): UsageGroup[] {
  const groups: UsageGroup[] = [];
  const byKey = new Map<string, UsageGroup>();
  for (const window of windows) {
    const { key, name } = usageSource(window, agentLabel);
    let group = byKey.get(key);
    if (!group) {
      group = { key, name, windows: [], peak: null, tone: "none" };
      byKey.set(key, group);
      groups.push(group);
    }
    group.windows.push(window);
  }
  for (const group of groups) {
    const order = group.windows.map((w, i) => ({ w, i, h: periodHours(usagePeriod(w, group.name)) }));
    order.sort((x, y) => (x.h != null && y.h != null ? x.h - y.h : x.i - y.i));
    group.windows = order.map((entry) => entry.w);
    const percents = group.windows
      .filter((w) => w.percentage != null && w.kind !== "cost")
      .map((w) => w.percentage as number);
    group.peak = percents.length > 0 ? Math.max(...percents) : null;
    group.tone = group.peak == null ? "none" : group.peak >= 90 ? "hot" : group.peak >= 75 ? "warn" : "ok";
  }
  return groups;
}

/** Lower-case, non-alphanumerics → `-` (the engine's provider-id slug rule). */
function slug(value: string): string {
  return value.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
}

/**
 * Bridge model ids: grok/opencode/pi `marionette/<provider>/<model>`, DSH
 * `marionette::<provider>/<model>`, zcode `marionette\<provider>/<model>`.
 * Returns the engine source (provider) segment, or null when the id has none.
 */
export function bridgeSourceOf(modelId: string | null | undefined): string | null {
  const id = modelId?.trim() ?? "";
  const rest = id.startsWith("marionette::")
    ? id.slice("marionette::".length)
    : id.startsWith("marionette/") || id.startsWith("marionette\\")
      ? id.slice("marionette/".length)
      : null;
  if (rest == null) return null;
  const slash = rest.search(/[\\/]/);
  if (slash <= 0) return null;
  return rest.slice(0, slash) || null;
}

export type QuotaPrimary = {
  group: UsageGroup;
  /** Billing source is the agent's own subscription. */
  own: boolean;
  /** Bridged model whose source could not be matched → showed the agent's plan. */
  unknown: boolean;
};

export type QuotaView = {
  primary: QuotaPrimary | null;
  /** Everything else, one row each (the primary source is not repeated). */
  others: UsageGroup[];
};

/** The agent's own (non-engine) subscription / provider group, if it reported any. */
function ownGroup(groups: UsageGroup[]): UsageGroup | null {
  const agent = groups.find((g) => g.key === "agent");
  if (agent) return agent;
  // OpenCode & co. report their provider's balance/limits under the provider's name.
  return groups.find((g) => g.key.startsWith("label:")) ?? null;
}

export function buildQuotaView(args: {
  windows: UsageWindow[];
  agentLabel: string;
  modelId?: string | null;
}): QuotaView {
  const { windows, agentLabel, modelId } = args;
  const groups = groupUsageBySource(windows.filter(isMeaningfulWindow), agentLabel);
  const own = ownGroup(groups);
  const bridged = isBridgeNativeModelId(modelId);

  let primary: QuotaPrimary | null = null;
  if (bridged) {
    const source = bridgeSourceOf(modelId);
    const wanted = source ? slug(source) : "";
    const hit = wanted
      ? groups.find((g) => g !== own && (slug(g.key) === wanted || slug(g.name) === wanted))
      : undefined;
    if (hit) primary = { group: hit, own: false, unknown: false };
    else if (own) primary = { group: own, own: true, unknown: true };
  } else if (own) {
    primary = { group: own, own: true, unknown: false };
  }

  const others = groups.filter((g) => g !== primary?.group);
  return { primary, others };
}

/** The tightest window: highest used %, the shorter window winning a tie. */
export function tightestWindow(group: UsageGroup): UsageWindow | null {
  let best: UsageWindow | null = null;
  for (const w of group.windows) {
    if (w.percentage == null) continue;
    if (best == null || w.percentage > (best.percentage as number)) best = w;
  }
  return best;
}

/** Reset hint pulled out of a window's detail (`3/4 12:00 重置`, `resets …`). */
export function resetHint(window: UsageWindow | null): string | null {
  const detail = window?.detail;
  if (!detail) return null;
  const part = detail
    .split(" · ")
    .map((s) => s.trim())
    .find((s) => /重置|resets?\b/i.test(s));
  if (!part) return null;
  // Sources word this differently ("resets 11月7日 21:09", "10/30 00:07 重置"): keep only
  // the moment, so the rail prints one short "11/7 21:09" form through rail.resetAt.
  const cn = part.match(/(\d{1,2})\s*月\s*(\d{1,2})\s*日\s*(\d{1,2}:\d{2})?/);
  if (cn) return [`${Number(cn[1])}/${Number(cn[2])}`, cn[3]].filter(Boolean).join(" ");
  const slash = part.match(/(\d{1,2})\/(\d{1,2})(?:\s+(\d{1,2}:\d{2}))?/);
  if (slash) return [`${Number(slash[1])}/${Number(slash[2])}`, slash[3]].filter(Boolean).join(" ");
  const time = part.match(/\b(\d{1,2}:\d{2}(?:\s*[AP]M)?)\b/i);
  if (time?.[1]) return time[1];
  return part.replace(/resets?(\s+at)?|重置于?/gi, "").trim() || null;
}

/** Colour class for a percentage: ≥95 red, ≥80 yellow, otherwise neutral / ink. */
export function pctLevel(pct: number | null | undefined): "hot" | "warn" | "ok" {
  if (pct == null) return "ok";
  if (pct >= 95) return "hot";
  if (pct >= 80) return "warn";
  return "ok";
}

export function formatPct(pct: number): string {
  return `${Math.round(pct * 10) / 10}%`;
}

/** Money of a balance-only group (first window that carries one). */
export function groupBalance(group: UsageGroup): string | null {
  for (const w of group.windows) {
    const b = windowBalance(w);
    if (b) return b;
  }
  return null;
}
