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

/**
 * Engine subscription family that belongs to each agent — the engine row whose
 * provider id matches this is the agent's own subscription (codex session →
 * engine `codex` entry, grok → `grok`, …). Used to (a) keep own-family engine
 * rows as a fallback behind the native probe rows and (b) let `buildQuotaView`
 * promote that engine group to the primary block when the native probe has
 * nothing (2026-11-11: the probe is exactly what fails while a turn is in
 * flight, so the panel used to go blank for the family you're looking at).
 */
export const ENGINE_OWN_FAMILY: Record<string, string> = {
  codex: "codex",
  "codex-acp": "codex",
  "claude-code": "claude",
  claude: "claude",
  "grok-build": "grok",
  grok: "grok",
};

/**
 * Semantic window slot shared by native probe rows and engine rows of the same
 * family: native ids ("five-hour", "weekly", "weekly-opus") and engine window
 * names ("5 hours", "7 days", "MCP · Month") both normalise to "5h"/"weekly"/…,
 * so a native row can claim its slot and suppress the engine fallback copy.
 */
export function quotaSlotKey(text: string): string {
  const s = text.trim().toLowerCase();
  if (!s) return s;
  if (s.startsWith("five-hour")) return "5h";
  if (/^mcp\b/.test(s)) return "mcp";
  // 裸周期词才归槽：`weekly-opus` / `Weekly (Opus)` 是按模型分桶，跟聚合
  // weekly 不是同一个窗口，不能互相顶替。
  const bare = s.replace(/\s*(?:limit|usage|used)\s*$/, "").replace(/\s+/g, " ").trim();
  if (/^(?:weekly|week|7 days?)$/.test(bare)) return "weekly";
  if (/^(?:monthly|month|30 days?)$/.test(bare)) return "monthly";
  const h = /(\d+(?:\.\d+)?)\s*[- ]?\s*(?:h|hour)/.exec(bare);
  if (h) return `${h[1]}h`;
  const d = /(\d+(?:\.\d+)?)\s*[- ]?\s*(?:d|day)/.exec(bare);
  if (d) return Number(d[1]) === 7 ? "weekly" : `${d[1]}d`;
  const m = /(\d+(?:\.\d+)?)\s*[- ]?\s*(?:min|minute)/.exec(bare);
  if (m) return `${m[1]}m`;
  return s;
}

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
  return pctLevel(window.percentage);
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
    group.tone = group.peak == null ? "none" : pctLevel(group.peak);
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

/**
 * The agent's own (non-engine) subscription / provider group, if it reported
 * any. Own-family engine rows (engine fallback, `ENGINE_OWN_FAMILY`) count as
 * the agent's own subscription too — without a live probe they are the only
 * own-subscription rows the panel has.
 */
function ownGroup(groups: UsageGroup[], ownFamily?: string): UsageGroup | null {
  const agent = groups.find((g) => g.key === "agent");
  if (agent) return agent;
  if (ownFamily) {
    const family = groups.find((g) => g.key === ownFamily);
    if (family) return family;
  }
  // OpenCode & co. report their provider's balance/limits under the provider's name.
  return groups.find((g) => g.key.startsWith("label:")) ?? null;
}

export function buildQuotaView(args: {
  windows: UsageWindow[];
  agentLabel: string;
  agentId?: string;
  modelId?: string | null;
}): QuotaView {
  const { windows, agentLabel, agentId, modelId } = args;
  const groups = groupUsageBySource(windows.filter(isMeaningfulWindow), agentLabel);
  const own = ownGroup(groups, agentId ? ENGINE_OWN_FAMILY[agentId] : undefined);
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

/**
 * Per-row reset time, as short as it can be: `21:09` when the reset is today,
 * `11/9` (or `11/9 21:09` with `withTime`) otherwise. Anything resetHint could not
 * normalise is returned as-is.
 */
export function compactReset(hint: string | null, now: Date = new Date(), withTime = false): string | null {
  if (!hint) return null;
  const m = /^(\d{1,2})\/(\d{1,2})(?:\s+(\S+))?$/.exec(hint);
  if (!m) return hint;
  const today = Number(m[1]) === now.getMonth() + 1 && Number(m[2]) === now.getDate();
  if (today && m[3]) return m[3];
  return withTime && m[3] ? `${m[1]}/${m[2]} ${m[3]}` : `${m[1]}/${m[2]}`;
}

/**
 * Quota colour bands, shared by every surface that paints a percentage (dials,
 * meter rows, group tone, rail summary). These are *used* percentages, so high is
 * bad: ≥90 means a tenth or less is left, ≥75 is the amber warning line.
 *
 * One source of truth on purpose — the dial used to keep its own 95/80 copy and
 * showed "92% used" in the calm accent colour while the row right under it was
 * already amber.
 */
export const QUOTA_WARN_PCT = 75;
export const QUOTA_HOT_PCT = 90;

/** Credits effectively gone: the dial reads "Full" and the source folds away. */
export const QUOTA_EXHAUSTED_PCT = 99.5;

/** Colour class for a used percentage: ≥90 red, ≥75 yellow, otherwise neutral / ink. */
export function pctLevel(pct: number | null | undefined): "hot" | "warn" | "ok" {
  if (pct == null) return "ok";
  if (pct >= QUOTA_HOT_PCT) return "hot";
  if (pct >= QUOTA_WARN_PCT) return "warn";
  return "ok";
}

export function isExhausted(pct: number | null | undefined): boolean {
  return pct != null && pct >= QUOTA_EXHAUSTED_PCT;
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
